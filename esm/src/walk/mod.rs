//! The sole interactive record surface (see
//! `docs/adr/0001-walk-interactive-chase-pipeline-json.md`): computes one
//! compact per-record-type **digest** ([`Digest`]) of an ESM record and the
//! chain it references, instead of a series of raw `esm get` dumps. Accepts
//! any record type.
//!
//! ```text
//! esm --esm <path> walk <formid|edid> [--refs] [--depth N] [--ref-limit N] [--json]
//! ```
//!
//! This module computes only — [`digest_node`] and its per-type `digest_*`
//! helpers build typed [`Digest`] payloads (real values: FormID ref stubs,
//! numbers, classified [`crate::chase::Hop`]s, ...), never pre-formatted
//! prose. Turning a [`Digest`] into printed lines is [`render`]'s job — see
//! that submodule for the human-text renderer `--json` output serializes the
//! `Digest` values directly, so a consumer gets the same computed data this
//! module built, not a screen-scrape of a rendered line.
//!
//! [`walk`] does a breadth-first traversal (queue + visited set keyed on
//! FormID, depth-capped) starting from one resolved record, computing a
//! record-type-specific digest for each node and enqueueing whatever's
//! worth following one hop further (a magic effect's granted Perk/Equip
//! Ability, a PERK's Ability SPEL, an OMOD's ENCH property, ...). It
//! composes the same two primitives
//! `esm::chase`'s "chase pattern" uses — [`ChaseFetcher::bulk_get`] and
//! [`ChaseFetcher::refs`] — no new trait, no new wire `Op`.
//!
//! **OMOD roots get more than a forward-only digest.** [`digest_node`]'s
//! `"OMOD"` arm calls straight into `esm::chase`'s mechanism classifier
//! ([`crate::chase::omod_chase`]) on the already-fetched root — no redundant
//! re-fetch — and its classified `Data.Properties[]` hops become
//! [`OmodDigest::hops`] directly (the same [`crate::chase::Hop`] type
//! `esm chase`'s JSON emits — see [`render`] for how `render_omod_hops`
//! turns each mechanism into path-sliced evidence lines: a keyword/AVIF hook
//! names the gating SPEL/PERK consumer and shows only the gated
//! `Effects[N]` rows it triggers (never the consumer's full digest), a perk
//! grant or direct SPEL/ENCH/PROJ attachment shows the granted record's own
//! effects, and an MGEF pass-through ("Perk to Apply"/"Equip Ability") is
//! named the same way `chase` names it). This mechanism slice runs
//! unconditionally as part of the root digest — `--depth` only governs BFS
//! *enqueueing*, not whether the root's own mechanisms get classified. A
//! directly-attached ENCH or PROJ property is also enqueued as its own BFS
//! node (see [`omod_hops_enqueue`]), so an OMOD → ENCH → MGEF →
//! granted-perk chain still lands in one `walk` call. `Data.Includes[]`
//! stubs (the `_PARENT_*` empty-shell OMOD pattern) are named too, straight
//! off the already-stub-resolved fields — zero extra
//! fetches.
//!
//! **LVLI roots get resolved drop odds, not a raw field dump.** [`digest_node`]'s
//! `"LVLI"` arm calls [`crate::lvli::drop_table`] (pool/`Use All`/`Use First
//! Match` selection math, flat-vs-GLOB-vs-Curve-Table chance-none, recursion
//! through nested sublists to leaf items) and [`LvliDigest`] wraps the
//! resulting [`crate::lvli::DropTable`] verbatim — see `crate::lvli`'s
//! module docs for the mechanics and what isn't modeled. `--level` (default
//! [`crate::lvli::DEFAULT_LEVEL`]) feeds Curve Table evaluation and Minimum
//! Level filtering. A direct sublist entry is also enqueued as its own BFS
//! node so an intermediate list stays inspectable, even though the
//! aggregated table already flattens through it.
//!
//! Every record is fetched at [`ResolveDepth::Stub`], so every direct FormID
//! reference on a fetched record's own fields already arrives pre-annotated
//! as `{"formid", "editor_id", "record_type"}` (the same annotation
//! `esm get --resolve stub` produces) — no follow-up per-reference fetch
//! needed. One exception remains: a GLOB *reference*'s own `Value` field
//! isn't expanded by Stub resolution (which only annotates the *reference*,
//! not the referenced record's fields), so magnitude/duration/condition
//! GLOB annotations still require one batched extra `bulk_get` (see
//! [`resolve_glob_ref`]).
//!
//! Two responsibilities stay with the caller (`cmd_walk` in
//! `src/bin/cli.rs`) rather than living in this module, since neither fits
//! through `ChaseFetcher`'s narrow bulk_get/refs-with-type-filter seam:
//! - **not-found → search fallback**: when the root selector doesn't
//!   resolve, [`walk`] returns a [`WalkResult`] with [`WalkResult::not_found`]
//!   set and an empty `matches` list; the CLI driver runs one `Op::Search`
//!   and fills `matches` in before rendering.
//! - **`--refs` reverse-reference summary**: needs an *unfiltered* reverse
//!   `refs` walk (every referencing record type, not just SPEL/PERK), which
//!   `ChaseFetcher::refs`'s mandatory type-filter parameter can't express.
//!   The CLI driver runs one unfiltered `Op::ReferencedBy` call and passes
//!   the raw rows to [`build_refs_digest`] (a pure function, easily unit
//!   tested without any fetcher).

mod render;

pub use render::{render_digest, render_text};

use crate::chase::{
    ChaseFetcher, ChaseOptions, Hop, HopKind, RootStub, consumer_refs_by_type, omod_chase,
    summarize_explosion_detail,
};
use crate::ipc::RecordSel;
use crate::{BulkRecordEntry, FormId, RecordRow, RefRow, ResolveDepth};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

/// Default BFS depth cap.
pub const DEFAULT_DEPTH: usize = 2;

/// Reverse-ref walk depth/cap for the KYWD/AVIF "who gates on this?" digest.
/// [`render::CONSUMER_ROWS_SHOWN`] (a *display* cap) further
/// trims the fetched rows at render time; [`ConsumerGroup::total`] preserves
/// the true count either way.
const CONSUMER_REF_DEPTH: usize = 1;
const CONSUMER_REF_LIMIT: usize = 10;

/// Record types whose direct references to a target count as a "player-facing
/// signal" in [`build_refs_digest`].
const OBTAINABLE_TYPES: [&str; 7] = ["COBJ", "GMRW", "LGDI", "QUST", "CONT", "MISC", "FLST"];

/// Model/render/sound noise that never matters for damage or obtainability —
/// dropped from the generic fallback digest.
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
    /// root's own consumer digest and is left as-is.
    pub ref_limit: usize,
    /// Player level assumed by an LVLI root's drop-odds digest (see
    /// [`crate::lvli::DropOptions::level`]) — Minimum Level filtering and
    /// Curve Table evaluation both key off it. Unused by every other digest.
    pub level: f32,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            ref_limit: crate::chase::DEFAULT_REF_LIMIT,
            level: crate::lvli::DEFAULT_LEVEL,
        }
    }
}

/// The walk's output: either a not-found report (root selector didn't
/// resolve) or the BFS node list, plus an optional `--refs` digest for the
/// root. Kept as a flat struct with skip-if-empty/None fields (rather than an
/// enum) so `--json` output stays a single flat object — see `esm::chase`'s
/// `ChaseTree` for the same convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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
/// searches (see module docs); `Op::Walk`'s dispatch (or, pre-D4, the CLI
/// driver) fills it in via one search call before rendering/serializing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct NotFound {
    pub target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<RecordRow>,
}

/// One BFS-visited record: identity plus its record-type-specific computed
/// [`Digest`]. `--json` serializes `digest` as real structured data (an
/// externally-tagged `{"kind": "...", ...}` object, one shape per record
/// type); the plain-text CLI path renders it via [`render::render_digest`],
/// two-space-indented relative to the node header (matching the TS
/// original's `emit(2, ...)` sub-bullets) — see [`render::render_text`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct WalkNode {
    pub depth: usize,
    pub sig: String,
    pub formid: String,
    pub editor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub digest: Digest,
}

/// One record-type group in the `--refs` summary (see [`build_refs_digest`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefsDigest {
    pub groups: Vec<RefsDigestGroup>,
}

// ─── the digest data model ──────────────────────────────────────────────────
//
// One variant per record type `digest_node` dispatches on. Each payload
// holds the real values the corresponding `digest_*` function computes —
// FormID ref stubs (already Stub-annotated), numbers, classified
// `chase::Hop`s — never a pre-formatted line. `render::render_digest` is the
// only place that turns one of these into printed text.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Digest {
    Glob(GlobDigest),
    Avif(AvifDigest),
    Kywd(KywdDigest),
    Mgef(MgefDigest),
    MagicItem(MagicItemDigest),
    Perk(PerkDigest),
    Weap(WeapDigest),
    Proj(ProjDigest),
    Expl(ExplDigest),
    Lvli(LvliDigest),
    Omod(OmodDigest),
    /// Fallback for every record type without a dedicated digest: the
    /// trimmed field tree (see [`trim_generic_fields`]), not a rigid struct
    /// — this record class is genuinely variable-shape, matching the plan's
    /// carve-out for exactly this case.
    Generic(GenericDigest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct GlobDigest {
    #[cfg_attr(test, ts(type = "unknown"))]
    pub value: Option<Value>,
}

/// One record-type group of reverse-chased KYWD/AVIF consumers (shared by
/// [`AvifDigest`] and [`KywdDigest`] — see [`digest_keyword_or_av`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ConsumerGroup {
    pub record_type: String,
    /// Every consumer fetched, bounded by [`CONSUMER_REF_LIMIT`].
    pub rows: Vec<ConsumerRow>,
    /// Pre-truncation count from the underlying `RefList` — may exceed
    /// `rows.len()`, letting a renderer show "+N more" with no extra fetch.
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ConsumerRow {
    pub formid: String,
    pub editor_id: String,
    /// The first `--paths` field path this consumer's reference was found
    /// at, if any.
    pub via: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct AvifDigest {
    pub abbreviation: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub default_value: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub maximum_value: Option<Value>,
    pub consumers: Vec<ConsumerGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct KywdDigest {
    pub consumers: Vec<ConsumerGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct MgefDigest {
    pub archetype: Option<String>,
    pub casting_type: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub target_av: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub resist_av: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub perk_to_apply: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub equip_ability: Option<Value>,
    pub description: Option<String>,
}

/// One `Effects[]` entry of a [`MagicItemDigest`] (SPEL/ENCH/ALCH share this
/// identical shape). `conditions` rows are already GLOB-resolved (see
/// [`resolve_condition_row`]) so `render` needs no fetcher of its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct MagicEffectRow {
    pub index: usize,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub base_effect: Option<Value>,
    pub archetype: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub actor_value: Option<Value>,
    /// `Effect Item Data.Magnitude`, raw (defaults to `0` when absent).
    #[cfg_attr(test, ts(type = "unknown"))]
    pub magnitude: Value,
    /// `Effect Item Data.Duration`, raw (defaults to `0` when absent).
    #[cfg_attr(test, ts(type = "unknown"))]
    pub duration: Value,
    /// A sibling top-level `Magnitude` GLOB reference, if present — distinct
    /// from `magnitude` above (see module docs on the two "Magnitude"
    /// fields). GLOB-resolved (carries `"resolved_value"` when the ref is a
    /// GLOB) via [`resolve_glob_ref`].
    #[cfg_attr(test, ts(type = "unknown"))]
    pub magnitude_glob: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub duration_glob: Option<Value>,
    /// The raw `Curve Table` field (points + `curve_path`), if present.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub curve_table: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub curve_input_av: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub conditions: Vec<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub perk_to_apply: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub equip_ability: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct MagicItemDigest {
    pub effects: Vec<MagicEffectRow>,
}

/// One classified `Effects[]` entry of a [`PerkDigest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PerkEffectRow {
    Ability {
        index: usize,
        #[cfg_attr(test, ts(type = "unknown"))]
        target: Value,
    },
    EntryPoint {
        index: usize,
        entry_point_name: Option<String>,
        function_name: Option<String>,
        #[cfg_attr(test, ts(type = "unknown"))]
        float_value: Option<Value>,
        #[cfg_attr(test, ts(type = "unknown"))]
        actor_value: Option<Value>,
        #[cfg_attr(test, ts(type = "unknown"))]
        conditions: Vec<Value>,
    },
    /// An effect type neither `Ability` nor `Entry Point` names — rendered
    /// bare.
    Other { index: usize, type_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct PerkDigest {
    pub description: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub num_ranks: Option<Value>,
    pub playable: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub next_perk: Option<Value>,
    /// `None` when the record has no `Effects[]` array at all — the bonus is
    /// engine/script-side (description only).
    pub effects: Option<Vec<PerkEffectRow>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct WeapDigest {
    /// `WeaponType*`/`HasLegendary*`/`ma_*`-prefixed keywords only.
    pub relevant_keywords: Vec<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub ap_cost: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub speed: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub reload_speed: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub eligible_levels: Vec<Value>,
    pub attach_slots: usize,
    pub has_object_template: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ProjDigest {
    pub proj_type: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub speed: Option<Value>,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub explosion: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ExplDigest {
    /// `chase::summarize_explosion_detail`'s already-structured output
    /// (radius/force/stagger/impact/chain/damage) — reused verbatim rather
    /// than recomputed, the same detail an OMOD's PROJ mechanism evidence
    /// carries.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct LvliDigest {
    pub table: crate::lvli::DropTable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct OmodDigest {
    /// Classified `Data.Properties[]` rows from `esm::chase`'s mechanism
    /// classifier — the same [`Hop`] type `esm chase`'s `ChaseTree` JSON
    /// emits, reused directly rather than re-derived. A directly-attached
    /// ENCH property (like a PROJ) renders via this list's own
    /// `DirectProperty` hop rather than a separate ENCH-follow pass — see
    /// [`omod_hops_enqueue`].
    pub hops: Vec<Hop>,
    /// `Data.Includes[]` targets, capped at [`OMOD_INCLUDE_ENQUEUE_CAP`].
    #[cfg_attr(test, ts(type = "unknown"))]
    pub includes: Vec<Value>,
    /// Pre-cap count of valid `Data.Includes[]` rows.
    pub includes_total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct GenericDigest {
    #[cfg_attr(test, ts(type = "unknown"))]
    pub trimmed: Value,
}

// ─── generic JSON helpers ───────────────────────────────────────────────────

/// A decoded FormID reference at [`ResolveDepth::Stub`] is a
/// `{"formid", "editor_id", "record_type"}` object (see module docs).
fn is_ref_stub(v: &Value) -> bool {
    matches!(v, Value::Object(map) if map.contains_key("formid"))
}

pub(crate) fn stub_formid(v: Option<&Value>) -> Option<FormId> {
    let obj = v?.as_object()?;
    let s = obj.get("formid")?.as_str()?;
    crate::parse_form_id_input(s).ok()
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

pub(crate) fn dedup_sorted(fids: &mut Vec<FormId>) {
    fids.sort_by_key(|f| f.0);
    fids.dedup();
}

/// Batch-fetch `fids` at [`ResolveDepth::Stub`] and return them keyed by their
/// display-formid string (matches `BulkRecordEntry::sel` for a
/// `RecordSel::FormId` selector — see `esm::chase`'s identical `by_sel`
/// pattern).
pub(crate) fn bulk_fetch_map(
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

/// When `v` is a formid-ref-stub pointing at a GLOB, clone it and inject the
/// GLOB's own resolved `Value` field under the extra key `"resolved_value"`
/// — computed once here, at digest-build time, so `walk::render`'s
/// formatters need no fetcher/by_sel map of their own (mirrors the existing
/// convention of Stub resolution itself annotating a ref inline). Non-GLOB
/// refs (and non-refs) pass through unchanged.
fn resolve_glob_ref(by_sel: &HashMap<String, BulkRecordEntry>, v: &Value) -> Value {
    let mut out = v.clone();
    if let Value::Object(map) = &mut out {
        let is_glob = map.get("record_type").and_then(Value::as_str) == Some("GLOB");
        if is_glob {
            let fid = map
                .get("formid")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let value = by_sel
                .get(&fid)
                .and_then(|e| e.fields.as_ref())
                .and_then(|flds| flds.get("Value"))
                .cloned();
            if let Some(value) = value {
                map.insert("resolved_value".to_string(), value);
            }
        }
    }
    out
}

/// Resolve every GLOB-valued `Parameter 1`/`Comparison Value` operand on one
/// raw condition row (see [`resolve_glob_ref`]) so the row is a
/// self-contained computed value.
fn resolve_condition_row(by_sel: &HashMap<String, BulkRecordEntry>, row: &Value) -> Value {
    let mut out = row.clone();
    if let Value::Object(map) = &mut out {
        for key in ["Parameter 1", "Comparison Value"] {
            if let Some(v) = map.get(key)
                && is_ref_stub(v)
            {
                let resolved = resolve_glob_ref(by_sel, v);
                map.insert(key.to_string(), resolved);
            }
        }
    }
    out
}

// ─── conditions ─────────────────────────────────────────────────────────────

/// Pull the flat condition rows out of a SPEL/ENCH/ALCH/MGEF-style
/// `Conditions` node. LVLI entries decode `Conditions` into this identical shape (see
/// `crate::lvli`), so this is shared rather than duplicated.
pub(crate) fn flatten_condition_rows(node: &Value) -> Vec<Value> {
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
/// `"Target"`.
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

/// Collect every GLOB/other FormID reference nested inside `conditions_node`
/// so callers can batch-prefetch GLOB `Value`s before rendering.
pub(crate) fn collect_condition_refs(conditions_node: &Value, out: &mut Vec<FormId>) {
    collect_ref_formids(conditions_node, out);
}

// ─── per-type digests ───────────────────────────────────────────────────────

fn digest_glob(fields: &Value) -> GlobDigest {
    GlobDigest {
        value: fields.get("Value").cloned(),
    }
}

fn digest_avif(
    f: &mut impl ChaseFetcher,
    formid: FormId,
    fields: &Value,
) -> anyhow::Result<AvifDigest> {
    Ok(AvifDigest {
        abbreviation: fields
            .get("Abbreviation")
            .and_then(Value::as_str)
            .map(str::to_string),
        default_value: fields.get("Default Value").cloned(),
        maximum_value: fields.get("Maximum Value").cloned(),
        consumers: digest_keyword_or_av(f, formid)?,
    })
}

fn digest_kywd(f: &mut impl ChaseFetcher, formid: FormId) -> anyhow::Result<KywdDigest> {
    Ok(KywdDigest {
        consumers: digest_keyword_or_av(f, formid)?,
    })
}

/// KYWD/AVIF records carry no behavior themselves — they're read by whichever
/// SPEL/PERK gates an effect on them. Reverse-`refs --type ... --paths` finds
/// those consumers and the exact field path each one gates through.
fn digest_keyword_or_av(
    f: &mut impl ChaseFetcher,
    formid: FormId,
) -> anyhow::Result<Vec<ConsumerGroup>> {
    let grouped = consumer_refs_by_type(f, formid, CONSUMER_REF_DEPTH, CONSUMER_REF_LIMIT)?;
    let mut groups = Vec::new();
    for (record_type, ref_list) in grouped {
        if ref_list.rows.is_empty() {
            continue;
        }
        let total = ref_list.total;
        let rows = ref_list
            .rows
            .into_iter()
            .map(|r| ConsumerRow {
                formid: r.form_id,
                editor_id: r.editor_id.unwrap_or_default(),
                via: r.field_paths.and_then(|p| p.into_iter().next()),
            })
            .collect();
        groups.push(ConsumerGroup {
            record_type: record_type.to_string(),
            rows,
            total,
        });
    }
    Ok(groups)
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

fn digest_mgef(fields: &Value, enqueue: &mut Vec<EnqueueTarget>) -> MgefDigest {
    let summary = mgef_summary(fields);
    if let Some(fid) = summary.perk_to_apply.and_then(|p| stub_formid(Some(p))) {
        enqueue.push((fid, "Perk to Apply".to_string()));
    }
    if let Some(fid) = summary.equip_ability.and_then(|eq| stub_formid(Some(eq))) {
        enqueue.push((fid, "Equip Ability".to_string()));
    }
    MgefDigest {
        archetype: summary.archetype.map(str::to_string),
        casting_type: summary.casting_type.map(str::to_string),
        target_av: summary.actor_value.cloned(),
        resist_av: summary.resist_value.cloned(),
        perk_to_apply: summary.perk_to_apply.cloned(),
        equip_ability: summary.equip_ability.cloned(),
        description: summary
            .description
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

/// SPEL/ENCH/ALCH share an identical `Effects[]` shape: per effect, a `Base
/// Effect` -> MGEF, a flat `Effect Item Data.Magnitude`/`.Duration`, an
/// optional sibling GLOB `Magnitude`/`Duration`, an optional `Curve Table` +
/// `Actor Value` input axis, `Conditions`, and the MGEF's own one-hop
/// `Perk to Apply`/`Equip Ability`.
fn digest_magic_item(
    f: &mut impl ChaseFetcher,
    fields: &Value,
    enqueue: &mut Vec<EnqueueTarget>,
) -> anyhow::Result<MagicItemDigest> {
    let empty = Vec::new();
    let effects = fields
        .get("Effects")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if effects.is_empty() {
        return Ok(MagicItemDigest {
            effects: Vec::new(),
        });
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

    let mut rows = Vec::with_capacity(effects.len());
    for (i, item) in effects.iter().enumerate() {
        let Some(e) = item.get("Effect") else {
            continue;
        };
        let base_effect = e.get("Base Effect").cloned();
        let mgef_fields = base_effect
            .as_ref()
            .and_then(|b| stub_formid(Some(b)))
            .and_then(|fid| by_sel.get(&fid.display()))
            .and_then(|entry| entry.fields.as_ref());
        let summary = mgef_fields.map(mgef_summary);
        let archetype = summary
            .as_ref()
            .and_then(|s| s.archetype)
            .map(str::to_string);
        let actor_value = summary.as_ref().and_then(|s| s.actor_value).cloned();

        let item_data = e.get("Effect Item Data");
        let magnitude = item_data
            .and_then(|d| d.get("Magnitude"))
            .cloned()
            .unwrap_or(json!(0));
        let duration = item_data
            .and_then(|d| d.get("Duration"))
            .cloned()
            .unwrap_or(json!(0));

        let magnitude_glob = e
            .get("Magnitude")
            .filter(|v| is_ref_stub(v))
            .map(|v| resolve_glob_ref(&by_sel, v));
        let duration_glob = e
            .get("Duration")
            .filter(|v| is_ref_stub(v))
            .map(|v| resolve_glob_ref(&by_sel, v));

        let curve_table = e.get("Curve Table").cloned();
        let curve_input_av = e.get("Actor Value").filter(|v| is_ref_stub(v)).cloned();

        let conditions = e
            .get("Conditions")
            .map(flatten_condition_rows)
            .unwrap_or_default()
            .iter()
            .map(|row| resolve_condition_row(&by_sel, row))
            .collect();

        let perk_to_apply = summary.as_ref().and_then(|s| s.perk_to_apply).cloned();
        let equip_ability = summary.as_ref().and_then(|s| s.equip_ability).cloned();
        if let Some(fid) = perk_to_apply.as_ref().and_then(|p| stub_formid(Some(p))) {
            enqueue.push((fid, "Perk to Apply".to_string()));
        }
        if let Some(fid) = equip_ability.as_ref().and_then(|eq| stub_formid(Some(eq))) {
            enqueue.push((fid, "Equip Ability".to_string()));
        }

        rows.push(MagicEffectRow {
            index: i,
            base_effect,
            archetype,
            actor_value,
            magnitude,
            duration,
            magnitude_glob,
            duration_glob,
            curve_table,
            curve_input_av,
            conditions,
            perk_to_apply,
            equip_ability,
        });
    }
    Ok(MagicItemDigest { effects: rows })
}

/// PERK: description; ranks/playable/next; per-effect Ability (enqueue) or
/// Entry Point (fn/value/AV + perk conditions), or `NO effects` when the
/// bonus is engine/script-side. Perk-entry field misattribution is already
/// fixed upstream in the decoder, so no repair shim is needed here.
fn digest_perk(
    f: &mut impl ChaseFetcher,
    fields: &Value,
    enqueue: &mut Vec<EnqueueTarget>,
) -> anyhow::Result<PerkDigest> {
    let data = fields.get("Data");
    let description = fields
        .get("Description")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let playable = data
        .and_then(|d| d.pointer("/Playable/name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let num_ranks = data.and_then(|d| d.get("Num Ranks")).cloned();
    let next_perk = fields.get("Next Perk").filter(|v| is_ref_stub(v)).cloned();

    let Some(effects) = fields.get("Effects").and_then(Value::as_array) else {
        return Ok(PerkDigest {
            description,
            num_ranks,
            playable,
            next_perk,
            effects: None,
        });
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

    let mut rows = Vec::with_capacity(effects.len());
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
                    if let Some(fid) = stub_formid(Some(ability)) {
                        enqueue.push((fid, "Ability".to_string()));
                    }
                    rows.push(PerkEffectRow::Ability {
                        index: i,
                        target: ability.clone(),
                    });
                }
            }
            "Entry Point" => {
                let ep = e.get("Entry Point");
                let entry_point_name = ep
                    .and_then(|v| v.pointer("/Entry Point/name"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let function_name = ep
                    .and_then(|v| v.pointer("/Function/name"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let float_value = e.get("Float").filter(|v| v.as_f64().is_some()).cloned();
                let actor_value = e
                    .get("Function Parameter 3 (Actor Value)")
                    .filter(|v| is_ref_stub(v))
                    .cloned();
                let conditions = e
                    .get("Perk Conditions")
                    .map(flatten_perk_condition_rows)
                    .unwrap_or_default()
                    .iter()
                    .map(|row| resolve_condition_row(&by_sel, row))
                    .collect();
                rows.push(PerkEffectRow::EntryPoint {
                    index: i,
                    entry_point_name,
                    function_name,
                    float_value,
                    actor_value,
                    conditions,
                });
            }
            other => rows.push(PerkEffectRow::Other {
                index: i,
                type_name: other.to_string(),
            }),
        }
    }
    Ok(PerkDigest {
        description,
        num_ranks,
        playable,
        next_perk,
        effects: Some(rows),
    })
}

fn digest_weap(fields: &Value) -> WeapDigest {
    let data = fields.get("Data");
    let keyword_ids = fields
        .pointer("/Keywords/Keywords")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut relevant_keywords = Vec::new();
    for k in &keyword_ids {
        if let Some(edid) = k.get("editor_id").and_then(Value::as_str)
            && (edid.starts_with("WeaponType")
                || edid.starts_with("HasLegendary")
                || edid.starts_with("ma_"))
        {
            relevant_keywords.push(edid.to_string());
        }
    }
    let eligible_levels = fields
        .get("Eligible Levels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attach_slots = fields
        .get("Attach Parent Slots")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let has_object_template = fields.get("Object Template").is_some_and(|v| !v.is_null());
    WeapDigest {
        relevant_keywords,
        ap_cost: data.and_then(|d| d.get("Action Point Cost")).cloned(),
        speed: data.and_then(|d| d.get("Speed")).cloned(),
        reload_speed: data.and_then(|d| d.get("Reload Speed")).cloned(),
        eligible_levels,
        attach_slots,
        has_object_template,
    }
}

fn is_generic_noise_value(v: &Value) -> bool {
    matches!(v, Value::Null) || matches!(v, Value::String(s) if s.is_empty())
}

fn has_raw_marker(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.contains_key("_raw"))
}

/// Trimmed field tree for any record type without a dedicated digest: drop
/// null/empty values, the `Unknown`/`_record_type`/`Editor ID` keys, `_raw`-
/// bearing objects, and [`GENERIC_NOISE_KEYS`]. Every FormID reference is
/// already annotated by the Stub-resolved fetch — no per-reference
/// round-trip needed — so the trimmed tree is itself the computed
/// [`GenericDigest`] payload; `render` owns turning it into a capped
/// pretty-printed dump.
fn trim_generic_fields(fields: &Value) -> Value {
    if is_ref_stub(fields) {
        return fields.clone();
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

fn digest_generic(fields: &Value) -> GenericDigest {
    GenericDigest {
        trimmed: trim_generic_fields(fields),
    }
}

/// Classify an OMOD root's `Data.Properties[]` rows via `esm::chase`'s
/// mechanism classifier ([`omod_chase`]) and return its classified [`Hop`]s
/// directly — [`render::render_omod_hops`] turns them into path-sliced
/// evidence lines. Runs on the already-fetched root — `fields` was already
/// pulled down by [`walk`]'s own `bulk_get`, so this only spends fetches on
/// whatever forward/reverse evidence the classifier needs. `ref_limit`
/// bounds the reverse keyword/AVIF consumer walk (see
/// [`WalkOptions::ref_limit`]); runs regardless of `--depth`, which only
/// governs BFS enqueueing.
fn digest_omod_mechanisms(
    f: &mut impl ChaseFetcher,
    formid: FormId,
    sig: &str,
    editor_id: &str,
    fields: &Value,
    ref_limit: usize,
) -> anyhow::Result<Vec<Hop>> {
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
    Ok(tree.hops)
}

/// Scan classified [`Hop`]s for the forward-fetched targets worth visiting as
/// their own BFS node: a `DirectProperty` hop targeting a PROJ (a directly
/// attached projectile override) or an ENCH (so an OMOD → ENCH → MGEF →
/// granted-perk chain lands in one `walk` call, reusing what the classifier
/// already knows via `chase::FORWARD_FETCH_TYPES` rather than a separate
/// re-scan). Include-sourced hops (`source_omod.is_some()`) are skipped —
/// walk enqueues includes as their own BFS nodes separately (see
/// [`digest_omod_includes`]).
fn omod_hops_enqueue(hops: &[Hop], enqueue: &mut Vec<EnqueueTarget>) {
    for hop in hops {
        if hop.source_omod.is_some() {
            continue;
        }
        let Some(target) = &hop.target else {
            continue;
        };
        let target_rt = target
            .get("record_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        if hop.kind == HopKind::DirectProperty
            && (target_rt == "PROJ" || target_rt == "ENCH")
            && let Some(fid) = stub_formid(Some(target))
        {
            enqueue.push((fid, "OMOD property".to_string()));
        }
    }
}

/// `Data.Includes[]` names another OMOD this one composes from — the
/// `_PARENT_*` empty-shell pattern (properties compose onto the includer) or
/// a `modcol_*` collection (each include is an alternative). Nothing in the
/// data reliably distinguishes the two, so we don't merge: each include is
/// enqueued into the BFS as its own walked node (same pattern as
/// [`omod_hops_enqueue`]'s ENCH/PROJ case), bounded by
/// [`OMOD_INCLUDE_ENQUEUE_CAP`]. Returns the shown (capped) include targets
/// plus the pre-cap total.
fn digest_omod_includes(
    fields: &Value,
    editor_id: &str,
    enqueue: &mut Vec<EnqueueTarget>,
) -> (Vec<Value>, usize) {
    let Some(includes) = fields.pointer("/Data/Includes").and_then(Value::as_array) else {
        return (Vec::new(), 0);
    };
    let mut shown = Vec::new();
    let mut total = 0usize;
    for inc in includes {
        let Some(target) = inc.get("Mod").filter(|v| is_ref_stub(v)) else {
            continue;
        };
        total += 1;
        if shown.len() >= OMOD_INCLUDE_ENQUEUE_CAP {
            continue;
        }
        if let Some(fid) = stub_formid(Some(target)) {
            enqueue.push((fid, format!("include of {editor_id}")));
        }
        shown.push(target.clone());
    }
    (shown, total)
}

fn digest_proj(fields: &Value, enqueue: &mut Vec<EnqueueTarget>) -> ProjDigest {
    let Some(data) = fields.get("Data") else {
        return ProjDigest::default();
    };
    let proj_type = data
        .get("Type")
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let speed = data.get("Speed").cloned();
    let explosion = data.get("Explosion").filter(|v| is_ref_stub(v)).cloned();
    if let Some(fid) = explosion.as_ref().and_then(|expl| stub_formid(Some(expl))) {
        enqueue.push((fid, "projectile explosion".to_string()));
    }
    ProjDigest {
        proj_type,
        speed,
        explosion,
    }
}

fn digest_expl(fields: &Value) -> ExplDigest {
    ExplDigest {
        detail: summarize_explosion_detail(fields),
    }
}

/// Enqueue each entry's own *direct* sublist target (an entry whose
/// `Reference`/legacy `Item` resolves to another LVLI) as its own BFS node,
/// so `--depth` can drill into an intermediate list's own digest. The
/// aggregated table [`digest_lvli`] renders already flattens the full
/// recursive expansion down to leaf items regardless of `--depth` — this is
/// purely so the intermediate list stays visible, not load-bearing for the
/// odds themselves.
fn lvli_direct_sublists(fields: &Value, enqueue: &mut Vec<EnqueueTarget>) {
    let Some(list) = fields.get("Leveled List Entries").and_then(Value::as_array) else {
        return;
    };
    for item in list {
        let Some(entry) = item.get("Leveled List Entry") else {
            continue;
        };
        let target = entry
            .get("Reference")
            .or_else(|| entry.pointer("/Base Data/Item"));
        let Some(target) = target else {
            continue;
        };
        if target.get("record_type").and_then(Value::as_str) == Some("LVLI")
            && let Some(fid) = stub_formid(Some(target))
        {
            enqueue.push((fid, "leveled list entry".to_string()));
        }
    }
}

/// LVLI: resolve full drop odds via [`crate::lvli::drop_table`] (pool/`Use
/// All`/`Use First Match` selection, chance-none, Curve Table evaluation at
/// `level`, recursion through nested sublists) and wrap the result verbatim.
/// Direct sublist targets are also enqueued as their own BFS nodes (see
/// [`lvli_direct_sublists`]).
fn digest_lvli(
    f: &mut impl ChaseFetcher,
    formid: FormId,
    fields: &Value,
    level: f32,
    enqueue: &mut Vec<EnqueueTarget>,
) -> anyhow::Result<LvliDigest> {
    let opts = crate::lvli::DropOptions {
        level,
        ..Default::default()
    };
    let table = crate::lvli::drop_table(f, formid, fields, &opts)?;
    lvli_direct_sublists(fields, enqueue);
    Ok(LvliDigest { table })
}

// ─── refs digest (root-only, `--refs`) ──────────────────────────────────────

/// Group an unfiltered reverse-`refs` row list by `record_type`, sorted by
/// count descending, each with up to 5 sample EditorIDs (⚠NONPLAYABLE-flagged)
/// and an obtainability tag. Pure —
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

/// Compute one node's record-type-specific [`Digest`], returning it plus any
/// FormIDs it wants enqueued one hop further (with the "via" edge label to
/// attach to the enqueued node). `ref_limit` only matters for the OMOD arm's
/// mechanism slice (see [`digest_omod_mechanisms`]) and the KYWD/AVIF root's
/// own consumer digest (which ignores it — see [`WalkOptions::ref_limit`]).
fn digest_node(
    f: &mut impl ChaseFetcher,
    sig: &str,
    formid: FormId,
    editor_id: &str,
    fields: &Value,
    ref_limit: usize,
    level: f32,
) -> anyhow::Result<(Digest, Vec<EnqueueTarget>)> {
    let mut enqueue = Vec::new();
    let digest = match sig {
        "GLOB" => Digest::Glob(digest_glob(fields)),
        "LVLI" => Digest::Lvli(digest_lvli(f, formid, fields, level, &mut enqueue)?),
        "AVIF" => Digest::Avif(digest_avif(f, formid, fields)?),
        "KYWD" => Digest::Kywd(digest_kywd(f, formid)?),
        "MGEF" => Digest::Mgef(digest_mgef(fields, &mut enqueue)),
        "SPEL" | "ENCH" | "ALCH" => Digest::MagicItem(digest_magic_item(f, fields, &mut enqueue)?),
        "PERK" => Digest::Perk(digest_perk(f, fields, &mut enqueue)?),
        "WEAP" => Digest::Weap(digest_weap(fields)),
        "PROJ" => Digest::Proj(digest_proj(fields, &mut enqueue)),
        "EXPL" => Digest::Expl(digest_expl(fields)),
        "OMOD" => {
            // Classify Data.Properties[] via chase's mechanism classifier
            // first (ENCH/PROJ direct attachments are enqueued from the same
            // classified hops — see `omod_hops_enqueue` — rather than a
            // separate re-scan), then the Includes[] pointers (enqueued as
            // their own BFS nodes — no property merge).
            let hops = digest_omod_mechanisms(f, formid, sig, editor_id, fields, ref_limit)?;
            omod_hops_enqueue(&hops, &mut enqueue);
            let (includes, includes_total) = digest_omod_includes(fields, editor_id, &mut enqueue);
            Digest::Omod(OmodDigest {
                hops,
                includes,
                includes_total,
            })
        }
        _ => Digest::Generic(digest_generic(fields)),
    };
    Ok((digest, enqueue))
}

/// Run the walk for one root selector: BFS out to `opts.depth`, computing a
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

        let (digest, enqueue) = digest_node(
            f,
            &header.signature,
            formid,
            &editor_id,
            &fields,
            opts.ref_limit,
            opts.level,
        )?;

        nodes.push(WalkNode {
            depth,
            sig: header.signature,
            formid: formid.display(),
            editor_id,
            name,
            via,
            digest,
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
