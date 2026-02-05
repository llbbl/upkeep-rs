//! Shared utilities for running external cargo tools (geiger, machete, etc.).
//!
//! This module provides a common abstraction for running external cargo subcommands
//! and handling their output, including error detection for missing tools and
//! unknown flags.

use std::io;
use std::path::Path;
use tokio::process::Command;

use crate::core::error::{ErrorCode, Result, UpkeepError};

/// Configuration for running an external cargo tool.
pub struct ExternalToolConfig<'a> {
    /// The name of the tool (e.g., "geiger", "machete")
    pub tool_name: &'a str,
    /// The installation command hint (e.g., "cargo install cargo-geiger")
    pub install_hint: &'a str,
}

/// Runs a cargo subcommand and returns its output.
///
/// This function handles common error cases:
/// - Cargo not found on PATH
/// - Other I/O errors during execution
pub async fn run_cargo_tool(
    args: &[&str],
    workspace_root: &Path,
    config: &ExternalToolConfig<'_>,
) -> Result<std::process::Output> {
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
                format!("failed to execute cargo {}", config.tool_name),
                err,
            ),
        })
}

/// Handles common error cases in cargo tool output.
///
/// Returns `Ok(output)` if:
/// - The command succeeded
/// - The command failed but produced stdout (some tools exit non-zero but still work)
///
/// Returns an error if:
/// - The tool is not installed (detected via stderr patterns)
/// - The command failed with no useful output
pub fn handle_tool_output(
    output: std::process::Output,
    config: &ExternalToolConfig<'_>,
    is_missing_tool: impl FnOnce(&str) -> bool,
) -> Result<std::process::Output> {
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_missing_tool(&stderr) {
        return Err(UpkeepError::message(
            ErrorCode::MissingTool,
            format!(
                "cargo-{} is not installed; install with `{}`",
                config.tool_name, config.install_hint
            ),
        ));
    }

    // Some tools exit non-zero but still produce valid output
    if !output.stdout.is_empty() {
        return Ok(output);
    }

    let message = stderr.trim();
    if message.is_empty() {
        return Err(UpkeepError::message(
            ErrorCode::ExternalCommand,
            format!("cargo {} failed with no stderr output", config.tool_name),
        ));
    }
    Err(UpkeepError::message(
        ErrorCode::ExternalCommand,
        format!("cargo {} failed: {message}", config.tool_name),
    ))
}

/// Checks if stderr indicates a missing cargo subcommand.
///
/// This detects patterns like "no such subcommand" or "unknown subcommand"
/// combined with the tool name.
pub fn is_missing_subcommand(stderr: &str, tool_name: &str) -> bool {
    let lower = stderr.to_lowercase();
    let tool_name_lower = tool_name.to_lowercase();
    (lower.contains("no such subcommand") || lower.contains("unknown subcommand"))
        && lower.contains(&tool_name_lower)
}

/// Checks if stderr indicates an unknown command-line flag.
///
/// This detects patterns like "unexpected argument" or "found argument"
/// combined with the flag name.
pub fn is_unknown_flag(stderr: &str, flag_name: &str) -> bool {
    let lower = stderr.to_lowercase();
    let flag_name_lower = flag_name.to_lowercase();
    (lower.contains("unexpected argument") || lower.contains("found argument"))
        && lower.contains(&flag_name_lower)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_is_missing_subcommand() {
        // Positive cases - should detect missing subcommand
        assert!(is_missing_subcommand(
            "error: no such subcommand: `geiger`",
            "geiger"
        ));
        assert!(is_missing_subcommand(
            "error: Unknown subcommand 'machete'",
            "machete"
        ));
        // Case insensitive
        assert!(is_missing_subcommand(
            "error: NO SUCH SUBCOMMAND: `GEIGER`",
            "geiger"
        ));

        // Negative cases - should not detect
        assert!(!is_missing_subcommand("some other error", "geiger"));
        assert!(!is_missing_subcommand(
            "no such subcommand: `other`",
            "geiger"
        ));
        // Missing the subcommand pattern
        assert!(!is_missing_subcommand("geiger is not installed", "geiger"));
        // Empty stderr
        assert!(!is_missing_subcommand("", "geiger"));
    }

    #[test]
    fn test_is_unknown_flag() {
        // Positive cases - should detect unknown flag
        assert!(is_unknown_flag(
            "error: unexpected argument '--output-format'",
            "--output-format"
        ));
        assert!(is_unknown_flag(
            "error: Found argument '--json' which wasn't expected",
            "--json"
        ));
        // Case insensitive
        assert!(is_unknown_flag(
            "error: UNEXPECTED ARGUMENT '--JSON'",
            "--json"
        ));

        // Negative cases - should not detect
        assert!(!is_unknown_flag("some other error", "--json"));
        assert!(!is_unknown_flag("unexpected argument '--other'", "--json"));
        // Missing the flag pattern
        assert!(!is_unknown_flag("--json is not recognized", "--json"));
        // Empty stderr
        assert!(!is_unknown_flag("", "--json"));
    }

    #[test]
    fn test_is_missing_subcommand_with_multiline_stderr() {
        let stderr = r#"error: no such subcommand: `geiger`

        Did you mean `generate`?

        View all installed commands with `cargo --list`"#;
        assert!(is_missing_subcommand(stderr, "geiger"));
    }

    #[test]
    fn test_is_unknown_flag_with_multiline_stderr() {
        let stderr = r#"error: Found argument '--output-format' which wasn't expected, or isn't valid in this context

        USAGE:
            cargo machete [OPTIONS]

        For more information try --help"#;
        assert!(is_unknown_flag(stderr, "--output-format"));
    }

    #[test]
    fn handle_tool_output_success_returns_ok() {
        let config = ExternalToolConfig {
            tool_name: "machete",
            install_hint: "cargo install cargo-machete",
        };
        let output = output_with(0, "{\"unused\":[]}", "");
        let result = handle_tool_output(output, &config, |_| false).unwrap();
        assert!(!result.stdout.is_empty());
    }

    #[test]
    fn handle_tool_output_missing_tool_returns_missing_tool_error() {
        let config = ExternalToolConfig {
            tool_name: "geiger",
            install_hint: "cargo install cargo-geiger",
        };
        let output = output_with(1, "", "error: no such subcommand: `geiger`");
        let err = handle_tool_output(output, &config, |stderr| {
            is_missing_subcommand(stderr, "geiger")
        })
        .unwrap_err();

        assert!(matches!(err, UpkeepError::Message { .. }));
        assert_eq!(err.code(), ErrorCode::MissingTool);
        assert!(err.to_string().contains("cargo-geiger is not installed"));
    }

    #[test]
    fn handle_tool_output_nonzero_with_stdout_is_ok() {
        let config = ExternalToolConfig {
            tool_name: "machete",
            install_hint: "cargo install cargo-machete",
        };
        let output = output_with(1, "{\"unused\":[]}", "warning");
        let result = handle_tool_output(output, &config, |_| false).unwrap();
        assert!(!result.stdout.is_empty());
    }

    #[test]
    fn handle_tool_output_empty_stderr_returns_error() {
        let config = ExternalToolConfig {
            tool_name: "machete",
            install_hint: "cargo install cargo-machete",
        };
        let output = output_with(1, "", "");
        let err = handle_tool_output(output, &config, |_| false).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ExternalCommand);
        assert!(err
            .to_string()
            .contains("cargo machete failed with no stderr output"));
    }

    #[test]
    fn handle_tool_output_stderr_message_bubbles_up() {
        let config = ExternalToolConfig {
            tool_name: "machete",
            install_hint: "cargo install cargo-machete",
        };
        let output = output_with(1, "", "boom");
        let err = handle_tool_output(output, &config, |_| false).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ExternalCommand);
        assert!(err.to_string().contains("cargo machete failed: boom"));
    }
}
