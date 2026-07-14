mod common;

use common::{
    append_record, append_subrecord, cstr, make_minimal_esm, tes4_header, unique_temp_path,
    wrap_grup,
};
use esm::ipc::{dispatch, Op, RecordSel, Request, Response};
use esm::registry::Registry;
use esm::{
    BodyDetail, BulkRecordEntry, Database, DiffOptions, DiffResult, ResolveDepth, SearchField,
};
use std::io::Write;
use std::path::PathBuf;

fn open_test_db() -> (PathBuf, Registry) {
    let buf = make_minimal_esm();
    let tmp_path = unique_temp_path("ipc_dispatch");
    {
        let mut f = std::fs::File::create(&tmp_path).expect("create temp file");
        f.write_all(&buf).expect("write");
    }
    let reg = Registry::new();
    reg.get_or_open(&tmp_path).expect("open");
    (tmp_path, reg)
}

#[test]
fn dispatch_file_info_matches_direct() {
    let (path, reg) = open_test_db();
    let db = Database::open(&path).expect("open");

    let req = Request {
        esm: path.clone(),
        op: Op::FileInfo,
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };

    let direct = db.file_info().expect("file_info");
    let via_dispatch: esm::reader::FileInfo = serde_json::from_value(data).unwrap();
    assert_eq!(via_dispatch.record_count, direct.record_count);
    assert_eq!(via_dispatch.version, direct.version);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn dispatch_list_by_type() {
    let (path, reg) = open_test_db();

    let req = Request {
        esm: path.clone(),
        op: Op::ListByType {
            sig: "WEAP".to_string(),
            limit: 10,
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let entries: Vec<esm::ListEntry> = serde_json::from_value(data).unwrap();
    assert_eq!(entries.len(), 2);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn dispatch_search_wildcard() {
    let (path, reg) = open_test_db();

    let req = Request {
        esm: path.clone(),
        op: Op::Search {
            pattern: "*".to_string(),
            types: vec!["WEAP".to_string()],
            field: SearchField::Edid,
            limit: 0,
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let rows: Vec<esm::RecordRow> = serde_json::from_value(data).unwrap();
    // Minimal synthetic ESM has no EDID/FULL subrecords, so search returns empty.
    assert!(rows.is_empty());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn dispatch_record_by_formid() {
    let (path, reg) = open_test_db();

    let req = Request {
        esm: path.clone(),
        op: Op::Record {
            sel: RecordSel::FormId(esm::FormId(1)),
            depth: ResolveDepth::None,
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    assert!(data.get("header").is_some());
    assert!(data.get("fields").is_some());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn dispatch_list_groups() {
    let (path, reg) = open_test_db();

    let req = Request {
        esm: path.clone(),
        op: Op::ListGroups,
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let groups: Vec<esm::GroupNode> = serde_json::from_value(data).unwrap();
    assert_eq!(groups.len(), 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn dispatch_diff_same_path_does_not_deadlock() {
    let (path, reg) = open_test_db();

    let req = Request {
        esm: path.clone(),
        op: Op::Diff {
            b: path.clone(),
            record_type: None,
            options: DiffOptions::default(),
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let diff: esm::DiffResult = serde_json::from_value(data).unwrap();
    assert!(diff.added.is_empty());
    assert!(diff.removed.is_empty());
    assert!(diff.changed.is_empty());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn local_backend_parity_with_dispatch() {
    use esm::backend::{LocalBackend, QueryBackend};

    let (path, reg) = open_test_db();
    let mut local = LocalBackend::new();

    let op = Op::FileInfo;
    let req = Request {
        esm: path.clone(),
        op: op.clone(),
    };
    let Response::Ok { data: via_reg } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let via_local = local.run(&path, op).expect("local run");
    assert_eq!(via_reg, via_local);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn looks_like_formid_heuristic() {
    // FormID-looking tokens.
    assert!(esm::looks_like_formid("0x463F"));
    assert!(esm::looks_like_formid("0X463F"));
    assert!(esm::looks_like_formid("463F"));
    assert!(esm::looks_like_formid("18000"));
    assert!(esm::looks_like_formid("cafe")); // short all-hex EditorIDs read as FormIDs
    assert!(esm::looks_like_formid("DEADBEEF")); // exactly 8 hex digits
    assert!(esm::looks_like_formid("  463F  ")); // surrounding whitespace trimmed

    // EditorID-looking tokens.
    assert!(!esm::looks_like_formid("AssaultRifle"));
    assert!(!esm::looks_like_formid("Enc_Raider"));
    assert!(!esm::looks_like_formid("DEADBEEF1")); // 9 hex digits — too long for a u32
    assert!(!esm::looks_like_formid(""));
    assert!(!esm::looks_like_formid("0x"));
}

#[test]
fn record_sel_from_input_auto_detects() {
    match RecordSel::from_input("0x463F").unwrap() {
        RecordSel::FormId(f) => assert_eq!(f, esm::FormId(0x463F)),
        other => panic!("expected FormId, got {other:?}"),
    }
    match RecordSel::from_input("18000").unwrap() {
        RecordSel::FormId(f) => assert_eq!(f, esm::FormId(18000)),
        other => panic!("expected FormId, got {other:?}"),
    }
    match RecordSel::from_input("AssaultRifle").unwrap() {
        RecordSel::Edid(e) => assert_eq!(e, "AssaultRifle"),
        other => panic!("expected Edid, got {other:?}"),
    }
}

/// `RecordSel` must survive a JSON round-trip.
///
/// `RecordSel` uses adjacently-tagged serde (`tag = "kind", content = "value"`):
/// internally-tagged (`tag = "kind"`) on a newtype enum whose payload is a
/// primitive (u32 / String) fails to serialize — serde_json errors with
/// "cannot serialize tagged newtype variant … containing an integer".
#[test]
fn record_sel_json_round_trip() {
    let formid_sel = RecordSel::FormId(esm::FormId(0x0010_ABCD));
    let edid_sel = RecordSel::Edid("AssaultRifle".to_string());

    for sel in [formid_sel, edid_sel] {
        let op = Op::Record {
            sel: sel.clone(),
            depth: ResolveDepth::None,
        };
        let req = Request {
            esm: PathBuf::from("Game.esm"),
            op,
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let back: Request = serde_json::from_str(&json).expect("deserialize");
        // Re-serialize the round-tripped value and compare JSON strings.
        let json2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json2, "round-trip mismatch for {:?}", sel);
    }
}

/// A `Response::Ok { data }` carrying an extreme-magnitude decoded float
/// (subnormal or near f32::MAX) must survive a JSON text round-trip exactly —
/// this is what happens on every `-p`/daemon request: the server serializes
/// `Response` to text, sends it over HTTP, and the client re-parses it.
///
/// serde_json's *default* float parser does not guarantee exact round-trip
/// precision for every f64 (particularly extreme exponents): parsing back a
/// clean, shortest-round-trip string like `"2.803e-42"` can land on a
/// *different* nearby f64 whose own shortest representation needs many more
/// digits (`"2.8030000000000003e-42"`), silently reintroducing noise that
/// `decode::json_f32` (see decode.rs) already removed at decode time. This is
/// fixed by enabling serde_json's `float_roundtrip` feature in Cargo.toml —
/// this test guards against that feature flag ever being dropped.
#[test]
fn response_json_round_trip_preserves_extreme_float_precision() {
    let cases: &[f64] = &[
        2.803e-42,    // subnormal-range f32 widened to f64 (real QUST QSTA "Radius" case)
        3.4028235e38, // f32::MAX widened to f64 (used elsewhere as a sentinel)
        1.401298e-45, // smallest positive subnormal f32, widened to f64
    ];

    for &value in cases {
        let resp = Response::Ok {
            data: serde_json::json!({ "Radius": value }),
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let back: Response = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(
            json, json2,
            "extreme float {value} did not round-trip losslessly through JSON text"
        );
    }
}

/// A pre-`options` wire client (e.g. an older N-API build) sends `Op::Diff`
/// JSON with no `options` field at all. `#[serde(default)]` on that field
/// must fill in `DiffOptions::default()` rather than failing to deserialize.
#[test]
fn op_diff_without_options_field_deserializes() {
    let json = r#"{
        "esm": "Game.esm",
        "op": {
            "op": "diff",
            "b": "Other.esm",
            "record_type": null
        }
    }"#;
    let req: Request = serde_json::from_str(json).expect("deserialize");
    match req.op {
        Op::Diff {
            b,
            record_type,
            options,
        } => {
            assert_eq!(b, PathBuf::from("Other.esm"));
            assert_eq!(record_type, None);
            assert_eq!(options.bodies, BodyDetail::Full);
            assert!(options.suppress_noise);
            assert!(options.exclude_types.is_empty());
        }
        other => panic!("expected Op::Diff, got {other:?}"),
    }
}

/// A pre-`type_filter`/`paths` wire client (e.g. an older N-API build) sends
/// `Op::ReferencedBy` JSON with neither field. `#[serde(default)]` on both
/// must fill in `None`/`false` rather than failing to deserialize.
#[test]
fn op_referenced_by_without_new_fields_deserializes() {
    let json = r#"{
        "esm": "Game.esm",
        "op": {
            "op": "referenced_by",
            "sel": {"kind": "form_id", "value": 4667},
            "limit": 100,
            "depth": 1
        }
    }"#;
    let req: Request = serde_json::from_str(json).expect("deserialize");
    match req.op {
        Op::ReferencedBy {
            limit,
            depth,
            type_filter,
            paths,
            ..
        } => {
            assert_eq!(limit, 100);
            assert_eq!(depth, 1);
            assert_eq!(type_filter, None);
            assert!(!paths);
        }
        other => panic!("expected Op::ReferencedBy, got {other:?}"),
    }
}

/// `Op::ReferencedBy` with `type_filter`/`paths` set must survive a full JSON
/// round-trip, and dispatching it end-to-end must apply the filter and
/// annotate rows with `field_paths` — exercising the daemon IPC path (not
/// just `referenced_by_enriched` directly, which `tests/refs.rs` covers).
#[test]
fn dispatch_referenced_by_with_type_filter_and_paths() {
    let buf = common::make_xref_esm();
    let tmp = common::unique_temp_path("ipc_refs_paths");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp esm");
        f.write_all(&buf).expect("write temp esm");
    }
    let reg = Registry::new();

    let op = Op::ReferencedBy {
        sel: RecordSel::FormId(esm::FormId(1)),
        limit: 0,
        depth: 1,
        type_filter: Some("WEAP".to_string()),
        paths: true,
    };

    // Round-trip check.
    let req = Request {
        esm: tmp.clone(),
        op: op.clone(),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let back: Request = serde_json::from_str(&json).expect("deserialize");
    let json2 = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json2, "round-trip mismatch");

    // End-to-end dispatch.
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let list: esm::RefList = serde_json::from_value(data).expect("RefList");
    assert_eq!(list.rows.len(), 1);
    assert_eq!(list.rows[0].record_type.as_deref(), Some("WEAP"));
    assert_eq!(
        list.rows[0].field_paths,
        Some(vec![
            "Sound - Pickup".to_string(),
            "Sound - Putdown".to_string()
        ])
    );

    // A non-matching type filter excludes the one referencer entirely.
    let op_no_match = Op::ReferencedBy {
        sel: RecordSel::FormId(esm::FormId(1)),
        limit: 0,
        depth: 1,
        type_filter: Some("MISC".to_string()),
        paths: false,
    };
    let req_no_match = Request {
        esm: tmp.clone(),
        op: op_no_match,
    };
    let Response::Ok { data } = dispatch(&reg, &req_no_match) else {
        panic!("expected Ok");
    };
    let list: esm::RefList = serde_json::from_value(data).expect("RefList");
    assert!(list.rows.is_empty());

    let _ = std::fs::remove_file(&tmp);
}

/// Non-default `DiffOptions` (stub bodies, noise kept, explicit type
/// exclusions) must survive a full JSON round-trip on `Op::Diff` — both the
/// re-serialized wire form and each individual field.
#[test]
fn op_diff_with_options_roundtrip() {
    let op = Op::Diff {
        b: PathBuf::from("Other.esm"),
        record_type: Some("WEAP".to_string()),
        options: DiffOptions {
            bodies: BodyDetail::Stub,
            suppress_noise: false,
            exclude_types: vec!["LAND".to_string(), "NAVM".to_string()],
        },
    };
    let req = Request {
        esm: PathBuf::from("Game.esm"),
        op,
    };

    let json = serde_json::to_string(&req).expect("serialize");
    let back: Request = serde_json::from_str(&json).expect("deserialize");
    let json2 = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json2, "round-trip mismatch");

    match back.op {
        Op::Diff {
            b,
            record_type,
            options,
        } => {
            assert_eq!(b, PathBuf::from("Other.esm"));
            assert_eq!(record_type, Some("WEAP".to_string()));
            assert_eq!(options.bodies, BodyDetail::Stub);
            assert!(!options.suppress_noise);
            assert_eq!(
                options.exclude_types,
                vec!["LAND".to_string(), "NAVM".to_string()]
            );
        }
        other => panic!("expected Op::Diff, got {other:?}"),
    }
}

/// End-to-end: dispatch `Op::Diff` with non-default options (`bodies: None`,
/// an explicit `exclude_types` filter) across two distinct synthetic ESMs and
/// confirm both the added/removed bookkeeping and the options themselves took
/// effect through the full `Registry` → `dispatch` path (not just
/// `diff_databases_with` called directly, which `tests/diff.rs` already
/// covers).
#[test]
fn dispatch_diff_two_esms_with_options() {
    // A: one WEAP(1) record. B: that WEAP is gone, and a new MISC(2) record
    // exists instead — so WEAP(1) is "removed" and MISC(2) is "added".
    let mut weap_recs = Vec::new();
    append_record(&mut weap_recs, b"WEAP", 1, &[]);
    let mut buf_a = tes4_header();
    buf_a.extend(wrap_grup(b"WEAP", &weap_recs));

    let mut misc_recs = Vec::new();
    append_record(&mut misc_recs, b"MISC", 2, &[]);
    let mut buf_b = tes4_header();
    buf_b.extend(wrap_grup(b"MISC", &misc_recs));

    let path_a = unique_temp_path("ipc_diff_opts_a");
    let path_b = unique_temp_path("ipc_diff_opts_b");
    std::fs::File::create(&path_a)
        .expect("create a")
        .write_all(&buf_a)
        .expect("write a");
    std::fs::File::create(&path_b)
        .expect("create b")
        .write_all(&buf_b)
        .expect("write b");

    let reg = Registry::new();
    let req = Request {
        esm: path_a.clone(),
        op: Op::Diff {
            b: path_b.clone(),
            record_type: None,
            options: DiffOptions {
                bodies: BodyDetail::None,
                suppress_noise: true,
                exclude_types: vec!["MISC".to_string()],
            },
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let diff: DiffResult = serde_json::from_value(data).expect("DiffResult");

    // MISC is excluded outright, so only the WEAP removal survives.
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.removed[0].record_type, "WEAP");
    assert!(diff.added.is_empty(), "MISC(2) must be excluded by options");

    // bodies: None must skip the decoded body on the surviving stub.
    assert!(
        diff.removed[0].fields.is_none(),
        "BodyDetail::None must skip fields"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

// ─── Op::RecordBulk ─────────────────────────────────────────────────────────

/// A pre-bulk wire client only ever sends `Op::Record`'s shape (no `sels` /
/// `RecordBulk` tag at all). Adding the new `RecordBulk` variant to `Op` must
/// not disturb decoding that old shape — `#[serde(tag = "op")]` dispatches
/// purely on the `"op"` discriminant string, but this locks the invariant in
/// with a literal hand-written wire payload rather than relying on that
/// reasoning alone.
#[test]
fn op_record_old_wire_shape_still_deserializes() {
    let json = r#"{
        "esm": "Game.esm",
        "op": {
            "op": "record",
            "sel": {"kind": "form_id", "value": 4667},
            "depth": "none"
        }
    }"#;
    let req: Request = serde_json::from_str(json).expect("deserialize");
    match req.op {
        Op::Record { sel, depth } => {
            match sel {
                RecordSel::FormId(f) => assert_eq!(f, esm::FormId(4667)),
                other => panic!("expected FormId, got {other:?}"),
            }
            assert_eq!(depth, ResolveDepth::None);
        }
        other => panic!("expected Op::Record, got {other:?}"),
    }
}

/// Build a small ESM with two WEAP records, each carrying a distinct EDID —
/// used by the `Op::RecordBulk` tests below to exercise a mix of FormID and
/// EditorID selectors (and, with a bogus FormID thrown in, per-selector
/// failure isolation).
fn make_bulk_test_esm() -> Vec<u8> {
    let mut rec1_subs = Vec::new();
    append_subrecord(&mut rec1_subs, b"EDID", &cstr("AssaultRifle"));
    let mut rec2_subs = Vec::new();
    append_subrecord(&mut rec2_subs, b"EDID", &cstr("PipePistol"));

    let mut recs = Vec::new();
    append_record(&mut recs, b"WEAP", 1, &rec1_subs);
    append_record(&mut recs, b"WEAP", 2, &rec2_subs);

    let mut buf = tes4_header();
    buf.extend(wrap_grup(b"WEAP", &recs));
    buf
}

/// `Op::RecordBulk` resolves a mix of FormID and EditorID selectors in one
/// call, preserving the requested order, and its JSON wire form round-trips —
/// the bulk counterpart to `record_sel_json_round_trip` / `dispatch_record_by_formid`
/// above.
#[test]
fn dispatch_record_bulk_mixed_selectors_round_trip() {
    let buf = make_bulk_test_esm();
    let tmp = unique_temp_path("ipc_bulk_mixed");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp esm");
        f.write_all(&buf).expect("write temp esm");
    }
    let reg = Registry::new();

    let op = Op::RecordBulk {
        sels: vec![
            RecordSel::FormId(esm::FormId(1)),
            RecordSel::Edid("PipePistol".to_string()),
        ],
        depth: ResolveDepth::None,
    };
    let req = Request {
        esm: tmp.clone(),
        op: op.clone(),
    };

    // Wire round-trip.
    let json = serde_json::to_string(&req).expect("serialize");
    let back: Request = serde_json::from_str(&json).expect("deserialize");
    let json2 = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json2, "round-trip mismatch");

    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let entries: Vec<BulkRecordEntry> = serde_json::from_value(data).expect("entries");
    assert_eq!(entries.len(), 2);

    assert_eq!(entries[0].sel, "0x00000001");
    assert!(entries[0].error.is_none());
    assert_eq!(entries[0].editor_id.as_deref(), Some("AssaultRifle"));
    assert!(entries[0].header.is_some());
    assert!(entries[0].fields.is_some());

    assert_eq!(entries[1].sel, "PipePistol");
    assert!(entries[1].error.is_none());
    assert_eq!(entries[1].editor_id.as_deref(), Some("PipePistol"));
    assert!(entries[1].header.is_some());
    assert!(entries[1].fields.is_some());

    let _ = std::fs::remove_file(&tmp);
}

/// One bad selector in a bulk request must not fail the whole call: the good
/// selectors on either side of it still decode normally, and the bad one gets
/// an isolated `error` entry in its place (same position in the output).
#[test]
fn dispatch_record_bulk_isolates_per_selector_failure() {
    let buf = make_bulk_test_esm();
    let tmp = unique_temp_path("ipc_bulk_failure");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp esm");
        f.write_all(&buf).expect("write temp esm");
    }
    let reg = Registry::new();

    let req = Request {
        esm: tmp.clone(),
        op: Op::RecordBulk {
            sels: vec![
                RecordSel::FormId(esm::FormId(1)),          // valid
                RecordSel::FormId(esm::FormId(0x00999999)), // bogus — not present
                RecordSel::Edid("PipePistol".to_string()),  // valid
                RecordSel::Edid("NoSuchEdid".to_string()),  // bogus — not present
            ],
            depth: ResolveDepth::None,
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let entries: Vec<BulkRecordEntry> = serde_json::from_value(data).expect("entries");
    assert_eq!(entries.len(), 4, "one entry per selector, order preserved");

    assert_eq!(entries[0].sel, "0x00000001");
    assert!(entries[0].error.is_none(), "valid FormID must succeed");
    assert!(entries[0].fields.is_some());

    assert_eq!(entries[1].sel, "0x00999999");
    assert!(
        entries[1].error.is_some(),
        "bogus FormID must produce an isolated error, not fail the whole call"
    );
    assert!(entries[1].fields.is_none());
    assert!(entries[1].header.is_none());

    assert_eq!(entries[2].sel, "PipePistol");
    assert!(entries[2].error.is_none(), "valid EditorID must succeed");
    assert!(entries[2].fields.is_some());

    assert_eq!(entries[3].sel, "NoSuchEdid");
    assert!(
        entries[3].error.is_some(),
        "bogus EditorID must produce an isolated error"
    );
    assert!(entries[3].fields.is_none());

    let _ = std::fs::remove_file(&tmp);
}

/// Build an ESM with a target WEAP(1) carrying an EDID, and a referencer
/// WEAP(2) whose `YNAM`/`ZNAM` (Sound - Pickup / Sound - Putdown) both point
/// at FormId(1) — the same field layout
/// `dispatch_referenced_by_with_type_filter_and_paths` above exercises. Used
/// to confirm `Op::RecordBulk` honors `ResolveDepth::Stub` the same way a
/// plain `Op::Record` lookup does.
fn make_bulk_stub_test_esm() -> Vec<u8> {
    let mut target_subs = Vec::new();
    append_subrecord(&mut target_subs, b"EDID", &cstr("TargetWeap"));

    let mut ref_subs = Vec::new();
    append_subrecord(&mut ref_subs, b"YNAM", &1u32.to_le_bytes());
    append_subrecord(&mut ref_subs, b"ZNAM", &1u32.to_le_bytes());

    let mut recs = Vec::new();
    append_record(&mut recs, b"WEAP", 1, &target_subs);
    append_record(&mut recs, b"WEAP", 2, &ref_subs);

    let mut buf = tes4_header();
    buf.extend(wrap_grup(b"WEAP", &recs));
    buf
}

/// `Op::RecordBulk` with `depth: ResolveDepth::Stub` must annotate FormID
/// references inline for every record in the batch, exactly like `--resolve
/// stub` does for a single `esm get` — the mode the `/patch-notes` deep
/// agents rely on to avoid follow-up lookups.
#[test]
fn dispatch_record_bulk_with_resolve_stub_annotates_references() {
    let buf = make_bulk_stub_test_esm();
    let tmp = unique_temp_path("ipc_bulk_stub");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp esm");
        f.write_all(&buf).expect("write temp esm");
    }
    let reg = Registry::new();

    let req = Request {
        esm: tmp.clone(),
        op: Op::RecordBulk {
            sels: vec![
                RecordSel::FormId(esm::FormId(2)),
                RecordSel::FormId(esm::FormId(1)),
            ],
            depth: ResolveDepth::Stub,
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let entries: Vec<BulkRecordEntry> = serde_json::from_value(data).expect("entries");
    assert_eq!(entries.len(), 2);

    let referencer = &entries[0];
    assert_eq!(referencer.sel, "0x00000002");
    assert!(referencer.error.is_none());
    let fields = referencer.fields.as_ref().expect("fields present");
    for field in ["Sound - Pickup", "Sound - Putdown"] {
        let stub = fields
            .get(field)
            .unwrap_or_else(|| panic!("missing '{field}'"));
        assert_eq!(
            stub.get("editor_id").and_then(|v| v.as_str()),
            Some("TargetWeap"),
            "'{field}' must resolve to the target's EditorID under --resolve stub"
        );
        assert_eq!(
            stub.get("record_type").and_then(|v| v.as_str()),
            Some("WEAP")
        );
        assert_eq!(
            stub.get("formid").and_then(|v| v.as_str()),
            Some("0x00000001")
        );
    }

    // Second entry is the plain target record itself.
    assert_eq!(entries[1].sel, "0x00000001");
    assert!(entries[1].error.is_none());
    assert_eq!(entries[1].editor_id.as_deref(), Some("TargetWeap"));

    let _ = std::fs::remove_file(&tmp);
}
