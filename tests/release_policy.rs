use toml::Value;

const CLIFF_CONFIG: &str = include_str!("../cliff.toml");

fn config() -> Value {
    toml::from_str(CLIFF_CONFIG).expect("cliff.toml should be valid TOML")
}

#[test]
fn pre_1_0_features_bump_patch_and_breaking_changes_bump_minor() {
    let config = config();
    let bump = config
        .get("bump")
        .and_then(Value::as_table)
        .expect("cliff.toml should define [bump]");

    assert_eq!(
        bump.get("features_always_bump_minor")
            .and_then(Value::as_bool),
        Some(false),
        "ordinary feat commits must remain patch releases before 1.0"
    );
    assert_eq!(
        bump.get("breaking_always_bump_major")
            .and_then(Value::as_bool),
        Some(false),
        "breaking commits must bump minor rather than major before 1.0"
    );
}

#[test]
fn release_commit_parsers_keep_features_and_skip_ci() {
    let config = config();
    let git = config
        .get("git")
        .and_then(Value::as_table)
        .expect("cliff.toml should define [git]");

    assert_eq!(
        git.get("conventional_commits").and_then(Value::as_bool),
        Some(true),
        "bump policy depends on conventional commit parsing"
    );
    assert_eq!(
        git.get("filter_unconventional").and_then(Value::as_bool),
        Some(true),
        "unconventional commits must not accidentally trigger a release"
    );

    let parsers = git
        .get("commit_parsers")
        .and_then(Value::as_array)
        .expect("[git] should define commit_parsers");
    let feature = parsers
        .iter()
        .filter_map(Value::as_table)
        .find(|parser| parser.get("message").and_then(Value::as_str) == Some("^feat"))
        .expect("features should have a commit parser");
    assert_eq!(
        feature.get("group").and_then(Value::as_str),
        Some("Features"),
        "feat and feat! commits should remain honestly grouped as features"
    );
    assert_ne!(
        feature.get("skip").and_then(Value::as_bool),
        Some(true),
        "features must remain releasable"
    );

    let ci = parsers
        .iter()
        .filter_map(Value::as_table)
        .find(|parser| parser.get("message").and_then(Value::as_str) == Some("^ci"))
        .expect("CI changes should have a commit parser");
    assert_eq!(
        ci.get("skip").and_then(Value::as_bool),
        Some(true),
        "CI-only commits must remain non-releasing"
    );
}
