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
use serde_json::Value;
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
    bare_ctx_fv(schema, 208)
}

/// Like [`bare_ctx`] but with an explicit `form_version`.
///
/// Records carry their own form_version in the record header, and the decoder
/// uses it to gate version-conditional fields.  When a test embeds the verbatim
/// bytes of a real record, it must decode them at that record's form_version —
/// not the 208 default — or version-gated fields shift and the decode diverges
/// from what the CLI produced.  The matching `--raw` dump prints
/// `header.form_version`.
///
/// `is_localized` stays `false`: the reference ESM is non-localized, so FULL /
/// DESC subrecords carry inline (optionally `<ID=…>`-prefixed) strings rather
/// than string-table IDs.
pub fn bare_ctx_fv(schema: &Schema, form_version: u16) -> DecodeContext<'_> {
    DecodeContext {
        schema,
        form_version,
        is_localized: false,
        localization: None,
        curves: None,
        resolve_depth: ResolveDepth::None,
        resolver: None,
        outer_struct: None,
        record_signature: None,
        record_edid_char: None,
        scope_min_doc_index: None,
        scope_max_doc_index: None,
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

/// Build a `Vec<OwnedSubrecord>` from an ordered slice of `(signature, hex)`
/// pairs, assigning `doc_index` by position.
///
/// This mirrors the order subrecords appear on disk, which the decoder relies
/// on (it consumes subrecords in schema order).  Pairs are typically the
/// verbatim output of `esm get <FILE> --formid <ID> --raw` — copy each
/// subrecord's `signature` and `hex` straight in.
pub fn subrecords_from(pairs: &[(&str, &str)]) -> Vec<OwnedSubrecord> {
    pairs
        .iter()
        .enumerate()
        .map(|(i, (sig, hex))| sr(sig, hex, i))
        .collect()
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

// ──────────────────────────────────────────────────────────────────────────────
// Decode-quality assertions
// ──────────────────────────────────────────────────────────────────────────────

/// Recursively collect every "decode problem" from a decoded record JSON value.
///
/// Three marker types indicate a decode gap:
///
/// | Marker | Key(s) present | Meaning |
/// |---|---|---|
/// | `_unknown_record` | `_unknown_record: true` | Signature not in schema |
/// | `raw_fallback` | `_raw: true` + `reason: "…"` | Field used a raw-bytes fallback |
/// | `_unmapped` | `_unmapped: { … }` | Subrecords not consumed by any schema member |
///
/// The third column intentionally excludes `_unresolved: true` (unresolved
/// LString IDs from localized ESMs) — that marker indicates a missing string
/// table, not a decode bug.  Tests in this suite run against a non-localized
/// ESM, so `_unresolved` cannot appear.
///
/// Returns a list of human-readable problem strings.  An empty return means the
/// record decoded fully with no markers.
pub fn collect_decode_problems(v: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    collect_decode_problems_inner(v, "", &mut problems);
    problems
}

fn collect_decode_problems_inner(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            // _unknown_record
            if map.get("_unknown_record") == Some(&Value::Bool(true)) {
                let sig = map
                    .get("_record_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unknown>");
                out.push(format!("[{path}] _unknown_record for signature '{sig}'"));
            }
            // raw_fallback: _raw=true AND reason key present (but NOT inside _unmapped)
            if map.get("_raw") == Some(&Value::Bool(true)) && map.contains_key("reason") {
                let reason = map
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<no reason>");
                out.push(format!("[{path}] raw_fallback: {reason}"));
            }
            // _unmapped: count the entry sigs, don't recurse into their raw hex
            if let Some(Value::Object(unmapped)) = map.get("_unmapped") {
                for sig in unmapped.keys() {
                    out.push(format!("[{path}] _unmapped subrecord sig '{sig}'"));
                }
            }
            // Recurse into non-_unmapped children
            for (k, child) in map {
                if k == "_unmapped" {
                    continue;
                }
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                collect_decode_problems_inner(child, &child_path, out);
            }
        }
        Value::Array(arr) => {
            for (i, child) in arr.iter().enumerate() {
                let child_path = format!("{path}[{i}]");
                collect_decode_problems_inner(child, &child_path, out);
            }
        }
        _ => {}
    }
}

/// Assert that `decoded` contains no `_unknown_record`, `raw_fallback`, or
/// `_unmapped` markers anywhere in its JSON tree.  Panics with a detailed
/// listing of every problem found.
pub fn assert_fully_decoded(decoded: &Value) {
    let problems = collect_decode_problems(decoded);
    assert!(
        problems.is_empty(),
        "record did not decode fully — {} problem(s) found:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Assert that `decoded` contains **only** the documented drift markers and
/// nothing else — and that at least one drift marker is present.
///
/// Each entry in `allowed_sigs` is a subrecord signature (e.g. `"XALG"`,
/// `"LVLD"`, `"AWPB"`) that is known version-drift: it is present in the raw
/// ESM bytes but intentionally absent from the schema (newer than the TES5Edit
/// Pascal reference, or gated by a version condition that excludes live data).
///
/// The function:
/// 1. Asserts at least one problem exists — so the test cannot pass silently on
///    a record that carries none of the drift sigs.
/// 2. Asserts every problem string contains `'<sig>'` (the quoted form produced
///    by [`collect_decode_problems`] for `_unmapped` markers) for one of the
///    `allowed_sigs`.  Any unexpected problem — a raw fallback, a new unmapped
///    sig, or an unknown-record marker — fails the test, locking the drift
///    boundary against regressions.
pub fn assert_only_drift_markers(decoded: &Value, allowed_sigs: &[&str]) {
    let problems = collect_decode_problems(decoded);
    assert!(
        !problems.is_empty(),
        "expected at least one drift marker ({allowed_sigs:?}) but found none; \
         the test record must carry at least one of the documented drift subrecords"
    );
    for problem in &problems {
        let allowed = allowed_sigs
            .iter()
            .any(|sig| problem.contains(&format!("'{sig}'")));
        assert!(
            allowed,
            "unexpected decode problem (not a documented drift marker): {problem}\n\
             allowed drift sigs: {allowed_sigs:?}"
        );
    }
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
