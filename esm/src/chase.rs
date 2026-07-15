//! Native port of `tools/chase/chase.py` — automates the "chase pattern" for
//! unique-weapon OMOD effects documented under "How unique-weapon effects are
//! implemented (the chase pattern)" in `.claude/skills/patch-notes/mechanics-kb.md`,
//! and (since the OMOD-only original) generalized to also accept PERK, SPEL,
//! ALCH, and ENCH selectors directly — the record types an OMOD's own
//! forward-fetch hops resolve into. Read the KB section first — this module
//! is a mechanical implementation of the walks it describes, nothing more.
//!
//! [`chase`] dispatches on the resolved selector's record type:
//!
//! - **OMOD** (`omod_chase`) — a `mod_Custom_*` (or similarly named) OMOD
//!   implements its unique mechanic via one or more `Data.Properties[]` rows.
//!   Each row is classified exactly the way the KB describes:
//!
//!   1. **Direct property** — the property's `Value 1` is either a plain number
//!      (a bare stat tweak, nothing to chase further) or a FormID pointing at
//!      an AVIF (an actor value — chased by reverse `refs` to find who reads
//!      it) or an ENCH/SPEL attached directly to the weapon (chased by a
//!      forward `get`, since the effect lives on that record, not behind a
//!      keyword gate).
//!   2. **Perk grant** — `Value 1` is a PERK (property 116/"Perks"). Chased by
//!      a forward `get` on the granted PERK — its `Effects` ARE the mechanic.
//!   3. **Keyword hook** — `Value 1` is a KYWD (property 31/"Keywords"). The
//!      keyword itself carries no behavior; chased by a reverse `refs --type
//!      SPEL,PERK --paths` walk to find the SPEL/PERK whose Conditions test
//!      `WornHasKeyword(<keyword>)`, then the exact `Effects[N]` entry gated by
//!      that condition (located via the `--paths` field path, not a full
//!      record dump).
//!
//! - **PERK/SPEL/ALCH/ENCH** (`effect_chase`) — these records carry their
//!   mechanic directly in their own `Effects[]` array; there's no property-row
//!   indirection to classify. Each entry is either SPEL/ALCH/ENCH-shaped (a
//!   `Base Effect` pointing at an MGEF) or PERK-shaped (a union of
//!   Ability/Quest/Spell/Item/Leveled Item targets, no `Base Effect` key at
//!   all) — `effect_chase` checks for one shape then the other, per entry, and
//!   forward-fetches whatever formid target it finds (mirroring OMOD's
//!   `perk_grant` pattern).
//!
//! **MGEF pass-through** (a 4th mechanism, layered onto both walks above): an
//! MGEF reached via a `Base Effect` reference sometimes itself carries a
//! `"Perk to Apply"` (→ PERK) or `"Equip Ability"` (→ SPEL) field — the real
//! mechanism behind several "tech-migrated" legendary effects (see the KB's
//! "Severing's confirmed chase": `ENCH -> MGEF (Perk to Apply) -> PERK`,
//! previously a manually-chased worked example). `mgef_pass_through` follows
//! this one bounded extra hop and is shared by both walks — OMOD's
//! forward-fetched ENCH/SPEL targets, and `effect_chase`'s own Base-Effect
//! entries and forward-fetched PERK targets.
//!
//! This is a (no-longer-exactly-1:1, since generalized past the original
//! scope) port of the retired Python prototype (`chase.py`), still sharing its
//! output JSON shape for OMOD roots so the /patch-notes deep-writer agent
//! keeps working unchanged. Composes the same operations (`Op::RecordBulk`,
//! `Op::ReferencedBy`) in-process through the [`ChaseFetcher`] seam — no new
//! `Op` variant, no daemon round-trip required by the trait itself (the CLI's
//! concrete fetcher still goes through `Backend::run`, which may hit the warm
//! daemon, but the pure logic here doesn't know or care).

use crate::ipc::RecordSel;
use crate::{BulkRecordEntry, FormId, RefList, RefRow, ResolveDepth};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Record types whose Conditions are checked for a `WornHasKeyword` (or
/// similar) gate on a keyword/AVIF this OMOD ADDs. Mirrors the KB's "SPEL/PERK
/// effect conditioned on WornHasKeyword(...)" — these are the only two record
/// types the chase pattern names as mechanic carriers.
const CONSUMER_TYPES: [&str; 2] = ["SPEL", "PERK"];

/// Target record types whose own Effects/Description are pulled directly
/// (forward fetch) because the OMOD property attaches them straight to the
/// weapon rather than gating them behind a keyword condition.
const FORWARD_FETCH_TYPES: [&str; 3] = ["PERK", "ENCH", "SPEL"];

/// Human `_record_type` values (the long name, e.g. `"Perk"` — not the 4-char
/// signature) [`chase`] accepts as a root selector besides OMOD's own
/// `"Object Modification"`. SPEL/ALCH/ENCH share an identical `Effects[]`
/// shape (`Effect."Base Effect"` -> MGEF); PERK's `Effects[]` is a distinct
/// union (Ability/Quest/Spell/Item/Leveled Item) with no `Base Effect` key at
/// all — `effect_chase` walks both uniformly by checking for one shape then
/// the other, per entry.
const EFFECT_ROOT_TYPES: [&str; 4] = ["Perk", "Spell", "Ingestible", "Enchantment"];

/// `Effects[N].Effect` keys `effect_chase` treats as a PERK-shaped forward
/// target — the first of these present as a formid stub wins. Mirrors the
/// module docs' PERK union description.
const PERK_EFFECT_TARGET_KEYS: [&str; 5] = ["Ability", "Quest", "Spell", "Item", "Leveled Item"];

/// Reverse-ref walk depth for keyword/AVIF consumer lookups (the KB's chase
/// pattern is a single hop; raise only if a mechanic is gated through an
/// intermediary, e.g. a quest alias).
pub const DEFAULT_DEPTH: usize = 1;
/// Cap on refs rows fetched per record-type filter before bulk-fetching consumers.
pub const DEFAULT_REF_LIMIT: usize = 25;

// ─── fetch seam ─────────────────────────────────────────────────────────────

/// Everything [`chase`] needs from the outside world: a bulk record fetch
/// (`Op::RecordBulk`) and a reverse-reference walk (`Op::ReferencedBy`).
///
/// Mirrors the Python prototype's `EsmGateway`/`FakeGateway` seam — keeping
/// all I/O out of the pure walk/classification logic below, so tests can
/// exercise `chase()` against a `FakeFetcher` with no real ESM or daemon
/// involved. The concrete implementor (`BackendFetcher` in `src/bin/cli.rs`)
/// holds the `&Path` to the ESM being queried; `chase()` itself only deals in
/// selectors and FormIDs.
pub trait ChaseFetcher {
    fn bulk_get(
        &mut self,
        sels: &[RecordSel],
        depth: ResolveDepth,
    ) -> anyhow::Result<Vec<BulkRecordEntry>>;

    fn refs(
        &mut self,
        target: FormId,
        depth: usize,
        limit: usize,
        type_filter: &str,
        paths: bool,
    ) -> anyhow::Result<RefList>;
}

// ─── output types ───────────────────────────────────────────────────────────

/// Options controlling the reverse-ref walk depth/cap (see [`DEFAULT_DEPTH`]/
/// [`DEFAULT_REF_LIMIT`]). Only consulted by the OMOD walk — a PERK/SPEL/
/// ALCH/ENCH root's mechanic is already inline in its own `Effects[]`, so
/// `effect_chase` never reverse-chases and these options are a no-op there.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ChaseOptions {
    pub depth: usize,
    pub ref_limit: usize,
}

impl Default for ChaseOptions {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            ref_limit: DEFAULT_REF_LIMIT,
        }
    }
}

/// The evidence tree returned by [`chase`]. `hops` is populated for an OMOD
/// root (classified `Data.Properties[]` rows); `effect_hops` for a
/// PERK/SPEL/ALCH/ENCH root (classified `Effects[]` entries) — exactly one of
/// the two is ever non-empty for a given `chase()` call. Kept as a flat
/// struct with two vectors (rather than an enum) so existing OMOD callers/
/// tests don't need to match on a variant just to reach `.hops`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChaseTree {
    pub root: RootStub,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hops: Vec<Hop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effect_hops: Vec<EffectHop>,
}

/// The chased record's own identity — mirrors the Python prototype's
/// `omod_stub` dict, generalized to any root type [`chase`] accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootStub {
    pub formid: Option<String>,
    pub editor_id: Option<String>,
    /// Short record signature (e.g. `"OMOD"`, `"PERK"`) — same convention
    /// `Hop.target`/`fmt_stub` use elsewhere in this module.
    pub record_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Value>,
}

/// How one `Data.Properties[]` row was classified (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HopKind {
    DirectProperty,
    PerkGrant,
    KeywordHook,
}

impl HopKind {
    fn as_str(self) -> &'static str {
        match self {
            HopKind::DirectProperty => "direct_property",
            HopKind::PerkGrant => "perk_grant",
            HopKind::KeywordHook => "keyword_hook",
        }
    }
}

/// One classified `Data.Properties[]` row plus whatever evidence the chase
/// found to explain what it does downstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hop {
    pub property_index: usize,
    pub property: Value,
    pub function: Value,
    pub value1: Value,
    pub value2: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curve_table: Option<Value>,
    pub kind: HopKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    pub evidence: Vec<Evidence>,
}

/// How one `Effects[]` entry was classified — the Effects[]-array-side analog
/// of [`HopKind`]. Kept as a separate enum (rather than widening `HopKind`)
/// because the two field shapes genuinely don't overlap: see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectHopKind {
    /// SPEL/ALCH/ENCH-shaped entry: `Effect."Base Effect"` -> MGEF.
    BaseEffect,
    /// PERK-shaped entry: one of `PERK_EFFECT_TARGET_KEYS` present as a formid.
    ForwardTarget,
    /// Neither shape matched — a bare stat tweak or entry-point-only effect,
    /// terminal (matches OMOD's bare-scalar `DirectProperty` treatment).
    NoTarget,
}

impl EffectHopKind {
    fn as_str(self) -> &'static str {
        match self {
            EffectHopKind::BaseEffect => "base_effect",
            EffectHopKind::ForwardTarget => "forward_target",
            EffectHopKind::NoTarget => "no_target",
        }
    }
}

/// One classified `Effects[]` entry from a PERK/SPEL/ALCH/ENCH root — the
/// Effects[]-array-side analog of [`Hop`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectHop {
    pub effect_index: usize,
    pub kind: EffectHopKind,
    /// The raw `{"Effect": {...}}` entry — feeds `summarize_effect` for
    /// rendering and is included verbatim in JSON output.
    pub effect: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    pub evidence: Vec<Evidence>,
}

/// One piece of evidence found for a hop — a forward-fetched record's own
/// Description/Effects, a reverse-chased consumer's gated `Effects[N]` entry,
/// or an MGEF pass-through's `Perk to Apply`/`Equip Ability`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub source: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub detail: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hop_depth: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_chain: Option<Value>,
}

// ─── schema helpers (pure `serde_json::Value` walking) ─────────────────────

/// Extract the human name from a `{"value":.., "name":..}` schema enum
/// object, or return the value unchanged if it isn't wrapped that way.
fn named(field: Option<&Value>) -> Value {
    match field {
        Some(Value::Object(map)) => map
            .get("name")
            .cloned()
            .unwrap_or_else(|| field_or_null(field)),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

fn field_or_null(field: Option<&Value>) -> Value {
    field.cloned().unwrap_or(Value::Null)
}

fn is_formid_stub(value: &Value) -> bool {
    matches!(value, Value::Object(map) if map.contains_key("formid"))
}

/// Python-truthiness for a JSON value (`None`/`0`/`""`/`[]`/`{}`/`false` are
/// falsy, matching the prototype's bare `if x:` checks throughout).
fn is_truthy(v: Option<&Value>) -> bool {
    match v {
        None => false,
        Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// Render a JSON value the way Python's `str()`/f-string interpolation would
/// (unquoted strings, `None`/`True`/`False` for null/bool). Used for text
/// rendering and condition summaries only — not for evidence-tree JSON output.
fn pyish(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Python `repr()`-ish rendering, used only for the `value1=.../value2=...`
/// fallback line in [`render_text`] (mirrors `f"value1={value!r}"`).
fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        other => pyish(other),
    }
}

/// `a or b` (Python truthiness), rendered as text — used wherever the
/// prototype does `x.get("editor_id") or x.get("formid")`.
fn py_or_display(a: Option<&Value>, b: Option<&Value>) -> String {
    if is_truthy(a) {
        return pyish(a.unwrap());
    }
    if is_truthy(b) {
        return pyish(b.unwrap());
    }
    "None".to_string()
}

fn truncate_json(v: &Value, max_chars: usize) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    if s.chars().count() > max_chars {
        s.chars().take(max_chars).collect()
    } else {
        s
    }
}

/// `collect_formid_paths` (`src/lib.rs`) builds paths as dot-joined JSON
/// object keys with array indices appended directly to the preceding key,
/// e.g. `"Effects[1].Effect.Conditions.Conditions[0].Condition.Condition
/// Data.Parameter 1"`. Key names may contain spaces but never dots or
/// brackets, so splitting on `.` is safe.
///
/// Split one dot-separated token into its bare key (empty if the token is
/// purely bracket indices) and the list of bracket indices that follow it,
/// e.g. `"Effects[1]"` -> `("Effects", [1])`, `"Conditions"` -> `("Conditions", [])`.
fn split_token(part: &str) -> Option<(&str, Vec<usize>)> {
    match part.find('[') {
        None => Some((part, Vec::new())),
        Some(pos) => {
            let key = &part[..pos];
            let mut rest = &part[pos..];
            let mut indices = Vec::new();
            while !rest.is_empty() {
                let stripped = rest.strip_prefix('[')?;
                let end = stripped.find(']')?;
                let idx: usize = stripped[..end].parse().ok()?;
                indices.push(idx);
                rest = &stripped[end + 1..];
            }
            Some((key, indices))
        }
    }
}

/// Return the path prefix up to and including the first `[N]` index, e.g.
/// `"Effects[1].Effect.Conditions..."` -> `"Effects[1]"`. This isolates the one
/// Effects entry a keyword/AVIF gates, instead of the whole record.
fn first_array_container(path: &str) -> Option<String> {
    let mut prefix: Vec<&str> = Vec::new();
    for part in path.split('.') {
        prefix.push(part);
        if part.contains('[') {
            return Some(prefix.join("."));
        }
    }
    None
}

/// Descend into a decoded record's `fields` value along a dot/`[N]` path.
fn walk_path<'v>(fields: &'v Value, path: &str) -> Option<&'v Value> {
    let mut cur = fields;
    for part in path.split('.') {
        let (key, indices) = split_token(part)?;
        if !key.is_empty() {
            cur = cur.as_object()?.get(key)?;
        }
        for idx in indices {
            cur = cur.as_array()?.get(idx)?;
        }
    }
    Some(cur)
}

fn slice_effect<'v>(fields: &'v Value, path: &str) -> Option<&'v Value> {
    let container = first_array_container(path)?;
    walk_path(fields, &container)
}

/// Recursively find every `Condition Data`-shaped object (has both `Function`
/// and `Operator`) inside `obj` and render it compactly. SPEL and PERK nest
/// conditions differently (`Conditions.Conditions[]` vs `Perk
/// Conditions[].Perk Condition.Conditions[]`) — this walks either.
fn extract_conditions(obj: &Value, acc: &mut Vec<String>) {
    match obj {
        Value::Object(map) => {
            if map.contains_key("Function") && map.contains_key("Operator") {
                let fn_ = pyish(map.get("Function").unwrap_or(&Value::Null));
                let op = pyish(map.get("Operator").unwrap_or(&Value::Null));
                let val = pyish(map.get("Comparison Value").unwrap_or(&Value::Null));
                let param = map.get("Parameter 1");
                let line = match param {
                    Some(Value::Object(pmap)) => {
                        let param_txt = py_or_display(pmap.get("editor_id"), pmap.get("formid"));
                        format!("{fn_}({param_txt}) {op} {val}")
                    }
                    Some(p) if !p.is_null() => format!("{fn_}({}) {op} {val}", pyish(p)),
                    _ => format!("{fn_} {op} {val}"),
                };
                acc.push(line);
            } else {
                for v in map.values() {
                    extract_conditions(v, acc);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                extract_conditions(v, acc);
            }
        }
        _ => {}
    }
}

/// Keys already surfaced explicitly by [`summarize_effect`] — anything else on
/// the effect object is scanned generically so fields like PERK's "Function
/// Parameter 3 (Actor Value)" (e.g. the AVIF a Kill-Streak-style perk reads)
/// aren't silently dropped just because they're not one of the handful of
/// well-known keys.
const HANDLED_EFFECT_KEYS: &[&str] = &[
    "Base Effect",
    "Entry Point",
    "Effect Item Data",
    "Float",
    "Conditions",
    "Perk Conditions",
    "Effect Header",
    "Effect Flags",
    "Cooldown Duration",
    "Effect End",
];

/// Render one `Effects[]` element (`{"Effect": {...}}`, SPEL or PERK shape) as
/// a single compact line: base effect / entry point, magnitude, duration, any
/// other FormID reference on the effect, and any gating conditions found
/// anywhere inside it.
fn summarize_effect(effect_entry: &Value) -> String {
    let inner = match effect_entry.as_object() {
        Some(map) => map.get("Effect").unwrap_or(effect_entry),
        None => return truncate_json(effect_entry, 200),
    };
    let Some(inner_map) = inner.as_object() else {
        return truncate_json(effect_entry, 200);
    };

    let mut parts: Vec<String> = Vec::new();

    if let Some(base) = inner_map.get("Base Effect").and_then(Value::as_object) {
        parts.push(py_or_display(base.get("editor_id"), base.get("formid")));
    }

    if let Some(entry_point) = inner_map.get("Entry Point").and_then(Value::as_object) {
        let ep_name = named(entry_point.get("Entry Point"));
        let fn_name = named(entry_point.get("Function"));
        if is_truthy(Some(&ep_name)) {
            if is_truthy(Some(&fn_name)) {
                parts.push(format!("{}/{}", pyish(&ep_name), pyish(&fn_name)));
            } else {
                parts.push(pyish(&ep_name));
            }
        }
    }

    if let Some(item_data) = inner_map.get("Effect Item Data").and_then(Value::as_object) {
        if let Some(mag) = item_data.get("Magnitude") {
            if !mag.is_null() {
                parts.push(format!("Magnitude={}", pyish(mag)));
            }
        }
        if is_truthy(item_data.get("Duration")) {
            parts.push(format!(
                "Duration={}",
                pyish(item_data.get("Duration").unwrap())
            ));
        }
    }

    if let Some(f) = inner_map.get("Float") {
        parts.push(format!("Float={}", pyish(f)));
    }

    // Generic pass: any other FormID-stub field on the effect itself (not
    // nested under Conditions, handled separately below) — e.g. a PERK's
    // "Function Parameter N (Actor Value)" pointing at an AVIF.
    for (key, val) in inner_map {
        if HANDLED_EFFECT_KEYS.contains(&key.as_str()) {
            continue;
        }
        if let Some(obj) = val.as_object() {
            if obj.contains_key("formid") {
                parts.push(format!(
                    "{key}={}",
                    py_or_display(obj.get("editor_id"), obj.get("formid"))
                ));
            }
        }
    }

    let mut text = parts.join("  ");
    let conditions_src = inner_map
        .get("Conditions")
        .filter(|v| is_truthy(Some(*v)))
        .or_else(|| {
            inner_map
                .get("Perk Conditions")
                .filter(|v| is_truthy(Some(*v)))
        });
    if let Some(cs) = conditions_src {
        let mut acc = Vec::new();
        extract_conditions(cs, &mut acc);
        if !acc.is_empty() {
            if !text.is_empty() {
                text.push_str("  ");
            }
            text.push_str("Conditions: ");
            text.push_str(&acc.join("; "));
        }
    }

    if text.is_empty() {
        truncate_json(inner, 200)
    } else {
        text
    }
}

/// Scan an `Effects[]` array for `Effect."Base Effect"` formid-stub targets
/// whose `record_type` is `"MGEF"` (present on SPEL/ALCH/ENCH-shaped Effects;
/// absent on PERK's Ability/Quest/Item union — this check is naturally a
/// no-op there, no type-specific gating needed), deduped by formid.
fn mgef_targets_in_effects_array(effects: &[Value]) -> Vec<Value> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for entry in effects {
        let Some(inner) = entry.get("Effect").and_then(Value::as_object) else {
            continue;
        };
        let Some(base) = inner.get("Base Effect") else {
            continue;
        };
        if !is_formid_stub(base) {
            continue;
        }
        let rt = base
            .get("record_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        if rt != "MGEF" {
            continue;
        }
        let fid = base
            .get("formid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if fid.is_empty() || !seen.insert(fid) {
            continue;
        }
        out.push(stub(base));
    }
    out
}

/// Given an already-bulk-fetched MGEF target (looked up in `by_sel`, keyed by
/// formid string), extract `"Perk to Apply"`/`"Equip Ability"` into a compact
/// [`Evidence`]. `None` if the MGEF wasn't found/failed to fetch, or has
/// neither field set — the common case, most magic effects are plain
/// damage/buff effects with nothing further to chase.
fn mgef_pass_through_evidence(
    mgef_target: &Value,
    by_sel: &HashMap<&str, &BulkRecordEntry>,
) -> Option<Evidence> {
    let formid = mgef_target
        .get("formid")
        .and_then(Value::as_str)
        .unwrap_or("");
    let entry = by_sel.get(formid).copied()?;
    let fields = entry.fields.as_ref()?;
    let perk_to_apply = walk_path(fields, "Magic Effect Data.Data.Perk to Apply")
        .filter(|v| is_truthy(Some(*v)))
        .cloned();
    let equip_ability = walk_path(fields, "Magic Effect Data.Data.Equip Ability")
        .filter(|v| is_truthy(Some(*v)))
        .cloned();
    if perk_to_apply.is_none() && equip_ability.is_none() {
        return None;
    }
    let mut detail = serde_json::Map::new();
    if let Some(p) = perk_to_apply {
        detail.insert("perk_to_apply".to_string(), p);
    }
    if let Some(e) = equip_ability {
        detail.insert("equip_ability".to_string(), e);
    }
    Some(Evidence {
        source: stub(mgef_target),
        via: Some("Base Effect".to_string()),
        detail: Value::Object(detail),
        hop_depth: None,
        path_chain: None,
    })
}

// ─── the chase ───────────────────────────────────────────────────────────────

/// Normalize a FormID-stub-shaped value (`{"formid"/"form_id", "editor_id",
/// "record_type"}`) to the canonical `{"formid", "editor_id", "record_type"}`
/// shape. Accepts either key spelling for the FormID itself so it can stub
/// both a decoded FormID reference (`"formid"`, from [`crate::FormIdStub`])
/// and a `RefRow` (`"form_id"`) with the same helper.
fn stub(v: &Value) -> Value {
    let formid = v
        .get("formid")
        .or_else(|| v.get("form_id"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "formid": formid,
        "editor_id": v.get("editor_id").cloned().unwrap_or(Value::Null),
        "record_type": v.get("record_type").cloned().unwrap_or(Value::Null),
    })
}

fn forward_evidence(target: &Value, by_sel: &HashMap<&str, &BulkRecordEntry>) -> Evidence {
    let formid = target.get("formid").and_then(Value::as_str).unwrap_or("");
    let entry = by_sel.get(formid).copied();
    let entry = match entry {
        None => {
            return Evidence {
                source: stub(target),
                via: None,
                detail: json!({"note": "fetch failed: no response"}),
                hop_depth: None,
                path_chain: None,
            }
        }
        Some(e) => e,
    };
    if let Some(err) = &entry.error {
        return Evidence {
            source: stub(target),
            via: None,
            detail: json!({"note": format!("fetch failed: {err}")}),
            hop_depth: None,
            path_chain: None,
        };
    }
    let fields = entry.fields.clone().unwrap_or(Value::Null);
    let mut detail = serde_json::Map::new();
    if is_truthy(fields.get("Description")) {
        detail.insert("description".to_string(), fields["Description"].clone());
    }
    if let Some(effects) = fields.get("Effects").and_then(Value::as_array) {
        if !effects.is_empty() {
            let capped: Vec<Value> = effects.iter().take(12).cloned().collect();
            let truncated = effects.len().saturating_sub(capped.len());
            detail.insert("effects".to_string(), Value::Array(capped));
            if truncated > 0 {
                detail.insert("effects_truncated".to_string(), json!(truncated));
            }
        }
    }
    if detail.is_empty() {
        detail.insert(
            "note".to_string(),
            json!("no Description/Effects field on this record"),
        );
    }
    Evidence {
        source: stub(target),
        via: None,
        detail: Value::Object(detail),
        hop_depth: None,
        path_chain: None,
    }
}

/// One reverse-`refs` call per [`CONSUMER_TYPES`] entry (SPEL, PERK) against a
/// keyword/AVIF `target_fid` — the "who reads this?" half of the chase
/// pattern. Shared verbatim by [`reverse_chase`] (which flattens every type's
/// rows into one evidence list) and `walk`'s KYWD/AVIF digest (which keeps
/// SPEL/PERK grouped under separate headers) — factored here so the `refs`
/// call sequence isn't duplicated between the two callers.
pub(crate) fn consumer_refs_by_type(
    f: &mut impl ChaseFetcher,
    target_fid: FormId,
    depth: usize,
    limit: usize,
) -> anyhow::Result<Vec<(&'static str, RefList)>> {
    let mut out = Vec::with_capacity(CONSUMER_TYPES.len());
    for record_type in CONSUMER_TYPES {
        let ref_list = f.refs(target_fid, depth, limit, record_type, true)?;
        out.push((record_type, ref_list));
    }
    Ok(out)
}

/// Reverse `refs --type SPEL` + `--type PERK` walk on `target` (a keyword or
/// AVIF), then a single bulk fetch for every distinct consumer found, slicing
/// out just the `Effects[N]` entry each `--paths` field path points at (see
/// module docs, pattern 1/3).
fn reverse_chase(
    f: &mut impl ChaseFetcher,
    target: &Value,
    depth: usize,
    limit: usize,
) -> anyhow::Result<Vec<Evidence>> {
    let formid_str = target.get("formid").and_then(Value::as_str).unwrap_or("");
    let target_fid = crate::parse_form_id_input(formid_str)
        .with_context(|| format!("invalid target FormID {formid_str:?} on reverse-chase target"))?;

    let rows: Vec<RefRow> = consumer_refs_by_type(f, target_fid, depth, limit)?
        .into_iter()
        .flat_map(|(_, ref_list)| ref_list.rows)
        .collect();
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut ids: Vec<String> = rows.iter().map(|r| r.form_id.clone()).collect();
    ids.sort();
    ids.dedup();
    let sels: Vec<RecordSel> = ids
        .iter()
        .map(|s| crate::parse_form_id_input(s).map(RecordSel::FormId))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let fetched = f.bulk_get(&sels, ResolveDepth::Stub)?;
    let by_sel: HashMap<&str, &BulkRecordEntry> =
        fetched.iter().map(|e| (e.sel.as_str(), e)).collect();

    let mut evidence = Vec::new();
    for row in &rows {
        let entry = by_sel.get(row.form_id.as_str()).copied();
        let fields = entry.and_then(|e| e.fields.clone()).unwrap_or(Value::Null);
        let paths: Vec<Option<&str>> = match &row.field_paths {
            Some(p) if !p.is_empty() => p.iter().map(|s| Some(s.as_str())).collect(),
            _ => vec![None],
        };
        let row_value = serde_json::to_value(row).unwrap_or(Value::Null);
        for path in paths {
            let sliced = path.and_then(|p| slice_effect(&fields, p));
            let detail = match sliced {
                Some(v) => json!({"effect": v}),
                None => json!({
                    "note": "reference confirmed but the exact effect could not be \
                             isolated from the field path; inspect the full record"
                }),
            };
            let (hop_depth, path_chain) = if row.depth > 1 {
                (
                    Some(row.depth),
                    Some(serde_json::to_value(&row.path).unwrap_or(Value::Null)),
                )
            } else {
                (None, None)
            };
            evidence.push(Evidence {
                source: stub(&row_value),
                via: path.map(str::to_string),
                detail,
                hop_depth,
                path_chain,
            });
        }
    }
    Ok(evidence)
}

/// Follow `Effects[].Base Effect -> MGEF -> {Perk to Apply, Equip Ability}` as
/// one bounded extra forward hop, batched into a single `bulk_get` regardless
/// of how many `sources` are scanned. `sources` pairs a hop-vector index with
/// the `Effects[]` array to scan for that hop (a forward-fetched target's own
/// `detail.effects`, or a root's own Base-Effect-shaped entry). Returns
/// `(index, Evidence)` pairs for the caller to push onto the right hop's
/// evidence list; a source with no MGEF carrying a pass-through field
/// contributes nothing.
fn mgef_pass_through(
    f: &mut impl ChaseFetcher,
    sources: &[(usize, Vec<Value>)],
) -> anyhow::Result<Vec<(usize, Evidence)>> {
    let mut all_targets: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (_, effects) in sources {
        for t in mgef_targets_in_effects_array(effects) {
            let fid = t
                .get("formid")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if seen.insert(fid) {
                all_targets.push(t);
            }
        }
    }
    if all_targets.is_empty() {
        return Ok(Vec::new());
    }

    let sels: Vec<RecordSel> = all_targets
        .iter()
        .map(|t| {
            let fid_str = t.get("formid").and_then(Value::as_str).unwrap_or("");
            crate::parse_form_id_input(fid_str).map(RecordSel::FormId)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let fetched = f.bulk_get(&sels, ResolveDepth::Stub)?;
    let by_sel: HashMap<&str, &BulkRecordEntry> =
        fetched.iter().map(|e| (e.sel.as_str(), e)).collect();

    let mut out = Vec::new();
    for (idx, effects) in sources {
        for target in mgef_targets_in_effects_array(effects) {
            if let Some(ev) = mgef_pass_through_evidence(&target, &by_sel) {
                out.push((*idx, ev));
            }
        }
    }
    Ok(out)
}

fn build_root_stub(entry: &BulkRecordEntry, fields: &Value) -> RootStub {
    RootStub {
        formid: entry.header.as_ref().map(|h| h.form_id.display()),
        record_type: entry.header.as_ref().map(|h| h.signature.clone()),
        editor_id: entry.editor_id.clone(),
        name: fields.get("Name").filter(|v| is_truthy(Some(*v))).cloned(),
        description: fields
            .get("Description")
            .filter(|v| is_truthy(Some(*v)))
            .cloned(),
    }
}

/// Run the full chase for one selector and return the evidence tree.
/// Dispatches on the resolved record's `_record_type`: OMOD gets the
/// `Data.Properties[]` walk ([`omod_chase`]); PERK/SPEL/ALCH/ENCH get the
/// `Effects[]` walk ([`effect_chase`]); anything else is rejected.
///
/// `f` is anything implementing [`ChaseFetcher`] — normally a
/// `Backend`-backed fetcher (see `cmd_chase` in `src/bin/cli.rs`), or a fake
/// for tests (see `tests/chase.rs`).
pub fn chase(
    f: &mut impl ChaseFetcher,
    selector: RecordSel,
    opts: &ChaseOptions,
) -> anyhow::Result<ChaseTree> {
    let selector_display = selector.display();
    let entries = f.bulk_get(std::slice::from_ref(&selector), ResolveDepth::Stub)?;
    let entry = entries
        .into_iter()
        .next()
        .with_context(|| format!("bulk_get returned no entries for {selector_display:?}"))?;
    if let Some(err) = &entry.error {
        bail!("failed to resolve {selector_display:?}: {err}");
    }

    let fields = entry.fields.clone().unwrap_or(Value::Null);
    let record_type = fields.get("_record_type").and_then(Value::as_str);
    let root = build_root_stub(&entry, &fields);

    if let Some(rt) = record_type {
        if rt == "Object Modification" {
            return omod_chase(f, root, &fields, opts);
        }
        if EFFECT_ROOT_TYPES.contains(&rt) {
            return effect_chase(f, root, &fields);
        }
    }

    let got = record_type
        .map(str::to_string)
        .or_else(|| entry.header.as_ref().map(|h| h.signature.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    bail!(
        "{selector_display:?} resolves to a {got:?} record — chase supports \
         OMOD, PERK, SPEL, ALCH, and ENCH selectors only"
    );
}

/// Run the chase for an OMOD root: classify each `Data.Properties[]` row into
/// direct-property/perk-grant/keyword-hook (see the module docs) and forward-
/// or reverse-fetch whatever record carries the mechanic.
fn omod_chase(
    f: &mut impl ChaseFetcher,
    root: RootStub,
    fields: &Value,
    opts: &ChaseOptions,
) -> anyhow::Result<ChaseTree> {
    let properties: Vec<Value> = fields
        .pointer("/Data/Properties")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut hops: Vec<Hop> = Vec::with_capacity(properties.len());
    let mut forward_targets: Vec<(usize, Value)> = Vec::new();
    let mut reverse_targets: Vec<(usize, Value)> = Vec::new();

    for (i, prop) in properties.iter().enumerate() {
        let prop_name = named(prop.get("Property"));
        let function = named(prop.get("Function Type"));
        let value1 = field_or_null(prop.get("Value 1"));
        let value2 = field_or_null(prop.get("Value 2"));
        let curve_table = prop
            .get("Curve Table")
            .filter(|v| is_truthy(Some(*v)))
            .cloned();

        let mut hop = Hop {
            property_index: i,
            property: prop_name,
            function,
            value1: value1.clone(),
            value2,
            curve_table,
            kind: HopKind::DirectProperty,
            target: None,
            evidence: Vec::new(),
        };

        if !is_formid_stub(&value1) {
            hop.kind = HopKind::DirectProperty;
            hop.evidence = Vec::new();
            hops.push(hop);
            continue;
        }

        let target = stub(&value1);
        let rt = target
            .get("record_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        hop.target = Some(target.clone());

        if rt == "KYWD" {
            hop.kind = HopKind::KeywordHook;
            reverse_targets.push((i, target));
        } else if rt == "PERK" {
            hop.kind = HopKind::PerkGrant;
            forward_targets.push((i, target));
        } else if FORWARD_FETCH_TYPES.contains(&rt.as_str()) {
            hop.kind = HopKind::DirectProperty;
            forward_targets.push((i, target));
        } else if rt == "AVIF" {
            hop.kind = HopKind::DirectProperty;
            reverse_targets.push((i, target));
        } else {
            hop.kind = HopKind::DirectProperty;
            hop.evidence = Vec::new();
        }

        hops.push(hop);
    }

    // ---- forward fetch (perk_grant + direct ENCH/SPEL attachments): 1 bulk call ----
    if !forward_targets.is_empty() {
        let sels: Vec<RecordSel> = forward_targets
            .iter()
            .map(|(_, t)| {
                let fid_str = t.get("formid").and_then(Value::as_str).unwrap_or("");
                crate::parse_form_id_input(fid_str).map(RecordSel::FormId)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let fetched = f.bulk_get(&sels, ResolveDepth::Stub)?;
        let by_sel: HashMap<&str, &BulkRecordEntry> =
            fetched.iter().map(|e| (e.sel.as_str(), e)).collect();
        for (i, target) in &forward_targets {
            hops[*i].evidence = vec![forward_evidence(target, &by_sel)];
        }

        // MGEF pass-through: if a forward-fetched target's own Effects[] carry
        // a Base Effect resolving to an MGEF with "Perk to Apply"/"Equip
        // Ability" set (the ENCH -> MGEF -> PERK "tech migration" pattern from
        // the mechanics KB), surface it as one more Evidence entry on the hop.
        let mgef_sources: Vec<(usize, Vec<Value>)> = forward_targets
            .iter()
            .filter_map(|(i, _)| {
                let effects = hops[*i]
                    .evidence
                    .first()?
                    .detail
                    .get("effects")?
                    .as_array()?
                    .clone();
                Some((*i, effects))
            })
            .collect();
        for (idx, ev) in mgef_pass_through(f, &mgef_sources)? {
            hops[idx].evidence.push(ev);
        }
    }

    // ---- reverse chase (keyword_hook + AVIF consumer lookup) ----
    for (i, target) in &reverse_targets {
        hops[*i].evidence = reverse_chase(f, target, opts.depth, opts.ref_limit)?;
    }

    Ok(ChaseTree {
        root,
        hops,
        effect_hops: Vec::new(),
    })
}

/// Run the chase for a PERK/SPEL/ALCH/ENCH root: walk its own `Effects[]`
/// array, classifying each entry as a direct SPEL/ALCH/ENCH-shaped `Base
/// Effect` (chased one hop further into its MGEF's `Perk to Apply`/`Equip
/// Ability`, see [`mgef_pass_through`]) or a PERK-shaped forward target
/// (Ability/Quest/Spell/Item/Leveled Item — forward-fetched via the same
/// [`forward_evidence`] OMOD's `perk_grant` hops use) or a bare stat tweak
/// with nothing to chase. Never reverse-chases — these records already carry
/// their mechanic inline, unlike an OMOD's indirect property-row mechanism.
fn effect_chase(
    f: &mut impl ChaseFetcher,
    root: RootStub,
    fields: &Value,
) -> anyhow::Result<ChaseTree> {
    let effects: Vec<Value> = fields
        .get("Effects")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut hops: Vec<EffectHop> = Vec::with_capacity(effects.len());
    let mut forward_targets: Vec<(usize, Value)> = Vec::new();

    for (i, entry) in effects.iter().enumerate() {
        let inner_obj = entry.get("Effect").and_then(Value::as_object);

        let mut kind = EffectHopKind::NoTarget;
        let mut target: Option<Value> = None;

        if let Some(inner) = inner_obj {
            if let Some(base) = inner.get("Base Effect") {
                if is_formid_stub(base) {
                    kind = EffectHopKind::BaseEffect;
                    target = Some(stub(base));
                }
            }
            if target.is_none() {
                for key in PERK_EFFECT_TARGET_KEYS {
                    if let Some(t) = inner.get(key) {
                        if is_formid_stub(t) {
                            kind = EffectHopKind::ForwardTarget;
                            target = Some(stub(t));
                            break;
                        }
                    }
                }
            }
        }

        if kind == EffectHopKind::ForwardTarget {
            forward_targets.push((i, target.clone().unwrap()));
        }

        hops.push(EffectHop {
            effect_index: i,
            kind,
            effect: entry.clone(),
            target,
            evidence: Vec::new(),
        });
    }

    // ---- forward fetch (PERK's Ability/Quest/Spell/Item/Leveled Item targets): 1 bulk call ----
    if !forward_targets.is_empty() {
        let sels: Vec<RecordSel> = forward_targets
            .iter()
            .map(|(_, t)| {
                let fid_str = t.get("formid").and_then(Value::as_str).unwrap_or("");
                crate::parse_form_id_input(fid_str).map(RecordSel::FormId)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let fetched = f.bulk_get(&sels, ResolveDepth::Stub)?;
        let by_sel: HashMap<&str, &BulkRecordEntry> =
            fetched.iter().map(|e| (e.sel.as_str(), e)).collect();
        for (i, target) in &forward_targets {
            hops[*i].evidence = vec![forward_evidence(target, &by_sel)];
        }
    }

    // ---- MGEF pass-through: the root's own Base-Effect-shaped entries, plus
    // any forward-fetched target's own Effects[] (e.g. a PERK rank granting a
    // SPEL Ability whose Base Effect -> MGEF carries "Perk to Apply"). ----
    let mut mgef_sources: Vec<(usize, Vec<Value>)> = Vec::new();
    for hop in &hops {
        if hop.kind == EffectHopKind::BaseEffect {
            mgef_sources.push((hop.effect_index, vec![hop.effect.clone()]));
        }
    }
    for (i, _) in &forward_targets {
        if let Some(effects) = hops[*i]
            .evidence
            .first()
            .and_then(|ev| ev.detail.get("effects"))
            .and_then(Value::as_array)
        {
            mgef_sources.push((*i, effects.clone()));
        }
    }
    for (idx, ev) in mgef_pass_through(f, &mgef_sources)? {
        hops[idx].evidence.push(ev);
    }

    Ok(ChaseTree {
        root,
        hops: Vec::new(),
        effect_hops: hops,
    })
}

// ─── rendering ───────────────────────────────────────────────────────────────

fn fmt_stub(stub: &Value) -> String {
    let rt = stub
        .get("record_type")
        .filter(|v| is_truthy(Some(*v)))
        .map(pyish)
        .unwrap_or_else(|| "?".to_string());
    let fid = stub
        .get("formid")
        .filter(|v| is_truthy(Some(*v)))
        .map(pyish)
        .unwrap_or_else(|| "?".to_string());
    let edid = stub
        .get("editor_id")
        .filter(|v| is_truthy(Some(*v)))
        .map(pyish)
        .unwrap_or_default();
    format!("{rt} {fid} {edid}").trim_end().to_string()
}

/// Render one [`Evidence`] entry's lines (source/via header, then whichever of
/// description/effect/effects+truncation/perk_to_apply/equip_ability/note the
/// detail carries) — shared by the OMOD hops loop and the PERK/SPEL/ALCH/ENCH
/// effect-hops loop in [`render_text`].
fn render_evidence(lines: &mut Vec<String>, ev: &Evidence) {
    let via = ev
        .via
        .as_deref()
        .map(|v| format!("  via {v}"))
        .unwrap_or_default();
    lines.push(format!("      -> {}{}", fmt_stub(&ev.source), via));
    let detail = &ev.detail;
    if let Some(desc) = detail.get("description").filter(|v| is_truthy(Some(*v))) {
        lines.push(format!("         Description: \"{}\"", pyish(desc)));
    }
    if let Some(effect) = detail.get("effect") {
        lines.push(format!("         Effect: {}", summarize_effect(effect)));
    }
    if let Some(effects) = detail.get("effects").and_then(Value::as_array) {
        for eff in effects {
            lines.push(format!("         Effect: {}", summarize_effect(eff)));
        }
        if let Some(trunc) = detail
            .get("effects_truncated")
            .filter(|v| is_truthy(Some(*v)))
        {
            lines.push(format!(
                "         ... +{} more effects (truncated)",
                pyish(trunc)
            ));
        }
    }
    if let Some(p) = detail.get("perk_to_apply").filter(|v| is_truthy(Some(*v))) {
        lines.push(format!("         Perk to Apply: {}", fmt_stub(p)));
    }
    if let Some(e) = detail.get("equip_ability").filter(|v| is_truthy(Some(*v))) {
        lines.push(format!("         Equip Ability: {}", fmt_stub(e)));
    }
    if let Some(note) = detail.get("note") {
        lines.push(format!("         Note: {}", pyish(note)));
    }
}

fn render_effect_hop(lines: &mut Vec<String>, hop: &EffectHop) {
    lines.push(String::new());
    lines.push(format!(
        "  [{}] {} ({})",
        hop.effect_index,
        summarize_effect(&hop.effect),
        hop.kind.as_str()
    ));
    if let Some(target) = &hop.target {
        lines.push(format!("      -> {}", fmt_stub(target)));
    }
    for ev in &hop.evidence {
        render_evidence(lines, ev);
    }
}

/// Render a [`ChaseTree`] as human-readable text — the CLI's default (non
/// `--json`) output. Dispatches on which of `hops`/`effect_hops` is populated;
/// the OMOD path ports `chase.py`'s `render_text` line-for-line.
pub fn render_text(tree: &ChaseTree) -> String {
    let mut lines: Vec<String> = Vec::new();
    let root = &tree.root;
    let mut header = format!(
        "{} {} {}",
        root.record_type.as_deref().unwrap_or("?"),
        root.formid.as_deref().unwrap_or("None"),
        root.editor_id.as_deref().unwrap_or("None"),
    );
    if is_truthy(root.name.as_ref()) {
        header.push_str(&format!("  \"{}\"", pyish(root.name.as_ref().unwrap())));
    }
    lines.push(header);
    if is_truthy(root.description.as_ref()) {
        lines.push(format!(
            "  Description: \"{}\"",
            pyish(root.description.as_ref().unwrap())
        ));
    }

    if !tree.effect_hops.is_empty() {
        for hop in &tree.effect_hops {
            render_effect_hop(&mut lines, hop);
        }
        return lines.join("\n");
    }

    if tree.hops.is_empty() {
        lines.push("  (nothing to chase — no Properties/Effects on this record)".to_string());
        return lines.join("\n");
    }

    for hop in &tree.hops {
        lines.push(String::new());
        lines.push(format!(
            "  [{}] {} {} ({})",
            hop.property_index,
            pyish(&hop.property),
            pyish(&hop.function),
            hop.kind.as_str()
        ));
        if let Some(target) = &hop.target {
            lines.push(format!("      -> {}", fmt_stub(target)));
        } else {
            lines.push(format!(
                "      value1={} value2={}",
                py_repr(&hop.value1),
                py_repr(&hop.value2)
            ));
        }
        if let Some(ct) = &hop.curve_table {
            lines.push(format!("      curve_table={}", pyish(ct)));
        }

        let is_avif = hop
            .target
            .as_ref()
            .and_then(|t| t.get("record_type"))
            .and_then(Value::as_str)
            == Some("AVIF");
        if hop.evidence.is_empty() {
            if hop.kind == HopKind::KeywordHook || is_avif {
                lines.push(
                    "      (no SPEL/PERK condition references this target — dead end; \
                     may be UI-only, native-engine-consumed, or a shared/common tag)"
                        .to_string(),
                );
            }
            continue;
        }

        for ev in &hop.evidence {
            render_evidence(&mut lines, ev);
        }
    }

    lines.join("\n")
}

// ─── colocated unit tests for private helpers ───────────────────────────────
// `first_array_container`/`walk_path`/`named`/`is_formid_stub`/`stub` are
// private and not reachable from an external `tests/` integration crate, so
// these stay colocated (see esm/CLAUDE.md's testing conventions).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_extracts_name_from_enum_object() {
        let v = json!({"value": 31, "name": "Keywords"});
        assert_eq!(named(Some(&v)), json!("Keywords"));
    }

    #[test]
    fn named_passes_through_non_enum_values() {
        assert_eq!(named(Some(&json!(1.5))), json!(1.5));
        assert_eq!(named(None), Value::Null);
    }

    #[test]
    fn is_formid_stub_detects_formid_key() {
        assert!(is_formid_stub(
            &json!({"formid": "0x123", "record_type": "KYWD"})
        ));
        assert!(!is_formid_stub(&json!(1.5)));
        assert!(!is_formid_stub(&json!({"editor_id": "x"})));
    }

    #[test]
    fn first_array_container_isolates_first_index() {
        assert_eq!(
            first_array_container("Effects[1].Effect.Conditions.Conditions[0].Parameter 1"),
            Some("Effects[1]".to_string())
        );
        assert_eq!(first_array_container("Description"), None);
    }

    #[test]
    fn first_array_container_handles_consecutive_brackets() {
        // Array-of-arrays: two bracket groups on one token, no dot between them.
        assert_eq!(
            first_array_container("Foo[0][1].Bar"),
            Some("Foo[0][1]".to_string())
        );
    }

    #[test]
    fn walk_path_descends_through_objects_and_arrays() {
        let fields = json!({
            "Effects": [
                {"Effect": {"Base Effect": {"formid": "0x1"}}},
                {"Effect": {"Base Effect": {"formid": "0x2"}}},
            ]
        });
        let found = walk_path(&fields, "Effects[1].Effect.Base Effect.formid");
        assert_eq!(found, Some(&json!("0x2")));
    }

    #[test]
    fn walk_path_returns_none_on_missing_key_or_out_of_range_index() {
        let fields = json!({"Effects": [{"a": 1}]});
        assert_eq!(walk_path(&fields, "Effects[5].a"), None);
        assert_eq!(walk_path(&fields, "Missing.Key"), None);
    }

    #[test]
    fn slice_effect_returns_the_containing_array_element() {
        let fields = json!({
            "Effects": [
                {"Effect": {"x": 1}},
                {"Effect": {"x": 2, "Conditions": {"Conditions": []}}},
            ]
        });
        let sliced = slice_effect(
            &fields,
            "Effects[1].Effect.Conditions.Conditions[0].Parameter 1",
        );
        assert_eq!(
            sliced,
            Some(&json!({"Effect": {"x": 2, "Conditions": {"Conditions": []}}}))
        );
    }

    #[test]
    fn stub_normalizes_formid_and_form_id_keys() {
        let from_value1 = json!({"formid": "0x1", "editor_id": "e", "record_type": "KYWD"});
        assert_eq!(
            stub(&from_value1),
            json!({"formid": "0x1", "editor_id": "e", "record_type": "KYWD"})
        );

        let from_ref_row = json!({"form_id": "0x2", "editor_id": "f", "record_type": "SPEL"});
        assert_eq!(
            stub(&from_ref_row),
            json!({"formid": "0x2", "editor_id": "f", "record_type": "SPEL"})
        );
    }

    #[test]
    fn is_truthy_matches_python_semantics() {
        assert!(!is_truthy(None));
        assert!(!is_truthy(Some(&Value::Null)));
        assert!(!is_truthy(Some(&json!(0))));
        assert!(!is_truthy(Some(&json!(""))));
        assert!(!is_truthy(Some(&json!([]))));
        assert!(!is_truthy(Some(&json!({}))));
        assert!(is_truthy(Some(&json!(1))));
        assert!(is_truthy(Some(&json!("x"))));
    }

    #[test]
    fn summarize_effect_renders_base_effect_magnitude_and_conditions() {
        let effect = json!({
            "Effect": {
                "Base Effect": {"formid": "0x500031", "editor_id": "TestSpellEffect"},
                "Effect Item Data": {"Magnitude": 25},
                "Conditions": {
                    "Conditions": [
                        {
                            "Function": "WornHasKeyword",
                            "Operator": "EqualTo",
                            "Comparison Value": 1.0,
                            "Parameter 1": {"formid": "0x500010", "editor_id": "if_tmp_TestTag"},
                        }
                    ]
                }
            }
        });
        let text = summarize_effect(&effect);
        assert!(text.contains("TestSpellEffect"));
        assert!(text.contains("Magnitude=25"));
        assert!(text.contains("Conditions: WornHasKeyword(if_tmp_TestTag) EqualTo 1"));
    }

    #[test]
    fn mgef_targets_in_effects_array_finds_mgef_base_effect() {
        let effects = vec![json!({
            "Effect": {"Base Effect": {"formid": "0x1", "editor_id": "e", "record_type": "MGEF"}}
        })];
        let targets = mgef_targets_in_effects_array(&effects);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0]["formid"], json!("0x1"));
    }

    #[test]
    fn mgef_targets_in_effects_array_dedupes_repeated_mgef() {
        let effects = vec![
            json!({"Effect": {"Base Effect": {"formid": "0x1", "record_type": "MGEF"}}}),
            json!({"Effect": {"Base Effect": {"formid": "0x1", "record_type": "MGEF"}}}),
        ];
        assert_eq!(mgef_targets_in_effects_array(&effects).len(), 1);
    }

    #[test]
    fn mgef_targets_in_effects_array_no_ops_on_perk_shaped_entries() {
        // PERK's Ability-shaped effect has no "Base Effect" key at all.
        let effects = vec![json!({
            "Effect": {"Ability": {"formid": "0x1", "record_type": "SPEL"}}
        })];
        assert!(mgef_targets_in_effects_array(&effects).is_empty());
    }

    #[test]
    fn mgef_targets_in_effects_array_ignores_non_mgef_base_effect() {
        let effects = vec![json!({
            "Effect": {"Base Effect": {"formid": "0x1", "record_type": "SPEL"}}
        })];
        assert!(mgef_targets_in_effects_array(&effects).is_empty());
    }

    #[test]
    fn mgef_pass_through_evidence_extracts_perk_to_apply_and_equip_ability() {
        let mgef_target = json!({"formid": "0x1", "editor_id": "e", "record_type": "MGEF"});
        let entry = ok_test_entry(
            "0x1",
            json!({
                "Magic Effect Data": {"Data": {
                    "Perk to Apply": {"formid": "0x2", "editor_id": "p", "record_type": "PERK"},
                    "Equip Ability": {"formid": "0x3", "editor_id": "s", "record_type": "SPEL"},
                }}
            }),
        );
        let by_sel: HashMap<&str, &BulkRecordEntry> = [("0x1", &entry)].into_iter().collect();
        let ev = mgef_pass_through_evidence(&mgef_target, &by_sel).expect("evidence");
        assert_eq!(ev.detail["perk_to_apply"]["formid"], json!("0x2"));
        assert_eq!(ev.detail["equip_ability"]["formid"], json!("0x3"));
    }

    #[test]
    fn mgef_pass_through_evidence_returns_none_when_neither_field_set() {
        let mgef_target = json!({"formid": "0x1", "record_type": "MGEF"});
        let entry = ok_test_entry("0x1", json!({"Magic Effect Data": {"Data": {}}}));
        let by_sel: HashMap<&str, &BulkRecordEntry> = [("0x1", &entry)].into_iter().collect();
        assert!(mgef_pass_through_evidence(&mgef_target, &by_sel).is_none());
    }

    fn ok_test_entry(sel: &str, fields: Value) -> BulkRecordEntry {
        BulkRecordEntry {
            sel: sel.to_string(),
            header: None,
            editor_id: None,
            fields: Some(fields),
            error: None,
        }
    }
}
