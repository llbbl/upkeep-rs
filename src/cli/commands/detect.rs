use crate::core::error::{ErrorCode, Result, UpkeepError};
use cargo_metadata::MetadataCommand;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::core::output::{print_json, DetectOutput};

pub async fn run(json: bool) -> Result<()> {
    let metadata = MetadataCommand::new().exec().map_err(|err| {
        UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
    })?;
    let root = PathBuf::from(&metadata.workspace_root);
    let root_package = metadata.root_package();

    let (package, version, edition, msrv, dependencies, features, targets) =
        if let Some(pkg) = root_package {
            let edition = Some(pkg.edition.to_string());
            let msrv = pkg.rust_version.as_ref().map(|ver| ver.to_string());
            let dependencies = pkg.dependencies.len();
            let features = pkg.features.keys().cloned().collect();
            let targets = collect_targets(&pkg.targets);

            (
                Some(pkg.name.clone()),
                Some(pkg.version.to_string()),
                edition,
                msrv,
                dependencies,
                features,
                targets,
            )
        } else {
            (None, None, None, None, 0, Vec::new(), Vec::new())
        };

    // A workspace exists if:
    // 1. There's no root package (virtual workspace), OR
    // 2. There are multiple members
    let is_workspace = root_package.is_none() || metadata.workspace_members.len() > 1;

    let output = DetectOutput {
        edition,
        msrv,
        workspace: is_workspace,
        members: metadata
            .workspace_members
            .iter()
            .map(|id| id.repr.clone())
            .collect(),
        package,
        version,
        dependencies,
        features,
        targets,
        tooling: detect_tooling(&root),
        ci: detect_ci(&root),
    };

    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
}

fn collect_targets(targets: &[cargo_metadata::Target]) -> Vec<String> {
    let mut kinds = BTreeSet::new();
    for target in targets {
        for kind in &target.kind {
            kinds.insert(kind.clone());
        }
    }
    kinds.into_iter().collect()
}

fn detect_tooling(root: &Path) -> Vec<String> {
    let mut tooling = Vec::new();

    if has_any(root, &["clippy.toml", ".clippy.toml"]) {
        tooling.push("clippy".to_string());
    }
    if has_any(root, &["rustfmt.toml", ".rustfmt.toml"]) {
        tooling.push("rustfmt".to_string());
    }
    if root.join("deny.toml").is_file() {
        tooling.push("cargo-deny".to_string());
    }
    if root.join("rust-toolchain.toml").is_file() {
        tooling.push("rust-toolchain".to_string());
    }

    tooling
}

fn detect_ci(root: &Path) -> Vec<String> {
    let mut ci = Vec::new();

    let github_workflows = root.join(".github").join("workflows");
    if has_workflows(&github_workflows) {
        ci.push("github-actions".to_string());
    }
    if root.join(".gitlab-ci.yml").is_file() {
        ci.push("gitlab-ci".to_string());
    }
    if root.join(".circleci").join("config.yml").is_file() {
        ci.push("circleci".to_string());
    }

    ci
}

fn has_any(root: &Path, files: &[&str]) -> bool {
    files.iter().any(|name| root.join(name).is_file())
}

fn has_workflows(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }

    match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().any(|entry| {
            let path = entry.path();
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml") | Some("yaml")
            )
        }),
        Err(_) => false,
    }
}
