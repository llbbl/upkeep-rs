use cargo_metadata::MetadataCommand;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

use crate::core::analyzers::external_tool::{
    handle_tool_output, is_missing_subcommand, is_unknown_flag, run_cargo_tool, ExternalToolConfig,
};
use crate::core::analyzers::util::describe_json_schema;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{UnsafeOutput, UnsafePackage, UnsafeSummary};

const GEIGER_CONFIG: ExternalToolConfig<'static> = ExternalToolConfig {
    tool_name: "geiger",
    install_hint: "cargo install cargo-geiger",
};

pub async fn run_unsafe() -> Result<UnsafeOutput> {
    let metadata = MetadataCommand::new().exec().map_err(|err| {
        UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
    })?;
    let workspace_root = PathBuf::from(&metadata.workspace_root);

    let output = run_geiger_json(&workspace_root).await?;
    let stdout = String::from_utf8(output.stdout).map_err(|err| {
        UpkeepError::context(
            ErrorCode::InvalidData,
            "cargo geiger output was not valid UTF-8",
            err,
        )
    })?;
    match parse_geiger_output(&stdout) {
        Ok(output) => Ok(output),
        Err(parse_err) => {
            if output.status.success() {
                return Err(parse_err);
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr_message = stderr.trim();
            let stdout_message = stdout.trim();
            let stderr_message = if stderr_message.is_empty() {
                "<empty>"
            } else {
                stderr_message
            };

            let mut message =
                format!("cargo geiger failed: stderr: {stderr_message}; parse error: {parse_err}");
            if !stdout_message.is_empty() {
                message.push_str(" stdout: ");
                message.push_str(stdout_message);
            }

            Err(UpkeepError::message(ErrorCode::ExternalCommand, message))
        }
    }
}

async fn run_geiger_json(workspace_root: &Path) -> Result<std::process::Output> {
    let output = run_cargo_tool(
        &["geiger", "--output-format", "json"],
        workspace_root,
        &GEIGER_CONFIG,
    )
    .await?;

    // If the output flag is not recognized, try the alternative flag
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_unknown_flag(&stderr, "--output-format") {
            return run_geiger_json_alt(workspace_root).await;
        }
    }

    handle_tool_output(output, &GEIGER_CONFIG, |stderr| {
        is_missing_subcommand(stderr, "geiger")
    })
}

async fn run_geiger_json_alt(workspace_root: &Path) -> Result<std::process::Output> {
    let output = run_cargo_tool(
        &["geiger", "--format", "json"],
        workspace_root,
        &GEIGER_CONFIG,
    )
    .await?;
    handle_tool_output(output, &GEIGER_CONFIG, |stderr| {
        is_missing_subcommand(stderr, "geiger")
    })
}

fn parse_geiger_output(stdout: &str) -> Result<UnsafeOutput> {
    let value: Value = serde_json::from_str(stdout).map_err(|err| {
        UpkeepError::context(
            ErrorCode::InvalidData,
            "cargo geiger output was not valid JSON",
            err,
        )
    })?;
    let packages = parse_geiger_packages(&value)?;
    let summary = summarize(&packages);

    Ok(UnsafeOutput { summary, packages })
}

fn parse_geiger_packages(value: &Value) -> Result<Vec<UnsafePackage>> {
    let packages = if let Some(items) = value.get("packages").and_then(|v| v.as_array()) {
        items
    } else if let Some(items) = value.get("crate_stats").and_then(|v| v.as_array()) {
        items
    } else if let Some(items) = value.as_array() {
        items
    } else {
        let schema = describe_json_schema(value);
        return Err(UpkeepError::message(
            ErrorCode::InvalidData,
            format!("cargo geiger JSON schema is not recognized: {schema}"),
        ));
    };

    let mut output = Vec::new();
    let mut missing_identifiers = 0usize;
    for item in packages {
        if let Some(package) = parse_package(item) {
            output.push(package);
        } else if !has_package_identifier(item) {
            missing_identifiers += 1;
        }
    }

    if output.is_empty() && !packages.is_empty() && missing_identifiers == packages.len() {
        return Err(UpkeepError::message(
            ErrorCode::InvalidData,
            "cargo geiger JSON missing package identifiers; schema may have changed",
        ));
    }

    Ok(output)
}

fn parse_package(value: &Value) -> Option<UnsafePackage> {
    let package_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| id.to_string());
    let (id_name, id_version) = package_id
        .as_deref()
        .map(parse_name_version_from_id)
        .unwrap_or((None, None));

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("crate").and_then(|v| v.as_str()))
        .or_else(|| {
            value
                .get("package")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
        })
        .or(id_name.as_deref())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return None;
    }

    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("package")
                .and_then(|v| v.get("version"))
                .and_then(|v| v.as_str())
        })
        .or(id_version.as_deref())
        .unwrap_or("unknown")
        .to_string();

    let stats = value
        .get("stats")
        .or_else(|| value.get("geiger"))
        .or_else(|| value.get("counts"));

    let unsafe_functions = extract_unsafe_for(stats, value, &["functions", "fn"]);
    let unsafe_impls = extract_unsafe_for(stats, value, &["impls", "impl"]);
    let unsafe_traits = extract_unsafe_for(stats, value, &["traits", "trait"]);
    let unsafe_blocks = extract_unsafe_for(stats, value, &["blocks", "block"]);
    let unsafe_expressions = extract_unsafe_for(stats, value, &["exprs", "expressions", "expr"]);
    let total_unsafe =
        unsafe_functions + unsafe_impls + unsafe_traits + unsafe_blocks + unsafe_expressions;

    Some(UnsafePackage {
        name,
        version,
        package_id,
        unsafe_functions,
        unsafe_impls,
        unsafe_traits,
        unsafe_blocks,
        unsafe_expressions,
        total_unsafe,
    })
}

fn has_package_identifier(value: &Value) -> bool {
    value.get("id").and_then(|v| v.as_str()).is_some()
        || value.get("name").and_then(|v| v.as_str()).is_some()
        || value.get("crate").and_then(|v| v.as_str()).is_some()
        || value
            .get("package")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .is_some()
}

fn parse_name_version_from_id(id: &str) -> (Option<String>, Option<String>) {
    let mut parts = id.split_whitespace();
    let name = parts.next().map(str::to_string);
    let version = parts.next().map(str::to_string);
    (name, version)
}

fn extract_unsafe_for(stats: Option<&Value>, fallback: &Value, keys: &[&str]) -> usize {
    if let Some(stats) = stats {
        if let Some(count) = extract_unsafe_from_stats(stats, keys) {
            return count;
        }
    }

    extract_unsafe_from_stats(fallback, keys).unwrap_or(0)
}

fn extract_unsafe_from_stats(stats: &Value, keys: &[&str]) -> Option<usize> {
    for key in keys {
        if let Some(value) = stats.get(*key) {
            if let Some(count) = extract_unsafe_count(value) {
                return Some(count);
            }
        }
    }

    if let Some(unsafe_map) = stats.get("unsafe") {
        for key in keys {
            if let Some(value) = unsafe_map.get(*key) {
                if let Some(count) = extract_unsafe_count(value) {
                    return Some(count);
                }
            }
        }
    }

    None
}

fn extract_unsafe_count(value: &Value) -> Option<usize> {
    if let Some(count) = value.as_u64() {
        // Use try_into to safely handle potential overflow on 32-bit platforms
        return count.try_into().ok();
    }

    let map = value.as_object()?;
    for key in ["unsafe", "unsafe_count", "unsafe_total", "count_unsafe"] {
        if let Some(count) = map.get(key).and_then(|v| v.as_u64()) {
            // Use try_into to safely handle potential overflow on 32-bit platforms
            return count.try_into().ok();
        }
    }

    None
}

fn summarize(packages: &[UnsafePackage]) -> UnsafeSummary {
    let mut summary = UnsafeSummary {
        packages: packages.len(),
        unsafe_functions: 0,
        unsafe_impls: 0,
        unsafe_traits: 0,
        unsafe_blocks: 0,
        unsafe_expressions: 0,
        total_unsafe: 0,
    };

    for package in packages {
        summary.unsafe_functions = summary
            .unsafe_functions
            .saturating_add(package.unsafe_functions);
        summary.unsafe_impls = summary.unsafe_impls.saturating_add(package.unsafe_impls);
        summary.unsafe_traits = summary.unsafe_traits.saturating_add(package.unsafe_traits);
        summary.unsafe_blocks = summary.unsafe_blocks.saturating_add(package.unsafe_blocks);
        summary.unsafe_expressions = summary
            .unsafe_expressions
            .saturating_add(package.unsafe_expressions);
        summary.total_unsafe = summary.total_unsafe.saturating_add(package.total_unsafe);
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_geiger_output_supports_packages_schema() {
        let json = r#"{"packages":[{"name":"foo","version":"1.0.0","stats":{"functions":{"unsafe":1},"impls":{"unsafe":2},"traits":{"unsafe":0},"blocks":{"unsafe":3},"exprs":{"unsafe":4}}}]}"#;
        let output = parse_geiger_output(json).expect("parse");
        assert_eq!(output.summary.total_unsafe, 10);
        assert_eq!(output.packages[0].name, "foo");
    }

    #[test]
    fn parse_geiger_output_supports_crate_stats_schema() {
        let json = r#"{"crate_stats":[{"id":"bar 0.2.0 (path+file://...)","geiger":{"impls":{"unsafe":1},"blocks":{"unsafe":2}}}]}"#;
        let output = parse_geiger_output(json).expect("parse");
        assert_eq!(output.packages[0].name, "bar");
        assert_eq!(output.packages[0].version, "0.2.0");
        assert_eq!(output.summary.total_unsafe, 3);
    }

    #[test]
    fn parse_geiger_output_supports_top_level_array_schema() {
        let json = r#"[{"package":{"name":"baz","version":"0.3.0"},"counts":{"unsafe":{"functions":2,"impls":1,"traits":0,"blocks":0,"expressions":1}}}]"#;
        let output = parse_geiger_output(json).expect("parse");
        assert_eq!(output.packages[0].name, "baz");
        assert_eq!(output.packages[0].version, "0.3.0");
        assert_eq!(output.summary.total_unsafe, 4);
    }

    #[test]
    fn parse_geiger_output_rejects_missing_identifiers() {
        let json = r#"{"packages":[{"stats":{}}]}"#;
        let err = parse_geiger_output(json).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidData);
        assert!(err.to_string().contains("cargo geiger"));
    }
}
