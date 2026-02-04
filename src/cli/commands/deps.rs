use anyhow::{bail, Context, Result};
use cargo_metadata::{DependencyKind, MetadataCommand, PackageId};
use semver::Version;
use std::collections::{HashMap, HashSet};

use crate::core::analyzers::crates_io::CratesIoClient;
use crate::core::output::{
    print_json, DependencyType, DepsOutput, OutdatedPackage, SkipReason, SkippedDependency,
    UpdateType,
};

pub async fn run(json: bool, include_security: bool) -> Result<()> {
    if include_security {
        bail!("--security is not implemented yet for deps output");
    }

    let metadata = MetadataCommand::new()
        .exec()
        .context("failed to load cargo metadata")?;
    let root_package = metadata
        .root_package()
        .context("no root package found (virtual workspaces are not supported yet)")?;
    let resolve = metadata
        .resolve
        .as_ref()
        .context("metadata missing resolve data")?;

    let mut packages_by_id: HashMap<PackageId, _> = HashMap::new();
    for package in &metadata.packages {
        packages_by_id.insert(package.id.clone(), package);
    }

    let root_node = resolve
        .nodes
        .iter()
        .find(|node| node.id == root_package.id)
        .with_context(|| {
            format!(
                "root package {} not found in resolve graph",
                root_package.name
            )
        })?;

    let mut resolved_versions: HashMap<String, Version> = HashMap::new();
    for dep in &root_node.deps {
        if let Some(package) = packages_by_id.get(&dep.pkg) {
            resolved_versions.insert(dep.name.clone(), package.version.clone());
            resolved_versions.insert(package.name.clone(), package.version.clone());
        }
    }

    let member_names = metadata
        .workspace_members
        .iter()
        .filter_map(|id| packages_by_id.get(id).map(|pkg| pkg.name.clone()))
        .collect::<Vec<_>>();
    let skipped_members = if metadata.workspace_members.len() > 1 {
        member_names
            .iter()
            .filter(|name| *name != &root_package.name)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let mut dependency_names = HashSet::new();
    let mut skipped_packages = Vec::new();
    let mut dependencies = Vec::new();

    for dep in &root_package.dependencies {
        let dependency_type = match dep.kind {
            DependencyKind::Normal => DependencyType::Normal,
            DependencyKind::Development => DependencyType::Dev,
            DependencyKind::Build => DependencyType::Build,
            _ => DependencyType::Normal, // Treat unknown kinds as normal dependencies
        };

        let source = dep.source.as_ref().map(|source| source.to_string());
        if !is_registry_source(dep.source.as_ref()) {
            skipped_packages.push(SkippedDependency {
                name: dep.name.clone(),
                alias: dep.rename.clone(),
                required: dep.req.to_string(),
                reason: SkipReason::NonRegistry,
                dependency_type,
                source,
                target: dep.target.as_ref().map(|target| target.to_string()),
            });
            continue;
        }

        dependency_names.insert(dep.name.clone());
        dependencies.push((dep, dependency_type));
    }

    let crates_io = CratesIoClient::new()?;
    let dependency_names = dependency_names.into_iter().collect::<Vec<_>>();
    let latest_versions = crates_io
        .fetch_latest_versions(&dependency_names, false)
        .await
        .context("failed to fetch latest crate versions")?;

    let mut packages = Vec::new();
    let mut major = 0;
    let mut minor = 0;
    let mut patch = 0;

    for (dep, dependency_type) in dependencies {
        let dep_name = dep.name.clone();
        let dep_key = dep.rename.as_deref().unwrap_or(&dep.name);
        let current = match resolved_versions
            .get(dep_key)
            .or_else(|| resolved_versions.get(&dep_name))
        {
            Some(version) => version.clone(),
            None => {
                let reason = if dep.optional {
                    SkipReason::OptionalNotActivated
                } else if dep.target.is_some() {
                    SkipReason::TargetSpecific
                } else {
                    SkipReason::MissingResolve
                };
                skipped_packages.push(SkippedDependency {
                    name: dep_name,
                    alias: dep.rename.clone(),
                    required: dep.req.to_string(),
                    reason,
                    dependency_type,
                    source: dep.source.as_ref().map(|source| source.to_string()),
                    target: dep.target.as_ref().map(|target| target.to_string()),
                });
                continue;
            }
        };

        let latest = match latest_versions
            .get(&dep_name)
            .and_then(|info| info.latest.clone())
        {
            Some(latest) => latest,
            None => {
                skipped_packages.push(SkippedDependency {
                    name: dep_name,
                    alias: dep.rename.clone(),
                    required: dep.req.to_string(),
                    reason: SkipReason::RegistryMetadataMissing,
                    dependency_type,
                    source: dep.source.as_ref().map(|source| source.to_string()),
                    target: dep.target.as_ref().map(|target| target.to_string()),
                });
                continue;
            }
        };

        let latest_version = Version::parse(&latest).with_context(|| {
            format!(
                "failed to parse latest version for {}: {}",
                dep.name, latest
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

    let output = DepsOutput {
        total: root_package.dependencies.len(),
        outdated: packages.len(),
        major,
        minor,
        patch,
        packages,
        skipped: skipped_packages.len(),
        skipped_packages,
        workspace: metadata.workspace_members.len() > 1,
        members: member_names,
        skipped_members,
    };

    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
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
