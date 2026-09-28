//! Synthetic ESM bytes for tests: records, subrecords, GRUPs and the TES4
//! header, plus a temp-file writer.
//!
//! Standard library only, so the unit tests (`crate::testkit`) and the
//! integration tests (`tests/common`, which includes this file by path)
//! share one copy.

// Each test binary uses a different subset.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// The form_version stamped on every record built by [`append_record`].
pub const TEST_FORM_VERSION: u16 = 208;

/// Append a subrecord (4-byte signature + `u16` LE size + data) to `out`.
pub fn append_subrecord(out: &mut Vec<u8>, sig: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
}

/// NUL-terminated ASCII bytes for an inline EDID/FULL/DESC-style string field
/// (the non-localized encoding).
pub fn cstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// Append one full record (24-byte header + already-serialized `subrecords`)
/// to `out`, stamped with [`TEST_FORM_VERSION`].
pub fn append_record(out: &mut Vec<u8>, sig: &[u8; 4], form_id: u32, subrecords: &[u8]) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&(subrecords.len() as u32).to_le_bytes()); // data_size
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.extend_from_slice(&form_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // vcs1
    out.extend_from_slice(&TEST_FORM_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // vcs2
    out.extend_from_slice(subrecords);
}

/// One serialized record with no subrecords.
pub fn empty_record(sig: &[u8; 4], form_id: u32) -> Vec<u8> {
    let mut rec = Vec::new();
    append_record(&mut rec, sig, form_id, &[]);
    rec
}

/// One serialized record carrying nothing but an `EDID`.
pub fn record_with_edid(sig: &[u8; 4], form_id: u32, edid: &str) -> Vec<u8> {
    let mut subs = Vec::new();
    append_subrecord(&mut subs, b"EDID", &cstr(edid));
    let mut rec = Vec::new();
    append_record(&mut rec, sig, form_id, &subs);
    rec
}

/// A GRUP of `group_type` around `body`. `label` is the 4 label bytes: a
/// record signature for a top-level group, a little-endian FormID or block
/// number for the others.
pub fn grup(label: [u8; 4], group_type: i32, body: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(24 + body.len());
    buf.extend_from_slice(b"GRUP");
    buf.extend_from_slice(&((24 + body.len()) as u32).to_le_bytes());
    buf.extend_from_slice(&label);
    buf.extend_from_slice(&group_type.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // stamp
    buf.extend_from_slice(&0u32.to_le_bytes()); // unknown
    buf.extend_from_slice(body);
    buf
}

/// Wrap already-serialized records under a single top-level GRUP of `label`.
pub fn wrap_grup(label: &[u8; 4], records: &[u8]) -> Vec<u8> {
    grup(*label, 0, records)
}

/// A bare, non-localized TES4 header (24 bytes, `data_size = 0`): the start
/// of every synthetic ESM buffer.
pub fn tes4_header() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"TES4");
    buf.extend_from_slice(&0u32.to_le_bytes()); // data_size
    buf.extend_from_slice(&0u32.to_le_bytes()); // flags (unset Localized bit)
    buf.extend_from_slice(&0u32.to_le_bytes()); // form_id
    buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
    buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
    buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2
    buf
}

/// A temp `.esm` path unique to this process and call, named from `stem`.
pub fn unique_temp_path(stem: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fo76_esm_test_{stem}_{}_{n}.esm",
        std::process::id()
    ))
}

/// Write `buf` to a [`unique_temp_path`]; the caller removes it.
pub fn write_temp_esm(buf: &[u8], stem: &str) -> PathBuf {
    let path = unique_temp_path(stem);
    std::fs::write(&path, buf).expect("write temp esm");
    path
}
