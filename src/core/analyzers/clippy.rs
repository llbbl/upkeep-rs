#![allow(dead_code)]

use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;

use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{ClippyIssue, ClippyOutput};

#[derive(Debug, Deserialize)]
struct CargoMessage {
    reason: String,
    message: Option<DiagnosticMessage>,
}

#[derive(Debug, Deserialize)]
struct DiagnosticMessage {
    level: String,
    code: Option<DiagnosticCode>,
    message: String,
    spans: Vec<DiagnosticSpan>,
}

#[derive(Debug, Deserialize)]
struct DiagnosticCode {
    code: String,
}

#[derive(Debug, Deserialize)]
struct DiagnosticSpan {
    file_name: String,
    line_start: u32,
    is_primary: bool,
}

pub fn run_clippy() -> Result<ClippyOutput> {
    let output = Command::new("cargo")
        .args([
            "clippy",
            "--message-format=json",
            "--all-targets",
            "--all-features",
        ])
        .output()
        .map_err(|err| {
            UpkeepError::context(
                ErrorCode::ExternalCommand,
                "failed to execute cargo clippy",
                err,
            )
        })?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && is_clippy_missing(&stderr) {
        return Err(UpkeepError::message(
            ErrorCode::MissingTool,
            "clippy is not installed; run `rustup component add clippy`",
        ));
    }

    let clippy_failed = !output.status.success();
    let clippy_status = output.status.to_string();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut warnings = 0;
    let mut errors = 0;
    let mut warnings_by_lint: HashMap<String, usize> = HashMap::new();
    let mut details = Vec::new();

    for line in stdout.lines() {
        let message: CargoMessage = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => continue,
        };

        if message.reason != "compiler-message" {
            continue;
        }

        let Some(diagnostic) = message.message else {
            continue;
        };

        if diagnostic.level != "warning" && diagnostic.level != "error" {
            continue;
        }

        let lint = match diagnostic.code.as_ref().map(|code| code.code.clone()) {
            Some(code) if code.starts_with("clippy::") => code,
            _ => continue,
        };

        if diagnostic.level == "warning" {
            warnings += 1;
            *warnings_by_lint.entry(lint.clone()).or_insert(0) += 1;
        } else {
            errors += 1;
        }

        let span = diagnostic
            .spans
            .iter()
            .find(|span| span.is_primary)
            .or_else(|| diagnostic.spans.first());

        details.push(ClippyIssue {
            lint,
            level: diagnostic.level,
            message: diagnostic.message,
            file: span.map(|span| span.file_name.clone()),
            line: span.map(|span| span.line_start),
        });
    }

    // Only add synthetic error if clippy failed and no actual errors were parsed
    if clippy_failed && errors == 0 {
        errors += 1;
        details.push(ClippyIssue {
            lint: "clippy::driver".to_string(),
            level: "error".to_string(),
            message: format!("cargo clippy exited with status {clippy_status}"),
            file: None,
            line: None,
        });
    }

    let penalty = (warnings as u64 * 2) + (errors as u64 * 10);
    let score = 100u64.saturating_sub(penalty) as f32;

    Ok(ClippyOutput {
        warnings,
        errors,
        warnings_by_lint,
        details,
        score,
    })
}

fn is_clippy_missing(stderr: &str) -> bool {
    let stderr = stderr.to_lowercase();
    stderr.contains("clippy is not installed")
        || stderr.contains("no such subcommand: clippy")
        || stderr.contains("component 'clippy' is missing")
        || stderr.contains("rustup component add clippy")
}
