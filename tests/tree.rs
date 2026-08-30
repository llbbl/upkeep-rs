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

fn create_shared_subtree_project() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    write_file(
        &root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nnormal_left = { path = \"crates/normal_left\" }\nnormal_right = { path = \"crates/normal_right\" }\nreverse_hub = { path = \"crates/reverse_hub\" }\ndup = { path = \"crates/dup_v2\" }\n",
    );
    write_file(&root.join("src/lib.rs"), "pub fn app() {}\n");

    for parent in ["normal_left", "normal_right"] {
        write_file(
            &root.join(format!("crates/{parent}/Cargo.toml")),
            &format!(
                "[package]\nname = \"{parent}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nshared = {{ path = \"../shared\" }}\n"
            ),
        );
        write_file(
            &root.join(format!("crates/{parent}/src/lib.rs")),
            "pub fn parent() {}\n",
        );
    }

    write_file(
        &root.join("crates/shared/Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nshared_leaf = { path = \"../shared_leaf\" }\ndup = { path = \"../dup_v1\" }\n",
    );
    write_file(
        &root.join("crates/shared/src/lib.rs"),
        "pub fn shared() {}\n",
    );

    write_file(
        &root.join("crates/shared_leaf/Cargo.toml"),
        "[package]\nname = \"shared_leaf\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("crates/shared_leaf/src/lib.rs"),
        "pub fn shared_leaf() {}\n",
    );

    write_file(
        &root.join("crates/dup_v1/Cargo.toml"),
        "[package]\nname = \"dup\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("crates/dup_v1/src/lib.rs"),
        "pub fn dup_v1() {}\n",
    );
    write_file(
        &root.join("crates/dup_v2/Cargo.toml"),
        "[package]\nname = \"dup\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("crates/dup_v2/src/lib.rs"),
        "pub fn dup_v2() {}\n",
    );

    write_file(
        &root.join("crates/reverse_hub/Cargo.toml"),
        "[package]\nname = \"reverse_hub\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nreverse_left = { path = \"../reverse_left\" }\nreverse_right = { path = \"../reverse_right\" }\n",
    );
    write_file(
        &root.join("crates/reverse_hub/src/lib.rs"),
        "pub fn reverse_hub() {}\n",
    );

    for parent in ["reverse_left", "reverse_right"] {
        write_file(
            &root.join(format!("crates/{parent}/Cargo.toml")),
            &format!(
                "[package]\nname = \"{parent}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nreverse_target = {{ path = \"../reverse_target\" }}\n"
            ),
        );
        write_file(
            &root.join(format!("crates/{parent}/src/lib.rs")),
            "pub fn reverse_parent() {}\n",
        );
    }

    write_file(
        &root.join("crates/reverse_target/Cargo.toml"),
        "[package]\nname = \"reverse_target\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("crates/reverse_target/src/lib.rs"),
        "pub fn reverse_target() {}\n",
    );

    temp_dir
}

fn create_expanding_dag_project() -> tempfile::TempDir {
    const LAYERS: usize = 15;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let root = temp_dir.path();

    write_file(
        &root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nlayer_0_a = { path = \"crates/layer_0_a\" }\nlayer_0_b = { path = \"crates/layer_0_b\" }\n",
    );
    write_file(&root.join("src/lib.rs"), "pub fn app() {}\n");

    for layer in 0..LAYERS {
        for branch in ['a', 'b'] {
            let name = format!("layer_{layer}_{branch}");
            let dependencies = if layer + 1 < LAYERS {
                format!(
                    "\n[dependencies]\nlayer_{next}_a = {{ path = \"../layer_{next}_a\" }}\nlayer_{next}_b = {{ path = \"../layer_{next}_b\" }}\n",
                    next = layer + 1
                )
            } else {
                String::new()
            };
            write_file(
                &root.join(format!("crates/{name}/Cargo.toml")),
                &format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{dependencies}"
                ),
            );
            write_file(
                &root.join(format!("crates/{name}/src/lib.rs")),
                "pub fn node() {}\n",
            );
        }
    }

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

fn find_direct_child<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("dependencies")
        .and_then(Value::as_array)
        .and_then(|dependencies| {
            dependencies
                .iter()
                .find(|child| child.get("name").and_then(Value::as_str) == Some(name))
        })
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
fn tree_expands_shared_subtree_under_each_parent() {
    let temp_dir = create_shared_subtree_project();
    let output = run_tree(temp_dir.path(), &[]);

    for parent_name in ["normal_left", "normal_right"] {
        let parent = find_direct_child(&output["root"], parent_name).expect("normal parent");
        let shared = find_direct_child(parent, "shared").expect("shared dependency");
        assert!(
            find_direct_child(shared, "shared_leaf").is_some(),
            "shared subtree under {parent_name} should include shared_leaf"
        );
        assert!(
            find_direct_child(shared, "dup").is_some(),
            "shared subtree under {parent_name} should include dup"
        );
    }

    assert_eq!(output["stats"]["total_crates"], 11);
    assert_eq!(output["stats"]["direct_deps"], 4);
    assert_eq!(output["stats"]["transitive_deps"], 6);
    assert_eq!(output["stats"]["duplicate_crates"], 1);
}

#[test]
fn tree_invert_expands_converging_ancestor_subtree_on_each_branch() {
    let temp_dir = create_shared_subtree_project();
    let output = run_tree(temp_dir.path(), &["--invert", "reverse_target"]);

    assert_eq!(output["root"]["name"], "reverse_target");
    for parent_name in ["reverse_left", "reverse_right"] {
        let parent = find_direct_child(&output["root"], parent_name).expect("reverse parent");
        let hub = find_direct_child(parent, "reverse_hub").expect("repeated reverse hub");
        assert!(
            find_direct_child(hub, "app").is_some(),
            "reverse_hub under {parent_name} should retain its app ancestor"
        );
    }

    assert_eq!(output["stats"]["total_crates"], 5);
    assert_eq!(output["stats"]["direct_deps"], 2);
    assert_eq!(output["stats"]["transitive_deps"], 2);
    assert_eq!(output["stats"]["duplicate_crates"], 0);
}

#[test]
fn tree_duplicates_filter_keeps_each_shared_path_to_duplicate() {
    let temp_dir = create_shared_subtree_project();
    let output = run_tree(temp_dir.path(), &["--duplicates"]);

    for parent_name in ["normal_left", "normal_right"] {
        let parent = find_direct_child(&output["root"], parent_name).expect("normal parent");
        let shared = find_direct_child(parent, "shared").expect("shared dependency");
        let duplicate = find_direct_child(shared, "dup").expect("duplicate dependency");
        assert_eq!(duplicate["version"], "0.1.0");
        assert_eq!(duplicate["duplicate"], true);
    }

    let direct_duplicate = find_direct_child(&output["root"], "dup").expect("direct duplicate");
    assert_eq!(direct_duplicate["version"], "0.2.0");
    assert_eq!(direct_duplicate["duplicate"], true);
    assert!(find_node(&output["root"], "shared_leaf").is_none());

    assert_eq!(output["stats"]["total_crates"], 6);
    assert_eq!(output["stats"]["direct_deps"], 3);
    assert_eq!(output["stats"]["transitive_deps"], 2);
    assert_eq!(output["stats"]["duplicate_crates"], 1);
}

#[test]
fn tree_rejects_expansion_beyond_rendered_node_limit() {
    let temp_dir = create_expanding_dag_project();
    let mut cmd = cargo_bin_cmd!("cargo-upkeep");
    let output = cmd
        .current_dir(temp_dir.path())
        .args(["tree", "--json"])
        .output()
        .expect("run tree");

    assert!(
        !output.status.success(),
        "unbounded layered DAG expansion should fail"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr");
    assert!(stderr.contains("rendered-node limit of 50000"), "{stderr}");
    assert!(stderr.contains("--depth"), "{stderr}");

    let depth_limited = run_tree(temp_dir.path(), &["--depth", "10"]);
    assert_eq!(depth_limited["root"]["name"], "app");
    assert_eq!(depth_limited["stats"]["total_crates"], 21);
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

    assert_eq!(dup_nodes.len(), 4);
    assert_eq!(
        dup_nodes
            .iter()
            .filter(|node| node.get("version").and_then(Value::as_str) == Some("0.1.0"))
            .count(),
        2
    );
    assert_eq!(
        dup_nodes
            .iter()
            .filter(|node| node.get("version").and_then(Value::as_str) == Some("0.2.0"))
            .count(),
        2
    );
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

    // Unique packages: app, dep_a, dep_b, mid, dup (2 versions), dev_only, leaf,
    // and build_only. The virtual root has the four workspace members as direct edges.
    assert_eq!(stats["total_crates"], 9);
    assert_eq!(stats["direct_deps"], 4);
    assert_eq!(stats["transitive_deps"], 5);
    assert_eq!(stats["duplicate_crates"], 1);
}
