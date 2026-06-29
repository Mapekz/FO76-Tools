//! Pairwise diff engine for two versions of the same base ESM.
//!
//! Records are aligned by raw FormID. A byte-equality fast-path skips decoding
//! for unchanged records; only records with different payloads are decoded and
//! field-diffed via `json_diff`.

use crate::formid::{parse_formid, FormId};
use crate::reader::{edid_from_subrecords, lstring_id_from_subrecords};
use crate::sources::{Source, SourcesOptions};
use crate::strings::StringKind;
use crate::Database;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Record types for which added records get fully-decoded spec sheets.
const ADDED_DETAIL_TYPES: &[&str] = &[
    "WEAP", "ARMO", "AMMO", "PROJ", "EXPL", "COBJ", "OMOD", "AVIF", "NPC_", "LVLI", "MGEF", "ENCH",
];

#[inline]
fn wants_detail(sig: &str) -> bool {
    ADDED_DETAIL_TYPES.contains(&sig)
}

/// Lightweight record identity for added/removed entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordStub {
    pub form_id: String,
    pub editor_id: Option<String>,
    pub record_type: String,
    pub offset: u64,
    /// Resolved FULL display name (when localization is available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Resolved DESC description (when localization is available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Fully-decoded fields (ResolveDepth::Full) for *added* gameplay records.
    /// Absent for removed/changed stubs and non-gameplay added types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
    /// Drop/acquisition sources for *added* records (reverse-reference walk).
    /// Absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Source>,
}

/// A record present in both ESMs whose decoded fields changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordDiff {
    pub stub: RecordStub,
    /// Sparse JSON object: only changed fields, each `{ "from": .., "to": .. }`.
    pub field_changes: Value,
    /// EditorID from the A (old) side.  Only present when it differs from
    /// `stub.editor_id` (the B side), which indicates an EDID rename this
    /// patch (e.g. a `ZZZ_` deprecation prefix being added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_editor_id: Option<String>,
}

/// Resolved display information for a FormID that appears in `field_changes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefName {
    pub record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// One-hop resolved names for every FormID hex string that appears in any
    /// `field_changes` value.  Keyed by the bare hex string (e.g. `"0x00ABCDEF"`).
    /// Empty when no localization is available or no FormID references exist.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ref_names: BTreeMap<String, RefName>,
}

/// Compare two ESM databases and return a structured diff.
///
/// Records are aligned by raw FormID.  The decompressed data payload is
/// compared byte-for-byte to skip unchanged records (fast-path).  Only
/// changed records are decoded and field-diffed.
///
/// When either database has a localization table loaded, each `RecordStub`
/// is enriched with `name` (FULL) and `description` (DESC), and `DiffResult`
/// gains a `ref_names` sidecar mapping every FormID hex reference found in
/// `field_changes` to its resolved record type, EditorID, and name.
pub fn diff_databases(a: &Database, b: &Database) -> anyhow::Result<DiffResult> {
    let a_ids: HashSet<FormId> = a.index.form_index.keys().copied().collect();
    let b_ids: HashSet<FormId> = b.index.form_index.keys().copied().collect();

    // Added: in B but not A
    let mut added = Vec::new();
    for id in b_ids.difference(&a_ids) {
        let meta = b.index.form_index[id].clone();
        let mut stub = record_stub_from_db(b, &meta, *id)?;
        // Decode full fields for gameplay types (best-effort; never aborts the diff).
        if wants_detail(&stub.record_type) {
            if let Ok(r) = b.record_by_formid_resolved(*id, crate::decode::ResolveDepth::Full) {
                stub.fields = Some(r.fields);
            }
        }
        added.push(stub);
    }
    added.sort_by(|x, y| x.form_id.cmp(&y.form_id));

    // Removed: in A but not B
    let mut removed = Vec::new();
    for id in a_ids.difference(&b_ids) {
        let meta = a.index.form_index[id].clone();
        let stub = record_stub_from_db(a, &meta, *id)?;
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

        // Resolve name/description from the B-side raw subrecords.
        let (name, description) = resolve_stub_names(b, &meta_b);

        let stub = RecordStub {
            form_id: id.display(),
            editor_id: rb.editor_id.clone(),
            record_type: meta_b.signature.clone(),
            offset: meta_b.offset,
            name,
            description,
            ..Default::default()
        };

        // Capture A-side EditorID only when it changed — signals an EDID rename
        // (e.g. a ZZZ_ / CUT_ deprecation prefix being applied this patch).
        let prev_editor_id = if ra.editor_id != rb.editor_id {
            ra.editor_id
        } else {
            None
        };

        changed.push(RecordDiff {
            stub,
            field_changes,
            prev_editor_id,
        });
    }
    changed.sort_by(|x, y| x.stub.form_id.cmp(&y.stub.form_id));

    // Build ref_names: one-hop FormID resolution for every hex ref in field_changes
    // and added records' decoded fields. Populated when either side has localization
    // loaded, or when curves are loaded (so added-record FormIDs get names too).
    let ref_names = if b.localization.is_some()
        || a.localization.is_some()
        || b.curves.is_some()
        || a.curves.is_some()
    {
        let mut refs: HashSet<String> = HashSet::new();
        for rd in &changed {
            collect_formid_refs(&rd.field_changes, &mut refs);
        }
        for stub in &added {
            if let Some(f) = &stub.fields {
                collect_formid_refs(f, &mut refs);
            }
        }
        refs.into_iter()
            .filter_map(|fid_str| resolve_ref_name(&fid_str, b, a).map(|rn| (fid_str, rn)))
            .collect()
    } else {
        BTreeMap::new()
    };

    Ok(DiffResult {
        added,
        removed,
        changed,
        ref_names,
    })
}

/// Build a `RecordStub` from a database, resolving name/description when
/// localization is available.
fn record_stub_from_db(
    db: &Database,
    meta: &crate::reader::RecordMeta,
    id: FormId,
) -> anyhow::Result<RecordStub> {
    let rec = db.esm.parse_record_at(meta.offset)?;
    let editor_id = edid_from_subrecords(&rec.subrecords);

    let (name, description) = if let Some(loc) = &db.localization {
        let n = lstring_id_from_subrecords(&rec.subrecords, "FULL")
            .and_then(|lid| loc.lookup(StringKind::Strings, lid).map(str::to_owned));
        let d = lstring_id_from_subrecords(&rec.subrecords, "DESC")
            .and_then(|lid| loc.lookup(StringKind::DlStrings, lid).map(str::to_owned));
        (n, d)
    } else {
        (None, None)
    };

    Ok(RecordStub {
        form_id: id.display(),
        editor_id,
        record_type: meta.signature.clone(),
        offset: meta.offset,
        name,
        description,
        ..Default::default()
    })
}

/// Resolve FULL (name) and DESC (description) from the raw record at `meta.offset`.
/// Returns `(None, None)` on any parse error or when localization is absent.
fn resolve_stub_names(
    db: &Database,
    meta: &crate::reader::RecordMeta,
) -> (Option<String>, Option<String>) {
    let loc = match &db.localization {
        Some(l) => l,
        None => return (None, None),
    };
    let rec = match db.esm.parse_record_at(meta.offset) {
        Ok(r) => r,
        Err(_) => return (None, None),
    };
    let name = lstring_id_from_subrecords(&rec.subrecords, "FULL")
        .and_then(|lid| loc.lookup(StringKind::Strings, lid).map(str::to_owned));
    let description = lstring_id_from_subrecords(&rec.subrecords, "DESC")
        .and_then(|lid| loc.lookup(StringKind::DlStrings, lid).map(str::to_owned));
    (name, description)
}

/// Return `true` if `s` is a FormID hex string as produced by `FormId::display()`:
/// exactly `0x` followed by 8 ASCII hex digits (case-insensitive).
fn is_formid_str(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10 && b[0] == b'0' && b[1] == b'x' && b[2..].iter().all(|c| c.is_ascii_hexdigit())
}

/// Recursively collect all FormID-shaped strings from a JSON value tree.
fn collect_formid_refs(val: &Value, out: &mut HashSet<String>) {
    match val {
        Value::String(s) if is_formid_str(s) => {
            out.insert(s.clone());
        }
        Value::Object(map) => {
            for v in map.values() {
                collect_formid_refs(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_formid_refs(v, out);
            }
        }
        _ => {}
    }
}

/// Resolve a FormID hex string to a `RefName` by looking up the record in
/// `primary` (B / new side) first, then `fallback` (A / old side).
fn resolve_ref_name(fid_str: &str, primary: &Database, fallback: &Database) -> Option<RefName> {
    let id = parse_formid(fid_str).ok()?;
    for db in [primary, fallback] {
        if let Some(meta) = db.index.form_index.get(&id) {
            let offset = meta.offset;
            let sig = meta.signature.clone();
            let rec = match db.esm.parse_record_at(offset) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let editor_id = edid_from_subrecords(&rec.subrecords);
            let name = db.localization.as_ref().and_then(|loc| {
                lstring_id_from_subrecords(&rec.subrecords, "FULL")
                    .and_then(|lid| loc.lookup(StringKind::Strings, lid).map(str::to_owned))
            });
            return Some(RefName {
                record_type: sig,
                editor_id,
                name,
            });
        }
    }
    None
}

/// Attach drop/acquisition sources to every added record that has at least one
/// source.
///
/// Must be called *after* [`diff_databases`] against the B-side database (added
/// records live in B).  Requires `&mut Database` for the lazy xref index, which
/// is built once on the first call to [`sources_of`] and then reused from the
/// on-disk cache.
///
/// Errors on individual records are logged to stderr and do not abort the walk.
pub fn enrich_added_sources(
    b: &mut Database,
    result: &mut DiffResult,
    opts: &SourcesOptions,
) -> anyhow::Result<()> {
    for stub in &mut result.added {
        let id = match parse_formid(&stub.form_id) {
            Ok(id) => id,
            Err(_) => continue,
        };
        match crate::sources::sources_of(b, id, opts) {
            Ok(list) if !list.sources.is_empty() => {
                stub.sources = list.sources;
            }
            Ok(_) => {}
            Err(e) => eprintln!("Warning: sources_of {} failed: {e:#}", stub.form_id),
        }
    }
    Ok(())
}

/// Apply optional record-type filter to a diff result in-place.
pub fn apply_type_filter(result: &mut DiffResult, record_type: &Option<String>) {
    if let Some(sig) = record_type {
        let sig = sig.to_uppercase();
        result.added.retain(|s| s.record_type == sig);
        result.removed.retain(|s| s.record_type == sig);
        result.changed.retain(|d| d.stub.record_type == sig);
        // ref_names is a display sidecar — keep unrestricted.
    }
}

/// Recursive JSON diff.  Returns a sparse object with only changed fields.
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
