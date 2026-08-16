//! Noise suppression: the three sequential sub-stages `diff_databases_with`
//! runs over a `changed` record's `field_changes` before deciding whether to
//! keep or drop it entirely (see `DiffOptions::suppress_noise`). Stage order
//! is load-bearing and must not change: unconditional → version-gated →
//! restamp-#18 → calibrated-#22 (the last needs a global cross-record
//! frequency pass, hence runs after the per-record loop in
//! `diff_databases_with`). Each stage may only ever *narrow* `field_changes`
//! — over-stripping silently hides real patch-notes content.
//!
//! # Unconditional suppression
//!
//! Full-world ESM diffs (e.g. between two weekly snapshots) are dominated by
//! mechanically-regenerated bookkeeping: precombined-mesh bookkeeping on CELL
//! records, and position/scale churn on placement records that the game's
//! tooling re-serializes on every save even when nothing gameplay-relevant
//! moved. `strip_noise_fields` removes these known-noisy top-level keys from
//! a record's `field_changes` so `diff_databases_with` can drop the record
//! entirely when nothing else changed — see `DiffOptions::suppress_noise`.

use super::array_diff::is_empty_diff;
use super::{RecordDiff, SuppressedDefault};
use crate::schema::{MemberDef, Schema};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

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

/// True when every leaf under `v` has a null value on `null_side`.
///
/// An `_array_diff` carrying added or removed elements is a real structural
/// change and never counts as a pure transition.
fn is_pure_transition(v: &Value, null_side: &str) -> bool {
    let Some(map) = v.as_object() else {
        return false;
    };
    if map.len() == 2 && map.contains_key("from") && map.contains_key("to") {
        return map.get(null_side).is_some_and(Value::is_null);
    }
    if let Some(ad) = map.get("_array_diff").and_then(Value::as_object) {
        if ad.contains_key("added") || ad.contains_key("removed") {
            return false;
        }
        return ad
            .get("changed")
            .and_then(Value::as_array)
            .is_some_and(|cs| {
                cs.iter().all(|c| {
                    c.get("changes")
                        .is_some_and(|changes| is_pure_transition(changes, null_side))
                })
            });
    }
    let mut saw = false;
    for (k, x) in map.iter() {
        if k.starts_with('_') {
            continue;
        }
        saw = true;
        if !is_pure_transition(x, null_side) {
            return false;
        }
    }
    saw
}

/// True when every leaf under `v` is a `null -> <something>` transition.
fn is_pure_appearance(v: &Value) -> bool {
    is_pure_transition(v, "from")
}

/// True when every leaf under `v` is a `<something> -> null` transition.
fn is_pure_disappearance(v: &Value) -> bool {
    is_pure_transition(v, "to")
}

/// Return the form-version activation bounds used by `decode::member_version_ok`.
fn member_version_bounds(member: &MemberDef) -> (Option<u16>, Option<u16>) {
    match member {
        MemberDef::Struct {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Integer {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Float {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Unused {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Empty {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Bytes {
            from_version,
            below_version,
            ..
        }
        | MemberDef::FormId {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        _ => (None, None),
    }
}

/// Mirror `decode::member_version_ok`, including the strict below-version bound.
fn member_version_active(member: &MemberDef, form_version: u16) -> bool {
    let (from_version, below_version) = member_version_bounds(member);
    from_version.is_none_or(|v| form_version >= v) && below_version.is_none_or(|v| form_version < v)
}

fn member_name(member: &MemberDef) -> Option<&str> {
    match member {
        MemberDef::Struct { name, .. }
        | MemberDef::RStruct { name, .. }
        | MemberDef::RArray { name, .. }
        | MemberDef::Array { name, .. }
        | MemberDef::Union { name, .. }
        | MemberDef::Integer { name, .. }
        | MemberDef::Float { name, .. }
        | MemberDef::String { name, .. }
        | MemberDef::LString { name, .. }
        | MemberDef::FormId { name, .. }
        | MemberDef::Bytes { name, .. }
        | MemberDef::ByteRgba { name, .. }
        | MemberDef::Vec3 { name, .. }
        | MemberDef::Empty { name, .. }
        | MemberDef::Unknown { name, .. }
        | MemberDef::RawFallback { name, .. }
        | MemberDef::Vmad { name, .. }
        | MemberDef::Ctda { name, .. }
        | MemberDef::ModelInfo { name, .. } => Some(name),
        MemberDef::Unused { .. } => None,
    }
}

/// Whether this member or any descendant crosses the requested activation state.
fn member_subtree_crosses_gate(
    member: &MemberDef,
    fv_a: u16,
    fv_b: u16,
    active_a: bool,
    active_b: bool,
) -> bool {
    if member_version_active(member, fv_a) == active_a
        && member_version_active(member, fv_b) == active_b
    {
        return true;
    }

    match member {
        MemberDef::Struct { fields, .. } => fields
            .iter()
            .any(|m| member_subtree_crosses_gate(m, fv_a, fv_b, active_a, active_b)),
        MemberDef::RStruct { members, .. } => members
            .iter()
            .any(|m| member_subtree_crosses_gate(m, fv_a, fv_b, active_a, active_b)),
        MemberDef::RArray { element, .. } | MemberDef::Array { element, .. } => {
            member_subtree_crosses_gate(element, fv_a, fv_b, active_a, active_b)
        }
        MemberDef::Union { variants, .. } => variants
            .iter()
            .any(|m| member_subtree_crosses_gate(m, fv_a, fv_b, active_a, active_b)),
        _ => false,
    }
}

/// Drop only pure top-level transitions explained by schema activation changes.
///
/// A pure appearance is removed only when the corresponding top-level schema
/// member or one of its descendants changes from inactive to active. A pure
/// disappearance is handled symmetrically for active-to-inactive transitions.
/// The former blanket `null -> X` rule was incorrect: it discarded genuine new
/// ungated subrecords and hid the addition half of field swaps.
pub(crate) fn strip_version_gated_transitions(
    field_changes: &mut Value,
    schema: &Schema,
    sig: &str,
    fv_a: u16,
    fv_b: u16,
) {
    let Some(map) = field_changes.as_object_mut() else {
        return;
    };
    let Some(record) = schema.record(sig) else {
        return;
    };

    map.retain(|key, value| {
        let mut matching_members = record
            .members
            .iter()
            .filter(|member| member_name(member) == Some(key.as_str()));
        if is_pure_appearance(value) {
            !matching_members
                .clone()
                .any(|member| member_subtree_crosses_gate(member, fv_a, fv_b, false, true))
        } else if is_pure_disappearance(value) {
            !matching_members
                .any(|member| member_subtree_crosses_gate(member, fv_a, fv_b, true, false))
        } else {
            true
        }
    });
}

// ---------------------------------------------------------------------------
// Restamp-appearance suppression (issue #18)
// ---------------------------------------------------------------------------
//
// `strip_version_gated_transitions` only catches noise explained by a schema
// `from_version`/`below_version` gate crossing between the two form_versions.
// A PTS re-save that bumps `form_version` (without any authored edit)
// produces three noise shapes with NO schema gate at all, so they survive
// that pass untouched:
//
//   (a) appearances nested inside `_array_diff.changed[].changes` — e.g. INFO
//       `Responses[].Response.Unknown: null -> {"hex":"00","_raw":true}`;
//   (b) `_raw` trailing-byte growth on schema `kind: "unknown"` members — the
//       newer serializer writes more padding bytes than the older one left
//       implicit, e.g. STAT `Unknown: null -> {"hex":"00000000...","_raw":true}`;
//   (c) subrecords the newer serializer materializes on re-save that the
//       older one left implicit — e.g. INFO `Previous INFO: null -> <formid>`
//       (the PNAM chain link).
//
// `strip_restamp_appearances` is a second, independent pass (schema-free —
// unlike `strip_version_gated_transitions` it needs no `&Schema`, only the
// record signature for rule (c)'s table lookup) that catches these. It uses
// a different traversal shape (arbitrary-depth recursion into `_array_diff`
// elements and plain nested structs, with empty-parent pruning) rather than
// an extension of `strip_version_gated_transitions`'s one-level `retain`
// loop, so the two stay independently testable.

/// `(record signature, top-level member name)` pairs whose `null -> X`
/// appearance is known to be a subrecord the game's serializer now writes
/// explicitly on re-save where an older serializer left it implicit — not an
/// authored edit. Directional only: an `X -> null` disappearance of one of
/// these members could be a genuine authored unlink (e.g. clearing a PNAM
/// dialogue-chain link) and must stay visible.
const MATERIALIZED_ON_RESAVE: &[(&str, &str)] = &[("INFO", "Previous INFO")];

/// True when `hex` is a well-formed (even-length) hex-digit string that
/// decodes to all-zero bytes. Malformed input (odd length, non-hex digits)
/// is conservatively treated as non-zero — only unambiguous all-zero padding
/// is ever eligible for suppression.
fn is_zero_hex(hex: &str) -> bool {
    hex.len().is_multiple_of(2) && hex.bytes().all(|b| b == b'0')
}

/// True when `v` is the decoder's raw-bytes-fallback shape `{"hex": H,
/// "_raw": true}` (see `decode::mod::hex`, the `MemberDef::Unknown` /
/// no-matching-union-variant fallback) and `H` is all-zero (see
/// [`is_zero_hex`]).
fn is_zero_raw_blob(v: &Value) -> bool {
    let Some(map) = v.as_object() else {
        return false;
    };
    map.get("_raw") == Some(&Value::Bool(true))
        && map
            .get("hex")
            .and_then(Value::as_str)
            .is_some_and(is_zero_hex)
}

/// Rule (b): a leaf `{"from": F, "to": T}` change is restamp noise when
/// either side is `null` and the other is an all-zero `_raw` blob
/// ([`is_zero_raw_blob`]). Symmetric — zero-padding growing OR shrinking
/// across a re-save carries no information in either direction (see design
/// review Q2, in contrast to rule (c) which stays directional).
fn is_zero_raw_transition(from: &Value, to: &Value) -> bool {
    (from.is_null() && is_zero_raw_blob(to)) || (to.is_null() && is_zero_raw_blob(from))
}

/// Rule (c): a leaf `{"from": null, "to": X}` appearance is restamp noise
/// when `(sig, key)` is one of [`MATERIALIZED_ON_RESAVE`]'s tabled pairs.
/// Never symmetric — see that constant's doc comment.
fn is_materialized_on_resave(sig: &str, key: &str, from: &Value, to: &Value) -> bool {
    from.is_null() && !to.is_null() && MATERIALIZED_ON_RESAVE.contains(&(sig, key))
}

/// Strip a `_array_diff` envelope's noise in place (`ad` is the object under
/// the `"_array_diff"` key: `strategy`/`key_fields`/`count_from`/`count_to`/
/// `added`/`removed`/`changed`). Walks every `changed[]` element's `changes`
/// recursively via [`strip_restamp_leaves`], drops elements whose `changes`
/// collapses to empty, and drops the `"changed"` key itself when every
/// element was dropped. Returns `true` when the whole envelope is now
/// noise-free and should be removed by the caller: `added`/`removed` were
/// never present (rule (a) never touches genuine structural changes — see
/// `is_pure_transition`'s same reasoning) and `changed` is now empty or
/// absent.
///
/// `count_from == count_to` is NOT re-derived or asserted here as a
/// correctness dependency — it's a defensive invariant that already holds
/// whenever `added`/`removed` are both absent, by construction of the two
/// strategies that populate `changed[]`: `positional_diff` only runs when
/// `a.len() == b.len()`, and `keyed_diff`'s empty `added`/`removed` means
/// every element paired 1:1 (see those functions' doc comments) — so a
/// mismatch here would itself indicate a bug in an array-diff builder, not
/// in this pass.
fn strip_restamp_array_diff(ad: &mut serde_json::Map<String, Value>, sig: &str) -> bool {
    if let Some(Value::Array(elements)) = ad.get_mut("changed") {
        elements.retain_mut(|elem| {
            let Some(changes) = elem.get_mut("changes").and_then(Value::as_object_mut) else {
                return true;
            };
            strip_restamp_leaves(changes, sig);
            !changes.is_empty()
        });
        if elements.is_empty() {
            ad.remove("changed");
        }
    }

    !(ad.contains_key("added") || ad.contains_key("removed") || ad.contains_key("changed"))
}

/// Strip restamp-noise leaves from `value` (the entry keyed by `key` in a
/// record of type `sig`) in place, and report whether the parent map should
/// drop `key` entirely. Three shapes:
///
/// - a leaf `{"from": .., "to": ..}` change — tested directly against rules
///   (b)/(c), no recursion;
/// - an `_array_diff` envelope — delegates to [`strip_restamp_array_diff`];
/// - a plain nested struct object (e.g. `Responses[].changes.Response`, one
///   level of struct nesting under an array element's `changes`, per the
///   design review) — recurse via [`strip_restamp_leaves`], then drop if the
///   recursion emptied it.
fn should_drop_after_strip(value: &mut Value, sig: &str, key: &str) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };

    if let Some(Value::Object(ad)) = obj.get_mut("_array_diff") {
        return strip_restamp_array_diff(ad, sig);
    }

    if obj.len() == 2 && obj.contains_key("from") && obj.contains_key("to") {
        let from = &obj["from"];
        let to = &obj["to"];
        return is_zero_raw_transition(from, to) || is_materialized_on_resave(sig, key, from, to);
    }

    strip_restamp_leaves(obj, sig);
    obj.is_empty()
}

/// Recursively strip restamp-noise leaves from a `changes`-shaped object
/// map, in place, pruning parent keys whose value collapses to empty. See
/// [`should_drop_after_strip`] for the per-key logic; this just drives it
/// over every key in `map` and removes the ones it flags.
fn strip_restamp_leaves(map: &mut serde_json::Map<String, Value>, sig: &str) {
    let keys: Vec<String> = map.keys().cloned().collect();
    let mut to_remove = Vec::new();
    for key in keys {
        if let Some(value) = map.get_mut(&key)
            && should_drop_after_strip(value, sig, &key)
        {
            to_remove.push(key);
        }
    }
    for key in to_remove {
        map.remove(&key);
    }
}

/// Drop restamp-only appearances/disappearances (see module docs above) from
/// `field_changes`, in place. Schema-free — unlike
/// [`strip_version_gated_transitions`] this doesn't need a `&Schema`, only
/// `sig` for rule (c)'s table lookup. A no-op when `field_changes` isn't a
/// JSON object (defensive; `json_diff` on two records always produces one).
pub(crate) fn strip_restamp_appearances(field_changes: &mut Value, sig: &str) {
    let Some(map) = field_changes.as_object_mut() else {
        return;
    };
    strip_restamp_leaves(map, sig);
}

// ---------------------------------------------------------------------------
// Restamp calibrated-default + padding-zero suppression (issue #22)
// ---------------------------------------------------------------------------
//
// After #18, a PTS form_version bump still leaves two residual noise shapes:
//
//   (d) padding-zeroing — a `_raw` hex leaf whose value goes from garbage
//       bytes to all zeros, e.g. `{"hex": {"from": "3809c7", "to": "000000"}}`.
//       Both sides are present (so not an appearance); the newer serializer
//       deterministically zeroed uninitialized padding. Structurally
//       unambiguous — strip unconditionally, no frequency test.
//
//   (e) calibrated appearance defaults — `null → <engine default>` leaves
//       whose `(leaf_name, value)` pair repeats across ≥N records in the
//       same diff and never also appears as a genuine authored edit. Leaf
//       name (last path segment, `[N]`/`[]` stripped) is the key — not the
//       full path — so the same serializer constant at
//       `Model.Enlighten Auto UV` / `Female.World Model.Enlighten Auto UV` /
//       `Male.World Model.Enlighten Auto UV` collapses into one rule.
//
// Both run only when form_versions differ (same gate as #18), and only when
// `DiffOptions::suppress_noise` is on. The calibrated pass needs a global
// frequency count, so it runs after the per-record loop.

/// Deterministic JSON serialization used as a HashMap key for appearance
/// values. `serde_json::to_string` is stable for a given `Value` shape as
/// produced by the decoder (schema field order).
fn canonical_json(v: &Value) -> String {
    serde_json::to_string(v).expect("serde_json::Value always serializes")
}

/// Last path segment with any `[N]`/`[]` index suffix stripped.
/// `"Effects[].Effect.Cooldown Duration"` → `"Cooldown Duration"`.
fn leaf_name(path: &str) -> &str {
    let last = path.rsplit('.').next().unwrap_or(path);
    match last.find('[') {
        Some(i) => &last[..i],
        None => last,
    }
}

/// True when `v` is a leaf `{"from": .., "to": ..}` change.
fn is_from_to_leaf(v: &Value) -> bool {
    v.as_object()
        .is_some_and(|m| m.len() == 2 && m.contains_key("from") && m.contains_key("to"))
}

/// True when `v` is the residual `_raw` hex-diff shape
/// `{"hex": {"from": <str>, "to": <all-zeros, non-empty>}}` — garbage padding
/// bytes zeroed by a newer serializer (issue #22 rule (d)).
fn is_padding_zeroed_value(v: &Value) -> bool {
    let Some(map) = v.as_object() else {
        return false;
    };
    // After json_diff, equal `_raw: true` on both sides is omitted, leaving
    // exactly the `hex` key with a from/to string change.
    if map.len() != 1 {
        return false;
    }
    is_padding_zeroed_hex_diff(map.get("hex").unwrap_or(&Value::Null))
}

/// True when `v` is `{"from": <hex str>, "to": <all-zeros, non-empty>}`.
fn is_padding_zeroed_hex_diff(v: &Value) -> bool {
    let Some(m) = v.as_object() else {
        return false;
    };
    if m.len() != 2 || !m.contains_key("from") || !m.contains_key("to") {
        return false;
    }
    let Some(from) = m.get("from").and_then(Value::as_str) else {
        return false;
    };
    let Some(to) = m.get("to").and_then(Value::as_str) else {
        return false;
    };
    !from.is_empty() && !to.is_empty() && to.bytes().all(|b| b == b'0')
}

/// Walk every from/to leaf under a `field_changes` tree, invoking `f(path, leaf)`.
/// Paths use `.` for nesting and append `[]` when descending into an
/// `_array_diff` envelope (matching the issue #22 leaf-name examples).
fn walk_diff_leaves(v: &Value, path: &str, f: &mut dyn FnMut(&str, &Value)) {
    let Some(map) = v.as_object() else {
        return;
    };

    if is_from_to_leaf(v) {
        f(path, v);
        return;
    }

    if let Some(ad) = map.get("_array_diff").and_then(Value::as_object) {
        if let Some(elems) = ad.get("changed").and_then(Value::as_array) {
            for elem in elems {
                if let Some(changes) = elem.get("changes") {
                    walk_diff_leaves(changes, path, f);
                }
            }
        }
        return;
    }

    for (k, child) in map {
        if k.starts_with('_') {
            continue;
        }
        let child_path = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        let child_path = if child
            .as_object()
            .is_some_and(|m| m.contains_key("_array_diff"))
        {
            format!("{child_path}[]")
        } else {
            child_path
        };
        walk_diff_leaves(child, &child_path, f);
    }
}

/// `(leaf_name, canonical_json(value))` key used by the calibrated-default pass.
type DefaultKey = (String, String);

/// Appearance frequency map: key → `(representative value, count)`.
type AppearanceCounts = HashMap<DefaultKey, (Value, usize)>;

/// Collect global appearance frequencies and the "seen as real edit" set
/// across every `changed` record (issue #22 pass 1).
fn collect_restamp_default_stats(
    changed: &[RecordDiff],
) -> (AppearanceCounts, HashSet<DefaultKey>) {
    let mut appearance_counts: AppearanceCounts = HashMap::new();
    let mut real_edits: HashSet<DefaultKey> = HashSet::new();

    for rd in changed {
        walk_diff_leaves(&rd.field_changes, "", &mut |path, leaf| {
            let Some(map) = leaf.as_object() else {
                return;
            };
            let from = map.get("from").unwrap_or(&Value::Null);
            let to = map.get("to").unwrap_or(&Value::Null);
            let name = leaf_name(path).to_owned();

            // Padding-zero hex leaves are neither appearances nor real edits.
            if name == "hex" && is_padding_zeroed_hex_diff(leaf) {
                return;
            }

            if from.is_null() && !to.is_null() {
                let key = canonical_json(to);
                appearance_counts
                    .entry((name, key))
                    .and_modify(|(_, c)| *c += 1)
                    .or_insert_with(|| (to.clone(), 1));
            } else if !to.is_null() {
                // Genuine authored value change (both sides present, or a
                // non-null `to` that isn't an appearance). Record only the
                // `to` value — a disappearance `V → null` must NOT poison
                // the appearance `null → V` (the old value being removed is
                // not an authored write of V). Recording `to` is what blocks
                // a real bulk rollout that happens to share a constant with
                // a serializer default.
                real_edits.insert((name, canonical_json(to)));
            }
        });
    }

    (appearance_counts, real_edits)
}

/// Strip padding-zeroed `_raw` hex leaves (issue #22 rule (d)) from an
/// `_array_diff` envelope. Returns the number of leaves stripped; `true` when
/// the envelope is now empty of structural content and should be dropped.
fn strip_padding_zeroed_array_diff(ad: &mut serde_json::Map<String, Value>) -> (usize, bool) {
    let mut stripped = 0;
    if let Some(Value::Array(elements)) = ad.get_mut("changed") {
        elements.retain_mut(|elem| {
            let Some(changes) = elem.get_mut("changes").and_then(Value::as_object_mut) else {
                return true;
            };
            stripped += strip_padding_zeroed_map(changes);
            !changes.is_empty()
        });
        if elements.is_empty() {
            ad.remove("changed");
        }
    }
    let empty =
        !(ad.contains_key("added") || ad.contains_key("removed") || ad.contains_key("changed"));
    (stripped, empty)
}

/// Strip padding-zeroed leaves from a changes-shaped map; returns count stripped.
fn strip_padding_zeroed_map(map: &mut serde_json::Map<String, Value>) -> usize {
    let keys: Vec<String> = map.keys().cloned().collect();
    let mut stripped = 0;
    let mut to_remove = Vec::new();
    for key in keys {
        let Some(value) = map.get_mut(&key) else {
            continue;
        };
        let (n, drop) = strip_padding_zeroed_value(value);
        stripped += n;
        if drop {
            to_remove.push(key);
        }
    }
    for key in to_remove {
        map.remove(&key);
    }
    stripped
}

/// Strip padding-zero noise from `value` in place. Returns `(leaves_stripped,
/// should_drop_from_parent)`.
fn strip_padding_zeroed_value(value: &mut Value) -> (usize, bool) {
    if is_padding_zeroed_value(value) {
        return (1, true);
    }
    let Some(obj) = value.as_object_mut() else {
        return (0, false);
    };
    if let Some(Value::Object(ad)) = obj.get_mut("_array_diff") {
        let (n, empty) = strip_padding_zeroed_array_diff(ad);
        return (n, empty);
    }
    // Nested struct (or a hex-bearing object with sibling keys): strip hex
    // padding-zero if present, then recurse.
    let mut stripped = 0;
    if obj.get("hex").is_some_and(is_padding_zeroed_hex_diff) {
        obj.remove("hex");
        stripped += 1;
    }
    stripped += strip_padding_zeroed_map(obj);
    (stripped, obj.is_empty())
}

/// Strip all padding-zeroed `_raw` hex leaves from `field_changes`. Returns
/// the number of leaves removed.
fn strip_padding_zeroed(field_changes: &mut Value) -> usize {
    let Some(map) = field_changes.as_object_mut() else {
        return 0;
    };
    strip_padding_zeroed_map(map)
}

/// Strip calibrated appearance-default leaves matching `suppressible` from an
/// `_array_diff` envelope. Returns `true` when the envelope should be dropped.
fn strip_calibrated_array_diff(
    ad: &mut serde_json::Map<String, Value>,
    path: &str,
    suppressible: &HashSet<DefaultKey>,
) -> bool {
    if let Some(Value::Array(elements)) = ad.get_mut("changed") {
        elements.retain_mut(|elem| {
            let Some(changes) = elem.get_mut("changes").and_then(Value::as_object_mut) else {
                return true;
            };
            strip_calibrated_map(changes, path, suppressible);
            !changes.is_empty()
        });
        if elements.is_empty() {
            ad.remove("changed");
        }
    }
    !(ad.contains_key("added") || ad.contains_key("removed") || ad.contains_key("changed"))
}

/// Strip matching appearance leaves from a changes-shaped map, in place.
fn strip_calibrated_map(
    map: &mut serde_json::Map<String, Value>,
    path_prefix: &str,
    suppressible: &HashSet<DefaultKey>,
) {
    let keys: Vec<String> = map.keys().cloned().collect();
    let mut to_remove = Vec::new();
    for key in keys {
        let child_path = if path_prefix.is_empty() {
            key.clone()
        } else {
            format!("{path_prefix}.{key}")
        };
        let Some(value) = map.get_mut(&key) else {
            continue;
        };
        let child_path = if value
            .as_object()
            .is_some_and(|m| m.contains_key("_array_diff"))
        {
            format!("{child_path}[]")
        } else {
            child_path
        };
        if should_drop_calibrated(value, &child_path, suppressible) {
            to_remove.push(key);
        }
    }
    for key in to_remove {
        map.remove(&key);
    }
}

/// Strip calibrated defaults from `value` in place; return whether the parent
/// should drop this key (value emptied or leaf matched).
fn should_drop_calibrated(
    value: &mut Value,
    path: &str,
    suppressible: &HashSet<DefaultKey>,
) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };

    if let Some(Value::Object(ad)) = obj.get_mut("_array_diff") {
        return strip_calibrated_array_diff(ad, path, suppressible);
    }

    if obj.len() == 2 && obj.contains_key("from") && obj.contains_key("to") {
        if obj.get("from").is_some_and(Value::is_null)
            && let Some(to) = obj.get("to")
            && !to.is_null()
        {
            let pair = (leaf_name(path).to_owned(), canonical_json(to));
            return suppressible.contains(&pair);
        }
        return false;
    }

    strip_calibrated_map(obj, path, suppressible);
    obj.is_empty()
}

/// Strip appearance leaves whose `(leaf_name, value)` is in `suppressible`.
fn strip_calibrated_defaults(field_changes: &mut Value, suppressible: &HashSet<DefaultKey>) {
    let Some(map) = field_changes.as_object_mut() else {
        return;
    };
    strip_calibrated_map(map, "", suppressible);
}

/// Issue #22 second-stage suppression: padding-zeroing + calibrated
/// appearance defaults. Mutates `changed` in place (drops emptied restamp
/// records), updates `suppressed_counts`, and returns the audit list of
/// auto-classified defaults (sorted by count descending).
pub(crate) fn apply_restamp_calibrated_suppression(
    changed: &mut Vec<RecordDiff>,
    restamp: &[bool],
    suppressed_counts: &mut BTreeMap<String, usize>,
    min_count: usize,
) -> Vec<SuppressedDefault> {
    debug_assert_eq!(changed.len(), restamp.len());

    let (appearance_counts, real_edits) = collect_restamp_default_stats(changed);

    let mut auto_suppressed_defaults: Vec<SuppressedDefault> = appearance_counts
        .into_iter()
        .filter(|((name, key), (_, count))| {
            *count >= min_count && !real_edits.contains(&(name.clone(), key.clone()))
        })
        .map(|((leaf_name, _), (value, count))| SuppressedDefault {
            leaf_name,
            value,
            count,
        })
        .collect();
    auto_suppressed_defaults.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.leaf_name.cmp(&b.leaf_name))
    });

    let suppressible: HashSet<DefaultKey> = auto_suppressed_defaults
        .iter()
        .map(|s| (s.leaf_name.clone(), canonical_json(&s.value)))
        .collect();

    let mut kept = Vec::with_capacity(changed.len());
    for (rd, is_restamp) in changed.drain(..).zip(restamp.iter().copied()) {
        let mut rd = rd;
        if is_restamp {
            let n = strip_padding_zeroed(&mut rd.field_changes);
            if n > 0 {
                *suppressed_counts
                    .entry("padding_zeroed".to_owned())
                    .or_insert(0) += n;
            }
            if !suppressible.is_empty() {
                strip_calibrated_defaults(&mut rd.field_changes, &suppressible);
            }
            if is_empty_diff(&rd.field_changes) {
                *suppressed_counts
                    .entry(rd.stub.record_type.clone())
                    .or_insert(0) += 1;
                continue;
            }
        }
        kept.push(rd);
    }
    *changed = kept;

    auto_suppressed_defaults
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::RecordStub;
    use serde_json::json;

    // `is_pure_appearance` / `strip_version_gated_transitions` are private, so
    // these live here per the crate convention. The risk being covered is
    // OVER-suppression: dropping an authored edit as if it were a field that
    // merely switched on with a newer form_version.

    fn test_schema(members: Value) -> Schema {
        Schema::from_json(
            &json!({
                "records": {
                    "TEST": {
                        "name": "Test",
                        "members": members,
                    }
                }
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn appearance_detects_null_to_value() {
        assert!(is_pure_appearance(
            &json!({"from": null, "to": "0x0000000F"})
        ));
        assert!(is_pure_appearance(&json!({
            "Enlighten Auto UV": {"from": null, "to": {"Unknown": 1}},
            "Unknown CTRN": {"from": null, "to": {"hex": "00"}},
        })));
    }

    #[test]
    fn appearance_rejects_authored_edits() {
        // A real value change, even to null, is not an appearance.
        assert!(!is_pure_appearance(&json!({"from": 5.0, "to": 20.0})));
        assert!(!is_pure_appearance(&json!({"from": "0x0F", "to": null})));
        // One authored edit anywhere disqualifies the whole subtree.
        assert!(!is_pure_appearance(&json!({
            "Unknown CTRN":  {"from": null, "to": {"hex": "00"}},
            "Attack Damage": {"from": 30, "to": 45},
        })));
    }

    #[test]
    fn appearance_rejects_structural_array_changes() {
        // Added/removed elements are structural, never an appearance.
        assert!(!is_pure_appearance(&json!({
            "_array_diff": {"strategy": "keyed", "added": [{"x": 1}]}
        })));
        assert!(!is_pure_appearance(&json!({
            "_array_diff": {"strategy": "keyed", "removed": [{"x": 1}]}
        })));
        // A changed element counts only if its own changes are appearances.
        assert!(is_pure_appearance(&json!({
            "_array_diff": {"strategy": "keyed", "changed": [
                {"key": {"name": "a"}, "changes": {"Unknown": {"from": null, "to": 1}}}
            ]}
        })));
        assert!(!is_pure_appearance(&json!({
            "_array_diff": {"strategy": "keyed", "changed": [
                {"key": {"name": "a"}, "changes": {"Magnitude": {"from": 10, "to": 20}}}
            ]}
        })));
    }

    #[test]
    fn appearance_rejects_unkeyed_structural_array_changes() {
        // Same rule as `appearance_rejects_structural_array_changes`, for the
        // `unkeyed` strategy (`unkeyed_array_diff` — CTDA `Conditions[]` is
        // the canonical producer). `added`/`removed` here are the *whole*
        // element lists, not per-element diffs, but the same "never an
        // appearance" rule applies: a real element count change must never
        // be suppressed as if it were a form-version-driven default flip.
        assert!(!is_pure_appearance(&json!({
            "_array_diff": {"strategy": "unkeyed", "count_from": 2, "count_to": 1,
                "removed": [{"Condition": {"Function": "A"}}, {"Condition": {"Function": "B"}}],
                "added": [{"Condition": {"Function": "C"}}]}
        })));
        assert!(!is_pure_disappearance(&json!({
            "_array_diff": {"strategy": "unkeyed", "count_from": 1, "count_to": 0,
                "removed": [{"Condition": {"Function": "A"}}]}
        })));
    }

    #[test]
    fn restamp_leaves_unkeyed_array_diff_untouched() {
        // `strip_restamp_array_diff` only walks `changed[]`; an `unkeyed`
        // envelope never has one (see `unkeyed_array_diff`), so the whole
        // element lists must survive a restamp pass completely intact —
        // confirms the A2 review finding by construction, not just reading.
        let mut fc = json!({
            "Conditions": {
                "_array_diff": {
                    "strategy": "unkeyed",
                    "count_from": 2,
                    "count_to": 1,
                    "removed": [
                        {"Condition": {"Function": "GetGlobalValue", "Comparison Value": 10.0}},
                        {"Condition": {"Function": "GetGlobalValue", "Comparison Value": 7.0}},
                    ],
                    "added": [
                        {"Condition": {"Function": "GetGlobalValue", "Comparison Value": 0.0}},
                    ],
                }
            }
        });
        let before = fc.clone();
        strip_restamp_appearances(&mut fc, "INFO");
        assert_eq!(fc, before);
    }

    #[test]
    fn padding_zeroed_leaves_unkeyed_array_diff_untouched() {
        // Same shape check for the padding-zero pass: `changed[]` is where
        // `_raw` hex leaves live, and an `unkeyed` envelope never has one.
        let mut fc = json!({
            "Conditions": {
                "_array_diff": {
                    "strategy": "unkeyed",
                    "count_from": 1,
                    "count_to": 2,
                    "removed": [{"Condition": {"Function": "A"}}],
                    "added": [{"Condition": {"Function": "B"}}, {"Condition": {"Function": "C"}}],
                }
            }
        });
        let before = fc.clone();
        let stripped = strip_padding_zeroed(&mut fc);
        assert_eq!(stripped, 0);
        assert_eq!(fc, before);
    }

    #[test]
    fn calibrated_leaves_unkeyed_array_diff_untouched() {
        // Same shape check for the calibrated-appearance-default pass:
        // `strip_calibrated_array_diff` only walks `changed[].changes`,
        // never `added`/`removed`.
        let mut ad = json!({
            "strategy": "unkeyed",
            "count_from": 1,
            "count_to": 2,
            "removed": [{"Condition": {"Function": "A"}}],
            "added": [{"Condition": {"Function": "B"}}, {"Condition": {"Function": "C"}}],
        })
        .as_object()
        .unwrap()
        .clone();
        let before = ad.clone();
        let suppressible: HashSet<DefaultKey> = HashSet::new();
        let dropped = strip_calibrated_array_diff(&mut ad, "Conditions", &suppressible);
        assert!(
            !dropped,
            "added/removed present — envelope is not noise-free"
        );
        assert_eq!(ad, before);
    }

    #[test]
    fn strip_keeps_authored_edits_and_drops_appearances() {
        let schema = test_schema(json!([
            {
                "kind": "integer",
                "name": "Enlighten Auto UV",
                "width": "u8",
                "from_version": 200
            },
            {
                "kind": "formid",
                "name": "Previous INFO",
                "from_version": 200
            },
            {"kind": "integer", "name": "Attack Damage", "width": "u16"}
        ]));
        let mut fc = json!({
            "Enlighten Auto UV": {"from": null, "to": {"Unknown": 1}},
            "Previous INFO":     {"from": null, "to": "0x0031E5CD"},
            "Attack Damage":     {"from": 30, "to": 45},
        });
        strip_version_gated_transitions(&mut fc, &schema, "TEST", 199, 200);
        assert_eq!(fc, json!({"Attack Damage": {"from": 30, "to": 45}}));
    }

    #[test]
    fn strip_is_a_no_op_on_non_objects() {
        let schema = test_schema(json!([]));
        let mut v = json!("not an object");
        strip_version_gated_transitions(&mut v, &schema, "TEST", 199, 200);
        assert_eq!(v, json!("not an object"));
    }

    #[test]
    fn strip_keeps_ungated_appearance() {
        let schema = test_schema(json!([
            {"kind": "float", "name": "Sneak Attack Multiplier"}
        ]));
        let mut fc = json!({
            "Sneak Attack Multiplier": {"from": null, "to": 2.0}
        });

        strip_version_gated_transitions(&mut fc, &schema, "TEST", 201, 209);

        assert_eq!(
            fc,
            json!({"Sneak Attack Multiplier": {"from": null, "to": 2.0}})
        );
    }

    #[test]
    fn strip_drops_appearance_when_from_version_is_crossed() {
        let schema = test_schema(json!([{
            "kind": "struct",
            "name": "Data",
            "fields": [{
                "kind": "bytes",
                "name": "New Bytes",
                "from_version": 205
            }]
        }]));
        let mut fc = json!({"Data": {"New Bytes": {"from": null, "to": {"hex": "00"}}}});

        strip_version_gated_transitions(&mut fc, &schema, "TEST", 201, 209);

        assert_eq!(fc, json!({}));
    }

    #[test]
    fn strip_keeps_appearance_when_gate_is_active_at_both_versions() {
        let schema = test_schema(json!([{
            "kind": "integer",
            "name": "Already Active",
            "width": "u8",
            "from_version": 200
        }]));
        let mut fc = json!({"Already Active": {"from": null, "to": 1}});

        strip_version_gated_transitions(&mut fc, &schema, "TEST", 201, 209);

        assert_eq!(fc, json!({"Already Active": {"from": null, "to": 1}}));
    }

    #[test]
    fn strip_drops_disappearance_when_below_version_is_crossed() {
        let schema = test_schema(json!([{
            "kind": "formid",
            "name": "Value Currency",
            "below_version": 205
        }]));
        let mut fc = json!({"Value Currency": {"from": "0x0000000F", "to": null}});

        strip_version_gated_transitions(&mut fc, &schema, "TEST", 201, 209);

        assert_eq!(fc, json!({}));
    }

    #[test]
    fn strip_keeps_ungated_disappearance() {
        let schema = test_schema(json!([{
            "kind": "formid",
            "name": "Value Currency"
        }]));
        let mut fc = json!({"Value Currency": {"from": "0x0000000F", "to": null}});

        strip_version_gated_transitions(&mut fc, &schema, "TEST", 201, 209);

        assert_eq!(
            fc,
            json!({"Value Currency": {"from": "0x0000000F", "to": null}})
        );
    }

    // `strip_restamp_appearances` and its helpers are private, so these live
    // here too (issue #18). Same over-suppression risk as above, plus a new
    // one specific to this pass: rule (b)/(c) are content-based heuristics,
    // not schema-verified, so the tests lean extra hard on the boundary
    // cases (non-zero hex, wrong signature/member, wrong direction).

    #[test]
    fn restamp_suppresses_null_to_zero_raw_leaf() {
        let mut fc = json!({
            "Unknown": {"from": null, "to": {"hex": "0000000000000000", "_raw": true}}
        });
        strip_restamp_appearances(&mut fc, "STAT");
        assert_eq!(fc, json!({}));
    }

    #[test]
    fn restamp_suppresses_zero_raw_to_null_disappearance() {
        // Symmetric with the appearance case above (design review Q2): the
        // serializer shrinking away all-zero padding carries no information
        // either, same as it growing.
        let mut fc = json!({
            "Unknown": {"from": {"hex": "0000000000000000", "_raw": true}, "to": null}
        });
        strip_restamp_appearances(&mut fc, "STAT");
        assert_eq!(fc, json!({}));
    }

    #[test]
    fn restamp_keeps_null_to_nonzero_raw_leaf() {
        // A single non-zero byte anywhere means this could be real data —
        // must survive.
        let fc = json!({
            "Unknown": {"from": null, "to": {"hex": "0000000100000000", "_raw": true}}
        });
        let mut got = fc.clone();
        strip_restamp_appearances(&mut got, "STAT");
        assert_eq!(got, fc);
    }

    #[test]
    fn restamp_strips_nested_array_element_leaf_and_drops_whole_key() {
        // Shape of the real INFO `Responses[].Response.Unknown` case: one
        // `_array_diff.changed[]` element, entirely noise, no added/removed.
        let mut fc = json!({
            "Responses": {
                "_array_diff": {
                    "strategy": "keyed",
                    "count_from": 1,
                    "count_to": 1,
                    "changed": [{
                        "key": {"index": 0},
                        "index_from": 0,
                        "index_to": 0,
                        "changes": {
                            "Response": {
                                "Unknown": {"from": null, "to": {"hex": "00", "_raw": true}}
                            }
                        }
                    }]
                }
            }
        });
        strip_restamp_appearances(&mut fc, "INFO");
        assert_eq!(fc, json!({}));
    }

    #[test]
    fn restamp_keeps_mixed_array_element_real_edit() {
        // The critical regression test for per-leaf (not whole-element)
        // stripping: the same element also carries a genuine authored edit
        // alongside the noise leaf. Only the noise leaf must go; the element
        // and the array key must survive with the real edit intact.
        let mut fc = json!({
            "Responses": {
                "_array_diff": {
                    "strategy": "keyed",
                    "count_from": 1,
                    "count_to": 1,
                    "changed": [{
                        "key": {"index": 0},
                        "index_from": 0,
                        "index_to": 0,
                        "changes": {
                            "Response": {
                                "Unknown": {"from": null, "to": {"hex": "00", "_raw": true}}
                            },
                            "Response Text": {"from": "Old line", "to": "New line"}
                        }
                    }]
                }
            }
        });
        strip_restamp_appearances(&mut fc, "INFO");
        assert_eq!(
            fc,
            json!({
                "Responses": {
                    "_array_diff": {
                        "strategy": "keyed",
                        "count_from": 1,
                        "count_to": 1,
                        "changed": [{
                            "key": {"index": 0},
                            "index_from": 0,
                            "index_to": 0,
                            "changes": {
                                "Response Text": {"from": "Old line", "to": "New line"}
                            }
                        }]
                    }
                }
            })
        );
    }

    #[test]
    fn restamp_drops_only_fully_noisy_elements_keeps_others() {
        // Two changed[] elements: one pure noise (dropped), one with a real
        // edit in its own element (kept) — the array key survives because
        // `changed` isn't fully emptied.
        let mut fc = json!({
            "Responses": {
                "_array_diff": {
                    "strategy": "keyed",
                    "count_from": 2,
                    "count_to": 2,
                    "changed": [
                        {
                            "key": {"index": 0},
                            "index_from": 0,
                            "index_to": 0,
                            "changes": {
                                "Response": {
                                    "Unknown": {"from": null, "to": {"hex": "00", "_raw": true}}
                                }
                            }
                        },
                        {
                            "key": {"index": 1},
                            "index_from": 1,
                            "index_to": 1,
                            "changes": {
                                "Response Text": {"from": "Old", "to": "New"}
                            }
                        }
                    ]
                }
            }
        });
        strip_restamp_appearances(&mut fc, "INFO");
        assert_eq!(
            fc,
            json!({
                "Responses": {
                    "_array_diff": {
                        "strategy": "keyed",
                        "count_from": 2,
                        "count_to": 2,
                        "changed": [{
                            "key": {"index": 1},
                            "index_from": 1,
                            "index_to": 1,
                            "changes": {
                                "Response Text": {"from": "Old", "to": "New"}
                            }
                        }]
                    }
                }
            })
        );
    }

    #[test]
    fn restamp_suppresses_materialized_on_resave_pnam() {
        let mut fc = json!({
            "Previous INFO": {"from": null, "to": "0x0031E5CD"}
        });
        strip_restamp_appearances(&mut fc, "INFO");
        assert_eq!(fc, json!({}));
    }

    #[test]
    fn restamp_keeps_materialized_member_disappearance() {
        // Directional guard (design review Q2): unlike rule (b), rule (c)
        // must NOT be symmetric — an authored unlink of a PNAM chain must
        // stay visible.
        let fc = json!({
            "Previous INFO": {"from": "0x0031E5CD", "to": null}
        });
        let mut got = fc.clone();
        strip_restamp_appearances(&mut got, "INFO");
        assert_eq!(got, fc);
    }

    #[test]
    fn restamp_keeps_materialized_field_on_wrong_signature_or_member() {
        // Same leaf shape/value, but neither the signature nor the member
        // name is the tabled (INFO, "Previous INFO") pair — precision guard
        // on MATERIALIZED_ON_RESAVE, not just member-name matching.
        let wrong_sig = json!({"Previous INFO": {"from": null, "to": "0x0031E5CD"}});
        let mut got_wrong_sig = wrong_sig.clone();
        strip_restamp_appearances(&mut got_wrong_sig, "STAT");
        assert_eq!(got_wrong_sig, wrong_sig);

        let wrong_member = json!({"Next INFO": {"from": null, "to": "0x0031E5CD"}});
        let mut got_wrong_member = wrong_member.clone();
        strip_restamp_appearances(&mut got_wrong_member, "INFO");
        assert_eq!(got_wrong_member, wrong_member);
    }

    #[test]
    fn restamp_keeps_null_to_formid_when_not_in_table() {
        // Guards against rule (c) accidentally becoming a generic "any
        // formid appearance is noise" rule — this member isn't all-zero hex
        // and isn't in MATERIALIZED_ON_RESAVE, so it must survive untouched.
        let fc = json!({"Base Object": {"from": null, "to": "0x0001234A"}});
        let mut got = fc.clone();
        strip_restamp_appearances(&mut got, "REFR");
        assert_eq!(got, fc);
    }

    #[test]
    fn materialized_on_resave_members_are_still_ungated_in_live_schema() {
        // Canary (design review Q6 #9): MATERIALIZED_ON_RESAVE exists only
        // because the live schema does NOT version-gate these members — if
        // the extractor ever adds proper from_version/below_version gating
        // for one of them, `strip_version_gated_transitions` would already
        // catch it and this table entry becomes redundant (not wrong, but
        // worth a human noticing). `schema/fo76.json` is a generated,
        // drifting artifact, so this is worth pinning down.
        let schema = Schema::load_embedded().expect("embedded schema must load");
        for (sig, wanted) in MATERIALIZED_ON_RESAVE.iter().copied() {
            let record = schema
                .record(sig)
                .unwrap_or_else(|| panic!("schema has no record {sig}"));
            let member = record
                .members
                .iter()
                .find(|m| member_name(m) == Some(wanted))
                .unwrap_or_else(|| panic!("{sig} has no member named {wanted:?}"));
            let (from_version, below_version) = member_version_bounds(member);
            assert!(
                from_version.is_none() && below_version.is_none(),
                "{sig}.{wanted} is now schema-gated ({from_version:?}..{below_version:?}) — \
                 the MATERIALIZED_ON_RESAVE entry may be redundant now that \
                 strip_version_gated_transitions would already catch this"
            );
        }
    }

    // -------------------------------------------------------------------
    // Issue #22: padding-zeroing + calibrated appearance-default suppression
    // -------------------------------------------------------------------

    fn stub_diff(form_id: &str, record_type: &str, field_changes: Value) -> RecordDiff {
        RecordDiff {
            stub: RecordStub {
                form_id: form_id.to_owned(),
                record_type: record_type.to_owned(),
                ..Default::default()
            },
            field_changes,
            prev_editor_id: None,
        }
    }

    #[test]
    fn leaf_name_strips_array_index_suffix() {
        assert_eq!(
            leaf_name("Effects[].Effect.Cooldown Duration"),
            "Cooldown Duration"
        );
        assert_eq!(leaf_name("Model.Enlighten Auto UV"), "Enlighten Auto UV");
        assert_eq!(
            leaf_name("Female.World Model.Enlighten Auto UV"),
            "Enlighten Auto UV"
        );
        assert_eq!(leaf_name("Unknown[0]"), "Unknown");
    }

    #[test]
    fn padding_zeroed_detects_garbage_to_zeros_hex_leaf() {
        assert!(is_padding_zeroed_value(&json!({
            "hex": {"from": "3809c7", "to": "000000"}
        })));
        // Non-zero destination must survive.
        assert!(!is_padding_zeroed_value(&json!({
            "hex": {"from": "3809c7", "to": "000001"}
        })));
        // Appearance (null → zero raw) is #18's job, not this shape.
        assert!(!is_padding_zeroed_value(&json!({
            "from": null,
            "to": {"hex": "000000", "_raw": true}
        })));
    }

    #[test]
    fn padding_zeroed_strips_leaf_and_counts() {
        let mut fc = json!({
            "Unknown": {"hex": {"from": "3809c7", "to": "000000"}},
            "Attack Damage": {"from": 30, "to": 45},
        });
        assert_eq!(strip_padding_zeroed(&mut fc), 1);
        assert_eq!(fc, json!({"Attack Damage": {"from": 30, "to": 45}}));
    }

    #[test]
    fn calibrated_collapses_leaf_name_across_paths() {
        // Same (leaf_name, value) at three different full paths — counting by
        // leaf name (not full path) is what lets N=3 fire here. A path-keyed
        // rule would see each path only once and miss the threshold.
        let default = json!({"Unknown": 1, "Max Distance": 50.0});
        let mut changed = vec![
            stub_diff(
                "0x00000001",
                "STAT",
                json!({"Model": {"Enlighten Auto UV": {"from": null, "to": default.clone()}}}),
            ),
            stub_diff(
                "0x00000002",
                "NPC_",
                json!({"Female": {"World Model": {
                    "Enlighten Auto UV": {"from": null, "to": default.clone()}
                }}}),
            ),
            stub_diff(
                "0x00000003",
                "NPC_",
                json!({"Male": {"World Model": {
                    "Enlighten Auto UV": {"from": null, "to": default}
                }}}),
            ),
        ];
        let restamp = vec![true, true, true];
        let mut counts = BTreeMap::new();
        let auto = apply_restamp_calibrated_suppression(&mut changed, &restamp, &mut counts, 3);
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].leaf_name, "Enlighten Auto UV");
        assert_eq!(auto[0].count, 3);
        assert!(
            changed.is_empty(),
            "all three records were appearance-only noise: {changed:?}"
        );
        assert_eq!(counts.get("STAT").copied(), Some(1));
        assert_eq!(counts.get("NPC_").copied(), Some(2));
    }

    #[test]
    fn calibrated_preserves_appearance_that_collides_with_real_edit() {
        let shared = json!(2.0);
        let mut changed = vec![
            // Appearances of Sneak Attack Multiplier → 2.0 (would hit N=2 alone)
            stub_diff(
                "0x00000001",
                "WEAP",
                json!({"Sneak Attack Multiplier": {"from": null, "to": shared.clone()}}),
            ),
            stub_diff(
                "0x00000002",
                "WEAP",
                json!({"Sneak Attack Multiplier": {"from": null, "to": shared.clone()}}),
            ),
            // Genuine authored edit of the same leaf_name to the same value
            // elsewhere — must poison the auto-classify rule.
            stub_diff(
                "0x00000003",
                "WEAP",
                json!({"Sneak Attack Multiplier": {"from": 1.5, "to": shared}}),
            ),
        ];
        let restamp = vec![true, true, true];
        let mut counts = BTreeMap::new();
        let auto = apply_restamp_calibrated_suppression(&mut changed, &restamp, &mut counts, 2);
        assert!(
            auto.is_empty(),
            "rule must not fire when the value is also a real edit: {auto:?}"
        );
        assert_eq!(changed.len(), 3, "nothing stripped: {changed:?}");
    }

    #[test]
    fn calibrated_disappearance_does_not_poison_appearance_default() {
        // A `V → null` disappearance records the old value on `from`, but that
        // must NOT block suppressing `null → V` appearances of the same
        // serializer default (issue #22: only authored `to` values poison).
        let default = json!("0x0000000F");
        let mut changed = vec![
            stub_diff(
                "0x00000001",
                "WEAP",
                json!({"Value Currency": {"from": null, "to": default.clone()}}),
            ),
            stub_diff(
                "0x00000002",
                "WEAP",
                json!({"Value Currency": {"from": null, "to": default.clone()}}),
            ),
            stub_diff(
                "0x00000003",
                "WEAP",
                json!({"Value Currency": {"from": default, "to": null}}),
            ),
        ];
        let restamp = vec![true, true, true];
        let mut counts = BTreeMap::new();
        let auto = apply_restamp_calibrated_suppression(&mut changed, &restamp, &mut counts, 2);
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].leaf_name, "Value Currency");
        // Two appearance-only records dropped; the disappearance remains.
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].stub.form_id, "0x00000003");
    }

    #[test]
    fn calibrated_drops_noise_only_record_keeps_mixed() {
        let default = json!(0);
        let mut changed = vec![
            stub_diff(
                "0x00000001",
                "OMOD",
                json!({"Required Count": {"from": null, "to": default.clone()}}),
            ),
            stub_diff(
                "0x00000002",
                "OMOD",
                json!({
                    "Required Count": {"from": null, "to": default.clone()},
                    "Attack Damage": {"from": 10, "to": 20},
                }),
            ),
            stub_diff(
                "0x00000003",
                "OMOD",
                json!({"Required Count": {"from": null, "to": default}}),
            ),
        ];
        let restamp = vec![true, true, true];
        let mut counts = BTreeMap::new();
        let auto = apply_restamp_calibrated_suppression(&mut changed, &restamp, &mut counts, 3);
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].leaf_name, "Required Count");
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].stub.form_id, "0x00000002");
        assert_eq!(
            changed[0].field_changes,
            json!({"Attack Damage": {"from": 10, "to": 20}})
        );
        assert_eq!(counts.get("OMOD").copied(), Some(2));
    }

    #[test]
    fn calibrated_skips_non_restamp_records() {
        // form_versions matched → restamp=false → pass must not touch the record,
        // even when the same appearance would otherwise clear the threshold.
        let default = json!(false);
        let mut changed = vec![
            stub_diff(
                "0x00000001",
                "ACTI",
                json!({"Activator Can Be Instanced": {"from": null, "to": default.clone()}}),
            ),
            stub_diff(
                "0x00000002",
                "ACTI",
                json!({"Activator Can Be Instanced": {"from": null, "to": default}}),
            ),
        ];
        // Count still sees both (global collect), but apply only hits restamp=true.
        let restamp = vec![false, false];
        let mut counts = BTreeMap::new();
        let auto = apply_restamp_calibrated_suppression(&mut changed, &restamp, &mut counts, 2);
        assert_eq!(auto.len(), 1, "rule is classified from global count");
        assert_eq!(
            changed.len(),
            2,
            "non-restamp records must survive: {changed:?}"
        );
        assert!(counts.is_empty());
    }
}
