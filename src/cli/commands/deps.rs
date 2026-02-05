use cargo_metadata::{
    Dependency, DependencyKind, Metadata, MetadataCommand, Node, Package, PackageId,
};
use semver::Version;
use std::collections::{HashMap, HashSet};

use crate::core::analyzers::crates_io::{CratesIoClient, VersionInfo};
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{
    print_json, DependencyType, DepsOutput, OutdatedPackage, SkipReason, SkippedDependency,
    UpdateType,
};

pub async fn run(json: bool, include_security: bool) -> Result<()> {
    let output = analyze(include_security).await?;
    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
}

pub async fn analyze(include_security: bool) -> Result<DepsOutput> {
    if include_security {
        return Err(UpkeepError::message(
            ErrorCode::InvalidData,
            "--security is not implemented yet for deps output",
        ));
    }

    let metadata = load_metadata()?;
    let root_package = get_root_package(&metadata)?;
    let resolve = get_resolve(&metadata)?;
    let packages_by_id = build_packages_map(&metadata);
    let root_node = find_root_node(resolve, root_package)?;
    let resolved_versions = build_resolved_versions(root_node, &packages_by_id);

    let (member_names, skipped_members) =
        compute_workspace_info(&metadata, &packages_by_id, root_package);

    let (dependency_names, mut skipped_packages, dependencies) =
        partition_dependencies(root_package);

    let (latest_versions, registry_available, warnings) =
        fetch_latest_versions(&dependency_names).await;

    let (packages, major, minor, patch) = process_dependencies(
        dependencies,
        &resolved_versions,
        &latest_versions,
        registry_available,
        &mut skipped_packages,
    )?;

    Ok(DepsOutput {
        total: root_package.dependencies.len(),
        outdated: packages.len(),
        major,
        minor,
        patch,
        packages,
        skipped: skipped_packages.len(),
        skipped_packages,
        warnings,
        workspace: metadata.workspace_members.len() > 1,
        members: member_names,
        skipped_members,
    })
}

fn load_metadata() -> Result<Metadata> {
    MetadataCommand::new().exec().map_err(|err| {
        UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
    })
}

fn get_root_package(metadata: &Metadata) -> Result<&Package> {
    metadata.root_package().ok_or_else(|| {
        UpkeepError::message(
            ErrorCode::InvalidData,
            "no root package found (virtual workspaces are not supported yet)",
        )
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

fn find_root_node<'a>(
    resolve: &'a cargo_metadata::Resolve,
    root_package: &Package,
) -> Result<&'a Node> {
    resolve
        .nodes
        .iter()
        .find(|node| node.id == root_package.id)
        .ok_or_else(|| {
            UpkeepError::message(
                ErrorCode::InvalidData,
                format!(
                    "root package {} not found in resolve graph",
                    root_package.name
                ),
            )
        })
}

fn build_resolved_versions(
    root_node: &Node,
    packages_by_id: &HashMap<PackageId, &Package>,
) -> HashMap<String, Version> {
    let mut resolved_versions = HashMap::new();
    for dep in &root_node.deps {
        if let Some(package) = packages_by_id.get(&dep.pkg) {
            resolved_versions.insert(dep.name.clone(), package.version.clone());
            resolved_versions.insert(package.name.clone(), package.version.clone());
        }
    }
    resolved_versions
}

fn compute_workspace_info(
    metadata: &Metadata,
    packages_by_id: &HashMap<PackageId, &Package>,
    root_package: &Package,
) -> (Vec<String>, Vec<String>) {
    let member_names: Vec<String> = metadata
        .workspace_members
        .iter()
        .filter_map(|id| packages_by_id.get(id).map(|pkg| pkg.name.clone()))
        .collect();

    let skipped_members = if metadata.workspace_members.len() > 1 {
        member_names
            .iter()
            .filter(|name| *name != &root_package.name)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    (member_names, skipped_members)
}

fn partition_dependencies(
    root_package: &Package,
) -> (
    HashSet<String>,
    Vec<SkippedDependency>,
    Vec<(&Dependency, DependencyType)>,
) {
    let mut dependency_names = HashSet::new();
    let mut skipped_packages = Vec::new();
    let mut dependencies = Vec::new();

    for dep in &root_package.dependencies {
        let dependency_type = convert_dependency_kind(dep.kind);

        let source = dep.source.as_ref().map(ToString::to_string);
        if !is_registry_source(dep.source.as_ref()) {
            skipped_packages.push(SkippedDependency {
                name: dep.name.clone(),
                alias: dep.rename.clone(),
                required: dep.req.to_string(),
                reason: SkipReason::NonRegistry,
                dependency_type,
                source,
                target: dep.target.as_ref().map(ToString::to_string),
            });
            continue;
        }

        dependency_names.insert(dep.name.clone());
        dependencies.push((dep, dependency_type));
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
