mod common;

use common::unique_temp_path;
use esm::ipc::{dispatch, Op, RecordSel, Request, Response};
use esm::registry::Registry;
use esm::{sources_of, FormId, SourceKind, SourcesOptions};
use std::io::Write;

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

/// Item (1) ← Inner LVLI (2) ← Outer LVLI (3) ← CONT (4)
fn make_sources_chain_esm() -> Vec<u8> {
    let mut buf = Vec::new();
    // TES4 header
    buf.extend_from_slice(b"TES4");
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());

    buf.extend(wrap_grup(b"MISC", &build_misc(1, "TestItem")));
    let mut lvli_group = build_lvli(2, "InnerList", 1);
    lvli_group.extend(build_lvli(3, "OuterList", 2));
    buf.extend(wrap_grup(b"LVLI", &lvli_group));
    buf.extend(wrap_grup(b"CONT", &build_cont(4, "TestContainer", 3)));
    buf
}

fn open_chain_db() -> (std::path::PathBuf, esm::Database) {
    let buf = make_sources_chain_esm();
    let tmp_path = unique_temp_path("sources_chain");
    {
        let mut f = std::fs::File::create(&tmp_path).expect("create temp file");
        f.write_all(&buf).expect("write");
    }
    let db = esm::Database::open(&tmp_path).expect("open");
    (tmp_path, db)
}

#[test]
fn sources_of_finds_container_through_lvli_chain() {
    let (path, mut db) = open_chain_db();
    let list = sources_of(&mut db, FormId(1), &SourcesOptions::default()).expect("sources_of");

    assert_eq!(list.target, "0x00000001");
    assert_eq!(list.sources.len(), 1);
    let src = &list.sources[0];
    assert_eq!(src.kind, SourceKind::Container);
    assert_eq!(src.form_id, "0x00000004");
    assert_eq!(src.editor_id.as_deref(), Some("TestContainer"));
    assert_eq!(src.path.len(), 4);
    assert_eq!(src.path[0].form_id, "0x00000001");
    assert_eq!(src.path[3].form_id, "0x00000004");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn sources_of_respects_depth_bound() {
    let (path, mut db) = open_chain_db();
    // Depth 1 stops at the first LVLI without reaching CONT.
    let list = sources_of(
        &mut db,
        FormId(1),
        &SourcesOptions { max_depth: 1 },
    )
    .expect("sources_of");
    assert!(list.sources.is_empty());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn sources_of_orphan_leveled_list() {
    let (path, mut db) = open_chain_db();
    // InnerList (2) is referenced only by OuterList; with depth 2 we still reach CONT.
    // Target InnerList directly: parent is OuterList, terminal is CONT.
    let list = sources_of(&mut db, FormId(2), &SourcesOptions::default()).expect("sources_of");
    assert_eq!(list.sources.len(), 1);
    assert_eq!(list.sources[0].kind, SourceKind::Container);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn sources_of_dedups_terminal_by_formid() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"TES4");
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());

    // Item 1 referenced by two LVLIs that both roll into the same CONT.
    buf.extend(wrap_grup(b"MISC", &build_misc(1, "TestItem")));
    let mut lvli_group = build_lvli(2, "ListA", 1);
    lvli_group.extend(build_lvli(3, "ListB", 1));
    buf.extend(wrap_grup(b"LVLI", &lvli_group));

    let mut cont_group = build_cont(4, "ContainerA", 2);
    cont_group.extend(build_cont(5, "ContainerB", 3));
    buf.extend(wrap_grup(b"CONT", &cont_group));

    let tmp_path = unique_temp_path("sources_dedup");
    {
        let mut f = std::fs::File::create(&tmp_path).expect("create");
        f.write_all(&buf).expect("write");
    }
    let mut db = esm::Database::open(&tmp_path).expect("open");
    let list = sources_of(&mut db, FormId(1), &SourcesOptions::default()).expect("sources_of");
    assert_eq!(list.sources.len(), 2);
    let ids: Vec<_> = list.sources.iter().map(|s| s.form_id.as_str()).collect();
    assert!(ids.contains(&"0x00000004"));
    assert!(ids.contains(&"0x00000005"));

    let _ = std::fs::remove_file(&tmp_path);
}

#[test]
fn dispatch_sources_op() {
    let buf = make_sources_chain_esm();
    let tmp_path = unique_temp_path("sources_dispatch");
    {
        let mut f = std::fs::File::create(&tmp_path).expect("create");
        f.write_all(&buf).expect("write");
    }
    let reg = Registry::new();
    reg.get_or_open(&tmp_path).expect("warm");

    let req = Request {
        esm: tmp_path.clone(),
        op: Op::Sources {
            sel: RecordSel::FormId(FormId(1)),
            max_depth: None,
        },
    };
    let Response::Ok { data } = dispatch(&reg, &req) else {
        panic!("expected Ok");
    };
    let list: esm::SourceList = serde_json::from_value(data).unwrap();
    assert_eq!(list.sources.len(), 1);
    assert_eq!(list.sources[0].kind, SourceKind::Container);

    let _ = std::fs::remove_file(&tmp_path);
}
