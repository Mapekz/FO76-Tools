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
                events.push(format!("Record({},{})", r.signature, r.form_id.0));
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

/// One subrecord on the wire: 4-byte signature, u16 size, payload.
fn sub(sig: &[u8; 4], size: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = sig.to_vec();
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// An `XXXX` subrecord carrying the real u32 size of the next subrecord, whose
/// own u16 size field is then 0 (the payload is too large to fit in it).
fn xxxx(real_size: u32) -> Vec<u8> {
    sub(b"XXXX", 4, &real_size.to_le_bytes())
}

#[test]
fn xxxx_supplies_the_size_of_a_following_zero_size_subrecord() {
    let big = vec![0xAB; 70_000];
    let mut buf = xxxx(big.len() as u32);
    buf.extend(sub(b"DATA", 0, &big));
    buf.extend(sub(b"EDID", 3, b"ok\0"));

    let subs = esm::reader::parse_subrecords(&buf).unwrap();
    let sigs: Vec<&str> = subs.iter().map(|s| s.signature.as_str()).collect();
    assert_eq!(sigs, ["DATA", "EDID"], "XXXX itself is not emitted");
    assert_eq!(subs[0].data.len(), 70_000);
    assert_eq!(subs[1].data, b"ok\0");
}

#[test]
fn xxxx_is_ignored_when_the_next_subrecord_has_its_own_size() {
    let mut buf = xxxx(9_999);
    buf.extend(sub(b"EDID", 3, b"ok\0"));
    buf.extend(sub(b"FULL", 0, b""));

    let subs = esm::reader::parse_subrecords(&buf).unwrap();
    assert_eq!(subs[0].data, b"ok\0");
    assert!(
        subs[1].data.is_empty(),
        "a pending XXXX size is consumed by the very next subrecord only"
    );
}

#[test]
fn xxxx_size_past_the_end_of_the_record_is_clamped() {
    let mut buf = xxxx(1_000_000);
    buf.extend(sub(b"DATA", 0, &[1, 2, 3]));

    let subs = esm::reader::parse_subrecords(&buf).unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].data, [1, 2, 3]);
}
