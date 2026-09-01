//! Output types and serialization helpers.

use serde::Serialize;
use std::collections::HashMap;
use std::fmt;

use crate::core::error::Result;

#[derive(Debug, Serialize)]
pub struct DetectOutput {
    pub edition: Option<String>,
    pub msrv: Option<String>,
    pub workspace: bool,
    pub members: Vec<String>,
    pub package: Option<String>,
    pub version: Option<String>,
    pub dependencies: usize,
    pub features: Vec<String>,
    pub targets: Vec<String>,
    pub tooling: Vec<String>,
    pub ci: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DepsOutput {
    /// Every dependency *edge*: one per declaration by one workspace member.
    ///
    /// Raw declarations, with no deduplication and no regard for kind. A crate
    /// listed in both `[dependencies]` and `[dev-dependencies]` counts twice, and
    /// a crate declared by three workspace members counts three times.
    ///
    /// **This is not the unit of [`DepsOutput::outdated`] or
    /// [`DepsOutput::checked`]**, and it is not a denominator. See `checked`.
    pub total: usize,
    /// Dependency units the freshness question was actually settled for.
    ///
    /// Counted in *groups*, the same unit as [`DepsOutput::outdated`]: edges
    /// merged by `(name, resolved version)`, so the crate that counts twice in
    /// [`DepsOutput::total`] counts once here. `checked <= total` always, and the
    /// difference is *not* the skip count — it is mostly re-declaration.
    ///
    /// Concretely this is every resolved group whose latest version was fetched
    /// and compared, plus every dependency skipped as *not applicable* rather
    /// than unmeasured (`NonRegistry`, `TargetSpecific`, `OptionalNotActivated`):
    /// a git or path dependency has no registry release to be behind, so it is
    /// neither outdated nor unchecked. Groups the registry could not answer for
    /// (`RegistryUnavailable`, `RegistryMetadataMissing`, `UnsupportedRegistry`,
    /// `MissingResolve`, `AmbiguousResolve`) are excluded — those are comparisons
    /// that were owed and never made.
    ///
    /// `outdated <= checked` holds, which is what makes `(checked - outdated) /
    /// checked` a meaningful freshness ratio. Deriving that denominator as
    /// `total - skipped` does not hold: it subtracts a group count from an edge
    /// count and credits comparisons that never happened.
    pub checked: usize,
    pub outdated: usize,
    pub major: usize,
    pub minor: usize,
    pub patch: usize,
    pub packages: Vec<OutdatedPackage>,
    pub skipped: usize,
    pub skipped_packages: Vec<SkippedDependency>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<DepsSecurityOutput>,
    pub workspace: bool,
    pub members: Vec<String>,
    pub skipped_members: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DepsSecurityOutput {
    pub summary: AuditSummary,
    pub packages: Vec<DepsSecurityPackage>,
}

#[derive(Debug, Serialize)]
pub struct DepsSecurityPackage {
    pub name: String,
    pub alias: Option<String>,
    pub current: String,
    pub dependency_type: DependencyType,
    /// Workspace members that declared this dependency at this resolved version,
    /// sorted and deduplicated. Rows for one crate at two versions are distinct
    /// entries, so this is what says which member owns the vulnerable one.
    pub members: Vec<String>,
    pub vulnerabilities: Vec<DepsSecurityVulnerability>,
}

#[derive(Debug, Serialize)]
pub struct DepsSecurityVulnerability {
    pub advisory_id: String,
    pub severity: Severity,
    pub title: String,
    pub fix_available: bool,
}

#[derive(Debug, Serialize)]
pub struct UnusedOutput {
    pub unused: Vec<UnusedDep>,
    pub possibly_unused: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UnsafeOutput {
    pub summary: UnsafeSummary,
    pub packages: Vec<UnsafePackage>,
}

#[derive(Debug, Serialize)]
pub struct AuditOutput {
    pub vulnerabilities: Vec<Vulnerability>,
    /// Non-vulnerability findings such as informational advisories and yanked
    /// resolved crate versions. These do not contribute to `summary`.
    pub warnings: Vec<AuditWarning>,
    pub summary: AuditSummary,
}

#[derive(Debug, Serialize)]
pub struct AuditWarning {
    pub kind: AuditWarningKind,
    pub package: String,
    pub package_version: String,
    pub advisory_id: Option<String>,
    pub title: Option<String>,
    pub path: Vec<String>,
    /// Whether the advisory names a patched version. Yanked findings leave
    /// this `None`: a yank alone does not establish a safe replacement.
    pub fix_available: Option<bool>,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum AuditWarningKind {
    Notice,
    Unmaintained,
    Unsound,
    Yanked,
}

impl fmt::Display for AuditWarningKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Notice => "notice",
            Self::Unmaintained => "unmaintained",
            Self::Unsound => "unsound",
            Self::Yanked => "yanked",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Serialize)]
pub struct TreeOutput {
    pub root: TreeNode,
    pub stats: TreeStats,
}

#[derive(Debug, Serialize)]
pub struct TreeNode {
    pub name: String,
    pub version: String,
    pub package_id: String,
    pub features: Vec<String>,
    pub dependencies: Vec<TreeNode>,
    pub is_dev: bool,
    pub is_build: bool,
    pub duplicate: bool,
}

#[derive(Debug, Serialize)]
pub struct TreeStats {
    pub total_crates: usize,
    pub direct_deps: usize,
    pub transitive_deps: usize,
    pub duplicate_crates: usize,
}

#[derive(Debug, Serialize)]
pub struct AuditSummary {
    pub critical: usize,
    pub high: usize,
    pub moderate: usize,
    pub low: usize,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct UnsafeSummary {
    pub packages: usize,
    pub unsafe_functions: usize,
    pub unsafe_impls: usize,
    pub unsafe_traits: usize,
    pub unsafe_blocks: usize,
    pub unsafe_expressions: usize,
    pub total_unsafe: usize,
}

/// A project health grade over the metrics that could actually be measured.
///
/// Metrics whose analyzer did not run are **excluded** rather than assumed
/// healthy: they carry a `None` score in `breakdown`, are listed in
/// `unavailable`, and their weight is removed from the denominator. `score` is
/// therefore always "of what we could measure", and `complete` says whether
/// that was everything. Consumers that treat a grade as a health gate must
/// check `complete` — an `A` over 40% of the weight is not an `A`.
#[derive(Debug, Serialize)]
pub struct QualityOutput {
    /// Weighted mean of the measured metrics, rescaled to 0-100.
    ///
    /// `None` when no metric could be measured at all, because there is then no
    /// honest number to report. Serializes as JSON `null`.
    pub score: Option<f32>,
    /// Grade for `score`, and `None` for the same reason.
    pub grade: Option<Grade>,
    /// True only when every metric was measured.
    pub complete: bool,
    /// Fraction of the total metric weight that was measured, from 0.0 to 1.0.
    ///
    /// This is the denominator `score` was divided by, exposed so a caller can
    /// judge how much the grade is actually standing on.
    pub measured_weight: f32,
    pub breakdown: Vec<MetricScore>,
    /// Metrics that did not run, with why. Empty when `complete` is true.
    pub unavailable: Vec<UnavailableMetric>,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct MetricScore {
    pub name: String,
    /// `None` when this metric could not be measured. Never substitute a value
    /// here — a defaulted `100.0` is indistinguishable from a real perfect
    /// score, which is the bug this type exists to prevent.
    pub score: Option<f32>,
    pub weight: f32,
}

/// A metric that could not be measured, and why.
#[derive(Debug, Serialize)]
pub struct UnavailableMetric {
    pub name: String,
    /// The weight this metric would have carried, had it run.
    pub weight: f32,
    pub reason: UnavailableReason,
    /// Human-readable explanation, including an install hint when the cause is
    /// a missing tool.
    pub detail: String,
}

/// Why a quality metric could not be measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// The analyzer's external tool is not installed, or is installed but too
    /// old to produce output this crate can read. Actionable by the user — in
    /// both cases by running the tool's install command — and not a sign of
    /// anything wrong with the project.
    NotInstalled,
    /// The analyzer ran but failed. Something is genuinely broken.
    Failed,
}

impl fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            UnavailableReason::NotInstalled => "not installed",
            UnavailableReason::Failed => "failed",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Serialize)]
pub struct ClippyOutput {
    pub warnings: usize,
    pub errors: usize,
    pub warnings_by_lint: HashMap<String, usize>,
    pub details: Vec<ClippyIssue>,
    pub score: f32,
}

#[derive(Debug, Serialize)]
pub struct ClippyIssue {
    pub lint: String,
    pub level: String,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct OutdatedPackage {
    pub name: String,
    pub alias: Option<String>,
    pub current: String,
    pub latest: String,
    pub required: String,
    pub update_type: UpdateType,
    pub dependency_type: DependencyType,
    /// Workspace members that declared this dependency edge, sorted and deduplicated.
    ///
    /// Always populated: for a single-crate project this is the crate's own name.
    /// Entries are grouped by `(name, current)`, so a workspace whose members resolve
    /// the same crate to semver-incompatible versions produces one entry per version,
    /// each naming only the members that resolved to it.
    pub members: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UnusedDep {
    pub name: String,
    pub dependency_type: DependencyType,
    pub confidence: Confidence,
}

#[derive(Debug, Serialize)]
pub struct UnsafePackage {
    pub name: String,
    pub version: String,
    pub package_id: Option<String>,
    pub unsafe_functions: usize,
    pub unsafe_impls: usize,
    pub unsafe_traits: usize,
    pub unsafe_blocks: usize,
    pub unsafe_expressions: usize,
    pub total_unsafe: usize,
}

#[derive(Debug, Serialize)]
pub struct SkippedDependency {
    pub name: String,
    pub alias: Option<String>,
    pub required: String,
    pub reason: SkipReason,
    pub dependency_type: DependencyType,
    pub source: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Vulnerability {
    pub id: String,
    pub package: String,
    pub package_version: String,
    pub severity: Severity,
    pub title: String,
    pub path: Vec<String>,
    pub fix_available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Moderate,
    Low,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateType {
    Major,
    Minor,
    Patch,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum DependencyType {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    NonRegistry,
    MissingResolve,
    /// The declared package name resolved to several distinct versions within one
    /// member and the edge matched none of the resolve-graph keys that would say
    /// which instance it is. Reporting either version would be a coin flip, so the
    /// edge is skipped instead.
    AmbiguousResolve,
    OptionalNotActivated,
    TargetSpecific,
    RegistryMetadataMissing,
    RegistryUnavailable,
    UnsupportedRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Grade {
    A,
    B,
    C,
    D,
    F,
}

/// Confidence level for detection results.
///
/// Higher confidence indicates more reliable detection:
/// - `High`: JSON-parsed output from tools with structured output
/// - `Medium`: Text-parsed output from tools without structured output
/// - `Low`: Heuristic-based detection or fallback parsing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        };
        write!(f, "{label}")
    }
}

pub fn print_json<T: Serialize>(output: &T) -> Result<()> {
    let payload = serde_json::to_string_pretty(output)?;
    println!("{payload}");
    Ok(())
}

impl fmt::Display for DetectOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let members = if self.members.is_empty() {
            "none".to_string()
        } else {
            self.members.join(", ")
        };
        let features = if self.features.is_empty() {
            "none".to_string()
        } else {
            self.features.join(", ")
        };
        let targets = if self.targets.is_empty() {
            "none".to_string()
        } else {
            self.targets.join(", ")
        };
        let tooling = if self.tooling.is_empty() {
            "none".to_string()
        } else {
            self.tooling.join(", ")
        };
        let ci = if self.ci.is_empty() {
            "none".to_string()
        } else {
            self.ci.join(", ")
        };

        writeln!(
            f,
            "Package: {}",
            self.package.as_deref().unwrap_or("unknown")
        )?;
        writeln!(
            f,
            "Version: {}",
            self.version.as_deref().unwrap_or("unknown")
        )?;
        writeln!(
            f,
            "Edition: {}",
            self.edition.as_deref().unwrap_or("unknown")
        )?;
        writeln!(f, "MSRV: {}", self.msrv.as_deref().unwrap_or("unknown"))?;
        writeln!(f, "Workspace: {}", self.workspace)?;
        writeln!(f, "Members: {}", members)?;
        writeln!(f, "Dependencies: {}", self.dependencies)?;
        writeln!(f, "Features: {}", features)?;
        writeln!(f, "Targets: {}", targets)?;
        writeln!(f, "Tooling: {}", tooling)?;
        writeln!(f, "CI: {}", ci)?;
        Ok(())
    }
}

impl fmt::Display for DepsOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Total: {}", self.total)?;
        writeln!(f, "Outdated: {}", self.outdated)?;
        writeln!(f, "Major: {}", self.major)?;
        writeln!(f, "Minor: {}", self.minor)?;
        writeln!(f, "Patch: {}", self.patch)?;
        writeln!(f, "Skipped: {}", self.skipped)?;
        if !self.warnings.is_empty() {
            writeln!(f, "Warnings: {}", self.warnings.join("; "))?;
        }
        if let Some(security) = &self.security {
            writeln!(
                f,
                "Security: {} vulnerabilities (critical {}, high {}, moderate {}, low {})",
                security.summary.total,
                security.summary.critical,
                security.summary.high,
                security.summary.moderate,
                security.summary.low
            )?;
            if security.packages.is_empty() {
                writeln!(f, "Vulnerable dependencies: none")?;
            } else {
                writeln!(f, "Vulnerable dependencies:")?;
                for package in &security.packages {
                    let display_name = match package.alias.as_deref() {
                        Some(alias) => format!("{} ({})", alias, package.name),
                        None => package.name.clone(),
                    };
                    for vulnerability in &package.vulnerabilities {
                        let fix_label = if vulnerability.fix_available {
                            "fix available"
                        } else {
                            "no fix"
                        };
                        writeln!(
                            f,
                            "- {} ({}) current {}: {} ({}, {})",
                            display_name,
                            package.dependency_type,
                            package.current,
                            vulnerability.advisory_id,
                            vulnerability.severity,
                            fix_label
                        )?;
                    }
                }
            }
        }
        writeln!(f, "Workspace: {}", self.workspace)?;
        if self.workspace {
            let members = if self.members.is_empty() {
                "none".to_string()
            } else {
                self.members.join(", ")
            };
            writeln!(f, "Members: {}", members)?;
            if !self.skipped_members.is_empty() {
                writeln!(f, "Skipped members: {}", self.skipped_members.join(", "))?;
            }
        }
        if self.packages.is_empty() {
            writeln!(f, "Outdated packages: none")?;
        } else {
            writeln!(f, "Outdated packages:")?;
            for package in &self.packages {
                let display_name = match package.alias.as_deref() {
                    Some(alias) => format!("{} ({})", alias, package.name),
                    None => package.name.clone(),
                };
                // The member is shown because one crate can legitimately appear on
                // several rows at different resolved versions, and the member is what
                // a reader has to go and edit.
                let members = if package.members.is_empty() {
                    "none".to_string()
                } else {
                    package.members.join(", ")
                };
                writeln!(
                    f,
                    "- {} ({}) current {} latest {} required {} members {}",
                    display_name,
                    package.dependency_type,
                    package.current,
                    package.latest,
                    package.required,
                    members
                )?;
            }
        }
        if !self.skipped_packages.is_empty() {
            writeln!(f, "Skipped dependencies:")?;
            for skipped in &self.skipped_packages {
                let source = skipped.source.as_deref().unwrap_or("unknown");
                let target = skipped.target.as_deref().unwrap_or("none");
                let display_name = match skipped.alias.as_deref() {
                    Some(alias) => format!("{} ({})", alias, skipped.name),
                    None => skipped.name.clone(),
                };
                writeln!(
                    f,
                    "- {} ({}, type {}, required {}, source {}, target {})",
                    display_name,
                    skipped.reason,
                    skipped.dependency_type,
                    skipped.required,
                    source,
                    target
                )?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for UnusedOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Unused: {}", self.unused.len())?;
        writeln!(f, "Possibly unused: {}", self.possibly_unused.len())?;

        if self.unused.is_empty() {
            writeln!(f, "Unused details: none")?;
        } else {
            writeln!(f, "Unused details:")?;
            for dependency in &self.unused {
                writeln!(
                    f,
                    "- {} ({}, confidence {})",
                    dependency.name, dependency.dependency_type, dependency.confidence
                )?;
            }
        }

        if self.possibly_unused.is_empty() {
            writeln!(f, "Possibly unused details: none")?;
        } else {
            writeln!(f, "Possibly unused details:")?;
            for dependency in &self.possibly_unused {
                writeln!(f, "- {dependency}")?;
            }
        }

        Ok(())
    }
}

impl fmt::Display for UnsafeOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Packages: {}", self.summary.packages)?;
        writeln!(f, "Total unsafe: {}", self.summary.total_unsafe)?;
        writeln!(f, "Unsafe functions: {}", self.summary.unsafe_functions)?;
        writeln!(f, "Unsafe impls: {}", self.summary.unsafe_impls)?;
        writeln!(f, "Unsafe traits: {}", self.summary.unsafe_traits)?;
        writeln!(f, "Unsafe blocks: {}", self.summary.unsafe_blocks)?;
        writeln!(f, "Unsafe expressions: {}", self.summary.unsafe_expressions)?;

        if self.packages.is_empty() {
            writeln!(f, "Details: none")?;
            return Ok(());
        }

        writeln!(f, "Details:")?;
        for package in &self.packages {
            writeln!(
                f,
                "- {} {}: unsafe {} (fn {}, impls {}, traits {}, blocks {}, exprs {})",
                package.name,
                package.version,
                package.total_unsafe,
                package.unsafe_functions,
                package.unsafe_impls,
                package.unsafe_traits,
                package.unsafe_blocks,
                package.unsafe_expressions
            )?;
        }

        Ok(())
    }
}

impl fmt::Display for AuditOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Vulnerabilities: {}", self.summary.total)?;
        writeln!(f, "Critical: {}", self.summary.critical)?;
        writeln!(f, "High: {}", self.summary.high)?;
        writeln!(f, "Moderate: {}", self.summary.moderate)?;
        writeln!(f, "Low: {}", self.summary.low)?;
        if self.vulnerabilities.is_empty() {
            writeln!(f, "Details: none")?;
        } else {
            let mut id_width = "id".len();
            let mut package_width = "package".len();
            let mut version_width = "version".len();
            let mut severity_width = "severity".len();

            for vulnerability in &self.vulnerabilities {
                id_width = id_width.max(vulnerability.id.len());
                package_width = package_width.max(vulnerability.package.len());
                version_width = version_width.max(vulnerability.package_version.len());
                severity_width = severity_width.max(vulnerability.severity.to_string().len());
            }

            writeln!(f, "Details:")?;
            writeln!(
                f,
                "{:<id_width$}  {:<package_width$}  {:<version_width$}  {:<severity_width$}",
                "id",
                "package",
                "version",
                "severity",
                id_width = id_width,
                package_width = package_width,
                version_width = version_width,
                severity_width = severity_width
            )?;
            for vulnerability in &self.vulnerabilities {
                writeln!(
                    f,
                    "{:<id_width$}  {:<package_width$}  {:<version_width$}  {:<severity_width$}",
                    vulnerability.id,
                    vulnerability.package,
                    vulnerability.package_version,
                    vulnerability.severity,
                    id_width = id_width,
                    package_width = package_width,
                    version_width = version_width,
                    severity_width = severity_width
                )?;
            }
        }

        writeln!(f, "Warnings: {}", self.warnings.len())?;
        if self.warnings.is_empty() {
            writeln!(f, "Warning details: none")?;
            return Ok(());
        }

        writeln!(f, "Warning details:")?;
        for warning in &self.warnings {
            let advisory = warning.advisory_id.as_deref().unwrap_or("-");
            let title = warning.title.as_deref().unwrap_or("-");
            let fix = match warning.fix_available {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            };
            writeln!(
                f,
                "{} {} {}: advisory {}; fix available {}; path {}; {}",
                warning.kind,
                warning.package,
                warning.package_version,
                advisory,
                fix,
                warning.path.join(" -> "),
                title
            )?;
        }
        Ok(())
    }
}

/// Column width for the score cell in the text breakdown, wide enough for the
/// literal `unmeasured`.
const QUALITY_SCORE_COLUMN: usize = 10;

impl fmt::Display for QualityOutput {
    /// Renders the report with any incompleteness stated **before** the score.
    ///
    /// The ordering is deliberate: a reader who stops at the first line must
    /// not walk away with a headline grade that stood on metrics which never
    /// ran. Recommendations are printed too — they used to exist only in the
    /// JSON output, so `cargo upkeep quality` silently dropped them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.complete {
            writeln!(
                f,
                "Analysis incomplete: {} of {} metrics could not be measured ({:.0}% of weight measured).",
                self.unavailable.len(),
                self.breakdown.len(),
                self.measured_weight * 100.0
            )?;
            for metric in &self.unavailable {
                writeln!(
                    f,
                    "- {} (weight {:.2}, {}): {}",
                    metric.name, metric.weight, metric.reason, metric.detail
                )?;
            }
            writeln!(f)?;
        }

        match (self.score, self.grade) {
            // Two decimals, matching the JSON contract and the precision the
            // scorer rounds to. Printing fewer digits than the scorer keeps
            // would let a score of 89.96 render as `90.0` beside `Grade: B`.
            (Some(score), Some(grade)) if self.complete => {
                writeln!(f, "Score: {score:.2}")?;
                writeln!(f, "Grade: {grade}")?;
            }
            (Some(score), Some(grade)) => {
                writeln!(f, "Score: {score:.2} (of the metrics measured)")?;
                writeln!(f, "Grade: {grade} (partial)")?;
            }
            _ => {
                writeln!(f, "Score: unavailable (no metric could be measured)")?;
                writeln!(f, "Grade: unavailable")?;
            }
        }

        if !self.breakdown.is_empty() {
            let name_width = self
                .breakdown
                .iter()
                .map(|metric| metric.name.len())
                .max()
                .unwrap_or(0);

            writeln!(f)?;
            writeln!(f, "Breakdown:")?;
            for metric in &self.breakdown {
                let cell = match metric.score {
                    Some(score) => format!("{score:.2}"),
                    None => "unmeasured".to_string(),
                };
                writeln!(
                    f,
                    "- {:<name_width$}  {:>QUALITY_SCORE_COLUMN$}  weight {:.2}",
                    metric.name, cell, metric.weight
                )?;
            }
        }

        if !self.recommendations.is_empty() {
            writeln!(f)?;
            writeln!(f, "Recommendations:")?;
            for recommendation in &self.recommendations {
                writeln!(f, "- {recommendation}")?;
            }
        }

        Ok(())
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Severity::Critical => "critical",
            Severity::High => "high",
            Severity::Moderate => "moderate",
            Severity::Low => "low",
        };
        write!(f, "{label}")
    }
}

impl fmt::Display for UpdateType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            UpdateType::Major => "major",
            UpdateType::Minor => "minor",
            UpdateType::Patch => "patch",
        };
        write!(f, "{label}")
    }
}

impl fmt::Display for DependencyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            DependencyType::Normal => "normal",
            DependencyType::Dev => "dev",
            DependencyType::Build => "build",
        };
        write!(f, "{label}")
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            SkipReason::NonRegistry => "non_registry",
            SkipReason::MissingResolve => "missing_resolve",
            SkipReason::AmbiguousResolve => "ambiguous_resolve",
            SkipReason::OptionalNotActivated => "optional_not_activated",
            SkipReason::TargetSpecific => "target_specific",
            SkipReason::RegistryMetadataMissing => "registry_metadata_missing",
            SkipReason::RegistryUnavailable => "registry_unavailable",
            SkipReason::UnsupportedRegistry => "unsupported_registry",
        };
        write!(f, "{label}")
    }
}

impl fmt::Display for Grade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Grade::A => "A",
            Grade::B => "B",
            Grade::C => "C",
            Grade::D => "D",
            Grade::F => "F",
        };
        write!(f, "{label}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `UnsafeSummary` is aliased: this module defines its own, and `use
    // super::*` above would otherwise make which one is meant ambiguous.
    use crate::core::doc_examples::{serialized_value, DocumentedExamples};
    use crate::core::scorers::quality::{
        score_quality, Availability, ClippySummary, DependencyFreshness, MsrvStatus, QualityInputs,
        SecuritySummary, UnsafeSummary as ScoredUnsafe, UnusedSummary,
    };
    use serde_json::Value;

    fn value_at<'a>(value: &'a Value, key: &str) -> &'a Value {
        value
            .get(key)
            .unwrap_or_else(|| panic!("missing key {key}"))
    }

    /// Every analyzer down, scored the way production scores it.
    fn all_unavailable() -> QualityInputs {
        QualityInputs {
            dependency_freshness: Availability::failed("crates.io unreachable"),
            security: Availability::failed("advisory database unreachable"),
            unused: Availability::failed("advisory database unreachable"),
            unsafe_code: Availability::failed("advisory database unreachable"),
            clippy: Availability::failed("advisory database unreachable"),
            msrv: Availability::failed("advisory database unreachable"),
        }
    }

    #[test]
    fn serialize_enums() {
        assert_eq!(
            serde_json::to_value(Severity::High).unwrap(),
            Value::String("high".into())
        );
        assert_eq!(
            serde_json::to_value(UpdateType::Minor).unwrap(),
            Value::String("minor".into())
        );
        assert_eq!(
            serde_json::to_value(DependencyType::Dev).unwrap(),
            Value::String("dev".into())
        );
        assert_eq!(
            serde_json::to_value(SkipReason::TargetSpecific).unwrap(),
            Value::String("target_specific".into())
        );
        assert_eq!(
            serde_json::to_value(SkipReason::UnsupportedRegistry).unwrap(),
            Value::String("unsupported_registry".into())
        );
        // Pinned because it is a user-visible addition to the `skipped_packages[].reason`
        // contract; the Display label above must not drift from it either.
        assert_eq!(
            serde_json::to_value(SkipReason::AmbiguousResolve).unwrap(),
            Value::String("ambiguous_resolve".into())
        );
        assert_eq!(
            serde_json::to_value(Grade::A).unwrap(),
            Value::String("A".into())
        );
        assert_eq!(
            serde_json::to_value(Confidence::Medium).unwrap(),
            Value::String("medium".into())
        );
    }

    #[test]
    fn documented_json_examples_match_output_contracts() {
        let detect = DetectOutput {
            edition: Some("2021".to_string()),
            msrv: Some("1.70".to_string()),
            workspace: true,
            members: vec!["core".to_string(), "upkeep".to_string()],
            package: Some("upkeep".to_string()),
            version: Some("0.1.0".to_string()),
            dependencies: 3,
            features: vec!["default".to_string()],
            targets: vec!["bin".to_string()],
            tooling: vec!["clippy".to_string()],
            ci: vec!["github-actions".to_string()],
        };

        let audit = AuditOutput {
            vulnerabilities: vec![Vulnerability {
                id: "RUSTSEC-0000-0000".to_string(),
                package: "serde".to_string(),
                package_version: "1.0.0".to_string(),
                severity: Severity::High,
                title: "Example".to_string(),
                path: vec!["root".to_string(), "serde".to_string()],
                fix_available: true,
            }],
            warnings: vec![AuditWarning {
                kind: AuditWarningKind::Unmaintained,
                package: "example-old".to_string(),
                package_version: "0.1.0".to_string(),
                advisory_id: Some("RUSTSEC-0000-0001".to_string()),
                title: Some("Example crate is unmaintained".to_string()),
                path: vec!["root".to_string(), "example-old".to_string()],
                fix_available: Some(false),
            }],
            summary: AuditSummary {
                critical: 0,
                high: 1,
                moderate: 0,
                low: 0,
                total: 1,
            },
        };

        let deps = DepsOutput {
            total: 2,
            checked: 2,
            outdated: 1,
            major: 0,
            minor: 0,
            patch: 1,
            packages: vec![OutdatedPackage {
                name: "serde".to_string(),
                alias: None,
                current: "1.0.0".to_string(),
                latest: "1.0.1".to_string(),
                required: "^1.0".to_string(),
                update_type: UpdateType::Patch,
                dependency_type: DependencyType::Normal,
                members: vec!["core".to_string()],
            }],
            skipped: 1,
            skipped_packages: vec![SkippedDependency {
                name: "serde".to_string(),
                alias: None,
                required: "^1.0".to_string(),
                reason: SkipReason::TargetSpecific,
                dependency_type: DependencyType::Normal,
                source: None,
                target: Some("x86_64-unknown-linux-gnu".to_string()),
            }],
            warnings: vec![
                "security scan uses Cargo.lock and reports direct workspace dependencies only"
                    .to_string(),
            ],
            security: Some(DepsSecurityOutput {
                summary: AuditSummary {
                    critical: 0,
                    high: 1,
                    moderate: 0,
                    low: 0,
                    total: 1,
                },
                packages: vec![DepsSecurityPackage {
                    name: "serde".to_string(),
                    alias: None,
                    current: "1.0.0".to_string(),
                    dependency_type: DependencyType::Normal,
                    members: vec!["core".to_string()],
                    vulnerabilities: vec![DepsSecurityVulnerability {
                        advisory_id: "RUSTSEC-0000-0000".to_string(),
                        severity: Severity::High,
                        title: "Example".to_string(),
                        fix_available: true,
                    }],
                }],
            }),
            workspace: true,
            members: vec!["core".to_string()],
            skipped_members: vec!["legacy".to_string()],
        };

        let unused = UnusedOutput {
            unused: vec![UnusedDep {
                name: "tokio".to_string(),
                dependency_type: DependencyType::Dev,
                confidence: Confidence::High,
            }],
            possibly_unused: vec!["serde".to_string()],
        };

        let unsafe_output = UnsafeOutput {
            summary: UnsafeSummary {
                packages: 1,
                unsafe_functions: 2,
                unsafe_impls: 1,
                unsafe_traits: 0,
                unsafe_blocks: 3,
                unsafe_expressions: 1,
                total_unsafe: 7,
            },
            packages: vec![UnsafePackage {
                name: "ffi".to_string(),
                version: "0.1.0".to_string(),
                package_id: Some("ffi 0.1.0 (path+file://...)".to_string()),
                unsafe_functions: 2,
                unsafe_impls: 1,
                unsafe_traits: 0,
                unsafe_blocks: 3,
                unsafe_expressions: 1,
                total_unsafe: 7,
            }],
        };

        let tree = TreeOutput {
            root: TreeNode {
                name: "root".to_string(),
                version: "0.1.0".to_string(),
                package_id: "root 0.1.0".to_string(),
                features: vec!["default".to_string()],
                dependencies: vec![TreeNode {
                    name: "dep".to_string(),
                    version: "1.2.3".to_string(),
                    package_id: "dep 1.2.3".to_string(),
                    features: Vec::new(),
                    dependencies: Vec::new(),
                    is_dev: false,
                    is_build: false,
                    duplicate: false,
                }],
                is_dev: false,
                is_build: false,
                duplicate: false,
            },
            stats: TreeStats {
                total_crates: 2,
                direct_deps: 1,
                transitive_deps: 0,
                duplicate_crates: 0,
            },
        };

        let quality = score_quality(QualityInputs {
            dependency_freshness: Availability::Measured(DependencyFreshness {
                total: 10,
                outdated: 1,
            }),
            security: Availability::Measured(SecuritySummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
            }),
            unused: Availability::Measured(UnusedSummary { unused_count: 0 }),
            unsafe_code: Availability::not_installed(
                "cargo-geiger is not installed; install with `cargo install cargo-geiger`",
            ),
            clippy: Availability::Measured(ClippySummary {
                warnings: 0,
                errors: 0,
            }),
            msrv: Availability::Measured(MsrvStatus::Valid),
        });
        assert_eq!(quality.breakdown.len(), 6);
        assert_eq!(quality.unavailable.len(), 1);
        assert!(quality.recommendations.is_empty());

        let detect_value = serialized_value(&detect);
        assert_eq!(value_at(&detect_value, "workspace"), &Value::Bool(true));
        assert_eq!(
            value_at(&detect_value, "members")[0],
            Value::String("core".into())
        );

        let audit_value = serialized_value(&audit);
        assert_eq!(
            value_at(&audit_value, "summary")["high"],
            Value::Number(1.into())
        );
        assert_eq!(
            value_at(&audit_value, "vulnerabilities")[0]["severity"],
            Value::String("high".into())
        );
        assert_eq!(
            value_at(&audit_value, "vulnerabilities")[0]["id"],
            Value::String("RUSTSEC-0000-0000".into())
        );
        assert_eq!(
            value_at(&audit_value, "warnings")[0]["kind"],
            Value::String("unmaintained".into())
        );
        assert_eq!(
            value_at(&audit_value, "warnings")[0]["fix_available"],
            Value::Bool(false)
        );

        let deps_value = serialized_value(&deps);
        assert_eq!(value_at(&deps_value, "outdated"), &Value::Number(1.into()));
        assert_eq!(
            value_at(&deps_value, "security")["packages"][0]["vulnerabilities"][0]["advisory_id"],
            Value::String("RUSTSEC-0000-0000".into())
        );

        let unused_value = serialized_value(&unused);
        assert_eq!(
            value_at(&unused_value, "unused")[0]["confidence"],
            Value::String("high".into())
        );

        let unsafe_value = serialized_value(&unsafe_output);
        assert_eq!(
            value_at(&unsafe_value, "summary")["total_unsafe"],
            Value::Number(7.into())
        );
        assert_eq!(
            value_at(&unsafe_value, "packages")[0]["name"],
            Value::String("ffi".into())
        );

        let tree_value = serialized_value(&tree);
        assert_eq!(
            value_at(&tree_value, "stats")["direct_deps"],
            Value::Number(1.into())
        );
        assert_eq!(
            value_at(&tree_value, "root")["dependencies"][0]["name"],
            Value::String("dep".into())
        );

        let quality_value = serialized_value(&quality);
        assert_eq!(
            value_at(&quality_value, "grade"),
            &Value::String("A".into())
        );
        assert_eq!(
            value_at(&quality_value, "breakdown")[0]["name"],
            Value::String("Dependency freshness".into())
        );

        let documentation = DocumentedExamples::load("docs/commands.md");

        for (command, expected) in [
            ("detect", detect_value),
            ("deps", deps_value),
            ("audit", audit_value),
            ("quality", quality_value),
            ("tree", tree_value),
            ("unused", unused_value),
            ("unsafe-code", unsafe_value),
        ] {
            let documented = documentation.example(command);
            assert_eq!(
                documented, expected,
                "{command}: docs/commands.md JSON example drifted from its serialized output contract"
            );
        }
    }

    #[test]
    fn display_detect_output_with_defaults() {
        let output = DetectOutput {
            edition: None,
            msrv: None,
            workspace: false,
            members: Vec::new(),
            package: None,
            version: None,
            dependencies: 0,
            features: Vec::new(),
            targets: Vec::new(),
            tooling: Vec::new(),
            ci: Vec::new(),
        };

        let text = format!("{output}");
        assert!(text.contains("Package: unknown"));
        assert!(text.contains("Version: unknown"));
        assert!(text.contains("Edition: unknown"));
        assert!(text.contains("MSRV: unknown"));
        assert!(text.contains("Workspace: false"));
        assert!(text.contains("Members: none"));
        assert!(text.contains("Features: none"));
        assert!(text.contains("Targets: none"));
        assert!(text.contains("Tooling: none"));
        assert!(text.contains("CI: none"));
    }

    #[test]
    fn display_detect_output_with_values() {
        let output = DetectOutput {
            edition: Some("2021".to_string()),
            msrv: Some("1.70".to_string()),
            workspace: true,
            members: vec!["core".to_string()],
            package: Some("upkeep".to_string()),
            version: Some("0.1.0".to_string()),
            dependencies: 3,
            features: vec!["default".to_string()],
            targets: vec!["x86_64-apple-darwin".to_string()],
            tooling: vec!["clippy".to_string()],
            ci: vec!["github".to_string()],
        };

        let text = format!("{output}");
        assert!(text.contains("Package: upkeep"));
        assert!(text.contains("Version: 0.1.0"));
        assert!(text.contains("Edition: 2021"));
        assert!(text.contains("MSRV: 1.70"));
        assert!(text.contains("Workspace: true"));
        assert!(text.contains("Members: core"));
        assert!(text.contains("Dependencies: 3"));
        assert!(text.contains("Features: default"));
        assert!(text.contains("Targets: x86_64-apple-darwin"));
        assert!(text.contains("Tooling: clippy"));
        assert!(text.contains("CI: github"));
    }

    #[test]
    fn display_deps_output_full_sections() {
        let output = DepsOutput {
            total: 2,
            checked: 2,
            outdated: 1,
            major: 1,
            minor: 0,
            patch: 0,
            packages: vec![OutdatedPackage {
                name: "serde".to_string(),
                alias: Some("serde_renamed".to_string()),
                current: "1.0.0".to_string(),
                latest: "1.0.1".to_string(),
                required: "^1.0".to_string(),
                update_type: UpdateType::Patch,
                dependency_type: DependencyType::Normal,
                members: vec!["demo".to_string()],
            }],
            skipped: 1,
            skipped_packages: vec![SkippedDependency {
                name: "tokio".to_string(),
                alias: None,
                required: "^1".to_string(),
                reason: SkipReason::NonRegistry,
                dependency_type: DependencyType::Dev,
                source: None,
                target: None,
            }],
            warnings: vec!["registry unavailable".to_string()],
            security: Some(DepsSecurityOutput {
                summary: AuditSummary {
                    critical: 0,
                    high: 1,
                    moderate: 0,
                    low: 0,
                    total: 1,
                },
                packages: vec![DepsSecurityPackage {
                    name: "serde".to_string(),
                    alias: None,
                    current: "1.0.0".to_string(),
                    dependency_type: DependencyType::Normal,
                    members: vec!["core".to_string()],
                    vulnerabilities: vec![DepsSecurityVulnerability {
                        advisory_id: "RUSTSEC-0000-0000".to_string(),
                        severity: Severity::High,
                        title: "Example".to_string(),
                        fix_available: true,
                    }],
                }],
            }),
            workspace: true,
            members: vec!["core".to_string()],
            skipped_members: vec!["legacy".to_string()],
        };

        let text = format!("{output}");
        assert!(text.contains("Warnings: registry unavailable"));
        assert!(text.contains("Security: 1 vulnerabilities"));
        assert!(text.contains("Vulnerable dependencies:"));
        assert!(text.contains("serde (normal) current 1.0.0"));
        assert!(text.contains("RUSTSEC-0000-0000"));
        assert!(text.contains("Workspace: true"));
        assert!(text.contains("Members: core"));
        assert!(text.contains("Skipped members: legacy"));
        assert!(text.contains("Outdated packages:"));
        assert!(text.contains("serde_renamed (serde)"));
        // The declaring member is what a reader acts on, and it disambiguates rows
        // for one crate resolved to several versions.
        assert!(text.contains("required ^1.0 members demo"));
        assert!(text.contains("Skipped dependencies:"));
        assert!(text.contains("tokio"));
    }

    #[test]
    fn display_deps_output_empty_sections() {
        let output = DepsOutput {
            total: 0,
            checked: 0,
            outdated: 0,
            major: 0,
            minor: 0,
            patch: 0,
            packages: Vec::new(),
            skipped: 0,
            skipped_packages: Vec::new(),
            warnings: Vec::new(),
            security: None,
            workspace: false,
            members: Vec::new(),
            skipped_members: Vec::new(),
        };

        let text = format!("{output}");
        assert!(text.contains("Outdated packages: none"));
        assert!(!text.contains("Warnings:"));
        assert!(!text.contains("Security:"));
        assert!(!text.contains("Skipped dependencies:"));
    }

    #[test]
    fn display_unused_output_variants() {
        let empty = UnusedOutput {
            unused: Vec::new(),
            possibly_unused: Vec::new(),
        };
        let text = format!("{empty}");
        assert!(text.contains("Unused details: none"));
        assert!(text.contains("Possibly unused details: none"));

        let populated = UnusedOutput {
            unused: vec![UnusedDep {
                name: "tokio".to_string(),
                dependency_type: DependencyType::Dev,
                confidence: Confidence::High,
            }],
            possibly_unused: vec!["serde".to_string()],
        };
        let text = format!("{populated}");
        assert!(text.contains("Unused details:"));
        assert!(text.contains("tokio (dev, confidence high)"));
        assert!(text.contains("Possibly unused details:"));
        assert!(text.contains("- serde"));
    }

    #[test]
    fn display_unsafe_output_variants() {
        let empty = UnsafeOutput {
            summary: UnsafeSummary {
                packages: 0,
                unsafe_functions: 0,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 0,
                unsafe_expressions: 0,
                total_unsafe: 0,
            },
            packages: Vec::new(),
        };
        let text = format!("{empty}");
        assert!(text.contains("Details: none"));

        let populated = UnsafeOutput {
            summary: UnsafeSummary {
                packages: 1,
                unsafe_functions: 1,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 1,
                unsafe_expressions: 0,
                total_unsafe: 2,
            },
            packages: vec![UnsafePackage {
                name: "ffi".to_string(),
                version: "0.1.0".to_string(),
                package_id: None,
                unsafe_functions: 1,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 1,
                unsafe_expressions: 0,
                total_unsafe: 2,
            }],
        };
        let text = format!("{populated}");
        assert!(text.contains("Details:"));
        assert!(text.contains("ffi 0.1.0: unsafe 2"));
    }

    #[test]
    fn display_audit_output_variants() {
        let empty = AuditOutput {
            vulnerabilities: Vec::new(),
            warnings: Vec::new(),
            summary: AuditSummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
                total: 0,
            },
        };
        let text = format!("{empty}");
        assert!(text.contains("Details: none"));

        let populated = AuditOutput {
            vulnerabilities: vec![Vulnerability {
                id: "RUSTSEC-0000-0000".to_string(),
                package: "serde".to_string(),
                package_version: "1.0.0".to_string(),
                severity: Severity::High,
                title: "Example".to_string(),
                path: vec!["root".to_string()],
                fix_available: false,
            }],
            warnings: vec![AuditWarning {
                kind: AuditWarningKind::Yanked,
                package: "old-dep".to_string(),
                package_version: "0.1.0".to_string(),
                advisory_id: None,
                title: None,
                path: vec!["root".to_string(), "old-dep".to_string()],
                fix_available: None,
            }],
            summary: AuditSummary {
                critical: 0,
                high: 1,
                moderate: 0,
                low: 0,
                total: 1,
            },
        };
        let text = format!("{populated}");
        assert!(text.contains("Details:"));
        assert!(text.contains("id"));
        assert!(text.contains("RUSTSEC-0000-0000"));
        assert!(text.contains("Warnings: 1"));
        assert!(text.contains("yanked old-dep 0.1.0"));
        assert!(text.contains("advisory -"));
        assert!(text.contains("fix available unknown"));
        assert!(text.contains("root -> old-dep"));
    }

    #[test]
    fn display_quality_output_formats_score() {
        let output = QualityOutput {
            score: Some(92.54),
            grade: Some(Grade::A),
            complete: true,
            measured_weight: 1.0,
            breakdown: Vec::new(),
            unavailable: Vec::new(),
            recommendations: Vec::new(),
        };
        let text = format!("{output}");
        // Two decimals, the same precision the scorer rounds to and the JSON
        // reports. `{:.1}` here would print `92.5` for a stored `92.54`.
        assert!(text.contains("Score: 92.54"));
        assert!(text.contains("Grade: A"));
        // A complete run says nothing about incompleteness.
        assert!(!text.contains("Analysis incomplete"));
        assert!(!text.contains("partial"));
    }

    /// Scored through `score_quality` rather than hand-built.
    ///
    /// A literal `QualityOutput` can express states production never produces —
    /// a breakdown that is not all six metrics, or a `complete` flag that
    /// disagrees with `unavailable`, which is *derived* from it. Deriving the
    /// fixture means the banner counts ("1 of 6") and the renormalized score
    /// are the real ones, so this test cannot drift from what users see.
    #[test]
    fn display_quality_output_reports_incompleteness_before_the_score() {
        let output = score_quality(QualityInputs {
            dependency_freshness: Availability::Measured(DependencyFreshness {
                total: 10,
                outdated: 2,
            }),
            security: Availability::Measured(SecuritySummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
            }),
            unused: Availability::not_installed(
                "cargo-machete is not installed; install with `cargo install cargo-machete`",
            ),
            unsafe_code: Availability::Measured(ScoredUnsafe { total_unsafe: 2 }),
            clippy: Availability::Measured(ClippySummary {
                warnings: 12,
                errors: 0,
            }),
            msrv: Availability::Measured(MsrvStatus::Valid),
        });
        let text = format!("{output}");

        let warning = text
            .find("Analysis incomplete")
            .expect("incompleteness warning");
        let score = text.find("Score:").expect("score line");
        assert!(
            warning < score,
            "incompleteness must be stated before the score:\n{text}"
        );

        assert!(text.contains("1 of 6 metrics could not be measured (85% of weight measured)"));
        assert!(text.contains("not installed"));
        assert!(text.contains("cargo install cargo-machete"));

        // 0.20*80 + 0.25*100 + 0.15*90 + 0.15*76 + 0.10*100 = 75.9, over the
        // 0.85 of weight that ran: 75.9 / 0.85 = 89.29.
        assert!(
            text.contains("Score: 89.29 (of the metrics measured)"),
            "{text}"
        );
        assert!(text.contains("Grade: B (partial)"));

        // The breakdown shows the unmeasured metric as unmeasured, not as a number.
        assert!(text.contains("unmeasured"));

        // Recommendations are printed at all, which they previously were not.
        assert!(text.contains("Recommendations:"));
        assert!(text.contains("- Update outdated dependencies."));
    }

    #[test]
    fn display_quality_output_handles_nothing_measured() {
        let output = score_quality(all_unavailable());
        let text = format!("{output}");

        assert!(text.contains("6 of 6 metrics could not be measured"));
        assert!(text.contains("(0% of weight measured)"));
        assert!(text.contains("Score: unavailable (no metric could be measured)"));
        assert!(text.contains("Grade: unavailable"));
        assert!(text.contains("failed"));
    }

    #[test]
    fn display_enum_labels() {
        assert_eq!(format!("{}", Confidence::High), "high");
        assert_eq!(format!("{}", Confidence::Medium), "medium");
        assert_eq!(format!("{}", Confidence::Low), "low");
        assert_eq!(format!("{}", Severity::Critical), "critical");
        assert_eq!(format!("{}", UpdateType::Major), "major");
        assert_eq!(format!("{}", DependencyType::Build), "build");
        assert_eq!(
            format!("{}", SkipReason::RegistryUnavailable),
            "registry_unavailable"
        );
        assert_eq!(
            format!("{}", SkipReason::UnsupportedRegistry),
            "unsupported_registry"
        );
        assert_eq!(
            format!("{}", SkipReason::AmbiguousResolve),
            "ambiguous_resolve"
        );
        assert_eq!(format!("{}", Grade::F), "F");
    }
}
