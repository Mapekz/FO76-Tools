//! Native port of `tools/chase/chase.py` — automates the "chase pattern" for
//! unique-weapon OMOD effects documented under "How unique-weapon effects are
//! implemented (the chase pattern)" in `.claude/skills/patch-notes/mechanics-kb.md`.
//! Read that section first — this module is a mechanical implementation of
//! the walk it describes, nothing more.
//!
//! A `mod_Custom_*` (or similarly named) OMOD implements its unique mechanic
//! via one or more `Data.Properties[]` rows. Each row is classified exactly
//! the way the KB describes:
//!
//! 1. **Direct property** — the property's `Value 1` is either a plain number
//!    (a bare stat tweak, nothing to chase further) or a FormID pointing at
//!    an AVIF (an actor value — chased by reverse `refs` to find who reads
//!    it) or an ENCH/SPEL attached directly to the weapon (chased by a
//!    forward `get`, since the effect lives on that record, not behind a
//!    keyword gate).
//! 2. **Perk grant** — `Value 1` is a PERK (property 116/"Perks"). Chased by
//!    a forward `get` on the granted PERK — its `Effects` ARE the mechanic.
//! 3. **Keyword hook** — `Value 1` is a KYWD (property 31/"Keywords"). The
//!    keyword itself carries no behavior; chased by a reverse `refs --type
//!    SPEL,PERK --paths` walk to find the SPEL/PERK whose Conditions test
//!    `WornHasKeyword(<keyword>)`, then the exact `Effects[N]` entry gated by
//!    that condition (located via the `--paths` field path, not a full
//!    record dump).
//!
//! This is a 1:1 behavioral port of the Python prototype (`chase.py`), sharing
//! its output JSON shape so the /patch-notes deep-writer agent keeps working
//! unchanged. Unlike the prototype (which shells out to a warm daemon via
//! `EsmGateway`), this composes the same operations (`Op::RecordBulk`,
//! `Op::ReferencedBy`) in-process through the [`ChaseFetcher`] seam — no new
//! `Op` variant, no daemon round-trip required by the trait itself (the CLI's
//! concrete fetcher still goes through `Backend::run`, which may hit the warm
//! daemon, but the pure logic here doesn't know or care).

use crate::ipc::RecordSel;
use crate::{BulkRecordEntry, FormId, RefList, RefRow, ResolveDepth};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Record types whose Conditions are checked for a `WornHasKeyword` (or
/// similar) gate on a keyword/AVIF this OMOD ADDs. Mirrors the KB's "SPEL/PERK
/// effect conditioned on WornHasKeyword(...)" — these are the only two record
/// types the chase pattern names as mechanic carriers.
const CONSUMER_TYPES: [&str; 2] = ["SPEL", "PERK"];

/// Target record types whose own Effects/Description are pulled directly
/// (forward fetch) because the OMOD property attaches them straight to the
/// weapon rather than gating them behind a keyword condition.
const FORWARD_FETCH_TYPES: [&str; 3] = ["PERK", "ENCH", "SPEL"];

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
/// [`DEFAULT_REF_LIMIT`]).
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

/// The evidence tree returned by [`chase`] — same JSON shape as the Python
/// prototype's `chase()` return value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChaseTree {
    pub omod: OmodStub,
    pub hops: Vec<Hop>,
}

/// The chased OMOD's own identity — mirrors Python's `omod_stub` dict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmodStub {
    pub formid: Option<String>,
    pub editor_id: Option<String>,
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

/// One piece of evidence found for a hop — a forward-fetched record's own
/// Description/Effects, or a reverse-chased consumer's gated `Effects[N]`.
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

    let mut rows: Vec<RefRow> = Vec::new();
    for record_type in CONSUMER_TYPES {
        let ref_list = f.refs(target_fid, depth, limit, record_type, true)?;
        rows.extend(ref_list.rows);
    }
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

/// Run the full chase for one OMOD selector and return the evidence tree.
///
/// `f` is anything implementing [`ChaseFetcher`] — normally a
/// `Backend`-backed fetcher (see `cmd_chase` in `src/bin/cli.rs`), or a fake
/// for tests (see `tests/chase.rs`).
pub fn chase(
    f: &mut impl ChaseFetcher,
    omod: RecordSel,
    opts: &ChaseOptions,
) -> anyhow::Result<ChaseTree> {
    let omod_display = omod.display();
    let entries = f.bulk_get(std::slice::from_ref(&omod), ResolveDepth::Stub)?;
    let entry = entries
        .into_iter()
        .next()
        .with_context(|| format!("bulk_get returned no entries for {omod_display:?}"))?;
    if let Some(err) = &entry.error {
        bail!("failed to resolve {omod_display:?}: {err}");
    }

    let fields = entry.fields.clone().unwrap_or(Value::Null);
    let record_type = fields.get("_record_type").and_then(Value::as_str);
    if record_type != Some("Object Modification") {
        let got = record_type
            .map(str::to_string)
            .or_else(|| entry.header.as_ref().map(|h| h.signature.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        bail!(
            "{omod_display:?} resolves to a {got:?} record, not an OMOD \
             (Object Modification) — chase only supports OMOD input"
        );
    }

    let omod_stub = OmodStub {
        formid: entry.header.as_ref().map(|h| h.form_id.display()),
        editor_id: entry.editor_id.clone(),
        name: fields.get("Name").filter(|v| is_truthy(Some(*v))).cloned(),
        description: fields
            .get("Description")
            .filter(|v| is_truthy(Some(*v)))
            .cloned(),
    };

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
    }

    // ---- reverse chase (keyword_hook + AVIF consumer lookup) ----
    for (i, target) in &reverse_targets {
        hops[*i].evidence = reverse_chase(f, target, opts.depth, opts.ref_limit)?;
    }

    Ok(ChaseTree {
        omod: omod_stub,
        hops,
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

/// Render a [`ChaseTree`] as human-readable text — the CLI's default (non
/// `--json`) output. Ports `chase.py`'s `render_text` line-for-line.
pub fn render_text(tree: &ChaseTree) -> String {
    let mut lines: Vec<String> = Vec::new();
    let omod = &tree.omod;
    let mut header = format!(
        "OMOD {} {}",
        omod.formid.as_deref().unwrap_or("None"),
        omod.editor_id.as_deref().unwrap_or("None"),
    );
    if is_truthy(omod.name.as_ref()) {
        header.push_str(&format!("  \"{}\"", pyish(omod.name.as_ref().unwrap())));
    }
    lines.push(header);
    if is_truthy(omod.description.as_ref()) {
        lines.push(format!(
            "  Description: \"{}\"",
            pyish(omod.description.as_ref().unwrap())
        ));
    }

    if tree.hops.is_empty() {
        lines.push("  (no Properties on this OMOD — nothing to chase)".to_string());
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
            if let Some(note) = detail.get("note") {
                lines.push(format!("         Note: {}", pyish(note)));
            }
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
}
