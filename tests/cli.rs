use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use std::process::Command;

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
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
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
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn cli_help_flag_works() {
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
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
        let mut cmd = cargo_bin_cmd!("cargo-upkeep");
        cmd.args(["upkeep", subcommand, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage"));
    }
}

#[test]
fn cli_detect_command_runs() {
    let temp_dir = create_temp_crate("cli-detect");
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.current_dir(temp_dir.path())
        .args(["detect", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"package\": \"cli-detect\""));
}

#[test]
fn cli_deps_command_runs() {
    let temp_dir = create_temp_crate("cli-deps");
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.current_dir(temp_dir.path())
        .args(["deps", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"total\": 0"));
}

#[test]
fn cli_tree_command_runs() {
    let temp_dir = create_temp_crate("cli-tree");
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.current_dir(temp_dir.path())
        .args(["tree", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"root\""));
}

#[test]
fn cli_quality_command_runs() {
    let temp_dir = create_temp_crate("cli-quality");
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.current_dir(temp_dir.path())
        .args(["quality", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"grade\""));
}

#[test]
fn cli_audit_command_works_without_lockfile() {
    // rustsec 0.32+ handles missing lockfiles gracefully
    let temp_dir = create_temp_crate("cli-audit");
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.current_dir(temp_dir.path())
        .args(["audit", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"vulnerabilities\""));
}

#[test]
fn cli_unused_command_runs_when_tool_available() {
    if !cargo_subcommand_available("machete") {
        eprintln!("Skipping test: cargo-machete not installed");
        return;
    }

    let temp_dir = create_temp_crate("cli-unused");
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
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
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.current_dir(temp_dir.path())
        .args(["unsafe-code", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"summary\""));
}
