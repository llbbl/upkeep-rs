//! Output types and serialization helpers.

#![allow(dead_code)]

use serde::Serialize;
use std::fmt;

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
    pub workspace: bool,
    pub members: Vec<String>,
    pub skipped_members: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditOutput {
    pub vulnerabilities: Vec<Vulnerability>,
    pub summary: AuditSummary,
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
pub struct UpkeepError {
    pub code: String,
    pub message: String,
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

#[derive(Debug, Serialize)]
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

pub fn print_json<T: Serialize>(output: &T) -> anyhow::Result<()> {
    let payload = serde_json::to_string_pretty(output)?;
    println!("{payload}");
    Ok(())
}

pub fn print_error(error: &UpkeepError) -> anyhow::Result<()> {
    let payload = serde_json::to_string_pretty(error)?;
    eprintln!("{payload}");
    Ok(())
}

impl fmt::Display for UpkeepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for UpkeepError {}

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
    use super::{print_json, Grade, QualityOutput};

    #[test]
    fn output_module_smoke() {
        let output = QualityOutput {
            score: 92.5,
            grade: Grade::A,
            breakdown: Vec::new(),
            recommendations: Vec::new(),
        };

        print_json(&output).unwrap();
    }
}
