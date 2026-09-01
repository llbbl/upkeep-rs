//! The `uv` adapter: capability detection, invocation, and normalization into
//! the schema in [`crate::core::python`].
//!
//! # Normalize, do not re-analyze
//!
//! `uv` already answers both questions this adapter asks. `uv tree --outdated`
//! resolves the graph and looks up the newest release of every package;
//! `uv audit` queries an advisory service. Reimplementing either would duplicate
//! work uv does well. What uv does *not* provide is a stable contract — both
//! commands self-declare `"schema": {"version": "preview"}` and `uv audit` prints
//! an experimental-tool warning to stderr — so the value added here is the
//! normalization, not the analysis.
//!
//! # Capabilities are probed, never inferred from a version number
//!
//! The installed `uv` on a developer machine or a CI image can sit anywhere
//! across a very wide capability range: 0.7.11 (June 2025) has no `audit`
//! subcommand at all, `uv audit` arrived around 0.10.10, its JSON output in
//! 0.11.15, and `uv tree --format json` exists in 0.12.8 while appearing in *no*
//! release note. A version-number lookup table would therefore be wrong in both
//! directions, and a changelog is not a capability oracle.
//!
//! So every capability is established by running the exact command line the
//! adapter intends to use, with the format value replaced by
//! [`CAPABILITY_PROBE_VALUE`], and reading what uv's own argument parser says
//! back. That is an argument-parsing failure: it costs about ten milliseconds,
//! reaches no network, and cannot be fooled by a version string. This crate has
//! been bitten by the inferred-capability class of bug before — see the
//! `cargo-machete --json` and `cargo-geiger --output-format` history in
//! [`crate::core::analyzers::external_tool`].
//!
//! A capability that does not answer becomes an `unavailable[]` entry and a
//! `null` report. It never becomes a clean result.

use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::pep440;
use crate::core::python::{
    normalize_package_name, PythonDependencyScope, PythonMarker, PythonOutdatedPackage,
    PythonOutdatedReport, PythonSecurityReport, PythonSecuritySummary, PythonSeverity,
    PythonUnavailableReason, PythonUpdateCounts, PythonUpdateType, PythonVulnerability,
};

/// Overrides which `uv` binary is executed.
///
/// Exists for the same reason `UPKEEP_ADVISORY_DB` does: the capability-gap
/// paths are the ones most worth testing end to end, and they are unreachable on
/// a machine with a current `uv` unless the binary can be pointed elsewhere.
pub const UV_BIN_ENV: &str = "UPKEEP_UV_BIN";

/// The value handed to a format flag so uv's argument parser enumerates what it
/// accepts.
///
/// It has to be a value uv will never add. If it somehow became valid, the probe
/// would run the real command instead of failing to parse, which is why
/// [`probe_capability`] treats a *successful* probe as inconclusive rather than
/// as a pass.
const CAPABILITY_PROBE_VALUE: &str = "cargo-upkeep-capability-probe";

/// What to tell a user whose `uv` is too old for a capability.
///
/// #72 asks for the exact upgrade command when a capability gap is what limits
/// the result. `uv self update` only works for a standalone install, so the
/// package-manager case is named too rather than silently failing for anyone who
/// installed uv through Homebrew, pipx, or mise.
const UPGRADE_HINT: &str =
    "upgrade uv (`uv self update`, or through whichever package manager installed it)";

/// `uv tree`'s arguments, with the format value left to the caller.
///
/// `--frozen` is what makes this read-only: it reads `uv.lock` as it stands and
/// never writes one. Every invocation in this module is `--frozen` or `--locked`
/// for that reason — `lock --upgrade`, `sync`, `add`, and `remove` all mutate the
/// project being inspected.
const TREE_ARGS: [&str; 4] = ["tree", "--outdated", "--frozen", "--format"];

/// `uv audit`'s arguments, with the format value left to the caller.
const AUDIT_ARGS: [&str; 3] = ["audit", "--frozen", "--output-format"];

/// A detected `uv` installation and the project it will be run against.
pub struct Uv {
    binary: OsString,
    project_root: PathBuf,
    version: Option<String>,
}

/// The outcome of probing one capability.
pub enum Capability {
    Available,
    Unavailable {
        reason: PythonUnavailableReason,
        detail: String,
    },
}

impl Uv {
    /// Locates `uv` and the project it should inspect.
    ///
    /// Both failures here are the documented "no supported Python manager could
    /// be detected" exit: there is no report to stand on, so this is one of the
    /// two conditions that fail without any flag being passed.
    pub async fn detect(start: &Path) -> Result<Self> {
        let binary = std::env::var_os(UV_BIN_ENV).unwrap_or_else(|| OsString::from("uv"));

        let project_root = find_project_root(start).ok_or_else(|| {
            UpkeepError::message(
                ErrorCode::InvalidData,
                format!(
                    "no supported Python manager could be detected: no pyproject.toml or uv.lock \
                     in {} or any parent directory",
                    start.display()
                ),
            )
        })?;

        let output = Command::new(&binary)
            .arg("--version")
            .current_dir(&project_root)
            .output()
            .await
            .map_err(|err| match err.kind() {
                io::ErrorKind::NotFound => UpkeepError::message(
                    ErrorCode::MissingTool,
                    "no supported Python manager could be detected: uv is not installed or not on \
                     PATH; see https://docs.astral.sh/uv/getting-started/installation/",
                ),
                _ => UpkeepError::context(
                    ErrorCode::ExternalCommand,
                    "failed to execute uv --version",
                    err,
                ),
            })?;

        Ok(Self {
            binary,
            project_root,
            version: parse_version(&String::from_utf8_lossy(&output.stdout)),
        })
    }

    /// The manager's own version string, or `None` when `uv --version` said
    /// something this crate does not recognize.
    ///
    /// Never used to decide what uv can do. It is reported so a human reading the
    /// payload can see what they ran; the capabilities are probed.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    async fn run(&self, args: &[&str]) -> Result<std::process::Output> {
        Command::new(&self.binary)
            .args(args)
            .current_dir(&self.project_root)
            .output()
            .await
            .map_err(|err| {
                UpkeepError::context(
                    ErrorCode::ExternalCommand,
                    format!("failed to execute uv {}", args.join(" ")),
                    err,
                )
            })
    }

    /// Probes whether `uv tree` can emit the JSON this adapter reads.
    pub async fn probe_outdated(&self) -> Capability {
        self.probe(&TREE_ARGS, "tree", "--format").await
    }

    /// Probes whether `uv audit` exists and can emit JSON.
    pub async fn probe_security(&self) -> Capability {
        self.probe(&AUDIT_ARGS, "audit", "--output-format").await
    }

    async fn probe(&self, args: &[&str], subcommand: &str, format_flag: &str) -> Capability {
        let mut argv: Vec<&str> = args.to_vec();
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
            args,
            subcommand,
            format_flag,
        )
    }

    /// Runs `uv tree --outdated --frozen --format json` and normalizes it.
    ///
    /// The [`ScopeIndex`] comes back alongside the report because `uv audit`
    /// names only a package and a version; direct-versus-transitive is a fact
    /// only the tree knows.
    pub async fn outdated(&self) -> Result<(PythonOutdatedReport, ScopeIndex)> {
        let mut argv: Vec<&str> = TREE_ARGS.to_vec();
        argv.push("json");
        let output = self.run(&argv).await?;

        // uv writes an experimental-output warning to stderr and the report to
        // stdout, so stdout is the only thing read here.
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() && stdout.trim().is_empty() {
            return Err(external_failure("uv tree --outdated", &output));
        }

        let tree: UvTree = serde_json::from_str(&stdout).map_err(|err| {
            UpkeepError::context(
                ErrorCode::InvalidData,
                "uv tree --format json did not produce the expected JSON",
                err,
            )
        })?;

        Ok(normalize_tree_with_scopes(&tree))
    }

    /// Runs `uv audit --frozen --output-format json` and normalizes it.
    ///
    /// `scopes` comes from the outdated capability when it ran; `uv audit` names
    /// only a package and a version, so without the graph every finding's scope
    /// is honestly [`PythonDependencyScope::Unknown`].
    pub async fn security(
        &self,
        scopes: &ScopeIndex,
    ) -> Result<(PythonSecurityReport, Vec<String>)> {
        let mut argv: Vec<&str> = AUDIT_ARGS.to_vec();
        argv.push("json");
        let output = self.run(&argv).await?;

        // `uv audit` exits 1 when it finds vulnerabilities, which is a successful
        // run with findings rather than a failure. Only an empty stdout means it
        // did not answer.
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Err(external_failure("uv audit", &output));
        }

        let audit: UvAudit = serde_json::from_str(&stdout).map_err(|err| {
            UpkeepError::context(
                ErrorCode::InvalidData,
                "uv audit --output-format json did not produce the expected JSON",
                err,
            )
        })?;

        Ok(normalize_audit(&audit, scopes))
    }
}

/// Turns a failed external run into an error carrying uv's own explanation.
fn external_failure(command: &str, output: &std::process::Output) -> UpkeepError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    UpkeepError::message(
        ErrorCode::ExternalCommand,
        if message.is_empty() {
            format!("{command} failed with no output")
        } else {
            format!("{command} failed: {message}")
        },
    )
}

/// Walks up from `start` looking for a project `uv` could act on.
///
/// `uv` performs the same walk itself, but the answer is needed *before* uv runs:
/// "there is no Python project here" is a different report from "uv is not
/// installed", and both are the no-report exit rather than an empty result.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(directory) = current {
        if directory.join("pyproject.toml").is_file() || directory.join("uv.lock").is_file() {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
}

/// Extracts the version from `uv --version` output.
///
/// The line is `uv 0.12.8 (68209e5c6 2026-08-31 aarch64-apple-darwin)`. Only the
/// version token is kept, and it is never parsed further: the schema reports a
/// manager version verbatim because these are not all PEP 440 versions.
fn parse_version(stdout: &str) -> Option<String> {
    let mut tokens = stdout.lines().next()?.split_whitespace();
    if tokens.next()? != "uv" {
        return None;
    }
    tokens.next().map(str::to_string)
}

/// Decides a capability from what uv's argument parser said about the probe.
///
/// Split out from [`Uv::probe`] so the classification can be tested against
/// captured stderr from real uv releases without running anything. The wordings
/// matched here were captured from uv 0.7.11 and 0.12.8, not recalled from
/// documentation.
pub fn probe_capability(
    stderr: &str,
    succeeded: bool,
    args: &[&str],
    subcommand: &str,
    format_flag: &str,
) -> Capability {
    // Newer uv rejects the sentinel and lists what it would have accepted. That
    // list is the capability answer, straight from the tool.
    if let Some(values) = possible_values(stderr, format_flag) {
        return if values.iter().any(|value| value == "json") {
            Capability::Available
        } else {
            Capability::Unavailable {
                reason: PythonUnavailableReason::NotInstalled,
                detail: format!(
                    "`uv {subcommand} {format_flag}` accepts only {} on this uv; {UPGRADE_HINT} \
                     for machine-readable output",
                    values.join(", "),
                ),
            }
        };
    }

    // uv 0.7.11: `error: unrecognized subcommand 'audit'`.
    if is_unrecognized_subcommand(stderr, subcommand) {
        return Capability::Unavailable {
            reason: PythonUnavailableReason::NotInstalled,
            detail: format!("this uv has no `{subcommand}` subcommand; {UPGRADE_HINT}"),
        };
    }

    // uv 0.7.11: `error: unexpected argument '--format' found`. Every flag the
    // real invocation uses is checked, not just the format flag, so a uv that
    // dropped `--outdated` or `--frozen` is reported against the flag it
    // actually rejected.
    for flag in args.iter().filter(|arg| arg.starts_with("--")) {
        if crate::core::analyzers::external_tool::is_unknown_flag(stderr, flag) {
            return Capability::Unavailable {
                reason: PythonUnavailableReason::NotInstalled,
                detail: format!(
                    "`uv {subcommand}` on this uv does not accept `{flag}`; {UPGRADE_HINT}"
                ),
            };
        }
    }

    // A probe that *succeeded* means the sentinel was accepted as a real format,
    // so nothing was learned and the real command may already have run. Reporting
    // that as available would be a guess.
    let detail = if succeeded {
        format!(
            "could not establish whether `uv {subcommand}` supports JSON output: uv accepted the \
             probe value `{CAPABILITY_PROBE_VALUE}` instead of rejecting it"
        )
    } else {
        format!(
            "could not establish whether `uv {subcommand}` supports JSON output: {}",
            first_line(stderr)
        )
    };
    Capability::Unavailable {
        reason: PythonUnavailableReason::Failed,
        detail,
    }
}

/// Reads clap's `[possible values: …]` list, but only when it belongs to the
/// probe we sent.
///
/// Both conditions matter. The list has to follow a rejection of
/// [`CAPABILITY_PROBE_VALUE`] for the named flag, or an unrelated argument error
/// elsewhere in the buffer would be read as an answer about this flag — the same
/// false positive `is_unknown_flag` is structured to avoid.
///
/// Whitespace is collapsed first because uv wraps its help and error output to
/// the terminal width, so the list can straddle a line break.
fn possible_values(stderr: &str, format_flag: &str) -> Option<Vec<String>> {
    let collapsed = stderr.split_whitespace().collect::<Vec<_>>().join(" ");

    let rejection = collapsed.find(&format!("invalid value '{CAPABILITY_PROBE_VALUE}'"))?;
    let after_rejection = &collapsed[rejection..];
    if !after_rejection.contains(format_flag) {
        return None;
    }

    let start = after_rejection.find("[possible values:")? + "[possible values:".len();
    let list = &after_rejection[start..];
    let end = list.find(']')?;

    Some(
        list[..end]
            .split(',')
            .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
            .filter(|value| !value.is_empty())
            .collect(),
    )
}

/// Whether uv rejected the subcommand itself.
///
/// Deliberately *not* added to `external_tool::MISSING_SUBCOMMAND_PATTERNS`.
/// That list is cargo's wording, and every entry added to it widens the surface
/// for misreading an unrelated cargo failure as a missing cargo subcommand. uv is
/// a different program with a different message, so it gets its own check.
///
/// Verbatim from uv 0.7.11: `error: unrecognized subcommand 'audit'`. The
/// subcommand must follow the pattern on the same line, for the reason
/// [`crate::core::analyzers::external_tool::is_missing_subcommand`] gives: a
/// flattened error chain can put our subcommand's name before an unrelated
/// nested complaint.
fn is_unrecognized_subcommand(stderr: &str, subcommand: &str) -> bool {
    const PATTERN: &str = "unrecognized subcommand";
    stderr.lines().any(|line| {
        let lower = line.to_lowercase();
        lower
            .find(PATTERN)
            .is_some_and(|start| lower[start + PATTERN.len()..].contains(subcommand))
    })
}

fn first_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("uv produced no output")
        .to_string()
}

// ===== `uv tree --format json` =====

/// The subset of `uv tree --format json` this adapter reads.
///
/// Modelled loosely on purpose. uv labels this schema `preview`, so every field
/// is optional and `kind` and `source` are held as raw [`Value`]s rather than
/// enums: an unrecognized `kind` has to degrade to "not a package we count",
/// never to a deserialization error that fails the whole run.
#[derive(Debug, Default, Deserialize)]
pub struct UvTree {
    #[serde(default)]
    roots: Vec<UvRef>,
    #[serde(default)]
    resolution: BTreeMap<String, UvNode>,
}

#[derive(Debug, Deserialize)]
struct UvRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct UvNamedRef {
    name: String,
    id: String,
}

#[derive(Debug, Deserialize)]
struct UvNode {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    latest_version: Option<String>,
    #[serde(default)]
    source: Option<Value>,
    #[serde(default)]
    kind: Option<Value>,
    #[serde(default)]
    dependencies: Vec<UvRef>,
    #[serde(default)]
    optional_dependencies: Vec<UvNamedRef>,
}

/// What a node in `resolution` represents.
///
/// uv's graph is not a list of packages. A workspace member appears once per
/// *section* — its base dependencies, each PEP 735 dependency group, and each
/// activated extra — and a package with an activated extra appears twice, once
/// as itself and once as an alias carrying the extra's own dependencies.
/// Counting those aliases as packages would double-count `requests` the moment
/// anything depends on `requests[socks]`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeKind {
    Package,
    Workspace,
    Group(String),
    Extra(String),
    /// A `kind` this version of the adapter does not recognize. Additive changes
    /// to a preview schema are expected, and an unknown kind must not be counted
    /// as a package.
    Unrecognized,
}

fn node_kind(kind: Option<&Value>) -> NodeKind {
    match kind {
        Some(Value::String(label)) if label == "package" => NodeKind::Package,
        Some(Value::String(label)) if label == "workspace" => NodeKind::Workspace,
        Some(Value::Object(map)) => {
            if let Some(Value::String(name)) = map.get("group") {
                NodeKind::Group(name.clone())
            } else if let Some(Value::String(name)) = map.get("extra") {
                NodeKind::Extra(name.clone())
            } else {
                NodeKind::Unrecognized
            }
        }
        _ => NodeKind::Unrecognized,
    }
}

/// Whether a node came from a package index.
///
/// Only registry packages have a "latest version" to be behind, so editable,
/// path, git, and URL sources leave the denominator entirely rather than
/// counting as up to date. That is the same rule `deps` applies to git and path
/// crates on the Rust side.
fn is_registry(source: Option<&Value>) -> bool {
    match source {
        Some(Value::Object(map)) => map.contains_key("registry"),
        Some(Value::String(text)) => text.starts_with("registry+"),
        _ => false,
    }
}

/// Which packages are direct dependencies, for joining `uv audit` findings to a
/// scope they cannot report themselves.
#[derive(Debug, Default)]
pub struct ScopeIndex {
    scopes: HashMap<String, PythonDependencyScope>,
}

impl ScopeIndex {
    /// The scope recorded for a package, or [`PythonDependencyScope::Unknown`]
    /// when the graph was never built or does not mention it.
    ///
    /// `Unknown` is a real answer here rather than a fallback: when the outdated
    /// capability did not run there is no graph, and a finding that cannot be
    /// established as direct must not be filed as transitive to fill the field.
    pub fn get(&self, name: &str) -> PythonDependencyScope {
        self.scopes
            .get(&normalize_package_name(name))
            .copied()
            .unwrap_or(PythonDependencyScope::Unknown)
    }
}

/// The graph facts the report needs, derived once from `resolution` and `roots`.
struct TreeGraph<'a> {
    tree: &'a UvTree,
    kinds: HashMap<&'a str, NodeKind>,
    /// Section entry points: a workspace member's base node, one node per
    /// dependency group, and one per activated project extra.
    roots: HashSet<&'a str>,
    /// An extra-alias node's id mapped to the package it is an alias *of*.
    alias_base: HashMap<&'a str, &'a str>,
    /// Every activated extra name, by the id of the package that carries it.
    extras: HashMap<&'a str, BTreeSet<String>>,
    /// Section names reaching each package id.
    groups: HashMap<&'a str, BTreeSet<String>>,
    direct: HashSet<&'a str>,
    reachable: HashSet<&'a str>,
}

impl<'a> TreeGraph<'a> {
    fn build(tree: &'a UvTree) -> Self {
        let kinds: HashMap<&str, NodeKind> = tree
            .resolution
            .iter()
            .map(|(id, node)| (id.as_str(), node_kind(node.kind.as_ref())))
            .collect();
        let roots: HashSet<&str> = tree.roots.iter().map(|root| root.id.as_str()).collect();

        // An extra alias is folded into the package it belongs to — unless it is
        // itself a section root, which is what a workspace member's own
        // `[project.optional-dependencies]` entry is. Folding those would let a
        // walk of the `main` section reach everything an extra pulls in.
        let mut alias_base = HashMap::new();
        let mut extras: HashMap<&str, BTreeSet<String>> = HashMap::new();
        for (id, node) in &tree.resolution {
            for optional in &node.optional_dependencies {
                if !tree.resolution.contains_key(&optional.id)
                    || roots.contains(optional.id.as_str())
                {
                    continue;
                }
                alias_base.insert(optional.id.as_str(), id.as_str());
                extras
                    .entry(id.as_str())
                    .or_default()
                    .insert(optional.name.clone());
            }
        }

        let mut graph = Self {
            tree,
            kinds,
            roots,
            alias_base,
            extras,
            groups: HashMap::new(),
            direct: HashSet::new(),
            reachable: HashSet::new(),
        };
        graph.walk_sections();
        graph
    }

    /// Resolves an extra alias to the package it stands for.
    fn resolve(&self, id: &'a str) -> &'a str {
        let mut current = id;
        // Bounded rather than `while let`: a preview schema that ever produced a
        // cycle here would otherwise hang the command.
        for _ in 0..self.tree.resolution.len() {
            match self.alias_base.get(current) {
                Some(base) => current = base,
                None => break,
            }
        }
        current
    }

    /// The dependencies of a package, including those its activated extras add.
    ///
    /// `requests[socks]` is not a package; it is `requests` with one more edge.
    /// Its dependencies therefore belong to `requests`, and `pysocks` is
    /// transitive rather than a direct dependency of whatever asked for the
    /// extra.
    fn effective_dependencies(&self, id: &'a str) -> Vec<&'a str> {
        let mut targets = Vec::new();
        let mut push = |node: &'a UvNode| {
            for dependency in &node.dependencies {
                let resolved = self.resolve(dependency.id.as_str());
                if resolved != id {
                    targets.push(resolved);
                }
            }
        };

        if let Some(node) = self.tree.resolution.get(id) {
            push(node);
        }
        for (alias, base) in &self.alias_base {
            if *base == id {
                if let Some(node) = self.tree.resolution.get(*alias) {
                    push(node);
                }
            }
        }
        targets
    }

    /// Labels every package with the sections that reach it, and records which
    /// are one hop from a section root.
    ///
    /// A walk stops at any other section root. Without that, the `extra-feature`
    /// section — whose node depends on the member's base node — would relabel
    /// every `main` dependency as belonging to the extra as well.
    fn walk_sections(&mut self) {
        for root in self.tree.roots.iter().map(|root| root.id.as_str()) {
            let Some(label) = self.section_label(root) else {
                continue;
            };

            let mut queue: VecDeque<&str> = VecDeque::new();
            let mut seen: HashSet<&str> = HashSet::new();

            for target in self.effective_dependencies(root) {
                if self.roots.contains(target) {
                    continue;
                }
                self.direct.insert(target);
                if seen.insert(target) {
                    queue.push_back(target);
                }
            }

            while let Some(current) = queue.pop_front() {
                self.reachable.insert(current);
                self.groups
                    .entry(current)
                    .or_default()
                    .insert(label.clone());

                for target in self.effective_dependencies(current) {
                    if self.roots.contains(target)
                        || self.kinds.get(target) == Some(&NodeKind::Workspace)
                    {
                        continue;
                    }
                    if seen.insert(target) {
                        queue.push_back(target);
                    }
                }
            }
        }
    }

    /// The section name a root stands for.
    ///
    /// `main` for a member's base dependencies, the group name for a PEP 735
    /// dependency group, and the extra name for a `[project.optional-dependencies]`
    /// entry.
    ///
    /// Project extras share the field with dependency groups because uv models
    /// both as parallel, separately selectable sections of one member — the same
    /// reason `--only-group` and `--no-extra` sit beside each other in `uv
    /// audit --help`. The alternative was reporting `groups: []` for a dependency
    /// the project genuinely declares, which reads as "belongs to nothing" and is
    /// the "unmeasured looks like none" mistake this schema exists to prevent.
    fn section_label(&self, root: &str) -> Option<String> {
        match self.kinds.get(root)? {
            NodeKind::Package => Some("main".to_string()),
            NodeKind::Group(name) => Some(name.clone()),
            NodeKind::Extra(name) => Some(name.clone()),
            NodeKind::Workspace | NodeKind::Unrecognized => None,
        }
    }

    fn scope(&self, id: &str) -> PythonDependencyScope {
        if self.direct.contains(id) {
            // A package can be both — declared directly and pulled in again
            // through something else. `direct` is the actionable answer, and the
            // schema has no way to say "both".
            PythonDependencyScope::Direct
        } else if self.reachable.contains(id) {
            PythonDependencyScope::Transitive
        } else {
            PythonDependencyScope::Unknown
        }
    }
}

/// Normalizes a parsed `uv tree --outdated` document into the schema, along with
/// the scope index `uv audit` needs.
pub fn normalize_tree_with_scopes(tree: &UvTree) -> (PythonOutdatedReport, ScopeIndex) {
    let graph = TreeGraph::build(tree);

    let mut checked = 0usize;
    let mut counts = PythonUpdateCounts {
        epoch: 0,
        major: 0,
        minor: 0,
        patch: 0,
        qualifier: 0,
        unclassified: 0,
    };
    let mut packages = Vec::new();
    let mut scopes = HashMap::new();
    let sections_known = !tree.roots.is_empty();

    for (id, node) in &tree.resolution {
        // Only real, registry-sourced packages are counted. Group and extra
        // aliases are other views of a package that is already in this loop, the
        // workspace node is not a package at all, and a non-registry source has
        // no newer version to be behind.
        if graph.kinds.get(id.as_str()) != Some(&NodeKind::Package)
            || !is_registry(node.source.as_ref())
        {
            continue;
        }
        let (Some(name), Some(current)) = (node.name.as_deref(), node.version.as_deref()) else {
            continue;
        };

        checked += 1;
        let normalized_name = normalize_package_name(name);
        scopes.insert(normalized_name.clone(), graph.scope(id));

        // uv omits `latest_version` for a package that is already current, so a
        // present-and-different value is the whole outdated signal. "Different"
        // is PEP 440 equality, not string equality: `1.0` and `1.0.0` are the
        // same version, and reporting one as an update available for the other
        // would put a package in `outdated` that nobody can act on.
        let Some(latest) = node.latest_version.as_deref() else {
            continue;
        };
        let update_type = pep440::classify(current, latest);
        if pep440::is_same_version(current, latest) {
            continue;
        }
        match update_type {
            PythonUpdateType::Epoch => counts.epoch += 1,
            PythonUpdateType::Major => counts.major += 1,
            PythonUpdateType::Minor => counts.minor += 1,
            PythonUpdateType::Patch => counts.patch += 1,
            PythonUpdateType::Qualifier => counts.qualifier += 1,
            PythonUpdateType::Unclassified => counts.unclassified += 1,
        }

        packages.push(PythonOutdatedPackage {
            name: normalized_name,
            current: current.to_string(),
            latest: latest.to_string(),
            update_type,
            scope: graph.scope(id),
            // With no roots to walk from, no package can be shown to belong to
            // any section — which is not the same claim as belonging to none.
            // `[]` would assert the latter. uv only reaches here by renaming or
            // dropping `roots`, which `#[serde(default)]` absorbs silently, so
            // this is the one place where upstream drift could turn into a
            // positive falsehood rather than a visible gap.
            groups: sections_known.then(|| {
                graph
                    .groups
                    .get(id.as_str())
                    .map(|labels| labels.iter().cloned().collect())
                    .unwrap_or_default()
            }),
            extras: Some(
                graph
                    .extras
                    .get(id.as_str())
                    .map(|names| names.iter().cloned().collect())
                    .unwrap_or_default(),
            ),
            // uv attaches an environment marker to a dependency *edge*, and only
            // under `--universal`. This adapter runs uv's default,
            // platform-filtered resolution, where uv has already evaluated every
            // marker and reports none — so there is nothing to report, and a
            // package reached by several edges could not be described by one
            // expression anyway.
            marker: PythonMarker::NotReported,
        });
    }

    packages.sort_by(|left, right| left.name.cmp(&right.name));

    (
        PythonOutdatedReport {
            checked,
            outdated: packages.len(),
            counts,
            packages,
        },
        ScopeIndex { scopes },
    )
}

// ===== `uv audit --output-format json` =====

/// The subset of `uv audit --output-format json` this adapter reads.
#[derive(Debug, Default, Deserialize)]
pub struct UvAudit {
    #[serde(default)]
    vulnerabilities: Vec<UvVulnerability>,
    #[serde(default)]
    adverse_statuses: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct UvVulnerability {
    id: String,
    #[serde(default)]
    aliases: Option<Vec<String>>,
    /// uv's short human summary, which is genuinely `null` for a good share of
    /// advisories. Mapped to the schema's `title`.
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    fix_versions: Option<Vec<String>>,
    dependency: UvAuditDependency,
}

#[derive(Debug, Deserialize)]
struct UvAuditDependency {
    name: String,
    version: String,
}

/// Normalizes a parsed `uv audit` document, returning the report and any
/// warnings the payload should carry.
pub fn normalize_audit(
    audit: &UvAudit,
    scopes: &ScopeIndex,
) -> (PythonSecurityReport, Vec<String>) {
    let findings: Vec<PythonVulnerability> = audit
        .vulnerabilities
        .iter()
        .map(|vulnerability| PythonVulnerability {
            id: vulnerability.id.clone(),
            aliases: vulnerability.aliases.clone(),
            package: normalize_package_name(&vulnerability.dependency.name),
            installed_version: vulnerability.dependency.version.clone(),
            // `uv audit` publishes no severity for any finding, so every one is
            // `unknown`. That is not a parsing gap to be improved later: the
            // schema carries `unknown` precisely so a missing severity is never
            // downgraded into `low`, where a gate would let it through.
            severity: PythonSeverity::Unknown,
            title: vulnerability
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|summary| !summary.is_empty())
                .map(str::to_string),
            scope: scopes.get(&vulnerability.dependency.name),
            fixed_versions: vulnerability.fix_versions.clone(),
        })
        .collect();

    let mut warnings = Vec::new();
    if !findings.is_empty() {
        warnings.push(
            "uv audit publishes no severity, so every finding is reported as `unknown`; an \
             `unknown` severity satisfies every --fail-on-vulnerability threshold"
                .to_string(),
        );
    }
    if !audit.adverse_statuses.is_empty() {
        // Yanked and withdrawn releases are not vulnerabilities and the schema
        // has no field for them. Dropping the count silently would hide
        // something uv went and looked up.
        warnings.push(format!(
            "uv audit reported {} package(s) with an adverse status (such as a yanked release); \
             the schema carries vulnerabilities only, so run `uv audit` directly for those",
            audit.adverse_statuses.len()
        ));
    }

    let summary = PythonSecuritySummary {
        critical: 0,
        high: 0,
        moderate: 0,
        low: 0,
        unknown: findings.len(),
        total: findings.len(),
    };

    (PythonSecurityReport { summary, findings }, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Loads a committed `uv` capture.
    ///
    /// Read at runtime rather than `include_str!`ed, because `tests/fixtures/**`
    /// is excluded from the published crate and a compile-time include would
    /// make `src/` unbuildable from the package — the trap that keeps
    /// `tests/release_policy.rs` excluded alongside `cliff.toml`.
    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("uv")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing uv fixture {}: {err}", path.display()))
    }

    fn tree_fixture() -> UvTree {
        serde_json::from_str(&fixture("tree-outdated.json")).expect("parse tree fixture")
    }

    fn audit_fixture() -> UvAudit {
        serde_json::from_str(&fixture("audit.json")).expect("parse audit fixture")
    }

    fn package<'a>(report: &'a PythonOutdatedReport, name: &str) -> &'a PythonOutdatedPackage {
        report
            .packages
            .iter()
            .find(|package| package.name == name)
            .unwrap_or_else(|| panic!("{name} missing from the outdated report"))
    }

    /// The denominator counts real packages once each.
    ///
    /// uv's `resolution` is not a package list: the fixture holds three nodes for
    /// the workspace member (base, `dev` group, `extra-feature` extra), one alias
    /// node for `requests[socks]`, and a `workspace` node. Counting nodes instead
    /// of packages inflates `checked` by five and reports `requests` twice.
    #[test]
    fn checked_counts_packages_not_graph_nodes() {
        let (report, _) = normalize_tree_with_scopes(&tree_fixture());

        assert_eq!(
            report.checked, 12,
            "expected the twelve registry packages, not the seventeen resolution nodes"
        );
        assert_eq!(
            report
                .packages
                .iter()
                .filter(|package| package.name == "requests")
                .count(),
            1,
            "`requests` and `requests[socks]` are one package"
        );
        assert!(
            !report
                .packages
                .iter()
                .any(|package| package.name == "demo-app"),
            "the editable workspace member has no registry version to be behind"
        );
    }

    /// The summary invariants `docs/python-schema.md` states as contracts.
    #[test]
    fn summaries_agree_with_their_entries() {
        let (report, _) = normalize_tree_with_scopes(&tree_fixture());
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

    /// A package uv reports no `latest_version` for is current, not unchecked.
    #[test]
    fn packages_without_a_latest_version_are_not_outdated() {
        let (report, _) = normalize_tree_with_scopes(&tree_fixture());

        for current in ["certifi", "pyparsing", "pysocks"] {
            assert!(
                !report
                    .packages
                    .iter()
                    .any(|package| package.name == current),
                "{current} carries no latest_version in the fixture and is up to date"
            );
        }
        assert_eq!(report.outdated, 9);
    }

    /// Direct versus transitive comes from the graph, and an extra alias must not
    /// promote the extra's own dependencies to direct.
    ///
    /// The member depends on `requests[socks]`, whose alias node lists `pysocks`.
    /// Resolving that edge to `requests` without folding the alias's dependencies
    /// into `requests` makes `pysocks` look like a direct dependency of the
    /// project, which it is not.
    #[test]
    fn scope_follows_the_dependency_graph() {
        let (report, scopes) = normalize_tree_with_scopes(&tree_fixture());

        for direct in ["requests", "jinja2", "pyyaml", "click", "packaging"] {
            assert_eq!(
                package(&report, direct).scope,
                PythonDependencyScope::Direct,
                "{direct} is declared by the project"
            );
        }
        for transitive in ["urllib3", "idna", "chardet", "markupsafe"] {
            assert_eq!(
                package(&report, transitive).scope,
                PythonDependencyScope::Transitive,
                "{transitive} is only reached through another package"
            );
        }

        // pysocks is up to date so it is not in `packages`, but the scope index
        // still has to place it correctly for the audit join.
        assert_eq!(scopes.get("pysocks"), PythonDependencyScope::Transitive);
    }
    /// With no roots there is nothing to walk from, so no package can be *shown*
    /// to belong to a section. `[]` would claim it belongs to none, which is a
    /// different and false statement. uv reaches this only by renaming or
    /// dropping `roots` — absorbed silently by `#[serde(default)]` — and it is
    /// the one spot where that drift could produce a positive falsehood instead
    /// of a visible gap.
    #[test]
    fn no_roots_reports_sections_as_unknown_rather_than_empty() {
        let mut tree = tree_fixture();
        tree.roots.clear();
        let (report, _) = normalize_tree_with_scopes(&tree);

        assert!(
            !report.packages.is_empty(),
            "fixture premise: packages are still resolved without roots"
        );
        for package in &report.packages {
            assert_eq!(
                package.groups, None,
                "{}: sections are unknown, not empty",
                package.name
            );
        }
    }

    /// Sections come from the roots, and a walk must stop at another root.
    ///
    /// `demo-app[extra-feature]` depends on `demo-app`, so a walk that did not
    /// stop at section roots would label every `main` dependency `extra-feature`
    /// as well. Note this exercises the guard in the seed loop; the same guard in
    /// the BFS body is not reached by this fixture, because the extra root's edge
    /// to the member base is caught by the seed loop first.
    #[test]
    fn groups_name_the_sections_that_reach_a_package() {
        let (report, _) = normalize_tree_with_scopes(&tree_fixture());

        assert_eq!(
            package(&report, "jinja2").groups.as_deref(),
            Some(&["main".to_string()][..])
        );
        assert_eq!(
            package(&report, "click").groups.as_deref(),
            Some(&["dev".to_string()][..])
        );
        assert_eq!(
            package(&report, "packaging").groups.as_deref(),
            Some(&["extra-feature".to_string()][..])
        );
        assert_eq!(
            package(&report, "markupsafe").groups.as_deref(),
            Some(&["main".to_string()][..]),
            "markupsafe is reached through jinja2, not through the docs group"
        );
    }

    /// Extras are reported, and belong to the package that carries them.
    ///
    /// Issue #71 assumed uv's JSON has no extras anywhere. It does: an activated
    /// extra appears as an alias node and in the carrying package's
    /// `optional_dependencies`, so `requests` reports `["socks"]` rather than the
    /// `null` an absent source would produce.
    #[test]
    fn extras_are_sourced_from_the_alias_nodes() {
        let (report, _) = normalize_tree_with_scopes(&tree_fixture());

        assert_eq!(
            package(&report, "requests").extras.as_deref(),
            Some(&["socks".to_string()][..])
        );
        assert_eq!(
            package(&report, "jinja2").extras.as_deref(),
            Some(&[][..]),
            "reported-and-empty, not null: uv does report extras"
        );
    }

    /// Markers are the field uv genuinely does not report in this mode.
    ///
    /// uv attaches a marker to a dependency edge and only under `--universal`,
    /// which this adapter does not pass. `not_reported` is the honest answer, and
    /// is a different claim from `absent`.
    #[test]
    fn markers_are_reported_as_unavailable_rather_than_absent() {
        let (report, _) = normalize_tree_with_scopes(&tree_fixture());
        assert!(report
            .packages
            .iter()
            .all(|package| package.marker == PythonMarker::NotReported));
    }

    /// Classification runs on the real capture, not only on the unit table.
    #[test]
    fn update_types_classify_the_captured_versions() {
        let (report, _) = normalize_tree_with_scopes(&tree_fixture());

        // 2.11.2 -> 3.1.6, 1.23 -> 2.7.0, 7.0 -> 8.5.0: all first-component.
        for major in ["jinja2", "urllib3", "click"] {
            assert_eq!(package(&report, major).update_type, PythonUpdateType::Major);
        }
        assert_eq!(report.counts.major, 8);
        assert_eq!(report.counts.unclassified, 0);
    }

    /// Every `uv audit` finding is `unknown`, and the summary says so.
    #[test]
    fn audit_findings_carry_no_severity() {
        let (_, scopes) = normalize_tree_with_scopes(&tree_fixture());
        let (report, warnings) = normalize_audit(&audit_fixture(), &scopes);

        assert_eq!(report.summary.total, 6);
        assert_eq!(report.summary.unknown, 6);
        assert_eq!(
            report.summary.critical
                + report.summary.high
                + report.summary.moderate
                + report.summary.low,
            0,
            "uv publishes no severity, so no finding may land in a graded bucket"
        );
        assert!(report
            .findings
            .iter()
            .all(|finding| finding.severity == PythonSeverity::Unknown));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("no severity")),
            "a payload of nothing but `unknown` needs the reason on it: {warnings:?}"
        );
    }

    /// The audit join borrows scope from the tree, and reports `unknown` when
    /// there is no tree to borrow from.
    #[test]
    fn audit_scope_comes_from_the_tree_when_there_is_one() {
        let (_, scopes) = normalize_tree_with_scopes(&tree_fixture());
        let audit = audit_fixture();

        let (joined, _) = normalize_audit(&audit, &scopes);
        let scope_of = |package: &str| {
            joined
                .findings
                .iter()
                .find(|finding| finding.package == package)
                .unwrap_or_else(|| panic!("no finding for {package}"))
                .scope
        };
        assert_eq!(scope_of("jinja2"), PythonDependencyScope::Direct);
        assert_eq!(scope_of("urllib3"), PythonDependencyScope::Transitive);
        assert_eq!(scope_of("click"), PythonDependencyScope::Direct);

        // Without the tree there is no graph, and guessing would be worse than
        // saying so.
        let (unjoined, _) = normalize_audit(&audit, &ScopeIndex::default());
        assert!(unjoined
            .findings
            .iter()
            .all(|finding| finding.scope == PythonDependencyScope::Unknown));
    }

    /// The set-valued fields keep the `null` versus `[]` distinction the schema
    /// rests on.
    #[test]
    fn audit_set_valued_fields_pass_through_unreported_as_null() {
        let (_, scopes) = normalize_tree_with_scopes(&tree_fixture());
        let (report, _) = normalize_audit(&audit_fixture(), &scopes);

        let finding = report
            .findings
            .iter()
            .find(|finding| finding.id == "PYSEC-2026-2132")
            .expect("the click advisory");
        assert_eq!(
            finding.title, None,
            "uv reports a null summary for this one"
        );
        assert_eq!(finding.aliases.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            finding.fixed_versions.as_deref(),
            Some(&["8.3.3".to_string()][..])
        );

        // A source that reported nothing at all must stay `null`, not become [].
        let empty: UvAudit = serde_json::from_str(
            r#"{"vulnerabilities":[{"id":"X","dependency":{"name":"pkg","version":"1.0"}}]}"#,
        )
        .expect("parse minimal audit");
        let (report, _) = normalize_audit(&empty, &ScopeIndex::default());
        assert_eq!(report.findings[0].aliases, None);
        assert_eq!(report.findings[0].fixed_versions, None);
    }

    /// A clean audit is an empty findings list, never a missing report.
    #[test]
    fn a_clean_audit_reports_zero_findings_and_no_severity_warning() {
        let clean: UvAudit = serde_json::from_str(
            r#"{"schema":{"version":"preview"},"summary":{"audited_packages":3,"vulnerabilities":0,"adverse_statuses":0},"vulnerabilities":[],"adverse_statuses":[]}"#,
        )
        .expect("parse clean audit");
        let (report, warnings) = normalize_audit(&clean, &ScopeIndex::default());

        assert_eq!(report.summary.total, 0);
        assert!(report.findings.is_empty());
        assert!(
            warnings.is_empty(),
            "a run with no findings has no severity caveat to give: {warnings:?}"
        );
    }

    /// An adverse status is reported rather than dropped, because the schema has
    /// no field for one.
    #[test]
    fn adverse_statuses_are_surfaced_as_a_warning() {
        let audit: UvAudit = serde_json::from_str(
            r#"{"vulnerabilities":[],"adverse_statuses":[{"kind":"yanked"},{"kind":"yanked"}]}"#,
        )
        .expect("parse audit with adverse statuses");
        let (_, warnings) = normalize_audit(&audit, &ScopeIndex::default());

        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains('2'),
            "unexpected warning: {warnings:?}"
        );
    }

    // ===== capability probing =====

    /// Verbatim stderr from `uv 0.12.8`, captured by running the probe.
    const CURRENT_TREE_PROBE: &str = "error: invalid value 'cargo-upkeep-capability-probe' for '--format <FORMAT>'\n  [possible values: text, json]\n\nFor more information, try '--help'.\n";
    const CURRENT_AUDIT_PROBE: &str = "error: invalid value 'cargo-upkeep-capability-probe' for '--output-format <OUTPUT_FORMAT>'\n  [possible values: text, json, sarif]\n\nFor more information, try '--help'.\n";

    /// Verbatim stderr from `uv 0.7.11`, captured by running the same probe
    /// against a downloaded 0.7.11 binary. 0.7.11 has no `audit` subcommand at
    /// all, and its `uv tree` has no `--format`.
    const LEGACY_AUDIT_PROBE: &str =
        "error: unrecognized subcommand 'audit'\n\nUsage: uv [OPTIONS] <COMMAND>\n\nFor more information, try '--help'.\n";
    const LEGACY_TREE_PROBE: &str =
        "error: unexpected argument '--format' found\n\nUsage: uv tree [OPTIONS]\n\nFor more information, try '--help'.\n";

    fn outdated_probe(stderr: &str, succeeded: bool) -> Capability {
        probe_capability(stderr, succeeded, &TREE_ARGS, "tree", "--format")
    }

    fn security_probe(stderr: &str, succeeded: bool) -> Capability {
        probe_capability(stderr, succeeded, &AUDIT_ARGS, "audit", "--output-format")
    }

    fn unavailable(capability: Capability) -> (PythonUnavailableReason, String) {
        match capability {
            Capability::Available => panic!("expected the capability to be unavailable"),
            Capability::Unavailable { reason, detail } => (reason, detail),
        }
    }

    /// The current profile: both capabilities probe as available.
    #[test]
    fn current_uv_advertises_json_for_both_capabilities() {
        assert!(matches!(
            outdated_probe(CURRENT_TREE_PROBE, false),
            Capability::Available
        ));
        assert!(matches!(
            security_probe(CURRENT_AUDIT_PROBE, false),
            Capability::Available
        ));
    }

    /// The pre-`audit` profile, which is the case #72 was filed for.
    ///
    /// A uv this old must produce an unavailable capability with an upgrade hint,
    /// never a clean security result — "no vulnerabilities" out of a scanner that
    /// does not exist is the #10/#34 defaulted-to-healthy bug in a new place.
    #[test]
    fn legacy_uv_reports_missing_capabilities_with_an_upgrade_hint() {
        let (reason, detail) = unavailable(security_probe(LEGACY_AUDIT_PROBE, false));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
        assert!(detail.contains("no `audit` subcommand"), "{detail}");
        assert!(detail.contains("uv self update"), "{detail}");

        let (reason, detail) = unavailable(outdated_probe(LEGACY_TREE_PROBE, false));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
        assert!(detail.contains("--format"), "{detail}");
        assert!(detail.contains("uv self update"), "{detail}");
    }

    /// A uv whose format flag exists but offers no JSON is a capability gap, not
    /// a success.
    ///
    /// This is the 0.11.x window where `uv audit` shipped before its JSON output
    /// did. A probe that only checked for the flag's existence would report the
    /// capability as available and then fail on unparseable text.
    #[test]
    fn a_format_flag_without_json_is_a_capability_gap() {
        let stderr = "error: invalid value 'cargo-upkeep-capability-probe' for '--output-format <OUTPUT_FORMAT>'\n  [possible values: text]\n";
        let (reason, detail) = unavailable(security_probe(stderr, false));
        assert_eq!(reason, PythonUnavailableReason::NotInstalled);
        assert!(detail.contains("accepts only text"), "{detail}");
    }

    /// uv wraps its error output to the terminal width, and clap wraps at
    /// whitespace — including inside the `invalid value '…'` phrase the probe
    /// looks for. A narrow terminal must not turn a supported capability into an
    /// unavailable one.
    #[test]
    fn a_wrapped_possible_values_list_is_still_read() {
        let stderr = "error: invalid value\n  'cargo-upkeep-capability-probe' for\n  '--output-format <OUTPUT_FORMAT>'\n  [possible values: text,\n  json, sarif]\n";
        assert!(matches!(
            security_probe(stderr, false),
            Capability::Available
        ));
    }

    /// An unrelated argument error elsewhere in the buffer is not an answer about
    /// our flag.
    ///
    /// The same false positive `is_unknown_flag` is structured to avoid: the list
    /// has to follow a rejection of our own probe value for our own flag.
    #[test]
    fn a_possible_values_list_for_another_flag_is_not_an_answer() {
        let stderr = "error: invalid value 'x' for '--service-format <SERVICE_FORMAT>'\n  [possible values: osv, json]\n";
        let (reason, _) = unavailable(security_probe(stderr, false));
        assert_eq!(
            reason,
            PythonUnavailableReason::Failed,
            "a list about a different flag establishes nothing"
        );
    }

    /// A probe uv *accepted* teaches nothing, and must not read as available.
    #[test]
    fn an_accepted_probe_value_is_inconclusive() {
        let (reason, detail) = unavailable(security_probe("", true));
        assert_eq!(reason, PythonUnavailableReason::Failed);
        assert!(detail.contains("accepted the probe value"), "{detail}");
    }

    /// `uv --version` is read for the report, never for a capability decision.
    #[test]
    fn version_is_parsed_from_uv_version_output() {
        assert_eq!(
            parse_version("uv 0.12.8 (68209e5c6 2026-08-31 aarch64-apple-darwin)\n").as_deref(),
            Some("0.12.8")
        );
        assert_eq!(
            parse_version("uv 0.7.11 (90a4416ab 2025-06-04)\n").as_deref(),
            Some("0.7.11")
        );
        assert_eq!(parse_version("something else\n"), None);
        assert_eq!(parse_version(""), None);
    }

    /// A preview schema is allowed to grow. An unrecognized `kind` must drop the
    /// node rather than fail the run.
    #[test]
    fn an_unrecognized_node_kind_is_not_a_parse_failure() {
        let tree: UvTree = serde_json::from_str(
            r#"{"roots":[],"resolution":{
                "a==1.0@registry+https://pypi.org/simple":{"name":"a","version":"1.0","kind":{"tomorrow":"x"},"source":{"registry":{"url":"https://pypi.org/simple"}}},
                "b==1.0@registry+https://pypi.org/simple":{"name":"b","version":"1.0","latest_version":"2.0","kind":"package","source":{"registry":{"url":"https://pypi.org/simple"}}}
            }}"#,
        )
        .expect("an unknown kind must still deserialize");

        let (report, _) = normalize_tree_with_scopes(&tree);
        assert_eq!(report.checked, 1, "only the recognized package is counted");
        assert_eq!(report.outdated, 1);
        assert_eq!(report.packages[0].name, "b");
    }

    /// Names are PEP 503 normalized on both sides, so the audit join lines up
    /// however the two commands happen to spell a name.
    #[test]
    fn package_names_are_normalized_on_both_sides() {
        let tree: UvTree = serde_json::from_str(
            r#"{"roots":[],"resolution":{
                "zope.interface==5.0@registry+https://pypi.org/simple":{"name":"Zope.Interface","version":"5.0","latest_version":"6.0","kind":"package","source":{"registry":{"url":"https://pypi.org/simple"}}}
            }}"#,
        )
        .expect("parse tree");
        let (report, scopes) = normalize_tree_with_scopes(&tree);
        assert_eq!(report.packages[0].name, "zope-interface");

        let audit: UvAudit = serde_json::from_str(
            r#"{"vulnerabilities":[{"id":"X","dependency":{"name":"Zope_Interface","version":"5.0"}}]}"#,
        )
        .expect("parse audit");
        let (security, _) = normalize_audit(&audit, &scopes);
        assert_eq!(security.findings[0].package, "zope-interface");
    }

    #[test]
    fn project_root_is_found_by_walking_up() {
        let temp = tempfile::tempdir().expect("temp dir");
        let nested = temp.path().join("src").join("deep");
        std::fs::create_dir_all(&nested).expect("create nested");

        assert_eq!(find_project_root(&nested), None);

        std::fs::write(temp.path().join("pyproject.toml"), "[project]\n").expect("write manifest");
        assert_eq!(find_project_root(&nested).as_deref(), Some(temp.path()));
    }
}
