//! Leveled-item (LVLI) drop-probability engine.
//!
//! Pure math + fetch logic behind `esm walk`'s LVLI digest ([`crate::walk`]'s
//! `digest_lvli`) — kept as its own module (rather than inlined in
//! `walk/mod.rs`) so another caller can wrap [`drop_table`]
//! without duplicating the selection math, per
//! `docs/adr/0001-walk-interactive-chase-pipeline-json.md`'s "one classifier
//! core, verbs differ in contract" shape. `walk/mod.rs` is the only
//! *renderer* — this module returns structured [`DropTable`]/[`DropRow`]
//! data, never a formatted string.
//!
//! Implements the selection model in `skills/esm-cli/SKILL.md`'s "Drop-chance
//! math (LVLI chains)" section.
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
//! it is not one of the CTDA eligibility gates above (see [`entry_chance_none`]).
//! It, `Quantity` and `Minimum Level` each have a flat value, a Global and a
//! Curve Table, resolved by one rule ([`resolve_scalar`]).
//!
//! ## What isn't modeled (never silently — see [`DropNote::Unresolved`])
//!
//! `Filter Keyword Chances` (LLKC), `Epic Loot Chance` (LVSG), list-level
//! `Max Count`/`Max Global`/`Max Curve Table`, and `Extra Data` (COED)
//! owner/rank gates. FO76's "does `Calculate from all levels <= player's
//! level` being unset collapse multiple qualifying Minimum Level tiers down
//! to just the top one" behavior (classic in older Bethesda engines) is
//! flagged unverified rather than assumed — see [`DropOptions::level`].

use crate::curves::eval as curve_eval;
use crate::fields::{dedup_sorted, flatten_condition_rows};
use crate::source::{RecordSource, bulk_fetch_map};
use crate::{FormId, Resolved};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Default player level assumed for Curve Table evaluation and Minimum
/// Level filtering when `--level` isn't passed.
pub const DEFAULT_LEVEL: f32 = 50.0;

/// Recursion cap for LVLI → LVLI chains (a sublist referencing another
/// sublist). FO76's authored chains run 1-3 deep in practice; this is a
/// safety backstop, not a measured corpus max.
pub const MAX_RECURSION_DEPTH: usize = 8;

/// Above this many entries, [`compute_pool_odds`]'s O(2^n) subset
/// enumeration stops being worth it.
pub const MAX_EXACT_POOL_ENTRIES: usize = 16;

const CALC_ALL_LEVELS: &str = "Calculate from all levels <= player's level";
const USE_ALL: &str = "Use All";
const USE_FIRST_MATCH: &str = "Use First Object That Matches All Conditions";

/// How a node picks among its entries (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub enum SelectionModel {
    Pool,
    UseAll,
    UseFirstMatch,
}

/// A caveat attached to one [`DropRow`] — something about its number that
/// isn't fully modeled or is only approximate. Never silently dropped; see
/// module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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

/// One direct entry of a list in a [`DropTable::tree`]. Odds are per
/// invocation of the *root* list, like `du -d`'s sizes, so a leaf reached by
/// one path matches its [`DropRow`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct DropBranch {
    pub formid: String,
    pub editor_id: String,
    pub record_type: String,
    /// Copies of a leaf item, or items out of a sublist.
    pub expected_count: f64,
    /// Probability this entry yields at least one item.
    pub p_at_least_one: f64,
    /// Caveats specific to this entry (gates, cycles, quantity on a
    /// sublist); list-wide caveats live on [`DropList::notes`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<DropNote>,
    /// Set when the entry is itself an LVLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sublist: Option<DropList>,
}

/// One list in a [`DropTable::tree`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct DropList {
    pub model: SelectionModel,
    /// Probability one invocation of this list yields nothing.
    pub p_nothing: f64,
    /// Distinct leaf items this list can yield, through every nesting level.
    pub item_count: usize,
    /// Caveats that apply to every entry of this list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<DropNote>,
    /// Direct entries sorted by `p_at_least_one` descending; `None` past
    /// [`DropOptions::tree_depth`], where the list is only a subtotal.
    pub entries: Option<Vec<DropBranch>>,
    /// Caveats from inside a collapsed list (`entries: None`), so a subtotal
    /// row still says it's approximate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested_notes: Vec<DropNote>,
}

/// The result of resolving one LVLI's odds, sorted by `expected_count`
/// descending.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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
    /// The same odds as a nesting tree, expanded [`DropOptions::tree_depth`]
    /// levels. `None` when that option is 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<DropList>,
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
    /// Nesting levels [`DropTable::tree`] lists entries for: 1 = the root's
    /// direct entries with sublists as subtotals. 0 = no tree. Independent of
    /// `max_depth`; the flat `rows` always recurse fully.
    #[serde(default)]
    pub tree_depth: usize,
}

impl Default for DropOptions {
    fn default() -> Self {
        Self {
            level: DEFAULT_LEVEL,
            max_depth: MAX_RECURSION_DEPTH,
            strict: false,
            tree_depth: 0,
        }
    }
}

// ─── field access ───────────────────────────────────────────────────────────

fn entries(fields: &Resolved) -> Vec<&Resolved> {
    fields
        .get("Leveled List Entries")
        .map(Resolved::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item.get("Leveled List Entry"))
        .collect()
}

/// `Reference` (form_version >= 174) or the legacy `Base Data.Item`.
fn entry_target(entry: &Resolved) -> Option<&Resolved> {
    entry
        .get("Reference")
        .or_else(|| entry.pointer("/Base Data/Item"))
}

fn is_legacy_entry(entry: &Resolved) -> bool {
    entry.get("Reference").is_none()
}

/// A GLOB reference's own `Value` field, already inlined onto the stub by
/// `--resolve stub` (see `src/decode/leaf_values.rs`) — no separate fetch
/// needed (see `docs/adr/0011-value-bearing-leaf-inlining.md`).
fn glob_stub_value(stub: Option<&Resolved>) -> Option<f64> {
    let obj = stub?.as_object()?;
    if obj.get("record_type").and_then(Resolved::as_str) != Some("GLOB") {
        return None;
    }
    obj.get("Value")?.as_f64()
}

/// A CURV reference's points are inlined onto the field regardless of
/// resolve depth (see `crate::decode::render_formid`'s curve branch) — no
/// fetch needed, just evaluate at `x`. `None` when the curve isn't loaded.
fn eval_curve(v: &Resolved, x: f32) -> Option<f64> {
    let points = crate::curves::points_from_resolved(v)?;
    curve_eval(&points, x).map(f64::from)
}

/// The field names of one leveled-list scalar's three sources.
struct ScalarFields {
    flat: &'static str,
    global: &'static str,
    curve: &'static str,
    /// Whether a curve with no Global reads the player level. Chance None and
    /// Quantity curves used alone are level-shaped (`CT_Creatures_Loot_
    /// WeaponUser_*` over x = 1–50); a Minimum Level curve is tier-indexed
    /// (`MinLevel_*_CT` over 0–3) and always comes with its tier Global.
    curve_reads_level: bool,
}

const CHANCE_NONE: ScalarFields = ScalarFields {
    flat: "Chance None Value",
    global: "Chance None Global",
    curve: "Chance None Curve Table",
    curve_reads_level: true,
};
const QUANTITY: ScalarFields = ScalarFields {
    flat: "Quantity",
    global: "Quantity Global",
    curve: "Quantity Curve Table",
    curve_reads_level: true,
};
const MIN_LEVEL: ScalarFields = ScalarFields {
    flat: "Minimum Level",
    global: "Minimum Level Global",
    // The schema's typo, preserved verbatim.
    curve: "Minimim Level Curve Table",
    curve_reads_level: false,
};

/// One scalar from its flat value, Global and Curve Table: a Curve Table
/// wins, evaluated at its Global's value when a Global is set and at `level`
/// otherwise (see [`ScalarFields::curve_reads_level`]); without one, a Global
/// wins over the flat value.
///
/// A Global beside a Curve Table is the curve's input: on 20260918 every such
/// Global is a tier (`*_ChanceNone_Tier` = 5–25 on `Container_*_ChanceNone`'s
/// x-knots 0/1/5/…/30/100, `MinLvl_*_ECON` = 1–3 on `MinLevel_*_CT`'s 0–3,
/// `ActorTier02`–`12` on `CT_Creatures_Tier_*`'s 1–12). A flat value beside a
/// Global mostly repeats it (`LL_Chems_High_ChanceNone_ECON` = 75 with flat
/// 75); the Global is the tunable source. A source that can't be read (a
/// curve not loaded, a Global naming a non-GLOB) is noted; a curve that
/// can't be evaluated falls back to the flat value, since its Global is its
/// input.
/// `None` when no source is set.
fn resolve_scalar(
    node: &Resolved,
    fields: &ScalarFields,
    level: f32,
    notes: &mut Vec<DropNote>,
) -> Option<f64> {
    let is_set = |v: &&Resolved| v.is_object();
    let global_ref = node.get(fields.global).filter(is_set);
    let global = glob_stub_value(global_ref);
    if let Some(reference) = global_ref
        && global.is_none()
    {
        let name = reference
            .get("editor_id")
            .and_then(Resolved::as_str)
            .unwrap_or("?");
        notes.push(DropNote::Unresolved {
            reason: format!("{} {name} has no GLOB value", fields.global),
        });
    }
    if let Some(curve) = node.get(fields.curve).filter(is_set) {
        // Beside a curve, the Global is the curve's input: without a
        // readable one the input is unknown (an unreadable Global was noted
        // above), and it is never the scalar itself.
        let x = match (global_ref, global) {
            (Some(_), Some(g)) => Some(g as f32),
            (Some(_), None) => None,
            (None, _) => fields.curve_reads_level.then_some(level),
        };
        match x.map(|x| eval_curve(curve, x)) {
            Some(Some(y)) => return Some(y),
            Some(None) => notes.push(DropNote::Unresolved {
                reason: format!("{} isn't loaded", fields.curve),
            }),
            None if global_ref.is_none() => notes.push(DropNote::Unresolved {
                reason: format!(
                    "{} has no Global to read its tier from, so it isn't evaluated",
                    fields.curve
                ),
            }),
            None => {}
        }
        return node.get(fields.flat).and_then(Resolved::as_f64);
    }
    global.or_else(|| node.get(fields.flat).and_then(Resolved::as_f64))
}

/// A list's chance-none as a probability in `[0, 1]` ([`resolve_scalar`]).
fn list_chance_none(fields: &Resolved, level: f32, notes: &mut Vec<DropNote>) -> f64 {
    percent_to_probability(resolve_scalar(fields, &CHANCE_NONE, level, notes))
}

/// An entry's chance-none as a probability in `[0, 1]`: [`resolve_scalar`],
/// or the `Base Data.Chance None` u8 on a legacy entry (which has no Global
/// or curve).
fn entry_chance_none(entry: &Resolved, level: f32, notes: &mut Vec<DropNote>) -> f64 {
    percent_to_probability(if is_legacy_entry(entry) {
        entry
            .pointer("/Base Data/Chance None")
            .and_then(Resolved::as_f64)
    } else {
        resolve_scalar(entry, &CHANCE_NONE, level, notes)
    })
}

fn percent_to_probability(percent: Option<f64>) -> f64 {
    (percent.unwrap_or(0.0) / 100.0).clamp(0.0, 1.0)
}

/// An entry's Minimum Level ([`resolve_scalar`], or `Base Data.Level` on a
/// legacy entry). `None` means no level gate.
fn resolve_min_level(entry: &Resolved, level: f32, notes: &mut Vec<DropNote>) -> Option<f32> {
    if is_legacy_entry(entry) {
        return entry
            .pointer("/Base Data/Level")
            .and_then(Resolved::as_f64)
            .map(|v| v as f32);
    }
    resolve_scalar(entry, &MIN_LEVEL, level, notes).map(|v| v as f32)
}

/// An entry's Quantity ([`resolve_scalar`], or `Base Data.Count` on a legacy
/// entry). `Quantity: 0` means "use the sublist's own count", not disabled —
/// normalized to `1.0` here.
fn resolve_quantity(entry: &Resolved, level: f32, notes: &mut Vec<DropNote>) -> f64 {
    let raw = if is_legacy_entry(entry) {
        entry.pointer("/Base Data/Count").and_then(Resolved::as_f64)
    } else {
        resolve_scalar(entry, &QUANTITY, level, notes)
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
fn lvlf_flags(fields: &Resolved) -> HashSet<String> {
    let key = if fields.get("Flags 2").is_some() {
        "Flags 2"
    } else {
        "Flags"
    };
    fields
        .get(key)
        .and_then(|f| f.get("flags"))
        .and_then(Resolved::as_array)
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
fn condition_row_prob(row: &Resolved, strict: bool, notes: &mut Vec<DropNote>) -> f64 {
    let function = row
        .get("Function")
        .and_then(Resolved::as_str)
        .unwrap_or("?")
        .to_string();
    let fallback = if strict { 0.0 } else { 1.0 };
    if function != "GetRandomPercent" {
        notes.push(DropNote::Gated { function });
        return fallback;
    }
    let operator = row
        .get("Operator")
        .and_then(Resolved::as_str)
        .unwrap_or("?");
    let cmp = match row.get("Comparison Value") {
        Some(v) if v.is_object() => glob_stub_value(Some(v)),
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
fn entry_gate_prob(rows: &[Resolved], strict: bool, notes: &mut Vec<DropNote>) -> f64 {
    if rows.is_empty() {
        return 1.0;
    }
    let mut total = 1.0_f64;
    let mut i = 0;
    while i < rows.len() {
        let mut group_fail = 1.0_f64;
        loop {
            let row = &rows[i];
            let p = condition_row_prob(row, strict, notes);
            group_fail *= 1.0 - p;
            let is_or = row.get("AND/OR").and_then(Resolved::as_str) == Some("OR");
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
/// [`MAX_EXACT_POOL_ENTRIES`] before calling this.
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

// ─── resolve: one list's entries at a level ─────────────────────────────────

/// The record an entry names.
struct Target {
    fid: FormId,
    editor_id: String,
    record_type: String,
}

impl Target {
    /// `None` without a usable, non-null FormID.
    fn of(entry: &Resolved) -> Option<Target> {
        let stub = entry_target(entry)?;
        let fid = stub.stub_id().filter(|fid| fid.raw() != 0)?;
        let text = |key: &str| {
            stub.get(key)
                .and_then(Resolved::as_str)
                .unwrap_or("")
                .to_string()
        };
        Some(Target {
            fid,
            editor_id: text("editor_id"),
            record_type: text("record_type"),
        })
    }

    fn is_sublist(&self) -> bool {
        self.record_type == "LVLI"
    }
}

/// An entry that qualifies at the requested level, with its scalars
/// resolved. An entry without a target still takes part in selection.
struct Eligible {
    target: Option<Target>,
    /// Chance its CTDA gate passes.
    gate_prob: f64,
    chance_none: f64,
    quantity: f64,
    notes: Vec<DropNote>,
}

/// One list resolved at a level: how it selects, what it notes, and the
/// entries that qualify.
struct ResolvedList {
    model: SelectionModel,
    /// Chance the list-level Chance None lets anything through.
    list_factor: f64,
    notes: Vec<DropNote>,
    entries: Vec<Eligible>,
    /// Sublists any entry names (eligible or not), for one batched fetch.
    sublists: Vec<FormId>,
}

fn resolve_list(fields: &Resolved, opts: &DropOptions) -> ResolvedList {
    let flags = lvlf_flags(fields);
    let model = selection_model(&flags);

    let mut notes: Vec<DropNote> = Vec::new();
    for key in [
        "Max Count",
        "Max Global",
        "Max Curve Table",
        "Filter Keyword Chances",
        "Epic Loot Chance",
    ] {
        if fields.get(key).is_some_and(|v| !v.is_null()) {
            notes.push(DropNote::Unresolved {
                reason: format!("{key} present on this list — not modeled"),
            });
        }
    }

    // Leaf targets need nothing further: their stub carries editor_id and
    // record_type (and a GLOB's Value, see `glob_stub_value`).
    let entry_vals = entries(fields);
    let mut sublists: Vec<FormId> = Vec::new();
    for e in &entry_vals {
        if let Some(target) = Target::of(e).filter(Target::is_sublist) {
            sublists.push(target.fid);
        }
        let extra_data = DropNote::Unresolved {
            reason: "Extra Data (COED owner/rank/condition) present — not modeled".to_string(),
        };
        if e.get("Extra Data").is_some_and(|v| !v.is_null()) && !notes.contains(&extra_data) {
            notes.push(extra_data);
        }
    }
    dedup_sorted(&mut sublists);

    let list_factor = 1.0 - list_chance_none(fields, opts.level, &mut notes);

    let mut entries = Vec::new();
    let mut min_levels: Vec<i64> = Vec::new();
    for e in &entry_vals {
        let mut entry_notes = Vec::new();
        if let Some(ml) = resolve_min_level(e, opts.level, &mut entry_notes) {
            if ml > opts.level {
                continue;
            }
            min_levels.push((ml * 1000.0).round() as i64);
        }
        let gate_prob = match e.get("Conditions") {
            Some(c) => entry_gate_prob(&flatten_condition_rows(c), opts.strict, &mut entry_notes),
            None => 1.0,
        };
        let chance_none = entry_chance_none(e, opts.level, &mut entry_notes);
        let quantity = resolve_quantity(e, opts.level, &mut entry_notes);
        entries.push(Eligible {
            target: Target::of(e),
            gate_prob,
            chance_none,
            quantity,
            notes: entry_notes,
        });
    }

    if !flags.contains(CALC_ALL_LEVELS) {
        let distinct: HashSet<i64> = min_levels.into_iter().collect();
        if distinct.len() > 1 {
            notes.push(DropNote::Unresolved {
                reason: format!(
                    "no \"{CALC_ALL_LEVELS}\" flag and multiple Minimum Level tiers qualify at \
                     level {} — whether FO76 collapses to only the highest tier here is \
                     unverified, so every qualifying tier is shown",
                    opts.level
                ),
            });
        }
    }

    ResolvedList {
        model,
        list_factor,
        notes,
        entries,
        sublists,
    }
}

// ─── evaluate: selection odds ───────────────────────────────────────────────

/// The chance each entry is selected under `model`, and whether the pool
/// odds are a mean-field approximation.
fn selection_odds(model: SelectionModel, entries: &[Eligible]) -> (Vec<f64>, bool) {
    let gates: Vec<f64> = entries.iter().map(|e| e.gate_prob).collect();
    match model {
        SelectionModel::UseAll => (gates, false),
        SelectionModel::UseFirstMatch => {
            let mut remaining = 1.0;
            let odds = gates
                .iter()
                .map(|g| {
                    let c = g * remaining;
                    remaining *= 1.0 - g;
                    c
                })
                .collect();
            (odds, false)
        }
        SelectionModel::Pool if gates.is_empty() => (Vec::new(), false),
        SelectionModel::Pool if gates.len() <= MAX_EXACT_POOL_ENTRIES => {
            (compute_pool_odds(&gates), false)
        }
        SelectionModel::Pool => (mean_field_pool_odds(&gates), true),
    }
}

// ─── the recursive walk ─────────────────────────────────────────────────────

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
    model: SelectionModel,
    notes: Vec<DropNote>,
    /// `Some` while tree levels remain (see [`TreeScale`]).
    branches: Option<Vec<DropBranch>>,
}

/// How [`walk_node`] builds [`DropTable::tree`] rows: `levels` still to
/// expand, and the multipliers that turn one invocation of this node into
/// one invocation of the root.
#[derive(Clone, Copy)]
struct TreeScale {
    levels: usize,
    p: f64,
    expected: f64,
}

/// Moves `node.branches` out rather than copying the built subtree.
fn into_drop_list(node: &mut NodeResult) -> DropList {
    let list_notes = node.notes.clone();
    let nested_notes = if node.branches.is_none() {
        let mut nested: Vec<DropNote> = Vec::new();
        let mut leaves: Vec<(&FormId, &LeafAgg)> = node.leaves.iter().collect();
        leaves.sort_by_key(|(fid, _)| fid.raw());
        for (_, agg) in leaves {
            for n in &agg.notes {
                if !list_notes.contains(n) && !nested.contains(n) {
                    nested.push(n.clone());
                }
            }
        }
        nested
    } else {
        Vec::new()
    };
    DropList {
        model: node.model,
        p_nothing: node.p_empty,
        item_count: node.leaves.len(),
        notes: list_notes,
        entries: node.branches.take(),
        nested_notes,
    }
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

/// A leaf item one invocation of a target yields: its chance of at least one
/// and expected count.
struct Reach {
    fid: FormId,
    p: f64,
    expected: f64,
    editor_id: String,
    record_type: String,
    notes: Vec<DropNote>,
}

impl Reach {
    /// A non-list target: itself, once.
    fn leaf(target: &Target) -> Reach {
        Reach {
            fid: target.fid,
            p: 1.0,
            expected: 1.0,
            editor_id: target.editor_id.clone(),
            record_type: target.record_type.clone(),
            notes: Vec::new(),
        }
    }
}

/// Recursively resolve one LVLI's fields into a [`NodeResult`]. `path`
/// carries every ancestor FormID for cycle detection (push/pop around each
/// recursive call — see call site).
fn walk_node(
    f: &mut impl RecordSource,
    fields: &Resolved,
    opts: &DropOptions,
    depth: usize,
    path: &mut Vec<FormId>,
    scale: TreeScale,
) -> anyhow::Result<NodeResult> {
    let list = resolve_list(fields, opts);
    let mut node_notes = list.notes;
    let by_fid = bulk_fetch_map(f, &list.sublists)?;
    let (chosen, mut truncated) = selection_odds(list.model, &list.entries);
    if truncated {
        node_notes.push(DropNote::PoolCapped);
    }
    let list_factor = list.list_factor;

    let disjoint = !matches!(list.model, SelectionModel::UseAll);
    let mut node_leaves: HashMap<FormId, LeafAgg> = HashMap::new();
    let mut sum_effective_survive = 0.0_f64;
    let mut prod_all_fail = 1.0_f64;
    let mut branches: Vec<DropBranch> = Vec::new();

    for (entry, &chosen_i) in list.entries.iter().zip(&chosen) {
        if chosen_i <= 0.0 {
            continue;
        }
        let effective_i = chosen_i * (1.0 - entry.chance_none);
        if effective_i <= 0.0 {
            continue;
        }
        let Some(target) = &entry.target else {
            continue;
        };
        let quantity = entry.quantity;

        let mut entry_notes = entry.notes.clone();
        let reach_p = scale.p * list_factor * effective_i;
        let reach_expected = scale.expected * list_factor * effective_i * quantity;
        let mut sublist: Option<DropList> = None;
        let (reaches, child_empty) = if !target.is_sublist() {
            (vec![Reach::leaf(target)], 0.0)
        } else if path.contains(&target.fid) {
            entry_notes.push(DropNote::Cycle);
            (Vec::new(), 1.0)
        } else if depth >= opts.max_depth {
            entry_notes.push(DropNote::DepthCapped);
            truncated = true;
            (vec![Reach::leaf(target)], 0.0)
        } else {
            match by_fid.get(&target.fid).and_then(|e| e.fields.as_ref()) {
                Some(sub_fields) => {
                    path.push(target.fid);
                    let child_scale = TreeScale {
                        levels: scale.levels.saturating_sub(1),
                        p: reach_p,
                        expected: reach_expected,
                    };
                    let mut child = walk_node(f, sub_fields, opts, depth + 1, path, child_scale)?;
                    path.pop();
                    if scale.levels > 0 {
                        sublist = Some(into_drop_list(&mut child));
                    }
                    truncated |= child.truncated;
                    if quantity != 1.0 {
                        entry_notes.push(DropNote::QuantityOnSublist);
                    }
                    // FormID order: the sums below must not depend on
                    // HashMap iteration order.
                    let mut leaves: Vec<_> = child.leaves.into_iter().collect();
                    leaves.sort_by_key(|(fid, _)| fid.raw());
                    let reaches = leaves
                        .into_iter()
                        .map(|(fid, agg)| Reach {
                            fid,
                            p: agg.p_at_least_one,
                            expected: agg.expected_count,
                            editor_id: agg.editor_id,
                            record_type: agg.record_type,
                            notes: agg.notes,
                        })
                        .collect();
                    (reaches, child.p_empty)
                }
                None => {
                    entry_notes.push(DropNote::Unresolved {
                        reason: "sublist fetch failed".to_string(),
                    });
                    (Vec::new(), 1.0)
                }
            }
        };

        for reach in &reaches {
            let mut all_notes = entry_notes.clone();
            for n in &reach.notes {
                if !all_notes.contains(n) {
                    all_notes.push(n.clone());
                }
            }
            merge_leaf(
                &mut node_leaves,
                reach.fid,
                reach.editor_id.clone(),
                reach.record_type.clone(),
                effective_i * reach.p,
                effective_i * quantity * reach.expected,
                disjoint,
                &all_notes,
            );
        }

        if scale.levels > 0 {
            let child_expected: f64 = reaches.iter().map(|r| r.expected).sum();
            branches.push(DropBranch {
                formid: target.fid.display(),
                editor_id: target.editor_id.clone(),
                record_type: target.record_type.clone(),
                expected_count: reach_expected * child_expected,
                p_at_least_one: (reach_p * (1.0 - child_empty)).clamp(0.0, 1.0),
                notes: entry_notes.clone(),
                sublist,
            });
        }

        let survive_i = effective_i * (1.0 - child_empty);
        if disjoint {
            sum_effective_survive += survive_i;
        } else {
            prod_all_fail *= 1.0 - survive_i;
        }
    }

    // ─── project: the node's odds, leaves and tree rows ─────────────────────
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

    let branches = (scale.levels > 0).then(|| {
        branches.sort_by(|a, b| {
            b.p_at_least_one
                .partial_cmp(&a.p_at_least_one)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.expected_count
                        .partial_cmp(&a.expected_count)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.formid.cmp(&b.formid))
        });
        branches
    });

    Ok(NodeResult {
        p_empty: node_p_empty,
        leaves: node_leaves,
        truncated,
        model: list.model,
        notes: node_notes,
        branches,
    })
}

/// Resolve `root_formid`'s (already-fetched) `fields` into a [`DropTable`].
/// `root_formid` seeds the cycle-detection path so a list that references
/// itself doesn't recurse forever.
pub fn drop_table(
    f: &mut impl RecordSource,
    root_formid: FormId,
    fields: &Resolved,
    opts: &DropOptions,
) -> anyhow::Result<DropTable> {
    let model = selection_model(&lvlf_flags(fields));
    let mut path = vec![root_formid];
    let scale = TreeScale {
        levels: opts.tree_depth,
        p: 1.0,
        expected: 1.0,
    };
    let mut result = walk_node(f, fields, opts, 0, &mut path, scale)?;
    let tree = (opts.tree_depth > 0).then(|| into_drop_list(&mut result));

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
        tree,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(v: &Value) -> Resolved {
        Resolved::from_stub_json(v)
    }
    use crate::source::MemorySource;
    use serde_json::{Value, json};

    fn glob_stub(fid: FormId, edid: &str, value: f64) -> Value {
        json!({"formid": fid.display(), "editor_id": edid, "record_type": "GLOB", "Value": value})
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
        let ge = json!({"Function": "GetRandomPercent", "Operator": "Greater Than Or Equal To", "Comparison Value": 92.0});
        assert!((condition_row_prob(&r(&ge), false, &mut notes) - 0.08).abs() < 1e-9);
        let lt = json!({"Function": "GetRandomPercent", "Operator": "Less Than", "Comparison Value": 10.0});
        assert!((condition_row_prob(&r(&lt), false, &mut notes) - 0.10).abs() < 1e-9);
        assert!(notes.is_empty());
    }

    #[test]
    fn condition_row_prob_flags_non_probability_gates_and_respects_strict() {
        let has_recipe = json!({"Function": "HasLearnedRecipe", "Operator": "Equal To", "Comparison Value": 0.0});
        let mut lenient_notes = Vec::new();
        assert_eq!(
            condition_row_prob(&r(&has_recipe), false, &mut lenient_notes),
            1.0
        );
        assert_eq!(lenient_notes.len(), 1);
        let mut strict_notes = Vec::new();
        assert_eq!(
            condition_row_prob(&r(&has_recipe), true, &mut strict_notes),
            0.0
        );
    }

    #[test]
    fn eval_curve_linearly_interpolates_between_points() {
        let v = json!({"curve": [{"x": 0.0, "y": 0.0}, {"x": 100.0, "y": 100.0}]});
        assert!((eval_curve(&r(&v), 50.0).unwrap() - 50.0).abs() < 1e-4);
    }

    /// A curve over x = 0..100 with y = 100 - x.
    fn falling_curve() -> Value {
        json!({"formid": "0x00001001", "curve": [{"x": 0.0, "y": 100.0}, {"x": 100.0, "y": 0.0}]})
    }

    /// The precedence rule: curve (at the Global, else the level) > Global > flat.
    #[test]
    fn scalar_precedence_table() {
        let global = glob_stub(FormId::new(0x1000), "Some_Tier", 15.0);
        let cases = [
            ("flat only", json!({"Chance None Value": 10.0}), Some(10.0)),
            (
                "Global over flat",
                json!({"Chance None Value": 10.0, "Chance None Global": global}),
                Some(15.0),
            ),
            (
                "curve at the level",
                json!({"Chance None Value": 10.0, "Chance None Curve Table": falling_curve()}),
                Some(50.0),
            ),
            (
                "curve at the Global",
                json!({
                    "Chance None Value": 10.0,
                    "Chance None Global": global,
                    "Chance None Curve Table": falling_curve(),
                }),
                Some(85.0),
            ),
            ("nothing set", json!({}), None),
        ];
        for (label, node, want) in cases {
            let mut notes = Vec::new();
            let got = resolve_scalar(&r(&node), &CHANCE_NONE, 50.0, &mut notes);
            assert_eq!(got.map(|v| (v * 1e6).round() / 1e6), want, "{label}");
            assert!(notes.is_empty(), "{label}: {notes:?}");
        }
    }

    /// Quantity and Minimum Level follow the same rule, including the curve
    /// reading its Global as the x input (a tier, for Minimum Level).
    #[test]
    fn quantity_and_min_level_use_the_same_rule() {
        let tier = glob_stub(FormId::new(0x1000), "MinLvl_Test_ECON", 2.0);
        let tier_curve = json!({"formid": "0x00001002", "curve": [
            {"x": 0.0, "y": 1.0}, {"x": 1.0, "y": 10.0}, {"x": 2.0, "y": 20.0}, {"x": 3.0, "y": 30.0}
        ]});
        let entry = json!({
            "Reference": {"formid": "0x00000001"},
            "Minimum Level": 1.0,
            "Minimum Level Global": tier,
            "Minimim Level Curve Table": tier_curve,
            "Quantity": 1.0,
            "Quantity Global": glob_stub(FormId::new(0x1003), "Reward_Count", 10.0),
        });
        let mut notes = Vec::new();
        assert_eq!(resolve_min_level(&r(&entry), 50.0, &mut notes), Some(20.0));
        assert_eq!(resolve_quantity(&r(&entry), 50.0, &mut notes), 10.0);
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A source that can't be read is noted, and the next one used.
    #[test]
    fn unreadable_sources_are_noted_and_skipped() {
        let avif =
            json!({"formid": "0x00001004", "editor_id": "PerDiem_Limit", "record_type": "AVIF"});
        let unloaded_curve = json!({"formid": "0x00001005", "editor_id": "Some_CT"});
        let mut notes = Vec::new();
        let node = json!({"Quantity": 6.0, "Quantity Global": avif});
        assert_eq!(
            resolve_scalar(&r(&node), &QUANTITY, 50.0, &mut notes),
            Some(6.0)
        );
        let node = json!({"Quantity": 3.0, "Quantity Curve Table": unloaded_curve});
        assert_eq!(
            resolve_scalar(&r(&node), &QUANTITY, 50.0, &mut notes),
            Some(3.0)
        );
        assert_eq!(notes.len(), 2, "{notes:?}");
    }

    /// A curve that can't be read falls back to the flat value, never to the
    /// Global that is its tier input (a `*_Tier` 15 is not a 15% chance).
    #[test]
    fn an_unreadable_curve_falls_back_to_flat_not_its_tier_global() {
        let tier = glob_stub(FormId::new(0x1000), "ItemTwo_Medium_ChanceNone_Tier", 15.0);
        let unloaded = json!({"formid": "0x00001005", "editor_id": "Container_Item2_ChanceNone"});
        let node = json!({
            "Chance None Value": 0.0,
            "Chance None Global": tier,
            "Chance None Curve Table": unloaded,
        });
        let mut notes = Vec::new();
        assert_eq!(
            resolve_scalar(&r(&node), &CHANCE_NONE, 50.0, &mut notes),
            Some(0.0)
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
        // An unreadable Global beside a curve leaves its input unknown: not
        // evaluated at the level either.
        let avif = json!({"formid": "0x00001004", "editor_id": "SomeAV", "record_type": "AVIF"});
        let node = json!({
            "Chance None Value": 5.0,
            "Chance None Global": avif,
            "Chance None Curve Table": falling_curve(),
        });
        let mut notes = Vec::new();
        assert_eq!(
            resolve_scalar(&r(&node), &CHANCE_NONE, 50.0, &mut notes),
            Some(5.0)
        );
    }

    /// Flat 0.0 with a GLOB of 85 is an 85% chance-none (esm-cli SKILL.md's
    /// TWZ07_LL_QuestReward_Event example: a 15% drop).
    #[test]
    fn a_zero_flat_chance_none_defers_to_its_global() {
        let node = json!({
            "Chance None Value": 0.0,
            "Chance None Global": glob_stub(FormId::new(0x1000), "SomeGlobal", 85.0),
        });
        assert!((list_chance_none(&r(&node), 50.0, &mut Vec::new()) - 0.85).abs() < 1e-9);
    }

    // ─── full tree resolution ───────────────────────────────────────────

    #[test]
    fn drop_table_pool_model_matches_008308d7() {
        let mut f = MemorySource::new();
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
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
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
        let mut f = MemorySource::new();
        let root = FormId::new(0x10);
        let fields = lvli_fields(
            &["Use All"],
            vec![
                entry_ref(target_stub(FormId::new(0x11), "MISC", "AlwaysA")),
                entry_ref(target_stub(FormId::new(0x12), "MISC", "AlwaysB")),
            ],
        );
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
        assert_eq!(table.model, SelectionModel::UseAll);
        // Both entries are unconditioned -> both always dispensed.
        assert!((row(&table, "AlwaysA").p_at_least_one - 1.0).abs() < 1e-9);
        assert!((row(&table, "AlwaysB").p_at_least_one - 1.0).abs() < 1e-9);
        assert!(table.p_nothing.abs() < 1e-9);
    }

    #[test]
    fn drop_table_use_first_match_is_an_ordered_cascade() {
        let mut f = MemorySource::new();
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
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
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
        let mut f = MemorySource::new();
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
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
        assert_eq!(table.model, SelectionModel::UseAll);
        assert!((row(&table, "A").p_at_least_one - 1.0).abs() < 1e-9);
        assert!((row(&table, "B").p_at_least_one - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_bridges_legacy_base_data_entries() {
        let mut f = MemorySource::new();
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
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
        let r = row(&table, "OldStyleItem");
        // Sole entry in a pool of one -> always chosen; 20% legacy chance-none.
        assert!((r.p_at_least_one - 0.80).abs() < 1e-9);
        assert!((r.expected_count - 0.80 * 2.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_recurses_and_multiplies_through_a_nested_sublist() {
        let mut f = MemorySource::new();
        let root = FormId::new(0x50);
        let child_fid = FormId::new(0x51);
        f.insert(
            child_fid,
            "LVLI",
            "",
            0,
            lvli_fields(
                &[],
                vec![entry_ref(target_stub(FormId::new(0x52), "WEAP", "Leaf"))],
            ),
        );
        let fields = lvli_fields(
            &[],
            vec![entry_ref(target_stub(child_fid, "LVLI", "Sublist"))],
        );
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
        let r = row(&table, "Leaf");
        assert_eq!(r.record_type, "WEAP");
        // Sole entry, always chosen; sublist also has a sole always-chosen
        // entry -> compound probability is 1.0, not just the outer edge's.
        assert!((r.p_at_least_one - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_cycle_guard_stops_self_referencing_lists() {
        let mut f = MemorySource::new();
        let root = FormId::new(0x60);
        let fields = lvli_fields(
            &[],
            vec![
                entry_ref(target_stub(root, "LVLI", "SelfReference")),
                entry_ref(target_stub(FormId::new(0x61), "MISC", "RealItem")),
            ],
        );
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
        // The self-referencing entry contributes nothing (flagged Cycle,
        // not expanded) rather than looping forever.
        assert!(table.rows.iter().all(|r| r.editor_id != "SelfReference"));
        let real = row(&table, "RealItem");
        assert!((real.p_at_least_one - 0.5).abs() < 1e-9); // pool of 2, one dead
    }

    #[test]
    fn drop_table_minimum_level_excludes_ineligible_entries() {
        let mut f = MemorySource::new();
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
        let table = drop_table(&mut f, root, &r(&fields), &opts).unwrap();
        assert!(table.rows.iter().any(|r| r.editor_id == "LowLevelItem"));
        assert!(table.rows.iter().all(|r| r.editor_id != "HighLevelItem"));
        // Only one entry was actually eligible -> it's a pool of one.
        assert!((row(&table, "LowLevelItem").p_at_least_one - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drop_table_pool_cap_falls_back_to_mean_field_and_flags_it() {
        let mut f = MemorySource::new();
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
        let table = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
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

    // ─── nesting tree ───────────────────────────────────────────────────

    fn sum_tree_leaves(entries: &[DropBranch], out: &mut HashMap<String, f64>) {
        for b in entries {
            match b.sublist.as_ref().and_then(|l| l.entries.as_deref()) {
                Some(children) => {
                    let child_sum: f64 = children.iter().map(|c| c.expected_count).sum();
                    assert!(
                        (b.expected_count - child_sum).abs() < 1e-9,
                        "{} subtotal {} != its entries' {child_sum}",
                        b.editor_id,
                        b.expected_count
                    );
                    sum_tree_leaves(children, out);
                }
                None => *out.entry(b.formid.clone()).or_default() += b.expected_count,
            }
        }
    }

    /// Three levels with every scaling factor away from 1: list and entry
    /// chance-none, quantity on a sublist and a leaf, a gated pool, Use All,
    /// and a cycle back to the root.
    fn three_level_fixture() -> (MemorySource, FormId, Value) {
        let (root, mid, deep) = (FormId::new(0x60), FormId::new(0x61), FormId::new(0x62));
        let mut f = MemorySource::new();
        f.insert(
            deep,
            "LVLI",
            "",
            0,
            lvli_fields(
                &[],
                vec![
                    entry_ref_gated(
                        target_stub(FormId::new(0x70), "WEAP", "Z"),
                        "Greater Than Or Equal To",
                        50.0,
                    ),
                    entry_ref(target_stub(FormId::new(0x71), "WEAP", "W")),
                ],
            ),
        );
        let mut y = entry_ref(target_stub(FormId::new(0x72), "ALCH", "Y"));
        y["Leveled List Entry"]["Quantity"] = json!(3.0);
        f.insert(
            mid,
            "LVLI",
            "",
            0,
            lvli_fields(
                &["Use All"],
                vec![
                    y,
                    entry_ref(target_stub(deep, "LVLI", "Deep")),
                    entry_ref(target_stub(root, "LVLI", "Root")),
                ],
            ),
        );
        let mut to_mid = entry_ref(target_stub(mid, "LVLI", "Mid"));
        to_mid["Leveled List Entry"]["Chance None Value"] = json!(50.0);
        to_mid["Leveled List Entry"]["Quantity"] = json!(2.0);
        let mut root_fields = lvli_fields(
            &[],
            vec![
                to_mid,
                entry_ref(target_stub(FormId::new(0x73), "MISC", "X")),
            ],
        );
        root_fields["Chance None Value"] = json!(25.0);
        (f, root, root_fields)
    }

    #[test]
    fn tree_leaves_and_subtotals_match_flat_rows_at_full_depth() {
        let (mut f, root, fields) = three_level_fixture();
        let opts = DropOptions {
            tree_depth: MAX_RECURSION_DEPTH,
            ..Default::default()
        };
        let table = drop_table(&mut f, root, &r(&fields), &opts).unwrap();
        let tree = table.tree.as_ref().unwrap();
        let mut sums = HashMap::new();
        sum_tree_leaves(tree.entries.as_ref().unwrap(), &mut sums);
        for r in &table.rows {
            let t = sums.get(&r.formid).copied().unwrap_or(0.0);
            assert!(
                (t - r.expected_count).abs() < 1e-9,
                "{}: tree {t} vs flat {}",
                r.editor_id,
                r.expected_count
            );
        }
        assert_eq!(table.rows.len(), 4);
        assert!((row(&table, "Y").expected_count - 0.75 * 0.5 * 0.5 * 2.0 * 3.0).abs() < 1e-9);
    }

    #[test]
    fn tree_depth_one_collapses_sublists_and_zero_omits_the_tree() {
        let (mut f, root, fields) = three_level_fixture();
        let opts = DropOptions {
            tree_depth: 1,
            ..Default::default()
        };
        let table = drop_table(&mut f, root, &r(&fields), &opts).unwrap();
        let entries = table.tree.as_ref().unwrap().entries.as_ref().unwrap();
        let mid = entries.iter().find(|b| b.editor_id == "Mid").unwrap();
        let mid_list = mid.sublist.as_ref().unwrap();
        assert!(mid_list.entries.is_none());
        assert_eq!(mid_list.item_count, 3);
        let flat: f64 = ["Y", "Z", "W"]
            .iter()
            .map(|e| row(&table, e).expected_count)
            .sum();
        assert!((mid.expected_count - flat).abs() < 1e-9);

        let no_tree = drop_table(&mut f, root, &r(&fields), &DropOptions::default()).unwrap();
        assert!(no_tree.tree.is_none());
        assert!(
            serde_json::to_value(&no_tree)
                .unwrap()
                .get("tree")
                .is_none()
        );
    }
}
