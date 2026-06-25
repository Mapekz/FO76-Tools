use esm::reader::{EsmFile, WalkEvent};
use std::io::Write;

/// Build a minimal ESM byte buffer:
/// - TES4 record: 24 bytes, data_size=0
/// - GRUP: 24 bytes header + 2 × 24-byte child records = 72 bytes total (group_size=72)
/// - 2 WEAP records (data_size=0 each)
fn make_minimal_esm() -> Vec<u8> {
    let mut buf = Vec::new();

    // TES4 header: sig=TES4, data_size=0, flags=0, form_id=0, vcs1=0, form_version=0, vcs2=0
    buf.extend_from_slice(b"TES4"); // signature
    buf.extend_from_slice(&0u32.to_le_bytes()); // data_size = 0
    buf.extend_from_slice(&0u32.to_le_bytes()); // flags
    buf.extend_from_slice(&0u32.to_le_bytes()); // form_id
    buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
    buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
    buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2
                                                // TES4 data_size=0, so no payload bytes

    // GRUP header: sig=GRUP, group_size=72, label=WEAP, group_type=0, stamp=0, unknown=0
    // group_size = 24 (header) + 24 (rec1) + 24 (rec2) = 72
    let group_size: u32 = 72;
    let label = u32::from_le_bytes(*b"WEAP");
    buf.extend_from_slice(b"GRUP"); // signature
    buf.extend_from_slice(&group_size.to_le_bytes()); // group_size
    buf.extend_from_slice(&label.to_le_bytes()); // label
    buf.extend_from_slice(&0i32.to_le_bytes()); // group_type = 0 (top-level)
    buf.extend_from_slice(&0u32.to_le_bytes()); // stamp
    buf.extend_from_slice(&0u32.to_le_bytes()); // unknown

    // WEAP record 1: sig=WEAP, data_size=0, flags=0, form_id=1, vcs1=0, form_version=0, vcs2=0
    buf.extend_from_slice(b"WEAP");
    buf.extend_from_slice(&0u32.to_le_bytes()); // data_size
    buf.extend_from_slice(&0u32.to_le_bytes()); // flags
    buf.extend_from_slice(&1u32.to_le_bytes()); // form_id = 1
    buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
    buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
    buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2

    // WEAP record 2: form_id = 2
    buf.extend_from_slice(b"WEAP");
    buf.extend_from_slice(&0u32.to_le_bytes()); // data_size
    buf.extend_from_slice(&0u32.to_le_bytes()); // flags
    buf.extend_from_slice(&2u32.to_le_bytes()); // form_id = 2
    buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
    buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
    buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2

    buf
}

#[test]
fn walk_structure_events_sequence() {
    let buf = make_minimal_esm();

    // Write to a temp file so EsmFile::open can mmap it
    let tmp_path = std::env::temp_dir().join("fo76_esm_test_walk_structure.esm");
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
