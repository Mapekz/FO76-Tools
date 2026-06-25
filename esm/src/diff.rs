//! Pairwise diff engine for two versions of the same base ESM.
//!
//! Records are aligned by raw FormID. A byte-equality fast-path skips decoding
//! for unchanged records; only records with different payloads are decoded and
//! field-diffed via `json_diff`.

use crate::formid::FormId;
use crate::reader::edid_from_subrecords;
use crate::Database;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lightweight record identity for added/removed entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordStub {
    pub form_id: String,
    pub editor_id: Option<String>,
    pub record_type: String,
    pub offset: u64,
}

/// A record present in both ESMs whose decoded fields changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordDiff {
    pub stub: RecordStub,
    /// Sparse JSON object: only changed fields, each `{ "from": .., "to": .. }`.
    pub field_changes: Value,
}

/// Top-level result of comparing two ESM files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    /// FormIDs present in B but not A.
    pub added: Vec<RecordStub>,
    /// FormIDs present in A but not B.
    pub removed: Vec<RecordStub>,
    /// FormIDs in both files where the decoded fields changed.
    pub changed: Vec<RecordDiff>,
}

/// Compare two ESM databases and return a structured diff.
///
/// Records are aligned by raw FormID. The decompressed data payload is compared
/// byte-for-byte to skip unchanged records (fast-path). Only changed records are
/// decoded and field-diffed.
pub fn diff_databases(a: &Database, b: &Database) -> anyhow::Result<DiffResult> {
    let a_ids: std::collections::HashSet<FormId> = a.index.form_index.keys().copied().collect();
    let b_ids: std::collections::HashSet<FormId> = b.index.form_index.keys().copied().collect();

    // Added: in B but not A
    let mut added = Vec::new();
    for id in b_ids.difference(&a_ids) {
        let meta = b.index.form_index[id].clone();
        let stub = record_stub_from(&b.esm, &meta, *id)?;
        added.push(stub);
    }
    added.sort_by(|x, y| x.form_id.cmp(&y.form_id));

    // Removed: in A but not B
    let mut removed = Vec::new();
    for id in a_ids.difference(&b_ids) {
        let meta = a.index.form_index[id].clone();
        let stub = record_stub_from(&a.esm, &meta, *id)?;
        removed.push(stub);
    }
    removed.sort_by(|x, y| x.form_id.cmp(&y.form_id));

    // Common: compare payloads, decode only on mismatch
    let mut changed = Vec::new();
    let mut common_ids: Vec<FormId> = a_ids.intersection(&b_ids).copied().collect();
    common_ids.sort_by_key(|id| id.raw());

    for id in common_ids {
        let meta_a = a.index.form_index[&id].clone();
        let meta_b = b.index.form_index[&id].clone();

        let payload_a = a
            .esm
            .record_payload_at(meta_a.offset)
            .with_context(|| format!("payload A for {}", id))?;
        let payload_b = b
            .esm
            .record_payload_at(meta_b.offset)
            .with_context(|| format!("payload B for {}", id))?;

        if payload_a == payload_b {
            continue; // fast-path: unchanged
        }

        // Decode both and field-diff
        let ra = a
            .record_at_meta(&meta_a)
            .with_context(|| format!("decode A for {}", id))?;
        let rb = b
            .record_at_meta(&meta_b)
            .with_context(|| format!("decode B for {}", id))?;

        let field_changes = json_diff(&ra.fields, &rb.fields);
        if field_changes == Value::Object(serde_json::Map::new()) {
            continue; // decoded-equal despite byte differences (volatile header bytes)
        }

        let stub = RecordStub {
            form_id: id.display(),
            editor_id: rb.editor_id,
            record_type: meta_b.signature.clone(),
            offset: meta_b.offset,
        };
        changed.push(RecordDiff {
            stub,
            field_changes,
        });
    }
    changed.sort_by(|x, y| x.stub.form_id.cmp(&y.stub.form_id));

    Ok(DiffResult {
        added,
        removed,
        changed,
    })
}

fn record_stub_from(
    esm: &crate::reader::EsmFile,
    meta: &crate::reader::RecordMeta,
    id: FormId,
) -> anyhow::Result<RecordStub> {
    let rec = esm.parse_record_at(meta.offset)?;
    let editor_id = edid_from_subrecords(&rec.subrecords);
    Ok(RecordStub {
        form_id: id.display(),
        editor_id,
        record_type: meta.signature.clone(),
        offset: meta.offset,
    })
}

/// Recursive JSON diff. Returns a sparse object with only changed fields.
/// Arrays are treated as opaque: any difference → `{ "from": a, "to": b }`.
pub fn json_diff(a: &Value, b: &Value) -> Value {
    match (a, b) {
        (Value::Object(ao), Value::Object(bo)) => {
            let mut out = serde_json::Map::new();
            let all_keys: std::collections::BTreeSet<&String> =
                ao.keys().chain(bo.keys()).collect();
            for key in all_keys {
                match (ao.get(key), bo.get(key)) {
                    (Some(av), Some(bv)) => {
                        if av == bv {
                            // unchanged — omit
                        } else {
                            let diff = json_diff(av, bv);
                            if let Value::Object(ref m) = diff {
                                if !m.is_empty() {
                                    out.insert(key.clone(), diff);
                                }
                            } else {
                                out.insert(key.clone(), diff);
                            }
                        }
                    }
                    (Some(av), None) => {
                        out.insert(key.clone(), serde_json::json!({"from": av, "to": null}));
                    }
                    (None, Some(bv)) => {
                        out.insert(key.clone(), serde_json::json!({"from": null, "to": bv}));
                    }
                    (None, None) => unreachable!(),
                }
            }
            Value::Object(out)
        }
        (av, bv) if av == bv => Value::Object(serde_json::Map::new()),
        (av, bv) => serde_json::json!({"from": av, "to": bv}),
    }
}
