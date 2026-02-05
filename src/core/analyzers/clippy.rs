use serde::Deserialize;
use std::collections::HashMap;
use tokio::process::Command;

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

pub async fn run_clippy() -> Result<ClippyOutput> {
    let output = Command::new("cargo")
        .args([
            "clippy",
            "--message-format=json",
            "--all-targets",
            "--all-features",
        ])
        .output()
        .await
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
    Ok(parse_clippy_output(&stdout, clippy_failed, &clippy_status))
}

fn is_clippy_missing(stderr: &str) -> bool {
    let stderr = stderr.to_lowercase();
    stderr.contains("clippy is not installed")
        || stderr.contains("no such subcommand: clippy")
        || stderr.contains("component 'clippy' is missing")
        || stderr.contains("rustup component add clippy")
}

fn parse_clippy_output(stdout: &str, clippy_failed: bool, clippy_status: &str) -> ClippyOutput {
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

    ClippyOutput {
        warnings,
        errors,
        warnings_by_lint,
        details,
        score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clippy_missing_detection() {
        assert!(is_clippy_missing("clippy is not installed"));
        assert!(is_clippy_missing("no such subcommand: clippy"));
        assert!(is_clippy_missing("component 'clippy' is missing"));
        assert!(is_clippy_missing("rustup component add clippy"));
        assert!(!is_clippy_missing("some other error"));
    }

    #[test]
    fn parse_clippy_output_counts_and_details() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::needless_return"},"message":"avoid needless return","spans":[{"file_name":"src/lib.rs","line_start":10,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"error","code":{"code":"clippy::panic"},"message":"do not panic","spans":[{"file_name":"src/main.rs","line_start":42,"is_primary":false},{"file_name":"src/main.rs","line_start":43,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"warning","code":{"code":"dead_code"},"message":"unused","spans":[]}}
{"reason":"compiler-message","message":{"level":"note","code":{"code":"clippy::style"},"message":"note","spans":[]}}"#;

        let output = parse_clippy_output(stdout, false, "0");
        assert_eq!(output.warnings, 1);
        assert_eq!(output.errors, 1);
        assert_eq!(output.warnings_by_lint.get("clippy::needless_return"), Some(&1));
        assert_eq!(output.details.len(), 2);
        assert_eq!(output.details[0].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(output.details[0].line, Some(10));
        assert_eq!(output.details[1].line, Some(43));
        // Score calculation: 100 - (1 warning * 2) - (1 error * 10) = 100 - 2 - 10 = 88
        assert_eq!(output.score, 88.0);
    }

    #[test]
    fn parse_clippy_output_adds_synthetic_error_on_failure() {
        let output = parse_clippy_output("", true, "1");
        assert_eq!(output.errors, 1);
        assert_eq!(output.details.len(), 1);
        assert_eq!(output.details[0].lint, "clippy::driver");
        assert_eq!(output.details[0].level, "error");
    }
}
