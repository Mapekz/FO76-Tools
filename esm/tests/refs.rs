mod common;

use common::{make_xref_esm, unique_temp_path};
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
