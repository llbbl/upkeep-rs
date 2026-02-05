use cargo_metadata::{
    Dependency, DependencyKind, Metadata, MetadataCommand, Node, Package, PackageId,
};
use semver::Version;
use std::collections::{HashMap, HashSet};

use super::run_with::run_with_output;
use crate::core::analyzers::audit::run_audit;
use crate::core::analyzers::crates_io::{CratesIoClient, VersionInfo};
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{
    print_json, AuditSummary, DependencyType, DepsOutput, DepsSecurityOutput, DepsSecurityPackage,
    DepsSecurityVulnerability, OutdatedPackage, Severity, SkipReason, SkippedDependency,
    UpdateType,
};

pub async fn run(json: bool, include_security: bool) -> Result<()> {
    run_with_output(json, analyze(include_security), print_json, |output| {
        println!("{output}");
        Ok(())
    })
    .await
}

pub async fn analyze(include_security: bool) -> Result<DepsOutput> {
    let metadata = load_metadata()?;
    // Compute workspace flag before checking for root package to match detect.rs logic:
    // A workspace exists if there's no root package (virtual workspace) OR multiple members
    let is_workspace = metadata.root_package().is_none() || metadata.workspace_members.len() > 1;
    let resolve = get_resolve(&metadata)?;
    let packages_by_id = build_packages_map(&metadata);
    let member_packages = workspace_packages(&metadata, &packages_by_id)?;
    let member_nodes = find_member_nodes(resolve, &member_packages)?;
    let resolved_versions = build_resolved_versions(&member_nodes, &packages_by_id);

    let (member_names, skipped_members) = compute_workspace_info(&metadata, &packages_by_id);

    let (dependency_names, mut skipped_packages, dependencies) =
        partition_dependencies(&member_packages);
    let total_dependencies = member_packages
        .iter()
        .map(|pkg| pkg.dependencies.len())
        .sum();
    let security = if include_security {
        Some(fetch_security(&dependencies, &resolved_versions).await?)
    } else {
        None
    };

    let (latest_versions, registry_available, mut warnings) =
        fetch_latest_versions(&dependency_names).await;
    if include_security {
        warnings.push(
            "security scan uses Cargo.lock and reports direct workspace dependencies only"
                .to_string(),
        );
    }

    let (packages, major, minor, patch) = process_dependencies(
        dependencies,
        &resolved_versions,
        &latest_versions,
        registry_available,
        &mut skipped_packages,
    )?;

    Ok(DepsOutput {
        total: total_dependencies,
        outdated: packages.len(),
        major,
        minor,
        patch,
        packages,
        skipped: skipped_packages.len(),
        skipped_packages,
        warnings,
        security,
        workspace: is_workspace,
        members: member_names,
        skipped_members,
    })
}

fn load_metadata() -> Result<Metadata> {
    MetadataCommand::new().exec().map_err(|err| {
        UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
    })
}

fn get_resolve(metadata: &Metadata) -> Result<&cargo_metadata::Resolve> {
    metadata.resolve.as_ref().ok_or_else(|| {
        UpkeepError::message(ErrorCode::InvalidData, "metadata missing resolve data")
    })
}

fn build_packages_map(metadata: &Metadata) -> HashMap<PackageId, &Package> {
    metadata
        .packages
        .iter()
        .map(|pkg| (pkg.id.clone(), pkg))
        .collect()
}

fn workspace_packages<'a>(
    metadata: &'a Metadata,
    packages_by_id: &'a HashMap<PackageId, &'a Package>,
) -> Result<Vec<&'a Package>> {
    let mut members = Vec::new();
    for id in &metadata.workspace_members {
        let package = packages_by_id.get(id).ok_or_else(|| {
            UpkeepError::message(
                ErrorCode::InvalidData,
                format!("workspace member {} missing from packages", id),
            )
        })?;
        members.push(*package);
    }

    if members.is_empty() {
        if let Some(root) = metadata.root_package() {
            members.push(root);
        }
    }

    if members.is_empty() {
        return Err(UpkeepError::message(
            ErrorCode::InvalidData,
            "no workspace members found",
        ));
    }

    Ok(members)
}

fn find_member_nodes<'a>(
    resolve: &'a cargo_metadata::Resolve,
    members: &[&Package],
) -> Result<Vec<&'a Node>> {
    let mut nodes = Vec::new();
    for member in members {
        let node = resolve
            .nodes
            .iter()
            .find(|node| node.id == member.id)
            .ok_or_else(|| {
                UpkeepError::message(
                    ErrorCode::InvalidData,
                    format!(
                        "workspace member {} not found in resolve graph",
                        member.name
                    ),
                )
            })?;
        nodes.push(node);
    }

    Ok(nodes)
}

fn build_resolved_versions(
    nodes: &[&Node],
    packages_by_id: &HashMap<PackageId, &Package>,
) -> HashMap<String, Version> {
    let mut resolved_versions = HashMap::new();
    for node in nodes {
        for dep in &node.deps {
            if let Some(package) = packages_by_id.get(&dep.pkg) {
                resolved_versions.insert(dep.name.clone(), package.version.clone());
                resolved_versions.insert(package.name.clone(), package.version.clone());
            }
        }
    }
    resolved_versions
}

fn compute_workspace_info(
    metadata: &Metadata,
    packages_by_id: &HashMap<PackageId, &Package>,
) -> (Vec<String>, Vec<String>) {
    let member_names: Vec<String> = metadata
        .workspace_members
        .iter()
        .filter_map(|id| packages_by_id.get(id).map(|pkg| pkg.name.clone()))
        .collect();

    (member_names, Vec::new())
}

fn partition_dependencies<'a>(
    members: &'a [&'a Package],
) -> (
    HashSet<String>,
    Vec<SkippedDependency>,
    Vec<(&'a Dependency, DependencyType)>,
) {
    let mut dependency_names = HashSet::new();
    let mut skipped_packages = Vec::new();
    let mut dependencies = Vec::new();
    // Track (name, version_req) tuples to deduplicate dependencies across workspace members
    let mut seen_deps: HashSet<(String, String)> = HashSet::new();

    for member in members {
        for dep in &member.dependencies {
            let dependency_type = convert_dependency_kind(dep.kind);

            let source = dep.source.as_ref().map(ToString::to_string);
            if !is_registry_source(dep.source.as_ref()) {
                // Deduplicate skipped packages by (name, version_req)
                let key = (dep.name.clone(), dep.req.to_string());
                if seen_deps.insert(key) {
                    skipped_packages.push(SkippedDependency {
                        name: dep.name.clone(),
                        alias: dep.rename.clone(),
                        required: dep.req.to_string(),
                        reason: SkipReason::NonRegistry,
                        dependency_type,
                        source,
                        target: dep.target.as_ref().map(ToString::to_string),
                    });
                }
                continue;
            }

            // Deduplicate dependencies by (name, version_req) to avoid inflating counts
            let key = (dep.name.clone(), dep.req.to_string());
            if seen_deps.insert(key) {
                dependency_names.insert(dep.name.clone());
                dependencies.push((dep, dependency_type));
            }
        }
    }

    (dependency_names, skipped_packages, dependencies)
}

fn convert_dependency_kind(kind: DependencyKind) -> DependencyType {
    match kind {
        DependencyKind::Development => DependencyType::Dev,
        DependencyKind::Build => DependencyType::Build,
        // Normal and unknown kinds are treated as normal dependencies
        _ => DependencyType::Normal,
    }
}

async fn fetch_latest_versions(
    dependency_names: &HashSet<String>,
) -> (HashMap<String, VersionInfo>, bool, Vec<String>) {
    let names: Vec<String> = dependency_names.iter().cloned().collect();
    let mut warnings = Vec::new();

    let crates_io = match CratesIoClient::new() {
        Ok(client) => client,
        Err(err) => {
            warnings.push(format!("failed to create crates.io client: {}", err));
            return (HashMap::new(), false, warnings);
        }
    };

    match crates_io.fetch_latest_versions(&names, false).await {
        Ok(versions) => (versions, true, warnings),
        Err(err) => {
            warnings.push(format!("failed to fetch latest crate versions: {}", err));
            (HashMap::new(), false, warnings)
        }
    }
}

fn process_dependencies(
    dependencies: Vec<(&Dependency, DependencyType)>,
    resolved_versions: &HashMap<String, Version>,
    latest_versions: &HashMap<String, VersionInfo>,
    registry_available: bool,
    skipped_packages: &mut Vec<SkippedDependency>,
) -> Result<(Vec<OutdatedPackage>, usize, usize, usize)> {
    let mut packages = Vec::new();
    let mut major = 0;
    let mut minor = 0;
    let mut patch = 0;

    for (dep, dependency_type) in dependencies {
        let dep_name = dep.name.clone();
        let dep_key = dep.rename.as_deref().unwrap_or(&dep.name);

        let current = match resolve_current_version(dep_key, &dep_name, resolved_versions) {
            Some(version) => version,
            None => {
                skip_unresolved_dependency(dep, &dep_name, dependency_type, skipped_packages);
                continue;
            }
        };

        let latest = match get_latest_version(&dep_name, latest_versions) {
            Some(latest) => latest,
            None => {
                skip_missing_registry_info(
                    dep,
                    &dep_name,
                    dependency_type,
                    registry_available,
                    skipped_packages,
                );
                continue;
            }
        };

        let latest_version = Version::parse(&latest).map_err(|err| {
            UpkeepError::context(
                ErrorCode::InvalidData,
                format!(
                    "failed to parse latest version for {}: {}",
                    dep.name, latest
                ),
                err,
            )
        })?;

        if latest_version <= current {
            continue;
        }

        let update_type = classify_update(&current, &latest_version);
        match update_type {
            UpdateType::Major => major += 1,
            UpdateType::Minor => minor += 1,
            UpdateType::Patch => patch += 1,
        }

        packages.push(OutdatedPackage {
            name: dep_name,
            alias: dep.rename.clone(),
            current: current.to_string(),
            latest,
            required: dep.req.to_string(),
            update_type,
            dependency_type,
        });
    }

    Ok((packages, major, minor, patch))
}

fn resolve_current_version(
    dep_key: &str,
    dep_name: &str,
    resolved_versions: &HashMap<String, Version>,
) -> Option<Version> {
    resolved_versions
        .get(dep_key)
        .or_else(|| resolved_versions.get(dep_name))
        .cloned()
}

fn get_latest_version(
    dep_name: &str,
    latest_versions: &HashMap<String, VersionInfo>,
) -> Option<String> {
    latest_versions
        .get(dep_name)
        .and_then(|info| info.latest.clone())
}

fn skip_unresolved_dependency(
    dep: &Dependency,
    dep_name: &str,
    dependency_type: DependencyType,
    skipped_packages: &mut Vec<SkippedDependency>,
) {
    let reason = if dep.optional {
        SkipReason::OptionalNotActivated
    } else if dep.target.is_some() {
        SkipReason::TargetSpecific
    } else {
        SkipReason::MissingResolve
    };
    skipped_packages.push(SkippedDependency {
        name: dep_name.to_string(),
        alias: dep.rename.clone(),
        required: dep.req.to_string(),
        reason,
        dependency_type,
        source: dep.source.as_ref().map(ToString::to_string),
        target: dep.target.as_ref().map(ToString::to_string),
    });
}

fn skip_missing_registry_info(
    dep: &Dependency,
    dep_name: &str,
    dependency_type: DependencyType,
    registry_available: bool,
    skipped_packages: &mut Vec<SkippedDependency>,
) {
    let reason = if registry_available {
        SkipReason::RegistryMetadataMissing
    } else {
        SkipReason::RegistryUnavailable
    };
    skipped_packages.push(SkippedDependency {
        name: dep_name.to_string(),
        alias: dep.rename.clone(),
        required: dep.req.to_string(),
        reason,
        dependency_type,
        source: dep.source.as_ref().map(ToString::to_string),
        target: dep.target.as_ref().map(ToString::to_string),
    });
}

fn classify_update(current: &Version, latest: &Version) -> UpdateType {
    if latest.major > current.major {
        UpdateType::Major
    } else if latest.minor > current.minor {
        UpdateType::Minor
    } else {
        UpdateType::Patch
    }
}

async fn fetch_security(
    dependencies: &[(&Dependency, DependencyType)],
    resolved_versions: &HashMap<String, Version>,
) -> Result<DepsSecurityOutput> {
    let audit_output = tokio::task::spawn_blocking(run_audit)
        .await
        .map_err(|err| {
            let reason = if err.is_panic() {
                "panicked"
            } else {
                "was cancelled"
            };
            UpkeepError::message(ErrorCode::TaskFailed, format!("audit task {reason}: {err}"))
        })??;

    let mut vulnerabilities_by_package: HashMap<(String, String), Vec<DepsSecurityVulnerability>> =
        HashMap::new();
    for vulnerability in audit_output.vulnerabilities {
        vulnerabilities_by_package
            .entry((vulnerability.package, vulnerability.package_version))
            .or_default()
            .push(DepsSecurityVulnerability {
                advisory_id: vulnerability.id,
                severity: vulnerability.severity,
                title: vulnerability.title,
                fix_available: vulnerability.fix_available,
            });
    }

    let mut summary = AuditSummary {
        critical: 0,
        high: 0,
        moderate: 0,
        low: 0,
        total: 0,
    };
    let mut packages = Vec::new();

    for (dep, dependency_type) in dependencies {
        let dep_name = dep.name.clone();
        let dep_key = dep.rename.as_deref().unwrap_or(&dep.name);
        let current = resolved_versions
            .get(dep_key)
            .or_else(|| resolved_versions.get(&dep_name))
            .map(|version| version.to_string());
        let Some(current) = current else {
            continue;
        };

        if let Some(vulns) = vulnerabilities_by_package.remove(&(dep_name.clone(), current.clone()))
        {
            for vuln in &vulns {
                summary.total += 1;
                match vuln.severity {
                    Severity::Critical => summary.critical += 1,
                    Severity::High => summary.high += 1,
                    Severity::Moderate => summary.moderate += 1,
                    Severity::Low => summary.low += 1,
                }
            }

            packages.push(DepsSecurityPackage {
                name: dep_name,
                alias: dep.rename.clone(),
                current,
                dependency_type: *dependency_type,
                vulnerabilities: vulns,
            });
        }
    }

    Ok(DepsSecurityOutput { summary, packages })
}

fn is_registry_source(source: Option<&String>) -> bool {
    // For Dependency.source:
    // - None means crates.io (default registry)
    // - Some("registry+...") means another registry
    // - Some("git+...") or Some("path+...") are non-registry sources
    match source {
        None => true, // crates.io
        Some(s) => s.starts_with("registry+"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_update, get_latest_version, is_registry_source, resolve_current_version,
        run_with_output,
    };
    use crate::core::analyzers::crates_io::VersionInfo;
    use crate::core::error::{ErrorCode, UpkeepError};
    use crate::core::output::{
        DependencyType, DepsOutput, OutdatedPackage, SkipReason, SkippedDependency, UpdateType,
    };
    use semver::Version;
    use serde_json::Value;
    use std::collections::HashMap;

    #[test]
    fn classify_update_detects_major_minor_patch() {
        let current = Version::new(1, 2, 3);
        let major = Version::new(2, 0, 0);
        let minor = Version::new(1, 3, 0);
        let patch = Version::new(1, 2, 4);

        assert!(matches!(
            classify_update(&current, &major),
            UpdateType::Major
        ));
        assert!(matches!(
            classify_update(&current, &minor),
            UpdateType::Minor
        ));
        assert!(matches!(
            classify_update(&current, &patch),
            UpdateType::Patch
        ));
    }

    #[test]
    fn resolve_current_version_prefers_alias_key() {
        let mut versions = HashMap::new();
        versions.insert("alias".to_string(), Version::new(2, 0, 0));
        versions.insert("name".to_string(), Version::new(1, 0, 0));

        let resolved = resolve_current_version("alias", "name", &versions);
        assert_eq!(resolved, Some(Version::new(2, 0, 0)));
    }

    #[test]
    fn resolve_current_version_falls_back_to_name() {
        let mut versions = HashMap::new();
        versions.insert("name".to_string(), Version::new(1, 4, 2));

        let resolved = resolve_current_version("alias", "name", &versions);
        assert_eq!(resolved, Some(Version::new(1, 4, 2)));
    }

    #[test]
    fn get_latest_version_returns_latest_when_present() {
        let mut versions = HashMap::new();
        versions.insert(
            "serde".to_string(),
            VersionInfo {
                name: "serde".to_string(),
                latest: Some("1.0.200".to_string()),
                latest_stable: None,
            },
        );

        let latest = get_latest_version("serde", &versions);
        assert_eq!(latest.as_deref(), Some("1.0.200"));
    }

    #[test]
    fn is_registry_source_handles_registry_and_non_registry() {
        let registry = Some("registry+https://example.com".to_string());
        let git = Some("git+https://example.com/repo.git".to_string());

        assert!(is_registry_source(None));
        assert!(is_registry_source(registry.as_ref()));
        assert!(!is_registry_source(git.as_ref()));
    }

    #[test]
    fn semver_parsing_and_comparison_handles_prerelease() {
        let stable = Version::parse("1.2.3").expect("parse stable");
        let prerelease = Version::parse("1.2.3-beta.1").expect("parse prerelease");
        let next_patch = Version::parse("1.2.4-beta.1").expect("parse next prerelease");

        assert!(prerelease < stable);
        assert!(next_patch > stable);
    }

    #[test]
    fn classify_update_handles_prerelease_versions() {
        let current = Version::parse("1.2.3").expect("parse current");
        let prerelease_patch = Version::parse("1.2.4-beta.1").expect("parse prerelease");

        assert!(matches!(
            classify_update(&current, &prerelease_patch),
            UpdateType::Patch
        ));
    }

    fn sample_output() -> DepsOutput {
        DepsOutput {
            total: 2,
            outdated: 1,
            major: 1,
            minor: 0,
            patch: 0,
            packages: vec![OutdatedPackage {
                name: "serde".to_string(),
                alias: None,
                current: "1.0.0".to_string(),
                latest: "2.0.0".to_string(),
                required: "^1".to_string(),
                update_type: UpdateType::Major,
                dependency_type: DependencyType::Normal,
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
            security: None,
            workspace: false,
            members: Vec::new(),
            skipped_members: Vec::new(),
        }
    }

    #[tokio::test]
    async fn run_with_output_json_shape() {
        run_with_output(
            true,
            async { Ok(sample_output()) },
            |output| {
                let value = serde_json::to_value(output)?;
                assert_eq!(value["outdated"], Value::Number(1.into()));
                assert_eq!(value["packages"][0]["name"], Value::String("serde".into()));
                assert_eq!(
                    value["packages"][0]["update_type"],
                    Value::String("major".into())
                );
                Ok(())
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn run_with_output_propagates_error() {
        let err = run_with_output(
            true,
            async { Err(UpkeepError::message(ErrorCode::InvalidData, "bad")) },
            |_: &DepsOutput| Ok(()),
            |_: &DepsOutput| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::InvalidData);
    }
}
