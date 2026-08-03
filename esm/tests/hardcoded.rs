//! `DatabaseResolver`-level tests for the hardcoded-engine-form fallback.
//!
//! `src/hardcoded.rs` has its own unit tests for the raw table lookup; these
//! tests exercise the integration point in `DatabaseResolver::stub`/`decode_full`
//! (`src/lib.rs`) — the index-miss fallback to `hardcoded::lookup` — against a
//! real (synthetic) `Database`, and confirm a real ESM record always wins over
//! the hardcoded table when both exist for the same FormID.

mod common;

use common::{append_record, append_subrecord, cstr, tes4_header, wrap_grup, write_and_open};
use esm::decode::FormIdRefResolver;
use esm::ipc::{Op, RecordSel, dispatch_op};
use esm::{DatabaseResolver, FormId, ResolveDepth};

/// FormID 0x00000399 is the engine-hardcoded AVIF `KillStreak` (verified
/// against xEdit's `Core/Hardcoded/Fallout76.esp`; the pseudo-plugin spells
/// it "Kill Streak", which the extractor normalizes). No record in this
/// synthetic ESM defines it, so the resolver must fall back to the embedded
/// hardcoded table rather than leaving it unresolved.
const KILL_STREAK: u32 = 0x0000_0399;

#[test]
fn stub_falls_back_to_hardcoded_table_on_index_miss() {
    let mut buf = tes4_header();
    let mut weap = Vec::new();
    append_record(&mut weap, b"WEAP", 1, &[]); // unrelated record, keeps the ESM non-empty
    buf.extend_from_slice(&wrap_grup(b"WEAP", &weap));

    let (path, db) = write_and_open(&buf, "hardcoded_stub_fallback");
    let resolver = DatabaseResolver::new(&db, 2);

    let stub = resolver
        .stub(FormId::new(KILL_STREAK))
        .expect("0x399 should resolve via the hardcoded fallback");
    assert_eq!(stub.formid, "0x00000399");
    assert_eq!(stub.record_type, "AVIF");
    assert_eq!(stub.editor_id.as_deref(), Some("KillStreak"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn decode_full_falls_back_to_hardcoded_table_on_index_miss() {
    let mut buf = tes4_header();
    let mut weap = Vec::new();
    append_record(&mut weap, b"WEAP", 1, &[]);
    buf.extend_from_slice(&wrap_grup(b"WEAP", &weap));

    let (path, db) = write_and_open(&buf, "hardcoded_full_fallback");
    let resolver = DatabaseResolver::new(&db, 2);

    let value = resolver
        .decode_full(FormId::new(KILL_STREAK))
        .expect("0x399 should resolve via the hardcoded fallback");
    assert_eq!(value["formid"], "0x00000399");
    assert_eq!(value["record_type"], "AVIF");
    assert_eq!(value["editor_id"], "KillStreak");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn real_esm_record_wins_over_hardcoded_table_entry() {
    // Deliberately reuse the hardcoded AVIF `KillStreak` FormID for a WEAP
    // record in the synthetic ESM. The index lookup must succeed here, so the
    // resolver should never consult the hardcoded table — the real record's
    // type and EditorID must come back, not "AVIF"/"KillStreak".
    let mut buf = tes4_header();
    let mut edid = Vec::new();
    append_subrecord(&mut edid, b"EDID", &cstr("TestOverride"));
    let mut weap = Vec::new();
    append_record(&mut weap, b"WEAP", KILL_STREAK, &edid);
    buf.extend_from_slice(&wrap_grup(b"WEAP", &weap));

    let (path, db) = write_and_open(&buf, "hardcoded_real_record_wins");
    let resolver = DatabaseResolver::new(&db, 2);

    let stub = resolver
        .stub(FormId::new(KILL_STREAK))
        .expect("real record should resolve");
    assert_eq!(stub.record_type, "WEAP");
    assert_eq!(stub.editor_id.as_deref(), Some("TestOverride"));

    let _ = std::fs::remove_file(&path);
}

/// `esm get`'s serving path (`ipc::dispatch_op` → `Op::Record` →
/// `record_resolved`), not just `DatabaseResolver`, must also recognize a
/// hardcoded-form miss and explain it instead of a bare "not found" — issue
/// #27's selector-resolution half. `KillStreak` now resolves as an
/// EditorID (via `ipc::resolve_sel`'s hardcoded fallback) but still has no
/// record to decode, so the miss must name the form and point at `esm refs`.
#[test]
fn dispatch_record_explains_hardcoded_edid_miss() {
    let mut buf = tes4_header();
    let mut weap = Vec::new();
    append_record(&mut weap, b"WEAP", 1, &[]); // unrelated record, keeps the ESM non-empty
    buf.extend_from_slice(&wrap_grup(b"WEAP", &weap));

    let (path, mut db) = write_and_open(&buf, "hardcoded_dispatch_edid_miss");

    let err = dispatch_op(
        &mut db,
        &Op::Record {
            sel: RecordSel::Edid("KillStreak".to_string()),
            depth: ResolveDepth::None,
        },
    )
    .expect_err("KillStreak resolves but has no record to decode");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("engine-hardcoded") && msg.contains("esm refs 0x00000399"),
        "expected an explanatory hardcoded-form message, got: {msg}"
    );

    let _ = std::fs::remove_file(&path);
}
