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

use serde::Deserialize;
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

/// Decides which manager owns the project containing `start`, if any.
///
/// The walk stops at the *first* directory carrying any marker and decides there.
/// It does not keep climbing looking for a better answer: a nested project with
/// its own `uv.lock` inside a Poetry monorepo is a `uv` project, and the
/// innermost marker is the one the user is standing in.
pub fn detect(start: &Path) -> Option<PythonProject> {
    let mut current = Some(start);
    while let Some(directory) = current {
        if ROOT_MARKERS
            .iter()
            .any(|marker| directory.join(marker).is_file())
        {
            return Some(PythonProject {
                root: directory.to_path_buf(),
                manager: manager_of(directory),
            });
        }
        current = directory.parent();
    }
    None
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

    #[test]
    fn a_directory_with_no_project_detects_nothing() {
        let temp = project(&[("README.md", "not a python project\n")]);
        assert!(detect(temp.path()).is_none());
    }
}
