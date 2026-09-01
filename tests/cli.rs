use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Must match `core::analyzers::audit::ADVISORY_DB_ENV`.
///
/// This crate has no library target, so the constant cannot be imported. A
/// rename that leaves this behind is caught by
/// [`cli_audit_uses_local_advisory_db_from_env`], which fails as soon as the
/// binary stops honouring the variable.
const ADVISORY_DB_ENV: &str = "UPKEEP_ADVISORY_DB";

/// The committed advisory-database fixture — see its README.
///
/// Panics rather than falling back to the real database if the fixture is
/// missing: a silent fallback would put the tests back on `~/.cargo/advisory-db`,
/// which is the shared mutable state this exists to avoid.
fn advisory_db_fixture() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("advisory-db");
    assert!(
        path.join("crates").is_dir(),
        "advisory-db fixture missing at {}; tests must not fall back to ~/.cargo/advisory-db",
        path.display()
    );
    path
}

/// Every `cargo-upkeep` invocation in this file, so no test can reach the shared
/// advisory database. Commands that never audit are built the same way on
/// purpose — one that grows an audit path later inherits the isolation.
fn upkeep_cmd() -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.env(ADVISORY_DB_ENV, advisory_db_fixture());
    cmd
}

fn create_temp_crate(name: &str) -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src");

    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            name
        ),
    )
    .expect("write Cargo.toml");

    fs::write(src_dir.join("main.rs"), "fn main() {}\n").expect("write main.rs");

    temp_dir
}

fn cargo_subcommand_available(name: &str) -> bool {
    Command::new("cargo")
        .args([name, "--version"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Must match `core::analyzers::uv::UV_BIN_ENV`.
///
/// This crate has no library target, so the constant cannot be imported. A rename
/// that leaves this behind is caught by
/// [`cli_python_reports_capability_gaps_on_an_old_uv`], whose stub would stop
/// being used and let a real `uv` answer instead.
const UV_BIN_ENV: &str = "UPKEEP_UV_BIN";

fn uv_available() -> bool {
    Command::new("uv")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// A minimal `uv` project whose lockfile is committed by hand.
///
/// Deliberately not built with `uv lock`: that reaches the network, and a
/// lockfile with no dependencies in it is enough for `uv tree --frozen` and
/// `uv audit --frozen` to run offline.
fn create_temp_uv_project(name: &str) -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    fs::write(
        root.join("pyproject.toml"),
        format!(
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\n\
             dependencies = []\n"
        ),
    )
    .expect("write pyproject.toml");
    fs::write(
        root.join("uv.lock"),
        format!(
            "version = 1\nrevision = 1\nrequires-python = \">=3.9\"\n\n\
             [[package]]\nname = \"{name}\"\nversion = \"0.1.0\"\n\
             source = {{ virtual = \".\" }}\n"
        ),
    )
    .expect("write uv.lock");

    temp_dir
}

/// Writes a fake `uv` that answers `--version` and rejects everything else the
/// way `uv 0.7.11` does.
///
/// The wordings are that release's verbatim stderr, captured by running the real
/// binary. This is the profile #72 was filed for — a `uv` predating the `audit`
/// subcommand entirely — and it is unreachable on a machine with a current `uv`
/// unless the binary can be pointed elsewhere.
#[cfg(unix)]
fn write_legacy_uv_stub(directory: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join("uv-0.7.11-stub");
    fs::write(
        &path,
        r#"#!/bin/sh
case "$1" in
  --version) echo "uv 0.7.11 (90a4416ab 2025-06-04)"; exit 0 ;;
  audit)
    echo "error: unrecognized subcommand 'audit'" >&2
    echo "" >&2
    echo "Usage: uv [OPTIONS] <COMMAND>" >&2
    exit 2 ;;
  tree)
    echo "error: unexpected argument '--format' found" >&2
    echo "" >&2
    echo "Usage: uv tree [OPTIONS]" >&2
    exit 2 ;;
esac
exit 2
"#,
    )
    .expect("write uv stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod uv stub");
    path
}

#[test]
fn cli_without_args_shows_help() {
    let mut cmd = upkeep_cmd();
    let output = cmd.output().expect("run cargo-upkeep");
    // arg_required_else_help should always exit with code 2
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit status 2 (arg_required_else_help); status: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Usage: cargo-upkeep"),
        "expected help output in stderr; stderr: {stderr}"
    );
}

#[test]
fn cli_version_flag_works() {
    let mut cmd = upkeep_cmd();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn cli_help_flag_works() {
    let mut cmd = upkeep_cmd();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Unified Rust project maintenance"));
}

#[test]
fn cli_subcommands_have_help() {
    let subcommands = [
        "detect",
        "audit",
        "deps",
        "quality",
        "unused",
        "unsafe-code",
        "tree",
        "python",
    ];

    for subcommand in subcommands {
        let mut cmd = upkeep_cmd();
        cmd.args(["upkeep", subcommand, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage"));
    }
}

#[test]
fn cli_detect_command_runs() {
    let temp_dir = create_temp_crate("cli-detect");
    let mut cmd = upkeep_cmd();
    cmd.current_dir(temp_dir.path())
        .args(["detect", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"package\": \"cli-detect\""));
}

#[test]
fn cli_deps_command_runs() {
    let temp_dir = create_temp_crate("cli-deps");
    let mut cmd = upkeep_cmd();
    cmd.current_dir(temp_dir.path())
        .args(["deps", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"total\": 0"));
}

#[test]
fn cli_tree_command_runs() {
    let temp_dir = create_temp_crate("cli-tree");
    let mut cmd = upkeep_cmd();
    cmd.current_dir(temp_dir.path())
        .args(["tree", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"root\""));
}

/// Also asserts that the audit ran.
///
/// `quality` absorbs an audit failure into an unavailable metric and still exits
/// zero, so asserting only on `grade` passes just as happily when the advisory
/// database could not be read at all. That is the asymmetry that let this
/// command hide a broken audit while `cli_audit_command_works_without_lockfile`
/// failed on the same fault.
#[test]
fn cli_quality_command_runs() {
    let temp_dir = create_temp_crate("cli-quality");
    let output = upkeep_cmd()
        .current_dir(temp_dir.path())
        .args(["quality", "--json"])
        .output()
        .expect("run quality");
    assert!(
        output.status.success(),
        "quality failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse quality json");
    assert!(json["grade"].is_string(), "expected a grade; got {json}");

    let security_unavailable = json["unavailable"]
        .as_array()
        .expect("unavailable array")
        .iter()
        .find(|metric| metric["name"] == "Security");
    assert!(
        security_unavailable.is_none(),
        "security metric was not measured: {}",
        security_unavailable.expect("checked above")
    );
}

#[test]
fn cli_audit_command_works_without_lockfile() {
    // rustsec 0.32+ handles missing lockfiles gracefully
    let temp_dir = create_temp_crate("cli-audit");
    let mut cmd = upkeep_cmd();
    cmd.current_dir(temp_dir.path())
        .args(["audit", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"vulnerabilities\""))
        .stdout(predicate::str::contains("\"warnings\""));
}

/// `UPKEEP_ADVISORY_DB` is wired through the CLI to the analyzer.
///
/// A deliberately missing path must be named in the structured stderr error.
/// This directly catches the binary ignoring the variable without requiring a
/// successful standalone audit, which now also performs a live crates.io yanked
/// check. Fixture contents and informational mapping are covered in analyzer
/// tests without any registry access.
#[test]
fn cli_audit_honors_local_advisory_db_env() {
    let temp_dir = create_temp_crate("cli-audit-local-db");
    let missing = temp_dir.path().join("missing-advisory-db");
    let output = upkeep_cmd()
        .env(ADVISORY_DB_ENV, &missing)
        .current_dir(temp_dir.path())
        .args(["audit", "--json"])
        .output()
        .expect("run audit");
    assert!(
        !output.status.success(),
        "audit unexpectedly succeeded; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to open RustSec advisory database")
            && stderr.contains(missing.to_string_lossy().as_ref()),
        "expected local advisory path in stderr; stderr: {stderr}"
    );
}

#[test]
fn cli_unused_command_runs_when_tool_available() {
    if !cargo_subcommand_available("machete") {
        eprintln!("Skipping test: cargo-machete not installed");
        return;
    }

    let temp_dir = create_temp_crate("cli-unused");
    let mut cmd = upkeep_cmd();
    cmd.current_dir(temp_dir.path())
        .args(["unused", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"unused\""));
}

#[test]
fn cli_unsafe_command_runs_when_tool_available() {
    if !cargo_subcommand_available("geiger") {
        eprintln!("Skipping test: cargo-geiger not installed");
        return;
    }

    let temp_dir = create_temp_crate("cli-unsafe");
    let mut cmd = upkeep_cmd();
    cmd.current_dir(temp_dir.path())
        .args(["unsafe-code", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"summary\""));
}

/// #34's whole point is that the exit status is *added* to the report, not
/// substituted for it: a CI author who does not parse JSON needs the status,
/// and still needs the analysis that explains it. `enforce_exit_policy` is a
/// pure function so it can be unit-tested, which means the ordering of the
/// print against the policy is only observable from out here.
#[test]
fn cli_quality_exits_nonzero_and_still_prints_the_report_when_nothing_is_measured() {
    // Deliberately not a cargo project, so every metric is unavailable.
    let empty = tempfile::tempdir().expect("temp dir");
    let output = upkeep_cmd()
        .current_dir(empty.path())
        .args(["quality", "--json"])
        .output()
        .expect("run quality");

    assert_eq!(
        output.status.code(),
        Some(1),
        "nothing was measured, so this must not report success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout)
        .expect("the full report must still be on stdout alongside the failing status");
    assert_eq!(json["score"], Value::Null, "premise: nothing was measured");
    assert_eq!(
        json["breakdown"].as_array().expect("breakdown").len(),
        6,
        "the report must be complete, not truncated because the command failed"
    );

    let err: Value = serde_json::from_slice(&output.stderr).expect("error object on stderr");
    assert_eq!(err["code"], "incomplete_analysis");
}

/// `Command` and `UpkeepCommand` are separate clap enums that `main` maps
/// between by hand, so a flag can parse correctly under one invocation form and
/// be silently dropped under the other. The parse-level test covers the two
/// enums; only running the binary covers the mapping between them.
///
/// Asserted against the run's own observed `complete` rather than a fixed exit
/// status, so it holds whether or not `cargo-machete` and `cargo-geiger` happen
/// to be installed on this machine.
#[test]
fn cli_quality_require_complete_is_plumbed_through_both_invocation_forms() {
    let temp_dir = create_temp_crate("cli-quality-require-complete");

    let baseline = upkeep_cmd()
        .current_dir(temp_dir.path())
        .args(["quality", "--json"])
        .output()
        .expect("run quality");
    assert!(
        baseline.status.success(),
        "the default path must stay backward compatible; stderr: {}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    let json: Value = serde_json::from_slice(&baseline.stdout).expect("parse quality json");
    let complete = json["complete"].as_bool().expect("complete");

    for form in [
        &["quality", "--require-complete", "--json"][..],
        &["upkeep", "quality", "--require-complete", "--json"][..],
    ] {
        let output = upkeep_cmd()
            .current_dir(temp_dir.path())
            .args(form)
            .output()
            .expect("run quality");
        assert_eq!(
            output.status.success(),
            complete,
            "invocation form {form:?} ignored --require-complete; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `python` is reachable, and its help reads the same under both forms.
///
/// `cli_subcommands_have_help` already runs the `upkeep python` form; this adds
/// the direct one and asserts the two are the same text, which is only true if
/// both enums carry the same `PythonArgs`.
#[test]
fn cli_python_help_matches_under_both_invocation_forms() {
    let direct = upkeep_cmd()
        .args(["python", "--help"])
        .output()
        .expect("run python --help");
    let nested = upkeep_cmd()
        .args(["upkeep", "python", "--help"])
        .output()
        .expect("run upkeep python --help");

    assert!(direct.status.success() && nested.status.success());
    let direct = String::from_utf8_lossy(&direct.stdout);
    let nested = String::from_utf8_lossy(&nested.stdout);

    for text in [&direct, &nested] {
        assert!(
            text.contains("--require-complete") && text.contains("--fail-on-vulnerability"),
            "both gates must be documented; got: {text}"
        );
    }
    // The usage line names the invocation, so only the body is compared.
    let body = |text: &str| {
        text.split_once("Options:")
            .map(|(_, options)| options.to_string())
            .unwrap_or_else(|| text.to_string())
    };
    assert_eq!(
        body(&direct),
        body(&nested),
        "the two invocation forms must document the same flags"
    );
}

/// A directory with no Python project is a no-report failure, not an empty
/// report.
///
/// One of the two conditions `docs/python-schema.md` says fail without any flag.
/// It emits an error object rather than a `PythonOutput`, so it deliberately
/// carries no `schema_version`.
///
/// `uv` is stubbed rather than left to the machine. Otherwise, on a runner
/// without `uv` installed, this passes on the *other* no-manager condition — uv
/// not on PATH — and stops testing project detection at all.
#[cfg(unix)]
#[test]
fn cli_python_without_a_project_fails_with_an_error_object() {
    let tools = tempfile::tempdir().expect("temp dir");
    let stub = write_legacy_uv_stub(tools.path());
    let empty = tempfile::tempdir().expect("temp dir");

    let output = upkeep_cmd()
        .env(UV_BIN_ENV, &stub)
        .current_dir(empty.path())
        .args(["python", "--json"])
        .output()
        .expect("run python");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "no manager means no report at all; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let err: Value = serde_json::from_slice(&output.stderr).expect("error object on stderr");
    let message = err["message"].as_str().expect("message");
    assert!(
        message.contains("no supported Python manager could be detected")
            && message.contains("pyproject.toml"),
        "the failure must be the missing project, not the stubbed uv: {err}"
    );
}

/// A `uv` too old for either capability reports the gaps rather than a clean run.
///
/// This is #72's whole point, and the failure it guards against is the #10/#34
/// one in a new place: "no vulnerabilities" out of a scanner that does not exist.
/// Both reports must be `null`, both capabilities `measured: false`, and the run
/// must exit nonzero because there is nothing left for `complete` to qualify.
#[cfg(unix)]
#[test]
fn cli_python_reports_capability_gaps_on_an_old_uv() {
    let project = create_temp_uv_project("legacy-uv-demo");
    let stub = write_legacy_uv_stub(project.path());

    let output = upkeep_cmd()
        .env(UV_BIN_ENV, &stub)
        .current_dir(project.path())
        .args(["python", "--json"])
        .output()
        .expect("run python");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a run that measured nothing must not report success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout)
        .expect("the report must still be on stdout alongside the failing status");
    assert_eq!(json["manager"]["version"], Value::from("0.7.11"));
    assert_eq!(json["complete"], Value::Bool(false));
    assert_eq!(json["outdated"], Value::Null, "nobody looked");
    assert_eq!(json["security"], Value::Null, "nobody looked");

    let unavailable = json["unavailable"].as_array().expect("unavailable array");
    assert_eq!(unavailable.len(), 2);
    for gap in unavailable {
        assert_eq!(
            gap["reason"],
            Value::from("not_installed"),
            "an old uv is the runner's problem, not the project's: {gap}"
        );
        assert!(
            gap["detail"]
                .as_str()
                .expect("detail")
                .contains("uv self update"),
            "a capability gap must name how to close it: {gap}"
        );
    }
    for capability in json["capabilities"].as_array().expect("capabilities") {
        assert_eq!(capability["measured"], Value::Bool(false));
    }
}

/// The gates reach the handler under both invocation forms.
///
/// The equivalent of `cli_quality_require_complete_is_plumbed_through_both_forms`
/// and it exists for the same reason: `Command` and `UpkeepCommand` are separate
/// clap enums that `main` maps between by hand, so a variant wired into only one
/// of them compiles and passes every parse-level test while silently breaking one
/// invocation form (#34).
///
/// Driven against the old-`uv` stub rather than a real `uv` so it needs no
/// network and no installed toolchain, and asserts on the *reason* for the exit
/// rather than the status alone — the status is 1 either way here, which would
/// make a dropped flag invisible.
#[cfg(unix)]
#[test]
fn cli_python_flags_are_plumbed_through_both_invocation_forms() {
    let project = create_temp_uv_project("python-both-forms");
    let stub = write_legacy_uv_stub(project.path());

    for form in [
        &["python", "--require-complete=security", "--json"][..],
        &["upkeep", "python", "--require-complete=security", "--json"][..],
    ] {
        let output = upkeep_cmd()
            .env(UV_BIN_ENV, &stub)
            .current_dir(project.path())
            .args(form)
            .output()
            .expect("run python");

        assert_eq!(
            output.status.code(),
            Some(1),
            "invocation form {form:?} must fail; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).expect("report on stdout");
        assert_eq!(json["schema_version"], Value::from(1));
    }

    // And a threshold that cannot fire still parses and reaches the handler under
    // both forms, which is what a dropped mapping would break silently.
    for form in [
        &["python", "--fail-on-vulnerability", "critical"][..],
        &["upkeep", "python", "--fail-on-vulnerability", "critical"][..],
    ] {
        let output = upkeep_cmd()
            .env(UV_BIN_ENV, &stub)
            .current_dir(project.path())
            .args(form)
            .output()
            .expect("run python");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("measured nothing"),
            "invocation form {form:?} did not reach the handler; stderr: {stderr}"
        );
    }
}

/// The whole pipeline against a real `uv`, when one is installed.
///
/// Skips rather than fails on a machine without `uv`, following
/// `cli_unused_command_runs_when_tool_available`. The project has no
/// dependencies, so `uv tree` and `uv audit` both run offline and both report a
/// genuine, measured zero — which is the claim `"outdated": null` would *not*
/// be making.
#[test]
fn cli_python_command_runs_when_uv_is_available() {
    if !uv_available() {
        eprintln!("Skipping test: uv not installed");
        return;
    }

    let project = create_temp_uv_project("cli-python");
    let output = upkeep_cmd()
        .current_dir(project.path())
        .args(["python", "--json"])
        .output()
        .expect("run python");

    assert!(
        output.status.success(),
        "findings are not failures and there are none here; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse python json");
    assert_eq!(json["schema_version"], Value::from(1));
    assert_eq!(json["manager"]["name"], Value::from("uv"));
    assert_eq!(json["complete"], Value::Bool(true));
    assert_eq!(json["unavailable"], Value::Array(Vec::new()));

    // A measured zero is an object with zero in it, never `null`.
    assert_eq!(json["outdated"]["outdated"], Value::from(0));
    assert_eq!(json["security"]["summary"]["total"], Value::from(0));
    assert!(json["security"]["findings"].is_array());
}

/// Writes a fake `uv` that reproduces the case the adapter's exit handling turns
/// on: `uv audit` **exits 1 when it finds vulnerabilities**, with the report on
/// stdout. That is a successful run with findings, not a failure.
///
/// The real binary cannot be used for this — a passing project has no findings,
/// so it exits 0 and the load-bearing branch is never taken. Without this, adding
/// a `status.success()` check to `Uv::security` leaves the whole suite green
/// while every vulnerability in a real project silently disappears.
#[cfg(unix)]
fn write_finding_uv_stub(directory: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let audit = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("uv")
            .join("audit.json"),
    )
    .expect("read audit fixture");

    let path = directory.join("uv-finding-stub");
    fs::write(
        &path,
        format!(
            r#"#!/bin/sh
# Probe responses are the real uv 0.12.8 wordings, so capability detection
# resolves to Available exactly as it would against the real binary.
if [ "$1" = "--version" ]; then echo "uv 0.12.8 (stub)"; exit 0; fi
for arg in "$@"; do
  if [ "$arg" = "cargo-upkeep-capability-probe" ]; then
    case "$1" in
      audit) echo "error: invalid value 'cargo-upkeep-capability-probe' for '--output-format <OUTPUT_FORMAT>'" >&2
             echo "  [possible values: text, json, sarif]" >&2 ;;
      tree)  echo "error: invalid value 'cargo-upkeep-capability-probe' for '--format <FORMAT>'" >&2
             echo "  [possible values: text, json]" >&2 ;;
    esac
    exit 2
  fi
done
if [ "$1" = "audit" ]; then
  cat <<'AUDITJSON'
{audit}
AUDITJSON
  # This is the whole point: findings mean exit 1.
  exit 1
fi
if [ "$1" = "tree" ]; then echo '{{"schema":{{"version":"preview"}},"roots":[],"resolution":{{}}}}'; exit 0; fi
exit 2
"#
        ),
    )
    .expect("write uv finding stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod uv stub");
    path
}

/// `uv audit` exits 1 when it finds vulnerabilities. Treating that as a failed
/// run would report `security: null` with an `unavailable` entry — a project
/// full of CVEs rendered as "we couldn't look", which exits 0 without a gate.
/// That is the #10/#34 defaulted-to-healthy bug in its most dangerous direction.
#[cfg(unix)]
#[test]
fn cli_python_reports_findings_when_uv_audit_exits_nonzero() {
    let project = create_temp_uv_project("uv-findings-demo");
    let stub = write_finding_uv_stub(project.path());

    let output = upkeep_cmd()
        .env(UV_BIN_ENV, &stub)
        .current_dir(project.path())
        .args(["python", "--json"])
        .output()
        .expect("run python");

    let json: Value = serde_json::from_slice(&output.stdout)
        .expect("the report must be on stdout; stderr and status are separate concerns");

    assert_ne!(
        json["security"],
        Value::Null,
        "uv answered with findings on stdout; a nonzero status does not mean it failed to look"
    );
    assert_eq!(
        json["security"]["summary"]["total"],
        Value::from(6),
        "every finding uv reported must survive normalization"
    );
    let unavailable = json["unavailable"].as_array().expect("unavailable array");
    assert!(
        !unavailable.iter().any(|entry| entry["name"] == "security"),
        "security ran and answered, so it must not be listed as unavailable: {unavailable:?}"
    );

    // Findings are not failures: without a gate this still exits 0.
    assert_eq!(
        output.status.code(),
        Some(0),
        "findings alone must not fail the run; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
