//! Shared test fixtures for the esm integration test suite.
//!
//! Cargo compiles each file directly under `tests/` as a separate test binary,
//! but files in a *subdirectory* of `tests/` are plain modules — so this file
//! is never compiled as its own binary and never produces a spurious
//! "running 0 tests" line.  Each integration test file that needs these helpers
//! pulls them in with:
//!
//! ```ignore
//! mod common;
//! ```
//!
//! Not every test binary uses every symbol here, so the blanket dead-code allow
//! prevents those unused-helper warnings from becoming errors under `-D warnings`.
#![allow(dead_code)]

use esm::decode::{DecodeContext, ResolveDepth};
use esm::format::Signature;
use esm::reader::OwnedSubrecord;
use esm::schema::Schema;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// Build a minimal `DecodeContext` around a borrowed `Schema`.
///
/// Uses form_version 208 (the standard FO76 post-C.A.M.P. version), no
/// localization, curves, or FormID resolver.
///
/// **Note:** an identical `bare_ctx` exists inside `src/decode.rs`'s private
/// `#[cfg(test)] mod tests` block for the unit tests that exercise the private
/// `decode_struct_fields` function.  If `DecodeContext` gains or loses a field,
/// update both copies.
pub fn bare_ctx(schema: &Schema) -> DecodeContext<'_> {
    DecodeContext {
        schema,
        form_version: 208,
        is_localized: false,
        localization: None,
        curves: None,
        resolve_depth: ResolveDepth::None,
        resolver: None,
        outer_struct: None,
        record_edid_char: None,
    }
}

/// Build an `OwnedSubrecord` from a 4-char ASCII signature and a lowercase hex
/// payload string (e.g. `"deadbeef"`).
///
/// Panics on malformed hex — these are test-only call sites with literal strings.
pub fn sr(sig: &str, hex: &str, idx: usize) -> OwnedSubrecord {
    OwnedSubrecord {
        signature: Signature::from_slice(sig.as_bytes()),
        data: hex_bytes(hex),
        doc_index: idx,
    }
}

/// Decode a lowercase hex string into a `Vec<u8>`.
pub fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// Build a minimal ESM byte buffer:
/// - TES4 record: 24 bytes, data_size = 0
/// - GRUP: 24-byte header + 2 × 24-byte child records = 72 bytes (group_size = 72)
/// - 2 WEAP records with form_ids 1 and 2, data_size = 0
pub fn make_minimal_esm() -> Vec<u8> {
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

/// Return a collision-free path under the system temp dir, suitable for a
/// synthetic `.esm` file that `EsmFile::open` can mmap.
///
/// The path incorporates the current process ID and a per-process counter, so
/// it is safe when test binaries run in parallel (different pids) and when
/// multiple tests within the same binary call this concurrently (different
/// counter values).  The caller is responsible for removing the file when done.
pub fn unique_temp_path(stem: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fo76_esm_test_{stem}_{}_{n}.esm",
        std::process::id()
    ))
}
