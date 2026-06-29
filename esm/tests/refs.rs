mod common;

use common::{make_xref_esm, unique_temp_path};
use esm::ipc::{referenced_by_enriched, RefList};
use esm::{Database, FormId};
use std::io::Write;

/// Verify that `Database::referenced_by` returns each referencing record
/// **exactly once**, even when that record references the target FormID in
/// multiple subrecords.
///
/// The `make_xref_esm` fixture contains:
///   - WEAP form_id=1  (target — no subrecords)
///   - WEAP form_id=2  (referencer — YNAM and ZNAM both pointing at form_id=1)
///
/// Before the dedup fix, two `RecordRow`s for form_id=2 would be returned.
/// After the fix, exactly one must appear.
#[test]
fn referenced_by_deduplicates_within_record() {
    let buf = make_xref_esm();
    let tmp = unique_temp_path("refs");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp esm");
        f.write_all(&buf).expect("write temp esm");
    }

    let mut db = Database::open(&tmp).expect("open db");
    let rows = db.referenced_by(FormId(1)).expect("referenced_by");

    assert_eq!(
        rows.len(),
        1,
        "expected exactly 1 referencing record, got {} — \
         each record must appear once even if it references the target \
         FormID multiple times; rows: {rows:#?}",
        rows.len()
    );
    assert_eq!(
        rows[0].form_id,
        FormId(2).display(),
        "the sole referencing record should be form_id=2"
    );

    let _ = std::fs::remove_file(&tmp);
}

// ── Helpers for building synthetic chain ESMs ────────────────────────────────

const FORM_VERSION: u16 = 208;

fn append_subrecord(out: &mut Vec<u8>, sig: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
}

fn edid_bytes(name: &str) -> Vec<u8> {
    let mut v = name.as_bytes().to_vec();
    v.push(0);
    v
}

fn append_record(out: &mut Vec<u8>, sig: &[u8; 4], form_id: u32, subrecords: &[u8]) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&(subrecords.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.extend_from_slice(&form_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // vcs1
    out.extend_from_slice(&FORM_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // vcs2
    out.extend_from_slice(subrecords);
}

fn build_misc(form_id: u32, edid: &str) -> Vec<u8> {
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &edid_bytes(edid));
    let mut rec = Vec::new();
    append_record(&mut rec, b"MISC", form_id, &subs);
    rec
}

fn build_lvli(form_id: u32, edid: &str, item_ref: u32) -> Vec<u8> {
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &edid_bytes(edid));
    append_subrecord(&mut subs, b"LLCT", &[1u8]);
    append_subrecord(&mut subs, b"LVLO", &item_ref.to_le_bytes());
    let mut rec = Vec::new();
    append_record(&mut rec, b"LVLI", form_id, &subs);
    rec
}

fn build_cont(form_id: u32, edid: &str, item_ref: u32) -> Vec<u8> {
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &edid_bytes(edid));
    append_subrecord(&mut subs, b"COCT", &1u32.to_le_bytes());
    let mut cnto = item_ref.to_le_bytes().to_vec();
    cnto.extend_from_slice(&1i32.to_le_bytes());
    append_subrecord(&mut subs, b"CNTO", &cnto);
    let mut rec = Vec::new();
    append_record(&mut rec, b"CONT", form_id, &subs);
    rec
}

fn wrap_grup(label: &[u8; 4], records: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let group_size = (24 + records.len()) as u32;
    buf.extend_from_slice(b"GRUP");
    buf.extend_from_slice(&group_size.to_le_bytes());
    buf.extend_from_slice(label);
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(records);
    buf
}

fn tes4_header() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"TES4");
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf
}

/// Build a 3-hop chain: MISC(1) ← LVLI(2) ← LVLI(3) ← CONT(4).
///
/// ```
/// depth=1 from MISC(1) → [LVLI(2)]
/// depth=2 from MISC(1) → [LVLI(2), LVLI(3)]
/// depth=3 from MISC(1) → [LVLI(2), LVLI(3), CONT(4)]
/// ```
fn make_chain_esm() -> Vec<u8> {
    let mut buf = tes4_header();
    buf.extend(wrap_grup(b"MISC", &build_misc(1, "TestItem")));
    let mut lvli_group = build_lvli(2, "InnerList", 1);
    lvli_group.extend(build_lvli(3, "OuterList", 2));
    buf.extend(wrap_grup(b"LVLI", &lvli_group));
    buf.extend(wrap_grup(b"CONT", &build_cont(4, "TestContainer", 3)));
    buf
}

fn open_chain_db() -> (std::path::PathBuf, Database) {
    let buf = make_chain_esm();
    let tmp = unique_temp_path("refs_chain");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp file");
        f.write_all(&buf).expect("write");
    }
    let db = Database::open(&tmp).expect("open");
    (tmp, db)
}

// ── recursive refs tests ─────────────────────────────────────────────────────

/// depth=1 yields exactly the direct referencers (single-level, today's behaviour).
#[test]
fn recursive_refs_depth1_matches_direct() {
    let (path, mut db) = open_chain_db();

    // Single-level old path
    let direct = db.referenced_by(FormId(1)).expect("referenced_by");
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].form_id, FormId(2).display());

    // New BFS path at depth=1
    let list: RefList = referenced_by_enriched(&mut db, FormId(1), 1, 0).expect("enriched");
    assert_eq!(list.rows.len(), 1);
    assert_eq!(list.rows[0].form_id, FormId(2).display());
    assert_eq!(list.rows[0].depth, 1);
    assert!(
        list.rows[0].path.is_empty(),
        "depth-1 row must have empty path"
    );

    let _ = std::fs::remove_file(&path);
}

/// depth=2 follows one hop beyond the direct referencers.
#[test]
fn recursive_refs_depth2_follows_one_extra_hop() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(&mut db, FormId(1), 2, 0).expect("enriched");
    // Expect LVLI(2) at depth=1 and LVLI(3) at depth=2.
    assert_eq!(
        list.rows.len(),
        2,
        "expected 2 rows at depth=2, got: {list:?}"
    );

    let row2 = list
        .rows
        .iter()
        .find(|r| r.form_id == FormId(2).display())
        .unwrap();
    assert_eq!(row2.depth, 1);
    assert!(row2.path.is_empty());

    let row3 = list
        .rows
        .iter()
        .find(|r| r.form_id == FormId(3).display())
        .unwrap();
    assert_eq!(row3.depth, 2);
    assert_eq!(
        row3.path.len(),
        1,
        "depth-2 row should carry the depth-1 intermediate"
    );
    assert_eq!(row3.path[0].form_id, FormId(2).display());

    let _ = std::fs::remove_file(&path);
}

/// depth=6 (or any depth ≥ 3) reaches all nodes in the 3-hop chain.
#[test]
fn recursive_refs_depth6_reaches_all_hops() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(&mut db, FormId(1), 6, 0).expect("enriched");
    assert_eq!(list.rows.len(), 3, "expected LVLI(2)+LVLI(3)+CONT(4)");

    let ids: Vec<_> = list.rows.iter().map(|r| r.form_id.as_str()).collect();
    assert!(ids.contains(&FormId(2).display().as_str()));
    assert!(ids.contains(&FormId(3).display().as_str()));
    assert!(ids.contains(&FormId(4).display().as_str()));

    // Verify path lengths: CONT(4) should have 2 intermediates [LVLI(2), LVLI(3)].
    let cont = list
        .rows
        .iter()
        .find(|r| r.form_id == FormId(4).display())
        .unwrap();
    assert_eq!(cont.depth, 3);
    assert_eq!(cont.path.len(), 2);

    let _ = std::fs::remove_file(&path);
}

/// depth=0 is clamped to 1 — behaves like depth=1.
#[test]
fn recursive_refs_depth0_clamps_to_1() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(&mut db, FormId(1), 0, 0).expect("enriched");
    assert_eq!(list.rows.len(), 1, "depth=0 should clamp to 1 direct ref");
    assert_eq!(list.rows[0].form_id, FormId(2).display());

    let _ = std::fs::remove_file(&path);
}

/// depth cap terminates the walk at max_depth even if more hops exist.
#[test]
fn recursive_refs_depth_cap_terminates() {
    let (path, mut db) = open_chain_db();

    // depth=2 stops before reaching CONT(4) which is 3 hops away.
    let list = referenced_by_enriched(&mut db, FormId(1), 2, 0).expect("enriched");
    let ids: Vec<_> = list.rows.iter().map(|r| r.form_id.as_str()).collect();
    assert!(
        !ids.contains(&FormId(4).display().as_str()),
        "CONT(4) must not appear at depth=2"
    );
    assert_eq!(list.rows.len(), 2);

    let _ = std::fs::remove_file(&path);
}

/// Cycle guard: a→b→a does not loop and each node appears exactly once.
///
/// Graph: WEAP(1) ← WEAP(2) ← WEAP(1) [cycle via cross-references]
/// We build: WEAP(1) has a FormID subrecord pointing at WEAP(2),
///           WEAP(2) has a FormID subrecord pointing at WEAP(1).
/// So referenced_by(WEAP(1)) = [WEAP(2)] and referenced_by(WEAP(2)) = [WEAP(1)].
/// With depth=6 the BFS should return WEAP(2) exactly once (target WEAP(1) is
/// excluded from results, breaking the cycle).
#[test]
fn recursive_refs_cycle_guard() {
    fn formid_subrecord(sig: &[u8; 4], fid: u32) -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(sig);
        s.extend_from_slice(&4u16.to_le_bytes());
        s.extend_from_slice(&fid.to_le_bytes());
        s
    }

    // WEAP(1) references WEAP(2) via YNAM; WEAP(2) references WEAP(1) via YNAM.
    let subs1 = formid_subrecord(b"YNAM", 2);
    let subs2 = formid_subrecord(b"YNAM", 1);
    let data_size1 = subs1.len() as u32;
    let data_size2 = subs2.len() as u32;
    let rec1_size = 24 + data_size1;
    let rec2_size = 24 + data_size2;
    let group_size = 24 + rec1_size + rec2_size;

    let mut buf = tes4_header();
    // GRUP header
    buf.extend_from_slice(b"GRUP");
    buf.extend_from_slice(&group_size.to_le_bytes());
    buf.extend_from_slice(&u32::from_le_bytes(*b"WEAP").to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    // WEAP(1)
    buf.extend_from_slice(b"WEAP");
    buf.extend_from_slice(&data_size1.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&subs1);
    // WEAP(2)
    buf.extend_from_slice(b"WEAP");
    buf.extend_from_slice(&data_size2.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&subs2);

    let tmp = unique_temp_path("refs_cycle");
    {
        let mut f = std::fs::File::create(&tmp).expect("create");
        f.write_all(&buf).expect("write");
    }
    let mut db = Database::open(&tmp).expect("open");

    let list = referenced_by_enriched(&mut db, FormId(1), 6, 0).expect("enriched");

    // Only WEAP(2) should appear — WEAP(1) is the target and excluded from results.
    // The cycle WEAP(1)→WEAP(2)→WEAP(1) must not cause WEAP(1) to appear as a result.
    let ids: Vec<_> = list.rows.iter().map(|r| r.form_id.as_str()).collect();
    assert_eq!(list.rows.len(), 1, "only WEAP(2) expected, got: {ids:?}");
    assert_eq!(list.rows[0].form_id, FormId(2).display());

    let _ = std::fs::remove_file(&tmp);
}

/// limit cap: when limit > 0, total reflects the real count and capped=true.
#[test]
fn recursive_refs_limit_caps_output() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(&mut db, FormId(1), 6, 1).expect("enriched");
    assert_eq!(list.rows.len(), 1, "limit=1 should cap to 1 row");
    assert_eq!(list.total, 3, "total should reflect the full depth=6 count");
    assert!(list.capped, "capped flag should be set");

    let _ = std::fs::remove_file(&path);
}
