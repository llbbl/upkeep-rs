use cargo_metadata::MetadataCommand;
use serde_json::Value;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use tokio::process::Command;

use crate::core::analyzers::util::describe_json_schema;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{UnsafeOutput, UnsafePackage, UnsafeSummary};

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
        Err(err) => {
            if output.status.success() {
                return Err(err);
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr_message = stderr.trim();
            let stdout_message = stdout.trim();
            let stderr_message = if stderr_message.is_empty() {
                "<empty>"
            } else {
                stderr_message
            };

            let mut message = format!("cargo geiger failed: stderr: {stderr_message}");
            if !stdout_message.is_empty() {
                message.push_str(" stdout: ");
                message.push_str(stdout_message);
            }

            Err(UpkeepError::message(ErrorCode::ExternalCommand, message))
        }
    }
}

async fn run_geiger_json(workspace_root: &Path) -> Result<std::process::Output> {
    let output = run_geiger(&["geiger", "--output-format", "json"], workspace_root).await?;

    // If the output flag is not recognized, try the alternative flag
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_unknown_output_flag(&stderr) {
            return run_geiger_json_alt(workspace_root).await;
        }
    }

    handle_geiger_output(output)
}

async fn run_geiger_json_alt(workspace_root: &Path) -> Result<std::process::Output> {
    let output = run_geiger(&["geiger", "--format", "json"], workspace_root).await?;
    handle_geiger_output(output)
}

/// Shared error handling for geiger command output.
///
/// Returns `Ok(output)` if the command succeeded or produced stdout.
/// Returns an appropriate error otherwise.
fn handle_geiger_output(output: std::process::Output) -> Result<std::process::Output> {
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_missing_geiger(&stderr) {
        return missing_geiger_error();
    }

    // Some geiger versions exit non-zero but still produce valid output
    if !output.stdout.is_empty() {
        return Ok(output);
    }

    let message = stderr.trim();
    if message.is_empty() {
        return Err(UpkeepError::message(
            ErrorCode::ExternalCommand,
            "cargo geiger failed with no stderr output",
        ));
    }
    Err(UpkeepError::message(
        ErrorCode::ExternalCommand,
        format!("cargo geiger failed: {message}"),
    ))
}

async fn run_geiger(args: &[&str], workspace_root: &Path) -> Result<std::process::Output> {
    Command::new("cargo")
        .args(args)
        .current_dir(workspace_root)
        .output()
        .await
        .map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => UpkeepError::message(
                ErrorCode::MissingTool,
                "cargo is not installed or not on PATH",
            ),
            _ => UpkeepError::context(
                ErrorCode::ExternalCommand,
                "failed to execute cargo geiger",
                err,
            ),
        })
}

fn missing_geiger_error() -> Result<std::process::Output> {
    Err(UpkeepError::message(
        ErrorCode::MissingTool,
        "cargo-geiger is not installed. Install it with `cargo install cargo-geiger`.",
    ))
}

fn is_missing_geiger(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    (lower.contains("no such subcommand") || lower.contains("unknown subcommand"))
        && lower.contains("geiger")
}

fn is_unknown_output_flag(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    (lower.contains("unexpected argument") || lower.contains("found argument"))
        && lower.contains("--output-format")
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
        summary.unsafe_functions += package.unsafe_functions;
        summary.unsafe_impls += package.unsafe_impls;
        summary.unsafe_traits += package.unsafe_traits;
        summary.unsafe_blocks += package.unsafe_blocks;
        summary.unsafe_expressions += package.unsafe_expressions;
        summary.total_unsafe += package.total_unsafe;
    }

    summary
}
