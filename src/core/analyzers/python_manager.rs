//! Which Python manager owns a project, and the vocabulary its adapters share.
//!
//! # Why detection is its own step
//!
//! Until #73 there was one Python adapter, so "find the project" and "find `uv`"
//! were the same question and [`crate::core::analyzers::uv::Uv::detect`] answered
//! both. Poetry and `uv` share `pyproject.toml`, so that no longer works: the
//! manager has to be decided from the project's own markers *before* any tool is
//! run, or a Poetry project gets `uv` pointed at it and reports a lockfile error
//! rather than a report.
//!
//! # The rule is deliberately asymmetric
//!
//! `uv` is the incumbent and #73 must not change what it already does, so Poetry
//! is chosen only on *positive* Poetry evidence with *no* `uv` evidence. Every
//! other combination — including a bare `pyproject.toml` with nothing else in it,
//! and a directory carrying both lockfiles — resolves to `uv` exactly as it did
//! before this module existed.
//!
//! That asymmetry is the point. A project with both `uv.lock` and `poetry.lock`
//! is genuinely ambiguous, and the safe answer to an ambiguity is the behaviour
//! that already shipped, not a coin flip that silently reroutes someone's CI.
//!
//! # Evidence, not heuristics
//!
//! Poetry 2.x projects declare their dependencies in PEP 621 `[project]`, so
//! `[tool.poetry]` is no longer reliably present — a modern Poetry project can
//! carry nothing but `[build-system]`, `[project]`, and `poetry.lock`. Two
//! signals are therefore accepted, and each is a fact about the project rather
//! than a guess:
//!
//! - `poetry.lock` exists — only Poetry writes one
//! - `[tool.poetry]` exists — Poetry's own configuration table
//!
//! The `uv` side reads the same way: `uv.lock`, or a `[tool.uv]` table.
//!
//! # Requirements files are a second walk, never a marker
//!
//! A `requirements.txt` project (#76) is found by a *separate* walk that runs
//! only when the first one returns `None`. Folding the requirements files into
//! [`ROOT_MARKERS`] would look equivalent and is not: the first walk stops at the
//! first directory carrying any marker, so a repo with `pyproject.toml` at the
//! root and a stray `requirements.txt` in a subdirectory would start reporting
//! pip-tools the moment someone ran the command from that subdirectory. Running
//! the requirements walk second, and only as a last resort, leaves every existing
//! detection outcome byte-identical.

use serde::Deserialize;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::core::python::{PythonManagerName, PythonUnavailableReason};

/// The outcome of probing one capability.
///
/// Shared by every Python adapter rather than owned by one, because
/// `cli::commands::python` dispatches on the manager and then treats the answer
/// identically whichever adapter produced it.
pub enum Capability {
    Available,
    Unavailable {
        reason: PythonUnavailableReason,
        detail: String,
    },
}

/// A detected Python project: where it is, and which manager owns it.
pub struct PythonProject {
    pub root: PathBuf,
    pub manager: PythonManagerName,
}

/// Files that make a directory a Python project root.
///
/// Broader than the set [`crate::core::analyzers::uv`] walks for, which is `uv`'s
/// own two markers. These are two different questions — "which manager owns this
/// tree" versus "where would `uv` act" — and a `poetry.lock` beside no
/// `pyproject.toml` is a root for the first and not the second.
const ROOT_MARKERS: [&str; 3] = ["pyproject.toml", "uv.lock", "poetry.lock"];

/// Files that make a directory a requirements-file project root.
///
/// Deliberately not part of [`ROOT_MARKERS`] — see the module note on why this is
/// a second walk rather than three more markers.
const REQUIREMENTS_MARKERS: [&str; 2] = ["requirements.txt", "requirements.in"];

/// Why neither capability can be measured for a requirements-file project.
///
/// Visible beyond this module so the doc-example test in
/// [`crate::core::python`] can pin `docs/python-schema.md` to these exact
/// strings — an example that paraphrases the payload is an example that can
/// drift from it. This crate is a binary with no `lib.rs`, so `pub` here is
/// crate-internal reach rather than published surface; the same is true of
/// [`crate::core::analyzers::poetry::SECURITY_UNSUPPORTED_DETAIL`], which is
/// visible for the same reason.
///
/// One wording covers both `pip` and `pip_tools`. The limitation is the same
/// either way, `manager.name` already says which was detected, and naming both
/// tools tells a reader of one gap what the other tool would not have answered
/// either.
pub const REQUIREMENTS_OUTDATED_DETAIL: &str =
    "Neither pip nor pip-tools reports newer versions for a requirements file: `pip list \
     --outdated` describes an installed environment rather than the pinned requirements, and \
     `pip-compile --upgrade` re-resolves the file rather than reporting on it. No install closes \
     this gap — uv or Poetry can answer it for a project they manage.";

/// The security half of the same refusal.
///
/// Written out separately rather than sharing one string with
/// [`REQUIREMENTS_OUTDATED_DETAIL`], because a consumer that reads only the
/// `security` gap has to learn why *security* specifically is missing — the
/// answer is a different fact from the outdated one, and it is the one that
/// decides whether a pipeline needs a scanner bolted on beside this command.
pub const REQUIREMENTS_SECURITY_DETAIL: &str =
    "Neither pip nor pip-tools ships a vulnerability scanner, and `uv audit` requires a \
     pyproject.toml rather than a requirements file, so there is nothing here to scan with. No \
     install closes this gap — uv or Poetry can answer it for a project they manage, and a \
     dedicated scanner can answer it in place.";

/// How far into a `requirements.txt` the `pip-compile` header is looked for.
///
/// A bound rather than a line count, so a pathological file — one enormous line,
/// or a comment block that never ends — cannot pull an arbitrary amount into
/// memory. pip-compile writes its header in the first few lines, so this is
/// generous by a wide margin.
const HEADER_SCAN_BYTES: u64 = 4096;

/// Decides which manager owns the project containing `start`, if any.
///
/// The walk stops at the *first* directory carrying any marker and decides there.
/// It does not keep climbing looking for a better answer: a nested project with
/// its own `uv.lock` inside a Poetry monorepo is a `uv` project, and the
/// innermost marker is the one the user is standing in.
///
/// A requirements-file project is only ever the answer when that walk found
/// nothing at all — see the module note.
pub fn detect(start: &Path) -> Option<PythonProject> {
    if let Some(root) = walk_to_root(start, &ROOT_MARKERS) {
        let manager = manager_of(&root);
        return Some(PythonProject { root, manager });
    }

    walk_to_root(start, &REQUIREMENTS_MARKERS).map(|root| {
        let manager = requirements_manager_of(&root);
        PythonProject { root, manager }
    })
}

/// Climbs from `start` to the filesystem root, stopping at the first directory
/// holding any of `markers`.
fn walk_to_root(start: &Path, markers: &[&str]) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(directory) = current {
        if markers
            .iter()
            .any(|marker| directory.join(marker).is_file())
        {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
}

/// Separates a compiled requirements file from a hand-written one.
///
/// Two signals, and each is a fact about the project rather than a guess: a
/// `requirements.in` is pip-compile's input file and nothing else writes one, and
/// the header pip-compile stamps into its output names the tool outright. Absent
/// both, the honest answer is plain `pip` — a hand-maintained `requirements.txt`
/// is a real and common shape, and filing it under pip-tools would put a tool in
/// `manager.name` that the project does not use.
fn requirements_manager_of(directory: &Path) -> PythonManagerName {
    if directory.join("requirements.in").is_file()
        || has_pip_compile_header(&directory.join("requirements.txt"))
    {
        PythonManagerName::PipTools
    } else {
        PythonManagerName::Pip
    }
}

/// Whether a comment line ahead of any content in `path` names `pip-compile`.
///
/// The real header reads `# This file is autogenerated by pip-compile with Python
/// 3.12`, which has already changed wording across pip-tools releases. Matching
/// the tool name case-insensitively inside a leading `#` line is the part that has
/// been stable, so that is what is matched.
///
/// The scan stops at the first line that is neither blank nor a comment, and a
/// file that cannot be opened or read as UTF-8 simply carries no header. Deciding
/// the manager is not the place to reject a requirements file.
///
/// Two false positives are accepted knowingly, because blank lines are skipped
/// rather than ending the scan and the match is a substring: a `pip-compile`
/// mention in a *later* comment block still counts, and so does a comment saying
/// not to run `pip-compile` on the file. Both cost one wrong word in
/// `manager.name`; the payload is otherwise byte-identical, since neither name
/// promises anything the run then fails to deliver.
fn has_pip_compile_header(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };

    let mut reader = BufReader::new(file.take(HEADER_SCAN_BYTES));
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(comment) = trimmed.strip_prefix('#') else {
            return false;
        };
        if comment.to_ascii_lowercase().contains("pip-compile") {
            return true;
        }
    }
}

/// Reads one directory's markers and names the manager.
///
/// Returns `uv` for everything that is not unambiguously Poetry — see the module
/// note on why the tie goes to the incumbent.
fn manager_of(directory: &Path) -> PythonManagerName {
    let manifest = std::fs::read_to_string(directory.join("pyproject.toml"))
        .ok()
        .and_then(|contents| toml::from_str::<PyProject>(&contents).ok())
        .unwrap_or_default();

    // An unreadable or unparseable `pyproject.toml` contributes no evidence at
    // all rather than failing the run. Deciding the manager is not the place to
    // reject a manifest: the tool that is about to be run will report a syntax
    // error far better than this function could.
    let uv_evidence = directory.join("uv.lock").is_file() || manifest.has_uv_table();

    // A `poetry.*` build backend is deliberately *not* a signal. It says how the
    // project is built, not who manages its dependencies, and a PEP 621 project
    // can build with poetry-core while uv owns its dependencies — a migration
    // mid-flight, or `uv pip compile` into a requirements file with no `uv.lock`.
    //
    // The two misroutings are not symmetric, which is what settles it. A Poetry
    // project sent to uv still gets a report, because uv reads a PEP 621 manifest
    // perfectly well. A uv project sent to Poetry on a machine without Poetry
    // gets nothing: `Poetry::detect` fails the whole run, so a `--json` caller
    // that had a payload with an outdated gap receives an error object carrying
    // no `schema_version` at all.
    let poetry_evidence = directory.join("poetry.lock").is_file() || manifest.has_poetry_table();

    if poetry_evidence && !uv_evidence {
        PythonManagerName::Poetry
    } else {
        PythonManagerName::Uv
    }
}

/// Only the parts of `pyproject.toml` that name a manager.
///
/// Every field is optional and the tables are held as opaque [`toml::Value`]s:
/// presence is the whole signal, and modelling their contents would make an
/// unrelated schema change upstream look like a missing manager.
#[derive(Debug, Default, Deserialize)]
struct PyProject {
    #[serde(default)]
    tool: Option<PyProjectTool>,
}

#[derive(Debug, Deserialize)]
struct PyProjectTool {
    #[serde(default)]
    poetry: Option<toml::Value>,
    #[serde(default)]
    uv: Option<toml::Value>,
}

impl PyProject {
    fn has_poetry_table(&self) -> bool {
        self.tool.as_ref().is_some_and(|tool| tool.poetry.is_some())
    }

    fn has_uv_table(&self) -> bool {
        self.tool.as_ref().is_some_and(|tool| tool.uv.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a project directory from `(relative path, contents)` pairs.
    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp dir");
        for (name, contents) in files {
            let path = temp.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(path, contents).expect("write fixture file");
        }
        temp
    }

    fn manager(files: &[(&str, &str)]) -> PythonManagerName {
        let temp = project(files);
        detect(temp.path()).expect("a project was written").manager
    }

    /// A modern Poetry project declares PEP 621 `[project]` and may carry no
    /// `[tool.poetry]` table at all, so each of the two signals has to stand on
    /// its own.
    #[test]
    fn each_poetry_signal_is_sufficient_on_its_own() {
        assert_eq!(
            manager(&[("poetry.lock", "")]),
            PythonManagerName::Poetry,
            "only Poetry writes a poetry.lock"
        );
        assert_eq!(
            manager(&[("pyproject.toml", "[tool.poetry]\nname = \"x\"\n")]),
            PythonManagerName::Poetry,
            "[tool.poetry] is Poetry's own configuration table"
        );

        // A `poetry.*` build backend on its own is NOT a signal: it names the
        // build system, not the dependency manager, and a uv project mid-migration
        // matches it. See the note in `manager_of` on why the tie goes to uv.
        assert_eq!(
            manager(&[(
                "pyproject.toml",
                "[project]\nname = \"x\"\n\n[build-system]\nbuild-backend = \
                 \"poetry.core.masonry.api\"\n"
            )]),
            PythonManagerName::Uv,
            "a build backend alone must not reroute a project away from uv"
        );
    }

    /// The incumbent keeps every case #73 did not have positive evidence for.
    ///
    /// A bare `pyproject.toml` resolved to `uv` before this module existed, and
    /// rerouting it to Poetry would change the behaviour of projects that never
    /// asked for anything.
    #[test]
    fn anything_short_of_unambiguous_poetry_stays_on_uv() {
        for files in [
            &[("pyproject.toml", "[project]\nname = \"x\"\n")][..],
            &[("uv.lock", "version = 1\n")][..],
            &[("pyproject.toml", "[tool.uv]\n")][..],
            // Both managers configured: genuinely ambiguous, so the answer is the
            // behaviour that already shipped rather than a coin flip that
            // silently reroutes a pipeline.
            &[
                (
                    "pyproject.toml",
                    "[tool.poetry]\nname = \"x\"\n\n[tool.uv]\n",
                ),
                ("poetry.lock", ""),
                ("uv.lock", "version = 1\n"),
            ][..],
        ] {
            assert_eq!(manager(files), PythonManagerName::Uv, "{files:?}");
        }
    }

    /// A manifest this crate cannot parse contributes no evidence rather than
    /// failing the run — the manager that is about to be invoked will explain the
    /// syntax error far better than detection could.
    #[test]
    fn an_unparseable_manifest_is_not_a_detection_failure() {
        assert_eq!(
            manager(&[("pyproject.toml", "this is not toml {{{\n")]),
            PythonManagerName::Uv
        );
        // Unparseable, but the lockfile beside it is still a fact.
        assert_eq!(
            manager(&[
                ("pyproject.toml", "this is not toml {{{\n"),
                ("poetry.lock", "")
            ]),
            PythonManagerName::Poetry
        );
    }

    /// The walk stops at the innermost marker, so a nested project is its own
    /// project rather than inheriting the manager of the tree it sits in.
    #[test]
    fn the_innermost_project_wins() {
        let temp = project(&[
            ("poetry.lock", ""),
            ("pyproject.toml", "[tool.poetry]\nname = \"outer\"\n"),
            ("nested/uv.lock", "version = 1\n"),
        ]);

        let outer = detect(temp.path()).expect("outer project");
        assert_eq!(outer.manager, PythonManagerName::Poetry);
        assert_eq!(outer.root, temp.path());

        let inner = detect(&temp.path().join("nested")).expect("nested project");
        assert_eq!(inner.manager, PythonManagerName::Uv);
        assert_eq!(inner.root, temp.path().join("nested"));
    }

    /// Walking up finds a project from a subdirectory that carries no marker of
    /// its own — including over a bare `pyproject.toml`, which is the uv shape.
    #[test]
    fn the_walk_climbs_to_the_nearest_project_root() {
        let temp = project(&[
            ("pyproject.toml", "[project]\nname = \"x\"\n"),
            ("src/pkg/module.py", ""),
        ]);

        let found = detect(&temp.path().join("src").join("pkg")).expect("walked up to the root");
        assert_eq!(found.root, temp.path());
        assert_eq!(found.manager, PythonManagerName::Uv);
    }

    /// Depends on the filesystem *above* the tempdir, since #76: both walks climb
    /// to the root, so this also requires that no `requirements.txt` or
    /// `requirements.in` exists anywhere above it. On macOS the per-user
    /// `/var/folders/…` tempdir makes that safe; on a Linux runner with a
    /// `/tmp/requirements.txt`, this goes red as a confusing failure here rather
    /// than as a signal about the pip path.
    #[test]
    fn a_directory_with_no_project_detects_nothing() {
        let temp = project(&[("README.md", "not a python project\n")]);
        assert!(detect(temp.path()).is_none());
    }

    /// The header pip-compile stamps into the file it generates, verbatim from
    /// pip-tools 7.6.1 — including the blank-ish `#` lines around it, which the
    /// scan has to walk through rather than stop at.
    const PIP_COMPILE_HEADER: &str = "#\n\
         # This file is autogenerated by pip-compile with Python 3.12\n\
         # by the following command:\n\
         #\n\
         #    pip-compile requirements.in\n\
         #\n";

    /// A `requirements.in` is pip-compile's input file and nothing else writes
    /// one, so its presence settles the question without reading anything.
    #[test]
    fn a_requirements_in_sibling_detects_pip_tools() {
        assert_eq!(
            manager(&[
                ("requirements.in", "requests\n"),
                ("requirements.txt", "requests==2.32.3\n"),
            ]),
            PythonManagerName::PipTools,
            "only pip-compile consumes a requirements.in"
        );

        // The `.in` alone is enough: the compiled output may simply not be
        // committed.
        assert_eq!(
            manager(&[("requirements.in", "requests\n")]),
            PythonManagerName::PipTools
        );
    }

    /// A hand-written `requirements.txt` is plain `pip`, and saying otherwise
    /// would put a tool in `manager.name` that the project does not use.
    #[test]
    fn a_bare_requirements_txt_detects_pip() {
        assert_eq!(
            manager(&[("requirements.txt", "requests==2.32.3\njinja2==3.1.4\n")]),
            PythonManagerName::Pip
        );

        // A comment block that is not pip-compile's stays plain pip. The match is
        // on the tool's name, not on "the file has a header".
        assert_eq!(
            manager(&[(
                "requirements.txt",
                "# production pins, reviewed quarterly\nrequests==2.32.3\n"
            )]),
            PythonManagerName::Pip
        );
    }

    /// The compiled output is often committed without its `.in`, so the header is
    /// the second signal and has to stand on its own.
    #[test]
    fn a_pip_compile_header_detects_pip_tools_without_an_in_file() {
        assert_eq!(
            manager(&[(
                "requirements.txt",
                &format!("{PIP_COMPILE_HEADER}requests==2.32.3\n")
            )]),
            PythonManagerName::PipTools
        );

        // Matched case-insensitively: the surrounding sentence has already changed
        // wording across pip-tools releases, so only the tool name is relied on.
        assert_eq!(
            manager(&[(
                "requirements.txt",
                "# Autogenerated by PIP-COMPILE\nrequests==2.32.3\n"
            )]),
            PythonManagerName::PipTools
        );
    }

    /// The scan covers the leading comment block and stops there.
    ///
    /// A `pip-compile` mention further down the file is a comment about a
    /// package, not evidence the file was generated.
    #[test]
    fn pip_compile_named_below_the_leading_comment_block_is_not_a_header() {
        assert_eq!(
            manager(&[(
                "requirements.txt",
                "requests==2.32.3\n# installed with pip-compile once, years ago\npip-tools==7.6.1\n"
            )]),
            PythonManagerName::Pip,
            "only the leading comment block is a header"
        );
    }

    /// The regression the second walk exists to prevent, in its simplest form.
    ///
    /// A `requirements.txt` beside a `pyproject.toml` is extremely common — an
    /// export, a CI pin, a leftover — and it must not change what the project is.
    ///
    /// This is the weaker of the two guards, and deliberately kept anyway. It
    /// survives the naive break of folding the requirements markers into
    /// [`ROOT_MARKERS`], because `manager_of` on this directory answers `Uv` by
    /// its own rule either way. `a_nested_requirements_txt_does_not_shadow_the_project_above_it`
    /// is what actually catches that one.
    #[test]
    fn a_requirements_txt_beside_a_pyproject_changes_nothing() {
        assert_eq!(
            manager(&[
                ("pyproject.toml", "[project]\nname = \"x\"\n"),
                ("requirements.txt", "requests==2.32.3\n"),
            ]),
            PythonManagerName::Uv,
            "a requirements file beside a manifest does not change the manager"
        );

        // Including when the requirements file is the compiled kind, and when the
        // project is Poetry's rather than uv's.
        assert_eq!(
            manager(&[
                ("pyproject.toml", "[tool.poetry]\nname = \"x\"\n"),
                ("poetry.lock", ""),
                ("requirements.in", "requests\n"),
                (
                    "requirements.txt",
                    &format!("{PIP_COMPILE_HEADER}requests==2.32.3\n")
                ),
            ]),
            PythonManagerName::Poetry
        );
    }

    /// The case that breaks if requirements files join `ROOT_MARKERS`.
    ///
    /// Both walks climb, so a `requirements.txt` in a subdirectory of a
    /// `pyproject.toml` project would be the *innermost* marker and would win the
    /// first walk outright — rerouting a `uv` project to pip-tools for anyone who
    /// happened to run the command from that subdirectory. Ordering the walks is
    /// what makes that impossible, so this asserts the root as well as the
    /// manager. The root assertion is not redundant: against the marker-fold
    /// break specifically, `manager_of` still answers `Uv` for the subdirectory,
    /// so the manager assertion passes and only the root catches it.
    #[test]
    fn a_nested_requirements_txt_does_not_shadow_the_project_above_it() {
        let temp = project(&[
            ("pyproject.toml", "[project]\nname = \"x\"\n"),
            ("deploy/requirements.txt", "requests==2.32.3\n"),
        ]);

        let found = detect(&temp.path().join("deploy")).expect("walked up to the project root");
        assert_eq!(
            found.manager,
            PythonManagerName::Uv,
            "a nested requirements file must not reroute the project above it"
        );
        assert_eq!(
            found.root,
            temp.path(),
            "the project root is the pyproject.toml directory, not the subdirectory"
        );
    }

    /// The requirements walk climbs like the primary one, so running the command
    /// from inside a requirements project still finds it.
    #[test]
    fn the_requirements_walk_climbs_to_its_own_root() {
        let temp = project(&[
            ("requirements.txt", "requests==2.32.3\n"),
            ("src/app.py", ""),
        ]);

        let found = detect(&temp.path().join("src")).expect("walked up to the requirements root");
        assert_eq!(found.manager, PythonManagerName::Pip);
        assert_eq!(found.root, temp.path());
    }
}
