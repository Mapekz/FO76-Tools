//! The sole interactive record surface (see
//! `docs/adr/0001-walk-interactive-chase-pipeline-json.md`): prints one
//! compact indented digest of an ESM record and the chain it references,
//! instead of a series of raw `esm get` dumps. Accepts any record type.
//!
//! ```text
//! esm --esm <path> walk <formid|edid> [--refs] [--depth N] [--ref-limit N] [--json]
//! ```
//!
//! [`walk`] does a breadth-first traversal (queue + visited set keyed on
//! FormID, depth-capped) starting from one resolved record, printing a
//! record-type-specific digest for each node and enqueueing whatever it
//! references that the TS spec calls out as worth following one hop further
//! (a magic effect's granted Perk/Equip Ability, a PERK's Ability SPEL, an
//! OMOD's ENCH property, ...). It composes the same two primitives
//! `esm::chase`'s "chase pattern" uses — [`ChaseFetcher::bulk_get`] and
//! [`ChaseFetcher::refs`] — no new trait, no new wire `Op`.
//!
//! **OMOD roots get more than a forward-only digest.** [`digest_node`]'s
//! `"OMOD"` arm calls straight into `esm::chase`'s mechanism classifier
//! ([`crate::chase::omod_chase`]) on the already-fetched root — no redundant
//! re-fetch — and renders each classified `Data.Properties[]` mechanism as
//! path-sliced evidence rows nested under the record's digest: a keyword/AVIF
//! hook names the gating SPEL/PERK consumer and shows only the gated
//! `Effects[N]` rows it triggers (never the consumer's full digest), a perk
//! grant or direct SPEL/ENCH attachment shows the granted record's own
//! effects, and an MGEF pass-through ("Perk to Apply"/"Equip Ability") is
//! named the same way `chase` names it. This mechanism slice runs
//! unconditionally as part of the root digest — `--depth` only governs BFS
//! *enqueueing*, not whether the root's own mechanisms get classified.
//! ENCH-typed properties are the one exception: they're still handled
//! entirely by [`digest_omod_ench_follow`] (forward-fetched *and* enqueued
//! into the BFS as their own node), not by the chase classifier, so an
//! OMOD → ENCH → MGEF → granted-perk chain still lands in one `walk` call.
//! `Data.Includes[]` stubs (the `_PARENT_*` empty-shell OMOD pattern) are
//! named too, straight off the already-stub-resolved fields — zero extra
//! fetches.
//!
//! Every record is fetched at [`ResolveDepth::Stub`], so every direct FormID
//! reference on a fetched record's own fields already arrives pre-annotated
//! as `{"formid", "editor_id", "record_type"}` (the same annotation
//! `esm get --resolve stub` produces) — this is a deliberate improvement over
//! the TS original, which shells out to a plain `esm get` (no `--resolve`)
//! and resolves each reference with its own follow-up `client.get()` call
//! (`ref()`/`resolveEdid()`). One exception remains: a GLOB *reference*'s own
//! `Value` field isn't expanded by Stub resolution (which only annotates the
//! *reference*, not the referenced record's fields), so magnitude/duration/
//! condition GLOB annotations still require one batched extra `bulk_get` —
//! mirroring the TS original's `globValue()`.
//!
//! Two things the TS original does that this module deliberately leaves to
//! its caller (`cmd_walk` in `src/bin/cli.rs`), since neither fits through
//! `ChaseFetcher`'s narrow bulk_get/refs-with-type-filter seam:
//! - **not-found → search fallback** (TS `walk()` lines 307-316): when the
//!   root selector doesn't resolve, [`walk`] returns a [`WalkResult`] with
//!   [`WalkResult::not_found`] set and an empty `matches` list; the CLI
//!   driver runs one `Op::Search` and fills `matches` in before rendering.
//! - **`--refs` reverse-reference summary**: needs an *unfiltered* reverse
//!   `refs` walk (every referencing record type, not just SPEL/PERK), which
//!   `ChaseFetcher::refs`'s mandatory type-filter parameter can't express.
//!   The CLI driver runs one unfiltered `Op::ReferencedBy` call and passes
//!   the raw rows to [`build_refs_digest`] (a pure function, easily unit
//!   tested without any fetcher).

use crate::chase::{
    ChaseFetcher, ChaseOptions, Evidence, Hop, HopKind, RootStub, consumer_refs_by_type,
    first_array_container, fmt_stub, omod_chase, summarize_effect, summarize_explosion_detail,
};
use crate::ipc::RecordSel;
use crate::{BulkRecordEntry, FormId, RecordRow, RefRow, ResolveDepth};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// Default BFS depth cap (mirrors the TS original's `depth = 2` default).
pub const DEFAULT_DEPTH: usize = 2;

/// Reverse-ref walk depth/cap for the KYWD/AVIF "who gates on this?" digest
/// (mirrors the TS original's `client.refs(formId, { depth: 1, type, paths:
/// true })`; only the top 10 rows per consumer type are ever printed, so a
/// small limit is enough — `RefList::total` (computed before truncation)
/// still gives an accurate "+N more" count).
const CONSUMER_REF_DEPTH: usize = 1;
const CONSUMER_REF_LIMIT: usize = 10;
const CONSUMER_ROWS_SHOWN: usize = 10;

/// Record types whose direct references to a target count as a "player-facing
/// signal" in [`build_refs_digest`] (mirrors the TS original's
/// `OBTAINABLE_TYPES`).
const OBTAINABLE_TYPES: [&str; 7] = ["COBJ", "GMRW", "LGDI", "QUST", "CONT", "MISC", "FLST"];

/// Model/render/sound noise that never matters for damage or obtainability —
/// dropped from the generic fallback digest (mirrors the TS original's
/// `GENERIC_NOISE_KEYS`).
const GENERIC_NOISE_KEYS: &[&str] = &[
    "Object Bounds",
    "Model",
    "Preview Transform",
    "Sound Level",
    "Sounds",
    "Sound",
    "Pickup Sound",
    "Putdown Sound",
    "Icon",
    "Message Icon",
    "Transform",
    "Animation Sound",
];

/// Cap on pretty-printed lines in the generic fallback digest before a
/// truncation trailer is emitted (mirrors the TS original's `MAX = 120`).
const GENERIC_DUMP_MAX_LINES: usize = 120;

/// Cap on candidate FormIDs scanned for an OMOD's ENCH-typed properties
/// (mirrors the TS original's `formIds.slice(0, 25)`).
const OMOD_ENCH_CANDIDATE_CAP: usize = 25;

/// Cap on `Data.Includes[]` targets enqueued into the walk BFS per OMOD node.
/// Shares [`crate::chase::OMOD_INCLUDE_ENQUEUE_CAP`] (corpus peak 79 on one
/// selector; 20 covers the overwhelming majority while bounding BFS breadth).
const OMOD_INCLUDE_ENQUEUE_CAP: usize = crate::chase::OMOD_INCLUDE_ENQUEUE_CAP;

/// A digest function's request to enqueue one more hop: the target FormID
/// plus the "via" edge label to attach to its [`WalkNode`] once visited.
type EnqueueTarget = (FormId, String);

// ─── options / result ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WalkOptions {
    /// BFS depth cap — nodes reached only via a chain longer than this are
    /// never fetched. 0 means "just the root, no enqueueing". Only governs
    /// BFS enqueueing — an OMOD root's inline mechanism slice (see
    /// `digest_omod_mechanisms`) always runs regardless of this cap.
    pub depth: usize,
    /// Cap on refs rows fetched per record-type filter, passed straight
    /// through to `esm::chase::ChaseOptions::ref_limit` for an OMOD root's
    /// keyword/AVIF mechanism consumer lookups (see
    /// `digest_omod_mechanisms`). Shares `esm::chase::DEFAULT_REF_LIMIT`
    /// rather than a duplicated constant. Unrelated to
    /// [`CONSUMER_REF_LIMIT`], which bounds a *directly*-walked KYWD/AVIF
    /// root's own consumer digest and is deliberately left as-is.
    pub ref_limit: usize,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            ref_limit: crate::chase::DEFAULT_REF_LIMIT,
        }
    }
}

/// The walk's output: either a not-found report (root selector didn't
/// resolve) or the BFS node list, plus an optional `--refs` digest for the
/// root. Kept as a flat struct with skip-if-empty/None fields (rather than an
/// enum) so `--json` output stays a single flat object — see `esm::chase`'s
/// `ChaseTree` for the same convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_found: Option<NotFound>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<WalkNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refs: Option<RefsDigest>,
}

/// Set instead of `nodes` when the root selector's initial `bulk_get` came
/// back with an error entry. `matches` starts empty — [`walk`] itself never
/// searches (see module docs); the CLI driver fills it in via one `Op::Search`
/// call before rendering/serializing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotFound {
    pub target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<RecordRow>,
}

/// One BFS-visited record: identity plus its record-type-specific digest
/// lines. `digest` lines are pre-indented relative to the node header (an
/// extra two leading spaces = one nesting level deeper, matching the TS
/// original's `emit(2, ...)` sub-bullets) — see [`render_text`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkNode {
    pub depth: usize,
    pub sig: String,
    pub formid: String,
    pub editor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub digest: Vec<String>,
}

/// One record-type group in the `--refs` summary (see [`build_refs_digest`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefsDigestGroup {
    pub record_type: String,
    pub count: usize,
    /// Up to 5 sample EditorIDs, each with `" ⚠NONPLAYABLE"` appended when the
    /// EditorID itself contains that substring (case-insensitive).
    pub sample: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefsDigest {
    pub groups: Vec<RefsDigestGroup>,
}

// ─── generic JSON helpers ───────────────────────────────────────────────────

/// Render a JSON value the way the TS original's template-literal
/// interpolation would (unquoted strings, `None`/`True`/`False` for
/// null/bool), with one deliberate improvement: whole-number floats print
/// without a trailing `.0` (matching JS's own number-to-string behavior,
/// which the TS original relies on implicitly) rather than Rust's
/// `serde_json::Number::to_string()`, which always keeps the decimal point.
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
        Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && f.is_finite()
                && f.fract() == 0.0
                && f.abs() < 1e15
            {
                return format!("{}", f as i64);
            }
            n.to_string()
        }
        other => other.to_string(),
    }
}

fn pyish_opt(v: Option<&Value>) -> String {
    v.map(pyish).unwrap_or_else(|| "?".to_string())
}

/// A decoded FormID reference at [`ResolveDepth::Stub`] is a
/// `{"formid", "editor_id", "record_type"}` object (see module docs).
fn is_ref_stub(v: &Value) -> bool {
    matches!(v, Value::Object(map) if map.contains_key("formid"))
}

fn stub_formid(v: Option<&Value>) -> Option<FormId> {
    let obj = v?.as_object()?;
    let s = obj.get("formid")?.as_str()?;
    crate::parse_form_id_input(s).ok()
}

/// "0xID EditorID" — the universal reference rendering (mirrors the TS
/// original's `ref()`, minus the extra round-trip: Stub resolution already
/// annotated `v` when its enclosing record was fetched).
fn fmt_ref(v: &Value) -> String {
    match v.as_object() {
        Some(obj) if obj.contains_key("formid") => {
            let fid = obj.get("formid").and_then(Value::as_str).unwrap_or("?");
            let edid = obj.get("editor_id").and_then(Value::as_str).unwrap_or("");
            if edid.is_empty() {
                fid.to_string()
            } else {
                format!("{fid} {edid}")
            }
        }
        _ => pyish(v),
    }
}

/// Recursively collect every FormID-reference-stub found anywhere in `v`
/// (object values keyed `"formid"`), deduped by insertion order. Used to
/// batch-prefetch GLOB targets referenced anywhere inside a Conditions
/// subtree, and as the OMOD ENCH-follow fallback scan (module docs).
fn collect_ref_formids(v: &Value, out: &mut Vec<FormId>) {
    match v {
        Value::Object(map) => {
            if let Some(fid) = stub_formid(Some(v)) {
                out.push(fid);
            }
            for val in map.values() {
                collect_ref_formids(val, out);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_ref_formids(item, out);
            }
        }
        _ => {}
    }
}

fn dedup_sorted(fids: &mut Vec<FormId>) {
    fids.sort_by_key(|f| f.0);
    fids.dedup();
}

/// Batch-fetch `fids` at [`ResolveDepth::Stub`] and return them keyed by their
/// display-formid string (matches `BulkRecordEntry::sel` for a
/// `RecordSel::FormId` selector — see `esm::chase`'s identical `by_sel`
/// pattern).
fn bulk_fetch_map(
    f: &mut impl ChaseFetcher,
    fids: &[FormId],
) -> anyhow::Result<HashMap<String, BulkRecordEntry>> {
    if fids.is_empty() {
        return Ok(HashMap::new());
    }
    let sels: Vec<RecordSel> = fids.iter().map(|fid| RecordSel::FormId(*fid)).collect();
    let entries = f.bulk_get(&sels, ResolveDepth::Stub)?;
    Ok(entries.into_iter().map(|e| (e.sel.clone(), e)).collect())
}

/// "EditorID=Value" — the magnitude/duration GLOB annotation (mirrors the TS
/// original's `globValue()`; no leading hex, unlike [`glob_inline_annotation`]).
fn glob_edid_value(by_sel: &HashMap<String, BulkRecordEntry>, stub: &Value) -> String {
    let Some(obj) = stub.as_object() else {
        return "?".to_string();
    };
    let fid = obj.get("formid").and_then(Value::as_str).unwrap_or("?");
    let edid = obj.get("editor_id").and_then(Value::as_str).unwrap_or("?");
    let value = by_sel
        .get(fid)
        .and_then(|e| e.fields.as_ref())
        .and_then(|flds| flds.get("Value"))
        .map(pyish)
        .unwrap_or_else(|| "?".to_string());
    format!("{edid}={value}")
}

/// "0xID<EditorID[=Value]>" — the inline condition-operand GLOB annotation
/// (mirrors the TS original's `fmtConditionsResolved`, which regex-replaces
/// every `0x...` occurrence in a rendered condition line the same way,
/// appending `=value` only for a GLOB target).
fn glob_inline_annotation(
    by_sel: &HashMap<String, BulkRecordEntry>,
    stub_obj: &serde_json::Map<String, Value>,
) -> String {
    let fid = stub_obj
        .get("formid")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let edid = stub_obj
        .get("editor_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_glob = stub_obj.get("record_type").and_then(Value::as_str) == Some("GLOB");
    if is_glob {
        let value = by_sel
            .get(fid)
            .and_then(|e| e.fields.as_ref())
            .and_then(|flds| flds.get("Value"))
            .map(pyish);
        match value {
            Some(v) => format!("{fid}<{edid}={v}>"),
            None => format!("{fid}<{edid}>"),
        }
    } else {
        format!("{fid}<{edid}>")
    }
}

/// "(x,y)(x,y)...  [curve_path]" — curve points are already decoded onto the
/// `Curve Table` field regardless of resolve depth (see [`crate::decode`]'s
/// CURV branch); this just reads them back out.
fn fmt_curve(v: &Value) -> Option<String> {
    let points = v.get("curve")?.as_array()?;
    if points.is_empty() {
        return None;
    }
    let pts: String = points
        .iter()
        .map(|p| {
            let x = p.get("x").map(pyish).unwrap_or_default();
            let y = p.get("y").map(pyish).unwrap_or_default();
            format!("({x},{y})")
        })
        .collect();
    match v.get("curve_path").and_then(Value::as_str) {
        Some(path) => Some(format!("{pts}  [{path}]")),
        None => Some(pts),
    }
}

// ─── conditions ─────────────────────────────────────────────────────────────

/// Pull the flat condition rows out of a SPEL/ENCH/ALCH/MGEF-style
/// `Conditions` node (mirrors the TS original's `flattenConditionRows`).
fn flatten_condition_rows(node: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(conditions) = node.get("Conditions").and_then(Value::as_array) else {
        return out;
    };
    for item in conditions {
        if let Some(data) = item.pointer("/Condition/Condition Data") {
            out.push(data.clone());
        }
    }
    out
}

/// Flatten a PERK "Perk Conditions" node (tabbed) into raw condition rows.
/// Tab-index 2 conditions run on the target, so their `Run On` is forced to
/// `"Target"` (mirrors the TS original's `flattenPerkConditionRows`).
fn flatten_perk_condition_rows(node: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(tabs) = node.as_array() else {
        return out;
    };
    for tab in tabs {
        let Some(pc) = tab.get("Perk Condition") else {
            continue;
        };
        let tab_index = pc
            .get("Run On (Tab Index)")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(conditions) = pc.get("Conditions").and_then(Value::as_array) else {
            continue;
        };
        for item in conditions {
            let Some(data) = item.pointer("/Condition/Condition Data") else {
                continue;
            };
            let mut row = data.clone();
            if tab_index == 2
                && let Value::Object(map) = &mut row
            {
                map.insert("Run On".to_string(), Value::String("Target".to_string()));
            }
            out.push(row);
        }
    }
    out
}

fn fmt_condition_operand(v: Option<&Value>, by_sel: &HashMap<String, BulkRecordEntry>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::Object(map)) if map.contains_key("formid") => {
            glob_inline_annotation(by_sel, map)
        }
        Some(other) => pyish(other),
    }
}

/// `Function(Param1) Operator ComparisonValue[ on RunOn][ [OR]]` — the
/// condition line format (mirrors the TS original's `fmtConditions` +
/// `fmtConditionsResolved` combined into one pass, since Stub resolution
/// already annotated any reference operand).
fn fmt_condition_row(row: &Value, by_sel: &HashMap<String, BulkRecordEntry>) -> String {
    let function = row.get("Function").and_then(Value::as_str).unwrap_or("?");
    let operator = row.get("Operator").and_then(Value::as_str).unwrap_or("==");
    let param1 = fmt_condition_operand(row.get("Parameter 1"), by_sel);
    let cmp = fmt_condition_operand(row.get("Comparison Value"), by_sel);
    let mut out = format!("{function}({param1}) {operator} {cmp}");
    if let Some(run_on) = row.get("Run On").and_then(Value::as_str)
        && run_on != "Subject"
    {
        out.push_str(&format!(" on {run_on}"));
    }
    if row.get("AND/OR").and_then(Value::as_str) == Some("OR") {
        out.push_str(" [OR]");
    }
    out
}

/// Collect every GLOB/other FormID reference nested inside `conditions_node`
/// so callers can batch-prefetch GLOB `Value`s before rendering.
fn collect_condition_refs(conditions_node: &Value, out: &mut Vec<FormId>) {
    collect_ref_formids(conditions_node, out);
}

// ─── per-type digests ───────────────────────────────────────────────────────

fn digest_glob(fields: &Value, lines: &mut Vec<String>) {
    lines.push(format!("value {}", pyish_opt(fields.get("Value"))));
}

fn digest_avif(fields: &Value, lines: &mut Vec<String>) {
    let abbrev = fields
        .get("Abbreviation")
        .and_then(Value::as_str)
        .unwrap_or("—");
    lines.push(format!(
        "abbrev {abbrev}  default {}  max {}",
        pyish_opt(fields.get("Default Value")),
        pyish_opt(fields.get("Maximum Value")),
    ));
}

/// KYWD/AVIF records carry no behavior themselves — they're read by whichever
/// SPEL/PERK gates an effect on them. Reverse-`refs --type ... --paths` finds
/// those consumers and the exact field path each one gates through (mirrors
/// the TS original's `digestKeywordOrAv`).
fn digest_keyword_or_av(
    f: &mut impl ChaseFetcher,
    formid: FormId,
    lines: &mut Vec<String>,
) -> anyhow::Result<()> {
    let grouped = consumer_refs_by_type(f, formid, CONSUMER_REF_DEPTH, CONSUMER_REF_LIMIT)?;
    for (record_type, ref_list) in grouped {
        if ref_list.rows.is_empty() {
            continue;
        }
        lines.push(format!("{record_type} consumers (gate on this):"));
        for r in ref_list.rows.iter().take(CONSUMER_ROWS_SHOWN) {
            let path = r.field_paths.as_ref().and_then(|p| p.first());
            let via = path.map(|p| format!("  via {p}")).unwrap_or_default();
            lines.push(format!(
                "  {} {}{via}",
                r.form_id,
                r.editor_id.as_deref().unwrap_or("")
            ));
        }
        if ref_list.total > CONSUMER_ROWS_SHOWN {
            lines.push(format!(
                "  … +{} more",
                ref_list.total - CONSUMER_ROWS_SHOWN
            ));
        }
    }
    Ok(())
}

struct MgefSummary<'v> {
    archetype: Option<&'v str>,
    casting_type: Option<&'v str>,
    actor_value: Option<&'v Value>,
    resist_value: Option<&'v Value>,
    perk_to_apply: Option<&'v Value>,
    equip_ability: Option<&'v Value>,
    description: Option<&'v Value>,
}

/// Pull the handful of fields both [`digest_mgef`] (a directly-visited MGEF
/// node) and [`digest_magic_item`] (an MGEF reached via a SPEL/ENCH/ALCH
/// effect's `Base Effect`) need out of an MGEF record's own decoded fields.
fn mgef_summary(fields: &Value) -> MgefSummary<'_> {
    let data = fields.pointer("/Magic Effect Data/Data");
    let get = |key: &str| data.and_then(|d| d.get(key));
    MgefSummary {
        archetype: data
            .and_then(|d| d.pointer("/Archetype/name"))
            .and_then(Value::as_str),
        casting_type: data
            .and_then(|d| d.pointer("/Casting Type/name"))
            .and_then(Value::as_str),
        actor_value: get("Actor Value").filter(|v| is_ref_stub(v)),
        resist_value: get("Resist Value").filter(|v| is_ref_stub(v)),
        perk_to_apply: get("Perk to Apply").filter(|v| is_ref_stub(v)),
        equip_ability: get("Equip Ability").filter(|v| is_ref_stub(v)),
        description: fields
            .get("Magic Item Description")
            .filter(|v| !v.is_null()),
    }
}

fn digest_mgef(fields: &Value, lines: &mut Vec<String>, enqueue: &mut Vec<EnqueueTarget>) {
    let summary = mgef_summary(fields);
    lines.push(format!(
        "archetype {}  casting {}",
        summary.archetype.unwrap_or("?"),
        summary.casting_type.unwrap_or("?"),
    ));
    if let Some(av) = summary.actor_value {
        lines.push(format!("target AV {}", fmt_ref(av)));
    }
    if let Some(rv) = summary.resist_value {
        lines.push(format!(
            "resist AV {} (element carrier for Damage archetype)",
            fmt_ref(rv)
        ));
    }
    if let Some(p) = summary.perk_to_apply {
        lines.push(format!("Perk to Apply → {}", fmt_ref(p)));
        if let Some(fid) = stub_formid(Some(p)) {
            enqueue.push((fid, "Perk to Apply".to_string()));
        }
    }
    if let Some(eq) = summary.equip_ability {
        lines.push(format!("Equip Ability → {}", fmt_ref(eq)));
        if let Some(fid) = stub_formid(Some(eq)) {
            enqueue.push((fid, "Equip Ability".to_string()));
        }
    }
    if let Some(desc) = summary.description.and_then(Value::as_str)
        && !desc.is_empty()
    {
        lines.push(format!("description \"{desc}\""));
    }
}

/// SPEL/ENCH/ALCH share an identical `Effects[]` shape: per effect, a `Base
/// Effect` -> MGEF, a flat `Effect Item Data.Magnitude`/`.Duration`, an
/// optional sibling GLOB `Magnitude`/`Duration`, an optional `Curve Table` +
/// `Actor Value` input axis, `Conditions`, and the MGEF's own one-hop
/// `Perk to Apply`/`Equip Ability` (mirrors the TS original's
/// `digestMagicItem`).
fn digest_magic_item(
    f: &mut impl ChaseFetcher,
    fields: &Value,
    lines: &mut Vec<String>,
    enqueue: &mut Vec<EnqueueTarget>,
) -> anyhow::Result<()> {
    let empty = Vec::new();
    let effects = fields
        .get("Effects")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if effects.is_empty() {
        return Ok(());
    }

    // One batched bulk_get for every MGEF (Base Effect) + GLOB (Magnitude/
    // Duration/condition-operand) reference across all effects.
    let mut want: Vec<FormId> = Vec::new();
    for item in effects {
        let Some(e) = item.get("Effect") else {
            continue;
        };
        if let Some(fid) = stub_formid(e.get("Base Effect")) {
            want.push(fid);
        }
        if let Some(fid) = stub_formid(e.get("Magnitude")) {
            want.push(fid);
        }
        if let Some(fid) = stub_formid(e.get("Duration")) {
            want.push(fid);
        }
        if let Some(cond) = e.get("Conditions") {
            collect_condition_refs(cond, &mut want);
        }
    }
    dedup_sorted(&mut want);
    let by_sel = bulk_fetch_map(f, &want)?;

    for (i, item) in effects.iter().enumerate() {
        let Some(e) = item.get("Effect") else {
            continue;
        };
        let base_effect = e.get("Base Effect");
        let mgef_fields = base_effect
            .and_then(|b| stub_formid(Some(b)))
            .and_then(|fid| by_sel.get(&fid.display()))
            .and_then(|entry| entry.fields.as_ref());
        let summary = mgef_fields.map(mgef_summary);
        let archetype = summary.as_ref().and_then(|s| s.archetype).unwrap_or("?");
        let av_part = summary
            .as_ref()
            .and_then(|s| s.actor_value)
            .map(|v| format!(", AV {}", fmt_ref(v)))
            .unwrap_or_default();
        lines.push(format!(
            "effect[{i}] → MGEF {} ({archetype}{av_part})",
            base_effect
                .map(fmt_ref)
                .unwrap_or_else(|| "None".to_string())
        ));

        let item_data = e.get("Effect Item Data");
        let flat_mag = item_data
            .and_then(|d| d.get("Magnitude"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let mag_display = item_data
            .and_then(|d| d.get("Magnitude"))
            .map(pyish)
            .unwrap_or_else(|| "0".to_string());
        let dur_display = item_data
            .and_then(|d| d.get("Duration"))
            .map(pyish)
            .unwrap_or_else(|| "0".to_string());
        lines.push(format!("  magnitude {mag_display}  duration {dur_display}"));

        if let Some(mag_ref) = e.get("Magnitude").filter(|v| is_ref_stub(v)) {
            let g = glob_edid_value(&by_sel, mag_ref);
            if flat_mag == 0.0 {
                lines.push(format!("  magnitude GLOB {g}  ← real value (flat is 0)"));
            } else {
                lines.push(format!(
                    "  sibling Magnitude GLOB {g}  ← IGNORE (flat wins; survival scale const)"
                ));
            }
        }
        if let Some(dur_ref) = e.get("Duration").filter(|v| is_ref_stub(v)) {
            lines.push(format!(
                "  duration GLOB {}",
                glob_edid_value(&by_sel, dur_ref)
            ));
        }

        if let Some(curve_str) = e.get("Curve Table").and_then(fmt_curve) {
            lines.push(format!("  curve {curve_str}"));
            if let Some(av) = e.get("Actor Value").filter(|v| is_ref_stub(v)) {
                lines.push(format!("  curve INPUT axis: AV {}", fmt_ref(av)));
            }
        }

        if let Some(cond) = e.get("Conditions") {
            for row in flatten_condition_rows(cond) {
                lines.push(format!("  cond: {}", fmt_condition_row(&row, &by_sel)));
            }
        }

        if let Some(perk) = summary.as_ref().and_then(|s| s.perk_to_apply) {
            lines.push(format!("  Perk to Apply → {}", fmt_ref(perk)));
            if let Some(fid) = stub_formid(Some(perk)) {
                enqueue.push((fid, "Perk to Apply".to_string()));
            }
        }
        if let Some(eq) = summary.as_ref().and_then(|s| s.equip_ability) {
            lines.push(format!("  Equip Ability → {}", fmt_ref(eq)));
            if let Some(fid) = stub_formid(Some(eq)) {
                enqueue.push((fid, "Equip Ability".to_string()));
            }
        }
    }
    Ok(())
}

/// PERK: description; ranks/playable/next; per-effect Ability (enqueue) or
/// Entry Point (fn/value/AV + perk conditions), or `NO effects` when the
/// bonus is engine/script-side (mirrors the TS original's `digestPerk`). The
/// TS original's `repairMisattributedPerkEntryFields` shim is deliberately
/// NOT ported — the decode bug it worked around is already fixed upstream
/// (commit 4031d96) and the shim itself is a verified no-op there.
fn digest_perk(
    f: &mut impl ChaseFetcher,
    fields: &Value,
    lines: &mut Vec<String>,
    enqueue: &mut Vec<EnqueueTarget>,
) -> anyhow::Result<()> {
    let data = fields.get("Data");
    if let Some(desc) = fields.get("Description").and_then(Value::as_str)
        && !desc.is_empty()
    {
        lines.push(format!("description \"{desc}\""));
    }
    let playable = data
        .and_then(|d| d.pointer("/Playable/name"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let mut header = format!(
        "ranks {}  playable {playable}",
        pyish_opt(data.and_then(|d| d.get("Num Ranks"))),
    );
    if let Some(next) = fields.get("Next Perk").filter(|v| is_ref_stub(v)) {
        header.push_str(&format!("  next → {}", fmt_ref(next)));
    }
    lines.push(header);

    let Some(effects) = fields.get("Effects").and_then(Value::as_array) else {
        lines.push("NO effects — bonus is engine/script-side (description only)".to_string());
        return Ok(());
    };

    // Batch-fetch every GLOB referenced by any effect's Perk Conditions.
    let mut want: Vec<FormId> = Vec::new();
    for item in effects {
        if let Some(pc) = item.pointer("/Effect/Perk Conditions") {
            collect_condition_refs(pc, &mut want);
        }
    }
    dedup_sorted(&mut want);
    let by_sel = bulk_fetch_map(f, &want)?;

    for (i, item) in effects.iter().enumerate() {
        let Some(e) = item.get("Effect") else {
            continue;
        };
        let type_name = e
            .pointer("/Effect Header/Effect Type/name")
            .and_then(Value::as_str)
            .unwrap_or("?");
        match type_name {
            "Ability" => {
                if let Some(ability) = e.get("Ability") {
                    lines.push(format!("effect[{i}] Ability → SPEL {}", fmt_ref(ability)));
                    if let Some(fid) = stub_formid(Some(ability)) {
                        enqueue.push((fid, "Ability".to_string()));
                    }
                }
            }
            "Entry Point" => {
                let ep = e.get("Entry Point");
                let ep_name = ep
                    .and_then(|v| v.pointer("/Entry Point/name"))
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let fn_name = ep
                    .and_then(|v| v.pointer("/Function/name"))
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let mut l = format!("effect[{i}] Entry Point \"{ep_name}\"  fn {fn_name}");
                if let Some(float_field) = e.get("Float").filter(|v| v.as_f64().is_some()) {
                    l.push_str(&format!("  value {}", pyish(float_field)));
                }
                if let Some(av) = e
                    .get("Function Parameter 3 (Actor Value)")
                    .filter(|v| is_ref_stub(v))
                {
                    l.push_str(&format!("  AV {}", fmt_ref(av)));
                }
                lines.push(l);
                if let Some(pc) = e.get("Perk Conditions") {
                    for row in flatten_perk_condition_rows(pc) {
                        lines.push(format!("  cond: {}", fmt_condition_row(&row, &by_sel)));
                    }
                }
            }
            other => lines.push(format!("effect[{i}] {other}")),
        }
    }
    Ok(())
}

fn digest_weap(fields: &Value, lines: &mut Vec<String>) {
    let data = fields.get("Data");
    let keyword_ids = fields
        .pointer("/Keywords/Keywords")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut kw_names = Vec::new();
    for k in &keyword_ids {
        if let Some(edid) = k.get("editor_id").and_then(Value::as_str)
            && (edid.starts_with("WeaponType")
                || edid.starts_with("HasLegendary")
                || edid.starts_with("ma_"))
        {
            kw_names.push(edid.to_string());
        }
    }
    lines.push(format!(
        "keywords: {}",
        if kw_names.is_empty() {
            "(none damage-relevant)".to_string()
        } else {
            kw_names.join(", ")
        }
    ));
    lines.push(format!(
        "apCost {}  speed {}  reloadSpeed {}",
        pyish_opt(data.and_then(|d| d.get("Action Point Cost"))),
        pyish_opt(data.and_then(|d| d.get("Speed"))),
        pyish_opt(data.and_then(|d| d.get("Reload Speed"))),
    ));
    let levels: Vec<String> = fields
        .get("Eligible Levels")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(pyish).collect())
        .unwrap_or_default();
    let attach_slots = fields
        .get("Attach Parent Slots")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    lines.push(format!(
        "eligible levels: {}  attach slots: {attach_slots}",
        if levels.is_empty() {
            "—".to_string()
        } else {
            levels.join(",")
        }
    ));
    if fields.get("Object Template").is_some_and(|v| !v.is_null()) {
        lines.push(
            "has Object Template (instance-template mods = POSSIBLE loadouts, never auto-apply)"
                .to_string(),
        );
    }
}

fn is_generic_noise_value(v: &Value) -> bool {
    matches!(v, Value::Null) || matches!(v, Value::String(s) if s.is_empty())
}

fn has_raw_marker(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.contains_key("_raw"))
}

/// Trimmed field dump for any record type without a dedicated digest: drop
/// null/empty values, the `Unknown`/`_record_type`/`Editor ID` keys, `_raw`-
/// bearing objects, and [`GENERIC_NOISE_KEYS`] (mirrors the TS original's
/// `digestGeneric`). Every FormID reference is already annotated by the
/// Stub-resolved fetch (rather than the TS original's own per-reference
/// `ref()` round-trip, bounded to the first ~40 to cap round-trips) — flattened
/// here to the same one-line `"0xID EditorID"` string TS's `ref()` produces,
/// rather than emitting the full `{"formid","editor_id","record_type"}`
/// object, so a reference-heavy record doesn't burn 3+ lines of the
/// [`GENERIC_DUMP_MAX_LINES`] budget per reference where TS spends one.
fn trim_generic_fields(fields: &Value) -> Value {
    if is_ref_stub(fields) {
        return Value::String(fmt_ref(fields));
    }
    match fields {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if k == "_record_type" || k == "Editor ID" || k == "Unknown" {
                    continue;
                }
                if GENERIC_NOISE_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if is_generic_noise_value(v) || has_raw_marker(v) {
                    continue;
                }
                out.insert(k.clone(), trim_generic_fields(v));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(trim_generic_fields).collect()),
        other => other.clone(),
    }
}

fn digest_generic(fields: &Value, lines: &mut Vec<String>) {
    let trimmed = trim_generic_fields(fields);
    let dump = serde_json::to_string_pretty(&trimmed).unwrap_or_default();
    let dump_lines: Vec<&str> = dump.lines().collect();
    for l in dump_lines.iter().take(GENERIC_DUMP_MAX_LINES) {
        lines.push((*l).to_string());
    }
    if dump_lines.len() > GENERIC_DUMP_MAX_LINES {
        lines.push(format!(
            "… {} more lines (use `esm get` for the full record)",
            dump_lines.len() - GENERIC_DUMP_MAX_LINES
        ));
    }
}

/// OMODs carry their payload in ENCH-typed properties — follow them so the
/// full OMOD → ENCH → MGEF → granted-perk chain lands in one invocation
/// (mirrors the TS original's OMOD post-processing step). Prefers the typed
/// `Data.Properties[].Value 1` stubs; falls back to a recursive scan of the
/// whole fields tree for formid-stub objects if that yields nothing, mirroring
/// the TS original's coverage (a regex scan of `JSON.stringify` — expressed
/// here as a stub-object scan, since Stub resolution turns references into
/// objects rather than raw hex strings).
fn digest_omod_ench_follow(
    f: &mut impl ChaseFetcher,
    fields: &Value,
    lines: &mut Vec<String>,
    enqueue: &mut Vec<EnqueueTarget>,
) -> anyhow::Result<()> {
    let mut candidates: Vec<FormId> = Vec::new();
    if let Some(props) = fields.pointer("/Data/Properties").and_then(Value::as_array) {
        for p in props {
            if let Some(fid) = stub_formid(p.get("Value 1")) {
                candidates.push(fid);
            }
        }
    }
    if candidates.is_empty() {
        collect_ref_formids(fields, &mut candidates);
    }
    dedup_sorted(&mut candidates);
    candidates.truncate(OMOD_ENCH_CANDIDATE_CAP);
    if candidates.is_empty() {
        return Ok(());
    }

    let by_sel = bulk_fetch_map(f, &candidates)?;
    for fid in &candidates {
        let Some(entry) = by_sel.get(&fid.display()) else {
            continue;
        };
        let is_ench = entry
            .header
            .as_ref()
            .map(|h| h.signature.as_str())
            .unwrap_or("")
            == "ENCH";
        if is_ench {
            let edid = entry.editor_id.clone().unwrap_or_default();
            lines.push(format!("enchantment → {} {edid}", fid.display()));
            enqueue.push((*fid, "OMOD property".to_string()));
        }
    }
    Ok(())
}

/// Classify an OMOD root's `Data.Properties[]` rows via `esm::chase`'s
/// mechanism classifier ([`omod_chase`]) and render each non-bare-number,
/// non-ENCH mechanism as path-sliced evidence rows nested under the record's
/// digest (see module docs). Runs on the already-fetched root — `fields` was
/// already pulled down by [`walk`]'s own `bulk_get`, so this only spends
/// fetches on whatever forward/reverse evidence the classifier needs.
/// `ref_limit` bounds the reverse keyword/AVIF consumer walk (see
/// [`WalkOptions::ref_limit`]); runs regardless of `--depth`, which only
/// governs BFS enqueueing.
#[allow(clippy::too_many_arguments)]
fn digest_omod_mechanisms(
    f: &mut impl ChaseFetcher,
    formid: FormId,
    sig: &str,
    editor_id: &str,
    fields: &Value,
    ref_limit: usize,
    lines: &mut Vec<String>,
    enqueue: &mut Vec<EnqueueTarget>,
) -> anyhow::Result<()> {
    // `tree.root` is discarded below (walk already knows the root's identity
    // from its own `WalkNode`) — Name/Description are left `None` since
    // `omod_chase` never reads them back off `root`, only echoes them.
    let root = RootStub {
        formid: Some(formid.display()),
        record_type: Some(sig.to_string()),
        editor_id: Some(editor_id.to_string()),
        name: None,
        description: None,
    };
    let opts = ChaseOptions {
        depth: crate::chase::DEFAULT_DEPTH,
        ref_limit,
    };
    let tree = omod_chase(f, root, fields, &opts)?;
    render_omod_hops(&tree.hops, lines, enqueue);
    Ok(())
}

/// Render classified [`Hop`]s in classifier order. Consecutive
/// [`HopKind::TagKeyword`] hops collapse into one `tags` block; include-
/// sourced hops (`source_omod.is_some()`) are skipped here — walk enqueues
/// includes as their own BFS nodes instead of folding them into the
/// includer's digest.
fn render_omod_hops(hops: &[Hop], lines: &mut Vec<String>, enqueue: &mut Vec<EnqueueTarget>) {
    let mut i = 0;
    while i < hops.len() {
        let hop = &hops[i];
        if hop.source_omod.is_some() {
            i += 1;
            continue;
        }
        let Some(target) = &hop.target else {
            i += 1;
            continue;
        };
        let target_rt = target
            .get("record_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        if target_rt == "ENCH" {
            i += 1;
            continue;
        }
        if hop.kind == HopKind::TagKeyword {
            let start = i;
            i += 1;
            while i < hops.len() {
                let next = &hops[i];
                if next.source_omod.is_some() || next.kind != HopKind::TagKeyword {
                    break;
                }
                i += 1;
            }
            render_tag_keyword_block(&hops[start..i], lines);
            continue;
        }
        render_omod_hop(hop, lines, enqueue);
        i += 1;
    }
}

/// One `tags` block for a contiguous run of [`HopKind::TagKeyword`] hops —
/// editor_id plus Notes when present; never a dead-end caveat.
fn render_tag_keyword_block(hops: &[Hop], lines: &mut Vec<String>) {
    if hops.is_empty() {
        return;
    }
    lines.push("tags".to_string());
    for hop in hops {
        let Some(target) = &hop.target else {
            continue;
        };
        let edid = target
            .get("editor_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let notes = hop
            .evidence
            .first()
            .and_then(|ev| ev.detail.get("notes"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        match notes {
            Some(n) => lines.push(format!("    {edid} — {n}")),
            None => lines.push(format!("    {edid}")),
        }
    }
}

/// Render one classified [`Hop`] as indented mechanism lines. Skips bare-
/// number properties (`hop.target.is_none()` — nothing to chase, unchanged
/// from before this walk had a dedicated OMOD digest) and ENCH-typed targets
/// ([`digest_omod_ench_follow`] already forward-follows and enqueues those as
/// their own BFS node — rendering them again here would be redundant).
fn render_omod_hop(hop: &Hop, lines: &mut Vec<String>, enqueue: &mut Vec<EnqueueTarget>) {
    let Some(target) = &hop.target else {
        return;
    };
    let target_rt = target
        .get("record_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    if target_rt == "ENCH" {
        return;
    }

    match hop.kind {
        HopKind::PerkGrant => {
            lines.push(format!("perk grant → {}", fmt_stub(target)));
            render_forward_evidence(&hop.evidence, lines);
        }
        HopKind::KeywordHook => {
            lines.push(format!("keyword hook → {}", fmt_stub(target)));
            render_reverse_evidence(&hop.evidence, lines);
        }
        HopKind::TagKeyword => {
            // Consecutive runs are flushed by [`render_omod_hops`]; a lone
            // call still renders a one-entry tags block.
            render_tag_keyword_block(std::slice::from_ref(hop), lines);
        }
        // An AVIF direct-property target is reverse-chased exactly like a
        // KYWD keyword_hook (see `esm::chase::omod_chase`) even though its
        // `HopKind` stays `DirectProperty` — the hop-kind taxonomy doesn't
        // distinguish "direct" from "AV hook", so dispatch on the target's
        // own record_type here instead.
        HopKind::DirectProperty if target_rt == "AVIF" => {
            lines.push(format!("AV hook → {}", fmt_stub(target)));
            render_reverse_evidence(&hop.evidence, lines);
        }
        HopKind::DirectProperty if target_rt == "PROJ" => {
            lines.push(format!("direct property → {}", fmt_stub(target)));
            render_projectile_evidence(&hop.evidence, lines);
            if let Some(fid) = stub_formid(Some(target)) {
                enqueue.push((fid, "OMOD property".to_string()));
            }
        }
        // Direct SPEL attachment (the other `FORWARD_FETCH_TYPES` member
        // besides ENCH/PERK) — forward-fetched the same way a perk grant is.
        HopKind::DirectProperty => {
            lines.push(format!("direct property → {}", fmt_stub(target)));
            render_forward_evidence(&hop.evidence, lines);
        }
    }
}

/// Compact PROJ/EXPL summary from chase's projectile evidence detail.
fn render_projectile_evidence(evidence: &[Evidence], lines: &mut Vec<String>) {
    for ev in evidence {
        let d = &ev.detail;
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = d.get("type").and_then(Value::as_str) {
            parts.push(format!("type {t}"));
        }
        if let Some(s) = d.get("speed") {
            parts.push(format!("speed {}", pyish(s)));
        }
        if !parts.is_empty() {
            lines.push(format!("  {}", parts.join("  ")));
        }
        if let Some(expl) = d.get("explosion").filter(|v| is_ref_stub(v)) {
            lines.push(format!("  explosion → {}", fmt_stub(expl)));
        }
        render_explosion_detail_lines(d, lines, "  ");
    }
}

/// Shared EXPL field lines (radius/force/stagger/impact/chain/damage) used by
/// both the OMOD projectile-evidence slice and the EXPL digest arm.
fn render_explosion_detail_lines(detail: &Value, lines: &mut Vec<String>, indent: &str) {
    if let Some(radius) = detail.get("radius").and_then(Value::as_array) {
        let inner = radius.first().map(pyish).unwrap_or_else(|| "?".to_string());
        let outer = radius.get(1).map(pyish).unwrap_or_else(|| "?".to_string());
        lines.push(format!("{indent}radius {inner}/{outer}"));
    }
    let mut phys: Vec<String> = Vec::new();
    if let Some(f) = detail.get("force") {
        phys.push(format!("force {}", pyish(f)));
    }
    if let Some(s) = detail.get("stagger").and_then(Value::as_str) {
        phys.push(format!("stagger {s}"));
    }
    if !phys.is_empty() {
        lines.push(format!("{indent}{}", phys.join("  ")));
    }
    if let Some(ipds) = detail.get("impact_data_set").and_then(Value::as_str) {
        lines.push(format!("{indent}impact {ipds}"));
    }
    if detail.get("chain").and_then(Value::as_bool) == Some(true) {
        lines.push(format!("{indent}chain"));
    }
    if let Some(placed) = detail.get("placed_object").filter(|v| is_ref_stub(v)) {
        lines.push(format!("{indent}placed object → {}", fmt_stub(placed)));
    }
    if let Some(spawn) = detail.get("spawn_projectile").filter(|v| is_ref_stub(v)) {
        lines.push(format!("{indent}spawn projectile → {}", fmt_stub(spawn)));
    }
    if let Some(damage) = detail.get("damage").and_then(Value::as_array) {
        if damage.is_empty() {
            // Empty + chain signals arc falloff elsewhere; empty alone is a
            // utility explosion (radius/force/stagger IS the effect).
            if detail.get("chain").and_then(Value::as_bool) != Some(true) {
                lines.push(format!("{indent}damage (none)"));
            }
        } else {
            for row in damage {
                lines.push(format!("{indent}damage {}", format_damage_row(row)));
            }
        }
    }
}

fn format_damage_row(row: &Value) -> String {
    if let Some(flat) = row.get("flat") {
        return format!("flat {}", pyish(flat));
    }
    if let Some(m) = row.get("base_weapon_mult") {
        return format!("base weapon mult {}", pyish(m));
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = row.get("type").and_then(Value::as_str) {
        parts.push(t.to_string());
    }
    if let Some(c) = row.get("curve").and_then(Value::as_str) {
        parts.push(format!("via {c}"));
    }
    if let Some(range) = row.get("range").and_then(Value::as_array)
        && range.len() >= 2
    {
        parts.push(format!("[{}–{}]", pyish(&range[0]), pyish(&range[1])));
    }
    if let Some(amount) = row.get("amount") {
        parts.push(format!("amount {}", pyish(amount)));
    }
    if parts.is_empty() {
        pyish(row)
    } else {
        parts.join("  ")
    }
}

/// Render forward-fetched [`Evidence`] (perk_grant / direct SPEL attachment):
/// the target's own `Effects[]` array (already capped by
/// `esm::chase`'s `forward_evidence`), path-labeled by array position since
/// there's no gating condition to slice down to one entry — plus a trailing
/// MGEF pass-through line when the target's own Base Effect carries "Perk to
/// Apply"/"Equip Ability" (named, not expanded further — same as `chase`).
fn render_forward_evidence(evidence: &[Evidence], lines: &mut Vec<String>) {
    for ev in evidence {
        if let Some(effects) = ev.detail.get("effects").and_then(Value::as_array) {
            for (i, eff) in effects.iter().enumerate() {
                lines.push(format!("  Effects[{i}] {}", summarize_effect(eff)));
            }
            if let Some(trunc) = ev
                .detail
                .get("effects_truncated")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
            {
                lines.push(format!("  … +{trunc} more effects (truncated)"));
            }
        }
        if let Some(p) = ev.detail.get("perk_to_apply").filter(|v| !v.is_null()) {
            lines.push(format!("  Perk to Apply → {}", fmt_stub(p)));
        }
        if let Some(e) = ev.detail.get("equip_ability").filter(|v| !v.is_null()) {
            lines.push(format!("  Equip Ability → {}", fmt_stub(e)));
        }
    }
}

/// Render reverse-chased [`Evidence`] (keyword_hook / AVIF consumer lookup):
/// group consecutive entries sharing the same gating consumer under one
/// `"gates <consumer>"` header, then the exact path-sliced `Effects[N]` row
/// each evidence entry names — never the consumer's full digest. Empty
/// evidence (no SPEL/PERK condition ever references this target) renders one
/// dead-end line instead of silently vanishing.
fn render_reverse_evidence(evidence: &[Evidence], lines: &mut Vec<String>) {
    if evidence.is_empty() {
        lines.push(
            "  (no SPEL/PERK condition references this — dead end; may be UI-only, \
             native-engine-consumed, or a shared/common tag)"
                .to_string(),
        );
        return;
    }
    let mut last_source: Option<String> = None;
    for ev in evidence {
        let source_fid = ev
            .source
            .get("formid")
            .and_then(Value::as_str)
            .unwrap_or("");
        if last_source.as_deref() != Some(source_fid) {
            lines.push(format!("  gates {}", fmt_stub(&ev.source)));
            last_source = Some(source_fid.to_string());
        }
        if let Some(effect) = ev.detail.get("effect") {
            let label = ev
                .via
                .as_deref()
                .and_then(first_array_container)
                .unwrap_or_else(|| "effect".to_string());
            lines.push(format!("    {label} {}", summarize_effect(effect)));
        } else if let Some(note) = ev.detail.get("note").and_then(Value::as_str) {
            lines.push(format!("    {note}"));
        }
    }
}

/// `Data.Includes[]` names another OMOD this one composes from — the
/// `_PARENT_*` empty-shell pattern (properties compose onto the includer) or
/// a `modcol_*` collection (each include is an alternative). Nothing in the
/// data reliably distinguishes the two, so we don't merge: each include is
/// enqueued into the BFS as its own walked node (same pattern as
/// [`digest_omod_ench_follow`]), bounded by [`OMOD_INCLUDE_ENQUEUE_CAP`].
fn digest_omod_includes(
    fields: &Value,
    editor_id: &str,
    lines: &mut Vec<String>,
    enqueue: &mut Vec<EnqueueTarget>,
) {
    let Some(includes) = fields.pointer("/Data/Includes").and_then(Value::as_array) else {
        return;
    };
    let mut named = 0usize;
    let mut total = 0usize;
    for inc in includes {
        let Some(target) = inc.get("Mod").filter(|v| is_ref_stub(v)) else {
            continue;
        };
        total += 1;
        if named >= OMOD_INCLUDE_ENQUEUE_CAP {
            continue;
        }
        lines.push(format!("include → {}", fmt_stub(target)));
        if let Some(fid) = stub_formid(Some(target)) {
            enqueue.push((fid, format!("include of {editor_id}")));
        }
        named += 1;
    }
    if total > OMOD_INCLUDE_ENQUEUE_CAP {
        lines.push(format!(
            "  … +{} more includes (truncated)",
            total - OMOD_INCLUDE_ENQUEUE_CAP
        ));
    }
}

fn digest_proj(fields: &Value, lines: &mut Vec<String>, enqueue: &mut Vec<EnqueueTarget>) {
    let Some(data) = fields.get("Data") else {
        return;
    };
    if let Some(t) = data
        .get("Type")
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
    {
        lines.push(format!("type {t}"));
    }
    if let Some(s) = data.get("Speed") {
        lines.push(format!("speed {}", pyish(s)));
    }
    if let Some(expl) = data.get("Explosion").filter(|v| is_ref_stub(v)) {
        lines.push(format!("explosion → {}", fmt_stub(expl)));
        if let Some(fid) = stub_formid(Some(expl)) {
            enqueue.push((fid, "projectile explosion".to_string()));
        }
    }
}

fn digest_expl(fields: &Value, lines: &mut Vec<String>) {
    let detail = summarize_explosion_detail(fields);
    render_explosion_detail_lines(&detail, lines, "");
}

// ─── refs digest (root-only, `--refs`) ──────────────────────────────────────

/// Group an unfiltered reverse-`refs` row list by `record_type`, sorted by
/// count descending, each with up to 5 sample EditorIDs (⚠NONPLAYABLE-flagged)
/// and an obtainability tag (mirrors the TS original's `digestRefs`). Pure —
/// takes the raw rows from whatever unfiltered `Op::ReferencedBy` call the
/// caller already made (see module docs); no fetcher involved, so this is
/// directly unit-testable.
pub fn build_refs_digest(rows: &[RefRow]) -> RefsDigest {
    let mut by_type: HashMap<String, Vec<&RefRow>> = HashMap::new();
    for r in rows {
        let key = r.record_type.clone().unwrap_or_else(|| "????".to_string());
        by_type.entry(key).or_default().push(r);
    }
    let mut groups: Vec<(String, Vec<&RefRow>)> = by_type.into_iter().collect();
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

    let out = groups
        .into_iter()
        .map(|(record_type, items)| {
            let sample: Vec<String> = items
                .iter()
                .take(5)
                .map(|r| {
                    let edid = r.editor_id.clone().unwrap_or_default();
                    if edid.to_uppercase().contains("NONPLAYABLE") {
                        format!("{edid} ⚠NONPLAYABLE")
                    } else {
                        edid
                    }
                })
                .collect();
            // Two leading spaces are baked into the tag itself (rather than
            // added at the join point in `render_text`) to match the TS
            // original's own tag string literals exactly.
            let tag = if OBTAINABLE_TYPES.contains(&record_type.as_str()) {
                Some("  [player-facing signal]".to_string())
            } else if record_type == "LVLI" {
                Some("  [only player-facing LVLI chains count]".to_string())
            } else {
                None
            };
            RefsDigestGroup {
                record_type,
                count: items.len(),
                sample,
                tag,
            }
        })
        .collect();
    RefsDigest { groups: out }
}

// ─── the walk ────────────────────────────────────────────────────────────────

/// Run one node's record-type-specific digest, returning the lines to attach
/// to its [`WalkNode`] plus any FormIDs it wants enqueued one hop further
/// (with the "via" edge label to attach to the enqueued node). `ref_limit`
/// only matters for the OMOD arm's mechanism slice (see
/// [`digest_omod_mechanisms`]) and the KYWD/AVIF root's own consumer digest
/// (which ignores it — see [`WalkOptions::ref_limit`]).
fn digest_node(
    f: &mut impl ChaseFetcher,
    sig: &str,
    formid: FormId,
    editor_id: &str,
    fields: &Value,
    ref_limit: usize,
) -> anyhow::Result<(Vec<String>, Vec<EnqueueTarget>)> {
    let mut lines = Vec::new();
    let mut enqueue = Vec::new();
    match sig {
        "GLOB" => digest_glob(fields, &mut lines),
        "AVIF" => {
            digest_avif(fields, &mut lines);
            digest_keyword_or_av(f, formid, &mut lines)?;
        }
        "KYWD" => digest_keyword_or_av(f, formid, &mut lines)?,
        "MGEF" => digest_mgef(fields, &mut lines, &mut enqueue),
        "SPEL" | "ENCH" | "ALCH" => digest_magic_item(f, fields, &mut lines, &mut enqueue)?,
        "PERK" => digest_perk(f, fields, &mut lines, &mut enqueue)?,
        "WEAP" => digest_weap(fields, &mut lines),
        "PROJ" => digest_proj(fields, &mut lines, &mut enqueue),
        "EXPL" => digest_expl(fields, &mut lines),
        "OMOD" => {
            // Ordered to match the mechanism-slice narrative: the ENCH chain
            // (a full BFS-followed node) first, then the property mechanisms
            // the classifier resolves inline, then the Includes[] pointers
            // (enqueued as their own BFS nodes — no property merge).
            digest_omod_ench_follow(f, fields, &mut lines, &mut enqueue)?;
            digest_omod_mechanisms(
                f,
                formid,
                sig,
                editor_id,
                fields,
                ref_limit,
                &mut lines,
                &mut enqueue,
            )?;
            digest_omod_includes(fields, editor_id, &mut lines, &mut enqueue);
        }
        _ => digest_generic(fields, &mut lines),
    }
    Ok((lines, enqueue))
}

/// Run the walk for one root selector: BFS out to `opts.depth`, printing a
/// digest for every visited node. Returns [`WalkResult::not_found`] (with an
/// empty `matches` list) instead of an `Err` when the root selector's own
/// `bulk_get` comes back with an error entry — see module docs for why the
/// actual search fallback is the caller's job.
pub fn walk(
    f: &mut impl ChaseFetcher,
    selector: RecordSel,
    opts: &WalkOptions,
) -> anyhow::Result<WalkResult> {
    let mut visited: HashSet<FormId> = HashSet::new();
    let mut nodes: Vec<WalkNode> = Vec::new();
    let mut queue: VecDeque<(RecordSel, usize, Option<String>)> = VecDeque::new();
    queue.push_back((selector.clone(), 0, None));

    while let Some((sel, depth, via)) = queue.pop_front() {
        let entry = f
            .bulk_get(std::slice::from_ref(&sel), ResolveDepth::Stub)?
            .into_iter()
            .next()
            .context("bulk_get returned no entries for the walk target")?;

        let header = if entry.error.is_none() {
            entry.header.clone()
        } else {
            None
        };
        let Some(header) = header else {
            if depth == 0 {
                return Ok(WalkResult {
                    not_found: Some(NotFound {
                        target: sel.display(),
                        matches: Vec::new(),
                    }),
                    nodes: Vec::new(),
                    refs: None,
                });
            }
            // A queued (non-root) target failed to resolve — skip it silently
            // rather than aborting the whole walk over one bad reference.
            continue;
        };

        let formid = header.form_id;
        if !visited.insert(formid) {
            continue;
        }

        let fields = entry.fields.clone().unwrap_or(Value::Null);
        let editor_id = entry.editor_id.clone().unwrap_or_default();
        let name = fields
            .get("Name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let (lines, enqueue) = digest_node(
            f,
            &header.signature,
            formid,
            &editor_id,
            &fields,
            opts.ref_limit,
        )?;

        nodes.push(WalkNode {
            depth,
            sig: header.signature,
            formid: formid.display(),
            editor_id,
            name,
            via,
            digest: lines,
        });

        if depth < opts.depth {
            for (target_fid, via_label) in enqueue {
                if !visited.contains(&target_fid) {
                    queue.push_back((RecordSel::FormId(target_fid), depth + 1, Some(via_label)));
                }
            }
        }
    }

    Ok(WalkResult {
        not_found: None,
        nodes,
        refs: None,
    })
}

// ─── rendering ───────────────────────────────────────────────────────────────

/// Render a [`WalkResult`] as human-readable text — the CLI's default
/// (non-`--json`) output.
pub fn render_text(result: &WalkResult) -> String {
    let mut out: Vec<String> = Vec::new();

    if let Some(nf) = &result.not_found {
        let suffix = if nf.matches.is_empty() {
            " No search matches either.".to_string()
        } else {
            " Search matches:".to_string()
        };
        out.push(format!("\"{}\" not found by get.{suffix}", nf.target));
        for m in &nf.matches {
            out.push(format!(
                "  {} {} {} {}",
                m.form_id,
                m.record_type.as_deref().unwrap_or("?"),
                m.editor_id.as_deref().unwrap_or(""),
                m.name.as_deref().unwrap_or("")
            ));
        }
        return out.join("\n");
    }

    for node in &result.nodes {
        out.push(String::new());
        let marker = "▸".repeat(node.depth + 1);
        let name = node
            .name
            .as_deref()
            .map(|n| format!(" \"{n}\""))
            .unwrap_or_default();
        let via = node
            .via
            .as_deref()
            .map(|v| format!("  (via {v})"))
            .unwrap_or_default();
        out.push(format!(
            "{marker} {} {} {}{name}{via}",
            node.sig, node.formid, node.editor_id
        ));
        for l in &node.digest {
            out.push(format!("  {l}"));
        }
    }

    if let Some(refs) = &result.refs {
        out.push(String::new());
        out.push("reverse refs:".to_string());
        if refs.groups.is_empty() {
            out.push(
                "  NO reverse references — normal for script/VMAD quest rewards, vendor grants, \
                 and account-side (ATX) items; check the rescue lists before assuming junk."
                    .to_string(),
            );
        } else {
            for g in &refs.groups {
                let more = if g.count > g.sample.len() {
                    ", …"
                } else {
                    ""
                };
                let tag = g.tag.as_deref().unwrap_or("");
                out.push(format!(
                    "  {} ×{}: {}{more}{tag}",
                    g.record_type,
                    g.count,
                    g.sample.join(", ")
                ));
            }
            out.push(
                "  Reminder: the record graph cannot distinguish shipped from UNRELEASED content \
                 (P62/The Drifter looked obtainable). Confirm release status before rescuing."
                    .to_string(),
            );
        }
    }

    out.join("\n")
}
