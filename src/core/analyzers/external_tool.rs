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
/// - The working directory does not exist
/// - Cargo not found on PATH
/// - Other I/O errors during execution
///
/// Spawning reports `NotFound` for two unrelated failures: the `cargo` binary
/// was not on PATH, and the `current_dir` chdir hit ENOENT. Only the first is a
/// missing tool, so `workspace_root` is checked before the error is attributed
/// to cargo. Getting this wrong reports a broken environment as
/// [`ErrorCode::MissingTool`], which `quality` renders as "an optional tool
/// isn't installed, nothing is wrong with your project" — with a message that
/// is flatly false.
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
            // The chdir failed, not the PATH lookup. Checked after the fact
            // rather than before: a pre-flight check would race the spawn, and
            // this only has to explain a failure that already happened.
            io::ErrorKind::NotFound if !workspace_root.is_dir() => UpkeepError::context(
                ErrorCode::Io,
                format!(
                    "workspace directory does not exist or is not a directory: {}",
                    workspace_root.display()
                ),
                err,
            ),
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

/// Patterns cargo uses to report a subcommand it cannot find.
///
/// Cargo's wording has changed over time. Older releases emit
/// ``error: no such subcommand: `machete` ``; current cargo (1.98) emits
/// ``error: no such command: `machete` ``. Both spellings are matched, along
/// with the documented historical `unknown subcommand` wording, so the
/// detection keeps working across the toolchain range this crate supports.
///
/// Every pattern here widens the surface for misclassifying an unrelated cargo
/// failure as a missing tool, so the list stays limited to wording cargo is
/// known to have used. `unknown command` is deliberately absent: no cargo
/// release is known to emit it.
const MISSING_SUBCOMMAND_PATTERNS: [&str; 3] = [
    "no such subcommand",
    "unknown subcommand",
    "no such command",
];

/// Checks if stderr indicates a missing cargo subcommand.
///
/// A match requires a single line to carry both one of
/// [`MISSING_SUBCOMMAND_PATTERNS`] and the tool name, with the tool name
/// appearing *after* the pattern — the shape of every wording cargo is known to
/// use, ``error: no such command: `geiger` ``.
///
/// Both constraints exist because the tools this runs on shell out to cargo
/// themselves: `cargo-geiger` invokes `cargo metadata`, so a nested "no such
/// command" for an unrelated subcommand can land in the same stderr as outer
/// text naming the outer tool. Matching anywhere in the buffer reads that as
/// "geiger is not installed". The same-line rule rejects the multi-line form;
/// the ordering rule rejects the single-line form an error chain flattens into
/// (``cargo-geiger failed: error: no such command: `metadata` ``), where the
/// tool name precedes the pattern. Cargo's own message can never take that
/// shape: when the subcommand really is missing, the binary that would prefix
/// its own name to the line never runs.
///
/// This is what makes [`ErrorCode::MissingTool`] fire, which in turn is what
/// lets `quality` report "optional tool not installed" separately from
/// "analyzer ran and failed" — so a wording drift here silently degrades that
/// distinction rather than breaking loudly.
pub fn is_missing_subcommand(stderr: &str, tool_name: &str) -> bool {
    let tool_name_lower = tool_name.to_lowercase();
    stderr.lines().any(|line| {
        let lower = line.to_lowercase();
        MISSING_SUBCOMMAND_PATTERNS.iter().any(|pattern| {
            lower
                .find(pattern)
                .is_some_and(|start| lower[start + pattern.len()..].contains(&tool_name_lower))
        })
    })
}

/// Patterns the argument parsers behind us use to report a flag they do not
/// recognize.
///
/// Two parsers are covered, because the two tools this drives do not use the
/// same one — and neither uses clap.
///
/// **clap**, whose wording changed at the 4.0 boundary. clap 2 and 3 emit
/// ``error: Found argument '--output-format' which wasn't expected, or isn't
/// valid in this context``; clap 4 emits
/// ``error: unexpected argument '--output-format' found``. Both are matched.
/// clap reaches us second-hand, from a **nested** `cargo` invocation — geiger
/// shells out to `cargo metadata`, and cargo is clap 4.
///
/// **argh**, which `cargo-machete` uses (every release from 0.5.0 through
/// 0.9.2). Its wording is `Unrecognized argument: --json`, captured verbatim
/// by running `cargo machete --json` against a pinned 0.9.2 binary, not
/// recalled from documentation. That is not a hypothetical older release:
/// **no published `cargo-machete` accepts `--json`** — the flag exists only on
/// the project's unreleased master — so without this pattern the `unused.rs`
/// fallback never fires and the analyzer fails outright against every
/// installable version of the tool.
///
/// `cargo-geiger` is the case no pattern can reach. It parses with
/// `pico-args`, whose error type has no unknown-argument variant at all, and
/// it never calls `Arguments::finish()` (checked 0.9.1 through 0.13.0), so the
/// leftovers holding an unrecognized flag are dropped in silence and there is
/// no stderr to match. Against a geiger too old for `--output-format` the run
/// therefore completes with default non-JSON output. No pattern fixes that —
/// there is nothing to match on — and no retry fixes it either, because
/// `--format` is a different flag rather than an alternate spelling. That case
/// is instead recognized from the *stdout* it produced; see
/// `looks_like_geiger_table` in `unsafe_code.rs`.
///
/// Every pattern here widens the surface for misreading an unrelated argument
/// error as "this specific flag is unsupported", so the list stays limited to
/// wording a parser one of these tools actually uses is known to emit.
const UNKNOWN_FLAG_PATTERNS: [&str; 3] = [
    "unexpected argument",
    "found argument",
    "unrecognized argument",
];

/// Checks if stderr indicates an unknown command-line flag.
///
/// A match requires a single line to carry both one of
/// [`UNKNOWN_FLAG_PATTERNS`] and the flag name, with the flag name appearing
/// *after* the pattern — the shape of every wording matched here, verified by
/// running clap 2.34, 3.2, and 4.6, and `cargo-machete` 0.9.2 (argh), against
/// an unrecognized flag.
///
/// Both constraints exist for the reason [`is_missing_subcommand`] has them:
/// the tools this runs on shell out to cargo themselves, so an argument error
/// from a nested invocation can land in the same stderr as outer text naming
/// the flag we asked about. The same-line rule rejects the multi-line form; the
/// ordering rule rejects the single-line form an error chain flattens into
/// (`error: --output-format requires metadata: unexpected argument '--x' found`),
/// where the flag name precedes the pattern. Neither parser's own message can
/// take that shape: the parse error is emitted before the tool's own code runs,
/// so there is nothing to prefix it with.
///
/// This drives the retry-with-alternate-flags fallback in the `unsafe_code` and
/// `unused` analyzers, not [`ErrorCode::MissingTool`], so a false positive here
/// costs one wasted retry rather than a wrong diagnosis.
pub fn is_unknown_flag(stderr: &str, flag_name: &str) -> bool {
    let flag_name_lower = flag_name.to_lowercase();
    stderr.lines().any(|line| {
        let lower = line.to_lowercase();
        UNKNOWN_FLAG_PATTERNS.iter().any(|pattern| {
            lower
                .find(pattern)
                .is_some_and(|start| lower[start + pattern.len()..].contains(&flag_name_lower))
        })
    })
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

    /// Verbatim stderr from cargo 1.98 for an uninstalled subcommand.
    ///
    /// Cargo dropped the word "sub" from this message, which previously made
    /// [`is_missing_subcommand`] return `false` for every uninstalled optional
    /// tool on a modern toolchain.
    #[test]
    fn test_is_missing_subcommand_current_cargo_wording() {
        let machete = "error: no such command: `machete`\n\n\
                       help: view all installed commands with `cargo --list`\n\
                       help: find a package to install `machete` with `cargo search cargo-machete`\n";
        assert!(is_missing_subcommand(machete, "machete"));

        let geiger = "error: no such command: `geiger`\n\n\
                      help: view all installed commands with `cargo --list`\n\
                      help: find a package to install `geiger` with `cargo search cargo-geiger`\n";
        assert!(is_missing_subcommand(geiger, "geiger"));

        // The tool name still has to match: machete's message is not geiger's.
        assert!(!is_missing_subcommand(machete, "geiger"));

        // `unknown command` is not a wording cargo is known to use, and is not
        // matched: an unverified pattern only widens the false-positive surface.
        assert!(!is_missing_subcommand(
            "error: unknown command `machete`",
            "machete"
        ));
    }

    #[test]
    fn handle_tool_output_current_cargo_wording_is_missing_tool() {
        let config = ExternalToolConfig {
            tool_name: "machete",
            install_hint: "cargo install cargo-machete",
        };
        let output = output_with(
            101,
            "",
            "error: no such command: `machete`\n\n\
             help: view all installed commands with `cargo --list`\n",
        );
        let err = handle_tool_output(output, &config, |stderr| {
            is_missing_subcommand(stderr, "machete")
        })
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::MissingTool);
        assert!(err.to_string().contains("cargo-machete is not installed"));
    }

    /// A nested cargo failure must not be read as the outer tool missing.
    ///
    /// `cargo-geiger` shells out to `cargo metadata`. When that nested call
    /// fails, cargo's "no such command" lands in the same stderr buffer as
    /// outer error text naming geiger — but never on the same line. Matching
    /// across the whole buffer reported an installed geiger as missing, and
    /// told the user to install a tool they already had.
    #[test]
    fn is_missing_subcommand_requires_pattern_and_tool_on_one_line() {
        let nested = "error: no such command: `metadata`\n\n\
                      help: view all installed commands with `cargo --list`\n\
                      error: could not run cargo-geiger\n";
        assert!(!is_missing_subcommand(nested, "geiger"));

        // The same buffer with the real wording on one line still fires: the
        // rule is same-line, not first-line.
        let genuine = "warning: unrelated noise\n\
                       error: no such command: `geiger`\n\n\
                       help: view all installed commands with `cargo --list`\n";
        assert!(is_missing_subcommand(genuine, "geiger"));
    }

    /// The tool name has to follow the pattern, not merely share its line.
    ///
    /// An error chain flattened onto one line puts the outer tool's name before
    /// the nested cargo message, which a bare same-line check still accepts.
    /// Cargo's own message can never take this shape: if the subcommand were
    /// really missing, the binary naming itself in the prefix would not have
    /// run at all.
    #[test]
    fn is_missing_subcommand_requires_tool_name_after_the_pattern() {
        assert!(!is_missing_subcommand(
            "error: cargo-geiger failed: error: no such command: `metadata`",
            "geiger"
        ));
        // Same line, correct order — cargo's actual wording — still matches.
        assert!(is_missing_subcommand(
            "error: no such command: `geiger`",
            "geiger"
        ));
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

    /// Pins the error line each clap generation emits for an unrecognized flag.
    ///
    /// The error lines were captured by running clap 2.34, 3.2 and 4.6 against
    /// `--output-format`, not recalled from documentation. clap 4 reworded the
    /// message and moved the word "found" behind the flag name —
    /// `unexpected argument '--x' found` — which is why "found argument" alone
    /// does not cover it, and `unexpected argument` alone does not cover 2 or 3.
    ///
    /// This test does not prove the per-line fix; it passed before it. It is
    /// here because it is the only thing pinning `found argument`'s reason to
    /// exist, so a later pass that "simplifies" the pattern list to one entry
    /// cannot do so with a green suite.
    ///
    /// The buffers carry each version's error line, not its whole output: clap
    /// 3.2 adds a hint line clap 2.34 does not, and clap 4 adds a
    /// `tip: a similar argument exists` line for a near-miss flag. Only the
    /// error line is read, and on that line 2.34 and 3.2 are byte-identical.
    #[test]
    fn is_unknown_flag_pins_clap_error_wording() {
        // The error line clap 2.34 and clap 3.2 both emit.
        let clap2_3 = "error: Found argument '--output-format' which wasn't expected, \
                       or isn't valid in this context\n\n\
                       USAGE:\n    \
                       cargo-machete [OPTIONS]\n\n\
                       For more information try --help\n";
        assert!(is_unknown_flag(clap2_3, "--output-format"));

        let clap4 = "error: unexpected argument '--output-format' found\n\n\
                     Usage: cargo-machete [OPTIONS]\n\n\
                     For more information, try '--help'.\n";
        assert!(is_unknown_flag(clap4, "--output-format"));

        // A different flag's rejection is not this flag's rejection. The flag
        // has to appear in the buffer for this to test anything — `--json`
        // occurs nowhere above, so asserting on it would pass trivially.
        let other = "error: unexpected argument '--json' found\n\n\
                     Usage: cargo-machete [OPTIONS]\n";
        assert!(is_unknown_flag(other, "--json"));
        assert!(!is_unknown_flag(other, "--output-format"));
    }

    /// Pins the error argh emits for an unrecognized flag, as `cargo-machete`
    /// actually emits it.
    ///
    /// The buffer is the verbatim stderr of `cargo machete --json` run against
    /// a pinned `cargo-machete` 0.9.2 — the latest release — captured with
    /// `od -c`, not recalled from documentation. It exits 1 with empty stdout,
    /// so `handle_tool_output` turns it into a hard error unless the retry in
    /// `unused.rs` fires first.
    ///
    /// This is not a legacy-version courtesy. No published `cargo-machete`
    /// accepts `--json`; the flag exists only on the project's unreleased
    /// master. Before this pattern existed the analyzer failed outright with
    /// ``cargo machete failed: Unrecognized argument: --json`` against every
    /// installable version of the tool.
    ///
    /// argh's shape satisfies both structural rules without changing them: the
    /// flag sits on the pattern's own line and follows it. It also prints no
    /// options listing — only a `Run --help` line — so the false positive
    /// `is_unknown_flag_ignores_flags_echoed_in_a_usage_block` guards against
    /// cannot arise from argh at all.
    #[test]
    fn is_unknown_flag_pins_argh_error_wording() {
        let argh = "Unrecognized argument: --json\n\n\
                    Run --help for more information.\n";
        assert!(is_unknown_flag(argh, "--json"));

        // A different flag's rejection is not this flag's rejection. This one
        // passes on absence — `--with-metadata` is nowhere in the buffer — so
        // it is a cheap sanity check, not the discrimination test. The
        // assertion below is the one that discriminates: the flag IS present,
        // and is rejected only because it precedes the pattern.
        assert!(!is_unknown_flag(argh, "--with-metadata"));

        // The ordering rule applies to argh's wording too: a flattened error
        // chain that names our flag before a nested tool's argument error is
        // not argh rejecting our flag.
        assert!(!is_unknown_flag(
            "error: --json requires metadata: Unrecognized argument: --other",
            "--json"
        ));
    }

    /// A nested argument error must not be read as the outer flag being unknown.
    ///
    /// `cargo-geiger` shells out to `cargo metadata`. When that nested call
    /// rejects an argument, clap's wording lands in the same stderr buffer as
    /// outer error text naming the flag we actually asked about — but never on
    /// the same line. Matching across the whole buffer sent the analyzer down
    /// the alternate-flag retry path for a failure the retry cannot fix.
    #[test]
    fn is_unknown_flag_requires_pattern_and_flag_on_one_line() {
        let nested = "error: unexpected argument '--bogus-flag' found\n\n\
                      Usage: cargo metadata [OPTIONS]\n\
                      error: cargo-geiger --output-format failed\n";
        assert!(!is_unknown_flag(nested, "--output-format"));

        // The same buffer with the real wording on one line still fires: the
        // rule is same-line, not first-line.
        let genuine = "warning: unrelated noise\n\
                       error: unexpected argument '--output-format' found\n\n\
                       Usage: cargo-geiger [OPTIONS]\n";
        assert!(is_unknown_flag(genuine, "--output-format"));
    }

    /// A help listing that names the flag is not a rejection of the flag.
    ///
    /// clap prints the accepted options underneath its error, so a buffer that
    /// rejects `--bogus-flag` also spells out `--output-format` as a *supported*
    /// flag a few lines down. Whole-buffer matching read that as "the flag we
    /// asked about is unknown" and retried with the alternate spelling, which
    /// fails identically because the real problem was never the flag.
    ///
    /// This is also what makes `test_is_unknown_flag_with_multiline_stderr`
    /// meaningful rather than lucky: there the flag sits on the error line, and
    /// its USAGE block is inert.
    #[test]
    fn is_unknown_flag_ignores_flags_echoed_in_a_usage_block() {
        let stderr = "error: unexpected argument '--bogus-flag' found\n\n\
                      Usage: cargo-geiger [OPTIONS]\n\n\
                      Options:\n      \
                      --output-format <FMT>  Output format\n";
        assert!(!is_unknown_flag(stderr, "--output-format"));
        // The flag that really was rejected still matches.
        assert!(is_unknown_flag(stderr, "--bogus-flag"));
    }

    /// The flag name has to follow the pattern, not merely share its line.
    ///
    /// An error chain flattened onto one line puts the flag we asked about
    /// before the nested tool's argument error, which a bare same-line check
    /// still accepts. clap's own message can never take this shape: the parse
    /// error is emitted before the tool's own code runs, so no prefix naming
    /// the flag can precede it.
    #[test]
    fn is_unknown_flag_requires_flag_name_after_the_pattern() {
        assert!(!is_unknown_flag(
            "error: --output-format requires metadata: unexpected argument '--bogus-flag' found",
            "--output-format"
        ));
        // Same line, correct order — clap's actual wording — still matches.
        assert!(is_unknown_flag(
            "error: unexpected argument '--output-format' found",
            "--output-format"
        ));
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

    /// A missing working directory is an environment failure, not a missing tool.
    ///
    /// Spawning with a `current_dir` that does not exist fails the chdir with
    /// ENOENT, which surfaces as `io::ErrorKind::NotFound` — the same kind as a
    /// cargo that is not on PATH. Attributing it to cargo downgrades a broken
    /// environment to "an optional tool isn't installed" via
    /// [`ErrorCode::MissingTool`], and says so with a message that is false.
    #[tokio::test]
    async fn run_cargo_tool_missing_workspace_dir_is_not_a_missing_tool() {
        use std::error::Error as _;

        let temp = tempfile::tempdir().expect("temp dir");
        let missing = temp.path().join("no-such-workspace");
        let config = ExternalToolConfig {
            tool_name: "machete",
            install_hint: "cargo install cargo-machete",
        };

        let err = run_cargo_tool(&["--version"], &missing, &config)
            .await
            .expect_err("spawning in a missing directory must fail");

        // The point of the fix, and true on every platform.
        assert_ne!(err.code(), ErrorCode::MissingTool);
        assert!(
            err.source().is_some(),
            "the io::Error must be preserved as the source"
        );

        // Only Unix reports the failed chdir as `NotFound`, so only there does
        // the guard fire and name the directory. Windows fails the same spawn
        // with `ERROR_DIRECTORY` -> `NotADirectory`, which falls to the generic
        // arm — still not `MissingTool`, so the bug cannot appear there either.
        #[cfg(unix)]
        {
            assert_eq!(err.code(), ErrorCode::Io);
            assert!(
                err.to_string().contains(&missing.display().to_string()),
                "message must name the directory, got: {err}"
            );
        }
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
