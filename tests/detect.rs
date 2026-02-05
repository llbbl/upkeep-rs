use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;

#[test]
fn detect_outputs_workspace_false_for_single_package() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src");

    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"detect-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");

    fs::write(src_dir.join("main.rs"), "fn main() {}\n").expect("write main.rs");

    let output = cargo_bin_cmd!("cargo-upkeep")
        .current_dir(root)
        .args(["detect", "--json"])
        .output()
        .expect("run detect");

    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&output).expect("parse json");
    assert_eq!(json["workspace"], false);
    assert_eq!(json["package"], "detect-test");
}

#[test]
fn detect_outputs_human_readable_summary() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("create src");

    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"detect-human\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");

    fs::write(src_dir.join("main.rs"), "fn main() {}\n").expect("write main.rs");

    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    cmd.current_dir(root)
        .arg("detect")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Workspace: false")
                .and(predicate::str::contains("Package: detect-human"))
                .and(predicate::str::contains("Dependencies: 0")),
        );
}

#[test]
fn detect_outputs_workspace_true_for_multi_member() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    fs::create_dir_all(root.join("members").join("member-a").join("src")).expect("create member-a");
    fs::create_dir_all(root.join("members").join("member-b").join("src")).expect("create member-b");

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"members/member-a\", \"members/member-b\"]\n",
    )
    .expect("write workspace Cargo.toml");

    fs::write(
        root.join("members").join("member-a").join("Cargo.toml"),
        "[package]\nname = \"member-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write member-a Cargo.toml");
    fs::write(
        root.join("members").join("member-b").join("Cargo.toml"),
        "[package]\nname = \"member-b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write member-b Cargo.toml");

    fs::write(
        root.join("members")
            .join("member-a")
            .join("src")
            .join("lib.rs"),
        "pub fn member_a() {}\n",
    )
    .expect("write member-a lib.rs");
    fs::write(
        root.join("members")
            .join("member-b")
            .join("src")
            .join("lib.rs"),
        "pub fn member_b() {}\n",
    )
    .expect("write member-b lib.rs");

    let output = cargo_bin_cmd!("cargo-upkeep")
        .current_dir(root)
        .args(["detect", "--json"])
        .output()
        .expect("run detect");

    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&output).expect("parse json");
    assert_eq!(json["workspace"], true);
    assert!(json["members"]
        .as_array()
        .is_some_and(|members| !members.is_empty()));
}

#[test]
fn detect_outputs_workspace_true_for_virtual_workspace() {
    // Create virtual workspace (no [package], only [workspace]) with single member
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    fs::create_dir_all(root.join("crates").join("single-crate").join("src"))
        .expect("create single-crate");

    // Virtual workspace: has [workspace] but no [package] section
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/single-crate\"]\n",
    )
    .expect("write workspace Cargo.toml");

    fs::write(
        root.join("crates").join("single-crate").join("Cargo.toml"),
        "[package]\nname = \"single-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write single-crate Cargo.toml");

    fs::write(
        root.join("crates")
            .join("single-crate")
            .join("src")
            .join("lib.rs"),
        "pub fn single_crate() {}\n",
    )
    .expect("write single-crate lib.rs");

    let output = cargo_bin_cmd!("cargo-upkeep")
        .current_dir(root)
        .args(["detect", "--json"])
        .output()
        .expect("run detect");

    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&output).expect("parse json");
    // Virtual workspace with 1 member should still report workspace: true
    assert_eq!(json["workspace"], true);
    assert_eq!(
        json["members"].as_array().map(|m| m.len()),
        Some(1),
        "should have exactly 1 member"
    );
}
