use cargo_metadata::{Metadata, MetadataCommand, PackageId};
use rustsec::advisory::Severity as RustsecSeverity;
use rustsec::database::Database;
use rustsec::report::{Report, Settings};
use rustsec::Lockfile;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;

use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{AuditOutput, AuditSummary, Severity, Vulnerability};

/// Names a local RustSec advisory-database checkout to read instead of fetching.
///
/// Unset — the default — clones or fetches into the shared `~/.cargo/advisory-db`.
/// That directory is process-global mutable state, and it fails in two ways.
///
/// Two rustsec-based runs do not corrupt each other: `Database::fetch` takes an
/// outer flock on `~/.cargo/advisory-db..lock` and *waits* on it for up to five
/// minutes. The cost there is a stall, not an error — long enough to look like a
/// hung CI job.
///
/// The hard failure comes from the git lock underneath, taken only once the outer
/// flock is already held: a stale `.git/index.lock` left by a killed process, or a
/// non-rustsec git client touching the repo, fails the audit immediately, because
/// `gix` makes a single attempt with no retry or backoff.
///
/// Set, the database is read from that path and nothing is fetched. The
/// integration tests use it to stay off the shared cache entirely; an offline or
/// air-gapped run can point it at a checkout it manages itself.
pub const ADVISORY_DB_ENV: &str = "UPKEEP_ADVISORY_DB";

pub fn run_audit() -> Result<AuditOutput> {
    let metadata = MetadataCommand::new().exec().map_err(|err| {
        UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
    })?;

    let workspace_root = PathBuf::from(&metadata.workspace_root);
    let lockfile_path = workspace_root.join("Cargo.lock");
    let lockfile = Lockfile::load(&lockfile_path).map_err(|err| {
        UpkeepError::context(
            ErrorCode::Rustsec,
            format!("failed to load {}", lockfile_path.display()),
            err,
        )
    })?;

    let db = load_database()?;
    let settings = Settings::default();
    let report = Report::generate(&db, &lockfile, &settings);

    let graph = DependencyGraph::build(&metadata)?;

    let mut vulnerabilities = Vec::new();
    for entry in &report.vulnerabilities.list {
        let advisory = &entry.advisory;
        let package_name = entry.package.name.to_string();
        let package_version = entry.package.version.to_string();

        let path = graph
            .path_to(
                &package_name,
                &package_version,
                entry
                    .package
                    .source
                    .as_ref()
                    .map(|s| s.to_string())
                    .as_deref(),
            )
            .unwrap_or_else(|| vec![package_name.clone()]);

        vulnerabilities.push(Vulnerability {
            id: advisory.id.to_string(),
            package: package_name,
            package_version,
            severity: map_severity(advisory.cvss.as_ref().map(|c| c.severity())),
            title: advisory.title.to_string(),
            path,
            fix_available: !entry.versions.patched().is_empty(),
        });
    }

    let summary = summarize(&vulnerabilities);
    Ok(AuditOutput {
        vulnerabilities,
        summary,
    })
}

/// Load the advisory database, honouring [`ADVISORY_DB_ENV`].
fn load_database() -> Result<Database> {
    match local_advisory_db(std::env::var_os(ADVISORY_DB_ENV))? {
        Some(path) => Database::open(&path).map_err(|err| {
            UpkeepError::context(
                ErrorCode::Rustsec,
                format!(
                    "failed to open RustSec advisory database at {} (from {ADVISORY_DB_ENV})",
                    path.display()
                ),
                err,
            )
        }),
        None => Database::fetch().map_err(|err| {
            UpkeepError::context(
                ErrorCode::Rustsec,
                "failed to fetch RustSec advisory database",
                err,
            )
        }),
    }
}

/// Interpret the raw value of [`ADVISORY_DB_ENV`]: `None` means fetch.
///
/// A set-but-empty value is an error rather than a fetch. Callers set this
/// variable specifically to avoid the shared cache, so falling back to fetching
/// would produce the one outcome they cannot detect — a run that silently used
/// `~/.cargo/advisory-db` after being told not to.
fn local_advisory_db(value: Option<OsString>) -> Result<Option<PathBuf>> {
    match value {
        None => Ok(None),
        Some(path) if path.is_empty() => Err(UpkeepError::message(
            ErrorCode::Config,
            format!("{ADVISORY_DB_ENV} is set but empty; unset it to fetch the shared database"),
        )),
        Some(path) => Ok(Some(PathBuf::from(path))),
    }
}

fn map_severity(severity: Option<RustsecSeverity>) -> Severity {
    match severity {
        Some(RustsecSeverity::Critical) => Severity::Critical,
        Some(RustsecSeverity::High) => Severity::High,
        Some(RustsecSeverity::Medium) => Severity::Moderate,
        Some(RustsecSeverity::Low) => Severity::Low,
        Some(RustsecSeverity::None) => Severity::Low, // Treat "none" as low
        None => Severity::High, // Unknown severity defaults to high for safety
    }
}

fn summarize(vulnerabilities: &[Vulnerability]) -> AuditSummary {
    let mut summary = AuditSummary {
        critical: 0,
        high: 0,
        moderate: 0,
        low: 0,
        total: vulnerabilities.len(),
    };

    for vuln in vulnerabilities {
        match vuln.severity {
            Severity::Critical => summary.critical += 1,
            Severity::High => summary.high += 1,
            Severity::Moderate => summary.moderate += 1,
            Severity::Low => summary.low += 1,
        }
    }

    summary
}

struct DependencyGraph {
    adjacency: HashMap<PackageId, Vec<PackageId>>,
    packages_by_id: HashMap<PackageId, cargo_metadata::Package>,
    by_name_version: HashMap<(String, String, Option<String>), PackageId>,
    roots: Vec<PackageId>,
}

impl DependencyGraph {
    fn build(metadata: &Metadata) -> Result<Self> {
        let resolve = metadata.resolve.as_ref().ok_or_else(|| {
            UpkeepError::message(ErrorCode::InvalidData, "metadata missing resolve data")
        })?;

        let mut packages_by_id = HashMap::new();
        let mut by_name_version = HashMap::new();
        for package in &metadata.packages {
            let source = package.source.as_ref().map(|src| src.to_string());
            let key = (
                package.name.to_string(),
                package.version.to_string(),
                source,
            );
            by_name_version.insert(key, package.id.clone());
            packages_by_id.insert(package.id.clone(), package.clone());
        }

        let mut adjacency: HashMap<PackageId, Vec<PackageId>> = HashMap::new();
        for node in &resolve.nodes {
            let deps = node.deps.iter().map(|dep| dep.pkg.clone()).collect();
            adjacency.insert(node.id.clone(), deps);
        }

        let roots = if let Some(root_package) = metadata.root_package() {
            vec![root_package.id.clone()]
        } else {
            metadata.workspace_members.clone()
        };

        Ok(Self {
            adjacency,
            packages_by_id,
            by_name_version,
            roots,
        })
    }

    fn path_to(&self, name: &str, version: &str, source: Option<&str>) -> Option<Vec<String>> {
        // Build lookup keys once, avoiding repeated allocations
        let name_str = name.to_string();
        let version_str = version.to_string();
        let source_str = source.map(str::to_string);

        // Try exact match first (with source)
        let target_id = self
            .by_name_version
            .get(&(name_str.clone(), version_str.clone(), source_str))
            // If no exact match, try without source (crates.io packages may have None source
            // in cargo_metadata but a source string from rustsec lockfile)
            .or_else(|| {
                self.by_name_version
                    .get(&(name_str.clone(), version_str.clone(), None))
            })
            // Also try matching any source with same name/version as fallback
            .or_else(|| {
                self.by_name_version
                    .iter()
                    .find(|((n, v, _), _)| n == name && v == version)
                    .map(|(_, id)| id)
            })?;

        // BFS to find path from any root to target
        // Use references where possible to reduce cloning
        let mut queue = VecDeque::new();
        let mut visited: HashSet<&PackageId> = HashSet::new();
        let mut parents: HashMap<&PackageId, &PackageId> = HashMap::new();

        for root in &self.roots {
            queue.push_back(root);
            visited.insert(root);
        }

        let mut found = false;
        while let Some(node) = queue.pop_front() {
            if node == target_id {
                found = true;
                break;
            }
            if let Some(deps) = self.adjacency.get(node) {
                for dep in deps {
                    if visited.insert(dep) {
                        parents.insert(dep, node);
                        queue.push_back(dep);
                    }
                }
            }
        }

        if !found {
            return None;
        }

        // Reconstruct path from target back to root
        let mut path_ids = Vec::new();
        let mut current = target_id;
        path_ids.push(current);
        while let Some(&parent) = parents.get(current) {
            path_ids.push(parent);
            current = parent;
        }
        path_ids.reverse();

        // Convert PackageIds to package names
        let path = path_ids
            .into_iter()
            .map(|id| {
                self.packages_by_id
                    .get(id)
                    .map(|pkg| pkg.name.to_string())
                    .unwrap_or_else(|| id.repr.clone())
            })
            .collect();

        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_metadata::MetadataCommand;

    fn vuln_with(severity: Severity) -> Vulnerability {
        Vulnerability {
            id: "RUSTSEC-0000-0000".to_string(),
            package: "pkg".to_string(),
            package_version: "1.0.0".to_string(),
            severity,
            title: "Example".to_string(),
            path: vec!["pkg".to_string()],
            fix_available: false,
        }
    }

    #[test]
    fn local_advisory_db_unset_means_fetch() {
        assert_eq!(local_advisory_db(None).expect("unset is valid"), None);
    }

    #[test]
    fn local_advisory_db_uses_the_given_path() {
        let path = local_advisory_db(Some(OsString::from("/tmp/advisory-db")))
            .expect("a path is valid")
            .expect("a path means local");
        assert_eq!(path, PathBuf::from("/tmp/advisory-db"));
    }

    /// An empty value must not degrade into a fetch — see [`local_advisory_db`].
    #[test]
    fn local_advisory_db_rejects_an_empty_value() {
        let err = local_advisory_db(Some(OsString::new())).expect_err("empty is an error");
        assert!(
            err.to_string().contains(ADVISORY_DB_ENV),
            "error should name the variable; got: {err}"
        );
    }

    #[test]
    fn map_severity_handles_all_levels() {
        assert!(matches!(
            map_severity(Some(RustsecSeverity::Critical)),
            Severity::Critical
        ));
        assert!(matches!(
            map_severity(Some(RustsecSeverity::High)),
            Severity::High
        ));
        assert!(matches!(
            map_severity(Some(RustsecSeverity::Medium)),
            Severity::Moderate
        ));
        assert!(matches!(
            map_severity(Some(RustsecSeverity::Low)),
            Severity::Low
        ));
        assert!(matches!(
            map_severity(Some(RustsecSeverity::None)),
            Severity::Low
        ));
        assert!(matches!(map_severity(None), Severity::High));
    }

    #[test]
    fn summarize_counts_and_totals() {
        let vulnerabilities = vec![
            vuln_with(Severity::Critical),
            vuln_with(Severity::High),
            vuln_with(Severity::Moderate),
            vuln_with(Severity::Low),
            vuln_with(Severity::Low),
        ];

        let summary = summarize(&vulnerabilities);
        assert_eq!(summary.critical, 1);
        assert_eq!(summary.high, 1);
        assert_eq!(summary.moderate, 1);
        assert_eq!(summary.low, 2);
        assert_eq!(summary.total, 5);
    }

    #[test]
    fn dependency_graph_path_to_returns_none_for_missing_package() {
        let metadata = MetadataCommand::new().exec().expect("metadata");
        let graph = DependencyGraph::build(&metadata).expect("graph");

        let missing = graph.path_to("missing-pkg-that-does-not-exist", "0.0.0", None);
        assert!(
            missing.is_none(),
            "path_to should return None for non-existent packages"
        );
    }

    /// Tests fallback path lookup using a fake source to trigger fallback logic.
    /// Requires the project to have a root package (not a workspace without root).
    #[test]
    fn dependency_graph_path_to_fallback_finds_root_package() {
        let metadata = MetadataCommand::new().exec().expect("metadata");
        let graph = DependencyGraph::build(&metadata).expect("graph");

        let root = metadata
            .root_package()
            .expect("test requires a root package (not a workspace without root)");

        let fallback_path = graph
            .path_to(&root.name, &root.version.to_string(), Some("registry+fake"))
            .expect("fallback path should find root package");
        assert_eq!(
            fallback_path.last().map(String::as_str),
            Some(root.name.as_str()),
            "fallback path should end with the root package name"
        );
    }

    /// Tests exact source match path lookup.
    /// Requires the project to have at least one registry dependency.
    #[test]
    fn dependency_graph_path_to_exact_match_finds_registry_package() {
        let metadata = MetadataCommand::new().exec().expect("metadata");
        let graph = DependencyGraph::build(&metadata).expect("graph");

        let registry_pkg = metadata
            .packages
            .iter()
            .find(|pkg| pkg.source.is_some())
            .expect("test requires at least one registry dependency");

        let exact_path = graph
            .path_to(
                &registry_pkg.name,
                &registry_pkg.version.to_string(),
                registry_pkg.source.as_ref().map(|src| src.repr.as_str()),
            )
            .expect("exact path should find registry package");
        assert_eq!(
            exact_path.last().map(String::as_str),
            Some(registry_pkg.name.as_str()),
            "exact path should end with the registry package name"
        );
    }
}
