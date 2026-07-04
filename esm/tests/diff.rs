mod common;

use common::{append_record, append_subrecord, cstr, tes4_header, wrap_grup, write_and_open};
use esm::diff::{diff_databases, diff_databases_with, json_diff, strip_noise_fields};
use esm::{BodyDetail, DiffOptions};
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
fn json_diff_array_of_numbers_uses_set_diff() {
    // Arrays of primitives (numbers, strings, ...) are no longer opaque —
    // they get a multiset "set" diff instead.
    let a = json!({"items": [1, 2, 3]});
    let b = json!({"items": [1, 2, 4]});
    let d = json_diff(&a, &b);
    let ad = &d["items"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("set"));
    assert_eq!(ad["removed"], json!([3]));
    assert_eq!(ad["added"], json!([4]));
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

// ---------------------------------------------------------------------------
// Keyed per-element array diffing
// ---------------------------------------------------------------------------

#[test]
fn array_diff_keyed_added_element() {
    let a = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 10.0}},
    ]});
    let b = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 10.0}},
        {"Effect": {"Base Effect": "0x00000002", "Magnitude": 5.0}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Effects"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert_eq!(ad["count_from"], json!(1));
    assert_eq!(ad["count_to"], json!(2));
    assert!(ad.get("removed").is_none());
    assert!(ad.get("changed").is_none());
    let added = ad["added"].as_array().unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["Effect"]["Base Effect"], json!("0x00000002"));
}

#[test]
fn array_diff_keyed_removed_element() {
    let a = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 10.0}},
        {"Effect": {"Base Effect": "0x00000002", "Magnitude": 5.0}},
    ]});
    let b = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 10.0}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Effects"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert_eq!(ad["count_from"], json!(2));
    assert_eq!(ad["count_to"], json!(1));
    assert!(ad.get("added").is_none());
    assert!(ad.get("changed").is_none());
    let removed = ad["removed"].as_array().unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["Effect"]["Base Effect"], json!("0x00000002"));
}

#[test]
fn array_diff_keyed_changed_element_sparse_changes() {
    let a = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 10.0, "Duration": 5}},
    ]});
    let b = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 20.0, "Duration": 5}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Effects"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert!(ad.get("added").is_none());
    assert!(ad.get("removed").is_none());
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["key"]["Base Effect"], json!("0x00000001"));
    // sparse: only the actually-changed field (Magnitude) appears; the
    // unchanged Duration and the (unchanged) key field do not.
    let changes = &changed[0]["changes"]["Effect"];
    assert_eq!(changes["Magnitude"]["from"], json!(10.0));
    assert_eq!(changes["Magnitude"]["to"], json!(20.0));
    assert!(changes.get("Duration").is_none());
    assert!(changes.get("Base Effect").is_none());
}

#[test]
fn array_diff_keyed_reorder_only_is_omitted_from_parent() {
    let a = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 10.0}},
        {"Effect": {"Base Effect": "0x00000002", "Magnitude": 5.0}},
    ]});
    let b = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000002", "Magnitude": 5.0}},
        {"Effect": {"Base Effect": "0x00000001", "Magnitude": 10.0}},
    ]});
    let d = json_diff(&a, &b);
    assert_eq!(
        d,
        json!({}),
        "reorder-only keyed array must be fully omitted from the parent diff"
    );
}

#[test]
fn array_diff_omod_composite_key_enum_canonicalization() {
    // OMOD "Property" entries: no wrapper, keyed on (Function Type, Property).
    // Function Type/Property drift between a bare int and an enum object
    // `{"value": .., "name": ..}` across snapshots but must still canonicalize
    // to the same key so the pair is recognized as "changed" (Value: 10->20)
    // rather than one added + one removed element.
    let a = json!({"Properties": [
        {"Function Type": 3, "Property": 1, "Value": 10},
    ]});
    let b = json!({"Properties": [
        {"Function Type": {"value": 3, "name": "Health"}, "Property": {"value": 1, "name": "Base"}, "Value": 20},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Properties"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert!(ad.get("added").is_none());
    assert!(ad.get("removed").is_none());
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(
        changed.len(),
        1,
        "enum-object vs bare-int Function Type/Property must canonicalize to the same key"
    );
    assert_eq!(changed[0]["changes"]["Value"]["from"], json!(10));
    assert_eq!(changed[0]["changes"]["Value"]["to"], json!(20));
}

#[test]
fn array_diff_lvli_alternate_field_names_canonicalize_to_same_key() {
    // LVLO union: "Reference"/"Item" and "Minimum Level"/"Level" are
    // alternate field names for the same concept across schema versions. A
    // single logical entry using one naming on each side must still be
    // recognized as the *same* entry — paired as one "changed" element, not
    // as one added + one removed (which is what a canonicalization failure
    // would look like).
    let a = json!({"Entries": [
        {"Leveled List Entry": {"Reference": "0x00000010", "Minimum Level": 5, "Count": 3}},
    ]});
    let b = json!({"Entries": [
        {"Leveled List Entry": {"Item": "0x00000010", "Level": 5, "Count": 3}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Entries"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert!(ad.get("added").is_none());
    assert!(ad.get("removed").is_none());
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    // "key" reflects the alternative actually resolved on the "to" side.
    assert_eq!(changed[0]["key"]["Item"], json!("0x00000010"));
    assert_eq!(changed[0]["key"]["Level"], json!(5));
}

#[test]
fn array_diff_lvli_duplicate_keys_pair_positionally() {
    // Two entries share the same (Reference, Minimum Level) key on both
    // sides — duplicates must pair positionally within the group (1st with
    // 1st, 2nd with 2nd), so only the entry that actually changed (Count
    // 2 -> 9) shows up, not both.
    let a = json!({"Entries": [
        {"Leveled List Entry": {"Reference": "0x00000010", "Minimum Level": 5, "Count": 1}},
        {"Leveled List Entry": {"Reference": "0x00000010", "Minimum Level": 5, "Count": 2}},
    ]});
    let b = json!({"Entries": [
        {"Leveled List Entry": {"Reference": "0x00000010", "Minimum Level": 5, "Count": 1}},
        {"Leveled List Entry": {"Reference": "0x00000010", "Minimum Level": 5, "Count": 9}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Entries"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert_eq!(ad["key_fields"], json!(["Reference", "Minimum Level"]));
    assert!(ad.get("added").is_none());
    assert!(ad.get("removed").is_none());
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(
        changed.len(),
        1,
        "only the second duplicate-key pair actually changed (Count 2 -> 9)"
    );
    assert_eq!(
        changed[0]["changes"]["Leveled List Entry"]["Count"]["from"],
        json!(2)
    );
    assert_eq!(
        changed[0]["changes"]["Leveled List Entry"]["Count"]["to"],
        json!(9)
    );
    assert_eq!(changed[0]["key"]["Reference"], json!("0x00000010"));
    assert_eq!(changed[0]["key"]["Minimum Level"], json!(5));
}

#[test]
fn array_diff_quest_objectives_keyed_by_objective_index() {
    let a = json!({"Objectives": [
        {"Objective": {"Objective Index": 0, "Text": "Find the thing"}},
        {"Objective": {"Objective Index": 1, "Text": "Bring it back"}},
    ]});
    let b = json!({"Objectives": [
        {"Objective": {"Objective Index": 0, "Text": "Find the thing"}},
        {"Objective": {"Objective Index": 1, "Text": "Return it"}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Objectives"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert_eq!(ad["key_fields"], json!(["Objective Index"]));
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["key"]["Objective Index"], json!(1));
    assert_eq!(
        changed[0]["changes"]["Objective"]["Text"]["to"],
        json!("Return it")
    );
}

#[test]
fn array_diff_quest_stages_keyed_by_dotted_indx_index() {
    let a = json!({"Stages": [
        {"Stage": {"INDX": {"Stage Index": 10}, "Notes": "a"}},
        {"Stage": {"INDX": {"Stage Index": 20}, "Notes": "b"}},
    ]});
    let b = json!({"Stages": [
        {"Stage": {"INDX": {"Stage Index": 10}, "Notes": "a"}},
        {"Stage": {"INDX": {"Stage Index": 20}, "Notes": "b-updated"}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Stages"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert_eq!(ad["key_fields"], json!(["INDX.Stage Index"]));
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["key"]["INDX.Stage Index"], json!(20));
    assert_eq!(
        changed[0]["changes"]["Stage"]["Notes"]["to"],
        json!("b-updated")
    );
}

#[test]
fn array_diff_heuristic_single_formid_shaped_member() {
    // No wrapper, no table match (Keyword present but no Sound sibling) —
    // falls to the generic "exactly one FormID-shaped member" heuristic.
    let a = json!({"Entries": [
        {"Keyword": "0x00000005", "Chance": 10},
    ]});
    let b = json!({"Entries": [
        {"Keyword": "0x00000005", "Chance": 50},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Entries"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert_eq!(ad["key_fields"], json!(["Keyword"]));
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["changes"]["Chance"]["from"], json!(10));
    assert_eq!(changed[0]["changes"]["Chance"]["to"], json!(50));
}

#[test]
fn array_diff_heuristic_generic_index_suffix_member() {
    let a = json!({"Slots": [
        {"Slot Index": 0, "Value": 1},
        {"Slot Index": 1, "Value": 2},
    ]});
    let b = json!({"Slots": [
        {"Slot Index": 0, "Value": 1},
        {"Slot Index": 1, "Value": 99},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Slots"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    assert_eq!(ad["key_fields"], json!(["Slot Index"]));
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["key"]["Slot Index"], json!(1));
}

#[test]
fn array_diff_positional_fallback_same_length() {
    // Two FormID-shaped members ("A" and "B") — heuristic 9 requires exactly
    // one, so no key applies; equal lengths fall back to positional pairing.
    let a = json!({"Pairs": [
        {"A": "0x00000001", "B": "0x00000002"},
        {"A": "0x00000003", "B": "0x00000004"},
    ]});
    let b = json!({"Pairs": [
        {"A": "0x00000001", "B": "0x00000002"},
        {"A": "0x00000009", "B": "0x00000004"},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Pairs"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("positional"));
    assert_eq!(ad["count_from"], json!(2));
    assert_eq!(ad["count_to"], json!(2));
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["key"], json!({"index": 1}));
    assert_eq!(changed[0]["index_from"], json!(1));
    assert_eq!(changed[0]["index_to"], json!(1));
    assert_eq!(changed[0]["changes"]["A"]["to"], json!("0x00000009"));
}

#[test]
fn array_diff_opaque_fallback_unkeyable_length_mismatch() {
    let a = json!({"Pairs": [
        {"A": "0x00000001", "B": "0x00000002"},
    ]});
    let b = json!({"Pairs": [
        {"A": "0x00000001", "B": "0x00000002"},
        {"A": "0x00000009", "B": "0x00000004"},
    ]});
    let d = json_diff(&a, &b);
    assert!(d["Pairs"].get("_array_diff").is_none());
    assert_eq!(d["Pairs"]["from"].as_array().unwrap().len(), 1);
    assert_eq!(d["Pairs"]["to"].as_array().unwrap().len(), 2);
}

#[test]
fn array_diff_primitive_set_strategy_multiset_semantics() {
    let a = json!({"Keywords": ["0x00000001", "0x00000002", "0x00000002"]});
    let b = json!({"Keywords": ["0x00000001", "0x00000002", "0x00000003"]});
    let d = json_diff(&a, &b);
    let ad = &d["Keywords"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("set"));
    assert_eq!(ad["count_from"], json!(3));
    assert_eq!(ad["count_to"], json!(3));
    // One copy of 0x2 (present twice on A, once on B) is removed; one copy
    // of 0x3 is added — not a wholesale re-list of both arrays.
    assert_eq!(ad["removed"], json!(["0x00000002"]));
    assert_eq!(ad["added"], json!(["0x00000003"]));
}

#[test]
fn array_diff_nested_array_inside_changed_element() {
    let a = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Keywords": ["0x00000010"]}},
    ]});
    let b = json!({"Effects": [
        {"Effect": {"Base Effect": "0x00000001", "Keywords": ["0x00000010", "0x00000020"]}},
    ]});
    let d = json_diff(&a, &b);
    let ad = &d["Effects"]["_array_diff"];
    assert_eq!(ad["strategy"], json!("keyed"));
    let changed = ad["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    let inner = &changed[0]["changes"]["Effect"]["Keywords"]["_array_diff"];
    assert_eq!(inner["strategy"], json!("set"));
    assert_eq!(inner["added"], json!(["0x00000020"]));
}

#[test]
fn diff_two_esm_versions_glob() {
    let Ok(esm_a) = std::env::var("RUST_TEST_ESM_A") else {
        eprintln!("RUST_TEST_ESM_A / RUST_TEST_ESM_B not set — skipping");
        return;
    };
    let Ok(esm_b) = std::env::var("RUST_TEST_ESM_B") else {
        eprintln!("RUST_TEST_ESM_B not set — skipping");
        return;
    };
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

// ---------------------------------------------------------------------------
// DiffOptions: BodyDetail on added/removed records
// ---------------------------------------------------------------------------

#[test]
fn removed_record_gets_full_body_from_a() {
    // MISC(1) exists only in A.
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &cstr("RemovedItem"));
    let mut recs = Vec::new();
    append_record(&mut recs, b"MISC", 1, &subs);

    let mut buf_a = tes4_header();
    buf_a.extend(wrap_grup(b"MISC", &recs));
    let buf_b = tes4_header();

    let (path_a, db_a) = write_and_open(&buf_a, "diff_removed_a");
    let (path_b, db_b) = write_and_open(&buf_b, "diff_removed_b");

    let result = diff_databases(&db_a, &db_b).expect("diff");
    assert_eq!(result.removed.len(), 1);
    assert_eq!(result.added.len(), 0);
    let fields = result.removed[0]
        .fields
        .as_ref()
        .expect("removed record must carry a decoded body from A (old-side decode)");
    assert_eq!(fields["Editor ID"], json!("RemovedItem"));

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

#[test]
fn added_record_any_type_gets_body() {
    // BOOK is not in the old ADDED_DETAIL_TYPES whitelist — it must still get
    // a decoded body now that every added type qualifies.
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &cstr("NewBook"));
    let mut recs = Vec::new();
    append_record(&mut recs, b"BOOK", 1, &subs);

    let buf_a = tes4_header();
    let mut buf_b = tes4_header();
    buf_b.extend(wrap_grup(b"BOOK", &recs));

    let (path_a, db_a) = write_and_open(&buf_a, "diff_added_a");
    let (path_b, db_b) = write_and_open(&buf_b, "diff_added_b");

    let result = diff_databases(&db_a, &db_b).expect("diff");
    assert_eq!(result.added.len(), 1);
    let fields = result.added[0]
        .fields
        .as_ref()
        .expect("BOOK (not in the old whitelist) must now get a decoded body");
    assert_eq!(fields["Editor ID"], json!("NewBook"));

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

#[test]
fn bodies_none_skips_fields_both_sides() {
    let mut misc_subs = Vec::new();
    append_subrecord(&mut misc_subs, b"EDID", &cstr("RemovedItem"));
    let mut misc_recs = Vec::new();
    append_record(&mut misc_recs, b"MISC", 1, &misc_subs);

    let mut book_subs = Vec::new();
    append_subrecord(&mut book_subs, b"EDID", &cstr("NewBook"));
    let mut book_recs = Vec::new();
    append_record(&mut book_recs, b"BOOK", 2, &book_subs);

    let mut buf_a = tes4_header();
    buf_a.extend(wrap_grup(b"MISC", &misc_recs));
    let mut buf_b = tes4_header();
    buf_b.extend(wrap_grup(b"BOOK", &book_recs));

    let (path_a, db_a) = write_and_open(&buf_a, "diff_bodies_none_a");
    let (path_b, db_b) = write_and_open(&buf_b, "diff_bodies_none_b");

    let opts = DiffOptions {
        bodies: BodyDetail::None,
        ..Default::default()
    };
    let result = diff_databases_with(&db_a, &db_b, &opts).expect("diff");
    assert_eq!(result.added.len(), 1);
    assert_eq!(result.removed.len(), 1);
    assert!(
        result.added[0].fields.is_none(),
        "BodyDetail::None must skip fields on the added side"
    );
    assert!(
        result.removed[0].fields.is_none(),
        "BodyDetail::None must skip fields on the removed side"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

// ---------------------------------------------------------------------------
// DiffOptions: exclude_types
// ---------------------------------------------------------------------------

#[test]
fn exclude_types_skips_all_buckets() {
    // NAVM(1): removed. NAVM(2): added. NAVM(3): changed (EDID differs).
    // MISC(5): changed (EDID differs) — a control record that must survive
    // the exclusion untouched.
    let mut navm1 = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &cstr("OldNav1"));
        append_record(&mut navm1, b"NAVM", 1, &subs);
    }
    let mut navm2 = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &cstr("NewNav2"));
        append_record(&mut navm2, b"NAVM", 2, &subs);
    }
    let mut navm3_a = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &cstr("NavA"));
        append_record(&mut navm3_a, b"NAVM", 3, &subs);
    }
    let mut navm3_b = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &cstr("NavB"));
        append_record(&mut navm3_b, b"NAVM", 3, &subs);
    }
    let mut misc5_a = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &cstr("KeepA"));
        append_record(&mut misc5_a, b"MISC", 5, &subs);
    }
    let mut misc5_b = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &cstr("KeepB"));
        append_record(&mut misc5_b, b"MISC", 5, &subs);
    }

    let mut navm_a = navm1;
    navm_a.extend(navm3_a);
    let mut navm_b = navm2;
    navm_b.extend(navm3_b);

    let mut buf_a = tes4_header();
    buf_a.extend(wrap_grup(b"NAVM", &navm_a));
    buf_a.extend(wrap_grup(b"MISC", &misc5_a));
    let mut buf_b = tes4_header();
    buf_b.extend(wrap_grup(b"NAVM", &navm_b));
    buf_b.extend(wrap_grup(b"MISC", &misc5_b));

    let (path_a, db_a) = write_and_open(&buf_a, "diff_exclude_a");
    let (path_b, db_b) = write_and_open(&buf_b, "diff_exclude_b");

    let opts = DiffOptions {
        exclude_types: vec!["NAVM".to_string()],
        ..Default::default()
    };
    let result = diff_databases_with(&db_a, &db_b, &opts).expect("diff");

    assert!(
        result.added.iter().all(|s| s.record_type != "NAVM"),
        "excluded type must be absent from added: {:?}",
        result.added
    );
    assert!(
        result.removed.iter().all(|s| s.record_type != "NAVM"),
        "excluded type must be absent from removed: {:?}",
        result.removed
    );
    assert!(
        result.changed.iter().all(|d| d.stub.record_type != "NAVM"),
        "excluded type must be absent from changed: {:?}",
        result.changed
    );
    // The control MISC record isn't excluded and must still show up changed.
    assert_eq!(result.changed.len(), 1);
    assert_eq!(result.changed[0].stub.record_type, "MISC");

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

// ---------------------------------------------------------------------------
// Noise suppression
// ---------------------------------------------------------------------------

#[test]
fn strip_noise_fields_position_only_change_is_fully_stripped() {
    let mut changes = json!({
        "Position/Rotation": {"from": "00000000", "to": "11111111"},
    });
    strip_noise_fields(&mut changes, "REFR");
    assert_eq!(changes, json!({}));
}

#[test]
fn strip_noise_fields_keeps_base_change_strips_position() {
    let mut changes = json!({
        "Base": {"from": "0x00000001", "to": "0x00000002"},
        "Position/Rotation": {"from": "00000000", "to": "11111111"},
    });
    strip_noise_fields(&mut changes, "REFR");
    assert_eq!(
        changes,
        json!({"Base": {"from": "0x00000001", "to": "0x00000002"}})
    );
}

#[test]
fn strip_noise_fields_cell_precombine_only_change_is_fully_stripped() {
    let mut changes = json!({
        "PreVis File Hash": {"from": "aa", "to": "bb"},
        "Precombined Object Level XY": {"from": 1, "to": 2},
        "Combined References": {"from": {}, "to": {}},
    });
    strip_noise_fields(&mut changes, "CELL");
    assert_eq!(changes, json!({}));
}

#[test]
fn strip_noise_fields_untouched_for_non_placement_non_cell_type() {
    // PLACEMENT_NOISE_FIELDS and CELL_NOISE_FIELDS are only stripped for their
    // respective types — a WEAP change under one of those same names (which
    // wouldn't occur in practice, but exercises the type gating) is untouched.
    let mut changes = json!({
        "Scale": {"from": 1.0, "to": 2.0},
    });
    strip_noise_fields(&mut changes, "WEAP");
    assert_eq!(changes, json!({"Scale": {"from": 1.0, "to": 2.0}}));
}

#[test]
fn strip_noise_fields_global_object_bounds_stripped_for_any_type() {
    let mut changes = json!({
        "Object Bounds": {"from": {"X1": 0}, "to": {"X1": 1}},
    });
    strip_noise_fields(&mut changes, "WEAP");
    assert_eq!(changes, json!({}));
}

/// Integration-level check: a REFR record differing only in position is
/// dropped entirely from `changed`, and counted in `suppressed_counts`; a
/// second REFR differing in both `Base` and position is kept with only the
/// position key stripped.
#[test]
fn suppress_noise_drops_position_only_refr_and_counts_it() {
    // REFR(10): Base constant, DATA (position) differs — should be fully
    // suppressed.
    let mut refr10_a = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"NAME", &1u32.to_le_bytes());
        append_subrecord(&mut subs, b"DATA", &[0u8; 24]);
        append_record(&mut refr10_a, b"REFR", 10, &subs);
    }
    let mut refr10_b = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"NAME", &1u32.to_le_bytes());
        append_subrecord(&mut subs, b"DATA", &[1u8; 24]);
        append_record(&mut refr10_b, b"REFR", 10, &subs);
    }

    // REFR(20): Base differs *and* DATA (position) differs — should survive
    // with only "Position/Rotation" stripped.
    let mut refr20_a = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"NAME", &1u32.to_le_bytes());
        append_subrecord(&mut subs, b"DATA", &[0u8; 24]);
        append_record(&mut refr20_a, b"REFR", 20, &subs);
    }
    let mut refr20_b = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"NAME", &2u32.to_le_bytes());
        append_subrecord(&mut subs, b"DATA", &[1u8; 24]);
        append_record(&mut refr20_b, b"REFR", 20, &subs);
    }

    let mut refr_a = refr10_a;
    refr_a.extend(refr20_a);
    let mut refr_b = refr10_b;
    refr_b.extend(refr20_b);

    let mut buf_a = tes4_header();
    buf_a.extend(wrap_grup(b"REFR", &refr_a));
    let mut buf_b = tes4_header();
    buf_b.extend(wrap_grup(b"REFR", &refr_b));

    let (path_a, db_a) = write_and_open(&buf_a, "diff_suppress_a");
    let (path_b, db_b) = write_and_open(&buf_b, "diff_suppress_b");

    let result = diff_databases(&db_a, &db_b).expect("diff"); // default: suppress_noise = true

    assert!(
        !result
            .changed
            .iter()
            .any(|d| d.stub.form_id == "0x0000000A"),
        "REFR(10) (position-only change) must be dropped: {:?}",
        result.changed
    );
    assert_eq!(
        result.suppressed_counts.get("REFR").copied(),
        Some(1),
        "exactly one REFR change must be counted as suppressed"
    );

    let kept = result
        .changed
        .iter()
        .find(|d| d.stub.form_id == "0x00000014")
        .expect("REFR(20) (Base + position change) must survive suppression");
    assert!(
        kept.field_changes.get("Position/Rotation").is_none(),
        "Position/Rotation must be stripped from the surviving record"
    );
    assert!(
        kept.field_changes.get("Base").is_some(),
        "Base change must survive suppression"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

/// Same REFR(10) position-only-change scenario, but with `suppress_noise:
/// false` — the record must be kept, unmodified.
#[test]
fn suppress_noise_false_keeps_position_only_refr_change() {
    let mut refr_a = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"NAME", &1u32.to_le_bytes());
        append_subrecord(&mut subs, b"DATA", &[0u8; 24]);
        append_record(&mut refr_a, b"REFR", 10, &subs);
    }
    let mut refr_b = Vec::new();
    {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"NAME", &1u32.to_le_bytes());
        append_subrecord(&mut subs, b"DATA", &[1u8; 24]);
        append_record(&mut refr_b, b"REFR", 10, &subs);
    }

    let mut buf_a = tes4_header();
    buf_a.extend(wrap_grup(b"REFR", &refr_a));
    let mut buf_b = tes4_header();
    buf_b.extend(wrap_grup(b"REFR", &refr_b));

    let (path_a, db_a) = write_and_open(&buf_a, "diff_no_suppress_a");
    let (path_b, db_b) = write_and_open(&buf_b, "diff_no_suppress_b");

    let opts = DiffOptions {
        suppress_noise: false,
        ..Default::default()
    };
    let result = diff_databases_with(&db_a, &db_b, &opts).expect("diff");

    assert_eq!(
        result.changed.len(),
        1,
        "the record must be kept: {:?}",
        result.changed
    );
    assert!(result.suppressed_counts.is_empty());
    assert!(
        result.changed[0]
            .field_changes
            .get("Position/Rotation")
            .is_some(),
        "with suppress_noise=false, Position/Rotation must remain in field_changes"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

// ---------------------------------------------------------------------------
// ref_names: description
// ---------------------------------------------------------------------------

#[test]
fn ref_names_includes_description() {
    // TGT: MISC(1), identical in both files — present in the index so
    // `resolve_ref_name` can look it up regardless of whether it changed.
    let mut tgt_subs = Vec::new();
    append_subrecord(&mut tgt_subs, b"EDID", &cstr("TargetItem"));
    append_subrecord(&mut tgt_subs, b"DESC", &cstr("A target description"));
    let mut tgt_rec = Vec::new();
    append_record(&mut tgt_rec, b"MISC", 1, &tgt_subs);

    // REF: WEAP(2) — YNAM ("Sound - Pickup") changes from NULL to TGT(1),
    // surfacing "0x00000001" inside `field_changes`.
    let mut ref_subs_a = Vec::new();
    append_subrecord(&mut ref_subs_a, b"YNAM", &0u32.to_le_bytes());
    let mut ref_rec_a = Vec::new();
    append_record(&mut ref_rec_a, b"WEAP", 2, &ref_subs_a);

    let mut ref_subs_b = Vec::new();
    append_subrecord(&mut ref_subs_b, b"YNAM", &1u32.to_le_bytes());
    let mut ref_rec_b = Vec::new();
    append_record(&mut ref_rec_b, b"WEAP", 2, &ref_subs_b);

    let mut buf_a = tes4_header();
    buf_a.extend(wrap_grup(b"MISC", &tgt_rec));
    buf_a.extend(wrap_grup(b"WEAP", &ref_rec_a));
    let mut buf_b = tes4_header();
    buf_b.extend(wrap_grup(b"MISC", &tgt_rec));
    buf_b.extend(wrap_grup(b"WEAP", &ref_rec_b));

    let (path_a, db_a) = write_and_open(&buf_a, "diff_ref_desc_a");
    let (path_b, db_b) = write_and_open(&buf_b, "diff_ref_desc_b");

    let result = diff_databases(&db_a, &db_b).expect("diff");
    let rn = result
        .ref_names
        .get("0x00000001")
        .expect("ref_names must include the resolved TGT(1) reference");
    assert_eq!(rn.editor_id.as_deref(), Some("TargetItem"));
    assert_eq!(rn.description.as_deref(), Some("A target description"));

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}
