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

/// Advisory ID carried only by the fixture database.
const FIXTURE_ADVISORY_ID: &str = "RUSTSEC-2099-0001";

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
        .stdout(predicate::str::contains("\"vulnerabilities\""));
}

/// `UPKEEP_ADVISORY_DB` really is the database that gets read.
///
/// The fixture carries a fabricated advisory against `serde` that no real
/// database contains, so this fails if the binary ignores the variable and
/// fetches `~/.cargo/advisory-db` instead — which is what every other test in
/// this file relies on not happening.
///
/// It runs against this repository because the advisory has to match a resolved
/// dependency, and the temp crates the other tests build have no dependencies.
/// No network: `cargo metadata` and the committed `Cargo.lock` are enough.
#[test]
fn cli_audit_uses_local_advisory_db_from_env() {
    let output = upkeep_cmd()
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["audit", "--json"])
        .output()
        .expect("run audit");
    assert!(
        output.status.success(),
        "audit failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse audit json");
    let found = json["vulnerabilities"]
        .as_array()
        .expect("vulnerabilities array")
        .iter()
        .find(|vuln| vuln["id"] == FIXTURE_ADVISORY_ID)
        .unwrap_or_else(|| {
            panic!(
                "{FIXTURE_ADVISORY_ID} missing; the fixture database was not the one read: {json}"
            )
        });
    assert_eq!(found["package"], "serde");
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
