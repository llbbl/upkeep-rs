use cargo_metadata::{
    Dependency, DependencyKind, Metadata, MetadataCommand, Node, Package, PackageId,
};
use semver::Version;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
    let member_versions = build_resolved_versions(&member_packages, &member_nodes, &packages_by_id);

    let (member_names, skipped_members) = compute_workspace_info(&metadata, &packages_by_id);

    let mut skipped = SkippedCollector::new();
    let (dependency_names, edges) = partition_dependencies(&member_packages, &mut skipped);
    let total_dependencies = member_packages
        .iter()
        .map(|pkg| pkg.dependencies.len())
        .sum();

    let dependencies = resolve_dependencies(edges, &member_versions, &mut skipped);

    let security = if include_security {
        Some(fetch_security(&dependencies).await?)
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

    let Processed {
        packages,
        major,
        minor,
        patch,
        compared,
    } = process_dependencies(
        dependencies,
        &latest_versions,
        registry_available,
        &mut skipped,
    )?;

    let skipped_packages = skipped.into_vec();

    Ok(DepsOutput {
        total: total_dependencies,
        checked: compared + not_applicable_count(&skipped_packages),
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

/// Look up the resolve-graph node for each workspace member.
///
/// The returned nodes are positionally parallel to `members`; the rest of this
/// module relies on that pairing to attribute a resolved version to the member
/// that actually declared the edge.
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

/// Resolved dependency versions for a single workspace member.
///
/// One map per member is what makes workspace resolution correct: members may
/// resolve the same crate name to semver-incompatible versions, and a single flat
/// map across the whole workspace collapses them last-write-wins.
#[derive(Debug, Default)]
struct MemberVersions {
    member: String,
    /// Keyed by the resolve-graph dep name: the lib target name with any rename
    /// applied (`serde_json`, or the alias for a renamed dependency).
    by_dep_name: HashMap<String, Version>,
    /// Keyed by the real package name (`serde-json`). Manifest dependency names use
    /// the package name, which differs from the lib target name whenever the crate
    /// name contains a dash.
    ///
    /// Cargo emits `node.deps` in package-id order, which says nothing about whether
    /// an edge was renamed, so when one package name has several resolved instances
    /// the first writer here is arbitrary. This map is therefore only a last-resort
    /// fallback in [`resolve_current_version`], reached after both `by_dep_name`
    /// lookups miss; a renamed edge never consults it at all.
    by_package_name: HashMap<String, Version>,
}

fn build_resolved_versions(
    members: &[&Package],
    nodes: &[&Node],
    packages_by_id: &HashMap<PackageId, &Package>,
) -> Vec<MemberVersions> {
    // `find_member_nodes` returns nodes positionally parallel to `members`; the
    // `zip` below silently truncates if that ever stops holding.
    debug_assert_eq!(members.len(), nodes.len());
    members
        .iter()
        .zip(nodes.iter())
        .map(|(member, node)| {
            let mut versions = MemberVersions {
                member: member.name.to_string(),
                ..MemberVersions::default()
            };
            for dep in &node.deps {
                if let Some(package) = packages_by_id.get(&dep.pkg) {
                    versions
                        .by_dep_name
                        .insert(dep.name.to_string(), package.version.clone());
                    versions
                        .by_package_name
                        .entry(package.name.to_string())
                        .or_insert_with(|| package.version.clone());
                }
            }
            versions
        })
        .collect()
}

fn compute_workspace_info(
    metadata: &Metadata,
    packages_by_id: &HashMap<PackageId, &Package>,
) -> (Vec<String>, Vec<String>) {
    let member_names: Vec<String> = metadata
        .workspace_members
        .iter()
        .filter_map(|id| packages_by_id.get(id).map(|pkg| pkg.name.to_string()))
        .collect();

    (member_names, Vec::new())
}

/// Collects skipped dependencies, deduplicating identical reports.
///
/// The same dependency can be declared by several workspace members; when it is
/// skipped for the same reason each time we only want to report it once.
#[derive(Debug, Default)]
struct SkippedCollector {
    seen: HashSet<(String, Option<String>, String, SkipReason)>,
    items: Vec<SkippedDependency>,
}

impl SkippedCollector {
    fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, skipped: SkippedDependency) {
        let key = (
            skipped.name.clone(),
            skipped.alias.clone(),
            skipped.required.clone(),
            skipped.reason,
        );
        if self.seen.insert(key) {
            self.items.push(skipped);
        }
    }

    fn into_vec(self) -> Vec<SkippedDependency> {
        self.items
    }
}

/// A single dependency declaration by a single workspace member.
///
/// Unlike the pre-grouping representation this keeps the declaring member and the
/// dependency kind, so neither is lost before the resolved version is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DependencyEdge {
    name: String,
    alias: Option<String>,
    required: String,
    source: Option<String>,
    target: Option<String>,
    optional: bool,
    dependency_type: DependencyType,
    /// Index into the `MemberVersions` slice built by [`build_resolved_versions`].
    member_index: usize,
}

fn partition_dependencies(
    members: &[&Package],
    skipped: &mut SkippedCollector,
) -> (HashSet<String>, Vec<DependencyEdge>) {
    let mut dependency_names = HashSet::new();
    let mut edges = Vec::new();

    for (member_index, member) in members.iter().enumerate() {
        for dep in &member.dependencies {
            let edge = build_edge(dep, member_index);

            if !is_registry_source(edge.source.as_ref()) {
                skipped.push(SkippedDependency {
                    name: edge.name,
                    alias: edge.alias,
                    required: edge.required,
                    reason: SkipReason::NonRegistry,
                    dependency_type: edge.dependency_type,
                    source: edge.source,
                    target: edge.target,
                });
                continue;
            }

            // Every edge is kept: deduplicating here by (name, version_req) is what
            // discarded the declaring member and the dependency kind. Edges are merged
            // later by (name, resolved version) instead.
            dependency_names.insert(edge.name.clone());
            edges.push(edge);
        }
    }

    (dependency_names, edges)
}

fn build_edge(dep: &Dependency, member_index: usize) -> DependencyEdge {
    DependencyEdge {
        name: dep.name.clone(),
        alias: dep.rename.clone(),
        required: dep.req.to_string(),
        source: dep.source.as_ref().map(ToString::to_string),
        target: dep.target.as_ref().map(ToString::to_string),
        optional: dep.optional,
        dependency_type: convert_dependency_kind(dep.kind),
        member_index,
    }
}

fn convert_dependency_kind(kind: DependencyKind) -> DependencyType {
    match kind {
        DependencyKind::Development => DependencyType::Dev,
        DependencyKind::Build => DependencyType::Build,
        // Normal and unknown kinds are treated as normal dependencies
        _ => DependencyType::Normal,
    }
}

/// One reported dependency: all edges that share a `(name, resolved version)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedDependency {
    name: String,
    alias: Option<String>,
    current: Version,
    required: String,
    source: Option<String>,
    target: Option<String>,
    dependency_type: DependencyType,
    /// Declaring workspace members, sorted and deduplicated.
    members: Vec<String>,
}

/// Precedence used when one `(name, resolved version)` group spans several kinds:
/// normal > build > dev. A dependency that is a normal dependency for any member is
/// reported as normal, because that is the strongest claim any member makes on it.
fn dependency_type_rank(dependency_type: DependencyType) -> u8 {
    match dependency_type {
        DependencyType::Normal => 2,
        DependencyType::Build => 1,
        DependencyType::Dev => 0,
    }
}

fn merge_dependency_type(current: DependencyType, other: DependencyType) -> DependencyType {
    if dependency_type_rank(other) > dependency_type_rank(current) {
        other
    } else {
        current
    }
}

/// Fields taken from a single representative edge when a group spans several edges.
///
/// Members can declare different requirement strings for the same resolved version
/// (`^0.8` and `>=0.8, <0.9`), so one edge has to win. The winner is the smallest
/// `(member, required, alias)` tuple, which makes the choice independent of the
/// order cargo happens to report members in. Ties on the member name fall through to
/// byte-order on the requirement string, not to any semver ordering.
///
/// Reporting one edge's `required` for the whole group is lossy but not misleading,
/// and the invariant that makes it so is enforced by [`resolve_current_version`]:
/// every edge in a group resolved to the *same* version through its own member's
/// resolve graph, and cargo only unifies edges whose requirements that version
/// actually satisfies. So while the group's `required` is one member's phrasing,
/// every other requirement it stands in for is genuinely satisfied by the reported
/// `current`. Letting an edge borrow another edge's resolved version would break
/// exactly that invariant, which is why the alias path never falls back.
type RepresentativeKey = (String, String, Option<String>);

#[derive(Debug)]
struct DependencyGroup {
    representative: RepresentativeKey,
    source: Option<String>,
    target: Option<String>,
    dependency_type: DependencyType,
    members: BTreeSet<String>,
}

/// Resolve every edge against its declaring member and merge the results.
///
/// Edges that cannot be resolved (optional and not activated, target-specific for
/// another platform, or otherwise absent from the resolve graph) are reported as
/// skipped instead.
///
/// The returned entries are ordered by `(name, current)`; `BTreeMap` gives that for
/// free and orders versions by semver rather than lexicographically.
fn resolve_dependencies(
    edges: Vec<DependencyEdge>,
    member_versions: &[MemberVersions],
    skipped: &mut SkippedCollector,
) -> Vec<ResolvedDependency> {
    let mut groups: BTreeMap<(String, Version), DependencyGroup> = BTreeMap::new();

    for edge in edges {
        let Some(versions) = member_versions.get(edge.member_index) else {
            // Unreachable while edges are built from the same member slice; treat a
            // desynchronised index as an unresolved edge rather than panicking.
            skipped.push(unresolved_skip(&edge));
            continue;
        };

        let Some(current) = resolve_current_version(edge.alias.as_deref(), &edge.name, versions)
        else {
            skipped.push(unresolved_skip(&edge));
            continue;
        };

        let representative = (
            versions.member.clone(),
            edge.required.clone(),
            edge.alias.clone(),
        );

        groups
            .entry((edge.name.clone(), current))
            .and_modify(|group| {
                group.dependency_type =
                    merge_dependency_type(group.dependency_type, edge.dependency_type);
                group.members.insert(versions.member.clone());
                if representative < group.representative {
                    group.representative = representative.clone();
                    group.source = edge.source.clone();
                    group.target = edge.target.clone();
                }
            })
            .or_insert_with(|| DependencyGroup {
                representative,
                source: edge.source.clone(),
                target: edge.target.clone(),
                dependency_type: edge.dependency_type,
                members: BTreeSet::from_iter([versions.member.clone()]),
            });
    }

    groups
        .into_iter()
        .map(|((name, current), group)| ResolvedDependency {
            name,
            alias: group.representative.2,
            current,
            required: group.representative.1,
            source: group.source,
            target: group.target,
            dependency_type: group.dependency_type,
            members: group.members.into_iter().collect(),
        })
        .collect()
}

fn unresolved_skip(edge: &DependencyEdge) -> SkippedDependency {
    let reason = if edge.optional {
        SkipReason::OptionalNotActivated
    } else if edge.target.is_some() {
        SkipReason::TargetSpecific
    } else {
        SkipReason::MissingResolve
    };
    SkippedDependency {
        name: edge.name.clone(),
        alias: edge.alias.clone(),
        required: edge.required.clone(),
        reason,
        dependency_type: edge.dependency_type,
        source: edge.source.clone(),
        target: edge.target.clone(),
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

/// Counts the skips that mean "there was never a comparison to make".
///
/// A git, path or inactive-optional dependency has no crates.io release to be
/// behind, so it is neither outdated nor unchecked — it belongs in the freshness
/// denominator, as one unit, exactly like a group that was compared and found
/// current. `RegistryUnavailable` and `RegistryMetadataMissing` are the opposite
/// case: a comparison was owed and could not be made, so they stay out.
///
/// These entries are already deduplicated by [`SkippedCollector`] on
/// `(name, alias, required, reason)`, so a crate declared by three members
/// contributes one — the same collapsing that `resolve_dependencies` applies to
/// the compared ones. Counting the raw edges here instead is precisely the
/// unit mismatch this function exists to avoid.
fn not_applicable_count(skipped: &[SkippedDependency]) -> usize {
    skipped
        .iter()
        .filter(|entry| {
            matches!(
                entry.reason,
                SkipReason::NonRegistry
                    | SkipReason::TargetSpecific
                    | SkipReason::OptionalNotActivated
            )
        })
        .count()
}

/// The outcome of comparing every resolved dependency against the registry.
struct Processed {
    packages: Vec<OutdatedPackage>,
    major: usize,
    minor: usize,
    patch: usize,
    /// Resolved groups whose latest version was fetched and compared, current
    /// ones included. This is the honest denominator for freshness, in the same
    /// unit as `packages.len()` — never `DepsOutput::total`, which counts edges.
    compared: usize,
}

fn process_dependencies(
    dependencies: Vec<ResolvedDependency>,
    latest_versions: &HashMap<String, VersionInfo>,
    registry_available: bool,
    skipped: &mut SkippedCollector,
) -> Result<Processed> {
    let mut packages = Vec::new();
    let mut major = 0;
    let mut minor = 0;
    let mut patch = 0;
    let mut compared = 0;

    for dependency in dependencies {
        let latest = match get_latest_version(&dependency.name, latest_versions) {
            Some(latest) => latest,
            None => {
                skipped.push(missing_registry_skip(&dependency, registry_available));
                continue;
            }
        };

        let latest_version = Version::parse(&latest).map_err(|err| {
            UpkeepError::context(
                ErrorCode::InvalidData,
                format!(
                    "failed to parse latest version for {}: {}",
                    dependency.name, latest
                ),
                err,
            )
        })?;

        // Past this point the comparison has been made, whatever its outcome.
        compared += 1;

        if latest_version <= dependency.current {
            continue;
        }

        let update_type = classify_update(&dependency.current, &latest_version);
        match update_type {
            UpdateType::Major => major += 1,
            UpdateType::Minor => minor += 1,
            UpdateType::Patch => patch += 1,
        }

        packages.push(OutdatedPackage {
            name: dependency.name,
            alias: dependency.alias,
            current: dependency.current.to_string(),
            latest,
            required: dependency.required,
            update_type,
            dependency_type: dependency.dependency_type,
            members: dependency.members,
        });
    }

    Ok(Processed {
        packages,
        major,
        minor,
        patch,
        compared,
    })
}

fn resolve_current_version(
    alias: Option<&str>,
    dep_name: &str,
    versions: &MemberVersions,
) -> Option<Version> {
    // A renamed edge is keyed in the resolve graph by its rename only. Falling back
    // to the package name here would borrow a different edge's resolved version.
    if let Some(alias) = alias {
        // `dep.rename` is the raw Cargo.toml key, but the resolve graph normalizes it
        // the same way it does target names, so a dashed rename (`"my-itoa" = { package
        // = "itoa" }`) is keyed `my_itoa`. Stay inside `by_dep_name`: it is keyed by
        // rename, so neither spelling can reach another edge's resolved version.
        return versions
            .by_dep_name
            .get(alias)
            .or_else(|| versions.by_dep_name.get(&alias.replace('-', "_")))
            .cloned();
    }

    versions
        .by_dep_name
        .get(dep_name)
        // Manifest names use the package name; the resolve graph uses the lib target
        // name, which is the package name with dashes replaced (`pretty-assertions`
        // -> `pretty_assertions`).
        .or_else(|| versions.by_dep_name.get(&dep_name.replace('-', "_")))
        // Last resort: a crate with a custom `[lib] name` matching neither.
        .or_else(|| versions.by_package_name.get(dep_name))
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

fn missing_registry_skip(
    dependency: &ResolvedDependency,
    registry_available: bool,
) -> SkippedDependency {
    let reason = if registry_available {
        SkipReason::RegistryMetadataMissing
    } else {
        SkipReason::RegistryUnavailable
    };
    SkippedDependency {
        name: dependency.name.clone(),
        alias: dependency.alias.clone(),
        required: dependency.required.clone(),
        reason,
        dependency_type: dependency.dependency_type,
        source: dependency.source.clone(),
        target: dependency.target.clone(),
    }
}

/// Whether cargo considers `latest` a semver-incompatible upgrade from `current`.
///
/// Cargo's rule is that the leftmost non-zero component must match, so which
/// component carries breakage depends on the version's shape: `1.x` breaks on the
/// major, `0.x` breaks on the minor, and `0.0.z` breaks on every patch.
fn is_semver_breaking(current: &Version, latest: &Version) -> bool {
    if current.major != latest.major {
        return true;
    }
    if current.major != 0 {
        return false;
    }
    // 0.x: the minor is the leftmost non-zero component, so `0.8 -> 0.10` breaks.
    if current.minor != latest.minor {
        return true;
    }
    if current.minor != 0 {
        return false;
    }
    // 0.0.z: nothing is compatible, so every patch bump breaks.
    current.patch != latest.patch
}

fn classify_update(current: &Version, latest: &Version) -> UpdateType {
    if is_semver_breaking(current, latest) {
        UpdateType::Major
    } else if latest.minor > current.minor {
        UpdateType::Minor
    } else {
        UpdateType::Patch
    }
}

async fn fetch_security(dependencies: &[ResolvedDependency]) -> Result<DepsSecurityOutput> {
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

    // `dependencies` is already grouped by (name, resolved version), which is exactly
    // how advisories are keyed, so each group matches at most one advisory bucket.
    for dependency in dependencies {
        let current = dependency.current.to_string();
        if let Some(vulns) =
            vulnerabilities_by_package.remove(&(dependency.name.clone(), current.clone()))
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
                name: dependency.name.clone(),
                alias: dependency.alias.clone(),
                current,
                dependency_type: dependency.dependency_type,
                members: dependency.members.clone(),
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
        classify_update, get_latest_version, is_registry_source, merge_dependency_type,
        not_applicable_count, process_dependencies, resolve_current_version, resolve_dependencies,
        run_with_output, DependencyEdge, MemberVersions, Processed, SkippedCollector,
    };
    use crate::core::analyzers::crates_io::VersionInfo;
    use crate::core::error::{ErrorCode, UpkeepError};
    use crate::core::output::{
        DependencyType, DepsOutput, OutdatedPackage, SkipReason, SkippedDependency, UpdateType,
    };
    use semver::Version;
    use serde_json::Value;
    use std::collections::HashMap;

    /// Build a member's resolve maps the way cargo actually shapes them.
    ///
    /// `by_dep_name` is keyed by the lib target name (the package name with dashes
    /// replaced by underscores); `by_package_name` is keyed by the real package name.
    /// The two keys diverge exactly when the crate name contains a dash. Writing the
    /// *same* key into both maps — as this fixture used to — makes every lookup path
    /// in `resolve_current_version` look interchangeable, which is how an incorrect
    /// fallback chain passed the whole unit suite.
    ///
    /// `deps` takes package names; pass a dashed name to exercise the divergent shape.
    fn member_versions(member: &str, deps: &[(&str, &str)]) -> MemberVersions {
        let mut versions = MemberVersions {
            member: member.to_string(),
            ..MemberVersions::default()
        };
        for (name, version) in deps {
            let parsed = Version::parse(version).expect("parse version");
            versions
                .by_dep_name
                .insert(name.replace('-', "_"), parsed.clone());
            versions
                .by_package_name
                .entry((*name).to_string())
                .or_insert(parsed);
        }
        versions
    }

    /// Register a renamed edge's resolve-graph entry: cargo keys it by the rename and
    /// by nothing else, so it deliberately touches neither `by_package_name` nor the
    /// package's lib target name.
    fn insert_renamed(versions: &mut MemberVersions, rename: &str, version: &str) {
        versions.by_dep_name.insert(
            rename.to_string(),
            Version::parse(version).expect("parse version"),
        );
    }

    fn aliased_edge(
        name: &str,
        alias: &str,
        required: &str,
        member_index: usize,
    ) -> DependencyEdge {
        DependencyEdge {
            alias: Some(alias.to_string()),
            ..edge(name, required, member_index)
        }
    }

    fn edge(name: &str, required: &str, member_index: usize) -> DependencyEdge {
        DependencyEdge {
            name: name.to_string(),
            alias: None,
            required: required.to_string(),
            source: None,
            target: None,
            optional: false,
            dependency_type: DependencyType::Normal,
            member_index,
        }
    }

    #[test]
    fn classify_update_detects_major_minor_patch() {
        // Cargo compatibility is decided by the leftmost non-zero component, so the
        // component that carries breakage moves with the version's shape.
        let cases: &[(&str, &str, UpdateType)] = &[
            // 1.x: the major carries breakage.
            ("1.2.3", "2.0.0", UpdateType::Major),
            ("1.2.3", "1.3.0", UpdateType::Minor),
            ("1.2.3", "1.2.4", UpdateType::Patch),
            // 0.x: the minor carries it. These were all reported Minor before.
            ("0.8.5", "0.10.2", UpdateType::Major),
            ("0.1.0", "0.2.0", UpdateType::Major),
            ("0.8.1", "0.8.5", UpdateType::Patch),
            // Leaving 0.x entirely is breaking too.
            ("0.9.0", "1.0.0", UpdateType::Major),
            // 0.0.z: nothing is compatible. This was reported Patch before.
            ("0.0.1", "0.0.2", UpdateType::Major),
            ("0.0.1", "0.1.0", UpdateType::Major),
        ];

        for (current, latest, expected) in cases {
            let current = Version::parse(current).expect("parse current");
            let latest = Version::parse(latest).expect("parse latest");
            let actual = classify_update(&current, &latest);
            assert_eq!(
                &actual, expected,
                "{current} -> {latest} classified {actual:?}, expected {expected:?}"
            );
        }
    }

    #[test]
    fn resolve_current_version_prefers_alias_key() {
        let mut versions = MemberVersions {
            member: "member".to_string(),
            ..MemberVersions::default()
        };
        versions
            .by_dep_name
            .insert("alias".to_string(), Version::new(2, 0, 0));
        versions
            .by_dep_name
            .insert("name".to_string(), Version::new(1, 0, 0));

        let resolved = resolve_current_version(Some("alias"), "name", &versions);
        assert_eq!(resolved, Some(Version::new(2, 0, 0)));
    }

    #[test]
    fn resolve_current_version_does_not_fall_back_from_a_missing_alias() {
        // A renamed edge appears in the resolve graph under its rename and nowhere
        // else, so a missing alias means the edge is genuinely unresolved. Falling
        // through to the package name here would hand this edge the *plain* edge's
        // resolved version and silently fabricate a row.
        let mut versions = MemberVersions {
            member: "member".to_string(),
            ..MemberVersions::default()
        };
        versions
            .by_dep_name
            .insert("name".to_string(), Version::new(1, 4, 2));
        versions
            .by_package_name
            .insert("name".to_string(), Version::new(1, 4, 2));

        assert_eq!(
            resolve_current_version(Some("alias"), "name", &versions),
            None
        );
    }

    #[test]
    fn resolve_current_version_normalizes_a_dashed_rename() {
        // `dep.rename` is the raw manifest key, but cargo keys the resolve graph by the
        // normalized form: `"my-itoa" = { package = "itoa" }` yields rename `my-itoa`
        // and node dep name `my_itoa`. Without normalization the alias path misses and
        // the edge is wrongly reported as `missing_resolve` despite being resolved.
        let mut versions = MemberVersions {
            member: "member".to_string(),
            ..MemberVersions::default()
        };
        versions
            .by_dep_name
            .insert("my_itoa".to_string(), Version::new(0, 4, 8));
        // The plain edge on the same package sits alongside it at a different version.
        versions
            .by_dep_name
            .insert("itoa".to_string(), Version::new(1, 0, 18));

        assert_eq!(
            resolve_current_version(Some("my-itoa"), "itoa", &versions),
            Some(Version::new(0, 4, 8)),
            "a dashed rename must resolve to its own edge, not the plain one"
        );
    }

    #[test]
    fn resolve_current_version_falls_back_to_lib_target_name() {
        // `wasm-bindgen` is a real dash-named crate: its lib target is `wasm_bindgen`,
        // so the manifest name never matches the resolve-graph key directly.
        let versions = member_versions("member", &[("wasm-bindgen", "0.2.100")]);
        assert!(!versions.by_dep_name.contains_key("wasm-bindgen"));

        let resolved = resolve_current_version(None, "wasm-bindgen", &versions);
        assert_eq!(resolved, Some(Version::parse("0.2.100").expect("parse")));
    }

    #[test]
    fn resolve_current_version_falls_back_to_package_name() {
        // Last resort: a crate with a custom `[lib] name` that matches neither the
        // package name nor its dash-substituted form.
        let mut versions = MemberVersions {
            member: "member".to_string(),
            ..MemberVersions::default()
        };
        versions
            .by_dep_name
            .insert("custom_lib_name".to_string(), Version::new(1, 4, 0));
        versions
            .by_package_name
            .insert("odd-crate".to_string(), Version::new(1, 4, 0));

        let resolved = resolve_current_version(None, "odd-crate", &versions);
        assert_eq!(resolved, Some(Version::new(1, 4, 0)));
    }

    #[test]
    fn resolve_current_version_returns_none_when_absent() {
        let versions = MemberVersions {
            member: "member".to_string(),
            ..MemberVersions::default()
        };

        assert_eq!(resolve_current_version(None, "name", &versions), None);
        assert_eq!(
            resolve_current_version(Some("alias"), "name", &versions),
            None
        );
    }

    #[test]
    fn merge_dependency_type_prefers_normal_then_build() {
        assert_eq!(
            merge_dependency_type(DependencyType::Dev, DependencyType::Normal),
            DependencyType::Normal
        );
        assert_eq!(
            merge_dependency_type(DependencyType::Normal, DependencyType::Dev),
            DependencyType::Normal
        );
        assert_eq!(
            merge_dependency_type(DependencyType::Dev, DependencyType::Build),
            DependencyType::Build
        );
        assert_eq!(
            merge_dependency_type(DependencyType::Build, DependencyType::Dev),
            DependencyType::Build
        );
    }

    #[test]
    fn resolve_dependencies_splits_semver_incompatible_versions() {
        // The regression this module exists for: two members resolving `rand` to
        // semver-incompatible versions must not collapse into one entry.
        let members = vec![
            member_versions("core-lib", &[("rand", "0.8.5")]),
            member_versions("cli-app", &[("rand", "0.9.2")]),
        ];
        let edges = vec![edge("rand", "^0.8", 0), edge("rand", "^0.9", 1)];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].current, Version::new(0, 8, 5));
        assert_eq!(resolved[0].members, vec!["core-lib".to_string()]);
        assert_eq!(resolved[0].required, "^0.8");
        assert_eq!(resolved[1].current, Version::new(0, 9, 2));
        assert_eq!(resolved[1].members, vec!["cli-app".to_string()]);
        assert_eq!(resolved[1].required, "^0.9");
        assert!(skipped.into_vec().is_empty());
    }

    #[test]
    fn resolve_dependencies_merges_agreeing_members() {
        let members = vec![
            member_versions("member-b", &[("serde", "1.0.200")]),
            member_versions("member-a", &[("serde", "1.0.200")]),
        ];
        let edges = vec![edge("serde", "^1.0", 0), edge("serde", "^1.0", 1)];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        assert_eq!(resolved.len(), 1);
        // Members are sorted, not reported in cargo's member order.
        assert_eq!(
            resolved[0].members,
            vec!["member-a".to_string(), "member-b".to_string()]
        );
    }

    #[test]
    fn resolve_dependencies_merges_differing_requirements_on_one_version() {
        // Cargo unifies semver-compatible requirements, so `^1.0` and `^1.0.100`
        // resolve to a single version and must produce a single entry. The reported
        // `required` comes from the first member in sorted order, which makes the
        // choice independent of cargo's member ordering.
        let members = vec![
            member_versions("member-b", &[("serde", "1.0.200")]),
            member_versions("member-a", &[("serde", "1.0.200")]),
        ];
        let edges = vec![edge("serde", "^1.0", 0), edge("serde", "^1.0.100", 1)];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].required, "^1.0.100");
        assert_eq!(
            resolved[0].members,
            vec!["member-a".to_string(), "member-b".to_string()]
        );
    }

    #[test]
    fn resolve_dependencies_populates_members_for_single_crate() {
        let members = vec![member_versions("solo", &[("serde", "1.0.200")])];
        let edges = vec![edge("serde", "^1.0", 0)];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].members, vec!["solo".to_string()]);
    }

    #[test]
    fn resolve_dependencies_uses_rename_for_lookup() {
        // `rand9 = { package = "rand", version = "0.9" }` alongside plain `rand 0.8`:
        // the alias is the resolve-graph key, the package name is the reported name.
        let mut versions = MemberVersions {
            member: "solo".to_string(),
            ..MemberVersions::default()
        };
        versions
            .by_dep_name
            .insert("rand".to_string(), Version::new(0, 8, 5));
        versions
            .by_dep_name
            .insert("rand9".to_string(), Version::new(0, 9, 2));
        versions
            .by_package_name
            .insert("rand".to_string(), Version::new(0, 8, 5));

        let mut renamed = edge("rand", "^0.9", 0);
        renamed.alias = Some("rand9".to_string());
        let edges = vec![edge("rand", "^0.8", 0), renamed];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &[versions], &mut skipped);

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].current, Version::new(0, 8, 5));
        assert_eq!(resolved[0].alias, None);
        assert_eq!(resolved[1].current, Version::new(0, 9, 2));
        assert_eq!(resolved[1].alias, Some("rand9".to_string()));
    }

    #[test]
    fn resolve_dependencies_keeps_a_plain_dash_named_edge_off_an_aliased_instance() {
        // Repro (a): `wasm-bindgen = "=0.2.100"` plus
        // `wb1 = { package = "wasm-bindgen", version = "0.1" }`.
        //
        // Cargo emits `node.deps` in package-id order, so first-writer-wins can pin
        // `by_package_name["wasm-bindgen"]` to the *aliased* instance's 0.1.0. The
        // plain edge must still find 0.2.100 via the lib target name rather than
        // borrowing 0.1.0 and reporting the impossible pair `current 0.1.0` /
        // `required =0.2.100`.
        let mut versions = MemberVersions {
            member: "solo".to_string(),
            ..MemberVersions::default()
        };
        versions.by_dep_name.insert(
            "wasm_bindgen".to_string(),
            Version::parse("0.2.100").expect("parse"),
        );
        insert_renamed(&mut versions, "wb1", "0.1.0");
        versions
            .by_package_name
            .insert("wasm-bindgen".to_string(), Version::new(0, 1, 0));

        let edges = vec![
            edge("wasm-bindgen", "=0.2.100", 0),
            aliased_edge("wasm-bindgen", "wb1", "^0.1", 0),
        ];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &[versions], &mut skipped);

        assert_eq!(resolved.len(), 2, "the two instances must not collapse");
        assert_eq!(resolved[0].current, Version::new(0, 1, 0));
        assert_eq!(resolved[0].alias, Some("wb1".to_string()));
        assert_eq!(resolved[0].required, "^0.1");
        assert_eq!(
            resolved[1].current,
            Version::parse("0.2.100").expect("parse")
        );
        assert_eq!(resolved[1].alias, None);
        assert_eq!(resolved[1].required, "=0.2.100");
        assert!(skipped.into_vec().is_empty());
    }

    #[test]
    fn resolve_dependencies_skips_an_inactive_optional_alias_of_a_declared_crate() {
        // Repro (b): `rand = "0.8"` plus
        // `rand7 = { package = "rand", version = "0.7", optional = true }` that is not
        // activated, so `rand7` is absent from the resolve graph.
        //
        // Borrowing the plain edge's 0.8.8 would not merely add a spurious row: the
        // borrowed edge joins the same group and, because `"^0.7" < "^0.8"` in the
        // representative tuple, *replaces* the correct requirement.
        let versions = member_versions("solo", &[("rand", "0.8.8")]);

        let mut optional = aliased_edge("rand", "rand7", "^0.7", 0);
        optional.optional = true;
        let edges = vec![edge("rand", "^0.8", 0), optional];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &[versions], &mut skipped);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].current, Version::new(0, 8, 8));
        assert_eq!(resolved[0].required, "^0.8");
        assert_eq!(resolved[0].alias, None);

        let skipped = skipped.into_vec();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].alias, Some("rand7".to_string()));
        assert_eq!(skipped[0].reason, SkipReason::OptionalNotActivated);
    }

    #[test]
    fn resolve_dependencies_does_not_alias_onto_a_dash_named_lib_target() {
        // The two fallbacks interacting: an unresolved alias of a dash-named crate
        // must not reach that crate's lib target key, which the plain edge owns.
        let versions = member_versions("solo", &[("wasm-bindgen", "0.2.100")]);

        let edges = vec![
            edge("wasm-bindgen", "=0.2.100", 0),
            aliased_edge("wasm-bindgen", "wb1", "^0.1", 0),
        ];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &[versions], &mut skipped);

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].current,
            Version::parse("0.2.100").expect("parse")
        );
        assert_eq!(resolved[0].required, "=0.2.100");

        let skipped = skipped.into_vec();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].alias, Some("wb1".to_string()));
        assert_eq!(skipped[0].reason, SkipReason::MissingResolve);
    }

    #[test]
    fn resolve_dependencies_keys_security_attribution_per_resolved_version() {
        // `fetch_security` matches advisories on `(name, current)`, so grouping is
        // what decides which member a vulnerable version is attributed to. Two members
        // on semver-incompatible `time` versions must yield two distinct keys; the
        // advisory lookup itself is covered by the audit tests.
        let members = vec![
            member_versions("core-lib", &[("time", "0.1.45")]),
            member_versions("cli-app", &[("time", "0.2.27")]),
        ];
        let edges = vec![edge("time", "^0.1", 0), edge("time", "^0.2", 1)];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        let keys: Vec<(String, String)> = resolved
            .iter()
            .map(|dep| (dep.name.clone(), dep.current.to_string()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("time".to_string(), "0.1.45".to_string()),
                ("time".to_string(), "0.2.27".to_string()),
            ]
        );
        // Each key carries only the member that actually resolved to it, which is the
        // attribution `DepsSecurityPackage::members` now reports.
        assert_eq!(resolved[0].members, vec!["core-lib".to_string()]);
        assert_eq!(resolved[1].members, vec!["cli-app".to_string()]);
        assert!(skipped.into_vec().is_empty());
    }

    #[test]
    fn resolve_dependencies_merges_dependency_kinds_by_precedence() {
        let members = vec![
            member_versions("core-lib", &[("serde", "1.0.200")]),
            member_versions("cli-app", &[("serde", "1.0.200")]),
        ];
        let mut dev_edge = edge("serde", "^1.0", 0);
        dev_edge.dependency_type = DependencyType::Dev;
        let edges = vec![dev_edge, edge("serde", "^1.0", 1)];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].dependency_type, DependencyType::Normal);
    }

    #[test]
    fn resolve_dependencies_is_sorted_by_name_then_version() {
        let members = vec![member_versions(
            "solo",
            &[("serde", "1.0.200"), ("anyhow", "1.0.86")],
        )];
        let mut versions = members;
        versions[0]
            .by_dep_name
            .insert("zzz".to_string(), Version::new(0, 1, 0));
        // Deliberately out of order, and with 0.10 after 0.9 to prove semver (not
        // lexicographic) ordering of the version component.
        versions[0]
            .by_dep_name
            .insert("aliased".to_string(), Version::new(0, 10, 0));

        let mut older = edge("zzz", "^0.9", 0);
        older.alias = Some("aliased".to_string());
        let edges = vec![
            edge("serde", "^1.0", 0),
            older,
            edge("anyhow", "^1.0", 0),
            edge("zzz", "^0.1", 0),
        ];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &versions, &mut skipped);

        let order: Vec<(String, String)> = resolved
            .iter()
            .map(|dep| (dep.name.clone(), dep.current.to_string()))
            .collect();
        assert_eq!(
            order,
            vec![
                ("anyhow".to_string(), "1.0.86".to_string()),
                ("serde".to_string(), "1.0.200".to_string()),
                ("zzz".to_string(), "0.1.0".to_string()),
                ("zzz".to_string(), "0.10.0".to_string()),
            ]
        );
    }

    #[test]
    fn resolve_dependencies_skips_unresolved_edges_with_reasons() {
        let members = vec![member_versions("solo", &[])];
        let mut optional = edge("serde", "^1.0", 0);
        optional.optional = true;
        let mut target_specific = edge("winapi", "^0.3", 0);
        target_specific.target = Some("cfg(windows)".to_string());
        let edges = vec![optional, target_specific, edge("anyhow", "^1.0", 0)];

        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        assert!(resolved.is_empty());
        let skipped = skipped.into_vec();
        assert_eq!(skipped.len(), 3);
        assert_eq!(skipped[0].reason, SkipReason::OptionalNotActivated);
        assert_eq!(skipped[1].reason, SkipReason::TargetSpecific);
        assert_eq!(skipped[2].reason, SkipReason::MissingResolve);
    }

    #[test]
    fn skipped_collector_deduplicates_identical_reports() {
        let members = vec![
            member_versions("member-a", &[]),
            member_versions("member-b", &[]),
        ];
        let edges = vec![edge("anyhow", "^1.0", 0), edge("anyhow", "^1.0", 1)];

        let mut skipped = SkippedCollector::new();
        resolve_dependencies(edges, &members, &mut skipped);

        assert_eq!(skipped.into_vec().len(), 1);
    }

    #[test]
    fn process_dependencies_reports_members_per_version() {
        let members = vec![
            member_versions("core-lib", &[("rand", "0.8.5")]),
            member_versions("cli-app", &[("rand", "0.9.2")]),
        ];
        let edges = vec![edge("rand", "^0.8", 0), edge("rand", "^0.9", 1)];
        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        let mut latest_versions = HashMap::new();
        latest_versions.insert(
            "rand".to_string(),
            VersionInfo {
                name: "rand".to_string(),
                latest: Some("0.10.2".to_string()),
                latest_stable: None,
            },
        );

        let Processed {
            packages,
            major,
            minor,
            patch,
            compared,
        } = process_dependencies(resolved, &latest_versions, true, &mut skipped)
            .expect("process dependencies");

        assert_eq!(packages.len(), 2);
        // Two groups reached a comparison. `rand` is one crate declared twice,
        // so the edge count is also 2 here only by coincidence of the versions
        // differing; `compared` counts groups regardless.
        assert_eq!(compared, 2);
        // Both edges are 0.x moving to a new minor, which cargo treats as breaking,
        // so each counts as a major update.
        assert_eq!(major, 2);
        assert_eq!((minor, patch), (0, 0));
        assert_eq!(packages[0].current, "0.8.5");
        assert_eq!(packages[0].members, vec!["core-lib".to_string()]);
        assert_eq!(packages[1].current, "0.9.2");
        assert_eq!(packages[1].members, vec!["cli-app".to_string()]);
    }

    #[test]
    fn process_dependencies_skips_when_registry_unavailable() {
        let members = vec![member_versions("solo", &[("serde", "1.0.200")])];
        let edges = vec![edge("serde", "^1.0", 0)];
        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        let processed = process_dependencies(resolved, &HashMap::new(), false, &mut skipped)
            .expect("process dependencies");

        assert!(processed.packages.is_empty());
        // Nothing was compared, so the freshness denominator is 0 — not the
        // edge count, and not the edge count minus the skips.
        assert_eq!(processed.compared, 0);
        let skipped = skipped.into_vec();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].reason, SkipReason::RegistryUnavailable);
    }

    /// The `total > groups` case, at the level of `analyze`'s own arithmetic.
    ///
    /// One crate declared twice by one member — `[dependencies]` and
    /// `[dev-dependencies]` — is two edges but one group. Offline that is one
    /// deduplicated skip and zero comparisons, so `checked` is 0 while `total`
    /// is 2. Deriving `checked` as `total - skipped` gives 1 and claims a
    /// comparison that never happened.
    #[test]
    fn checked_counts_groups_not_edges_for_a_redeclared_crate() {
        let members = vec![member_versions("solo", &[("serde", "1.0.200")])];
        let edges = vec![edge("serde", "^1.0", 0), edge("serde", "^1.0", 0)];
        let mut skipped = SkippedCollector::new();
        let resolved = resolve_dependencies(edges, &members, &mut skipped);

        // Two edges, one group.
        assert_eq!(resolved.len(), 1);

        let processed = process_dependencies(resolved, &HashMap::new(), false, &mut skipped)
            .expect("process dependencies");
        let skipped = skipped.into_vec();

        assert_eq!(processed.compared, 0);
        assert_eq!(skipped.len(), 1);
        assert_eq!(not_applicable_count(&skipped), 0);

        let checked = processed.compared + not_applicable_count(&skipped);
        assert_eq!(checked, 0, "nothing was compared, so nothing is checked");
        // The arithmetic this replaced, spelled out: 2 edges - 1 skip = 1.
        assert_ne!(checked, 2 - skipped.len());
    }

    /// Not-applicable skips are one unit each, not one per declaring member.
    #[test]
    fn not_applicable_count_covers_only_the_non_measurable_reasons() {
        let skipped = vec![
            skipped_with(SkipReason::NonRegistry),
            skipped_with(SkipReason::TargetSpecific),
            skipped_with(SkipReason::OptionalNotActivated),
            skipped_with(SkipReason::RegistryUnavailable),
            skipped_with(SkipReason::RegistryMetadataMissing),
            skipped_with(SkipReason::MissingResolve),
        ];

        assert_eq!(not_applicable_count(&skipped), 3);
    }

    fn skipped_with(reason: SkipReason) -> SkippedDependency {
        SkippedDependency {
            name: format!("{reason:?}"),
            alias: None,
            required: "1.0".to_string(),
            reason,
            dependency_type: DependencyType::Normal,
            source: None,
            target: None,
        }
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
            checked: 2,
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
                members: vec!["cargo-upkeep".to_string()],
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
                assert_eq!(
                    value["packages"][0]["members"],
                    Value::Array(vec![Value::String("cargo-upkeep".into())])
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
