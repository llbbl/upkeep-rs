use serde::Deserialize;
use std::collections::HashMap;
use tokio::process::Command;

use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{ClippyIssue, ClippyOutput};
// The penalty formula is the scorer's, not this module's: `ClippyOutput.score`
// and the quality breakdown's Clippy entry are the same number reported by two
// commands, and this module having its own copy let them drift (#37).
use crate::core::scorers::quality::{clippy_score, ClippySummary};

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

    interpret_clippy_run(&output)
}

/// Turns a finished `cargo clippy` invocation into a result.
///
/// Split from [`run_clippy`] so the classification can be tested against a
/// synthesized [`std::process::Output`]: driving it through a real cargo
/// invocation would mean mutating the process working directory, which races
/// every other test in the suite.
fn interpret_clippy_run(output: &std::process::Output) -> Result<ClippyOutput> {
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
    let mut parsed = parse_diagnostics(&stdout);

    if clippy_failed && parsed.errors == 0 {
        // Clippy exited non-zero without reporting a single lint. That is the
        // driver failing — most often the crate does not compile — not a clean
        // run. Synthesizing a `clippy::driver` error here scores 100 - 10 = 90
        // and reports it as `Measured`, so a build that never got linted lands
        // in the grade at full weight and still asserts `complete: true`. It is
        // an unavailable metric, which is what the caller already knows how to
        // report.
        if parsed.details.is_empty() {
            return Err(UpkeepError::message(
                ErrorCode::ExternalCommand,
                format!(
                    "cargo clippy exited with status {clippy_status} without emitting any \
                     diagnostics; the crate likely failed to compile"
                ),
            ));
        }

        // Warnings were reported but the exit status is still non-zero (a
        // `-D warnings` configuration, for instance). The lints were measured,
        // so the run counts — the driver failure is recorded alongside them.
        push_driver_error(&mut parsed, &clippy_status);
    }

    Ok(parsed)
}

/// Detects an absent clippy component in rustup's or cargo's stderr.
///
/// Matched in two loose parts — a phrase meaning "absent", and the word
/// `clippy` anywhere in the same stderr — rather than as fixed sentences. The
/// fixed-sentence version missed the message users actually get, because
/// `cargo clippy` is a **rustup proxy**: rustup resolves the component before
/// cargo is ever reached, and its wording interpolates the toolchain. Verbatim,
/// from `rustup 1.28` on a toolchain without the component:
///
/// ```text
/// error: 'cargo-clippy' is not installed for the toolchain '1.96.0-aarch64-apple-darwin'.
/// help: run `rustup component add --toolchain 1.96.0-aarch64-apple-darwin clippy` to install it
/// ```
///
/// Both halves defeat contiguous matching: `'cargo-clippy' is not installed`
/// breaks `"clippy is not installed"` on the apostrophe, and the
/// `--toolchain <name>` infix breaks `"rustup component add clippy"`. Only the
/// bare default-toolchain spelling was caught, so the check silently misfired
/// under `rust-toolchain.toml`, `cargo +<version>` and `RUSTUP_TOOLCHAIN` —
/// exactly the setups a pinned-MSRV project uses.
///
/// This has to fire for the right reason to be reported: an uninstalled clippy
/// that is not recognized here reaches the driver-failure path in
/// [`interpret_clippy_run`] and is reported as `failed` rather than
/// `not_installed`, which sends the user looking for a broken analyzer instead
/// of a missing component.
///
/// The `no such command` spelling is current cargo's (1.98) and `no such
/// subcommand` is the older wording; these are what a cargo installed without
/// rustup emits, and neither carries an install hint, so they stay in the
/// "absent" set rather than being dropped in favour of the rustup wording.
/// `component 'clippy' is missing` is kept as a literal because it already
/// names the component — a bare `is missing` would match an unrelated build
/// failure, and clippy's own lint help URLs put `clippy` in stderr on nearly
/// every run, so the second half is not the discriminator it looks like.
fn is_clippy_missing(stderr: &str) -> bool {
    let stderr = stderr.to_lowercase();
    let absent = stderr.contains("rustup component add")
        || stderr.contains("is not installed")
        || stderr.contains("no such subcommand")
        || stderr.contains("no such command");
    (absent && stderr.contains("clippy")) || stderr.contains("component 'clippy' is missing")
}

/// Parses the clippy diagnostics from `cargo clippy --message-format=json`.
///
/// This reports only what clippy actually said. The exit status is handled by
/// the caller, because "clippy ran and found nothing" and "clippy never ran"
/// are different outcomes that must not both become a `ClippyOutput`.
///
/// `pub(crate)` only so `clippy_command_and_quality_breakdown_agree` in
/// `cli::commands::quality` can build a real `ClippyOutput` from fixture
/// diagnostics. That test has to run the genuine `build_quality_output` to
/// cover the `ClippyOutput` -> `ClippySummary` mapping, and a hand-built
/// fixture would have to compute `score` itself — reintroducing exactly the
/// second copy of the formula #37 removed. It stays a pure function of its
/// input; `run_clippy` remains the only production entry point.
pub(crate) fn parse_diagnostics(stdout: &str) -> ClippyOutput {
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

    ClippyOutput {
        score: clippy_score(&ClippySummary { warnings, errors }),
        warnings,
        errors,
        warnings_by_lint,
        details,
    }
}

/// Records a non-zero clippy exit as an error alongside the lints it did emit.
fn push_driver_error(output: &mut ClippyOutput, clippy_status: &str) {
    output.errors += 1;
    output.details.push(ClippyIssue {
        lint: "clippy::driver".to_string(),
        level: "error".to_string(),
        message: format!("cargo clippy exited with status {clippy_status}"),
        file: None,
        line: None,
    });
    output.score = clippy_score(&ClippySummary {
        warnings: output.warnings,
        errors: output.errors,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim stderr from `RUSTUP_TOOLCHAIN=1.96.0-aarch64-apple-darwin cargo
    /// clippy --version` on a toolchain without the component, captured from
    /// rustup rather than written from memory.
    ///
    /// This is what users actually see, because `cargo clippy` is a rustup
    /// proxy: rustup resolves the component and fails before cargo runs, so
    /// cargo's `no such command` wording never appears.
    const RUSTUP_MISSING_CLIPPY: &str = "\
error: 'cargo-clippy' is not installed for the toolchain '1.96.0-aarch64-apple-darwin'.
help: run `rustup component add --toolchain 1.96.0-aarch64-apple-darwin clippy` to install it
";

    #[test]
    fn clippy_missing_detection() {
        // The real message, and the reason the previous patterns missed it:
        // `'cargo-clippy' is not installed` breaks "clippy is not installed" on
        // the apostrophe, and the `--toolchain <name>` infix breaks "rustup
        // component add clippy". Only the bare default-toolchain spelling was
        // caught, so this misfired under rust-toolchain.toml, `cargo +<version>`
        // and RUSTUP_TOOLCHAIN.
        assert!(is_clippy_missing(RUSTUP_MISSING_CLIPPY));

        // The same message for the implicit default toolchain, which has no
        // `--toolchain` infix and no quoted toolchain name.
        assert!(is_clippy_missing(
            "error: 'cargo-clippy' is not installed for the toolchain 'stable'.\n\
             help: run `rustup component add clippy` to install it\n"
        ));

        // A cargo installed without rustup has no component to add, and says so
        // in its own words. Backticks and all, from cargo 1.98 and its
        // predecessor's spelling.
        assert!(is_clippy_missing("error: no such command: `clippy`\n"));
        assert!(is_clippy_missing("error: no such subcommand: `clippy`\n"));

        // Older rustup wording, which names the component itself.
        assert!(is_clippy_missing("component 'clippy' is missing"));

        assert!(!is_clippy_missing("some other error"));

        // Lint output is not a missing component. Clippy puts its own name in
        // stderr on nearly every run via the help URL, so the `clippy` half of
        // the match carries no weight on its own — an unrelated failure must
        // not be reported as an uninstalled component.
        assert!(!is_clippy_missing(
            "warning: unused variable: `x`\n  = help: for further information visit \
             https://rust-lang.github.io/rust-clippy/master/index.html#unused_variable\n\
             error: could not compile `demo` (lib) due to 1 previous error\n"
        ));
    }

    #[test]
    fn parse_clippy_output_counts_and_details() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::needless_return"},"message":"avoid needless return","spans":[{"file_name":"src/lib.rs","line_start":10,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"error","code":{"code":"clippy::panic"},"message":"do not panic","spans":[{"file_name":"src/main.rs","line_start":42,"is_primary":false},{"file_name":"src/main.rs","line_start":43,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"warning","code":{"code":"dead_code"},"message":"unused","spans":[]}}
{"reason":"compiler-message","message":{"level":"note","code":{"code":"clippy::style"},"message":"note","spans":[]}}"#;

        let output = parse_diagnostics(stdout);
        assert_eq!(output.warnings, 1);
        assert_eq!(output.errors, 1);
        assert_eq!(
            output.warnings_by_lint.get("clippy::needless_return"),
            Some(&1)
        );
        assert_eq!(output.details.len(), 2);
        assert_eq!(output.details[0].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(output.details[0].line, Some(10));
        assert_eq!(output.details[1].line, Some(43));
        // Score calculation: 100 - (1 warning * 2) - (1 error * 10) = 100 - 2 - 10 = 88
        assert_eq!(output.score, 88.0);
    }

    /// A driver failure with no diagnostics at all is not a `ClippyOutput`.
    ///
    /// `run_clippy` returns `Err` for this, which `quality` reports as an
    /// unavailable metric. The old behavior synthesized a single
    /// `clippy::driver` error, scored it 90, weighted it at the full 0.15 and
    /// still claimed `complete: true` — a crate that did not compile presented
    /// as near-perfect. This test pins the *shape* of that decision; the
    /// end-to-end path is exercised by `run_clippy` itself.
    #[test]
    fn parse_diagnostics_reports_nothing_when_clippy_emitted_nothing() {
        let output = parse_diagnostics("");
        assert_eq!(output.warnings, 0);
        assert_eq!(output.errors, 0);
        assert!(output.details.is_empty());
        // Nothing was measured, so nothing may be scored from it. `run_clippy`
        // turns this state into an error rather than passing on the 100.
        assert_eq!(output.score, 100.0);
    }

    /// Clippy exiting non-zero *while reporting lints* still counts as a run.
    ///
    /// A `-D warnings` configuration produces exactly this: real diagnostics
    /// plus a failing status. The lints were measured, so the driver failure is
    /// recorded alongside them instead of discarding the result.
    #[test]
    fn push_driver_error_records_failure_alongside_real_lints() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::needless_return"},"message":"avoid needless return","spans":[{"file_name":"src/lib.rs","line_start":10,"is_primary":true}]}}"#;
        let mut output = parse_diagnostics(stdout);
        assert_eq!(output.score, 98.0);

        push_driver_error(&mut output, "exit status: 101");

        assert_eq!(output.warnings, 1);
        assert_eq!(output.errors, 1);
        assert_eq!(output.details.len(), 2);
        assert_eq!(output.details[1].lint, "clippy::driver");
        assert_eq!(output.details[1].level, "error");
        assert!(output.details[1].message.contains("exit status: 101"));
        // The score is recomputed, not left at the pre-failure value.
        assert_eq!(output.score, 88.0);
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

    /// The real driver-failure path: clippy on a crate that does not compile.
    ///
    /// rustc never gets far enough to run a single lint, so clippy exits
    /// non-zero having emitted no `clippy::` diagnostics. That must be an
    /// error, not a `ClippyOutput` scoring 90 at full weight while `quality`
    /// reports `complete: true`.
    #[test]
    fn interpret_clippy_run_errors_when_driver_failed_without_diagnostics() {
        let output = output_with(
            101,
            r#"{"reason":"compiler-message","message":{"level":"error","code":null,"message":"expected one of `)`","spans":[]}}"#,
            "error: could not compile `broken` (lib) due to 1 previous error\n",
        );

        let err = interpret_clippy_run(&output)
            .expect_err("a crate that does not compile is not a clippy score");
        assert_eq!(err.code(), ErrorCode::ExternalCommand);
        assert!(
            err.to_string().contains("without emitting any diagnostics"),
            "unexpected error: {err}"
        );
    }

    /// A missing clippy component keeps its own, more actionable error.
    ///
    /// Driven by the message rustup really emits, not cargo's — the previous
    /// version of this test fed `no such subcommand: \`clippy\``, which no
    /// rustup-managed toolchain produces, so the path that actually runs was
    /// never exercised.
    #[test]
    fn interpret_clippy_run_reports_a_missing_component_as_missing_tool() {
        let output = output_with(101, "", RUSTUP_MISSING_CLIPPY);
        let err = interpret_clippy_run(&output).unwrap_err();
        assert_eq!(err.code(), ErrorCode::MissingTool);
        assert!(err.to_string().contains("rustup component add clippy"));
    }

    /// A clean run is still a clean run.
    #[test]
    fn interpret_clippy_run_accepts_a_successful_run_with_no_lints() {
        let output = interpret_clippy_run(&output_with(0, "", "")).unwrap();
        assert_eq!(output.warnings, 0);
        assert_eq!(output.errors, 0);
        assert_eq!(output.score, 100.0);
    }

    /// Non-zero exit *with* lints keeps the lints and records the failure.
    #[test]
    fn interpret_clippy_run_keeps_lints_when_the_exit_status_is_non_zero() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::needless_return"},"message":"avoid needless return","spans":[{"file_name":"src/lib.rs","line_start":10,"is_primary":true}]}}"#;
        let parsed = interpret_clippy_run(&output_with(101, stdout, "")).unwrap();

        assert_eq!(parsed.warnings, 1);
        assert_eq!(parsed.errors, 1);
        assert_eq!(parsed.details[1].lint, "clippy::driver");
        assert_eq!(parsed.score, 88.0);
    }
}
