//! Per-element array diffing — the four pairing strategies [`array_diff`]
//! (called from [`super::json_diff`]'s array arm) chooses between, and the
//! `_array_diff` envelope shape ADR 0005 freezes: `strategy` is one of
//! `keyed`/`positional`/`set`/`unkeyed`, alongside `key_fields`/`count_from`/
//! `count_to`/`added`/`removed`/`changed`/`unchanged_count`.
//!
//! Decoded rarray elements are almost always either uniform primitives (a
//! FormID list) or single-member "rstruct" wrappers (`{"Leveled List Entry":
//! {..}}`). Diffing them wholesale hides which entries actually changed
//! inside a 50-element leveled list. The strategy below tries, in order: a
//! schema-aware key ([`element_key_spec`]), positional pairing (equal
//! length, no key), or a primitive multiset diff — falling back to the
//! opaque leaf only when none of those apply.

use super::{is_formid_str, json_diff};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

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
pub(crate) fn is_empty_diff(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.is_empty())
}

/// The `unkeyed` array-diff strategy — the two element lists' real
/// difference under `removed`/`added` (see [`lcs_align`]), wrapped in the
/// ordinary `_array_diff` envelope. Used when array elements can't be paired
/// meaningfully: heterogeneous element shapes, an unkeyable object shape, or
/// a proposed key that [`widen_key_spec_until_unique`] couldn't make unique.
///
/// This is a deliberate classification, not a fallback of last resort — CTDA
/// `Conditions[]` is the canonical case. A condition's position is semantic
/// (`AND`/`OR` chaining), so a synthetic key would pair unrelated rows and
/// report false mutations; "unkeyed" reports the lists' difference instead,
/// which is what every downstream reader (the patch-notes renderer, the
/// tier-assessor summary) needs to describe what actually changed. See
/// `docs/adr/0005-element-identity-owned-by-rust.md`.
///
/// `removed`/`added` hold only the LCS-trimmed difference, not the full two
/// lists — reporting every element on both sides would bury a small real
/// change (e.g. a handful of removed entries deep in a `Conditions[]`
/// cascade) under a wall of byte-identical rows. `unchanged_count` reports
/// how many elements the trim dropped as identical, so a reader isn't left
/// assuming the whole array turned over.
fn unkeyed_array_diff(a: &[Value], b: &[Value]) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("strategy".to_string(), Value::String("unkeyed".to_string()));
    out.insert("count_from".to_string(), serde_json::json!(a.len()));
    out.insert("count_to".to_string(), serde_json::json!(b.len()));

    let (removed, added, unchanged_count) = lcs_align(a, b);
    out.insert(
        "unchanged_count".to_string(),
        serde_json::json!(unchanged_count),
    );
    if !removed.is_empty() {
        out.insert("removed".to_string(), Value::Array(removed));
    }
    if !added.is_empty() {
        out.insert("added".to_string(), Value::Array(added));
    }
    wrap_array_diff(out)
}

/// Order-preserving alignment of `a` against `b` (longest common
/// subsequence): returns `(removed, added, unchanged_count)`, where
/// `removed`/`added` are exactly the elements *not* part of the alignment
/// and `unchanged_count` is the alignment's length.
///
/// Order-preserving rather than a multiset intersection — a
/// multiset diff would report a pure reorder (e.g. two conditions in a
/// `GetRandomPercent` cascade swapping position, where order changes
/// behavior) as no change at all, which is worse than today's whole-list
/// dump for exactly the arrays `unkeyed` exists to describe honestly. LCS
/// keeps one copy of a moved element aligned and reports the move as a
/// removed + added pair instead — a plain insertion or removal still trims
/// to just the inserted/removed elements, but a reorder stays visible.
///
/// A common prefix/suffix is trimmed first (any LCS can be extended to
/// include a matching prefix/suffix without losing optimality, so this
/// loses nothing) purely to shrink the O(n·m) DP below. If the remaining
/// middle is still large enough that the DP table would be expensive
/// (250,000 cells, i.e. two ~500-element remainders), the untrimmed middle
/// is reported rather than spending unbounded time on one record's array —
/// the same behavior as before this function existed, just for a smaller
/// slice.
fn lcs_align(a: &[Value], b: &[Value]) -> (Vec<Value>, Vec<Value>, usize) {
    let (n, m) = (a.len(), b.len());

    let mut start = 0;
    while start < n && start < m && a[start] == b[start] {
        start += 1;
    }
    let mut end_a = n;
    let mut end_b = m;
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let prefix_suffix_unchanged = start + (n - end_a);
    let (mid_a, mid_b) = (&a[start..end_a], &b[start..end_b]);

    if mid_a.len().saturating_mul(mid_b.len()) > 250_000 {
        return (mid_a.to_vec(), mid_b.to_vec(), prefix_suffix_unchanged);
    }

    let (removed, added, mid_unchanged) = lcs_dp(mid_a, mid_b);
    (removed, added, prefix_suffix_unchanged + mid_unchanged)
}

/// Classic LCS dynamic program plus backtrace, returning the elements of `a`
/// with no match in `b` (`removed`), the elements of `b` with no match in
/// `a` (`added`), and the LCS length (`unchanged`). Element equality is
/// exact JSON `Value` equality — the same notion `array_diff`'s `a == b`
/// short-circuit already uses.
fn lcs_dp(a: &[Value], b: &[Value]) -> (Vec<Value>, Vec<Value>, usize) {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut removed = Vec::new();
    let mut added = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            removed.push(a[i].clone());
            i += 1;
        } else {
            added.push(b[j].clone());
            j += 1;
        }
    }
    removed.extend(a[i..].iter().cloned());
    added.extend(b[j..].iter().cloned());
    (removed, added, dp[0][0])
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
    if m.len() == 1
        && let Some((k, Value::Object(inner))) = m.iter().next()
    {
        return (Some(k.as_str()), inner);
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
/// (equal lengths) or an unkeyed diff.
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
    // 7. RACE/NPC_ `Attacks[]` entries that carry no `Required Slot` sibling
    //    (some attacks are unconditional) decode as the single-key wrapper
    //    `{"Attack": {"Attack Data": ..., "Attack Event": ..., ...}}`, which
    //    `unwrap_wrapper` strips down to the inner struct — none of whose
    //    members are FormID-shaped, so the FormID-composite fallback below
    //    can't reach it either. `Attack Event` (e.g. `meleeStart_1`,
    //    `MeleeStart_Left`) is the animation-event name driving the attack
    //    and is unique per attack on every real record measured. When
    //    `Required Slot` *is* present, the element has two top-level keys
    //    and never reaches this arm (no unwrap fires) — heuristic 12's
    //    FormID composite handles that shape instead, widening onto
    //    `Attack.Attack Event` if `Required Slot` alone isn't unique.
    if wrapper == Some("Attack") {
        return Some(vec![vec!["Attack Event".to_string()]]);
    }
    // 8. Single-reference entries.
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
    // 9. Quest aliases: `unwrap_wrapper` already strips the alias-kind wrapper
    //    (`Location Alias` / `Reference Alias` / `Ref Collection Alias` /
    //    ...), leaving a body that always carries `Alias ID` regardless of
    //    kind.
    if body.contains_key("Alias ID") {
        return Some(vec![vec!["Alias ID".to_string()]]);
    }
    // 10. Generic "Index" / "* Index" member.
    let mut index_members: Vec<&String> = body
        .keys()
        .filter(|k| k.as_str() == "Index" || k.ends_with(" Index"))
        .collect();
    index_members.sort();
    if let Some(name) = index_members.into_iter().next() {
        return Some(vec![vec![name.clone()]]);
    }
    // 11. VMAD script properties: a Papyrus property is identified by its name.
    //    This must precede the FormID-shaped fallback below — a property's
    //    `value` is often FormID-shaped and is NOT an identity: several
    //    properties on one script routinely share a value (e.g. three quest
    //    aliases all pointing at the owning quest), so keying on it cannot
    //    tell them apart and a reordered property list reads as every
    //    property mutating.
    if body.contains_key("name") && body.contains_key("type") && body.contains_key("value") {
        return Some(vec![vec!["name".to_string()]]);
    }
    // 12. Every FormID-shaped member, composed. LCTN's various reference-list
    //    shapes are the motivating case, and a lesson in not hand-curating
    //    one heuristic per exact field combination: `Master Special
    //    References` carries `(Ref, Loc Ref Type, World/Cell)`, `Master
    //    Persist Location References` only `(Ref, World/Cell)`, `Master
    //    Enable Parent References` `(Ref, Enable Parent)`, `Master Unique
    //    NPCs` `(Actor Ref, NPC)` — four different combinations of the same
    //    underlying pattern ("one or more references identify this row").
    //    Composing every FormID-shaped member (sorted by name, deterministic)
    //    covers all of them with one rule instead of four; when the
    //    composite still isn't unique on some array,
    //    `widen_key_spec_until_unique` extends it with non-FormID scalar
    //    fields (e.g. `Grid X`/`Grid Y`) the same way it would for any other
    //    proposed key.
    let formid_members: Vec<&String> = {
        let mut v: Vec<&String> = body
            .iter()
            .filter(|(_, v)| is_formid_shaped(v))
            .map(|(k, _)| k)
            .collect();
        v.sort();
        v
    };
    if !formid_members.is_empty() {
        return Some(
            formid_members
                .into_iter()
                .map(|k| vec![k.clone()])
                .collect(),
        );
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

/// True when no two elements of `elems` resolve to the same `spec` group.
/// `element_key_spec`'s heuristics *propose* an identity from one sample
/// element's shape; they don't inspect the actual arrays, so a proposed key
/// can turn out non-unique — most commonly the FormID-composite fallback
/// (heuristic 11) proposing a single shared member where many elements
/// legitimately collide (e.g. several attacks keyed only by a shared
/// `Required Slot`).
fn key_groups_unique(elems: &[Value], spec: &KeySpec) -> bool {
    let mut seen: HashSet<String> = HashSet::with_capacity(elems.len());
    elems
        .iter()
        .all(|e| seen.insert(compute_key_info(e, spec).group))
}

/// Collect scalar (non-object, non-array) leaf field paths under `body`,
/// dot-separated for nested members (matching the path syntax `get_path`
/// already reads, e.g. `"Attack.Attack Event"`), bounded to `max_depth`
/// nested objects so a widen search can't walk into an unrelated deeply
/// nested subtree. A resolved FormID stub (`is_formid_shaped`) is treated as
/// a leaf rather than recursed into — its `formid`/`editor_id`/`record_type`
/// members aren't independent facts about the element, just one reference's
/// alternate representations. Nested arrays are skipped entirely: an array
/// member has no stable per-index identity of its own to key on. Returned
/// sorted, for a deterministic widen order across runs.
fn scalar_leaf_paths(body: &serde_json::Map<String, Value>, max_depth: usize) -> Vec<String> {
    fn walk(
        prefix: &str,
        obj: &serde_json::Map<String, Value>,
        depth: usize,
        max_depth: usize,
        out: &mut Vec<String>,
    ) {
        for (k, v) in obj {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                Value::Object(inner) if depth < max_depth && !is_formid_shaped(v) => {
                    walk(&path, inner, depth + 1, max_depth, out);
                }
                Value::Array(_) => {}
                _ => out.push(path),
            }
        }
    }
    let mut out = Vec::new();
    walk("", body, 0, max_depth, &mut out);
    out.sort();
    out
}

/// If `spec` doesn't uniquely identify every element on both sides, widen it
/// by appending further scalar leaf paths (sorted, deterministic) — one at a
/// time, cumulatively — until the widened spec is unique on both sides, or
/// every candidate is exhausted. A stricter key can only ever reclassify a
/// false `changed` pairing into an honest `added` + `removed`; it can't
/// produce a *wrong* pairing that `array_diff`'s original proposal wouldn't
/// already have risked. Returns `None` when no widening achieves uniqueness
/// — the caller then falls back to [`unkeyed_array_diff`] rather than
/// silently keeping a non-unique key, which `keyed_diff` would otherwise
/// pair FIFO-by-list-order within each duplicate group (i.e. positionally,
/// with no indication that happened).
fn widen_key_spec_until_unique(
    a: &[Value],
    b: &[Value],
    spec: &KeySpec,
    sample: &serde_json::Map<String, Value>,
) -> Option<KeySpec> {
    if key_groups_unique(a, spec) && key_groups_unique(b, spec) {
        return Some(spec.clone());
    }
    let (_, body) = unwrap_wrapper(sample);
    let used: HashSet<&str> = spec
        .iter()
        .flat_map(|alts| alts.iter().map(String::as_str))
        .collect();

    let mut widened = spec.clone();
    for path in scalar_leaf_paths(body, 2) {
        if used.contains(path.as_str()) {
            continue;
        }
        widened.push(vec![path]);
        if key_groups_unique(a, &widened) && key_groups_unique(b, &widened) {
            return Some(widened);
        }
    }
    None
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
/// element shape into one of four pairing strategies:
///
/// - **keyed** — elements are rstructs with a recognizable identity field
///   ([`element_key_spec`]) that [`widen_key_spec_until_unique`] confirms
///   (or makes) unique on both sides; paired by canonical key value
///   regardless of order or position.
/// - **positional** — same length, no usable key; paired index-for-index.
/// - **set** — uniform JSON primitives (e.g. a FormID keyword list); paired
///   as a multiset (order-insensitive, duplicate-aware).
/// - **unkeyed** — elements have no stable per-element identity at all
///   (heterogeneous shapes, an unkeyable object shape with mismatched
///   lengths, or a proposed key that no widening can make unique — CTDA
///   `Conditions[]` is the canonical case of the former, see
///   [`unkeyed_array_diff`]); the two element lists are reported as
///   `removed`/`added` (trimmed to their real difference, not paired).
///
/// Returns an empty object when the chosen strategy finds no differences —
/// e.g. a reorder-only keyed array — matching `json_diff`'s convention of
/// omitting unchanged fields entirely.
pub(crate) fn array_diff(a: &[Value], b: &[Value]) -> Value {
    if a == b {
        return Value::Object(serde_json::Map::new());
    }

    if a.iter().chain(b.iter()).all(is_primitive_value) {
        return set_diff(a, b);
    }

    // Arrays of arrays (Papyrus struct-array properties are the common case:
    // a VMAD property whose value is a list of structs, each a list of
    // name/type/value members). Pair them positionally so each inner array
    // gets its own classification — without this the whole nest collapses to
    // one opaque from/to blob, and a reorder *inside* any struct reprints
    // every struct on both sides.
    if a.len() == b.len() && a.iter().chain(b.iter()).all(Value::is_array) {
        return positional_diff(a, b);
    }

    if !a.iter().chain(b.iter()).all(Value::is_object) {
        // Heterogeneous element shapes (mixed primitive/object, nested
        // arrays of differing length, …) aren't classifiable — report the
        // two whole lists unkeyed.
        return unkeyed_array_diff(a, b);
    }

    let Some(sample) = a.iter().chain(b.iter()).find_map(Value::as_object) else {
        // Unreachable in practice (all-object + not `a == b` implies at
        // least one element exists), kept as a defensive fallback.
        return unkeyed_array_diff(a, b);
    };

    match element_key_spec(sample) {
        Some(spec) => match widen_key_spec_until_unique(a, b, &spec, sample) {
            Some(spec) => keyed_diff(a, b, &spec),
            None => unkeyed_array_diff(a, b),
        },
        None if a.len() == b.len() => positional_diff(a, b),
        None => unkeyed_array_diff(a, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lcs_align_common_prefix_suffix_trim_shrinks_the_dp_below_its_safety_cap() {
        // `lcs_align`'s DP is skipped above 250,000 (mid_a.len * mid_b.len)
        // cells to bound worst-case cost on one record's array — but the
        // common prefix/suffix trim runs FIRST and is unbounded, so a large
        // array that differs only by one inserted element in the middle
        // must still trim precisely: the trim alone reduces the DP's input
        // to near-nothing, regardless of the untrimmed array's size.
        let shared: Vec<Value> = (0..2000).map(|i| json!({"N": i})).collect();
        let mut a = shared.clone();
        let mut b = shared.clone();
        // Insert one real difference past the point a naive full-array DP
        // (2000 * 2001 cells) would already exceed the cap, to prove the
        // prefix/suffix trim — not luck — is what keeps this cheap.
        b.insert(1000, json!({"N": "inserted"}));
        a.push(json!({"N": "tail"}));
        b.push(json!({"N": "tail"}));

        let (removed, added, unchanged) = lcs_align(&a, &b);
        assert_eq!(removed, Vec::<Value>::new());
        assert_eq!(added, vec![json!({"N": "inserted"})]);
        assert_eq!(unchanged, a.len());
    }

    #[test]
    fn lcs_align_over_the_safety_cap_reports_the_untrimmed_middle() {
        // Two arrays too large (after prefix/suffix trim) to run the O(n*m)
        // DP within the 250,000-cell cap fall back to reporting the whole
        // untrimmed middle as removed/added — the same behavior `unkeyed`
        // had before LCS trimming existed, just scoped to the part the trim
        // couldn't cheaply resolve, not spending unbounded time on one
        // record's array.
        let a: Vec<Value> = (0..600).map(|i| json!({"N": i, "side": "a"})).collect();
        let b: Vec<Value> = (0..600).map(|i| json!({"N": i, "side": "b"})).collect();
        // No shared prefix/suffix (every element differs by `side`), and
        // 600 * 600 = 360,000 > 250,000, so the DP is skipped.
        let (removed, added, unchanged) = lcs_align(&a, &b);
        assert_eq!(unchanged, 0);
        assert_eq!(removed.len(), 600);
        assert_eq!(added.len(), 600);
    }
}
