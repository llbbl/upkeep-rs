use cargo_metadata::MetadataCommand;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{print_json, DetectOutput};

pub fn run(json: bool) -> Result<()> {
    run_with_metadata(json, load_metadata())
}

fn run_with_metadata(json: bool, metadata: Result<cargo_metadata::Metadata>) -> Result<()> {
    let metadata = metadata?;
    let output = build_output(&metadata);
    emit_output(json, &output)
}

fn build_output(metadata: &cargo_metadata::Metadata) -> DetectOutput {
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
                Some(pkg.name.to_string()),
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

    DetectOutput {
        edition,
        msrv,
        workspace: is_workspace,
        members: collect_member_names(metadata),
        package,
        version,
        dependencies,
        features,
        targets,
        tooling: detect_tooling(&root),
        ci: detect_ci(&root),
    }
}

fn emit_output(json: bool, output: &DetectOutput) -> Result<()> {
    if json {
        print_json(output)
    } else {
        println!("{output}");
        Ok(())
    }
}

fn load_metadata() -> Result<cargo_metadata::Metadata> {
    MetadataCommand::new().exec().map_err(|err| {
        UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
    })
}

fn collect_member_names(metadata: &cargo_metadata::Metadata) -> Vec<String> {
    metadata
        .workspace_members
        .iter()
        .map(|id| {
            metadata
                .packages
                .iter()
                .find(|pkg| pkg.id == *id)
                .map(|pkg| pkg.name.to_string())
                .unwrap_or_else(|| id.repr.clone())
        })
        .collect()
}

fn collect_targets(targets: &[cargo_metadata::Target]) -> Vec<String> {
    let mut kinds = BTreeSet::new();
    for target in targets {
        for kind in &target.kind {
            kinds.insert(kind.to_string());
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

#[cfg(test)]
mod tests {
    use super::{run_with_metadata, *};
    use crate::core::error::{ErrorCode, UpkeepError};
    use crate::core::output::DetectOutput;
    use serde_json::Value;
    use std::fs;

    #[test]
    fn detect_tooling_reports_known_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();

        fs::write(root.join("clippy.toml"), "").expect("write clippy");
        fs::write(root.join("rustfmt.toml"), "").expect("write rustfmt");
        fs::write(root.join("deny.toml"), "").expect("write deny");
        fs::write(root.join("rust-toolchain.toml"), "").expect("write toolchain");

        let tooling = detect_tooling(root);
        assert_eq!(
            tooling,
            vec![
                "clippy".to_string(),
                "rustfmt".to_string(),
                "cargo-deny".to_string(),
                "rust-toolchain".to_string(),
            ]
        );
    }

    #[test]
    fn emit_output_json_shape() {
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
            ci: vec!["github-actions".to_string()],
        };

        let value = serde_json::to_value(&output).expect("serialize");
        assert_eq!(value["workspace"], Value::Bool(true));
        assert_eq!(value["members"][0], Value::String("core".into()));
        assert_eq!(value["tooling"][0], Value::String("clippy".into()));
    }

    #[test]
    fn run_with_metadata_propagates_error() {
        let err = run_with_metadata(
            true,
            Err(UpkeepError::message(ErrorCode::Metadata, "metadata failed")),
        )
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Metadata);
    }

    #[test]
    fn detect_ci_reports_supported_providers() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();

        let workflows = root.join(".github").join("workflows");
        fs::create_dir_all(&workflows).expect("create workflows");
        fs::write(workflows.join("ci.yml"), "").expect("write workflow");
        fs::write(root.join(".gitlab-ci.yml"), "").expect("write gitlab");
        fs::create_dir_all(root.join(".circleci")).expect("create circleci");
        fs::write(root.join(".circleci").join("config.yml"), "").expect("write circleci");

        let ci = detect_ci(root);
        assert_eq!(
            ci,
            vec![
                "github-actions".to_string(),
                "gitlab-ci".to_string(),
                "circleci".to_string(),
            ]
        );
    }

    #[test]
    fn has_workflows_only_matches_yaml_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let dir = temp.path();

        fs::write(dir.join("README.txt"), "").expect("write text");
        assert!(!has_workflows(dir));

        fs::write(dir.join("ci.yaml"), "").expect("write yaml");
        assert!(has_workflows(dir));
    }
}
