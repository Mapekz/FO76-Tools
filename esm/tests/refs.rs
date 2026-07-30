mod common;

use common::{make_xref_esm, unique_temp_path};
use esm::ipc::{
    Op, RecordSel, RefList, dispatch_op, find_ref_path, referenced_by_enriched,
    referenced_by_enriched_multi, resolve_sel,
};
use esm::{Database, EntryPointRef, EntryPointSpec, FormId};
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
    build_lvli_multi(form_id, edid, &[item_ref])
}

fn build_lvli_multi(form_id: u32, edid: &str, item_refs: &[u32]) -> Vec<u8> {
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &edid_bytes(edid));
    append_subrecord(&mut subs, b"LLCT", &[item_refs.len() as u8]);
    for &item_ref in item_refs {
        append_subrecord(&mut subs, b"LVLO", &item_ref.to_le_bytes());
    }
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
    let list: RefList = referenced_by_enriched(
        &mut db,
        FormId(1),
        1,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
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

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        2,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
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

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
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

    // The chain terminates at 3 hops, well within depth=6 — no frontier left
    // unexpanded, so this result is a complete closure, not a truncated one.
    assert_eq!(list.requested_depth, 6);
    assert_eq!(list.effective_depth, Some(6));
    assert!(!list.depth_capped, "graph terminates before the cap");
    assert_eq!(list.frontier_remaining, 0);

    let _ = std::fs::remove_file(&path);
}

/// depth=0 requests an unbounded walk — it must reach exactly as deep as the
/// full chain, not clamp to 1 (the old semantics) or to DEFAULT_MAX_DEPTH.
#[test]
fn recursive_refs_depth0_is_unbounded() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        0,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(
        list.rows.len(),
        3,
        "depth=0 (unbounded) must reach LVLI(2)+LVLI(3)+CONT(4), same as depth=6"
    );
    let ids: Vec<_> = list.rows.iter().map(|r| r.form_id.as_str()).collect();
    assert!(ids.contains(&FormId(4).display().as_str()));

    assert_eq!(list.requested_depth, 0);
    assert_eq!(
        list.effective_depth, None,
        "unbounded walk has no fixed cap to report"
    );
    assert!(!list.depth_capped);
    assert_eq!(list.frontier_remaining, 0);

    let _ = std::fs::remove_file(&path);
}

/// depth cap terminates the walk at max_depth even if more hops exist, and
/// the result reports that it did so via depth_capped/frontier_remaining.
#[test]
fn recursive_refs_depth_cap_terminates() {
    let (path, mut db) = open_chain_db();

    // depth=2 stops before reaching CONT(4) which is 3 hops away.
    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        2,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    let ids: Vec<_> = list.rows.iter().map(|r| r.form_id.as_str()).collect();
    assert!(
        !ids.contains(&FormId(4).display().as_str()),
        "CONT(4) must not appear at depth=2"
    );
    assert_eq!(list.rows.len(), 2);

    assert_eq!(list.requested_depth, 2);
    assert_eq!(list.effective_depth, Some(2));
    assert!(
        list.depth_capped,
        "LVLI(3) has an unexpanded referencer (CONT(4)) beyond the cap"
    );
    assert!(list.frontier_remaining >= 1);

    let _ = std::fs::remove_file(&path);
}

/// `per_depth_totals` reflects the full walk (pre-`--limit`), and
/// `shown_max_depth` reflects only what survived truncation.
#[test]
fn recursive_refs_reports_per_depth_totals_and_shown_max_depth() {
    let (path, mut db) = open_chain_db();

    let full = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    // index 0 = carrier rows (none on a direct-target walk), 1 = LVLI(2),
    // 2 = LVLI(3), 3 = CONT(4).
    assert_eq!(full.per_depth_totals, vec![0, 1, 1, 1]);
    assert_eq!(full.shown_max_depth, 3);

    // limit=1 keeps only the shallowest row (FormID-sorted), but
    // per_depth_totals must still reflect all 3 rows found pre-truncation.
    let limited = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        1,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(limited.rows.len(), 1);
    assert_eq!(
        limited.per_depth_totals,
        vec![0, 1, 1, 1],
        "per_depth_totals is computed before --limit truncation"
    );
    assert_eq!(
        limited.shown_max_depth, 1,
        "only the depth-1 row survived the limit=1 truncation"
    );

    let _ = std::fs::remove_file(&path);
}

/// Build a 2-hop chain where FormID order and depth order disagree:
/// MISC(1) ← LVLI(50) [depth 1] ← LVLI(2) [depth 2].
/// FormID-ascending puts LVLI(2) first; depth-ascending puts LVLI(50) first.
fn make_sort_order_esm() -> Vec<u8> {
    let mut buf = tes4_header();
    buf.extend(wrap_grup(b"MISC", &build_misc(1, "Target")));
    let mut lvli_group = build_lvli(50, "ShallowHighId", 1);
    lvli_group.extend(build_lvli(2, "DeepLowId", 50));
    buf.extend(wrap_grup(b"LVLI", &lvli_group));
    buf
}

/// `--sort formid` (the default) keeps FormID-ascending order even when it
/// disagrees with hop depth; `--sort depth` reorders to breadth-first.
#[test]
fn recursive_refs_sort_depth_reorders_relative_to_formid() {
    let buf = make_sort_order_esm();
    let tmp = unique_temp_path("refs_sort_order");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp file");
        f.write_all(&buf).expect("write");
    }
    let mut db = Database::open(&tmp).expect("open");

    let by_formid = referenced_by_enriched(
        &mut db,
        FormId(1),
        2,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    let formid_order: Vec<_> = by_formid.rows.iter().map(|r| r.form_id.clone()).collect();
    assert_eq!(
        formid_order,
        vec![FormId(2).display(), FormId(50).display()],
        "default sort is FormID-ascending regardless of depth"
    );

    let by_depth = referenced_by_enriched(
        &mut db,
        FormId(1),
        2,
        0,
        None,
        false,
        esm::ipc::RefSort::Depth,
    )
    .expect("enriched");
    let depth_order: Vec<_> = by_depth.rows.iter().map(|r| r.form_id.clone()).collect();
    assert_eq!(
        depth_order,
        vec![FormId(50).display(), FormId(2).display()],
        "--sort depth yields a breadth-first (depth, form_id) order instead"
    );
    assert_eq!(by_depth.rows[0].depth, 1);
    assert_eq!(by_depth.rows[1].depth, 2);

    let _ = std::fs::remove_file(&tmp);
}

// ── find_ref_path (bidirectional search) tests ──────────────────────────────

/// The chain MISC(1) ← LVLI(2) ← LVLI(3) ← CONT(4) is exactly the shape
/// `find_ref_path` should discover: from=1 (the "referenced" endpoint),
/// to=4 (a transitive referencer), 3 hops.
#[test]
fn find_ref_path_discovers_known_chain() {
    let (path, mut db) = open_chain_db();

    let result = find_ref_path(&mut db, FormId(1), FormId(4), 0, false).expect("find_ref_path");
    assert_eq!(result.from, FormId(1).display());
    assert_eq!(result.to, FormId(4).display());
    assert_eq!(result.hops, Some(3));
    assert!(!result.budget_exhausted);
    let chain = result.chain.expect("path should be found");
    let ids: Vec<_> = chain.iter().map(|h| h.form_id.clone()).collect();
    assert_eq!(
        ids,
        vec![
            FormId(1).display(),
            FormId(2).display(),
            FormId(3).display(),
            FormId(4).display(),
        ]
    );

    let _ = std::fs::remove_file(&path);
}

/// `from == to` is a trivial 0-hop chain containing just that one node.
#[test]
fn find_ref_path_trivial_when_from_equals_to() {
    let (path, mut db) = open_chain_db();

    let result = find_ref_path(&mut db, FormId(1), FormId(1), 0, false).expect("find_ref_path");
    assert_eq!(result.hops, Some(0));
    let chain = result.chain.expect("trivial path should be found");
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].form_id, FormId(1).display());

    let _ = std::fs::remove_file(&path);
}

/// The chain relation is directional: CONT(4) does not transitively
/// reference MISC(1) (only the reverse holds), so asking for a path from
/// CONT to MISC must report "not found", not silently succeed by walking
/// the edges backwards.
#[test]
fn find_ref_path_is_directional_not_found_in_reverse() {
    let (path, mut db) = open_chain_db();

    let result = find_ref_path(&mut db, FormId(4), FormId(1), 0, false).expect("find_ref_path");
    assert_eq!(result.chain, None);
    assert_eq!(result.hops, None);
    assert!(
        !result.budget_exhausted,
        "the search space is tiny and fully exhausted, not budget-limited"
    );

    let _ = std::fs::remove_file(&path);
}

/// A `max_hops` ceiling smaller than the actual chain length reports "not
/// found" rather than a partial/incorrect chain.
#[test]
fn find_ref_path_respects_max_hops_ceiling() {
    let (path, mut db) = open_chain_db();

    // The real chain from 1 to 4 is 3 hops; a ceiling of 2 must not find it.
    let result = find_ref_path(&mut db, FormId(1), FormId(4), 2, false).expect("find_ref_path");
    assert_eq!(result.chain, None);
    assert!(!result.budget_exhausted);

    // Raising the ceiling to the exact chain length finds it again.
    let result = find_ref_path(&mut db, FormId(1), FormId(4), 3, false).expect("find_ref_path");
    assert_eq!(result.hops, Some(3));

    let _ = std::fs::remove_file(&path);
}

/// `include_paths` annotates every hop but the first with the JSON field
/// path where it references its predecessor — the same convention
/// `referenced_by_walk`'s `--paths` uses.
#[test]
fn find_ref_path_paths_annotate_every_hop_but_the_first() {
    let (path, mut db) = open_chain_db();

    let result = find_ref_path(&mut db, FormId(1), FormId(4), 0, true).expect("find_ref_path");
    let chain = result.chain.expect("path should be found");
    assert_eq!(
        chain[0].field_paths, None,
        "the first hop has no predecessor"
    );
    for hop in &chain[1..] {
        let fp = hop
            .field_paths
            .as_ref()
            .expect("later hops get field_paths");
        assert!(
            !fp.is_empty(),
            "each LVLI's LVLO entry should be a locatable field path"
        );
    }

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

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");

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

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        1,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(list.rows.len(), 1, "limit=1 should cap to 1 row");
    assert_eq!(list.total, 3, "total should reflect the full depth=6 count");
    assert!(list.capped, "capped flag should be set");

    let _ = std::fs::remove_file(&path);
}

// ── --paths tests (P2) ───────────────────────────────────────────────────────

/// `include_paths = false` (the default fast walk) never populates
/// `field_paths` — it must stay `None` so it's omitted from serialized JSON.
#[test]
fn field_paths_none_when_not_requested() {
    let buf = make_xref_esm();
    let tmp = unique_temp_path("refs_paths_off");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp esm");
        f.write_all(&buf).expect("write temp esm");
    }
    let mut db = Database::open(&tmp).expect("open db");

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        1,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(list.rows.len(), 1);
    assert!(
        list.rows[0].field_paths.is_none(),
        "field_paths must be None when --paths is not requested"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// `include_paths = true` decodes the referencing record and reports every
/// JSON path where the target FormID appears. WEAP(2) in `make_xref_esm`
/// references WEAP(1) via both YNAM ("Sound - Pickup") and ZNAM
/// ("Sound - Putdown") — both paths must be reported, in schema member order.
#[test]
fn field_paths_finds_all_occurrences_in_one_record() {
    let buf = make_xref_esm();
    let tmp = unique_temp_path("refs_paths_multi");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp esm");
        f.write_all(&buf).expect("write temp esm");
    }
    let mut db = Database::open(&tmp).expect("open db");

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        1,
        0,
        None,
        true,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(list.rows.len(), 1);
    assert_eq!(
        list.rows[0].field_paths,
        Some(vec![
            "Sound - Pickup".to_string(),
            "Sound - Putdown".to_string()
        ]),
        "expected both FormID-reference fields, got: {:?}",
        list.rows[0].field_paths
    );

    let _ = std::fs::remove_file(&tmp);
}

/// `Database::formid_reference_paths` is what `--paths` calls per row — verify
/// it directly against a real decoded LVLI record from the chain fixture.
/// LVLI(2) references MISC(1) through its single `LVLO` leveled-list entry.
#[test]
fn formid_reference_paths_locates_array_element_field() {
    let (path, db) = open_chain_db();

    let paths = db.formid_reference_paths(FormId(2), FormId(1));
    assert_eq!(
        paths,
        vec!["Leveled List Entries[0].Leveled List Entry.Reference".to_string()],
        "unexpected path(s): {paths:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// A referencer with no `meta` (unknown FormID) returns an empty vec rather
/// than erroring — `--paths` is best-effort enrichment.
#[test]
fn formid_reference_paths_unknown_referencer_returns_empty() {
    let (path, db) = open_chain_db();

    let paths = db.formid_reference_paths(FormId(0xDEAD_BEEF), FormId(1));
    assert!(paths.is_empty());

    let _ = std::fs::remove_file(&path);
}

// ── --type tests (P3) ────────────────────────────────────────────────────────

/// `type_filter` narrows emitted rows to the matching type but the walk keeps
/// traversing through non-matching nodes — CONT(4) (3 hops away, behind two
/// LVLI hops) must still be reachable when filtering for "CONT".
#[test]
fn type_filter_narrows_rows_but_keeps_traversing() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        0,
        Some("CONT"),
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(
        list.rows.len(),
        1,
        "only CONT(4) should survive the filter, got: {list:?}"
    );
    assert_eq!(list.rows[0].form_id, FormId(4).display());
    assert_eq!(list.rows[0].record_type.as_deref(), Some("CONT"));

    let _ = std::fs::remove_file(&path);
}

/// `type_filter` is case-insensitive.
#[test]
fn type_filter_case_insensitive() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        0,
        Some("cont"),
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(list.rows.len(), 1);
    assert_eq!(list.rows[0].form_id, FormId(4).display());

    let _ = std::fs::remove_file(&path);
}

/// `limit`/`total`/`capped` are computed against the *filtered* set, not the
/// pre-filter walk — this is the "server-side filter so limits/depth interact
/// correctly" requirement from the P3 backlog item.
#[test]
fn type_filter_limit_and_total_apply_post_filter() {
    let (path, mut db) = open_chain_db();

    // Unfiltered depth=6 walk has 3 rows (LVLI, LVLI, CONT); filtering to LVLI
    // only should report total=2, not 3, and limit=1 should cap that to 1.
    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        1,
        Some("LVLI"),
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(list.total, 2, "total must reflect the filtered count");
    assert_eq!(list.rows.len(), 1, "limit=1 should cap the filtered set");
    assert!(list.capped);
    assert_eq!(list.rows[0].record_type.as_deref(), Some("LVLI"));

    let _ = std::fs::remove_file(&path);
}

/// A type signature that isn't exactly 4 characters is rejected up front.
#[test]
fn type_filter_rejects_non_4char_signature() {
    let (path, mut db) = open_chain_db();

    let err = referenced_by_enriched(
        &mut db,
        FormId(1),
        1,
        0,
        Some("LV"),
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect_err("expected validation error for non-4-char type");
    assert!(
        err.to_string().contains("4-character"),
        "unexpected error message: {err}"
    );

    let _ = std::fs::remove_file(&path);
}

/// `--type` and `--paths` compose: filtered rows still get field_paths.
#[test]
fn type_filter_and_paths_compose() {
    let (path, mut db) = open_chain_db();

    let list = referenced_by_enriched(
        &mut db,
        FormId(1),
        6,
        0,
        Some("CONT"),
        true,
        esm::ipc::RefSort::Formid,
    )
    .expect("enriched");
    assert_eq!(list.rows.len(), 1);
    assert_eq!(
        list.rows[0].field_paths,
        Some(vec!["Items[0].Item.Item.Item".to_string()]),
        "unexpected path(s): {:?}",
        list.rows[0].field_paths
    );

    let _ = std::fs::remove_file(&path);
}

// ── Entry-point lookup (`refs --entry-point`/`--ep`) ─────────────────────────

/// Build a minimal PERK record with one "Entry Point"-typed effect per id in
/// `entry_points`: a `PRKE` (Effect Type=2 "Entry Point", Rank=0), then a
/// `DATA` (the Entry Point struct: id, Function=0, Perk Condition Tab
/// Count=0, unused=0), then a `PRKF` (empty Effect End). No `EPFT`/`EPFB`/
/// `EPFD` — those are optional trailers unrelated to entry-point *identity*,
/// which lives entirely in the `DATA` union's "Entry Point" variant (verified
/// against the real embedded schema: id 39 decodes to
/// `{"value":39,"name":"Mod Percent Blocked"}` from exactly this shape,
/// matching live `esm get` output byte-for-byte). An `entry_points` slice
/// with a repeated id builds one perk with several effects on the *same*
/// entry point — the multi-effect dedup case.
fn build_perk_entry_points(form_id: u32, edid: &str, entry_points: &[u8]) -> Vec<u8> {
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &edid_bytes(edid));
    append_subrecord(&mut subs, b"DATA", &[0x01, 0x00, 0x01]); // top-level Data: Playable/Hidden/Unknown
    for &ep in entry_points {
        append_subrecord(&mut subs, b"PRKE", &[0x02, 0x00]);
        append_subrecord(&mut subs, b"DATA", &[ep, 0x00, 0x00, 0x00]);
        append_subrecord(&mut subs, b"PRKF", &[]);
    }
    let mut rec = Vec::new();
    append_record(&mut rec, b"PERK", form_id, &subs);
    rec
}

/// PERKs:
///   10 CarrierA          — entry point 39 (Mod Percent Blocked)
///   11 CarrierB          — entry point 39 (Mod Percent Blocked)
///   12 OtherPerk         — entry point 40 (Mod Shield Deflect Arrow Chance)
///   13 UnnamedPerk       — entry point 212 (outside the 212-entry name
///                          table — real game data has exactly this case,
///                          `mod_custom_V63-BERTHA_Perk`)
///   14 MultiEffectPerk   — entry point 39 on *two* separate effects
///   15 GlobA             — entry point 41 (Mod Incoming Spell Magnitude)
///   16 GlobB             — entry point 42 (Mod Incoming Spell Duration)
///   17 GlobBoth          — entry points 41 *and* 42 (multi-EP carrier)
/// Referencers:
///   30 CONT RefCont      — → CarrierA/10
///   31 LVLI RefList      — → CarrierA/10
///   32 CONT RefContB     — → CarrierB/11
///   33 LVLI SharedDeep   — → RefCont/30 and RefContB/32 (depth 2 from both
///                          CarrierA and CarrierB — equal-depth union case)
///   34 LVLI SharedGlob   — → GlobA/15 and GlobB/16 (depth 1 from both;
///                          equal-depth union of distinct EPs 41+42)
fn make_entry_point_esm() -> Vec<u8> {
    let mut buf = tes4_header();
    let mut perk_group = build_perk_entry_points(10, "CarrierA", &[39]);
    perk_group.extend(build_perk_entry_points(11, "CarrierB", &[39]));
    perk_group.extend(build_perk_entry_points(12, "OtherPerk", &[40]));
    perk_group.extend(build_perk_entry_points(13, "UnnamedPerk", &[212]));
    perk_group.extend(build_perk_entry_points(14, "MultiEffectPerk", &[39, 39]));
    perk_group.extend(build_perk_entry_points(15, "GlobA", &[41]));
    perk_group.extend(build_perk_entry_points(16, "GlobB", &[42]));
    perk_group.extend(build_perk_entry_points(17, "GlobBoth", &[41, 42]));
    buf.extend(wrap_grup(b"PERK", &perk_group));
    let mut cont_group = build_cont(30, "RefCont", 10);
    cont_group.extend(build_cont(32, "RefContB", 11));
    buf.extend(wrap_grup(b"CONT", &cont_group));
    let mut lvli_group = build_lvli(31, "RefList", 10);
    lvli_group.extend(build_lvli_multi(33, "SharedDeep", &[30, 32]));
    lvli_group.extend(build_lvli_multi(34, "SharedGlob", &[15, 16]));
    buf.extend(wrap_grup(b"LVLI", &lvli_group));
    buf
}

/// Helper: wrap bare FormIDs as tagged seeds for [`referenced_by_enriched_multi`].
fn seeds_with_ep(ids: &[(u32, u16)]) -> Vec<(FormId, Vec<EntryPointRef>)> {
    ids.iter()
        .map(|&(fid, ep)| (FormId(fid), vec![EntryPointRef { id: ep, name: None }]))
        .collect()
}

fn open_entry_point_db() -> (std::path::PathBuf, Database) {
    let buf = make_entry_point_esm();
    let tmp = unique_temp_path("refs_entry_point");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp file");
        f.write_all(&buf).expect("write");
    }
    let db = Database::open(&tmp).expect("open");
    (tmp, db)
}

/// A second, separate ESM for the EditorID-vs-entry-point-name collision
/// case: PERK 18's *EditorID* is literally the string `"Mod Percent
/// Blocked"`, while PERK 19 is an unrelated perk that genuinely carries
/// entry point 39 (named "Mod Percent Blocked"). Kept apart from
/// `make_entry_point_esm` so the collision doesn't also feed the plain
/// entry-point tests above.
fn make_edid_collision_esm() -> Vec<u8> {
    let mut buf = tes4_header();
    let mut perk_group = build_perk_entry_points(18, "Mod Percent Blocked", &[40]);
    perk_group.extend(build_perk_entry_points(19, "RealCarrier", &[39]));
    buf.extend(wrap_grup(b"PERK", &perk_group));
    buf
}

fn open_edid_collision_db() -> (std::path::PathBuf, Database) {
    let buf = make_edid_collision_esm();
    let tmp = unique_temp_path("refs_entry_point_collision");
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp file");
        f.write_all(&buf).expect("write");
    }
    let db = Database::open(&tmp).expect("open");
    (tmp, db)
}

#[test]
fn entry_point_spec_parse_numeric_name_and_rejects_formid_like() {
    assert_eq!(EntryPointSpec::parse("39").unwrap(), EntryPointSpec::Id(39));
    assert_eq!(
        EntryPointSpec::parse("  40  ").unwrap(),
        EntryPointSpec::Id(40),
        "whitespace must be trimmed"
    );
    assert_eq!(
        EntryPointSpec::parse("Mod Percent Blocked").unwrap(),
        EntryPointSpec::Name("Mod Percent Blocked".to_string())
    );
    assert!(
        EntryPointSpec::parse("0x1A").is_err(),
        "a 0x-prefixed token must be rejected, not silently coerced into a \
         (never-matching) name pattern"
    );
    assert!(
        EntryPointSpec::parse("0X1a").is_err(),
        "uppercase 0X prefix must also be rejected"
    );
}

/// `perks_by_entry_point` matches by numeric id and by case-insensitive
/// exact name (not substring), dedups a perk with multiple effects on the
/// same entry point down to one row, and excludes perks on other entry
/// points entirely.
#[test]
fn perks_by_entry_point_matches_by_id_and_exact_case_insensitive_name() {
    let (path, mut db) = open_entry_point_db();

    let (label, seeds) = db
        .perks_by_entry_point(&EntryPointSpec::Id(39))
        .expect("perks_by_entry_point");
    let seed_ids: Vec<FormId> = seeds.iter().map(|(f, _)| *f).collect();
    assert_eq!(
        seed_ids,
        vec![FormId(10), FormId(11), FormId(14)],
        "expected CarrierA, CarrierB, and MultiEffectPerk (once, not twice)"
    );
    assert!(
        label.contains("39") && label.contains("Mod Percent Blocked"),
        "unexpected label: {label}"
    );
    for (_, tags) in &seeds {
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].id, 39);
        assert_eq!(tags[0].name.as_deref(), Some("Mod Percent Blocked"));
    }

    let (_, seeds_by_name) = db
        .perks_by_entry_point(&EntryPointSpec::Name("mod percent blocked".to_string()))
        .expect("perks_by_entry_point by name");
    assert_eq!(
        seeds_by_name, seeds,
        "case-insensitive name lookup must agree with the numeric lookup"
    );

    let (_, seeds_partial) = db
        .perks_by_entry_point(&EntryPointSpec::Name("Mod Percent".to_string()))
        .expect("perks_by_entry_point partial (non-glob)");
    assert!(
        seeds_partial.is_empty(),
        "a non-glob name pattern must match exactly, not as a substring: {seeds_partial:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// A `*`-glob name pattern can match several distinct entry points at once.
#[test]
fn perks_by_entry_point_glob_matches_multiple_distinct_entry_points() {
    let (path, mut db) = open_entry_point_db();

    let (label, seeds) = db
        .perks_by_entry_point(&EntryPointSpec::Name("Mod Incoming Spell*".to_string()))
        .expect("perks_by_entry_point glob");
    let seed_ids: Vec<FormId> = seeds.iter().map(|(f, _)| *f).collect();
    // Sorted by (primary EP id, form_id): GlobA(41,15), GlobBoth(41,17), GlobB(42,16).
    assert_eq!(seed_ids, vec![FormId(15), FormId(17), FormId(16)]);
    assert!(
        label.contains("2 matched") && label.contains("41") && label.contains("42"),
        "glob label should enumerate every matched entry point: {label}"
    );
    let both = seeds
        .iter()
        .find(|(f, _)| *f == FormId(17))
        .expect("GlobBoth");
    assert_eq!(
        both.1.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![41, 42],
        "GlobBoth must carry both matched entry points"
    );

    let _ = std::fs::remove_file(&path);
}

/// An entry point id outside the schema's name table (212, one past the last
/// named entry) is only reachable by its numeric id — never by name, since
/// it has none.
#[test]
fn perks_by_entry_point_reaches_unnamed_id_only_by_number() {
    let (path, mut db) = open_entry_point_db();

    let (label, seeds) = db
        .perks_by_entry_point(&EntryPointSpec::Id(212))
        .expect("perks_by_entry_point unnamed id");
    let seed_ids: Vec<FormId> = seeds.iter().map(|(f, _)| *f).collect();
    assert_eq!(seed_ids, vec![FormId(13)]);
    assert!(label.contains("unnamed"), "unexpected label: {label}");

    let (_, seeds_by_name) = db
        .perks_by_entry_point(&EntryPointSpec::Name("Unknown 212".to_string()))
        .expect("perks_by_entry_point unnamed by (nonexistent) name");
    assert!(
        seeds_by_name.is_empty(),
        "an unnamed id has no name to match against: {seeds_by_name:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// [`referenced_by_enriched_multi`] emits every seed as its own `depth: 0`
/// row before the BFS-found referencer rows, and referencers of either seed
/// are found and deduped together.
#[test]
fn referenced_by_enriched_multi_emits_carriers_at_depth_zero_then_bfs() {
    let (path, mut db) = open_entry_point_db();

    let list = referenced_by_enriched_multi(
        &mut db,
        &seeds_with_ep(&[(10, 39), (11, 39)]),
        "entry point 39 (Mod Percent Blocked)".to_string(),
        1,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("referenced_by_enriched_multi");

    assert_eq!(list.target, "entry point 39 (Mod Percent Blocked)");
    // 2 carriers + RefCont(30) + RefList(31) + RefContB(32); SharedDeep is
    // depth 2 and excluded at depth=1.
    assert_eq!(
        list.total, 5,
        "2 carriers + 3 depth-1 referencers: {list:?}"
    );
    assert!(!list.capped);

    let carrier_a = &list.rows[0];
    assert_eq!(carrier_a.form_id, FormId(10).display());
    assert_eq!(carrier_a.depth, 0);
    assert!(carrier_a.path.is_empty());
    assert_eq!(carrier_a.entry_points.len(), 1);
    assert_eq!(carrier_a.entry_points[0].id, 39);

    let carrier_b = &list.rows[1];
    assert_eq!(carrier_b.form_id, FormId(11).display());
    assert_eq!(carrier_b.depth, 0);

    let referencer_ids: Vec<&str> = list.rows[2..].iter().map(|r| r.form_id.as_str()).collect();
    assert_eq!(
        referencer_ids,
        vec![
            FormId(30).display(),
            FormId(31).display(),
            FormId(32).display()
        ],
        "referencer rows must sort by FormID after the carrier rows"
    );
    assert!(list.rows[2..].iter().all(|r| r.depth == 1));
    // path[0] is the originating carrier in EP mode.
    let ref_cont = list
        .rows
        .iter()
        .find(|r| r.form_id == FormId(30).display())
        .unwrap();
    assert_eq!(ref_cont.path.len(), 1);
    assert_eq!(ref_cont.path[0].form_id, FormId(10).display());
    assert_eq!(ref_cont.entry_points[0].id, 39);

    let _ = std::fs::remove_file(&path);
}

/// `--type` narrows both the BFS referencer rows *and* the carrier rows —
/// a carrier that isn't of the requested type is excluded just like any
/// other row.
#[test]
fn referenced_by_enriched_multi_type_filter_applies_to_carriers_too() {
    let (path, mut db) = open_entry_point_db();

    let list = referenced_by_enriched_multi(
        &mut db,
        &seeds_with_ep(&[(10, 39), (11, 39)]),
        "entry point 39 (Mod Percent Blocked)".to_string(),
        1,
        0,
        Some("CONT"),
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("referenced_by_enriched_multi with type filter");

    assert_eq!(
        list.rows.len(),
        2,
        "both PERK carriers and the LVLI referencer must be filtered out; \
         CONT RefCont + RefContB remain: {list:?}"
    );
    let ids: Vec<&str> = list.rows.iter().map(|r| r.form_id.as_str()).collect();
    assert_eq!(ids, vec![FormId(30).display(), FormId(32).display()]);
    assert!(
        list.rows
            .iter()
            .all(|r| r.record_type.as_deref() == Some("CONT"))
    );
    // Legend still attributable via inherited entry_points even with no
    // depth-0 carrier rows.
    assert!(list.rows.iter().all(|r| !r.entry_points.is_empty()));

    let _ = std::fs::remove_file(&path);
}

/// `resolve_sel` — used by every `Op` other than `ReferencedBy` — rejects an
/// entry-point selector outright rather than silently misinterpreting it.
#[test]
fn resolve_sel_rejects_entry_point_selector() {
    let (path, mut db) = open_entry_point_db();

    let err = resolve_sel(&mut db, &RecordSel::EntryPoint("39".to_string()))
        .expect_err("entry-point selector must not resolve to a single FormId");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("refs") || msg.contains("ReferencedBy"),
        "error should point at refs/ReferencedBy: {msg}"
    );

    let _ = std::fs::remove_file(&path);
}

/// The explicit `RecordSel::EntryPoint` selector (`--entry-point`/`--ep`)
/// resolves through the full `Op::ReferencedBy` dispatch path exactly like
/// the CLI/daemon/N-API would use it.
#[test]
fn dispatch_referenced_by_resolves_explicit_entry_point_selector() {
    let (path, mut db) = open_entry_point_db();

    let v = dispatch_op(
        &mut db,
        &Op::ReferencedBy {
            sel: RecordSel::EntryPoint("Mod Percent Blocked".to_string()),
            limit: 0,
            depth: 1,
            type_filter: None,
            paths: false,
            sort: esm::ipc::RefSort::Formid,
        },
    )
    .expect("dispatch_op");
    let list: RefList = serde_json::from_value(v).expect("RefList");

    assert!(list.target.contains("Mod Percent Blocked"));
    let carrier_ids: Vec<&str> = list
        .rows
        .iter()
        .filter(|r| r.depth == 0)
        .map(|r| r.form_id.as_str())
        .collect();
    assert_eq!(
        carrier_ids,
        vec![
            FormId(10).display(),
            FormId(11).display(),
            FormId(14).display()
        ]
    );

    let _ = std::fs::remove_file(&path);
}

/// A bare positional token that isn't a real EditorID (`RecordSel::Edid`,
/// exactly what the CLI's auto-detect builds for a spaced, non-hex-looking
/// token like `'Mod Percent Blocked'`) falls back to an entry-point name
/// match — the mechanism behind `esm refs 'Mod Percent Blocked'` working
/// without the explicit `--entry-point` flag.
#[test]
fn dispatch_referenced_by_edid_falls_back_to_entry_point_when_edid_miss() {
    let (path, mut db) = open_entry_point_db();

    let v = dispatch_op(
        &mut db,
        &Op::ReferencedBy {
            sel: RecordSel::Edid("Mod Percent Blocked".to_string()),
            limit: 0,
            depth: 1,
            type_filter: None,
            paths: false,
            sort: esm::ipc::RefSort::Formid,
        },
    )
    .expect("dispatch_op should fall back to entry-point matching");
    let list: RefList = serde_json::from_value(v).expect("RefList");

    assert!(list.target.contains("Mod Percent Blocked"));
    assert!(
        list.rows
            .iter()
            .any(|r| r.depth == 0 && r.form_id == FormId(10).display())
    );

    let _ = std::fs::remove_file(&path);
}

/// A real EditorID always wins over a same-named entry point — the fallback
/// only triggers on an EditorID *miss*, never overriding a genuine match.
#[test]
fn dispatch_referenced_by_edid_wins_over_entry_point_name_collision() {
    let (path, mut db) = open_edid_collision_db();

    let v = dispatch_op(
        &mut db,
        &Op::ReferencedBy {
            sel: RecordSel::Edid("Mod Percent Blocked".to_string()),
            limit: 0,
            depth: 1,
            type_filter: None,
            paths: false,
            sort: esm::ipc::RefSort::Formid,
        },
    )
    .expect("dispatch_op");
    let list: RefList = serde_json::from_value(v).expect("RefList");

    assert_eq!(
        list.target,
        FormId(18).display(),
        "target should be the record's own FormID (legacy direct-target \
         label), not an entry-point label"
    );
    assert!(
        list.rows.iter().all(|r| r.depth != 0),
        "the direct-target path must never emit depth-0 carrier rows: {list:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// Neither an EditorID nor an entry-point name matches: `dispatch_op` fails
/// with a message naming both interpretations, mirroring `RecordSel::Auto`'s
/// existing dual-interpretation error.
#[test]
fn dispatch_referenced_by_edid_neither_interpretation_bails() {
    let (path, mut db) = open_entry_point_db();

    let err = dispatch_op(
        &mut db,
        &Op::ReferencedBy {
            sel: RecordSel::Edid("TotallyBogusTokenXYZ".to_string()),
            limit: 0,
            depth: 1,
            type_filter: None,
            paths: false,
            sort: esm::ipc::RefSort::Formid,
        },
    )
    .expect_err("neither interpretation should resolve");
    let msg = format!("{err:#}");
    assert!(msg.contains("EditorID"), "message: {msg}");
    assert!(msg.contains("entry point"), "message: {msg}");

    let _ = std::fs::remove_file(&path);
}

/// Entry-point tags are inherited onto BFS rows, and a record reached from
/// two carriers at the same depth gets both carriers' tags unioned.
#[test]
fn entry_point_tags_inherited_and_unioned_on_equal_depth_re_reach() {
    let (path, mut db) = open_entry_point_db();

    let list = referenced_by_enriched_multi(
        &mut db,
        &seeds_with_ep(&[(15, 41), (16, 42)]),
        "entry point 'Mod Incoming Spell*' (2 matched)".to_string(),
        1,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("multi EP walk");

    let shared = list
        .rows
        .iter()
        .find(|r| r.form_id == FormId(34).display())
        .expect("SharedGlob must appear");
    assert_eq!(shared.depth, 1);
    let ids: Vec<u16> = shared.entry_points.iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        vec![41, 42],
        "equal-depth re-reach must union both carriers' entry points: {shared:?}"
    );
    // VIA/path keeps the first-reached carrier only.
    assert_eq!(shared.path.len(), 1);
    assert_eq!(
        shared.path[0].form_id,
        FormId(15).display(),
        "first-seed order wins path attribution"
    );

    // Depth-2 SharedDeep from CarrierA+CarrierB (same EP 39) also unions.
    let deep = referenced_by_enriched_multi(
        &mut db,
        &seeds_with_ep(&[(10, 39), (11, 39)]),
        "entry point 39".to_string(),
        2,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("depth-2 walk");
    let shared_deep = deep
        .rows
        .iter()
        .find(|r| r.form_id == FormId(33).display())
        .expect("SharedDeep");
    assert_eq!(shared_deep.depth, 2);
    assert_eq!(shared_deep.entry_points.len(), 1);
    assert_eq!(shared_deep.entry_points[0].id, 39);
    assert_eq!(
        shared_deep.path[0].form_id,
        FormId(10).display(),
        "path[0] is the originating carrier"
    );

    let _ = std::fs::remove_file(&path);
}

/// Seed input order is preserved for carrier row order and BFS attribution
/// (no re-sort by FormID inside `referenced_by_walk`).
#[test]
fn referenced_by_walk_preserves_seed_order_for_carriers_and_attribution() {
    let (path, mut db) = open_entry_point_db();

    let forward = referenced_by_enriched_multi(
        &mut db,
        &seeds_with_ep(&[(15, 41), (16, 42)]),
        "forward".to_string(),
        1,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("forward");
    let reverse = referenced_by_enriched_multi(
        &mut db,
        &seeds_with_ep(&[(16, 42), (15, 41)]),
        "reverse".to_string(),
        1,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("reverse");

    let fwd_carriers: Vec<&str> = forward
        .rows
        .iter()
        .filter(|r| r.depth == 0)
        .map(|r| r.form_id.as_str())
        .collect();
    let rev_carriers: Vec<&str> = reverse
        .rows
        .iter()
        .filter(|r| r.depth == 0)
        .map(|r| r.form_id.as_str())
        .collect();
    assert_eq!(
        fwd_carriers,
        vec![FormId(15).display(), FormId(16).display()]
    );
    assert_eq!(
        rev_carriers,
        vec![FormId(16).display(), FormId(15).display()]
    );

    let fwd_shared = forward
        .rows
        .iter()
        .find(|r| r.form_id == FormId(34).display())
        .unwrap();
    let rev_shared = reverse
        .rows
        .iter()
        .find(|r| r.form_id == FormId(34).display())
        .unwrap();
    assert_eq!(fwd_shared.path[0].form_id, FormId(15).display());
    assert_eq!(
        rev_shared.path[0].form_id,
        FormId(16).display(),
        "reversed seed order must flip first-reach attribution"
    );
    // Both still union to 41+42 regardless of order.
    assert_eq!(
        fwd_shared
            .entry_points
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        vec![41, 42]
    );
    assert_eq!(
        rev_shared
            .entry_points
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        vec![41, 42]
    );

    let _ = std::fs::remove_file(&path);
}

/// A Direct (single-target) walk keeps empty `entry_points` and empty
/// depth-1 `path` — bit-compatible with pre-EP-attribution behavior.
#[test]
fn referenced_by_enriched_direct_has_empty_entry_points_and_path_at_depth_1() {
    let (path, mut db) = open_entry_point_db();

    let list = referenced_by_enriched(
        &mut db,
        FormId(10),
        1,
        0,
        None,
        false,
        esm::ipc::RefSort::Formid,
    )
    .expect("direct walk");
    assert!(list.rows.iter().all(|r| r.entry_points.is_empty()));
    assert!(
        list.rows
            .iter()
            .filter(|r| r.depth == 1)
            .all(|r| r.path.is_empty()),
        "Direct depth-1 rows must keep an empty path: {list:?}"
    );
    assert!(list.carrier_total.is_none());
    assert!(list.entry_point_total.is_none());

    let _ = std::fs::remove_file(&path);
}
