use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Create a two-member workspace where each member pins `rand` to a different
/// requirement.
///
/// Cargo unifies semver-compatible requirements, so semver-INCOMPATIBLE
/// requirements are the only way to make cargo genuinely resolve two versions of
/// one crate name inside a single workspace. `0.6` and `0.7` are both long
/// superseded, so both stay outdated regardless of what crates.io publishes next.
fn create_temp_workspace(core_req: &str, cli_req: &str) -> TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"members/core-lib\", \"members/cli-app\"]\n",
    )
    .expect("write workspace Cargo.toml");

    write_member(
        root,
        "core-lib",
        core_req,
        "src/lib.rs",
        "pub fn stub() {}\n",
    );
    write_member(root, "cli-app", cli_req, "src/main.rs", "fn main() {}\n");

    temp_dir
}

fn write_member(root: &Path, name: &str, rand_req: &str, target: &str, body: &str) {
    let member_dir = root.join("members").join(name);
    let target_path = member_dir.join(target);
    fs::create_dir_all(target_path.parent().expect("target parent")).expect("create member dir");

    fs::write(
        member_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nrand = \"{rand_req}\"\n"
        ),
    )
    .expect("write member Cargo.toml");

    fs::write(&target_path, body).expect("write member source");
}

fn create_temp_crate_with_dep(name: &str, rand_req: &str) -> TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();
    fs::create_dir_all(root.join("src")).expect("create src");

    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nrand = \"{rand_req}\"\n"
        ),
    )
    .expect("write Cargo.toml");

    fs::write(root.join("src").join("main.rs"), "fn main() {}\n").expect("write main.rs");

    temp_dir
}

/// Skip a network-dependent test, unless the environment forbids skipping.
///
/// These tests degrade to a no-op offline, which means they also pass vacuously in
/// any environment that quietly loses network access — including CI. Setting
/// `UPKEEP_REQUIRE_NETWORK_TESTS` turns every skip path into a failure, so CI
/// asserts that these assertions actually ran.
fn skip_or_fail(reason: &str) -> Option<Value> {
    if std::env::var_os("UPKEEP_REQUIRE_NETWORK_TESTS").is_some() {
        panic!("network-dependent test cannot be skipped: {reason}");
    }
    eprintln!("Skipping test: {reason}");
    None
}

/// Run `deps --json` in `dir`.
///
/// Returns `None` when the environment cannot support the run: resolving these
/// fixtures needs the crates.io index, and reporting `packages` at all needs the
/// crates.io API. Both are skipped rather than failed, matching how the rest of
/// this suite treats missing external tooling — see [`skip_or_fail`] for how CI
/// opts out of that leniency.
fn run_deps_json(dir: &Path) -> Option<Value> {
    let output = cargo_bin_cmd!("cargo-upkeep")
        .current_dir(dir)
        .args(["deps", "--json"])
        .output()
        .expect("run deps");

    if !output.status.success() {
        return skip_or_fail(&format!(
            "`deps` failed (crates.io index likely unreachable); stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse deps json");

    let warnings = json["warnings"].as_array().expect("warnings array");
    if warnings
        .iter()
        .any(|warning| warning.as_str().is_some_and(|w| w.contains("failed to")))
    {
        return skip_or_fail(&format!(
            "crates.io API unavailable; warnings: {warnings:?}"
        ));
    }

    Some(json)
}

fn rand_entries(json: &Value) -> Vec<&Value> {
    json["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .filter(|package| package["name"] == "rand")
        .collect()
}

#[test]
fn deps_reports_one_entry_per_resolved_version_in_a_workspace() {
    let temp_dir = create_temp_workspace("0.6", "0.7");
    let Some(json) = run_deps_json(temp_dir.path()) else {
        return;
    };

    let entries = rand_entries(&json);
    assert_eq!(
        entries.len(),
        2,
        "expected one entry per resolved rand version; got {json:#}"
    );

    // Deterministic ordering: sorted by (name, current).
    assert_eq!(entries[0]["current"], "0.6.5");
    assert_eq!(entries[0]["required"], "^0.6");
    assert_eq!(entries[0]["members"], serde_json::json!(["core-lib"]));

    assert_eq!(entries[1]["current"], "0.7.3");
    assert_eq!(entries[1]["required"], "^0.7");
    assert_eq!(entries[1]["members"], serde_json::json!(["cli-app"]));

    assert_eq!(json["workspace"], true);
}

#[test]
fn deps_merges_members_that_agree_on_a_version() {
    let temp_dir = create_temp_workspace("0.6", "0.6");
    let Some(json) = run_deps_json(temp_dir.path()) else {
        return;
    };

    let entries = rand_entries(&json);
    assert_eq!(
        entries.len(),
        1,
        "members agreeing on a version must not split; got {json:#}"
    );
    assert_eq!(entries[0]["current"], "0.6.5");
    // Sorted, deduplicated, and covering both declaring members.
    assert_eq!(
        entries[0]["members"],
        serde_json::json!(["cli-app", "core-lib"])
    );
}

#[test]
fn deps_reports_one_entry_per_dependency_for_a_single_crate() {
    let temp_dir = create_temp_crate_with_dep("solo-crate", "0.6");
    let Some(json) = run_deps_json(temp_dir.path()) else {
        return;
    };

    let entries = rand_entries(&json);
    assert_eq!(
        entries.len(),
        1,
        "a single-crate project must emit one entry per dependency; got {json:#}"
    );
    assert_eq!(entries[0]["current"], "0.6.5");
    // `members` is always populated; for a single crate it is the crate's own name.
    assert_eq!(entries[0]["members"], serde_json::json!(["solo-crate"]));
    assert_eq!(json["workspace"], false);
}

#[test]
fn deps_reports_empty_members_free_output_for_a_dependency_free_crate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();
    fs::create_dir_all(root.join("src")).expect("create src");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"no-deps\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");
    fs::write(root.join("src").join("main.rs"), "fn main() {}\n").expect("write main.rs");

    let Some(json) = run_deps_json(root) else {
        return;
    };

    assert_eq!(json["total"], 0);
    assert_eq!(json["packages"], serde_json::json!([]));
}
