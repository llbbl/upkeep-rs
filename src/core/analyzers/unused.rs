use cargo_metadata::MetadataCommand;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

use crate::core::analyzers::external_tool::{
    handle_tool_output, is_missing_subcommand, is_unknown_flag, run_cargo_tool, ExternalToolConfig,
};
use crate::core::analyzers::util::describe_json_schema;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{Confidence, DependencyType, UnusedDep, UnusedOutput};

const MACHETE_CONFIG: ExternalToolConfig<'static> = ExternalToolConfig {
    tool_name: "machete",
    install_hint: "cargo install cargo-machete",
};

pub async fn run_unused() -> Result<UnusedOutput> {
    let metadata = MetadataCommand::new().exec().map_err(|err| {
        UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
    })?;
    let workspace_root = PathBuf::from(&metadata.workspace_root);

    let output = run_machete_json(&workspace_root).await?;
    let stdout = String::from_utf8(output.stdout).map_err(|err| {
        UpkeepError::context(
            ErrorCode::InvalidData,
            "cargo machete output was not valid UTF-8",
            err,
        )
    })?;
    let (unused, possibly_unused) = parse_machete_output(&stdout)?;

    Ok(UnusedOutput {
        unused,
        possibly_unused,
    })
}

async fn run_machete_json(workspace_root: &Path) -> Result<std::process::Output> {
    let output = run_cargo_tool(&["machete", "--json"], workspace_root, &MACHETE_CONFIG).await?;

    // If the --json flag is not recognized, fall back to plain output
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_unknown_flag(&stderr, "--json") {
            return run_machete_plain(workspace_root).await;
        }
    }

    handle_tool_output(output, &MACHETE_CONFIG, |stderr| {
        is_missing_subcommand(stderr, "machete")
    })
}

async fn run_machete_plain(workspace_root: &Path) -> Result<std::process::Output> {
    let output = run_cargo_tool(&["machete"], workspace_root, &MACHETE_CONFIG).await?;
    handle_tool_output(output, &MACHETE_CONFIG, |stderr| {
        is_missing_subcommand(stderr, "machete")
    })
}

fn parse_machete_output(stdout: &str) -> Result<(Vec<UnusedDep>, Vec<String>)> {
    if let Ok(value) = serde_json::from_str::<Value>(stdout) {
        if let Some(deps) = parse_machete_json(&value)? {
            return Ok(deps);
        }
        let schema = describe_json_schema(&value);
        return Err(UpkeepError::message(
            ErrorCode::InvalidData,
            format!("cargo machete JSON schema is not recognized: {schema}"),
        ));
    }

    Ok(parse_machete_text(stdout))
}

fn parse_machete_json(value: &Value) -> Result<Option<(Vec<UnusedDep>, Vec<String>)>> {
    if value.is_null() {
        return Ok(Some((Vec::new(), Vec::new())));
    }

    if let Some(items) = value.as_array() {
        let deps = parse_dependency_array(items, DependencyType::Normal)?;
        return Ok(Some((deps, Vec::new())));
    }

    let Some(map) = value.as_object() else {
        return Ok(None);
    };

    if let Some(manifests) = map.get("manifests").and_then(|v| v.as_array()) {
        let mut collected = Vec::new();
        let mut possibly_unused = Vec::new();
        let mut found = false;
        for manifest in manifests {
            if let Some(manifest_map) = manifest.as_object() {
                found |= collect_from_map(manifest_map, &mut collected, &mut possibly_unused)?;
            }
        }
        return Ok(found.then_some((collected, possibly_unused)));
    }

    let mut collected = Vec::new();
    let mut possibly_unused = Vec::new();
    let found = collect_from_map(map, &mut collected, &mut possibly_unused)?;
    Ok(found.then_some((collected, possibly_unused)))
}

fn collect_from_map(
    map: &serde_json::Map<String, Value>,
    collected: &mut Vec<UnusedDep>,
    possibly_unused: &mut Vec<String>,
) -> Result<bool> {
    let mut found = false;
    found |= collect_from_key(
        map,
        "unused_dependencies",
        DependencyType::Normal,
        collected,
    )?;
    found |= collect_from_key(
        map,
        "unused_dev_dependencies",
        DependencyType::Dev,
        collected,
    )?;
    found |= collect_from_key(
        map,
        "unused_build_dependencies",
        DependencyType::Build,
        collected,
    )?;

    if let Some(unused) = map.get("unused") {
        if let Some(unused_map) = unused.as_object() {
            found |= collect_from_key(
                unused_map,
                "dependencies",
                DependencyType::Normal,
                collected,
            )?;
            found |= collect_from_key(
                unused_map,
                "dev_dependencies",
                DependencyType::Dev,
                collected,
            )?;
            found |= collect_from_key(
                unused_map,
                "build_dependencies",
                DependencyType::Build,
                collected,
            )?;
        }
    }

    if let Some(possibly) = map.get("possibly_unused") {
        if let Some(items) = possibly.as_array() {
            collect_possibly_unused(items, possibly_unused);
            found = true;
        }
    }

    Ok(found)
}

fn collect_from_key(
    map: &serde_json::Map<String, Value>,
    key: &str,
    dependency_type: DependencyType,
    collected: &mut Vec<UnusedDep>,
) -> Result<bool> {
    let Some(value) = map.get(key) else {
        return Ok(false);
    };

    let deps = match value.as_array() {
        Some(items) => parse_dependency_array(items, dependency_type)?,
        None => Vec::new(),
    };

    collected.extend(deps);
    Ok(true)
}

fn parse_dependency_array(
    items: &[Value],
    fallback_type: DependencyType,
) -> Result<Vec<UnusedDep>> {
    let mut deps = Vec::new();
    for item in items {
        match item {
            Value::String(name) => deps.push(UnusedDep {
                name: name.clone(),
                dependency_type: fallback_type,
                confidence: Confidence::High,
            }),
            Value::Object(obj) => {
                let name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let dependency_type = obj
                    .get("kind")
                    .or_else(|| obj.get("dependency_type"))
                    .and_then(|v| v.as_str())
                    .and_then(parse_dependency_type)
                    .unwrap_or(fallback_type);
                // Extract confidence from JSON if present, otherwise use high (JSON default)
                let confidence = obj
                    .get("confidence")
                    .and_then(|v| v.as_str())
                    .and_then(parse_confidence)
                    .unwrap_or(Confidence::High);
                deps.push(UnusedDep {
                    name,
                    dependency_type,
                    confidence,
                });
            }
            _ => {}
        }
    }
    Ok(deps)
}

fn parse_confidence(value: &str) -> Option<Confidence> {
    match value.to_lowercase().as_str() {
        "high" => Some(Confidence::High),
        "medium" => Some(Confidence::Medium),
        "low" => Some(Confidence::Low),
        _ => None,
    }
}

fn parse_dependency_type(kind: &str) -> Option<DependencyType> {
    match kind {
        "normal" | "dependencies" => Some(DependencyType::Normal),
        "dev" | "dev-dependencies" | "dev_dependencies" => Some(DependencyType::Dev),
        "build" | "build-dependencies" | "build_dependencies" => Some(DependencyType::Build),
        _ => None,
    }
}

fn parse_machete_text(stdout: &str) -> (Vec<UnusedDep>, Vec<String>) {
    let mut dependencies = Vec::new();
    let mut possibly_unused = Vec::new();
    let mut current_type: Option<DependencyType> = None;
    let mut in_possibly_unused = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let lower = trimmed.to_lowercase();
        if lower.contains("possibly unused") {
            in_possibly_unused = true;
            current_type = None;
            continue;
        }
        if lower.contains("unused dependencies") {
            current_type = Some(DependencyType::Normal);
            in_possibly_unused = false;
            continue;
        }
        if lower.contains("unused dev-dependencies") || lower.contains("unused dev dependencies") {
            current_type = Some(DependencyType::Dev);
            in_possibly_unused = false;
            continue;
        }
        if lower.contains("unused build-dependencies")
            || lower.contains("unused build dependencies")
        {
            current_type = Some(DependencyType::Build);
            in_possibly_unused = false;
            continue;
        }

        let name = trimmed
            .trim_start_matches('-')
            .trim_start_matches('*')
            .trim()
            .trim_end_matches(':')
            .to_string();
        if name.is_empty() {
            continue;
        }

        if in_possibly_unused {
            possibly_unused.push(name);
            continue;
        }

        let Some(dependency_type) = current_type else {
            continue;
        };

        // Text output parsing is less reliable than JSON, so we use medium confidence
        dependencies.push(UnusedDep {
            name,
            dependency_type,
            confidence: Confidence::Medium,
        });
    }

    // Log if text parsing found no results (helps debug parsing issues)
    if dependencies.is_empty() && possibly_unused.is_empty() && !stdout.trim().is_empty() {
        tracing::debug!(
            "parse_machete_text found no results from non-empty output ({} bytes)",
            stdout.len()
        );
    }

    (dependencies, possibly_unused)
}

fn collect_possibly_unused(items: &[Value], possibly_unused: &mut Vec<String>) {
    for item in items {
        match item {
            Value::String(name) => possibly_unused.push(name.clone()),
            Value::Object(obj) => {
                if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                    possibly_unused.push(name.to_string());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    async fn run_machete_json_with<F, Fut>(
        workspace_root: &Path,
        run_tool: F,
    ) -> Result<std::process::Output>
    where
        F: Fn(&[&str], &Path, &ExternalToolConfig<'_>) -> Fut,
        Fut: Future<Output = Result<std::process::Output>>,
    {
        let output = run_tool(&["machete", "--json"], workspace_root, &MACHETE_CONFIG).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if is_unknown_flag(&stderr, "--json") {
                let output = run_tool(&["machete"], workspace_root, &MACHETE_CONFIG).await?;
                return handle_tool_output(output, &MACHETE_CONFIG, |stderr| {
                    is_missing_subcommand(stderr, "machete")
                });
            }
        }

        handle_tool_output(output, &MACHETE_CONFIG, |stderr| {
            is_missing_subcommand(stderr, "machete")
        })
    }

    #[test]
    fn parse_machete_output_handles_null() {
        let (unused, possibly_unused) = parse_machete_output("null").expect("parse");
        assert!(unused.is_empty());
        assert!(possibly_unused.is_empty());
    }

    #[test]
    fn parse_machete_output_handles_array_schema() {
        let json = r#"["serde", {"name":"tokio","kind":"dev","confidence":"low"}]"#;
        let (unused, possibly_unused) = parse_machete_output(json).expect("parse");
        assert!(possibly_unused.is_empty());
        assert_eq!(unused.len(), 2);
        assert_eq!(unused[0].name, "serde");
        assert_eq!(unused[0].dependency_type, DependencyType::Normal);
        assert_eq!(unused[0].confidence, Confidence::High);
        assert_eq!(unused[1].name, "tokio");
        assert_eq!(unused[1].dependency_type, DependencyType::Dev);
        assert_eq!(unused[1].confidence, Confidence::Low);
    }

    #[test]
    fn parse_machete_output_handles_manifest_schema() {
        let json = r#"{"manifests":[{"unused_dependencies":["serde"],"unused_dev_dependencies":[{"name":"tokio","kind":"dev","confidence":"medium"}],"unused_build_dependencies":[{"name":"cc","kind":"build"}],"possibly_unused":["rand"]}]}"#;
        let (unused, possibly_unused) = parse_machete_output(json).expect("parse");
        assert_eq!(possibly_unused, vec!["rand".to_string()]);
        assert_eq!(unused.len(), 3);
        assert_eq!(unused[0].name, "serde");
        assert_eq!(unused[0].dependency_type, DependencyType::Normal);
        assert_eq!(unused[1].name, "tokio");
        assert_eq!(unused[1].dependency_type, DependencyType::Dev);
        assert_eq!(unused[1].confidence, Confidence::Medium);
        assert_eq!(unused[2].name, "cc");
        assert_eq!(unused[2].dependency_type, DependencyType::Build);
    }

    #[test]
    fn parse_machete_output_handles_unused_section_schema() {
        let json = r#"{"unused":{"dependencies":["serde"],"dev_dependencies":[{"name":"tokio"}],"build_dependencies":[{"name":"cc"}]}}"#;
        let (unused, possibly_unused) = parse_machete_output(json).expect("parse");
        assert!(possibly_unused.is_empty());
        assert_eq!(unused.len(), 3);
        assert_eq!(unused[0].name, "serde");
        assert_eq!(unused[0].dependency_type, DependencyType::Normal);
        assert_eq!(unused[1].name, "tokio");
        assert_eq!(unused[1].dependency_type, DependencyType::Dev);
        assert_eq!(unused[2].name, "cc");
        assert_eq!(unused[2].dependency_type, DependencyType::Build);
    }

    #[test]
    fn parse_machete_output_rejects_unknown_schema() {
        let err = parse_machete_output("{\"unexpected\":true}").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidData);
        assert!(err
            .to_string()
            .contains("cargo machete JSON schema is not recognized"));
    }

    #[test]
    fn parse_machete_text_parses_sections() {
        let text = "Unused dependencies:\n- serde\nUnused dev-dependencies:\n* tokio\nUnused build-dependencies:\ncc\nPossibly unused dependencies:\n- rand\n";
        let (unused, possibly_unused) = parse_machete_text(text);
        assert_eq!(unused.len(), 3);
        assert_eq!(unused[0].name, "serde");
        assert_eq!(unused[0].dependency_type, DependencyType::Normal);
        assert_eq!(unused[0].confidence, Confidence::Medium);
        assert_eq!(unused[1].name, "tokio");
        assert_eq!(unused[1].dependency_type, DependencyType::Dev);
        assert_eq!(unused[2].name, "cc");
        assert_eq!(unused[2].dependency_type, DependencyType::Build);
        assert_eq!(possibly_unused, vec!["rand".to_string()]);
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code as u32)
    }

    fn output_with(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn run_machete_json_with_falls_back_on_unknown_flag() {
        let outputs = Arc::new(Mutex::new(vec![
            output_with(1, "", "error: Found argument '--json' which wasn't expected"),
            output_with(
                0,
                "Unused dependencies:\n- serde\nPossibly unused dependencies:\n- rand\n",
                "",
            ),
        ]));
        let calls = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));

        let run_tool = |args: &[&str], _: &Path, _: &ExternalToolConfig<'_>| {
            let outputs = Arc::clone(&outputs);
            let calls = Arc::clone(&calls);
            let args_vec = args.iter().map(|arg| (*arg).to_string()).collect();
            async move {
                calls.lock().unwrap().push(args_vec);
                Ok(outputs.lock().unwrap().remove(0))
            }
        };

        let output = run_machete_json_with(Path::new("."), &run_tool)
            .await
            .expect("output");
        let stdout = String::from_utf8(output.stdout).expect("stdout");
        let (unused, possibly_unused) = parse_machete_output(&stdout).expect("parse");

        assert_eq!(unused.len(), 1);
        assert_eq!(unused[0].name, "serde");
        assert_eq!(unused[0].confidence, Confidence::Medium);
        assert_eq!(possibly_unused, vec!["rand".to_string()]);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[
                vec!["machete".to_string(), "--json".to_string()],
                vec!["machete".to_string()]
            ]
        );
    }

    #[tokio::test]
    async fn run_machete_json_with_missing_tool_returns_error() {
        let outputs = Arc::new(Mutex::new(vec![output_with(
            1,
            "",
            "error: no such subcommand: `machete`",
        )]));

        let run_tool = |args: &[&str], _: &Path, _: &ExternalToolConfig<'_>| {
            let outputs = Arc::clone(&outputs);
            let args_vec: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
            async move {
                assert_eq!(args_vec, vec!["machete".to_string(), "--json".to_string()]);
                Ok(outputs.lock().unwrap().remove(0))
            }
        };

        let err = run_machete_json_with(Path::new("."), &run_tool)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::MissingTool);
        assert!(err.to_string().contains("cargo-machete is not installed"));
    }
}
