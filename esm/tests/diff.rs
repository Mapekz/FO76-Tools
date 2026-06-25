use esm::diff::json_diff;
use serde_json::json;

#[test]
fn json_diff_equal_objects_returns_empty() {
    let a = json!({"x": 1, "y": "hello"});
    let b = json!({"x": 1, "y": "hello"});
    assert_eq!(json_diff(&a, &b), json!({}));
}

#[test]
fn json_diff_changed_value() {
    let a = json!({"Float Value": 500.0});
    let b = json!({"Float Value": 1000.0});
    let d = json_diff(&a, &b);
    assert_eq!(d["Float Value"]["from"], json!(500.0));
    assert_eq!(d["Float Value"]["to"], json!(1000.0));
}

#[test]
fn json_diff_added_key() {
    let a = json!({});
    let b = json!({"new_field": 42});
    let d = json_diff(&a, &b);
    assert_eq!(d["new_field"]["from"], json!(null));
    assert_eq!(d["new_field"]["to"], json!(42));
}

#[test]
fn json_diff_removed_key() {
    let a = json!({"old_field": "x"});
    let b = json!({});
    let d = json_diff(&a, &b);
    assert_eq!(d["old_field"]["from"], json!("x"));
    assert_eq!(d["old_field"]["to"], json!(null));
}

#[test]
fn json_diff_nested_object_recurses() {
    let a = json!({"Data": {"x": 1, "y": 2}});
    let b = json!({"Data": {"x": 1, "y": 99}});
    let d = json_diff(&a, &b);
    assert_eq!(d["Data"]["y"]["from"], json!(2));
    assert_eq!(d["Data"]["y"]["to"], json!(99));
    // x is unchanged — must NOT appear in diff
    assert!(
        d["Data"].get("x").is_none() || d["Data"]["x"] == json!(null),
        "unchanged key 'x' should not appear"
    );
}

#[test]
fn json_diff_array_is_opaque() {
    let a = json!({"items": [1, 2, 3]});
    let b = json!({"items": [1, 2, 4]});
    let d = json_diff(&a, &b);
    assert!(d["items"]["from"].is_array());
    assert!(d["items"]["to"].is_array());
}

#[test]
fn json_diff_equal_primitives_returns_empty() {
    assert_eq!(json_diff(&json!(42), &json!(42)), json!({}));
}

#[test]
fn json_diff_unequal_primitives() {
    let d = json_diff(&json!(1), &json!(2));
    assert_eq!(d["from"], json!(1));
    assert_eq!(d["to"], json!(2));
}

#[test]
#[ignore = "requires RUST_TEST_ESM_A and RUST_TEST_ESM_B env vars"]
fn diff_two_esm_versions_glob() {
    let esm_a =
        std::env::var("RUST_TEST_ESM_A").expect("set RUST_TEST_ESM_A to path of older ESM");
    let esm_b =
        std::env::var("RUST_TEST_ESM_B").expect("set RUST_TEST_ESM_B to path of newer ESM");
    let db_a = esm::Database::open(&esm_a).unwrap();
    let db_b = esm::Database::open(&esm_b).unwrap();
    let result = esm::diff::diff_databases(&db_a, &db_b).unwrap();
    println!(
        "Added: {}, Removed: {}, Changed: {}",
        result.added.len(),
        result.removed.len(),
        result.changed.len()
    );
}
