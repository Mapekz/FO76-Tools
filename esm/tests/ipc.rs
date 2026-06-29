mod common;

use common::{make_minimal_esm, unique_temp_path};
use esm::ipc::{dispatch, Op, RecordSel, Request, Response};
use esm::registry::Registry;
use esm::{Database, ResolveDepth, SearchField};
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
