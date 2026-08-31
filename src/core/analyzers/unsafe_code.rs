use cargo_metadata::MetadataCommand;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

use crate::core::analyzers::external_tool::{
    handle_tool_output, is_missing_subcommand, run_cargo_tool, ExternalToolConfig,
};
use crate::core::analyzers::util::describe_json_schema;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{UnsafeOutput, UnsafePackage, UnsafeSummary};

const GEIGER_CONFIG: ExternalToolConfig<'static> = ExternalToolConfig {
    tool_name: "geiger",
    install_hint: "cargo install cargo-geiger",
};

/// The `--output-format` value geiger accepts for JSON.
///
/// Named rather than inlined so the capitalization is deliberate and testable:
/// geiger's `OutputFormat` parses case-sensitively and its `main` `unwrap`s the
/// parse, so a lowercase `json` panics the tool rather than erroring cleanly.
/// See [`run_geiger_json`].
const GEIGER_JSON_FORMAT: &str = "Json";

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
        Err(parse_err) => Err(describe_geiger_failure(
            parse_err,
            output.status.success(),
            &stdout,
            &String::from_utf8_lossy(&output.stderr),
        )),
    }
}

/// Decides what to report when geiger ran but its output could not be parsed.
///
/// Split out of [`run_unsafe`] so it can be tested: reaching it through
/// `run_unsafe` would mean spawning a real cargo, so the whole branch was
/// previously unreachable from the test module.
///
/// A successful exit means the parse failure is the whole story, so the parser's
/// own error stands. A non-zero exit usually means the parse failure is a
/// symptom and the exit is the cause, so both are reported together.
///
/// **Table output escapes that rule**, because a geiger too old to emit JSON is
/// the cause regardless of how it exited. It is not a success-only condition:
/// 0.10.2 exits 1 whenever the scan also emitted warnings —
/// `WARNING: Dependency file was never scanned` is routine for build scripts
/// and proc macros — having already printed its table to stdout. Keying the
/// diagnosis off the exit status would rewrap "your geiger is too old" as a
/// generic external-command failure, with the entire ASCII table pasted into
/// the message, for what is one of the likelier ways to hit this.
///
/// The escape tests the condition itself rather than the resulting error code.
/// Those are equivalent today — a banner on stdout always fails
/// `serde_json::from_str`, so nothing downstream of it can mint a competing
/// code — but the code is a proxy, and a later `MissingTool` from elsewhere in
/// the parser would silently lose its stderr context with no test noticing.
fn describe_geiger_failure(
    parse_err: UpkeepError,
    exited_successfully: bool,
    stdout: &str,
    stderr: &str,
) -> UpkeepError {
    if exited_successfully || looks_like_geiger_table(stdout) {
        return parse_err;
    }

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

    UpkeepError::message(ErrorCode::ExternalCommand, message)
}

/// Runs `cargo geiger --output-format Json`.
///
/// **The capital `J` is required.** `OutputFormat` derives a bare
/// `EnumString` — no `ascii_case_insensitive`, no `serialize_all`, verified in
/// 0.11.0 through 0.13.0 — so `FromStr` matches the variant name exactly, and
/// geiger's `main` `unwrap`s that parse. Passing `json` made every geiger from
/// 0.11.0 on panic with `Utf8ArgumentParsingFailed`, which surfaced as
/// `cargo geiger failed: thread 'main' panicked at ...`. Geiger's own help text
/// spells the accepted set `Ascii, GitHubMarkdown, Json, Utf8, Ratio`.
///
/// There is deliberately no retry with an alternate flag spelling here, unlike
/// the `unused` analyzer. This used to fall back to `--format json` when
/// `is_unknown_flag` matched geiger's stderr; that fallback was removed as dead
/// *and* wrong, on both counts:
///
/// - **Unreachable, for two different reasons either side of 0.11.0.** Against
///   a geiger too old for `--output-format`, `pico-args` drops the unknown flag
///   silently — its `Error` enum has no unknown-argument variant and geiger
///   never calls `Arguments::finish()` (checked 0.9.1 through 0.13.0) — so the
///   run exits 0 and the `!output.status.success()` guard stays shut. From
///   0.11.0 the guard *does* open, because of the case bug above, but the
///   stderr it opens on is a Rust panic message, which matches none of
///   `UNKNOWN_FLAG_PATTERNS`. Either way the retry never fired.
/// - **Wrong even if reached.** `--format` is not an alternate spelling of
///   `--output-format`. In every released geiger from 0.9.1 through 0.13.0 it
///   is the *format string* used to print dependency names, so `--format json`
///   would have set that pattern to the literal `json` and still printed an
///   ASCII table. JSON output arrived with `--output-format` in 0.11.0; before
///   that geiger could not emit JSON at all.
///
/// A pre-0.11.0 geiger still cannot produce JSON at all: the scan runs to
/// completion — exiting 0, or 1 if it also emitted scan warnings — with table
/// output on stdout. No retry can fix that, because there is no flag to retry
/// with. It is instead detected after the fact — see
/// [`looks_like_geiger_table`] — and reported as a tool that needs upgrading
/// rather than as malformed output.
async fn run_geiger_json(workspace_root: &Path) -> Result<std::process::Output> {
    let output = run_cargo_tool(
        &["geiger", "--output-format", GEIGER_JSON_FORMAT],
        workspace_root,
        &GEIGER_CONFIG,
    )
    .await?;

    handle_tool_output(output, &GEIGER_CONFIG, |stderr| {
        is_missing_subcommand(stderr, GEIGER_CONFIG.tool_name)
    })
}

/// Start of the banner geiger prints above its human-readable table.
///
/// Verified verbatim in 0.10.2, 0.11.0 and 0.13.0. Only the prefix is pinned,
/// and not because the banner changed between versions — it did not. 0.11.0
/// *added* an `--output-format Ratio` whose banner ends `x/y=z%`, while every
/// other format, in every version including 0.10.2, ends `x/y`. upkeep never
/// asks for `Ratio`, so the full line would do; keying on the substring that is
/// invariant across formats and versions costs nothing and needs no revisiting.
const GEIGER_TABLE_BANNER: &str = "Metric output format:";

/// The oldest geiger that can emit JSON, i.e. the first with `--output-format`.
const GEIGER_MIN_JSON_VERSION: &str = "0.11.0";

/// Whether geiger printed its table instead of the JSON we asked for.
///
/// Only consulted once JSON parsing has already failed, so a real JSON report
/// can never reach it — and in 0.11.0+ the JSON path never touches the table
/// printer at all, so a genuine report cannot carry the banner either way.
///
/// Given that, table output means `--output-format` was ignored rather than
/// honoured. `pico-args` drops unknown flags in silence, so the scan runs to
/// completion, exiting 0 — or 1 if it also emitted scan warnings. A 0.11.0+
/// geiger given a value it cannot parse panics instead, leaving stdout empty,
/// so `handle_tool_output` rejects it before the parser ever runs.
///
/// Matched at line start rather than anywhere in the buffer. Nothing realistic
/// embeds this string mid-line — crate names cannot contain spaces or colons —
/// but the banner is `println!`ed in every version, so anchoring has no
/// false-negative risk and removes the need to reconstruct that argument.
fn looks_like_geiger_table(stdout: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.starts_with(GEIGER_TABLE_BANNER))
}

fn parse_geiger_output(stdout: &str) -> Result<UnsafeOutput> {
    let value: Value = serde_json::from_str(stdout).map_err(|err| {
        if looks_like_geiger_table(stdout) {
            // Not a malformed report — a scan that succeeded in the wrong
            // format. Saying "output was not valid JSON" blames the tool's
            // output for what is a version problem, and sends the user looking
            // at their project instead of their toolchain.
            return UpkeepError::message(
                ErrorCode::MissingTool,
                format!(
                    "cargo-geiger printed its table output instead of the JSON that was \
                     requested; JSON output needs cargo-geiger \
                     {GEIGER_MIN_JSON_VERSION} or later. Upgrade with `{}`.",
                    GEIGER_CONFIG.install_hint
                ),
            );
        }
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

    /// geiger's `--output-format` value is case-sensitive and must stay `Json`.
    ///
    /// `OutputFormat` derives a bare `EnumString`, so `FromStr` matches the
    /// variant name exactly, and geiger's `main` does
    /// `Args::parse_args(..).unwrap()`. A lowercase `json` therefore does not
    /// produce a clean error — it panics the tool, and the panic text reaches
    /// the user as `cargo geiger failed: thread 'main' panicked at ...`. That
    /// is what `cargo upkeep unsafe` did against every geiger from 0.11.0 on.
    ///
    /// Pinned as a literal rather than compared to the constant, so "fixing"
    /// the constant to lowercase reddens this instead of passing.
    #[test]
    fn geiger_json_format_value_is_capitalized() {
        assert_eq!(GEIGER_JSON_FORMAT, "Json");
    }

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

    /// Abridged stdout from cargo-geiger 0.10.2, which has no `--output-format`.
    ///
    /// `pico-args` drops the unknown flag in silence, so the scan runs and
    /// prints this. The banner is copied verbatim from
    /// `cargo-geiger-0.10.2/src/cli.rs`, which `println!`s it literally; the
    /// column header is the content of that file's `UNSAFE_COUNTERS_HEADER`
    /// joined with a space, checked byte-for-byte against a real capture rather
    /// than copied from a `println!`. The symbol legend is trimmed to one entry
    /// and the table to one row, so this is a representative excerpt rather
    /// than a full capture.
    const GEIGER_TABLE_STDOUT_PRE_0_11: &str = "\n\
        Metric output format: x/y\n    \
        x = unsafe code used by the build\n    \
        y = total unsafe code found in the crate\n\n\
        Symbols: \n    \
        🔒  = No `unsafe` usage found, declares #![forbid(unsafe_code)]\n\n\
        Functions  Expressions  Impls  Traits  Methods  Dependency\n\n\
        0/0        0/0          0/0    0/0     0/0      🔒  demo 0.1.0\n";

    /// A geiger too old for JSON is a toolchain problem, not a bad report.
    ///
    /// The generic "output was not valid JSON" blamed geiger's output for what
    /// is a version gap, which sends the user to inspect their project rather
    /// than upgrade their tool. `MissingTool` also puts it in `quality`'s
    /// "optional tool needs attention" bucket rather than "the analyzer failed",
    /// which is the honest bucket: nothing is wrong with the project.
    #[test]
    fn parse_geiger_output_reports_a_too_old_geiger_rather_than_bad_json() {
        let err = parse_geiger_output(GEIGER_TABLE_STDOUT_PRE_0_11).unwrap_err();

        assert_eq!(err.code(), ErrorCode::MissingTool);
        let message = err.to_string();
        assert!(
            message.contains("0.11.0"),
            "the message must name the version needed, got: {message}"
        );
        assert!(
            message.contains("cargo install cargo-geiger"),
            "the message must carry the upgrade command, got: {message}"
        );
        assert!(
            !message.contains("not valid JSON"),
            "the generic JSON complaint is what this replaces, got: {message}"
        );
    }

    /// The banner's suffix varies by *output format*, not by version: `Ratio`
    /// ends `x/y=z%` and everything else ends `x/y`. upkeep never asks for
    /// `Ratio`, so neither case is one it provokes — but the detector must not
    /// be keyed to a suffix that only some formats emit.
    #[test]
    fn geiger_table_detection_is_not_keyed_to_one_output_format_suffix() {
        assert!(looks_like_geiger_table("Metric output format: x/y\n"));
        assert!(looks_like_geiger_table("Metric output format: x/y=z%\n"));
        // Anchored at line start: the banner is always `println!`ed, so this
        // costs no true positives and rejects an embedded mention.
        assert!(!looks_like_geiger_table(
            "note: see \"Metric output format: x/y\" in the docs\n"
        ));
    }

    /// A too-old geiger stays diagnosed even when the scan also exits non-zero.
    ///
    /// 0.10.2 exits 1 whenever it emitted scan warnings — routine for build
    /// scripts and proc macros — having already printed its table. Reporting on
    /// the exit status alone would rewrap the diagnosis as a generic
    /// external-command failure and paste the whole table into the message,
    /// which is the outcome this issue exists to remove.
    #[test]
    fn geiger_failure_keeps_the_too_old_diagnosis_on_a_nonzero_exit() {
        let parse_err = parse_geiger_output(GEIGER_TABLE_STDOUT_PRE_0_11).unwrap_err();

        let described = describe_geiger_failure(
            parse_err,
            false,
            GEIGER_TABLE_STDOUT_PRE_0_11,
            "WARNING: Dependency file was never scanned: /w/demo/build.rs",
        );

        assert_eq!(described.code(), ErrorCode::MissingTool);
        let message = described.to_string();
        assert!(
            message.contains("0.11.0"),
            "the upgrade advice must survive, got: {message}"
        );
        assert!(
            !message.contains("Metric output format"),
            "the table must not be pasted into the message, got: {message}"
        );
    }

    /// An ordinary parse failure on a non-zero exit still reports both halves.
    ///
    /// Asserts the whole message rather than a pair of `contains` checks. This
    /// is the only guard on [`describe_geiger_failure`] being a faithful
    /// extraction of the branch it replaced, and a `contains` pair waves through
    /// every mutation that matters: dropping the stdout section, reordering the
    /// halves, or swapping the two adjacent `&str` parameters. Distinct
    /// sentinels are what make that last one visible — verified by swapping
    /// them, which reddens this and its sibling.
    ///
    /// It does **not** guard the call site's argument order: this calls the
    /// function directly, so transposing the arguments in `run_unsafe` leaves
    /// every test green. Reaching that wiring means spawning a real cargo, so
    /// nothing here covers it. Checked by hand at the one call site.
    ///
    /// Exact equality is safe because `UpkeepError`'s `Display` is
    /// `#[error("{message}")]` for both variants, so no source chain leaks in.
    #[test]
    fn geiger_failure_on_a_nonzero_exit_still_reports_stderr_and_parse_error() {
        let parse_err = parse_geiger_output("{").unwrap_err();
        let parse_text = parse_err.to_string();

        let described = describe_geiger_failure(parse_err, false, "STDOUT_HERE", "STDERR_HERE");

        assert_eq!(described.code(), ErrorCode::ExternalCommand);
        assert_eq!(
            described.to_string(),
            format!(
                "cargo geiger failed: stderr: STDERR_HERE; parse error: {parse_text} \
                 stdout: STDOUT_HERE"
            ),
        );
    }

    /// Empty streams are labelled, not left as gaps after their label.
    ///
    /// Whitespace-only stderr becomes `<empty>` and whitespace-only stdout is
    /// omitted entirely rather than appended as a bare `stdout: `. Nothing else
    /// in the crate exercises either branch.
    #[test]
    fn geiger_failure_labels_empty_streams_rather_than_leaving_gaps() {
        let parse_err = parse_geiger_output("{").unwrap_err();
        let parse_text = parse_err.to_string();

        let described = describe_geiger_failure(parse_err, false, "   \n", "  \n ");

        assert_eq!(
            described.to_string(),
            format!("cargo geiger failed: stderr: <empty>; parse error: {parse_text}"),
        );
    }

    /// Malformed JSON is still malformed JSON.
    ///
    /// Without this, widening the detector to something like "contains
    /// `format`" would silently relabel every parse failure as an outdated
    /// tool, which is a worse error than the one being replaced.
    #[test]
    fn parse_geiger_output_still_reports_genuinely_broken_json() {
        let err = parse_geiger_output("{\"packages\": [").unwrap_err();

        assert_eq!(err.code(), ErrorCode::InvalidData);
        assert!(err.to_string().contains("not valid JSON"));
        assert!(!looks_like_geiger_table("{\"packages\": ["));
    }
}
