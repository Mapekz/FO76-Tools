//! Pairwise diff engine for two versions of the same base ESM.
//!
//! Records are aligned by raw FormID. A byte-equality fast-path skips decoding
//! for unchanged records; only records with different payloads are decoded and
//! field-diffed via `json_diff`.
//!
//! Two self-contained subsystems live in their own submodules: [`noise`] runs
//! the three sequential noise-suppression sub-stages over a `changed`
//! record's `field_changes` (unconditional field-stripping, issue #18
//! restamp-appearance stripping, issue #22 calibrated-default/padding-zero
//! stripping — see that module's docs for the stage order, which is
//! load-bearing), and [`array_diff`] is `json_diff`'s per-element array-diff
//! engine (the four `keyed`/`positional`/`set`/`unkeyed` pairing strategies
//! ADR 0005 documents). The two cross paths only through ordinary mutual
//! recursion — `json_diff` calls `array_diff::array_diff` for array fields,
//! and `array_diff`'s `keyed_diff`/`positional_diff` call back into
//! `json_diff` for per-element sub-diffs.

mod array_diff;
mod noise;

use crate::Database;
use crate::decode::ResolveDepth;
use crate::formid::{FormId, parse_formid};
use crate::reader::{
    OwnedSubrecord, edid_from_subrecords, inline_string_from_subrecords, lstring_id_from_subrecords,
};
use crate::strings::StringKind;
use anyhow::Context;
use array_diff::is_empty_diff;
pub use noise::strip_noise_fields;
use noise::strip_version_gated_transitions;
use noise::{apply_restamp_calibrated_suppression, strip_restamp_appearances};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// How much of an added/removed record's decoded body to attach to its stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyDetail {
    /// Don't attach decoded fields at all — stub identity only.
    None,
    /// Attach fields with FormID references resolved to stubs (`ResolveDepth::Stub`).
    Stub,
    /// Attach fields with FormID references recursively expanded (`ResolveDepth::Full`).
    Full,
}

impl BodyDetail {
    /// Map to the `ResolveDepth` used to decode the body, or `None` when no
    /// body should be decoded at all (`BodyDetail::None`).
    fn resolve_depth(self) -> Option<ResolveDepth> {
        match self {
            BodyDetail::None => None,
            BodyDetail::Stub => Some(ResolveDepth::Stub),
            BodyDetail::Full => Some(ResolveDepth::Full),
        }
    }
}

/// Options controlling [`diff_databases_with`]'s behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffOptions {
    /// Detail level for decoded fields attached to added/removed record stubs.
    pub bodies: BodyDetail,
    /// Strip known-noisy fields (placement transforms, CELL precombine data,
    /// …) from `changed` records, dropping the record entirely when nothing
    /// else changed. See [`strip_noise_fields`].
    pub suppress_noise: bool,
    /// 4-character record-type signatures (e.g. `["LAND", "NAVM"]`) to omit
    /// entirely from `added`, `removed`, and `changed`.
    pub exclude_types: Vec<String>,
    /// Minimum appearance count for a `(leaf_name, value)` pair to be treated
    /// as a serializer default and stripped when `form_version`s differ
    /// (issue #22). Measured on the 20260710→20260717 snapshot: N=100 lands
    /// `changed` at 10,571 records (also stripping 3,484 padding-zeroed `_raw`
    /// leaves), collapsing 40 distinct serializer-default rules — near the
    /// issue's ~11.6K true-churn estimate. Wiring this to CLI/config is issue
    /// #15 — the field exists so that can land without another diff-engine
    /// change.
    pub restamp_default_min_count: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            bodies: BodyDetail::Full,
            suppress_noise: true,
            exclude_types: Vec::new(),
            restamp_default_min_count: 100,
        }
    }
}

/// Lightweight record identity for added/removed entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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
    /// Decoded fields for `added`/`removed` records, at the depth requested
    /// by `DiffOptions::bodies` (see [`BodyDetail`]). `None` when
    /// `BodyDetail::None` was requested, or when the record failed to
    /// decode. Always absent on `changed` stubs (see `RecordDiff::field_changes`
    /// instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub fields: Option<Value>,
}

/// A record present in both ESMs whose decoded fields changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecordDiff {
    pub stub: RecordStub,
    /// Sparse JSON object: only changed fields, each `{ "from": .., "to": .. }`.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub field_changes: Value,
    /// EditorID from the A (old) side.  Only present when it differs from
    /// `stub.editor_id` (the B side), which indicates an EDID rename this
    /// patch (e.g. a `ZZZ_` deprecation prefix being added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_editor_id: Option<String>,
}

/// Resolved display information for a FormID that appears in `field_changes`.
///
/// Only ever constructed and serialized (see `resolve_ref_name`) — never
/// deserialized from wire JSON — so adding `#[serde(default)]` here (needed
/// for `ts-rs` to correctly infer these fields as omittable, not just
/// nullable, matching `skip_serializing_if`) has no effect on the actual
/// JSON this type produces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefName {
    pub record_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A `(leaf_name, value)` serializer-default rule auto-classified by the
/// calibrated appearance-default pass (issue #22), with the global appearance
/// count that triggered it. Emitted in [`DiffResult::auto_suppressed_defaults`]
/// so a human/agent can audit exactly what a given diff run dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct SuppressedDefault {
    /// Last path segment of the field, with any `[N]`/`[]` index suffix stripped
    /// (e.g. path `Effects[].Effect.Cooldown Duration` → `Cooldown Duration`).
    pub leaf_name: String,
    /// The appearance `to` value that was classified as a serializer default.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub value: Value,
    /// How many times this `(leaf_name, value)` appearance occurred across the
    /// whole diff (must be ≥ [`DiffOptions::restamp_default_min_count`]).
    pub count: usize,
}

/// Top-level result of comparing two ESM files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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
    /// Count of `changed` records dropped entirely by noise suppression
    /// (`DiffOptions::suppress_noise`), keyed by record-type signature.
    /// Telemetry for renderers, e.g. "312 placement moves omitted".
    /// Also holds leaf-level counters for issue #22 shapes (e.g.
    /// `"padding_zeroed"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub suppressed_counts: BTreeMap<String, usize>,
    /// Serializer-default `(leaf_name, value, count)` rules the calibrated
    /// appearance-default pass (issue #22) auto-classified and applied,
    /// sorted by `count` descending. Empty when the pass did not run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_suppressed_defaults: Vec<SuppressedDefault>,
}

/// Compare two ESM databases and return a structured diff, using default
/// [`DiffOptions`] (full bodies on added/removed records, noise suppression
/// on, no type exclusions).
///
/// This is a thin wrapper around [`diff_databases_with`]; see that function
/// for the full behavior.
pub fn diff_databases(a: &Database, b: &Database) -> anyhow::Result<DiffResult> {
    diff_databases_with(a, b, &DiffOptions::default())
}

/// Compare two ESM databases and return a structured diff.
///
/// Records are aligned by raw FormID.  The decompressed data payload is
/// compared byte-for-byte to skip unchanged records (fast-path).  Only
/// changed records are decoded and field-diffed.
///
/// `opts.bodies` controls whether — and how deeply — added/removed record
/// stubs get a decoded `fields` payload (see [`BodyDetail`]).  Decode
/// failures are swallowed (`fields` stays `None`); a single bad record never
/// aborts the whole diff.
///
/// `opts.suppress_noise` strips known-noisy top-level fields (placement
/// transforms, CELL precombine bookkeeping, …) from each `changed` record's
/// `field_changes` — see [`strip_noise_fields`]. When the two sides'
/// `form_version`s also differ, further passes run: schema-gated
/// appearance/disappearance suppression (`noise::strip_version_gated_transitions`);
/// for the noise shapes that carry no schema gate at all (nested inside
/// `_array_diff` elements, all-zero `_raw` padding growth, and known
/// materialized-on-resave subrecords like INFO's PNAM chain link),
/// `noise::strip_restamp_appearances` (issue #18); then a global calibrated
/// appearance-default pass plus padding-zeroing suppression (issue #22) —
/// see `noise::apply_restamp_calibrated_suppression`. A record is dropped
/// entirely when nothing else changed. Dropped counts are recorded in
/// `DiffResult::suppressed_counts`; auto-classified defaults land in
/// `DiffResult::auto_suppressed_defaults`.
///
/// `opts.exclude_types` omits matching 4-character signatures from `added`,
/// `removed`, and `changed` outright — checked before any payload
/// decompression or decode for that record.
///
/// When either database has a localization table loaded, each `RecordStub`
/// is enriched with `name` (FULL) and `description` (DESC), and `DiffResult`
/// gains a `ref_names` sidecar mapping every FormID hex reference found in
/// `field_changes` (and in added/removed decoded bodies) to its resolved
/// record type, EditorID, name, and description.
pub fn diff_databases_with(
    a: &Database,
    b: &Database,
    opts: &DiffOptions,
) -> anyhow::Result<DiffResult> {
    let exclude_types: HashSet<String> = opts
        .exclude_types
        .iter()
        .map(|s| s.to_uppercase())
        .collect();
    let depth = opts.bodies.resolve_depth();

    let a_ids: HashSet<FormId> = a.index.iter_form_ids().collect();
    let b_ids: HashSet<FormId> = b.index.iter_form_ids().collect();

    // Added: in B but not A
    let mut added = Vec::new();
    for id in b_ids.difference(&a_ids) {
        let meta = b.index.get_by_formid(*id).expect("present in b_ids");
        if exclude_types.contains(meta.signature.as_str()) {
            continue;
        }
        let mut stub = record_stub_from_db(b, &meta, *id)?;
        // Decode fields best-effort (never aborts the diff on failure).
        if let Some(depth) = depth
            && let Ok(r) = b.record_by_formid_resolved(*id, depth)
        {
            stub.fields = Some(r.fields);
        }
        added.push(stub);
    }
    added.sort_by(|x, y| x.form_id.cmp(&y.form_id));

    // Removed: in A but not B
    let mut removed = Vec::new();
    for id in a_ids.difference(&b_ids) {
        let meta = a.index.get_by_formid(*id).expect("present in a_ids");
        if exclude_types.contains(meta.signature.as_str()) {
            continue;
        }
        let mut stub = record_stub_from_db(a, &meta, *id)?;
        // Old-side decode: any FormID refs resolve against A, which is
        // correct since the referenced records may no longer exist in B.
        if let Some(depth) = depth
            && let Ok(r) = a.record_by_formid_resolved(*id, depth)
        {
            stub.fields = Some(r.fields);
        }
        removed.push(stub);
    }
    removed.sort_by(|x, y| x.form_id.cmp(&y.form_id));

    // Common: compare payloads, decode only on mismatch
    let mut changed = Vec::new();
    // Parallel to `changed`: true when this record's form_versions differed
    // (gates the issue #18/#22 restamp passes). Kept aside so #22's global
    // frequency pass can run after the per-record loop.
    let mut changed_restamp: Vec<bool> = Vec::new();
    let mut suppressed_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut common_ids: Vec<FormId> = a_ids.intersection(&b_ids).copied().collect();
    common_ids.sort_by_key(|id| id.raw());

    for id in common_ids {
        let meta_a = a
            .index
            .get_by_formid(id)
            .expect("present in a_ids/b_ids intersection");
        let meta_b = b
            .index
            .get_by_formid(id)
            .expect("present in a_ids/b_ids intersection");

        if exclude_types.contains(meta_a.signature.as_str()) {
            continue;
        }

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
            .record_at_meta_with_depth(&meta_a, ResolveDepth::None)
            .with_context(|| format!("decode A for {}", id))?;
        let rb = b
            .record_at_meta_with_depth(&meta_b, ResolveDepth::None)
            .with_context(|| format!("decode B for {}", id))?;

        let mut field_changes = json_diff(&ra.fields, &rb.fields);
        if field_changes == Value::Object(serde_json::Map::new()) {
            continue; // decoded-equal despite byte differences (volatile header bytes)
        }

        let mut restamp = false;
        if opts.suppress_noise {
            strip_noise_fields(&mut field_changes, meta_b.signature.as_str());
            // Suppress only pure appearances/disappearances whose schema
            // activation actually changes between these form versions. The
            // old blanket appearance rule discarded genuine new subrecords
            // and could show only the removal half of a field swap.
            if meta_a.form_version != meta_b.form_version {
                restamp = true;
                strip_version_gated_transitions(
                    &mut field_changes,
                    &b.schema,
                    meta_b.signature.as_str(),
                    meta_a.form_version,
                    meta_b.form_version,
                );
                // Second, independent pass: appearances/disappearances with
                // NO schema gate at all that the re-save's newer serializer
                // still manufactures — see `noise`'s module docs above
                // `strip_restamp_appearances`.
                strip_restamp_appearances(&mut field_changes, meta_b.signature.as_str());
            }
            if is_empty_diff(&field_changes) {
                *suppressed_counts
                    .entry(meta_b.signature.as_str().to_owned())
                    .or_insert(0) += 1;
                continue;
            }
        }

        // Resolve name/description from the B-side raw subrecords.
        let (name, description) = resolve_stub_names(b, &meta_b);

        let stub = RecordStub {
            form_id: id.display(),
            editor_id: rb.editor_id.clone(),
            record_type: meta_b.signature.as_str().to_owned(),
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
        changed_restamp.push(restamp);
    }

    // Issue #22: padding-zeroing + calibrated appearance-default suppression.
    // Needs a global frequency pass over all `changed` records, so it runs
    // after the per-record loop. Still gated by `suppress_noise` and applied
    // only to records whose form_versions differed (same gate as #18).
    let auto_suppressed_defaults = if opts.suppress_noise && changed_restamp.iter().any(|&r| r) {
        apply_restamp_calibrated_suppression(
            &mut changed,
            &changed_restamp,
            &mut suppressed_counts,
            opts.restamp_default_min_count,
        )
    } else {
        Vec::new()
    };

    changed.sort_by(|x, y| x.stub.form_id.cmp(&y.stub.form_id));

    // Build ref_names: one-hop FormID resolution for every hex ref in field_changes
    // and added/removed records' decoded fields. Populated when either side has
    // localization or curves loaded, or is non-localized (FULL/DESC are inline
    // text there, so names resolve without any string table).
    let ref_names =
        if a.has_enrichment() || b.has_enrichment() || !a.is_localized || !b.is_localized {
            let mut refs: HashSet<String> = HashSet::new();
            for rd in &changed {
                collect_formid_refs(&rd.field_changes, &mut refs);
            }
            for stub in &added {
                if let Some(f) = &stub.fields {
                    collect_formid_refs(f, &mut refs);
                }
            }
            for stub in &removed {
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
        suppressed_counts,
        auto_suppressed_defaults,
    })
}

/// Resolve a display-name field (`FULL`/`DESC`) from raw subrecords, honoring
/// the ESM's localization mode. Localized files store a 4-byte LString ID
/// that must be looked up in the loaded `Localization` tables; non-localized
/// files (e.g. FO76's `SeventySix.esm`) store the text inline — no string
/// tables required.
fn resolve_name_field(
    db: &Database,
    subs: &[OwnedSubrecord],
    sig: &str,
    kind: StringKind,
) -> Option<String> {
    if db.is_localized {
        let lid = lstring_id_from_subrecords(subs, sig)?;
        db.localization
            .as_ref()?
            .lookup(kind, lid)
            .map(str::to_owned)
    } else {
        inline_string_from_subrecords(subs, sig)
    }
}

/// Build a `RecordStub` from a database, resolving name/description when
/// localization is available.
fn record_stub_from_db(
    db: &Database,
    meta: &crate::reader::RecordMeta,
    id: FormId,
) -> anyhow::Result<RecordStub> {
    let rec = db.parse_record_at(meta.offset)?;
    let editor_id = edid_from_subrecords(&rec.subrecords);
    let name = resolve_name_field(db, &rec.subrecords, "FULL", StringKind::Strings);
    let description = resolve_name_field(db, &rec.subrecords, "DESC", StringKind::DlStrings);

    Ok(RecordStub {
        form_id: id.display(),
        editor_id,
        record_type: meta.signature.as_str().to_owned(),
        offset: meta.offset,
        name,
        description,
        ..Default::default()
    })
}

/// Resolve FULL (name) and DESC (description) from the raw record at `meta.offset`.
/// Returns `(None, None)` on any parse error.
fn resolve_stub_names(
    db: &Database,
    meta: &crate::reader::RecordMeta,
) -> (Option<String>, Option<String>) {
    let rec = match db.parse_record_at(meta.offset) {
        Ok(r) => r,
        Err(_) => return (None, None),
    };
    (
        resolve_name_field(db, &rec.subrecords, "FULL", StringKind::Strings),
        resolve_name_field(db, &rec.subrecords, "DESC", StringKind::DlStrings),
    )
}

/// Return `true` if `s` is a FormID hex string as produced by `FormId::display()`:
/// exactly `0x` followed by 8 ASCII hex digits (case-insensitive).
pub(crate) fn is_formid_str(s: &str) -> bool {
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
        if let Some(meta) = db.index.get_by_formid(id) {
            let offset = meta.offset;
            let sig = meta.signature.as_str().to_owned();
            let rec = match db.parse_record_at(offset) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let editor_id = edid_from_subrecords(&rec.subrecords);
            let name = resolve_name_field(db, &rec.subrecords, "FULL", StringKind::Strings);
            let description =
                resolve_name_field(db, &rec.subrecords, "DESC", StringKind::DlStrings);
            return Some(RefName {
                record_type: sig,
                editor_id,
                name,
                description,
            });
        }
    }
    None
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
/// Arrays get per-element treatment via [`array_diff::array_diff`]: a
/// `keyed` diff when elements have a recognizable identity field,
/// `positional` when lengths match but no key is available, `set` for
/// arrays of primitives, and `unkeyed` (whole old/new element lists) only as
/// a last resort.
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
                    // An absent key and a present-but-null key both mean "no
                    // value", so pairing them is not a change. Emitting one
                    // produced `null -> null` rows, which no consumer can act on.
                    (Some(av), None) => {
                        if !av.is_null() {
                            out.insert(key.clone(), serde_json::json!({"from": av, "to": null}));
                        }
                    }
                    (None, Some(bv)) => {
                        if !bv.is_null() {
                            out.insert(key.clone(), serde_json::json!({"from": null, "to": bv}));
                        }
                    }
                    (None, None) => unreachable!(),
                }
            }
            Value::Object(out)
        }
        (Value::Array(aa), Value::Array(ba)) => array_diff::array_diff(aa, ba),
        (av, bv) if av == bv => Value::Object(serde_json::Map::new()),
        (av, bv) => serde_json::json!({"from": av, "to": bv}),
    }
}
