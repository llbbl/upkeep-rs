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
///
/// Carries the same precondition as `a_directory_with_no_project_detects_nothing`
/// since #76: the requirements walk climbs too, so this also depends on no
/// `requirements.txt` existing anywhere above the tempdir. A red here on a Linux
/// runner is worth checking against `/tmp` before hunting in the uv stub.
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

// ===== Poetry (#73) =====

/// Must match `core::analyzers::poetry::POETRY_BIN_ENV`.
///
/// This crate has no library target, so the constant cannot be imported. A
/// rename that leaves this behind is caught by
/// [`cli_python_poetry_reports_security_as_unsupported`], whose stub would stop
/// being used and let a real Poetry — or no Poetry at all — answer instead.
const POETRY_BIN_ENV: &str = "UPKEEP_POETRY_BIN";

fn poetry_available() -> bool {
    Command::new("poetry")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn poetry_fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("poetry")
            .join(name),
    )
    .expect("read poetry fixture")
}

/// A minimal Poetry project.
///
/// `poetry.lock` is written by hand and left empty on purpose: detection only
/// needs the file to exist, and the stub never reads it. A real `poetry lock`
/// would reach the network.
fn create_temp_poetry_project(name: &str) -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    fs::write(
        root.join("pyproject.toml"),
        format!(
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\
             requires-python = \">=3.10\"\ndependencies = []\n\n\
             [build-system]\nrequires = [\"poetry-core>=2.0.0\"]\n\
             build-backend = \"poetry.core.masonry.api\"\n"
        ),
    )
    .expect("write pyproject.toml");
    fs::write(root.join("poetry.lock"), "").expect("write poetry.lock");

    temp_dir
}

/// Writes a fake `poetry` that answers with the committed fixtures.
///
/// The probe response is Poetry 2.4.2's verbatim wording, so capability
/// detection resolves exactly as it would against the real binary. The two
/// listings are the two committed captures, which is what lets the whole pipeline
/// run with no Poetry installed and no network.
#[cfg(unix)]
fn write_poetry_stub(directory: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join("poetry-stub");
    fs::write(
        &path,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "Poetry (version 2.4.2)"; exit 0; fi
for arg in "$@"; do
  if [ "$arg" = "cargo-upkeep-capability-probe" ]; then
    echo "Error: Invalid output format. Supported formats are: json, text." >&2
    exit 1
  fi
done
for arg in "$@"; do
  if [ "$arg" = "--top-level" ]; then
    cat <<'TOPLEVELJSON'
{top_level}
TOPLEVELJSON
    exit 0
  fi
done
cat <<'ALLJSON'
{all}
ALLJSON
exit 0
"#,
            top_level = poetry_fixture("show-latest-top-level.json"),
            all = poetry_fixture("show-latest.json"),
        ),
    )
    .expect("write poetry stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod poetry stub");
    path
}

/// Writes a fake `poetry` predating `poetry show --format`.
///
/// The wording is Poetry 2.4.2's verbatim response to an option it does not
/// have, which is the same message an older Poetry gives for `--format` itself.
#[cfg(unix)]
fn write_legacy_poetry_stub(directory: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join("poetry-legacy-stub");
    fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "Poetry (version 1.0.10)"; exit 0; fi
echo "" >&2
echo "The option \"--format\" does not exist" >&2
exit 1
"#,
    )
    .expect("write legacy poetry stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod poetry stub");
    path
}

/// **The reason this adapter exists.** `security` under Poetry is `unsupported`,
/// never `not_installed`.
///
/// The two reasons tell a user opposite things. `not_installed` says "install the
/// scanner"; there is no scanner to install, because Poetry does not ship one and
/// `poetry check` validates the lockfile rather than scanning it. Reporting this
/// gap as `not_installed` would send someone hunting for a tool that does not
/// exist, and `unsupported` had no caller in the codebase until now.
#[cfg(unix)]
#[test]
fn cli_python_poetry_reports_security_as_unsupported() {
    let project = create_temp_poetry_project("poetry-unsupported");
    let stub = write_poetry_stub(project.path());

    let output = upkeep_cmd()
        .env(POETRY_BIN_ENV, &stub)
        .current_dir(project.path())
        .args(["python", "--json"])
        .output()
        .expect("run python");

    let json: Value = serde_json::from_slice(&output.stdout).expect("report on stdout");

    assert_eq!(
        json["manager"]["name"],
        Value::from("poetry"),
        "a poetry.lock and a poetry build backend must route to the Poetry adapter, not uv"
    );
    assert_eq!(json["manager"]["version"], Value::from("2.4.2"));
    assert_eq!(
        json["security"],
        Value::Null,
        "nobody looked, and nobody can"
    );

    let gap = json["unavailable"]
        .as_array()
        .expect("unavailable array")
        .iter()
        .find(|entry| entry["name"] == "security")
        .expect("security must be listed as unavailable");
    assert_eq!(
        gap["reason"],
        Value::from("unsupported"),
        "`not_installed` would tell the user to install a scanner Poetry has never had: {gap}"
    );
    assert!(
        gap["detail"]
            .as_str()
            .expect("detail")
            .contains("poetry check"),
        "the gap must say what the adjacent command actually does: {gap}"
    );

    // The three unavailability signals move together.
    assert_eq!(json["complete"], Value::Bool(false));
    assert!(json["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .any(|capability| capability["name"] == "security"
            && capability["measured"] == Value::Bool(false)));

    // Outdated *was* measured, so a run that cannot scan is still a real report
    // and still exits 0. Findings are not failures.
    assert_ne!(json["outdated"], Value::Null);
    assert_eq!(
        output.status.code(),
        Some(0),
        "an unsupported capability is not a failure; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// **The other reason this adapter exists.** Poetry's JSON carries no groups, no
/// extras, and no markers, so all three are `null` / `not_reported` — never `[]`
/// and never `absent`.
///
/// `[]` would claim Poetry reported the field and it was empty. It did not
/// report it at all. That is the same class of falsehood as an unmeasured
/// capability defaulting to a clean result (#10, #34), and no other adapter in
/// this crate reaches all three states at once.
#[cfg(unix)]
#[test]
fn cli_python_poetry_reports_unreported_attributes_as_null() {
    let project = create_temp_poetry_project("poetry-null-attrs");
    let stub = write_poetry_stub(project.path());

    let output = upkeep_cmd()
        .env(POETRY_BIN_ENV, &stub)
        .current_dir(project.path())
        .args(["python", "--json"])
        .output()
        .expect("run python");

    let json: Value = serde_json::from_slice(&output.stdout).expect("report on stdout");
    let packages = json["outdated"]["packages"]
        .as_array()
        .expect("packages array");
    assert!(!packages.is_empty(), "fixture premise: there are entries");

    for package in packages {
        let object = package.as_object().expect("package object");
        for field in ["groups", "extras"] {
            assert!(
                object.contains_key(field),
                "{field} must stay present, not be omitted: {package}"
            );
            assert_eq!(
                package[field],
                Value::Null,
                "{field} must be null; `[]` would claim Poetry looked and found none: {package}"
            );
        }
        assert_eq!(
            package["marker"],
            serde_json::json!({ "status": "not_reported" }),
            "`absent` would claim Poetry reports markers and this one has none: {package}"
        );
    }

    // The denominator is every package, not just the ones that are behind — the
    // reason the adapter runs `--latest` rather than `--outdated`.
    assert_eq!(json["outdated"]["checked"], Value::from(16));
    assert_eq!(json["outdated"]["outdated"], Value::from(10));

    // And scope, the one thing Poetry does not report but the adapter can derive,
    // is filled from the second invocation.
    let scope_of = |name: &str| {
        packages
            .iter()
            .find(|package| package["name"] == name)
            .unwrap_or_else(|| panic!("no entry for {name}"))["scope"]
            .clone()
    };
    assert_eq!(scope_of("flask"), Value::from("direct"));
    assert_eq!(scope_of("werkzeug"), Value::from("transitive"));
}

/// A Poetry too old for `poetry show --format` reports the gap rather than a
/// clean run — and `security` stays `unsupported` even then.
///
/// The two reasons appearing side by side in one payload is the point. An old
/// Poetry is the runner's problem and an upgrade fixes it; the missing scanner is
/// Poetry's, and no upgrade fixes it. A payload that called both `not_installed`
/// would be telling the user to do one thing about two unrelated facts.
#[cfg(unix)]
#[test]
fn cli_python_poetry_separates_an_old_poetry_from_a_missing_scanner() {
    let project = create_temp_poetry_project("poetry-legacy");
    let stub = write_legacy_poetry_stub(project.path());

    let output = upkeep_cmd()
        .env(POETRY_BIN_ENV, &stub)
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
    assert_eq!(json["manager"]["version"], Value::from("1.0.10"));
    assert_eq!(json["outdated"], Value::Null, "nobody looked");
    assert_eq!(json["security"], Value::Null, "nobody looked");

    let reason_for = |name: &str| {
        json["unavailable"]
            .as_array()
            .expect("unavailable array")
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("no gap for {name}"))
            .clone()
    };

    let outdated = reason_for("outdated");
    assert_eq!(
        outdated["reason"],
        Value::from("not_installed"),
        "an old Poetry is the runner's problem, and upgrading fixes it: {outdated}"
    );
    assert!(
        outdated["detail"]
            .as_str()
            .expect("detail")
            .contains("poetry self update"),
        "a capability gap must name how to close it: {outdated}"
    );

    let security = reason_for("security");
    assert_eq!(
        security["reason"],
        Value::from("unsupported"),
        "no Poetry upgrade adds a scanner, so this gap is a different fact: {security}"
    );
}

/// The whole pipeline against a real Poetry, when one is installed.
///
/// Skips rather than fails on a machine without Poetry, following
/// `cli_python_command_runs_when_uv_is_available`. The project has no
/// dependencies and a hand-written empty lockfile, so `poetry show` runs offline
/// and reports a genuine, measured zero — which is the claim `"outdated": null`
/// would *not* be making.
#[test]
fn cli_python_poetry_runs_when_poetry_is_available() {
    if !poetry_available() {
        eprintln!("Skipping test: poetry not installed");
        return;
    }

    let project = create_temp_poetry_project("cli-poetry");
    // An empty `poetry.lock` is enough for detection but not for `poetry show`,
    // which needs a real lockfile header. This one is written by hand rather than
    // by `poetry lock`, which would reach the network.
    fs::write(
        project.path().join("poetry.lock"),
        "package = []\n\n[metadata]\nlock-version = \"2.1\"\n\
         python-versions = \">=3.10\"\ncontent-hash = \"0\"\n",
    )
    .expect("write poetry.lock");

    let output = upkeep_cmd()
        .current_dir(project.path())
        // `poetry show` creates a virtualenv when the project has none, and with
        // this set it creates it *inside the project* as `.venv/`. The adapter
        // passes `POETRY_VIRTUALENVS_CREATE=false` to stop that, and this is the
        // only place that suppression is observable: forcing in-project
        // virtualenvs turns "inspecting a project must not modify it" into a
        // directory that either exists or does not.
        .env("POETRY_VIRTUALENVS_IN_PROJECT", "true")
        .args(["python", "--json"])
        .output()
        .expect("run python");

    assert!(
        !project.path().join(".venv").exists(),
        "reporting on a project must not write into it; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "parse poetry json ({err}); stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });

    assert_eq!(json["schema_version"], Value::from(1));
    assert_eq!(json["manager"]["name"], Value::from("poetry"));
    assert!(
        json["manager"]["version"].is_string(),
        "a real Poetry reports its version: {json}"
    );

    // A measured zero is an object with zero in it, never `null`.
    assert_eq!(json["outdated"]["outdated"], Value::from(0));
    assert_eq!(json["outdated"]["packages"], Value::Array(Vec::new()));

    // Poetry can never measure security, so this run is honestly incomplete and
    // still exits 0 without a gate.
    assert_eq!(json["security"], Value::Null);
    assert_eq!(json["complete"], Value::Bool(false));
    assert_eq!(
        output.status.code(),
        Some(0),
        "an unsupported capability is not a failure; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A `uv.lock` beside a `poetry.lock` is ambiguous, and the tie goes to `uv`.
///
/// `uv` shipped first, so rerouting an ambiguous project to Poetry would silently
/// change what an existing pipeline measures. The stub is a Poetry that would
/// answer perfectly well if it were reached — so this test fails loudly if
/// detection ever prefers Poetry here, rather than passing for the wrong reason
/// on a machine with no Poetry installed.
#[cfg(unix)]
#[test]
fn cli_python_a_project_with_both_lockfiles_stays_on_uv() {
    let project = create_temp_poetry_project("both-lockfiles");
    fs::write(project.path().join("uv.lock"), "version = 1\n").expect("write uv.lock");
    let poetry_stub = write_poetry_stub(project.path());
    let uv_stub = write_legacy_uv_stub(project.path());

    let output = upkeep_cmd()
        .env(POETRY_BIN_ENV, &poetry_stub)
        .env(UV_BIN_ENV, &uv_stub)
        .current_dir(project.path())
        .args(["python", "--json"])
        .output()
        .expect("run python");

    let json: Value = serde_json::from_slice(&output.stdout).expect("report on stdout");
    assert_eq!(
        json["manager"]["name"],
        Value::from("uv"),
        "an ambiguous project must keep the behaviour that shipped first: {json}"
    );
}

/// A project whose only manifest is a requirements file.
///
/// `contents` is the `requirements.txt`; passing a `requirements.in` alongside is
/// what separates `pip_tools` from `pip`, so each caller says which shape it
/// wants rather than inheriting one.
fn create_temp_requirements_project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    for (name, contents) in files {
        fs::write(temp_dir.path().join(name), contents).expect("write requirements fixture");
    }
    temp_dir
}

/// A requirements-file project reports an honest refusal, and the caller keeps it.
///
/// This is #76's whole point. pip and pip-tools expose no outdated command, no
/// audit command, and no query interface at all, so both capabilities are
/// `unsupported` and the run exits 1 under the already-documented
/// every-capability-unavailable rule. The report has to reach stdout *before*
/// that status, or a failing exit costs the caller the explanation for it — which
/// is the only thing this run has to give.
#[test]
fn cli_python_pip_refuses_with_the_report_still_on_stdout() {
    let project = create_temp_requirements_project(&[("requirements.txt", "requests==2.32.3\n")]);

    let output = upkeep_cmd()
        .current_dir(project.path())
        .args(["python"])
        .output()
        .expect("run python");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a run that measured nothing must not report success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Manager: pip (unknown version)"),
        "the manager is named, and no tool ran so there is no version: {stdout}"
    );
    assert!(
        stdout.contains("Coverage: incomplete (0 of 2 capabilities measured)"),
        "{stdout}"
    );
    for capability in ["outdated", "security"] {
        assert!(
            stdout.contains(&format!("{capability} not measured (unsupported)")),
            "{capability} must be reported as structurally unsupported: {stdout}"
        );
    }
    assert!(
        stdout.contains("uv or Poetry can answer it"),
        "the refusal has to point somewhere: {stdout}"
    );
    assert!(
        stdout.contains("vulnerability scanner"),
        "the security gap explains why security specifically is missing: {stdout}"
    );
}

/// The concrete improvement over the previous behaviour: a parseable payload.
///
/// Before #76 this project hit "no manager detected" and produced a JSON *error
/// object* carrying no `schema_version` at all. A consumer pinning the version
/// could not read it. Now it is a real `PythonOutput` whose content is a refusal,
/// and the three unavailability signals — `measured: false`, an `unavailable[]`
/// entry, and a `null` report — agree for both capabilities.
#[test]
fn cli_python_pip_json_carries_the_schema_version() {
    for (files, expected_manager) in [
        (&[("requirements.txt", "requests==2.32.3\n")][..], "pip"),
        (
            &[
                ("requirements.in", "requests\n"),
                ("requirements.txt", "requests==2.32.3\n"),
            ][..],
            "pip_tools",
        ),
    ] {
        let project = create_temp_requirements_project(files);

        let output = upkeep_cmd()
            .current_dir(project.path())
            .args(["python", "--json"])
            .output()
            .expect("run python");

        assert_eq!(output.status.code(), Some(1));

        let json: Value = serde_json::from_slice(&output.stdout)
            .expect("the report must still be on stdout alongside the failing status");
        assert_eq!(
            json["schema_version"],
            Value::from(1),
            "the payload a consumer can pin, where an error object used to be: {json}"
        );
        assert_eq!(json["manager"]["name"], Value::from(expected_manager));
        assert_eq!(
            json["manager"]["version"],
            Value::Null,
            "no tool is run, so there is no version to report: {json}"
        );
        assert_eq!(json["complete"], Value::Bool(false));
        assert_eq!(json["outdated"], Value::Null, "nobody looked");
        assert_eq!(json["security"], Value::Null, "nobody looked");

        let capabilities = json["capabilities"].as_array().expect("capabilities array");
        assert_eq!(capabilities.len(), 2);
        for capability in capabilities {
            assert_eq!(
                capability["measured"],
                Value::Bool(false),
                "an unmeasured capability stays listed and says so: {capability}"
            );
        }

        let unavailable = json["unavailable"].as_array().expect("unavailable array");
        assert_eq!(unavailable.len(), 2);
        for gap in unavailable {
            assert_eq!(
                gap["reason"],
                Value::from("unsupported"),
                "installing something cannot close this gap, so it is not not_installed: {gap}"
            );
            assert!(
                gap["detail"]
                    .as_str()
                    .expect("detail")
                    .contains("uv or Poetry"),
                "a structural gap must name the tool that can answer instead: {gap}"
            );
        }
        assert_ne!(
            unavailable[0]["detail"], unavailable[1]["detail"],
            "each capability explains its own absence: {json}"
        );
    }
}
