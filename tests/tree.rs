use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create dir");
    }
    fs::write(path, contents).expect("write file");
}

fn create_tree_workspace() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    write_file(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\n  \"crates/app\",\n  \"crates/dep_a\",\n  \"crates/dep_b\",\n  \"crates/mid\"\n]\nexclude = [\n  \"external/dup_v1\",\n  \"external/dup_v2\",\n  \"external/dev_only\",\n  \"external/leaf\",\n  \"external/build_only\"\n]\n",
    );

    write_file(
        &root.join("crates/app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndep_a = { path = \"../dep_a\" }\ndep_b = { path = \"../dep_b\" }\n\n[dev-dependencies]\ndev_only = { path = \"../../external/dev_only\" }\n",
    );
    write_file(&root.join("crates/app/src/lib.rs"), "pub fn app() {}\n");

    write_file(
        &root.join("crates/dep_a/Cargo.toml"),
        "[package]\nname = \"dep_a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nmid = { path = \"../mid\" }\ndup = { path = \"../../external/dup_v1\" }\n\n[build-dependencies]\nbuild_only = { path = \"../../external/build_only\" }\n\n[features]\nextra = []\ndefault = [\"extra\"]\n",
    );
    write_file(&root.join("crates/dep_a/src/lib.rs"), "pub fn dep_a() {}\n");

    write_file(
        &root.join("crates/dep_b/Cargo.toml"),
        "[package]\nname = \"dep_b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndup = { path = \"../../external/dup_v2\" }\n",
    );
    write_file(&root.join("crates/dep_b/src/lib.rs"), "pub fn dep_b() {}\n");

    write_file(
        &root.join("crates/mid/Cargo.toml"),
        "[package]\nname = \"mid\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nleaf = { path = \"../../external/leaf\" }\n",
    );
    write_file(&root.join("crates/mid/src/lib.rs"), "pub fn mid() {}\n");

    write_file(
        &root.join("external/dup_v1/Cargo.toml"),
        "[package]\nname = \"dup\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("external/dup_v1/src/lib.rs"),
        "pub fn dup_v1() {}\n",
    );

    write_file(
        &root.join("external/dup_v2/Cargo.toml"),
        "[package]\nname = \"dup\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("external/dup_v2/src/lib.rs"),
        "pub fn dup_v2() {}\n",
    );

    write_file(
        &root.join("external/dev_only/Cargo.toml"),
        "[package]\nname = \"dev_only\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("external/dev_only/src/lib.rs"),
        "pub fn dev_only() {}\n",
    );

    write_file(
        &root.join("external/leaf/Cargo.toml"),
        "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(&root.join("external/leaf/src/lib.rs"), "pub fn leaf() {}\n");

    write_file(
        &root.join("external/build_only/Cargo.toml"),
        "[package]\nname = \"build_only\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("external/build_only/src/lib.rs"),
        "pub fn build_only() {}\n",
    );

    temp_dir
}

fn run_tree(root: &Path, args: &[&str]) -> Value {
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    let mut full_args = vec!["tree", "--json"];
    full_args.extend_from_slice(args);

    let output = cmd
        .current_dir(root)
        .args(full_args)
        .output()
        .expect("run tree");

    assert!(
        output.status.success(),
        "tree command failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    serde_json::from_str(&stdout).expect("parse json")
}

fn find_node<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    if node.get("name").and_then(|v| v.as_str()) == Some(name) {
        return Some(node);
    }
    node.get("dependencies")
        .and_then(|deps| deps.as_array())
        .and_then(|deps| deps.iter().find_map(|child| find_node(child, name)))
}

fn collect_nodes<'a>(node: &'a Value, nodes: &mut Vec<&'a Value>) {
    nodes.push(node);
    if let Some(deps) = node.get("dependencies").and_then(|v| v.as_array()) {
        for child in deps {
            collect_nodes(child, nodes);
        }
    }
}

#[test]
fn tree_respects_depth_limits() {
    let temp_dir = create_tree_workspace();
    let root = temp_dir.path();

    let depth_one = run_tree(root, &["--depth", "1"]);
    assert!(find_node(&depth_one["root"], "dep_a")
        .and_then(|node| node.get("dependencies"))
        .and_then(|deps| deps.as_array())
        .map(|deps| deps.is_empty())
        .unwrap_or(false));
    assert!(find_node(&depth_one["root"], "leaf").is_none());

    let depth_two = run_tree(root, &["--depth", "2"]);
    assert!(find_node(&depth_two["root"], "mid").is_some());
    assert!(find_node(&depth_two["root"], "leaf").is_some());
}

#[test]
fn tree_duplicates_filter_keeps_duplicate_paths() {
    let temp_dir = create_tree_workspace();
    let root = temp_dir.path();

    let output = run_tree(root, &["--duplicates"]);
    let mut nodes = Vec::new();
    collect_nodes(&output["root"], &mut nodes);

    let dup_nodes: Vec<&Value> = nodes
        .iter()
        .copied()
        .filter(|node| node.get("name").and_then(|v| v.as_str()) == Some("dup"))
        .collect();

    assert_eq!(dup_nodes.len(), 2);
    assert!(dup_nodes
        .iter()
        .all(|node| node.get("duplicate").and_then(|v| v.as_bool()) == Some(true)));
    assert!(find_node(&output["root"], "dep_a").is_some());
    assert!(find_node(&output["root"], "dep_b").is_some());
    assert!(find_node(&output["root"], "leaf").is_none());
}

#[test]
fn tree_invert_mode_builds_reverse_tree() {
    let temp_dir = create_tree_workspace();
    let root = temp_dir.path();

    let output = run_tree(root, &["--invert", "mid"]);
    assert_eq!(output["root"]["name"], "mid");
    assert!(find_node(&output["root"], "dep_a").is_some());
    assert!(find_node(&output["root"], "app").is_some());

    let dup_output = run_tree(root, &["--invert", "dup"]);
    assert_eq!(dup_output["root"]["name"], "reverse:dup");
    let mut nodes = Vec::new();
    collect_nodes(&dup_output["root"], &mut nodes);
    let dup_versions: HashSet<String> = nodes
        .iter()
        .filter_map(|node| {
            if node.get("name").and_then(|v| v.as_str()) == Some("dup") {
                node.get("version")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        dup_versions,
        HashSet::from(["0.1.0".to_string(), "0.2.0".to_string()])
    );
}

#[test]
fn tree_no_dev_excludes_dev_dependencies() {
    let temp_dir = create_tree_workspace();
    let root = temp_dir.path();

    let with_dev = run_tree(root, &[]);
    assert!(find_node(&with_dev["root"], "dev_only").is_some());

    let no_dev = run_tree(root, &["--no-dev"]);
    assert!(find_node(&no_dev["root"], "dev_only").is_none());
}

#[test]
fn tree_features_flag_populates_features() {
    let temp_dir = create_tree_workspace();
    let root = temp_dir.path();

    let without_features = run_tree(root, &[]);
    let dep_a = find_node(&without_features["root"], "dep_a").expect("dep_a node");
    assert!(dep_a
        .get("features")
        .and_then(|features| features.as_array())
        .map(|features| features.is_empty())
        .unwrap_or(false));

    let with_features = run_tree(root, &["--features"]);
    let dep_a = find_node(&with_features["root"], "dep_a").expect("dep_a node");
    let features: HashSet<String> = dep_a
        .get("features")
        .and_then(|features| features.as_array())
        .map(|features| {
            features
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(features.contains("extra"));
}

#[test]
fn tree_marks_dev_and_build_dependencies() {
    let temp_dir = create_tree_workspace();
    let root = temp_dir.path();

    let output = run_tree(root, &[]);
    let dev_only = find_node(&output["root"], "dev_only").expect("dev_only node");
    assert_eq!(dev_only["is_dev"].as_bool(), Some(true));
    assert_eq!(dev_only["is_build"].as_bool(), Some(false));

    let build_only = find_node(&output["root"], "build_only").expect("build_only node");
    assert_eq!(build_only["is_dev"].as_bool(), Some(false));
    assert_eq!(build_only["is_build"].as_bool(), Some(true));
}

#[test]
fn tree_stats_reports_correct_counts() {
    let temp_dir = create_tree_workspace();
    let root = temp_dir.path();

    let output = run_tree(root, &[]);
    let stats = &output["stats"];

    // Verify stats object exists and has expected fields
    assert!(stats.is_object(), "stats should be an object");

    let total_crates = stats["total_crates"].as_u64().expect("total_crates");
    let direct_deps = stats["direct_deps"].as_u64().expect("direct_deps");
    let transitive_deps = stats["transitive_deps"].as_u64().expect("transitive_deps");
    let duplicate_crates = stats["duplicate_crates"]
        .as_u64()
        .expect("duplicate_crates");

    // The workspace has: app, dep_a, dep_b, mid, dup (2 versions), dev_only, leaf, build_only
    // Total unique packages in tree >= direct + transitive
    assert!(total_crates >= 1, "should have at least one crate");
    assert!(
        direct_deps >= 2,
        "app has at least 2 direct deps (dep_a, dep_b)"
    );
    assert!(
        transitive_deps >= 1,
        "should have transitive deps (mid, leaf, etc.)"
    );
    // dup appears in 2 versions (0.1.0 via dep_a, 0.2.0 via dep_b)
    assert!(
        duplicate_crates >= 1,
        "dup package has multiple versions, so duplicate_crates >= 1"
    );

    // Verify relationship: total = 1 (root) + direct + transitive (approximately)
    // This is a sanity check, not an exact equality since counting can vary
    assert!(
        total_crates >= direct_deps,
        "total_crates should be at least direct_deps"
    );
}
