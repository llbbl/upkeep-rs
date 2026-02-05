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
    pub total: usize,
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
    pub summary: AuditSummary,
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

#[derive(Debug, Serialize)]
pub struct QualityOutput {
    pub score: f32,
    pub grade: Grade,
    pub breakdown: Vec<MetricScore>,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct MetricScore {
    pub name: String,
    pub score: f32,
    pub weight: f32,
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
    pub package: String,
    pub package_version: String,
    pub advisory_id: String,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateType {
    Major,
    Minor,
    Patch,
}

#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum DependencyType {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    NonRegistry,
    MissingResolve,
    OptionalNotActivated,
    TargetSpecific,
    RegistryMetadataMissing,
    RegistryUnavailable,
}

#[derive(Debug, Serialize)]
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
                writeln!(
                    f,
                    "- {} ({}) current {} latest {} required {}",
                    display_name,
                    package.dependency_type,
                    package.current,
                    package.latest,
                    package.required
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
            return Ok(());
        }

        let mut advisory_width = "advisory_id".len();
        let mut package_width = "package".len();
        let mut version_width = "version".len();
        let mut severity_width = "severity".len();

        for vulnerability in &self.vulnerabilities {
            advisory_width = advisory_width.max(vulnerability.advisory_id.len());
            package_width = package_width.max(vulnerability.package.len());
            version_width = version_width.max(vulnerability.package_version.len());
            severity_width = severity_width.max(vulnerability.severity.to_string().len());
        }

        writeln!(f, "Details:")?;
        writeln!(
            f,
            "{:<advisory_width$}  {:<package_width$}  {:<version_width$}  {:<severity_width$}",
            "advisory_id",
            "package",
            "version",
            "severity",
            advisory_width = advisory_width,
            package_width = package_width,
            version_width = version_width,
            severity_width = severity_width
        )?;
        for vulnerability in &self.vulnerabilities {
            writeln!(
                f,
                "{:<advisory_width$}  {:<package_width$}  {:<version_width$}  {:<severity_width$}",
                vulnerability.advisory_id,
                vulnerability.package,
                vulnerability.package_version,
                vulnerability.severity,
                advisory_width = advisory_width,
                package_width = package_width,
                version_width = version_width,
                severity_width = severity_width
            )?;
        }
        Ok(())
    }
}

impl fmt::Display for QualityOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Score: {:.1}", self.score)?;
        writeln!(f, "Grade: {}", self.grade)?;
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
            SkipReason::OptionalNotActivated => "optional_not_activated",
            SkipReason::TargetSpecific => "target_specific",
            SkipReason::RegistryMetadataMissing => "registry_metadata_missing",
            SkipReason::RegistryUnavailable => "registry_unavailable",
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
    use super::{Grade, QualityOutput};

    #[test]
    fn output_module_smoke() {
        let output = QualityOutput {
            score: 92.5,
            grade: Grade::A,
            breakdown: Vec::new(),
            recommendations: Vec::new(),
        };

        // Test that serialization succeeds without printing to stdout
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"score\": 92.5"));
        assert!(json.contains("\"grade\": \"A\""));
    }
}
