//! Pairwise diff engine for two versions of the same base ESM.
//!
//! Records are aligned by raw FormID. A byte-equality fast-path skips decoding
//! for unchanged records; only records with different payloads are decoded and
//! field-diffed via `json_diff`.

use crate::decode::ResolveDepth;
use crate::formid::{parse_formid, FormId};
use crate::reader::{
    edid_from_subrecords, inline_string_from_subrecords, lstring_id_from_subrecords, OwnedSubrecord,
};
use crate::strings::StringKind;
use crate::Database;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

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
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            bodies: BodyDetail::Full,
            suppress_noise: true,
            exclude_types: Vec::new(),
        }
    }
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
    /// Decoded fields for *added*/*removed* records, at the depth requested
    /// by `DiffOptions::bodies` (see [`BodyDetail`]). `None` when
    /// `BodyDetail::None` was requested, or when the record failed to
    /// decode. Always absent on `changed` stubs (see `RecordDiff::field_changes`
    /// instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    /// Count of `changed` records dropped entirely by noise suppression
    /// (`DiffOptions::suppress_noise`), keyed by record-type signature.
    /// Telemetry for renderers, e.g. "312 placement moves omitted".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub suppressed_counts: BTreeMap<String, usize>,
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
/// `field_changes`, dropping the record entirely when nothing else changed
/// — see [`strip_noise_fields`]. Dropped counts are recorded in
/// `DiffResult::suppressed_counts`.
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

    let a_ids: HashSet<FormId> = a.index.form_index.keys().copied().collect();
    let b_ids: HashSet<FormId> = b.index.form_index.keys().copied().collect();

    // Added: in B but not A
    let mut added = Vec::new();
    for id in b_ids.difference(&a_ids) {
        let meta = b.index.form_index[id].clone();
        if exclude_types.contains(meta.signature.as_str()) {
            continue;
        }
        let mut stub = record_stub_from_db(b, &meta, *id)?;
        // Decode fields best-effort (never aborts the diff on failure).
        if let Some(depth) = depth {
            if let Ok(r) = b.record_by_formid_resolved(*id, depth) {
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
        if exclude_types.contains(meta.signature.as_str()) {
            continue;
        }
        let mut stub = record_stub_from_db(a, &meta, *id)?;
        // Old-side decode: any FormID refs resolve against A, which is
        // correct since the referenced records may no longer exist in B.
        if let Some(depth) = depth {
            if let Ok(r) = a.record_by_formid_resolved(*id, depth) {
                stub.fields = Some(r.fields);
            }
        }
        removed.push(stub);
    }
    removed.sort_by(|x, y| x.form_id.cmp(&y.form_id));

    // Common: compare payloads, decode only on mismatch
    let mut changed = Vec::new();
    let mut suppressed_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut common_ids: Vec<FormId> = a_ids.intersection(&b_ids).copied().collect();
    common_ids.sort_by_key(|id| id.raw());

    for id in common_ids {
        let meta_a = a.index.form_index[&id].clone();
        let meta_b = b.index.form_index[&id].clone();

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

        if opts.suppress_noise {
            strip_noise_fields(&mut field_changes, &meta_b.signature);
            if is_empty_diff(&field_changes) {
                *suppressed_counts
                    .entry(meta_b.signature.clone())
                    .or_insert(0) += 1;
                continue;
            }
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
        record_type: meta.signature.clone(),
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

// ---------------------------------------------------------------------------
// Noise suppression
// ---------------------------------------------------------------------------
//
// Full-world ESM diffs (e.g. between two weekly snapshots) are dominated by
// mechanically-regenerated bookkeeping: precombined-mesh bookkeeping on CELL
// records, and position/scale churn on placement records that the game's
// tooling re-serializes on every save even when nothing gameplay-relevant
// moved. `strip_noise_fields` removes these known-noisy top-level keys from
// a record's `field_changes` so `diff_databases_with` can drop the record
// entirely when nothing else changed — see `DiffOptions::suppress_noise`.

/// Record types whose placement-transform fields are considered noise.
const PLACEMENT_TYPES: &[&str] = &["REFR", "ACHR", "PGRE", "PHZD"];

/// Top-level fields stripped for every record type.
const GLOBAL_NOISE_FIELDS: &[&str] = &["Object Bounds"];

/// Top-level fields stripped additionally for [`PLACEMENT_TYPES`].
const PLACEMENT_NOISE_FIELDS: &[&str] = &[
    "Position/Rotation",
    "Bound Half Extents",
    "Scale",
    "Radius",
    "Distant LOD Data",
];

/// Top-level fields stripped additionally for `CELL`.
const CELL_NOISE_FIELDS: &[&str] = &[
    "PreVis File Hash",
    "In PreVis File Of",
    "PreCombined Files Timestamp",
    "Combined References",
    "Physics References",
    "Combined Physics",
    "Precombined Object Level XY",
    "Precombined Object Level Z",
    "Max Height Data",
];

/// Strip known-noisy top-level keys from a decoded `field_changes` object, in
/// place. `GLOBAL_NOISE_FIELDS` are removed unconditionally; additionally
/// `PLACEMENT_NOISE_FIELDS` when `sig` is one of `PLACEMENT_TYPES`, and
/// `CELL_NOISE_FIELDS` when `sig == "CELL"`. Matching is an exact top-level
/// key match only — a same-named nested field (inside an array element or
/// substruct) is left untouched. A no-op when `field_changes` isn't a JSON
/// object (defensive; `json_diff` on two records always produces one).
pub fn strip_noise_fields(field_changes: &mut Value, sig: &str) {
    let Some(map) = field_changes.as_object_mut() else {
        return;
    };
    for key in GLOBAL_NOISE_FIELDS {
        map.remove(*key);
    }
    if PLACEMENT_TYPES.contains(&sig) {
        for key in PLACEMENT_NOISE_FIELDS {
            map.remove(*key);
        }
    }
    if sig == "CELL" {
        for key in CELL_NOISE_FIELDS {
            map.remove(*key);
        }
    }
}

/// Recursive JSON diff.  Returns a sparse object with only changed fields.
/// Arrays get per-element treatment via [`array_diff`]: a `keyed` diff when
/// elements have a recognizable identity field, `positional` when lengths
/// match but no key is available, `set` for arrays of primitives, and the
/// legacy opaque `{ "from": a, "to": b }` only as a last resort.
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
        (Value::Array(aa), Value::Array(ba)) => array_diff(aa, ba),
        (av, bv) if av == bv => Value::Object(serde_json::Map::new()),
        (av, bv) => serde_json::json!({"from": av, "to": bv}),
    }
}

// ---------------------------------------------------------------------------
// Keyed per-element array diffing
// ---------------------------------------------------------------------------
//
// Decoded rarray elements are almost always either uniform primitives (a
// FormID list) or single-member "rstruct" wrappers (`{"Leveled List Entry":
// {..}}`). Diffing them wholesale (the old opaque behavior) hides which
// entries actually changed inside a 50-element leveled list. The strategy
// below tries, in order: a schema-aware key (`element_key_spec`), positional
// pairing (equal length, no key), or a primitive multiset diff — falling
// back to the legacy opaque leaf only when none of those apply.

/// A resolved per-element keying strategy for [`array_diff`]: each inner
/// list gives the alternative field-name paths (dot-separated for one level
/// of nesting, e.g. `"INDX.Stage Index"`) for one key *component* — the
/// first alternative present on a given element wins for that component.
type KeySpec = Vec<Vec<String>>;

/// True for JSON scalars (string/number/bool/null) — anything that isn't an
/// object or array. Used to detect "arrays of primitives" for `set` diffing.
fn is_primitive_value(v: &Value) -> bool {
    !v.is_object() && !v.is_array()
}

/// `json_diff`'s canonical "no differences" sentinel: an empty JSON object.
fn is_empty_diff(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.is_empty())
}

/// The legacy opaque array diff — whole old/new arrays under `from`/`to`.
/// Used when array elements can't be paired meaningfully: heterogeneous
/// element shapes, or an unkeyable object shape with mismatched lengths.
fn opaque_array_diff(a: &[Value], b: &[Value]) -> Value {
    serde_json::json!({"from": Value::Array(a.to_vec()), "to": Value::Array(b.to_vec())})
}

/// Wrap a populated array-diff body in the `{"_array_diff": {...}}` envelope.
fn wrap_array_diff(inner: serde_json::Map<String, Value>) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("_array_diff".to_string(), Value::Object(inner));
    Value::Object(out)
}

/// Unwrap the single-member "rstruct" wrapper rarray elements are commonly
/// decoded into (e.g. `{"Leveled List Entry": {...}}`, `{"Effect": {...}}`).
/// Returns the wrapper's key name and the inner object to key on. Elements
/// that don't match the wrapper shape (not a single object-valued key) are
/// returned as-is with no wrapper name — key lookups against them then miss
/// every expected member, so they naturally end up with an all-null key.
fn unwrap_wrapper(
    m: &serde_json::Map<String, Value>,
) -> (Option<&str>, &serde_json::Map<String, Value>) {
    if m.len() == 1 {
        if let Some((k, Value::Object(inner))) = m.iter().next() {
            return (Some(k.as_str()), inner);
        }
    }
    (None, m)
}

/// True when `v` is a FormID reference as produced by the schema decoder:
/// either a bare hex string (see `is_formid_str`) or a resolved stub object
/// carrying a `"formid"` key.
fn is_formid_shaped(v: &Value) -> bool {
    match v {
        Value::String(s) => is_formid_str(s),
        Value::Object(m) => m.contains_key("formid"),
        _ => false,
    }
}

/// Resolve an array's per-element keying strategy from a sample element (the
/// first object found on either side), per a hardcoded table of known
/// rarray element shapes, falling back to generic heuristics. Returns `None`
/// when nothing applies — the caller then falls back to positional pairing
/// (equal lengths) or an opaque diff.
fn element_key_spec(sample: &serde_json::Map<String, Value>) -> Option<KeySpec> {
    let (wrapper, body) = unwrap_wrapper(sample);

    // 1. OMOD properties: composite (Function Type, Property) key.
    if body.contains_key("Function Type") && body.contains_key("Property") {
        return Some(vec![
            vec!["Function Type".to_string()],
            vec!["Property".to_string()],
        ]);
    }
    // 2. Leveled list entries: Reference/Item + Minimum Level/Level.
    if wrapper == Some("Leveled List Entry") {
        return Some(vec![
            vec!["Reference".to_string(), "Item".to_string()],
            vec!["Minimum Level".to_string(), "Level".to_string()],
        ]);
    }
    // 3. Magic effects, keyed by their base effect.
    if wrapper == Some("Effect") {
        return Some(vec![vec!["Base Effect".to_string()]]);
    }
    // 4. Recipe components / item-type entries.
    if body.contains_key("Component") {
        return Some(vec![vec!["Component".to_string()]]);
    }
    if body.contains_key("Item Type") {
        return Some(vec![vec!["Item Type".to_string()]]);
    }
    // 5. Quest objectives.
    if wrapper == Some("Objective") {
        return Some(vec![vec!["Objective Index".to_string()]]);
    }
    // 6. Quest stages — the index lives inside the nested INDX struct.
    if wrapper == Some("Stage") {
        return Some(vec![vec!["INDX.Stage Index".to_string()]]);
    }
    // 7. Single-reference entries.
    if body.contains_key("Faction") {
        return Some(vec![vec!["Faction".to_string()]]);
    }
    if body.contains_key("Perk") {
        return Some(vec![vec!["Perk".to_string()]]);
    }
    if body.contains_key("Mod") {
        return Some(vec![vec!["Mod".to_string()]]);
    }
    if body.contains_key("Keyword") && body.contains_key("Sound") {
        return Some(vec![vec!["Keyword".to_string()]]);
    }
    // 8. Generic "Index" / "* Index" member.
    let mut index_members: Vec<&String> = body
        .keys()
        .filter(|k| k.as_str() == "Index" || k.ends_with(" Index"))
        .collect();
    index_members.sort();
    if let Some(name) = index_members.into_iter().next() {
        return Some(vec![vec![name.clone()]]);
    }
    // 9. Exactly one FormID-shaped member.
    let formid_members: Vec<&String> = body
        .iter()
        .filter(|(_, v)| is_formid_shaped(v))
        .map(|(k, _)| k)
        .collect();
    if formid_members.len() == 1 {
        return Some(vec![vec![formid_members[0].clone()]]);
    }
    None
}

/// Look up a field path inside an element body — dot-separated paths reach
/// one level into a nested object member (e.g. `"INDX.Stage Index"`).
fn get_path<'a>(body: &'a serde_json::Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut cur = body.get(parts.next()?)?;
    for part in parts {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

/// Resolve one key component (a list of alternative field paths — the first
/// alternative present in `body` wins). Returns the alternative name that
/// was actually used (or the first alternative, if none matched) alongside
/// the raw value found (`None` when absent on this element).
fn resolve_key_component<'a>(
    alts: &'a [String],
    body: &'a serde_json::Map<String, Value>,
) -> (&'a str, Option<&'a Value>) {
    for alt in alts {
        if let Some(v) = get_path(body, alt) {
            return (alt.as_str(), Some(v));
        }
    }
    (alts[0].as_str(), None)
}

/// Canonicalize a key field's raw decoded value so schema drift between
/// snapshots — e.g. a bare int on one side vs an enum object
/// `{"value": int, "name": ..}` on the other, or a resolved-stub object vs a
/// bare FormID hex string — doesn't break key *matching* across sides. Used
/// only to compute the pairing group (`KeyInfo::group`); the `"key"` object
/// actually emitted on a `changed` entry is built separately by
/// [`display_key`] from the ORIGINAL (non-canonicalized) values, so a name
/// like `MUL+ADD` survives into the diff output instead of collapsing to its
/// bare enum int.
fn canonical_key_value(raw: Option<&Value>) -> Value {
    match raw {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Object(m)) => m
            .get("value")
            .or_else(|| m.get("formid"))
            .cloned()
            .unwrap_or_else(|| Value::Object(m.clone())),
        Some(other) => other.clone(),
    }
}

/// A single element's resolved matching key: a serialized canonical-value
/// tuple (see [`canonical_key_value`]) used to pair elements across sides.
/// This is for *matching* only — the display `"key"` object emitted on a
/// `changed` entry is computed separately by [`display_key`].
struct KeyInfo {
    group: String,
}

/// Compute an element's `KeyInfo` for `spec`. Elements that don't match the
/// wrapper shape `element_key_spec` was derived from simply fail every field
/// lookup, yielding an all-null key that (almost certainly) pairs with
/// nothing — they fall out as added/removed rather than panicking or being
/// silently dropped.
fn compute_key_info(elem: &Value, spec: &KeySpec) -> KeyInfo {
    // Callers only reach here after confirming every element is an object.
    let empty = serde_json::Map::new();
    let map = elem.as_object().unwrap_or(&empty);
    let (_, body) = unwrap_wrapper(map);

    let values: Vec<Value> = spec
        .iter()
        .map(|alts| canonical_key_value(resolve_key_component(alts, body).1))
        .collect();
    let group = serde_json::to_string(&values).unwrap_or_default();
    KeyInfo { group }
}

/// Build the display-ready `"key"` object for a `changed` pair: each key
/// component takes its ORIGINAL (non-canonicalized) value from `b_elem` (the
/// B/new side), falling back to `a_elem` (the A/old side) only when the
/// component is absent from `b_elem` entirely (every alternative name
/// missing). This differs from [`compute_key_info`]'s canonical group, which
/// exists purely so pairing survives schema drift (enum-object vs bare-int,
/// resolved-stub vs bare FormID hex) — the displayed key instead preserves
/// whichever representation the record actually carries, e.g.
/// `{"value": 1, "name": "MUL+ADD"}` rather than the collapsed `1`.
fn display_key(spec: &KeySpec, a_elem: &Value, b_elem: &Value) -> serde_json::Map<String, Value> {
    let empty = serde_json::Map::new();
    let a_map = a_elem.as_object().unwrap_or(&empty);
    let b_map = b_elem.as_object().unwrap_or(&empty);
    let (_, a_body) = unwrap_wrapper(a_map);
    let (_, b_body) = unwrap_wrapper(b_map);

    let mut fields = serde_json::Map::new();
    for alts in spec {
        let (b_name, b_raw) = resolve_key_component(alts, b_body);
        match b_raw {
            Some(v) => {
                fields.insert(b_name.to_string(), v.clone());
            }
            None => {
                let (a_name, a_raw) = resolve_key_component(alts, a_body);
                fields.insert(a_name.to_string(), a_raw.cloned().unwrap_or(Value::Null));
            }
        }
    }
    fields
}

/// Multiset ("set") diff for arrays of JSON primitives (numbers, strings,
/// bools, null) — e.g. a Keywords FormID list. Order doesn't matter, only
/// the multiset of values does: a value appearing twice on one side and
/// once on the other contributes a single `added`/`removed` entry (the
/// count delta), not two.
fn set_diff(a: &[Value], b: &[Value]) -> Value {
    let mut counts: BTreeMap<String, (Value, i64)> = BTreeMap::new();
    for v in a {
        let key = serde_json::to_string(v).unwrap_or_default();
        counts.entry(key).or_insert_with(|| (v.clone(), 0)).1 -= 1;
    }
    for v in b {
        let key = serde_json::to_string(v).unwrap_or_default();
        counts.entry(key).or_insert_with(|| (v.clone(), 0)).1 += 1;
    }

    let mut added = Vec::new();
    let mut removed = Vec::new();
    for (value, diff) in counts.into_values() {
        if diff > 0 {
            added.extend(std::iter::repeat_n(value, diff as usize));
        } else if diff < 0 {
            removed.extend(std::iter::repeat_n(value, (-diff) as usize));
        }
    }

    if added.is_empty() && removed.is_empty() {
        return Value::Object(serde_json::Map::new());
    }

    let mut out = serde_json::Map::new();
    out.insert("strategy".to_string(), Value::String("set".to_string()));
    out.insert("count_from".to_string(), serde_json::json!(a.len()));
    out.insert("count_to".to_string(), serde_json::json!(b.len()));
    if !added.is_empty() {
        out.insert("added".to_string(), Value::Array(added));
    }
    if !removed.is_empty() {
        out.insert("removed".to_string(), Value::Array(removed));
    }
    wrap_array_diff(out)
}

/// Index-aligned diff for two same-length arrays without a usable key.
fn positional_diff(a: &[Value], b: &[Value]) -> Value {
    let mut changed = Vec::new();
    for (i, (av, bv)) in a.iter().zip(b.iter()).enumerate() {
        let d = json_diff(av, bv);
        if !is_empty_diff(&d) {
            changed.push(serde_json::json!({
                "key": {"index": i},
                "index_from": i,
                "index_to": i,
                "changes": d,
            }));
        }
    }

    if changed.is_empty() {
        return Value::Object(serde_json::Map::new());
    }

    let mut out = serde_json::Map::new();
    out.insert(
        "strategy".to_string(),
        Value::String("positional".to_string()),
    );
    out.insert("count_from".to_string(), serde_json::json!(a.len()));
    out.insert("count_to".to_string(), serde_json::json!(b.len()));
    out.insert("changed".to_string(), Value::Array(changed));
    wrap_array_diff(out)
}

/// Keyed per-element diff. Elements are grouped by their canonical key
/// (`element_key_spec` + `compute_key_info`) and paired 1:1 within same-key
/// groups, in original array order — duplicate keys pair positionally
/// within their group. Leftover unpaired elements become `added`/`removed`.
/// Each `changed` entry's `"key"` is the *original* (non-canonicalized)
/// value from the pairing — see [`display_key`] — so e.g. an enum key
/// carries its `name` even though matching itself tolerated a bare int on
/// the other side.
fn keyed_diff(a: &[Value], b: &[Value], spec: &KeySpec) -> Value {
    let a_keys: Vec<KeyInfo> = a.iter().map(|v| compute_key_info(v, spec)).collect();
    let b_keys: Vec<KeyInfo> = b.iter().map(|v| compute_key_info(v, spec)).collect();

    let mut b_groups: HashMap<String, VecDeque<usize>> = HashMap::new();
    for (j, info) in b_keys.iter().enumerate() {
        b_groups.entry(info.group.clone()).or_default().push_back(j);
    }

    let mut matched_b: HashSet<usize> = HashSet::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for (i, a_info) in a_keys.iter().enumerate() {
        let paired = b_groups
            .get_mut(&a_info.group)
            .and_then(VecDeque::pop_front);
        match paired {
            Some(j) => {
                matched_b.insert(j);
                let d = json_diff(&a[i], &b[j]);
                if !is_empty_diff(&d) {
                    changed.push(serde_json::json!({
                        "key": Value::Object(display_key(spec, &a[i], &b[j])),
                        "index_from": i,
                        "index_to": j,
                        "changes": d,
                    }));
                }
            }
            None => removed.push(a[i].clone()),
        }
    }

    let mut added = Vec::new();
    for (j, elem) in b.iter().enumerate() {
        if !matched_b.contains(&j) {
            added.push(elem.clone());
        }
    }

    if added.is_empty() && removed.is_empty() && changed.is_empty() {
        return Value::Object(serde_json::Map::new());
    }

    let key_fields: Vec<String> = spec.iter().map(|alts| alts[0].clone()).collect();

    let mut out = serde_json::Map::new();
    out.insert("strategy".to_string(), Value::String("keyed".to_string()));
    out.insert("key_fields".to_string(), serde_json::json!(key_fields));
    out.insert("count_from".to_string(), serde_json::json!(a.len()));
    out.insert("count_to".to_string(), serde_json::json!(b.len()));
    if !added.is_empty() {
        out.insert("added".to_string(), Value::Array(added));
    }
    if !removed.is_empty() {
        out.insert("removed".to_string(), Value::Array(removed));
    }
    if !changed.is_empty() {
        out.insert("changed".to_string(), Value::Array(changed));
    }
    wrap_array_diff(out)
}

/// Per-element array diff, used by `json_diff`'s array arm. Classifies the
/// element shape into one of three pairing strategies:
///
/// - **keyed** — elements are rstructs with a recognizable identity field
///   ([`element_key_spec`]); paired by canonical key value regardless of
///   order or position.
/// - **positional** — same length, no usable key; paired index-for-index.
/// - **set** — uniform JSON primitives (e.g. a FormID keyword list); paired
///   as a multiset (order-insensitive, duplicate-aware).
///
/// Falls back to the legacy opaque `{"from": a, "to": b}` leaf when nothing
/// applies (heterogeneous shapes, or an unkeyable object shape with
/// mismatched lengths). Returns an empty object when the chosen strategy
/// finds no differences — e.g. a reorder-only keyed array — matching
/// `json_diff`'s convention of omitting unchanged fields entirely.
fn array_diff(a: &[Value], b: &[Value]) -> Value {
    if a == b {
        return Value::Object(serde_json::Map::new());
    }

    if a.iter().chain(b.iter()).all(is_primitive_value) {
        return set_diff(a, b);
    }

    if !a.iter().chain(b.iter()).all(Value::is_object) {
        // Heterogeneous element shapes (mixed primitive/object, nested
        // arrays, …) aren't classifiable — keep the legacy opaque form.
        return opaque_array_diff(a, b);
    }

    let Some(sample) = a.iter().chain(b.iter()).find_map(Value::as_object) else {
        // Unreachable in practice (all-object + not `a == b` implies at
        // least one element exists), kept as a defensive fallback.
        return opaque_array_diff(a, b);
    };

    match element_key_spec(sample) {
        Some(spec) => keyed_diff(a, b, &spec),
        None if a.len() == b.len() => positional_diff(a, b),
        None => opaque_array_diff(a, b),
    }
}
