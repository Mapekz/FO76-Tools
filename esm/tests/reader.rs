mod common;

use common::{make_minimal_esm, unique_temp_path};
use esm::reader::{EsmFile, WalkEvent};
use std::io::Write;

#[test]
fn walk_structure_events_sequence() {
    let buf = make_minimal_esm();

    // Write to a temp file so EsmFile::open can mmap it.  unique_temp_path
    // avoids the fixed-filename race when test binaries run in parallel.
    let tmp_path = unique_temp_path("walk_structure");
    {
        let mut f = std::fs::File::create(&tmp_path).expect("create temp file");
        f.write_all(&buf).expect("write");
    }

    let esm = EsmFile::open(&tmp_path).expect("open");

    let mut events = Vec::new();
    esm.walk_structure(|ev| {
        match &ev {
            WalkEvent::GroupStart {
                group_type, label, ..
            } => {
                events.push(format!("GroupStart(type={},label={})", group_type, label));
            }
            WalkEvent::GroupEnd { .. } => {
                events.push("GroupEnd".to_string());
            }
            WalkEvent::Record(r) => {
                events.push(format!("Record({},{})", r.record_type, r.form_id.0));
            }
        }
        Ok(())
    })
    .expect("walk_structure");

    let _ = std::fs::remove_file(&tmp_path);

    assert_eq!(
        events.len(),
        4,
        "expected GroupStart, Record, Record, GroupEnd; got {:?}",
        events
    );
    assert!(
        events[0].starts_with("GroupStart"),
        "first event is GroupStart"
    );
    assert_eq!(events[1], "Record(WEAP,1)");
    assert_eq!(events[2], "Record(WEAP,2)");
    assert_eq!(events[3], "GroupEnd");
}
