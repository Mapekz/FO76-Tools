//! Leveled-item (LVLI) drop-probability engine.
//!
//! Pure math + fetch logic behind `esm walk`'s LVLI digest ([`crate::walk`]'s
//! `digest_lvli`) — kept as its own module (rather than inlined in
//! `walk.rs`) so a future `chase` root or MCP tool can wrap [`drop_table`]
//! without duplicating the selection math, per
//! `docs/adr/0001-walk-interactive-chase-pipeline-json.md`'s "one classifier
//! core, verbs differ in contract" shape. `walk.rs` is still the only
//! *renderer* — this module returns structured [`DropTable`]/[`DropRow`]
//! data, never a formatted string.
//!
//! Mechanics ported from `skills/esm-cli/SKILL.md`'s "Drop-chance math (LVLI
//! chains)" section and the untracked `tools/lvli_audit.py` prototype's
//! `compute_pool_odds`/`_random_percent_prob` (a linter over the same
//! records, not a drop-rate calculator — this module is the first thing in
//! the repo that actually computes one).
//!
//! ## Selection models (LVLF flags)
//!
//! - **Pool** (no flag): every entry's gate rolls independently; the
//!   passing subset is pooled and one member is picked uniformly. Exact via
//!   subset enumeration up to [`MAX_EXACT_POOL_ENTRIES`] entries, then a
//!   documented mean-field approximation (flagged [`DropNote::PoolCapped`]).
//! - **`Use All`**: every entry's gate rolls independently and *all* that
//!   pass are dispensed — not mutually exclusive with each other.
//! - **`Use First Object That Matches All Conditions`**: ordered cascade —
//!   the first entry whose gate passes wins; a lower entry's true odds are
//!   its own gate times the product of every earlier entry's miss chance.
//!
//! `Chance None` (list- and entry-level) is layered on *after* selection —
//! it is not one of the CTDA eligibility gates above (see [`entry_chance_none`]/
//! [`resolve_chance_none`]) — and, like every other scalar here, a sibling
//! Curve Table takes precedence over a sibling Global over the flat value
//! (see [`eval_curve`]).
//!
//! ## What isn't modeled (never silently — see [`DropNote::Unresolved`])
//!
//! `Filter Keyword Chances` (LLKC), `Epic Loot Chance` (LVSG), list-level
//! `Max Count`/`Max Global`/`Max Curve Table`, and `Extra Data` (COED)
//! owner/rank gates. FO76's "does `Calculate from all levels <= player's
//! level` being unset collapse multiple qualifying Minimum Level tiers down
//! to just the top one" behavior (classic in older Bethesda engines) is
//! flagged unverified rather than assumed — see [`DropOptions::level`].

use crate::chase::ChaseFetcher;
use crate::curves::{CurvePoint, eval as curve_eval};
use crate::walk::{
    bulk_fetch_map, collect_condition_refs, dedup_sorted, flatten_condition_rows, stub_formid,
};
use crate::{BulkRecordEntry, FormId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Default player level assumed for Curve Table evaluation and Minimum
/// Level filtering when `--level` isn't passed.
pub const DEFAULT_LEVEL: f32 = 50.0;

/// Recursion cap for LVLI → LVLI chains (a sublist referencing another
/// sublist). FO76's authored chains run 1-3 deep in practice; this is a
/// safety backstop, not a measured corpus max.
pub const MAX_RECURSION_DEPTH: usize = 8;

/// Above this many entries, [`compute_pool_odds`]'s O(2^n) subset
/// enumeration stops being worth it (mirrors
/// `tools/lvli_audit.py::MAX_EXACT_ODDS_ENTRIES`).
pub const MAX_EXACT_POOL_ENTRIES: usize = 16;

const CALC_ALL_LEVELS: &str = "Calculate from all levels <= player's level";
const USE_ALL: &str = "Use All";
const USE_FIRST_MATCH: &str = "Use First Object That Matches All Conditions";

/// How a node picks among its entries (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionModel {
    Pool,
    UseAll,
    UseFirstMatch,
}

/// A caveat attached to one [`DropRow`] — something about its number that
/// isn't fully modeled or is only approximate. Never silently dropped; see
/// module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DropNote {
    /// A Condition gate isn't `GetRandomPercent` (e.g. `GetLevel`,
    /// `HasLearnedRecipe`) — a real gate, but not a probability this engine
    /// can compute. Assumed passing (or, under [`DropOptions::strict`],
    /// assumed failing) — either way, the row's number reflects that guess.
    Gated { function: String },
    /// A sublist FormID recurred onto its own ancestor path; treated as
    /// contributing nothing rather than looping forever.
    Cycle,
    /// Recursion stopped at [`DropOptions::max_depth`]; the unexpanded
    /// sublist is reported as its own pseudo-row instead.
    DepthCapped,
    /// More than [`MAX_EXACT_POOL_ENTRIES`] entries in a no-flag pool node —
    /// odds are a mean-field approximation, not exact subset enumeration.
    PoolCapped,
    /// `Quantity != 1` on an entry whose target is itself a sublist: the
    /// expected-count number is exact regardless (linearity of expectation),
    /// but whether the engine treats this as "N independent redraws" or "one
    /// draw, multiply the result" changes the presence probability, and
    /// that distinction isn't confirmed for FO76.
    QuantityOnSublist,
    /// A feature this engine doesn't model reached this row — see module
    /// docs' "what isn't modeled" list.
    Unresolved { reason: String },
}

/// One leaf item's aggregated odds across every path that reaches it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropRow {
    pub formid: String,
    pub editor_id: String,
    pub record_type: String,
    /// Expected number of copies per invocation of the root list — exact
    /// (linearity of expectation holds regardless of the approximations
    /// tracked in `notes`).
    pub expected_count: f64,
    /// Probability this item appears at least once per invocation.
    pub p_at_least_one: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<DropNote>,
}

/// The result of resolving one LVLI's odds, sorted by `expected_count`
/// descending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropTable {
    pub model: SelectionModel,
    pub level: f32,
    /// Probability this invocation of the root list yields nothing at all.
    pub p_nothing: f64,
    pub rows: Vec<DropRow>,
    /// Set if recursion hit [`DropOptions::max_depth`] anywhere, or a pool
    /// node exceeded [`MAX_EXACT_POOL_ENTRIES`] — the table is still
    /// complete, just not everywhere exact (see each row's `notes`).
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DropOptions {
    /// Player level assumed for Curve Table evaluation and Minimum Level
    /// filtering.
    pub level: f32,
    pub max_depth: usize,
    /// When true, a Condition gate this engine can't compute (see
    /// [`DropNote::Gated`]) is assumed to fail rather than pass.
    pub strict: bool,
}

impl Default for DropOptions {
    fn default() -> Self {
        Self {
            level: DEFAULT_LEVEL,
            max_depth: MAX_RECURSION_DEPTH,
            strict: false,
        }
    }
}

// ─── field access ───────────────────────────────────────────────────────────

fn entries(fields: &Value) -> Vec<&Value> {
    fields
        .get("Leveled List Entries")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("Leveled List Entry"))
                .collect()
        })
        .unwrap_or_default()
}

/// `Reference` (form_version >= 174) or the legacy `Base Data.Item`.
fn entry_target(entry: &Value) -> Option<&Value> {
    entry
        .get("Reference")
        .or_else(|| entry.pointer("/Base Data/Item"))
}

fn is_legacy_entry(entry: &Value) -> bool {
    entry.get("Reference").is_none()
}

/// A GLOB reference's own `Value` field, resolved from an already-populated
/// `by_sel` map (see [`bulk_fetch_map`]).
fn resolve_glob_value(
    stub: Option<&Value>,
    by_sel: &HashMap<String, BulkRecordEntry>,
) -> Option<f64> {
    let obj = stub?.as_object()?;
    if obj.get("record_type").and_then(Value::as_str) != Some("GLOB") {
        return None;
    }
    let fid = obj.get("formid")?.as_str()?;
    by_sel.get(fid)?.fields.as_ref()?.get("Value")?.as_f64()
}

/// A CURV reference's points are inlined onto the field regardless of
/// resolve depth (see `crate::decode::resolve_formid`'s CURV branch) — no
/// fetch needed, just evaluate at `level`.
fn eval_curve(v: &Value, level: f32) -> Option<f64> {
    let points = v.get("curve")?.as_array()?;
    let pts: Vec<CurvePoint> = points
        .iter()
        .filter_map(|p| {
            let x = p.get("x")?.as_f64()? as f32;
            let y = p.get("y")?.as_f64()? as f32;
            Some(CurvePoint { x, y })
        })
        .collect();
    curve_eval(&pts, level).map(f64::from)
}

/// Flat-wins-over-GLOB-over-Curve-Table chance-none resolution, shared by
/// the list level (`Chance None Value`/`Chance None Global`/`Chance None
/// Curve Table`) and the modern entry shape (same three key names — LVLI's
/// schema reuses them at both levels). Returns a probability in `[0, 1]`.
fn resolve_chance_none(node: &Value, level: f32, by_sel: &HashMap<String, BulkRecordEntry>) -> f64 {
    if let Some(c) = node
        .get("Chance None Curve Table")
        .and_then(|v| eval_curve(v, level))
    {
        return (c / 100.0).clamp(0.0, 1.0);
    }
    let flat = node
        .get("Chance None Value")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if flat != 0.0 {
        return (flat / 100.0).clamp(0.0, 1.0);
    }
    if let Some(g) = resolve_glob_value(node.get("Chance None Global"), by_sel) {
        return (g / 100.0).clamp(0.0, 1.0);
    }
    0.0
}

/// An entry's own chance-none: the modern flat/GLOB/curve trio for a
/// `Reference`-shaped entry, or the legacy `Base Data.Chance None` u8
/// (no GLOB/curve sibling exists on that pre-174 shape).
fn entry_chance_none(entry: &Value, level: f32, by_sel: &HashMap<String, BulkRecordEntry>) -> f64 {
    if is_legacy_entry(entry) {
        entry
            .pointer("/Base Data/Chance None")
            .and_then(Value::as_f64)
            .map(|v| (v / 100.0).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    } else {
        resolve_chance_none(entry, level, by_sel)
    }
}

/// An entry's Minimum Level: `Minimum Level Global` > flat `Minimum Level`
/// (modern shape), or `Base Data.Level` (legacy). `None` means no level gate.
///
/// Unlike [`resolve_chance_none`]/[`resolve_quantity`], a `Minimim Level
/// Curve Table` sibling (schema typo, preserved verbatim) is deliberately
/// **not** evaluated here. Spot-checked against live data:
/// `LL_Armor_Metal_ArmLeft`'s curve `MinLevel_Armor_Metal_CT` has points
/// `(0,1)(1,10)(2,25)(3,35)(99,35)(100,100)` — an x-domain that reads as an
/// item-quality-tier index (0-3, with 99/100 sentinel rows), not a player
/// level, unlike the Quantity/Chance-None curve tables checked at the same
/// time (`CT_Creatures_Loot_WeaponUser_Steel_Base`: `x` 1-50 with `y`
/// climbing 3→7; `Container_Item2_ChanceNone`: `x` 0-100 with `y` falling
/// 100→0), both of which read as genuinely level-shaped. Evaluating this one
/// at `--level` would silently invent a number off an unconfirmed axis, so
/// it's flagged instead of guessed.
fn resolve_min_level(
    entry: &Value,
    by_sel: &HashMap<String, BulkRecordEntry>,
    notes: &mut Vec<DropNote>,
) -> Option<f32> {
    if is_legacy_entry(entry) {
        return entry
            .pointer("/Base Data/Level")
            .and_then(Value::as_f64)
            .map(|v| v as f32);
    }
    if entry.get("Minimim Level Curve Table").is_some_and(|v| {
        v.get("curve")
            .is_some_and(|c| c.as_array().is_some_and(|a| !a.is_empty()))
    }) {
        notes.push(DropNote::Unresolved {
            reason: "Minimum Level Curve Table present — its input axis isn't confirmed to be \
                     player level (looks tier-indexed on spot-checked data), so it's not evaluated"
                .to_string(),
        });
    }
    if let Some(g) = resolve_glob_value(entry.get("Minimum Level Global"), by_sel) {
        return Some(g as f32);
    }
    entry
        .get("Minimum Level")
        .and_then(Value::as_f64)
        .map(|v| v as f32)
}

/// An entry's Quantity, Curve-Table > Global > flat (modern shape) or
/// `Base Data.Count` (legacy). `Quantity: 0` means "use the sublist's own
/// count", not disabled — normalized to `1.0` here.
fn resolve_quantity(entry: &Value, level: f32, by_sel: &HashMap<String, BulkRecordEntry>) -> f64 {
    let raw = if is_legacy_entry(entry) {
        entry.pointer("/Base Data/Count").and_then(Value::as_f64)
    } else if let Some(c) = entry
        .get("Quantity Curve Table")
        .and_then(|v| eval_curve(v, level))
    {
        Some(c)
    } else if let Some(g) = resolve_glob_value(entry.get("Quantity Global"), by_sel) {
        Some(g)
    } else {
        entry.get("Quantity").and_then(Value::as_f64)
    }
    .unwrap_or(1.0);
    if raw == 0.0 { 1.0 } else { raw }
}

/// LVLF flag names. `"Flags 2"` (when present) is *always* the LVLF union —
/// its own outer schema key collides with XALG's identically-named `Flags`
/// member, and XALG always decodes first (earlier in record order), so any
/// record carrying both lands XALG under `"Flags"` and LVLF under
/// `"Flags 2"` (see `src/decode/mod.rs`'s `insert_unique`). Only fall back to
/// plain `"Flags"` when `"Flags 2"` is absent — reading *both* keys and
/// filtering by name would misfire on `"Item Dispenser"`, which is a real
/// flag name in *both* XALG's and LVLF's vocabularies.
fn lvlf_flags(fields: &Value) -> HashSet<String> {
    let key = if fields.get("Flags 2").is_some() {
        "Flags 2"
    } else {
        "Flags"
    };
    fields
        .get(key)
        .and_then(|f| f.get("flags"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn selection_model(flags: &HashSet<String>) -> SelectionModel {
    if flags.contains(USE_ALL) {
        SelectionModel::UseAll
    } else if flags.contains(USE_FIRST_MATCH) {
        SelectionModel::UseFirstMatch
    } else {
        SelectionModel::Pool
    }
}

// ─── condition gates ────────────────────────────────────────────────────────

/// One condition row's pass probability. Only `GetRandomPercent` is a real
/// probability (a uniform 0-100 roll); anything else is a genuine gate this
/// engine can't compute, so it's noted and defaulted per `strict`.
fn condition_row_prob(
    row: &Value,
    by_sel: &HashMap<String, BulkRecordEntry>,
    strict: bool,
    notes: &mut Vec<DropNote>,
) -> f64 {
    let function = row
        .get("Function")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let fallback = if strict { 0.0 } else { 1.0 };
    if function != "GetRandomPercent" {
        notes.push(DropNote::Gated { function });
        return fallback;
    }
    let operator = row.get("Operator").and_then(Value::as_str).unwrap_or("?");
    let cmp = match row.get("Comparison Value") {
        Some(v) if v.is_object() => resolve_glob_value(Some(v), by_sel),
        Some(v) => v.as_f64(),
        None => None,
    };
    let Some(cmp) = cmp else {
        notes.push(DropNote::Gated { function });
        return fallback;
    };
    match operator {
        "Greater Than" | "Greater Than Or Equal To" => ((100.0 - cmp) / 100.0).clamp(0.0, 1.0),
        "Less Than" | "Less Than Or Equal To" => (cmp / 100.0).clamp(0.0, 1.0),
        _ => {
            notes.push(DropNote::Gated { function });
            fallback
        }
    }
}

/// An entry's overall gate-pass probability: OR-groups (a run of rows joined
/// by a trailing `"AND/OR": "OR"`) combine via `1 - Π(1 - p)`, then groups AND
/// together. No conditions at all means always-eligible (`1.0`).
fn entry_gate_prob(
    rows: &[Value],
    by_sel: &HashMap<String, BulkRecordEntry>,
    strict: bool,
    notes: &mut Vec<DropNote>,
) -> f64 {
    if rows.is_empty() {
        return 1.0;
    }
    let mut total = 1.0_f64;
    let mut i = 0;
    while i < rows.len() {
        let mut group_fail = 1.0_f64;
        loop {
            let row = &rows[i];
            let p = condition_row_prob(row, by_sel, strict, notes);
            group_fail *= 1.0 - p;
            let is_or = row.get("AND/OR").and_then(Value::as_str) == Some("OR");
            i += 1;
            if !is_or || i >= rows.len() {
                break;
            }
        }
        total *= 1.0 - group_fail;
    }
    total.clamp(0.0, 1.0)
}

// ─── selection math ─────────────────────────────────────────────────────────

/// Exact pool-then-uniform-pick odds per entry: enumerate every subset of
/// entries whose gates currently pass, weight by that subset's joint
/// probability, split evenly among the subset's members. O(2^n) — capped by
/// [`MAX_EXACT_POOL_ENTRIES`] before calling this (mirrors
/// `tools/lvli_audit.py::compute_pool_odds`).
fn compute_pool_odds(probs: &[f64]) -> Vec<f64> {
    let n = probs.len();
    let mut odds = vec![0.0; n];
    for mask in 1u32..(1u32 << n) {
        let mut p = 1.0;
        let mut k = 0usize;
        for (i, prob) in probs.iter().enumerate() {
            if mask & (1 << i) != 0 {
                p *= prob;
                k += 1;
            } else {
                p *= 1.0 - prob;
            }
        }
        if p == 0.0 {
            continue;
        }
        let share = p / k as f64;
        for (i, odd) in odds.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                *odd += share;
            }
        }
    }
    odds
}

/// Approximate pool odds for more than [`MAX_EXACT_POOL_ENTRIES`] gated
/// entries — each entry's share of the expected passing-pool size. Flagged
/// via [`DropNote::PoolCapped`]; not exact subset enumeration.
fn mean_field_pool_odds(probs: &[f64]) -> Vec<f64> {
    let s: f64 = probs.iter().sum();
    let denom = s.max(1.0);
    probs.iter().map(|p| p / denom).collect()
}

// ─── recursive tree walk ────────────────────────────────────────────────────

struct EligibleEntry<'a> {
    entry: &'a Value,
    gate_prob: f64,
    notes: Vec<DropNote>,
}

#[derive(Clone)]
struct LeafAgg {
    editor_id: String,
    record_type: String,
    p_at_least_one: f64,
    expected_count: f64,
    notes: Vec<DropNote>,
}

struct NodeResult {
    /// Probability this node's invocation yields nothing.
    p_empty: f64,
    leaves: HashMap<FormId, LeafAgg>,
    truncated: bool,
}

#[allow(clippy::too_many_arguments)]
fn merge_leaf(
    map: &mut HashMap<FormId, LeafAgg>,
    fid: FormId,
    editor_id: String,
    record_type: String,
    p_contribution: f64,
    expected_contribution: f64,
    disjoint: bool,
    notes: &[DropNote],
) {
    let agg = map.entry(fid).or_insert_with(|| LeafAgg {
        editor_id,
        record_type,
        p_at_least_one: 0.0,
        expected_count: 0.0,
        notes: Vec::new(),
    });
    agg.expected_count += expected_contribution;
    agg.p_at_least_one = if disjoint {
        // Different entries of the same pool/first-match node are mutually
        // exclusive — at most one fires — so their contributions to the
        // same leaf sum directly rather than combining via independence.
        (agg.p_at_least_one + p_contribution).min(1.0)
    } else {
        1.0 - (1.0 - agg.p_at_least_one) * (1.0 - p_contribution)
    };
    for n in notes {
        if !agg.notes.contains(n) {
            agg.notes.push(n.clone());
        }
    }
}

/// Recursively resolve one LVLI's fields into a [`NodeResult`]. `path`
/// carries every ancestor FormID for cycle detection (push/pop around each
/// recursive call — see call site).
fn walk_node(
    f: &mut impl ChaseFetcher,
    fields: &Value,
    opts: &DropOptions,
    depth: usize,
    path: &mut Vec<FormId>,
) -> anyhow::Result<NodeResult> {
    let entry_vals = entries(fields);
    let flags = lvlf_flags(fields);
    let model = selection_model(&flags);
    let calc_all_levels = flags.contains(CALC_ALL_LEVELS);

    let mut node_notes: Vec<DropNote> = Vec::new();
    for key in [
        "Max Count",
        "Max Global",
        "Max Curve Table",
        "Filter Keyword Chances",
        "Epic Loot Chance",
    ] {
        if fields.get(key).is_some_and(|v| !v.is_null()) {
            node_notes.push(DropNote::Unresolved {
                reason: format!("{key} present on this list — not modeled"),
            });
        }
    }

    // One batched fetch for every GLOB this node's list/entries reference
    // plus every sublist target's own fields (leaf targets need nothing
    // further — their stub already carries editor_id/record_type).
    let mut want: Vec<FormId> = Vec::new();
    if let Some(fid) = stub_formid(fields.get("Chance None Global")) {
        want.push(fid);
    }
    for e in &entry_vals {
        for key in [
            "Chance None Global",
            "Quantity Global",
            "Minimum Level Global",
        ] {
            if let Some(fid) = stub_formid(e.get(key)) {
                want.push(fid);
            }
        }
        if let Some(cond) = e.get("Conditions") {
            collect_condition_refs(cond, &mut want);
        }
        if let Some(target) = entry_target(e)
            && target.get("record_type").and_then(Value::as_str) == Some("LVLI")
            && let Some(fid) = stub_formid(Some(target))
        {
            want.push(fid);
        }
        if e.get("Extra Data").is_some_and(|v| !v.is_null()) {
            node_notes.push(DropNote::Unresolved {
                reason: "Extra Data (COED owner/rank/condition) present — not modeled".to_string(),
            });
        }
    }
    dedup_sorted(&mut want);
    let by_sel = bulk_fetch_map(f, &want)?;

    let list_factor = 1.0 - resolve_chance_none(fields, opts.level, &by_sel);

    let mut eligible: Vec<EligibleEntry> = Vec::new();
    let mut min_levels: Vec<i64> = Vec::new();
    for e in &entry_vals {
        let mut notes = Vec::new();
        if let Some(ml) = resolve_min_level(e, &by_sel, &mut notes) {
            if ml > opts.level {
                continue;
            }
            min_levels.push((ml * 1000.0).round() as i64);
        }
        let gate_prob = match e.get("Conditions") {
            Some(c) => {
                entry_gate_prob(&flatten_condition_rows(c), &by_sel, opts.strict, &mut notes)
            }
            None => 1.0,
        };
        eligible.push(EligibleEntry {
            entry: e,
            gate_prob,
            notes,
        });
    }

    if !calc_all_levels {
        let distinct: HashSet<i64> = min_levels.into_iter().collect();
        if distinct.len() > 1 {
            node_notes.push(DropNote::Unresolved {
                reason: format!(
                    "no \"{CALC_ALL_LEVELS}\" flag and multiple Minimum Level tiers qualify at \
                     level {} — whether FO76 collapses to only the highest tier here is \
                     unverified, so every qualifying tier is shown",
                    opts.level
                ),
            });
        }
    }

    let mut truncated = false;
    let n = eligible.len();
    let chosen: Vec<f64> = match model {
        SelectionModel::UseAll => eligible.iter().map(|e| e.gate_prob).collect(),
        SelectionModel::UseFirstMatch => {
            let mut remaining = 1.0;
            eligible
                .iter()
                .map(|e| {
                    let c = e.gate_prob * remaining;
                    remaining *= 1.0 - e.gate_prob;
                    c
                })
                .collect()
        }
        SelectionModel::Pool if n == 0 => Vec::new(),
        SelectionModel::Pool if n <= MAX_EXACT_POOL_ENTRIES => {
            compute_pool_odds(&eligible.iter().map(|e| e.gate_prob).collect::<Vec<_>>())
        }
        SelectionModel::Pool => {
            truncated = true;
            node_notes.push(DropNote::PoolCapped);
            mean_field_pool_odds(&eligible.iter().map(|e| e.gate_prob).collect::<Vec<_>>())
        }
    };

    let disjoint = !matches!(model, SelectionModel::UseAll);
    let mut node_leaves: HashMap<FormId, LeafAgg> = HashMap::new();
    let mut sum_effective_survive = 0.0_f64;
    let mut prod_all_fail = 1.0_f64;

    for (ee, &chosen_i) in eligible.iter().zip(&chosen) {
        if chosen_i <= 0.0 {
            continue;
        }
        let entry = ee.entry;
        let cn = entry_chance_none(entry, opts.level, &by_sel);
        let effective_i = chosen_i * (1.0 - cn);
        if effective_i <= 0.0 {
            continue;
        }
        let quantity = resolve_quantity(entry, opts.level, &by_sel);

        let Some(target) = entry_target(entry) else {
            continue;
        };
        let Some(target_fid) = stub_formid(Some(target)) else {
            continue;
        };
        if target_fid.raw() == 0 {
            continue;
        }
        let target_rt = target
            .get("record_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let target_edid = target
            .get("editor_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let mut entry_notes = ee.notes.clone();
        let (child_leaves, child_empty) = if target_rt == "LVLI" {
            if path.contains(&target_fid) {
                entry_notes.push(DropNote::Cycle);
                (Vec::new(), 1.0)
            } else if depth >= opts.max_depth {
                entry_notes.push(DropNote::DepthCapped);
                truncated = true;
                (
                    vec![(
                        target_fid,
                        1.0_f64,
                        1.0_f64,
                        target_edid.clone(),
                        target_rt.clone(),
                        Vec::new(),
                    )],
                    0.0,
                )
            } else {
                match by_sel
                    .get(&target_fid.display())
                    .and_then(|e| e.fields.as_ref())
                {
                    Some(sub_fields) => {
                        path.push(target_fid);
                        let child = walk_node(f, sub_fields, opts, depth + 1, path)?;
                        path.pop();
                        truncated |= child.truncated;
                        if quantity != 1.0 {
                            entry_notes.push(DropNote::QuantityOnSublist);
                        }
                        let leaves = child
                            .leaves
                            .into_iter()
                            .map(|(fid, agg)| {
                                (
                                    fid,
                                    agg.p_at_least_one,
                                    agg.expected_count,
                                    agg.editor_id,
                                    agg.record_type,
                                    agg.notes,
                                )
                            })
                            .collect();
                        (leaves, child.p_empty)
                    }
                    None => {
                        entry_notes.push(DropNote::Unresolved {
                            reason: "sublist fetch failed".to_string(),
                        });
                        (Vec::new(), 1.0)
                    }
                }
            }
        } else {
            (
                vec![(
                    target_fid,
                    1.0_f64,
                    1.0_f64,
                    target_edid,
                    target_rt,
                    Vec::new(),
                )],
                0.0,
            )
        };

        for (fid, p_child, exp_child, edid, rt, sub_notes) in &child_leaves {
            let mut all_notes = entry_notes.clone();
            for n in sub_notes {
                if !all_notes.contains(n) {
                    all_notes.push(n.clone());
                }
            }
            merge_leaf(
                &mut node_leaves,
                *fid,
                edid.clone(),
                rt.clone(),
                effective_i * p_child,
                effective_i * quantity * exp_child,
                disjoint,
                &all_notes,
            );
        }

        let survive_i = effective_i * (1.0 - child_empty);
        if disjoint {
            sum_effective_survive += survive_i;
        } else {
            prod_all_fail *= 1.0 - survive_i;
        }
    }

    let p_empty_pre_l = if disjoint {
        (1.0 - sum_effective_survive).clamp(0.0, 1.0)
    } else {
        prod_all_fail.clamp(0.0, 1.0)
    };
    let something_prob = list_factor * (1.0 - p_empty_pre_l);
    let node_p_empty = (1.0 - something_prob).clamp(0.0, 1.0);

    for agg in node_leaves.values_mut() {
        agg.p_at_least_one = (agg.p_at_least_one * list_factor).clamp(0.0, 1.0);
        agg.expected_count *= list_factor;
        for n in &node_notes {
            if !agg.notes.contains(n) {
                agg.notes.push(n.clone());
            }
        }
    }

    Ok(NodeResult {
        p_empty: node_p_empty,
        leaves: node_leaves,
        truncated,
    })
}

/// Resolve `root_formid`'s (already-fetched) `fields` into a [`DropTable`].
/// `root_formid` seeds the cycle-detection path so a list that references
/// itself doesn't recurse forever.
pub fn drop_table(
    f: &mut impl ChaseFetcher,
    root_formid: FormId,
    fields: &Value,
    opts: &DropOptions,
) -> anyhow::Result<DropTable> {
    let model = selection_model(&lvlf_flags(fields));
    let mut path = vec![root_formid];
    let result = walk_node(f, fields, opts, 0, &mut path)?;

    let mut rows: Vec<DropRow> = result
        .leaves
        .into_iter()
        .map(|(fid, agg)| DropRow {
            formid: fid.display(),
            editor_id: agg.editor_id,
            record_type: agg.record_type,
            expected_count: agg.expected_count,
            p_at_least_one: agg.p_at_least_one,
            notes: agg.notes,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.expected_count
            .partial_cmp(&a.expected_count)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.formid.cmp(&b.formid))
    });

    Ok(DropTable {
        model,
        level: opts.level,
        p_nothing: result.p_empty,
        rows,
        truncated: result.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResolveDepth;
    use crate::ipc::{RecordSel, RefList};
    use serde_json::json;

    /// Minimal `ChaseFetcher`: `drop_table` only ever calls `bulk_get`
    /// (never `refs`), and only for GLOB values and sublist LVLI fields —
    /// leaf targets carry everything needed inline on their own stub, so
    /// most tests below never even populate this map.
    struct FakeFetcher {
        records: HashMap<String, Value>,
    }

    impl FakeFetcher {
        fn new() -> Self {
            Self {
                records: HashMap::new(),
            }
        }

        fn insert(&mut self, formid: FormId, fields: Value) {
            self.records.insert(formid.display(), fields);
        }
    }

    impl ChaseFetcher for FakeFetcher {
        fn bulk_get(
            &mut self,
            sels: &[RecordSel],
            _depth: ResolveDepth,
        ) -> anyhow::Result<Vec<BulkRecordEntry>> {
            Ok(sels
                .iter()
                .map(|sel| {
                    let disp = sel.display();
                    match self.records.get(&disp) {
                        Some(fields) => BulkRecordEntry {
                            sel: disp,
                            header: None,
                            editor_id: None,
                            fields: Some(fields.clone()),
                            error: None,
                        },
                        None => BulkRecordEntry {
                            sel: disp,
                            header: None,
                            editor_id: None,
                            fields: None,
                            error: Some("not found".to_string()),
                        },
                    }
                })
                .collect())
        }

        fn refs(
            &mut self,
            _target: FormId,
            _depth: usize,
            _limit: usize,
            _type_filter: &str,
            _paths: bool,
        ) -> anyhow::Result<RefList> {
            unimplemented!("lvli::drop_table never calls refs()")
        }
    }

    fn glob_stub(fid: FormId, edid: &str) -> Value {
        json!({"formid": fid.display(), "editor_id": edid, "record_type": "GLOB"})
    }

    fn target_stub(fid: FormId, rt: &str, edid: &str) -> Value {
        json!({"formid": fid.display(), "editor_id": edid, "record_type": rt})
    }

    fn entry_ref(target: Value) -> Value {
        json!({"Leveled List Entry": {
            "Reference": target,
            "Chance None Value": 0.0,
            "Quantity": 1.0,
            "Minimum Level": 1.0,
        }})
    }

    fn entry_ref_gated(target: Value, operator: &str, cmp: f64) -> Value {
        json!({"Leveled List Entry": {
            "Reference": target,
            "Chance None Value": 0.0,
            "Quantity": 1.0,
            "Minimum Level": 1.0,
            "Conditions": {"Conditions": [{"Condition": {"Condition Data": {
                "Function": "GetRandomPercent",
                "Operator": operator,
                "Comparison Value": cmp,
                "AND/OR": "AND",
                "Run On": "Subject",
            }}}]},
        }})
    }

    fn lvli_fields(flags: &[&str], entries: Vec<Value>) -> Value {
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": flags},
            "Count": entries.len(),
            "Leveled List Entries": entries,
        })
    }

    fn row<'a>(table: &'a DropTable, edid: &str) -> &'a DropRow {
        table
            .rows
            .iter()
            .find(|r| r.editor_id == edid)
            .unwrap_or_else(|| panic!("no row for {edid} in {table:#?}"))
    }

    // ─── pure math ──────────────────────────────────────────────────────

    #[test]
    fn compute_pool_odds_two_certain_entries_split_evenly() {
        let odds = compute_pool_odds(&[1.0, 1.0]);
        assert!((odds[0] - 0.5).abs() < 1e-9);
        assert!((odds[1] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn compute_pool_odds_matches_the_008308d7_regression_fixture() {
        // SCORE_S22_Resources_Collector_SoulSoupServer_Food (0x008308D7): a
        // descending GetRandomPercent >= N ladder with an unconditioned
        // catch-all, no LVLF flags — the exact record this feature was
        // built to answer for. Verified against the real ESM via
        // `esm walk 008308D7`.
        let probs: Vec<f64> = [92.0, 80.0, 63.0, 45.0, 25.0]
            .iter()
            .map(|t| (100.0 - t) / 100.0)
            .chain(std::iter::once(1.0))
            .collect();
        let odds = compute_pool_odds(&probs);
        let expected = [0.0220, 0.0566, 0.1096, 0.1719, 0.2522, 0.3877];
        for (o, e) in odds.iter().zip(expected) {
            assert!((o - e).abs() < 1e-3, "{odds:?} vs {expected:?}");
        }
        let sum: f64 = odds.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn condition_row_prob_reads_lower_and_upper_operator_families() {
        let mut notes = Vec::new();
        let by_sel = HashMap::new();
        let ge = json!({"Function": "GetRandomPercent", "Operator": "Greater Than Or Equal To", "Comparison Value": 92.0});
        assert!((condition_row_prob(&ge, &by_sel, false, &mut notes) - 0.08).abs() < 1e-9);
        let lt = json!({"Function": "GetRandomPercent", "Operator": "Less Than", "Comparison Value": 10.0});
        assert!((condition_row_prob(&lt, &by_sel, false, &mut notes) - 0.10).abs() < 1e-9);
        assert!(notes.is_empty());
    }

    #[test]
    fn condition_row_prob_flags_non_probability_gates_and_respects_strict() {
        let by_sel = HashMap::new();
        let has_recipe = json!({"Function": "HasLearnedRecipe", "Operator": "Equal To", "Comparison Value": 0.0});
        let mut lenient_notes = Vec::new();
        assert_eq!(
            condition_row_prob(&has_recipe, &by_sel, false, &mut lenient_notes),
            1.0
        );
        assert_eq!(lenient_notes.len(), 1);
        let mut strict_notes = Vec::new();
        assert_eq!(
            condition_row_prob(&has_recipe, &by_sel, true, &mut strict_notes),
            0.0
        );
    }

    #[test]
    fn eval_curve_linearly_interpolates_between_points() {
        let v = json!({"curve": [{"x": 0.0, "y": 0.0}, {"x": 100.0, "y": 100.0}]});
        assert!((eval_curve(&v, 50.0).unwrap() - 50.0).abs() < 1e-4);
    }

    #[test]
    fn resolve_chance_none_flat_wins_over_glob() {
        let mut by_sel = HashMap::new();
        let glob_fid = FormId::new(0x1000);
        by_sel.insert(
            glob_fid.display(),
            BulkRecordEntry {
                sel: glob_fid.display(),
                header: None,
                editor_id: None,
                fields: Some(json!({"Value": 85.0})),
                error: None,
            },
        );
        let node = json!({
            "Chance None Value": 10.0,
            "Chance None Global": glob_stub(glob_fid, "SomeGlobal"),
        });
        assert!((resolve_chance_none(&node, 50.0, &by_sel) - 0.10).abs() < 1e-9);

        // Flat 0.0 -> the GLOB is the real chance-none (esm-cli SKILL.md's
        // TWZ07_LL_QuestReward_Event example: flat 0.0, GLOB 85 -> 15% drop).
        let node_zero_flat = json!({
            "Chance None Value": 0.0,
            "Chance None Global": glob_stub(glob_fid, "SomeGlobal"),
        });
        assert!((resolve_chance_none(&node_zero_flat, 50.0, &by_sel) - 0.85).abs() < 1e-9);
    }

    // ─── full tree resolution ───────────────────────────────────────────

    #[test]
    fn drop_table_pool_model_matches_008308d7() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x008308D7);
        let fields = lvli_fields(
            &[],
            vec![
                entry_ref_gated(
                    target_stub(FormId::new(0x1), "ALCH", "BrainFungusVegetableCookedSoup"),
                    "Greater Than Or Equal To",
                    92.0,
                ),
                entry_ref_gated(
                    target_stub(FormId::new(0x2), "ALCH", "SiltBeanVegetableCookedSoup"),
                    "Greater Than Or Equal To",
                    80.0,
                ),
                entry_ref_gated(
                    target_stub(FormId::new(0x3), "ALCH", "SwampPlantTastyTofuSoup"),
                    "Greater Than Or Equal To",
                    63.0,
                ),
                entry_ref_gated(
                    target_stub(FormId::new(0x4), "ALCH", "PumpkinVegetableCookedSoup"),
                    "Greater Than Or Equal To",
                    45.0,
                ),
                entry_ref_gated(
                    target_stub(FormId::new(0x5), "ALCH", "CornVegetableCookedSoup"),
                    "Greater Than Or Equal To",
                    25.0,
                ),
                entry_ref(target_stub(FormId::new(0x6), "ALCH", "FirecapCookedSoup")),
            ],
        );
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        assert_eq!(table.model, SelectionModel::Pool);
        assert!((table.p_nothing).abs() < 1e-9);

        let expected = [
            ("BrainFungusVegetableCookedSoup", 0.0220),
            ("SiltBeanVegetableCookedSoup", 0.0566),
            ("SwampPlantTastyTofuSoup", 0.1096),
            ("PumpkinVegetableCookedSoup", 0.1719),
            ("CornVegetableCookedSoup", 0.2522),
            ("FirecapCookedSoup", 0.3877),
        ];
        let mut total = 0.0;
        for (edid, p) in expected {
            let r = row(&table, edid);
            assert!((r.p_at_least_one - p).abs() < 1e-3, "{edid}: {r:?}");
            // Single edge to a leaf, no chance-none -> expected_count == p_at_least_one.
            assert!((r.expected_count - p).abs() < 1e-3, "{edid}: {r:?}");
            total += r.p_at_least_one;
        }
        assert!((total - 1.0).abs() < 1e-3);
    }

    #[test]
    fn drop_table_use_all_dispenses_every_passing_entry_independently() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x10);
        let fields = lvli_fields(
            &["Use All"],
            vec![
                entry_ref(target_stub(FormId::new(0x11), "MISC", "AlwaysA")),
                entry_ref(target_stub(FormId::new(0x12), "MISC", "AlwaysB")),
            ],
        );
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        assert_eq!(table.model, SelectionModel::UseAll);
        // Both entries are unconditioned -> both always dispensed.
        assert!((row(&table, "AlwaysA").p_at_least_one - 1.0).abs() < 1e-9);
        assert!((row(&table, "AlwaysB").p_at_least_one - 1.0).abs() < 1e-9);
        assert!(table.p_nothing.abs() < 1e-9);
    }

    #[test]
    fn drop_table_use_first_match_is_an_ordered_cascade() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x20);
        // First entry passes 10% of the time and wins outright when it does;
        // the second (unconditioned) entry only fires the other 90%.
        let fields = lvli_fields(
            &["Use First Object That Matches All Conditions"],
            vec![
                entry_ref_gated(
                    target_stub(FormId::new(0x21), "BOOK", "Recipe"),
                    "Less Than Or Equal To",
                    10.0,
                ),
                entry_ref(target_stub(FormId::new(0x22), "WEAP", "Fallback")),
            ],
        );
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        assert_eq!(table.model, SelectionModel::UseFirstMatch);
        assert!((row(&table, "Recipe").p_at_least_one - 0.10).abs() < 1e-9);
        assert!((row(&table, "Fallback").p_at_least_one - 0.90).abs() < 1e-9);
    }

    #[test]
    fn drop_table_flags_2_wins_over_flags_when_xalg_collides() {
        // A record carrying XALG: its own `Flags` land under "Flags" first,
        // so the LVLF union's own flags get renamed to "Flags 2" by
        // `insert_unique` (src/decode/mod.rs). "Item Dispenser" is a real
        // flag name in BOTH vocabularies — this only comes out right if
        // "Flags 2" (when present) is trusted over "Flags" wholesale rather
        // than merging both by name.
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x30);
        let mut fields = lvli_fields(
            &[],
            vec![
                entry_ref(target_stub(FormId::new(0x31), "MISC", "A")),
                entry_ref(target_stub(FormId::new(0x32), "MISC", "B")),
            ],
        );
        fields["Flags"] = json!({"value": "0x10", "flags": ["Item Dispenser"]}); // XALG's own
        fields["Flags 2"] = json!({"value": "0x4", "flags": ["Use All"]}); // LVLF's real flags
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        assert_eq!(table.model, SelectionModel::UseAll);
        assert!((row(&table, "A").p_at_least_one - 1.0).abs() < 1e-9);
        assert!((row(&table, "B").p_at_least_one - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_bridges_legacy_base_data_entries() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x40);
        let legacy_entry = json!({"Leveled List Entry": {
            "Base Data": {
                "Level": 5,
                "Item": target_stub(FormId::new(0x41), "MISC", "OldStyleItem"),
                "Count": 2,
                "Chance None": 20,
            },
        }});
        let fields = lvli_fields(&[], vec![legacy_entry]);
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        let r = row(&table, "OldStyleItem");
        // Sole entry in a pool of one -> always chosen; 20% legacy chance-none.
        assert!((r.p_at_least_one - 0.80).abs() < 1e-9);
        assert!((r.expected_count - 0.80 * 2.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_recurses_and_multiplies_through_a_nested_sublist() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x50);
        let child_fid = FormId::new(0x51);
        f.insert(
            child_fid,
            lvli_fields(
                &[],
                vec![entry_ref(target_stub(FormId::new(0x52), "WEAP", "Leaf"))],
            ),
        );
        let fields = lvli_fields(
            &[],
            vec![entry_ref(target_stub(child_fid, "LVLI", "Sublist"))],
        );
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        let r = row(&table, "Leaf");
        assert_eq!(r.record_type, "WEAP");
        // Sole entry, always chosen; sublist also has a sole always-chosen
        // entry -> compound probability is 1.0, not just the outer edge's.
        assert!((r.p_at_least_one - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_cycle_guard_stops_self_referencing_lists() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x60);
        let fields = lvli_fields(
            &[],
            vec![
                entry_ref(target_stub(root, "LVLI", "SelfReference")),
                entry_ref(target_stub(FormId::new(0x61), "MISC", "RealItem")),
            ],
        );
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        // The self-referencing entry contributes nothing (flagged Cycle,
        // not expanded) rather than looping forever.
        assert!(table.rows.iter().all(|r| r.editor_id != "SelfReference"));
        let real = row(&table, "RealItem");
        assert!((real.p_at_least_one - 0.5).abs() < 1e-9); // pool of 2, one dead
    }

    #[test]
    fn drop_table_minimum_level_excludes_ineligible_entries() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x70);
        let low = json!({"Leveled List Entry": {
            "Reference": target_stub(FormId::new(0x71), "MISC", "LowLevelItem"),
            "Chance None Value": 0.0, "Quantity": 1.0, "Minimum Level": 10.0,
        }});
        let high = json!({"Leveled List Entry": {
            "Reference": target_stub(FormId::new(0x72), "MISC", "HighLevelItem"),
            "Chance None Value": 0.0, "Quantity": 1.0, "Minimum Level": 60.0,
        }});
        let fields = lvli_fields(&[], vec![low, high]);
        let opts = DropOptions {
            level: 50.0,
            ..Default::default()
        };
        let table = drop_table(&mut f, root, &fields, &opts).unwrap();
        assert!(table.rows.iter().any(|r| r.editor_id == "LowLevelItem"));
        assert!(table.rows.iter().all(|r| r.editor_id != "HighLevelItem"));
        // Only one entry was actually eligible -> it's a pool of one.
        assert!((row(&table, "LowLevelItem").p_at_least_one - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_pool_cap_falls_back_to_mean_field_and_flags_it() {
        let mut f = FakeFetcher::new();
        let root = FormId::new(0x80);
        let entries: Vec<Value> = (0..(MAX_EXACT_POOL_ENTRIES + 1) as u32)
            .map(|i| {
                entry_ref(target_stub(
                    FormId::new(0x81 + i),
                    "MISC",
                    &format!("Item{i}"),
                ))
            })
            .collect();
        let fields = lvli_fields(&[], entries);
        let table = drop_table(&mut f, root, &fields, &DropOptions::default()).unwrap();
        assert!(table.truncated);
        assert!(
            table
                .rows
                .iter()
                .all(|r| r.notes.contains(&DropNote::PoolCapped))
        );
        // Mean-field odds should still sum close to 1 across all n unconditioned entries.
        let sum: f64 = table.rows.iter().map(|r| r.p_at_least_one).sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }
}
