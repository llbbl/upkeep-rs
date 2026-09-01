//! The Poetry adapter: capability detection, invocation, and normalization into
//! the schema in [`crate::core::python`].
//!
//! # What Poetry can and cannot answer
//!
//! `poetry show --latest --format json` emits an array of exactly
//! `{name, installed_status, version, latest_version, description}`. That is the
//! whole payload — there is no `groups`, no `extras`, and no `marker` anywhere in
//! it, so all three are reported as *not reported* rather than as empty. This is
//! the distinction the schema exists to carry: `[]` would claim Poetry looked and
//! found none, and Poetry never looked.
//!
//! **Poetry has no security command.** Its nearest neighbour, `poetry check`,
//! validates `pyproject.toml` against the lockfile; it is not a vulnerability
//! scan. So `security` is [`PythonUnavailableReason::Unsupported`] and not
//! `NotInstalled`: no install closes this gap, and saying otherwise would send a
//! user looking for a tool that does not exist. This adapter is that variant's
//! first caller.
//!
//! # Scope is derived from two invocations
//!
//! Nothing in the JSON says whether a package is a direct dependency, but
//! `--top-level` restricts the listing to direct dependencies, so the same query
//! run twice classifies every entry by set membership. The fixtures pin this: the
//! project declares `six` directly and `six` is up to date, which is what proves
//! `--top-level` lists *direct* dependencies rather than merely fewer outdated
//! ones.
//!
//! When the second invocation fails, every scope becomes
//! [`PythonDependencyScope::Unknown`] and the payload carries a warning. An entry
//! that cannot be *shown* to be direct must not be filed as transitive to fill
//! the field.
//!
//! # Capabilities are probed, never inferred from a version number
//!
//! `--format` on `poetry show` is not in every Poetry release, and neither is
//! `--top-level`. The discipline is the one #72 established for `uv`: run the
//! exact command line this adapter intends to use with the format value replaced
//! by [`CAPABILITY_PROBE_VALUE`], and read what Poetry's own argument parser says
//! back. It costs one process spawn, reaches no network, and cannot be fooled by
//! a version string the way a release-notes lookup table can.

use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use tokio::process::Command;

use crate::core::analyzers::python_manager::Capability;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::pep440;
use crate::core::python::{
    normalize_package_name, PythonDependencyScope, PythonMarker, PythonOutdatedPackage,
    PythonOutdatedReport, PythonUnavailableReason, PythonUpdateCounts, PythonUpdateType,
};

/// Overrides which `poetry` binary is executed.
///
/// The counterpart of `UV_BIN_ENV`, and it exists for the same reason: the
/// capability-gap paths are the ones most worth testing end to end, and they are
/// unreachable on a machine with a current Poetry unless the binary can be
/// pointed elsewhere.
pub const POETRY_BIN_ENV: &str = "UPKEEP_POETRY_BIN";

/// The value handed to `--format` so Poetry enumerates what it accepts.
///
/// It has to be a value Poetry will never add. If it somehow became valid, the
/// probe would run the real command instead of being rejected, which is why
/// [`probe_capability`] treats a *successful* probe as inconclusive rather than
/// as a pass.
const CAPABILITY_PROBE_VALUE: &str = "cargo-upkeep-capability-probe";

/// What to tell a user whose Poetry is too old for a capability.
///
/// `poetry self update` only works for a standalone install, so the
/// package-manager case is named too — Poetry is very often installed through
/// pipx, Homebrew, or mise, where `self update` refuses.
const UPGRADE_HINT: &str =
    "upgrade Poetry (`poetry self update`, or through whichever package manager installed it)";

/// The full listing, with the format value left to the caller.
///
/// `--latest` rather than `--outdated` deliberately. `--outdated` returns only
/// the packages that are behind, which would make `checked` equal `outdated` on
/// every run — a denominator that always equals its numerator is not a
/// denominator, and `docs/python-schema.md` defines `checked` as the distinct
/// packages the freshness question was actually *settled* for. `--latest` settles
/// it for every package and this adapter derives the outdated subset itself, with
/// the same PEP 440 comparison the `uv` adapter uses. Verified against Poetry
/// 2.4.2: the derived subset is exactly what `--outdated` returns.
const SHOW_ARGS: [&str; 3] = ["show", "--latest", "--format"];

/// The same listing restricted to direct dependencies, which is the scope signal.
const TOP_LEVEL_ARGS: [&str; 4] = ["show", "--latest", "--top-level", "--format"];

/// Why `security` is never measured under Poetry.
///
/// Public because `docs/python-schema.md`'s capability-gap example is pinned to
/// this exact string. The example documents Poetry's payload, so it has to be the
/// payload Poetry actually emits rather than a paraphrase that can drift from it.
pub const SECURITY_UNSUPPORTED_DETAIL: &str =
    "Poetry has no vulnerability scanner: `poetry check` validates pyproject.toml against the \
     lockfile and is not a scan. No install closes this gap — run a dedicated scanner and gate on \
     that instead.";

/// A detected Poetry installation and the project it will be run against.
pub struct Poetry {
    binary: OsString,
    project_root: PathBuf,
    version: Option<String>,
}

impl Poetry {
    /// Locates `poetry` for an already-detected project root.
    ///
    /// The root comes from [`crate::core::analyzers::python_manager::detect`],
    /// which is what decided this is a Poetry project in the first place, so this
    /// does not walk the filesystem again. A missing binary is the documented "no
    /// supported Python manager could be detected" exit: there is no report to
    /// stand on.
    pub async fn detect(project_root: PathBuf) -> Result<Self> {
        let binary = std::env::var_os(POETRY_BIN_ENV).unwrap_or_else(|| OsString::from("poetry"));

        let output = Command::new(&binary)
            .arg("--version")
            .current_dir(&project_root)
            .output()
            .await
            .map_err(|err| match err.kind() {
                io::ErrorKind::NotFound => UpkeepError::message(
                    ErrorCode::MissingTool,
                    "no supported Python manager could be detected: this project is a Poetry \
                     project, but poetry is not installed or not on PATH; see \
                     https://python-poetry.org/docs/#installation",
                ),
                _ => UpkeepError::context(
                    ErrorCode::ExternalCommand,
                    "failed to execute poetry --version",
                    err,
                ),
            })?;

        Ok(Self {
            binary,
            project_root,
            version: parse_version(&String::from_utf8_lossy(&output.stdout)),
        })
    }

    /// Poetry's own version string, or `None` when `poetry --version` said
    /// something this crate does not recognize.
    ///
    /// Never used to decide what Poetry can do. It is reported so a human reading
    /// the payload can see what they ran; the capabilities are probed.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    async fn run(&self, args: &[&str]) -> Result<std::process::Output> {
        Command::new(&self.binary)
            .args(args)
            .current_dir(&self.project_root)
            // Inspecting a project must not change it. `poetry show` creates a
            // virtualenv when the project has none — and with
            // `virtualenvs.in-project` set, that means writing a `.venv/`
            // directory into the user's tree. Suppressing creation is not a
            // read-only *flag* on the command; it is what makes the command
            // read-only at all. Verified against Poetry 2.4.2: an existing
            // environment is still used, and every field this adapter reads —
            // `name`, `version`, `latest_version` — is lock-derived and
            // unaffected. Only `installed_status` changes, flipping to
            // `not-installed` when no environment is found, and nothing here
            // reads it.
            .env("POETRY_VIRTUALENVS_CREATE", "false")
            .output()
            .await
            .map_err(|err| {
                UpkeepError::context(
                    ErrorCode::ExternalCommand,
                    format!("failed to execute poetry {}", args.join(" ")),
                    err,
                )
            })
    }

    /// Probes whether `poetry show` can emit the JSON this adapter reads.
    pub async fn probe_outdated(&self) -> Capability {
        let mut argv: Vec<&str> = SHOW_ARGS.to_vec();
        argv.push(CAPABILITY_PROBE_VALUE);

        let output = match self.run(&argv).await {
            Ok(output) => output,
            Err(err) => {
                return Capability::Unavailable {
                    reason: PythonUnavailableReason::Failed,
                    detail: err.to_string(),
                }
            }
        };

        probe_capability(
            &String::from_utf8_lossy(&output.stderr),
            output.status.success(),
            &SHOW_ARGS,
        )
    }

    /// Runs the two listings and normalizes them, returning the report and any
    /// warnings the payload should carry.
    pub async fn outdated(&self) -> Result<(PythonOutdatedReport, Vec<String>)> {
        let all = self.show(&SHOW_ARGS).await?;

        // Scope is a nice-to-have on top of a report that already stands without
        // it, so a failure here degrades rather than fails. `--top-level` is
        // newer than `--format`, so a Poetry that answered the first call can
        // genuinely reject this one.
        let mut warnings = Vec::new();
        let top_level = match self.show(&TOP_LEVEL_ARGS).await {
            Ok(packages) => Some(packages),
            Err(err) => {
                warnings.push(format!(
                    "`poetry show --top-level` did not answer, so no package could be shown to be \
                     a direct dependency and every `scope` is reported as `unknown`: {err}"
                ));
                None
            }
        };

        Ok((normalize_show(&all, top_level.as_deref()), warnings))
    }

    /// Runs one listing and parses it.
    async fn show(&self, args: &[&str]) -> Result<Vec<PoetryPackage>> {
        let mut argv: Vec<&str> = args.to_vec();
        argv.push("json");
        let output = self.run(&argv).await?;

        // Poetry writes its report to stdout and everything else — the
        // suppressed-virtualenv notice included — to stderr, so stdout is the
        // only thing read here.
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Err(external_failure(
                &format!("poetry {}", argv.join(" ")),
                &output,
            ));
        }

        serde_json::from_str(&stdout).map_err(|err| {
            UpkeepError::context(
                ErrorCode::InvalidData,
                format!(
                    "poetry {} did not produce the expected JSON",
                    argv.join(" ")
                ),
                err,
            )
        })
    }
}

/// Turns a failed external run into an error carrying Poetry's own explanation.
///
/// Poetry's failures are usually actionable verbatim — `Error: poetry.lock not
/// found. Run `poetry lock` to create it.` is the common one — so the message is
/// passed through rather than replaced with a summary of it.
fn external_failure(command: &str, output: &std::process::Output) -> UpkeepError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    UpkeepError::message(
        ErrorCode::ExternalCommand,
        match poetry_message(&stderr) {
            Some(message) => format!("{command} failed: {message}"),
            None => format!("{command} failed with no output"),
        },
    )
}

/// Poetry's notice for the virtualenv suppression *this adapter* asks for.
///
/// It is filtered out of every message a user sees, because it is our doing and
/// Poetry prints it to stderr ahead of its own complaint. Caught by running the
/// built binary against a Poetry project with no lockfile: the outdated gap read
/// `poetry show --latest --format json failed: Skipping virtualenv creation, as
/// specified in config file.` — true, entirely our fault, and no help at all to
/// someone whose real problem was a missing `poetry.lock`.
const VIRTUALENV_NOTICE: &str = "Skipping virtualenv creation";

/// The most useful line of Poetry's stderr, or `None` when it said nothing.
///
/// Poetry prefixes its own failures with `Error:`, so that line is preferred over
/// whatever chatter precedes it. The first surviving line is the fallback, for a
/// wording that does not carry the prefix.
fn poetry_message(stderr: &str) -> Option<String> {
    let interesting = || {
        stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(VIRTUALENV_NOTICE))
    };

    // An `Error:` line is self-contained, so it is reported alone. Without one,
    // Poetry spreads the complaint over consecutive lines and the *remedy* is
    // usually the last of them — an empty lockfile yields "The lock file does
    // not have a metadata entry." followed by "Regenerate the lock file with the
    // `poetry lock` command.". Taking the first line drops the half that tells
    // the user what to do, which is the same mistake the notice filter already
    // fixed once in the other direction.
    interesting()
        .find(|line| line.starts_with("Error:"))
        .map(str::to_string)
        .or_else(|| {
            let lines: Vec<&str> = interesting().collect();
            (!lines.is_empty()).then(|| lines.join(" "))
        })
}

/// Extracts the version from `poetry --version` output.
///
/// The line is `Poetry (version 2.4.2)`. Only the version token is kept, and it
/// is never parsed further: the schema reports a manager version verbatim because
/// these are not all PEP 440 versions.
fn parse_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?.trim();
    let inside = line.strip_prefix("Poetry (version ")?.strip_suffix(')')?;
    let version = inside.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// Decides the outdated capability from what Poetry's parser said about the probe.
///
/// Split out from [`Poetry::probe_outdated`] so the classification can be tested
/// against captured stderr without running anything. The wordings matched here
/// were captured from Poetry 2.4.2, not recalled from documentation.
pub fn probe_capability(stderr: &str, succeeded: bool, args: &[&str]) -> Capability {
    // Poetry rejects the sentinel and lists what it would have accepted. That
    // list is the capability answer, straight from the tool.
    if let Some(formats) = supported_formats(stderr) {
        return if formats.iter().any(|format| format == "json") {
            Capability::Available
        } else {
            Capability::Unavailable {
                reason: PythonUnavailableReason::NotInstalled,
                detail: format!(
                    "`poetry show --format` accepts only {} on this Poetry; {UPGRADE_HINT} for \
                     machine-readable output",
                    formats.join(", "),
                ),
            }
        };
    }

    // Every flag the real invocation uses is checked, not just `--format`, so a
    // Poetry that never had `--latest` is reported against the flag it actually
    // rejected rather than against the last one on the line.
    for flag in args.iter().filter(|arg| arg.starts_with("--")) {
        if is_unknown_option(stderr, flag) {
            return Capability::Unavailable {
                reason: PythonUnavailableReason::NotInstalled,
                detail: format!(
                    "`poetry show` on this Poetry does not accept `{flag}`; {UPGRADE_HINT}"
                ),
            };
        }
    }

    if is_unknown_command(stderr, "show") {
        return Capability::Unavailable {
            reason: PythonUnavailableReason::NotInstalled,
            detail: format!("this Poetry has no `show` command; {UPGRADE_HINT}"),
        };
    }

    // A probe Poetry *accepted* means the sentinel was taken as a real format, so
    // nothing was learned and the real command may already have run. Reporting
    // that as available would be a guess.
    let detail = if succeeded {
        format!(
            "could not establish whether `poetry show` supports JSON output: Poetry accepted the \
             probe value `{CAPABILITY_PROBE_VALUE}` instead of rejecting it"
        )
    } else {
        format!(
            "could not establish whether `poetry show` supports JSON output: {}",
            poetry_message(stderr).unwrap_or_else(|| "poetry produced no output".to_string())
        )
    };
    Capability::Unavailable {
        reason: PythonUnavailableReason::Failed,
        detail,
    }
}

/// Reads Poetry's `Supported formats are: …` list.
///
/// Verbatim from Poetry 2.4.2:
/// `Error: Invalid output format. Supported formats are: json, text.`
///
/// The whole leading phrase is required, not just the list, because only this one
/// rejection emits it — Poetry does not quote the offending value back the way
/// clap does, so the phrase is the only thing tying the list to the probe we
/// sent. Whitespace is collapsed first in case the message is ever wrapped.
fn supported_formats(stderr: &str) -> Option<Vec<String>> {
    const PREFIX: &str = "invalid output format. supported formats are:";

    // Search and slice the *same* string. `to_lowercase` is not length
    // preserving — `İ` is two bytes and lowercases to three — so an offset found
    // in a lowercased copy does not address the original. Applied to the
    // original it lands mid-character and panics, or slices a boundary early and
    // silently mangles the list, which reports a current Poetry as too old.
    // Nothing here is lost by lowercasing up front: the values were lowercased
    // on the way out anyway.
    let collapsed = stderr
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let list = collapsed.get(collapsed.find(PREFIX)? + PREFIX.len()..)?;
    let end = list.find('.').unwrap_or(list.len());

    Some(
        list.get(..end)?
            .split(',')
            .map(|format| format.trim().trim_matches(['`', '\'', '"']).to_string())
            .filter(|format| !format.is_empty())
            .collect(),
    )
}

/// Whether Poetry rejected one of our flags.
///
/// Verbatim from Poetry 2.4.2: `The option "--no-such-flag" does not exist`.
///
/// Deliberately *not* added to
/// [`crate::core::analyzers::external_tool::UNKNOWN_FLAG_PATTERNS`]. That list is
/// cargo and clap wording, every entry added to it widens the surface for
/// misreading an unrelated cargo failure, and Poetry's phrasing is the wrong
/// shape for it anyway: the flag name comes *before* the pattern, which the
/// shared matcher rejects by design.
///
/// The flag name is required between `the option "` and `" does not exist` rather
/// than merely somewhere on the line, so that Poetry's very similar
/// `The requested command X does not exist.` cannot be read as an answer about a
/// flag.
fn is_unknown_option(stderr: &str, flag: &str) -> bool {
    let needle = format!("the option \"{}\" does not exist", flag.to_lowercase());
    collapsed_lowercase(stderr).contains(&needle)
}

/// Whether Poetry rejected the subcommand itself.
///
/// Verbatim from Poetry 2.4.2:
/// `The requested command nosuchcmd does not exist.`
fn is_unknown_command(stderr: &str, command: &str) -> bool {
    let needle = format!(
        "the requested command {} does not exist",
        command.to_lowercase()
    );
    collapsed_lowercase(stderr).contains(&needle)
}

/// Poetry wraps its error output to the terminal width, so a message can straddle
/// a line break and has to be matched against one collapsed line.
fn collapsed_lowercase(stderr: &str) -> String {
    stderr
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ===== `poetry show --latest --format json` =====

/// One entry of `poetry show --format json`.
///
/// The real payload also carries `installed_status` and `description`, and
/// neither is modelled. `description` has no field in the schema, and
/// `installed_status` answers "is this in the virtualenv", which is a different
/// question from "is the locked version behind the newest release" — a package
/// Poetry reports as `not-installed` has still had its freshness settled.
///
/// `latest_version` is optional because `poetry show` omits it entirely without
/// `--latest`. Its absence means Poetry did not answer, which is why such an
/// entry is dropped from `checked` rather than counted as up to date.
#[derive(Debug, Deserialize)]
pub struct PoetryPackage {
    name: String,
    version: Option<String>,
    #[serde(default)]
    latest_version: Option<String>,
}

/// Normalizes the two listings into the schema.
///
/// `top_level` is `None` when that invocation did not answer, which makes every
/// scope `unknown` rather than defaulting the whole project to transitive.
pub fn normalize_show(
    all: &[PoetryPackage],
    top_level: Option<&[PoetryPackage]>,
) -> PythonOutdatedReport {
    // An empty second listing beside a non-empty first one is not the claim
    // "nothing is direct" — it is a listing that did not answer, and filing every
    // package as transitive on the strength of it is the same falsehood the
    // `None` branch exists to avoid, reached through the success path instead of
    // the failure one.
    let top_level = top_level.filter(|packages| !packages.is_empty() || all.is_empty());
    let direct: Option<HashSet<String>> = top_level.map(|packages| {
        packages
            .iter()
            .map(|package| normalize_package_name(&package.name))
            .collect()
    });

    // Keyed by normalized name so `checked` counts *distinct* packages, which is
    // what the schema defines it as. Poetry emits a flat array with no key of its
    // own, so nothing upstream guarantees uniqueness.
    let mut settled: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for package in all {
        let (Some(current), Some(latest)) = (
            package.version.as_deref(),
            package.latest_version.as_deref(),
        ) else {
            // Poetry reported no newest release for this package, so the
            // freshness question was not settled and it is not in the
            // denominator. This is the opposite of the `uv` adapter's rule, where
            // an omitted `latest_version` is uv's way of saying "already
            // current" — same field name, different claim.
            continue;
        };
        settled.insert(normalize_package_name(&package.name), (current, latest));
    }

    let mut counts = PythonUpdateCounts {
        epoch: 0,
        major: 0,
        minor: 0,
        patch: 0,
        qualifier: 0,
        unclassified: 0,
    };
    let mut packages = Vec::new();

    for (name, (current, latest)) in &settled {
        // PEP 440 equality, not string equality: `1.0` and `1.0.0` are the same
        // version, and reporting one as an update available for the other would
        // put a package in `outdated` that nobody can act on.
        if pep440::is_same_version(current, latest) {
            continue;
        }

        let update_type = pep440::classify(current, latest);
        match update_type {
            PythonUpdateType::Epoch => counts.epoch += 1,
            PythonUpdateType::Major => counts.major += 1,
            PythonUpdateType::Minor => counts.minor += 1,
            PythonUpdateType::Patch => counts.patch += 1,
            PythonUpdateType::Qualifier => counts.qualifier += 1,
            PythonUpdateType::Unclassified => counts.unclassified += 1,
        }

        packages.push(PythonOutdatedPackage {
            name: name.clone(),
            current: (*current).to_string(),
            latest: (*latest).to_string(),
            update_type,
            scope: match &direct {
                Some(direct) if direct.contains(name) => PythonDependencyScope::Direct,
                Some(_) => PythonDependencyScope::Transitive,
                // No second listing means nothing can be *shown* to be direct.
                None => PythonDependencyScope::Unknown,
            },
            // These three are the reason this adapter exists to prove the schema.
            // Poetry's JSON carries no groups, no extras, and no markers, so all
            // three are not-reported. `[]` would claim Poetry looked and found
            // none, which is the "unmeasured looks like none" falsehood the whole
            // payload is shaped to prevent (#10, #34).
            groups: None,
            extras: None,
            marker: PythonMarker::NotReported,
        });
    }

    // `settled` is a BTreeMap, so `packages` is already in normalized-name order.
    PythonOutdatedReport {
        checked: settled.len(),
        outdated: packages.len(),
        counts,
        packages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Loads a committed Poetry capture.
    ///
    /// Read at runtime rather than `include_str!`ed, because `tests/fixtures/**`
    /// is excluded from the published crate and a compile-time include would make
    /// `src/` unbuildable from the package.
    fn fixture(name: &str) -> Vec<PoetryPackage> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("poetry")
            .join(name);
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing poetry fixture {}: {err}", path.display()));
        serde_json::from_str(&contents)
            .unwrap_or_else(|err| panic!("parse poetry fixture {}: {err}", path.display()))
    }

    fn all() -> Vec<PoetryPackage> {
        fixture("show-latest.json")
    }

    fn top_level() -> Vec<PoetryPackage> {
        fixture("show-latest-top-level.json")
    }

    fn report() -> PythonOutdatedReport {
        normalize_show(&all(), Some(&top_level()))
    }

    fn package<'a>(report: &'a PythonOutdatedReport, name: &str) -> &'a PythonOutdatedPackage {
        report
            .packages
            .iter()
            .find(|package| package.name == name)
            .unwrap_or_else(|| panic!("{name} missing from the outdated report"))
    }

    /// **The point of this adapter.** Poetry's JSON carries no groups, no extras,
    /// and no markers, so all three must be *not reported* — never empty.
    ///
    /// `[]` would say Poetry reported the field and it was empty, which is the
    /// same class of falsehood as an unmeasured capability defaulting to a clean
    /// result (#10, #34). Nothing else in this crate reaches these three states at
    /// once, which is why #73 was worth building.
    #[test]
    fn unreported_attributes_are_null_and_never_empty() {
        let report = report();
        assert!(
            !report.packages.is_empty(),
            "fixture premise: there are entries to inspect"
        );

        for package in &report.packages {
            assert_eq!(
                package.groups, None,
                "{}: Poetry reports no groups, so `[]` would claim it has none",
                package.name
            );
            assert_eq!(
                package.extras, None,
                "{}: Poetry reports no extras, so `[]` would claim it has none",
                package.name
            );
            assert_eq!(
                package.marker,
                PythonMarker::NotReported,
                "{}: `absent` would claim Poetry reports markers and this one has none",
                package.name
            );
        }

        // And the encodings are genuinely distinct once serialized, so the
        // assertions above are not comparing two spellings of the same JSON.
        let serialized = serde_json::to_value(&report.packages[0]).expect("serialize");
        assert!(serialized["groups"].is_null());
        assert!(serialized["extras"].is_null());
        assert_ne!(serialized["groups"], serde_json::json!([]));
        assert_eq!(
            serialized["marker"],
            serde_json::json!({"status": "not_reported"})
        );
    }

    /// `checked` is every package Poetry settled the question for, not just the
    /// ones that are behind.
    ///
    /// This is why the adapter runs `--latest` rather than `--outdated`. Under
    /// `--outdated` the two numbers are equal by construction, and a denominator
    /// that always equals its numerator tells a caller nothing.
    #[test]
    fn checked_counts_every_package_not_only_the_outdated_ones() {
        let report = report();

        assert_eq!(report.checked, 16, "the fixture lists sixteen packages");
        assert_eq!(report.outdated, 10);
        assert!(
            report.checked > report.outdated,
            "an --outdated-only listing would make these equal and the denominator useless"
        );
        for current in [
            "certifi",
            "itsdangerous",
            "jinja2",
            "pyparsing",
            "pysocks",
            "six",
        ] {
            assert!(
                !report
                    .packages
                    .iter()
                    .any(|package| package.name == current),
                "{current} is at its latest version in the fixture"
            );
        }
    }

    /// An empty `--top-level` listing beside a non-empty full one did not answer.
    /// Treating it as "nothing is direct" would file every package as transitive —
    /// the same false claim the `None` branch exists to avoid, reached through the
    /// success path rather than the failure one.
    #[test]
    fn an_empty_top_level_listing_leaves_scope_unknown() {
        let report = normalize_show(&all(), Some(&[]));
        assert!(
            !report.packages.is_empty(),
            "fixture premise: packages were settled"
        );
        for package in &report.packages {
            assert_eq!(
                package.scope,
                PythonDependencyScope::Unknown,
                "{}: an unanswered listing is not evidence of transitivity",
                package.name
            );
        }
    }

    /// Scope comes from diffing the two listings.
    ///
    /// `six` is the entry that makes this test mean something: it is declared
    /// directly *and* is up to date, so it appears under `--top-level` while being
    /// absent from any outdated listing. Without it, a `--top-level` that returned
    /// "the outdated direct dependencies" would pass this test too.
    #[test]
    fn scope_comes_from_the_top_level_listing() {
        let report = report();

        for direct in [
            "requests",
            "flask",
            "pyyaml",
            "click",
            "packaging",
            "markupsafe",
        ] {
            assert_eq!(
                package(&report, direct).scope,
                PythonDependencyScope::Direct,
                "{direct} is declared by the project"
            );
        }
        for transitive in ["urllib3", "idna", "chardet", "werkzeug"] {
            assert_eq!(
                package(&report, transitive).scope,
                PythonDependencyScope::Transitive,
                "{transitive} is only reached through another package"
            );
        }

        assert!(
            top_level().iter().any(|package| package.name == "six"),
            "fixture premise: --top-level lists a direct dependency that is not outdated, which \
             is what proves it lists direct dependencies rather than fewer outdated ones"
        );
    }

    /// Without the second listing nothing can be *shown* to be direct, so every
    /// scope is `unknown` rather than defaulted to transitive.
    #[test]
    fn a_missing_top_level_listing_reports_unknown_scope() {
        let report = normalize_show(&all(), None);

        assert!(!report.packages.is_empty(), "fixture premise");
        assert!(
            report
                .packages
                .iter()
                .all(|package| package.scope == PythonDependencyScope::Unknown),
            "an entry that cannot be established as direct must not be filed as transitive"
        );
    }

    /// The summary invariants `docs/python-schema.md` states as contracts.
    #[test]
    fn summaries_agree_with_their_entries() {
        let report = report();
        let counts = &report.counts;

        assert_eq!(report.outdated, report.packages.len());
        assert_eq!(
            counts.epoch
                + counts.major
                + counts.minor
                + counts.patch
                + counts.qualifier
                + counts.unclassified,
            report.outdated
        );
        assert!(report.checked >= report.outdated);
    }

    /// Classification runs on the real capture, not only on the `pep440` unit
    /// table, and reuses that module rather than a second classifier.
    #[test]
    fn update_types_classify_the_captured_versions() {
        let report = report();

        // 3.0.4 -> 7.6.0, 2.0.0 -> 3.1.3, 1.23 -> 2.7.0: all first-component.
        for major in ["chardet", "flask", "urllib3"] {
            assert_eq!(package(&report, major).update_type, PythonUpdateType::Major);
        }
        // 8.0.1 -> 8.5.0 and 2.19.1 -> 2.34.2 keep their first component.
        for minor in ["click", "requests"] {
            assert_eq!(package(&report, minor).update_type, PythonUpdateType::Minor);
        }
        assert_eq!(report.counts.major, 8);
        assert_eq!(report.counts.minor, 2);
        assert_eq!(report.counts.unclassified, 0);
    }

    /// Two spellings of one version are not an available update.
    ///
    /// Poetry reports the locked version verbatim, so `1.0` against `1.0.0` is a
    /// shape a real project can produce, and string equality would file it as an
    /// update nobody can act on.
    #[test]
    fn versions_that_differ_only_in_spelling_are_not_outdated() {
        let packages: Vec<PoetryPackage> = serde_json::from_str(
            r#"[{"name":"Zope.Interface","installed_status":"installed","version":"1.0","latest_version":"1.0.0"},
                {"name":"real","installed_status":"installed","version":"1.0","latest_version":"1.1"}]"#,
        )
        .expect("parse");
        let report = normalize_show(&packages, None);

        assert_eq!(report.checked, 2, "both had their freshness settled");
        assert_eq!(report.outdated, 1);
        assert_eq!(report.packages[0].name, "real");
    }

    /// A `latest_version` *below* `current` is a downgrade, not an available
    /// major update. `docs/python-schema.md` names Poetry as the source that does
    /// this, when the newest stable release is behind an installed pre-release.
    #[test]
    fn a_latest_below_current_is_unclassified_rather_than_an_upgrade() {
        let packages: Vec<PoetryPackage> = serde_json::from_str(
            r#"[{"name":"pre","installed_status":"installed","version":"2.0.0rc1","latest_version":"1.9.0"}]"#,
        )
        .expect("parse");
        let report = normalize_show(&packages, None);

        assert_eq!(
            report.packages[0].update_type,
            PythonUpdateType::Unclassified,
            "calling a downgrade a `major` update would advertise it as an upgrade"
        );
        assert_eq!(report.counts.unclassified, 1);
        assert_eq!(report.counts.major, 0);
    }

    /// A package Poetry gave no `latest_version` for leaves the denominator.
    ///
    /// The opposite of the `uv` adapter's rule for the same field name: uv omits
    /// it to mean "already current", Poetry omits it when it did not answer. A
    /// package whose freshness was never settled must not be counted as settled.
    #[test]
    fn a_package_without_a_latest_version_is_not_counted_as_checked() {
        let packages: Vec<PoetryPackage> = serde_json::from_str(
            r#"[{"name":"answered","installed_status":"installed","version":"1.0","latest_version":"2.0"},
                {"name":"unanswered","installed_status":"installed","version":"1.0"}]"#,
        )
        .expect("parse");
        let report = normalize_show(&packages, None);

        assert_eq!(
            report.checked, 1,
            "only one package's freshness was settled"
        );
        assert_eq!(report.outdated, 1);
    }

    /// Names are PEP 503 normalized on both sides, so the scope diff lines up
    /// however the two invocations happen to spell a name.
    #[test]
    fn package_names_are_normalized_on_both_sides() {
        let all: Vec<PoetryPackage> = serde_json::from_str(
            r#"[{"name":"Zope.Interface","installed_status":"installed","version":"5.0","latest_version":"6.0"}]"#,
        )
        .expect("parse");
        let top: Vec<PoetryPackage> = serde_json::from_str(
            r#"[{"name":"zope_interface","installed_status":"installed","version":"5.0","latest_version":"6.0"}]"#,
        )
        .expect("parse");

        let report = normalize_show(&all, Some(&top));
        assert_eq!(report.packages[0].name, "zope-interface");
        assert_eq!(report.packages[0].scope, PythonDependencyScope::Direct);
    }

    // ===== capability probing =====

    /// Verbatim stderr from Poetry 2.4.2, captured by running the probe.
    const CURRENT_FORMAT_PROBE: &str =
        "Error: Invalid output format. Supported formats are: json, text.\n";

    /// Verbatim stderr from Poetry 2.4.2 for a flag it does not have. A Poetry
    /// predating `poetry show --format` answers this way.
    const UNKNOWN_OPTION_PROBE: &str = "\nThe option \"--format\" does not exist\n";

    fn unavailable(capability: Capability) -> (PythonUnavailableReason, String) {
        match capability {
            Capability::Available => panic!("expected the capability to be unavailable"),
            Capability::Unavailable { reason, detail } => (reason, detail),
        }
    }

    #[test]
    fn current_poetry_advertises_json() {
        assert!(matches!(
            probe_capability(CURRENT_FORMAT_PROBE, false, &SHOW_ARGS),
            Capability::Available
        ));
    }

    /// A Poetry too old for `--format` is the runner's problem, and the gap has to
    /// name how to close it.
    #[test]
    fn a_poetry_without_the_format_flag_reports_an_upgrade_hint() {
        let (reason, detail) =
            unavailable(probe_capability(UNKNOWN_OPTION_PROBE, false, &SHOW_ARGS));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
        assert!(detail.contains("--format"), "{detail}");
        assert!(detail.contains("poetry self update"), "{detail}");
    }

    /// Every flag the real invocation uses is probed, not just `--format`.
    #[test]
    fn a_rejected_flag_is_reported_against_the_flag_poetry_named() {
        let (reason, detail) = unavailable(probe_capability(
            "\nThe option \"--latest\" does not exist\n",
            false,
            &SHOW_ARGS,
        ));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
        assert!(detail.contains("--latest"), "{detail}");
    }

    /// A `--format` that exists but offers no JSON is a capability gap, not a
    /// success — the Poetry equivalent of the `uv audit` window where the flag
    /// shipped before its JSON did.
    #[test]
    fn a_format_flag_without_json_is_a_capability_gap() {
        let (reason, detail) = unavailable(probe_capability(
            "Error: Invalid output format. Supported formats are: text.\n",
            false,
            &SHOW_ARGS,
        ));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
        assert!(detail.contains("accepts only text"), "{detail}");
    }

    /// Poetry's "does not exist" wording covers both options and commands, and the
    /// two must not be confused: a rejected *command* is not an answer about a
    /// flag.
    #[test]
    fn a_rejected_command_is_not_read_as_a_rejected_flag() {
        let (reason, detail) = unavailable(probe_capability(
            "The requested command show does not exist.\n\nDocumentation: \
             https://python-poetry.org/docs/cli/\n",
            false,
            &SHOW_ARGS,
        ));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
        assert!(
            detail.contains("no `show` command"),
            "a missing command must not be reported as a missing flag: {detail}"
        );
    }

    /// A wrapped message is still read, because Poetry wraps to the terminal
    /// width.
    #[test]
    fn a_wrapped_message_is_still_read() {
        assert!(matches!(
            probe_capability(
                "Error: Invalid output\n  format. Supported formats\n  are: json, text.\n",
                false,
                &SHOW_ARGS
            ),
            Capability::Available
        ));
        let (reason, _) = unavailable(probe_capability(
            "The option\n  \"--format\" does not\n  exist\n",
            false,
            &SHOW_ARGS,
        ));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
    }

    /// A probe Poetry *accepted* teaches nothing, and must not read as available.
    #[test]
    fn an_accepted_probe_value_is_inconclusive() {
        let (reason, detail) = unavailable(probe_capability("", true, &SHOW_ARGS));
        assert_eq!(reason, PythonUnavailableReason::Failed);
        assert!(detail.contains("accepted the probe value"), "{detail}");
    }

    /// An unrecognized failure establishes nothing, and says so with Poetry's own
    /// words rather than guessing.
    #[test]
    fn an_unrecognized_failure_is_inconclusive_not_a_missing_tool() {
        let (reason, detail) = unavailable(probe_capability(
            "Error: poetry.lock not found. Run `poetry lock` to create it.\n",
            false,
            &SHOW_ARGS,
        ));
        assert_eq!(
            reason,
            PythonUnavailableReason::Failed,
            "a broken project is not an old Poetry"
        );
        assert!(detail.contains("poetry.lock not found"), "{detail}");
    }

    /// The notice this adapter's own virtualenv suppression provokes must never
    /// be reported as the reason something failed.
    ///
    /// Found by running the built binary against a Poetry project with no
    /// lockfile. Poetry prints `Skipping virtualenv creation, as specified in
    /// config file.` *before* its real complaint, so taking the first non-empty
    /// stderr line reported our own configuration choice as the user's problem —
    /// a message that is true, entirely self-inflicted, and no help to someone
    /// whose actual problem was a missing `poetry.lock`.
    #[test]
    fn our_own_virtualenv_notice_is_never_reported_as_the_failure() {
        const REAL: &str = "Skipping virtualenv creation, as specified in config file.\nError: \
                            poetry.lock not found. Run `poetry lock` to create it.\n";

        assert_eq!(
            poetry_message(REAL),
            Some("Error: poetry.lock not found. Run `poetry lock` to create it.".to_string()),
            "the actionable line is Poetry's, not ours"
        );

        let (_, detail) = unavailable(probe_capability(REAL, false, &SHOW_ARGS));
        assert!(
            detail.contains("poetry.lock not found"),
            "the probe detail must carry Poetry's complaint: {detail}"
        );
        assert!(
            !detail.contains(VIRTUALENV_NOTICE),
            "the probe detail must not blame our own suppression: {detail}"
        );
    }

    /// Poetry's `Error:` line wins over whatever chatter precedes it, and a
    /// failure with no wording at all is `None` rather than an empty string.
    #[test]
    fn the_reported_message_prefers_poetrys_own_error_line() {
        assert_eq!(
            poetry_message("Loading configuration\nError: something broke\ntrailing\n"),
            Some("Error: something broke".to_string())
        );
        // No `Error:` prefix anywhere: the first surviving line is the fallback.
        assert_eq!(
            poetry_message("\n  The option \"--format\" does not exist\n"),
            Some("The option \"--format\" does not exist".to_string())
        );
        // Poetry spreads a lockfile complaint over two lines with no `Error:`
        // prefix, and the remedy is the second one. Taking only the first would
        // hand back the diagnosis without the cure.
        assert_eq!(
            poetry_message(
                "Skipping virtualenv creation, as specified in config file.\n\n\
                 The lock file does not have a metadata entry.\n\
                 Regenerate the lock file with the `poetry lock` command.\n"
            ),
            Some(
                "The lock file does not have a metadata entry. Regenerate the lock file \
                 with the `poetry lock` command."
                    .to_string()
            ),
        );
        assert_eq!(poetry_message(""), None);
        assert_eq!(
            poetry_message("Skipping virtualenv creation, as specified in config file.\n"),
            None,
            "a stderr holding nothing but our own notice said nothing at all"
        );
    }

    /// `poetry --version` is read for the report, never for a capability decision.
    #[test]
    fn version_is_parsed_from_poetry_version_output() {
        assert_eq!(
            parse_version("Poetry (version 2.4.2)\n").as_deref(),
            Some("2.4.2")
        );
        assert_eq!(
            parse_version("Poetry (version 1.8.3)").as_deref(),
            Some("1.8.3")
        );
        assert_eq!(parse_version("something else\n"), None);
        assert_eq!(parse_version("Poetry (version )\n"), None);
        assert_eq!(parse_version(""), None);
    }
}

#[cfg(test)]
mod stderr_parsing_tests {
    use super::*;

    /// The format list is found in a lowercased copy of stderr but was sliced
    /// out of the original. `to_lowercase` is not length preserving — `İ`
    /// (U+0130) is two bytes and lowercases to three — so any such character
    /// before the prefix shifts every later offset.
    ///
    /// Poetry's own wording is ASCII, but this parses whatever reached stderr:
    /// a plugin's output, a localized line, a package or author name in a
    /// diagnostic. The consequences were a panic in a released binary, or a
    /// silently mangled list that reports a current Poetry as too old.
    /// `İ` shifts the offset by one byte; the `é` immediately after the prefix
    /// is where that lands — one byte into a two-byte character, which is a
    /// panic rather than a wrong answer.
    #[test]
    fn a_non_ascii_prefix_does_not_break_the_format_list() {
        let stderr = "İ Error: Invalid output format. Supported formats are:\u{e9}json, text.";
        assert_eq!(
            supported_formats(stderr),
            Some(vec!["\u{e9}json".to_string(), "text".to_string()]),
            "the list must be read from the same string the offset was found in"
        );
    }

    /// Without the character boundary to trip on, the same shift fails quietly
    /// instead: the first format name loses its leading byte. That is the worse
    /// half — a mangled list makes `json` unrecognizable, so a current Poetry is
    /// reported as too old, with an upgrade hint it does not need.
    #[test]
    fn a_non_ascii_prefix_does_not_corrupt_the_format_names() {
        let stderr = "İ Error: Invalid output format. Supported formats are:json, text.";
        let formats = supported_formats(stderr).expect("the list is still found");
        assert!(
            formats.iter().any(|format| format == "json"),
            "json must survive verbatim; got {formats:?}"
        );
    }

    #[test]
    fn an_unrelated_stderr_has_no_format_list() {
        assert_eq!(supported_formats("Error: something else entirely"), None);
    }
}
