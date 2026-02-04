use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

#[test]
fn cli_without_args_shows_help() {
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Usage: cargo-upkeep"));
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
