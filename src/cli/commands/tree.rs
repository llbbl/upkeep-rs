use anyhow::{bail, Context, Result};
use cargo_metadata::{CargoOpt, DependencyKind, MetadataCommand, PackageId};
use std::collections::{HashMap, HashSet};

use crate::cli::TreeArgs;
use crate::core::output::{print_json, TreeNode, TreeOutput, TreeStats};

#[derive(Clone)]
struct Edge {
    id: PackageId,
    is_dev: bool,
    is_build: bool,
}

pub async fn run(json: bool, args: TreeArgs) -> Result<()> {
    let metadata = MetadataCommand::new()
        .features(CargoOpt::AllFeatures)
        .exec()
        .context("failed to load cargo metadata")?;
    let resolve = metadata
        .resolve
        .as_ref()
        .context("metadata missing resolve data")?;

    let mut packages_by_id = HashMap::new();
    let mut versions_by_name: HashMap<String, HashSet<String>> = HashMap::new();
    for package in &metadata.packages {
        versions_by_name
            .entry(package.name.clone())
            .or_default()
            .insert(package.version.to_string());
        packages_by_id.insert(package.id.clone(), package.clone());
    }

    let duplicate_names: HashSet<String> = versions_by_name
        .into_iter()
        .filter_map(|(name, versions)| if versions.len() > 1 { Some(name) } else { None })
        .collect();

    let mut features_by_id = HashMap::new();
    let mut forward_graph: HashMap<PackageId, Vec<Edge>> = HashMap::new();

    for node in &resolve.nodes {
        features_by_id.insert(node.id.clone(), node.features.clone());
        for dep in &node.deps {
            if args.no_dev && !includes_non_dev(dep) {
                continue;
            }

            let is_dev = dep
                .dep_kinds
                .iter()
                .any(|kind| kind.kind == DependencyKind::Development);
            let is_build = dep
                .dep_kinds
                .iter()
                .any(|kind| kind.kind == DependencyKind::Build);

            forward_graph
                .entry(node.id.clone())
                .or_default()
                .push(Edge {
                    id: dep.pkg.clone(),
                    is_dev,
                    is_build,
                });
        }
    }

    let invert_target = args.invert.as_deref();
    let graph = if invert_target.is_some() {
        invert_graph(&forward_graph)
    } else {
        forward_graph
    };

    let mut expanded = HashSet::new();
    let mut path = HashSet::new();

    let root = match invert_target {
        Some(name) => build_inverted_root(
            name,
            &graph,
            &packages_by_id,
            &features_by_id,
            &args,
            &duplicate_names,
            &mut expanded,
            &mut path,
        )?,
        None => {
            let root_package = metadata
                .root_package()
                .context("no root package found (virtual workspaces are not supported yet)")?;
            build_node(
                &root_package.id,
                0,
                false,
                false,
                &graph,
                &packages_by_id,
                &features_by_id,
                &args,
                &duplicate_names,
                &mut expanded,
                &mut path,
            )?
            .unwrap_or_else(|| TreeNode::empty("root"))
        }
    };

    let stats = build_stats(&root);
    let output = TreeOutput { root, stats };

    if json {
        print_json(&output)
    } else {
        let rendered = render_tree(&output.root, args.features);
        println!("{rendered}");
        println!(
            "\nCrates: {}  Direct deps: {}  Transitive deps: {}  Duplicate crates: {}",
            output.stats.total_crates,
            output.stats.direct_deps,
            output.stats.transitive_deps,
            output.stats.duplicate_crates
        );
        Ok(())
    }
}

fn includes_non_dev(dep: &cargo_metadata::NodeDep) -> bool {
    dep.dep_kinds.iter().any(|kind| match kind.kind {
        DependencyKind::Development => false,
        DependencyKind::Normal | DependencyKind::Build | DependencyKind::Unknown => true,
    })
}

fn invert_graph(graph: &HashMap<PackageId, Vec<Edge>>) -> HashMap<PackageId, Vec<Edge>> {
    let mut inverted: HashMap<PackageId, Vec<Edge>> = HashMap::new();
    for (from, children) in graph {
        for child in children {
            inverted
                .entry(child.id.clone())
                .or_default()
                .push(Edge {
                    id: from.clone(),
                    is_dev: child.is_dev,
                    is_build: child.is_build,
                });
        }
    }
    inverted
}

#[allow(clippy::too_many_arguments)]
fn build_inverted_root(
    name: &str,
    graph: &HashMap<PackageId, Vec<Edge>>,
    packages_by_id: &HashMap<PackageId, cargo_metadata::Package>,
    features_by_id: &HashMap<PackageId, Vec<String>>,
    args: &TreeArgs,
    duplicate_names: &HashSet<String>,
    expanded: &mut HashSet<PackageId>,
    path: &mut HashSet<PackageId>,
) -> Result<TreeNode> {
    let mut matches = Vec::new();
    for (id, package) in packages_by_id {
        if package.name == name {
            matches.push(id.clone());
        }
    }

    if matches.is_empty() {
        bail!("no package named '{name}' found in metadata");
    }

    if matches.len() == 1 {
        return build_node(
            &matches[0],
            0,
            false,
            false,
            graph,
            packages_by_id,
            features_by_id,
            args,
            duplicate_names,
            expanded,
            path,
        )?
        .ok_or_else(|| anyhow::anyhow!("no dependencies to display for {name}"));
    }

    let mut root = TreeNode::empty(&format!("reverse:{name}"));
    for id in matches {
        if let Some(child) = build_node(
            &id,
            1,
            false,
            false,
            graph,
            packages_by_id,
            features_by_id,
            args,
            duplicate_names,
            expanded,
            path,
        )? {
            root.dependencies.push(child);
        }
    }

    Ok(root)
}

#[allow(clippy::too_many_arguments)]
fn build_node(
    id: &PackageId,
    depth: usize,
    is_dev: bool,
    is_build: bool,
    graph: &HashMap<PackageId, Vec<Edge>>,
    packages_by_id: &HashMap<PackageId, cargo_metadata::Package>,
    features_by_id: &HashMap<PackageId, Vec<String>>,
    args: &TreeArgs,
    duplicate_names: &HashSet<String>,
    expanded: &mut HashSet<PackageId>,
    path: &mut HashSet<PackageId>,
) -> Result<Option<TreeNode>> {
    let package = packages_by_id
        .get(id)
        .with_context(|| format!("package {id} missing from metadata"))?;

    let duplicate = duplicate_names.contains(&package.name);
    let mut node = TreeNode {
        name: package.name.clone(),
        version: package.version.to_string(),
        package_id: package.id.to_string(),
        features: if args.features {
            features_by_id.get(id).cloned().unwrap_or_default()
        } else {
            Vec::new()
        },
        dependencies: Vec::new(),
        is_dev,
        is_build,
        duplicate,
    };

    if args.depth.map(|limit| depth >= limit).unwrap_or(false) {
        return Ok(if args.duplicates && !duplicate && depth > 0 {
            None
        } else {
            Some(node)
        });
    }

    if !expanded.insert(id.clone()) {
        return Ok(if args.duplicates && !duplicate && depth > 0 {
            None
        } else {
            Some(node)
        });
    }

    // Track current path for cycle detection (checked in child loop below)
    path.insert(id.clone());

    let mut children = graph.get(id).cloned().unwrap_or_default();
    children.sort_by(|left, right| {
        let left_name = packages_by_id
            .get(&left.id)
            .map(|pkg| pkg.name.as_str())
            .unwrap_or("~");
        let right_name = packages_by_id
            .get(&right.id)
            .map(|pkg| pkg.name.as_str())
            .unwrap_or("~");
        left_name.cmp(right_name)
    });

    for child in children {
        if path.contains(&child.id) {
            continue;
        }
        if let Some(child_node) = build_node(
            &child.id,
            depth + 1,
            child.is_dev,
            child.is_build,
            graph,
            packages_by_id,
            features_by_id,
            args,
            duplicate_names,
            expanded,
            path,
        )? {
            node.dependencies.push(child_node);
        }
    }

    path.remove(id);

    if args.duplicates && !duplicate && depth > 0 && node.dependencies.is_empty() {
        return Ok(None);
    }

    Ok(Some(node))
}

fn build_stats(root: &TreeNode) -> TreeStats {
    let mut ids = HashSet::new();
    collect_ids(root, &mut ids);

    let mut versions_by_name: HashMap<String, HashSet<String>> = HashMap::new();
    collect_versions(root, &mut versions_by_name);
    let duplicate_crates = versions_by_name
        .values()
        .filter(|versions| versions.len() > 1)
        .count();

    let root_is_real = !root.package_id.is_empty();
    let direct_deps = root.dependencies.len();
    let total_crates = ids.len();
    let transitive_deps = total_crates.saturating_sub(direct_deps + usize::from(root_is_real));

    TreeStats {
        total_crates,
        direct_deps,
        transitive_deps,
        duplicate_crates,
    }
}

fn collect_ids(node: &TreeNode, ids: &mut HashSet<String>) {
    if !node.package_id.is_empty() {
        ids.insert(node.package_id.clone());
    }
    for child in &node.dependencies {
        collect_ids(child, ids);
    }
}

fn collect_versions(node: &TreeNode, versions_by_name: &mut HashMap<String, HashSet<String>>) {
    if !node.version.is_empty() {
        versions_by_name
            .entry(node.name.clone())
            .or_default()
            .insert(node.version.clone());
    }
    for child in &node.dependencies {
        collect_versions(child, versions_by_name);
    }
}

fn render_tree(root: &TreeNode, show_features: bool) -> String {
    let mut lines = Vec::new();
    lines.push(format_label(root, show_features));

    let children_len = root.dependencies.len();
    for (index, child) in root.dependencies.iter().enumerate() {
        let is_last = index + 1 == children_len;
        render_node(child, "", is_last, show_features, &mut lines);
    }

    lines.join("\n")
}

fn render_node(
    node: &TreeNode,
    prefix: &str,
    is_last: bool,
    show_features: bool,
    lines: &mut Vec<String>,
) {
    let connector = if is_last { "`-- " } else { "|-- " };
    lines.push(format!("{prefix}{connector}{}", format_label(node, show_features)));

    let next_prefix = if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}|   ")
    };

    let children_len = node.dependencies.len();
    for (index, child) in node.dependencies.iter().enumerate() {
        let child_last = index + 1 == children_len;
        render_node(child, &next_prefix, child_last, show_features, lines);
    }
}

fn format_label(node: &TreeNode, show_features: bool) -> String {
    let mut annotations: Vec<String> = Vec::new();
    if node.duplicate {
        annotations.push("dup".to_string());
    }
    if node.is_dev {
        annotations.push("dev".to_string());
    }
    if node.is_build {
        annotations.push("build".to_string());
    }

    if show_features && !node.features.is_empty() {
        annotations.push(format!("features: {}", node.features.join(", ")));
    }

    let version_suffix = if node.version.is_empty() {
        String::new()
    } else {
        format!(" v{}", node.version)
    };

    if annotations.is_empty() {
        format!("{}{}", node.name, version_suffix)
    } else {
        format!("{}{} [{}]", node.name, version_suffix, annotations.join(", "))
    }
}

impl TreeNode {
    fn empty(name: &str) -> Self {
        Self {
            name: name.to_string(),
            version: String::new(),
            package_id: String::new(),
            features: Vec::new(),
            dependencies: Vec::new(),
            is_dev: false,
            is_build: false,
            duplicate: false,
        }
    }
}
