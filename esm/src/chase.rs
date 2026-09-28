//! Mechanism classifier core, shared by two different contracts (see
//! `docs/adr/0001-walk-interactive-chase-pipeline-json.md`):
//!
//! - **`esm chase`** (this module's own CLI surface, via [`chase`]/[`ChaseTree`])
//!   is the machine-facing pipeline evidence contract: it always emits the
//!   classified `ChaseTree` as JSON and hard-errors on any selector that
//!   doesn't resolve to one of the five accepted root types. Its JSON shape
//!   is frozen: the `/patch-notes` deep-writer agent consumes it. This module
//!   has no human text renderer of its own; interactive reading of a
//!   classified OMOD lives entirely in `esm::walk`, which calls straight into
//!   this module's classification functions (see [`omod_chase`]) and renders
//!   the result itself.
//! - **`esm walk`** (`src/walk/`) is the sole interactive surface. On an
//!   OMOD root it runs this module's classifier inline and renders each
//!   mechanism as path-sliced evidence rows under the record's digest — see
//!   that module's docs for the exact rendering.
//!
//! Automates the "chase pattern" for unique-weapon OMOD effects documented
//! under "Chasing a unique-weapon effect" in
//! `patch-notes/skill/kb/mechanics.md`, generalized past OMODs to also
//! accept PERK, SPEL, ALCH, and ENCH selectors
//! directly — the record types an OMOD's own forward-fetch hops resolve
//! into. Read the KB section first — this module is a mechanical
//! implementation of the walks it describes, nothing more.
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
//! "Severing's confirmed chase": `ENCH -> MGEF (Perk to Apply) -> PERK`).
//! `mgef_pass_through` follows this one bounded extra hop and is shared by
//! both walks — OMOD's forward-fetched ENCH/SPEL targets, and
//! `effect_chase`'s own Base-Effect entries and forward-fetched PERK
//! targets.
//!
//! Composes the same operations (`Op::RecordBulk`, `Op::ReferencedBy`)
//! in-process through the [`RecordSource`] seam — no new `Op` variant; the
//! pure logic here doesn't know where its records come from.

use crate::fields::{dedup_sorted, is_ref_stub, is_truthy, named, stub_formid};
use crate::ops::{RecordSel, RefDepth};
use crate::source::{RecordSource, bulk_fetch_map};
use crate::{BulkRecordEntry, FormId, RefList, RefRow, ResolveDepth};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

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
pub const DEFAULT_DEPTH: RefDepth = RefDepth::DIRECT;
/// Cap on refs rows fetched per record-type filter before bulk-fetching consumers.
pub const DEFAULT_REF_LIMIT: usize = 25;

/// Cap on `Data.Includes[]` targets expanded per OMOD level (chase hop
/// expansion and walk BFS enqueue share this bound). Corpus include-breadth
/// peaks at 79 on one selector OMOD; 20 covers the overwhelming majority
/// while keeping chase/walk BFS bounded and fail-fast per
/// `docs/adr/0001-walk-interactive-chase-pipeline-json.md`.
pub const OMOD_INCLUDE_ENQUEUE_CAP: usize = 20;

/// Max depth when expanding `Data.Includes[]` chains inside [`omod_chase`].
/// Measured corpus max is 3; do not search deeper.
const OMOD_INCLUDE_MAX_DEPTH: usize = 3;

/// OMOD record flags (xEdit `wbDefinitionsFO76.pas`) that make an OMOD's
/// `Data.Includes[]` a set of alternatives rather than parts of itself.
const OMOD_MOD_COLLECTION: u32 = 0x0000_0080;
const OMOD_MOD_SELECTOR: u32 = 0x0000_0200;

/// How an OMOD's `Data.Includes[]` relate to it, by its record flags.
///
/// On 20260918 every include of a plain OMOD names a `Mod Template`
/// (`_PARENT_*`, which has no includes itself): the template's properties
/// apply as the includer's own. A `Mod Collection` (`modcol_*`) or
/// `Mod Selector` has no properties of its own; its includes are the plain
/// mods it picks among (collections gate them by `Minimum Level`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncludeRole {
    /// The includes' properties are this OMOD's properties.
    Compose,
    /// Each include is a separate mod this OMOD stands for one of.
    Alternatives,
}

pub(crate) fn include_role(record_flags: u32) -> IncludeRole {
    if record_flags & (OMOD_MOD_COLLECTION | OMOD_MOD_SELECTOR) != 0 {
        IncludeRole::Alternatives
    } else {
        IncludeRole::Compose
    }
}

/// One include of a `Mod Collection` or `Mod Selector` (see [`IncludeRole`]):
/// a mod it stands for, available from `minimum_level`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct IncludeAlternative {
    #[cfg_attr(test, ts(type = "unknown"))]
    pub omod: Value,
    pub minimum_level: u64,
}

/// The alternatives a `Mod Collection`/`Mod Selector` OMOD's decoded
/// `fields` include, in order.
pub(crate) fn include_alternatives(fields: &Value) -> Vec<IncludeAlternative> {
    fields
        .pointer("/Data/Includes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|inc| {
            let omod = inc.get("Mod").filter(|v| is_ref_stub(v))?.clone();
            let minimum_level = inc
                .get("Minimum Level")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Some(IncludeAlternative {
                omod,
                minimum_level,
            })
        })
        .collect()
}

// ─── fetch seam ─────────────────────────────────────────────────────────────

// ─── output types ───────────────────────────────────────────────────────────

/// Options controlling the reverse-ref walk depth/cap (see [`DEFAULT_DEPTH`]/
/// [`DEFAULT_REF_LIMIT`]). Only consulted by the OMOD walk — a PERK/SPEL/
/// ALCH/ENCH root's mechanic is already inline in its own `Effects[]`, so
/// `effect_chase` never reverse-chases and these options are a no-op there.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ChaseOptions {
    pub depth: RefDepth,
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
/// root (classified `Data.Properties[]` rows, including those of the mod
/// templates it includes), and `alternatives` for a `Mod Collection` or
/// `Mod Selector` OMOD (see [`IncludeRole`]); `effect_hops` for a
/// PERK/SPEL/ALCH/ENCH root (classified `Effects[]` entries). Kept as a flat
/// struct (rather than an enum) so existing OMOD callers/tests don't need to
/// match on a variant just to reach `.hops`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ChaseTree {
    pub root: RootStub,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hops: Vec<Hop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effect_hops: Vec<EffectHop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<IncludeAlternative>,
}

/// The chased record's own identity, for any root type [`chase`] accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RootStub {
    pub formid: Option<String>,
    pub editor_id: Option<String>,
    /// Short record signature (e.g. `"OMOD"`, `"PERK"`) — same convention
    /// `Hop.target`/`esm::walk::render`'s `fmt_stub` use.
    pub record_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub name: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub description: Option<Value>,
}

/// How one `Data.Properties[]` row was classified (see the module docs).
///
/// This is the *resolution* taxonomy (forward fetch vs reverse refs vs
/// non-mechanism tag), not the domain's four-way mechanism taxonomy — see
/// `CONTEXT.md`'s **Mechanism** / **Tag keyword** terms. `OverrideProjectile`
/// stays [`DirectProperty`]; tag keywords that are not SPEL/PERK gates are
/// [`TagKeyword`] rather than a misclassified [`KeywordHook`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
pub enum HopKind {
    DirectProperty,
    PerkGrant,
    KeywordHook,
    TagKeyword,
}

/// One classified `Data.Properties[]` row plus whatever evidence the chase
/// found to explain what it does downstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Hop {
    pub property_index: usize,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub property: Value,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub function: Value,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub value1: Value,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub value2: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub curve_table: Option<Value>,
    pub kind: HopKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub target: Option<Value>,
    /// Which direction `esm::chase` fetched this hop's evidence — forward
    /// `get` (the target's own Description/Effects) or reverse `refs` (who
    /// points at the target). `None` for a [`HopKind::TagKeyword`]
    /// (synthetic tag evidence, no fetch) or a bare-scalar
    /// `DirectProperty` (nothing to chase). This is the real fact that
    /// distinguishes an AV hook (an AVIF `DirectProperty` resolved by
    /// reverse chase, exactly like a `KeywordHook`) from a plain direct
    /// SPEL/ENCH/PROJ attachment (forward-fetched) — both share
    /// `HopKind::DirectProperty`, so `kind` alone can't tell them apart; see
    /// `CONTEXT.md`'s **Mechanism** entry. Kept here (rather than only
    /// inside [`classify_property_row`]'s internal `FetchDest`) so
    /// `esm::walk`'s renderer doesn't have to re-derive the same fact by
    /// string-matching `target.record_type == "AVIF"`. Additive to the
    /// frozen chase JSON shape
    /// (`docs/adr/0001-walk-interactive-chase-pipeline-json.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<FetchDirection>,
    /// When this hop's property row comes from a mod template the root
    /// includes (see [`IncludeRole::Compose`]) rather than the root OMOD
    /// itself — the included OMOD's stub.
    /// `None` for the root's own properties (additive to the frozen chase
    /// JSON shape; see `docs/adr/0001-walk-interactive-chase-pipeline-json.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub source_omod: Option<Value>,
    pub evidence: Vec<Evidence>,
}

/// Which direction one classified property row's evidence was fetched — see
/// [`Hop::resolution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FetchDirection {
    Forward,
    Reverse,
}

/// How one `Effects[]` entry was classified — the Effects[]-array-side analog
/// of [`HopKind`]. Kept as a separate enum (rather than widening `HopKind`)
/// because the two field shapes genuinely don't overlap: see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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

/// One classified `Effects[]` entry from a PERK/SPEL/ALCH/ENCH root — the
/// Effects[]-array-side analog of [`Hop`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct EffectHop {
    pub effect_index: usize,
    pub kind: EffectHopKind,
    /// The raw `{"Effect": {...}}` entry — feeds `summarize_effect` for
    /// rendering and is included verbatim in JSON output.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub effect: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub target: Option<Value>,
    pub evidence: Vec<Evidence>,
}

/// One piece of evidence found for a hop — a forward-fetched record's own
/// Description/Effects, a reverse-chased consumer's gated `Effects[N]` entry,
/// or an MGEF pass-through's `Perk to Apply`/`Equip Ability`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Evidence {
    #[cfg_attr(test, ts(type = "unknown"))]
    pub source: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub detail: EvidenceDetail,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hop_depth: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub path_chain: Option<Value>,
}

/// What an [`Evidence`] found, by how it was found. Untagged: the JSON is the
/// variant's own fields (the frozen chase JSON contract carries no tag).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(untagged)]
pub enum EvidenceDetail {
    /// Why nothing more could be said (a failed fetch, a record with no
    /// Description/Effects, an effect row that couldn't be isolated).
    Note { note: String },
    /// A [`HopKind::TagKeyword`]'s keyword: its own Notes and Type.
    Tag(TagDetail),
    /// One `Effects[N]` row of a reverse-chased consumer, sliced by the
    /// field path that references the keyword/AVIF.
    Effect {
        #[cfg_attr(test, ts(type = "unknown"))]
        effect: Value,
    },
    /// A forward-fetched record's Description and Effects.
    Record(RecordDetail),
    /// An MGEF's `Perk to Apply`/`Equip Ability`.
    PassThrough(PassThroughDetail),
    /// A projectile override: the PROJ's speed/type and its explosion.
    Projectile(Box<ProjectileDetail>),
}

impl EvidenceDetail {
    /// A forward-fetched record's `Effects[]` rows, if this is one.
    pub fn effects(&self) -> Option<&[Value]> {
        match self {
            EvidenceDetail::Record(record) => record.effects.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct TagDetail {
    /// Always `true`: marks the evidence as synthetic (no fetch).
    pub tag: bool,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub notes: Value,
    #[serde(rename = "type")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub kind: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct RecordDetail {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub description: Option<Value>,
    /// At most 12 of the record's `Effects[]` rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "Array<unknown> | null"))]
    pub effects: Option<Vec<Value>>,
    /// How many `Effects[]` rows the cap left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects_truncated: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct PassThroughDetail {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub perk_to_apply: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub equip_ability: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ProjectileDetail {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub speed: Option<Value>,
    #[serde(
        rename = "type",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub kind: Option<Value>,
    /// The linked EXPL's stub.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub explosion: Option<Value>,
    /// That explosion's summary; empty unless it was fetched.
    #[serde(flatten, default)]
    pub summary: ExplosionSummary,
}

/// An EXPL's radius/force/stagger/chain and damage, whichever of the five
/// corpus damage shapes it uses (see [`summarize_explosion`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ExplosionSummary {
    /// `[Inner Radius, Outer Radius]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "[unknown, unknown] | null"))]
    pub radius: Option<[Value; 2]>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub force: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub stagger: Option<Value>,
    /// The Impact Data Set's EditorID (or FormID when it has none).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub impact_data_set: Option<Value>,
    /// Whether the explosion chains; set whenever the EXPL has `Data`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub placed_object: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub spawn_projectile: Option<Value>,
    /// Every damage source (possibly none); absent from a projectile whose
    /// explosion wasn't fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage: Option<Vec<ExplosionDamage>>,
}

/// One damage source of an explosion: a per-type `Damage Types[]` row
/// (`type` plus a curve or amount), the legacy `Damage Curve Table`
/// (`curve`), `Base Weapon Damage Mult` or flat `Damage`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ExplosionDamage {
    /// The damage type's EditorID (present, possibly null, on a
    /// `Damage Types[]` row).
    #[serde(
        rename = "type",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub kind: Option<Value>,
    /// The curve table's EditorID.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub curve: Option<Value>,
    /// The curve's `[min y, max y]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub amount: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub base_weapon_mult: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub flat: Option<Value>,
}

/// Deserialize a present field, `null` included, as `Some`: an evidence
/// detail keeps a key whose value is `null` (e.g. an explosion damage row's
/// `"type": null`), and must read it back the same way.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

// ─── schema helpers (pure `serde_json::Value` walking) ─────────────────────

fn field_or_null(field: Option<&Value>) -> Value {
    field.cloned().unwrap_or(Value::Null)
}

/// `Node::formid_paths` (`src/decode/node.rs`) builds paths as dot-joined JSON
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
///
/// `pub(crate)` so `esm::walk`'s OMOD digest can turn an [`Evidence::via`]
/// field path back into the `"Effects[N]"` label it prints for a sliced row.
pub(crate) fn first_array_container(path: &str) -> Option<String> {
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
        if !is_ref_stub(base) {
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

/// Records fetched by FormID (see [`fetch_stubs`]).
type Fetched = HashMap<FormId, BulkRecordEntry>;

/// Fetch, once each, the records these reference stubs name.
fn fetch_stubs<'v>(
    f: &mut impl RecordSource,
    stubs: impl IntoIterator<Item = &'v Value>,
) -> anyhow::Result<Fetched> {
    let mut fids: Vec<FormId> = stubs
        .into_iter()
        .filter_map(|s| stub_formid(Some(s)))
        .collect();
    dedup_sorted(&mut fids);
    bulk_fetch_map(f, &fids)
}

/// The fetched record a reference stub names.
fn fetched<'a>(by_fid: &'a Fetched, stub: &Value) -> Option<&'a BulkRecordEntry> {
    by_fid.get(&stub_formid(Some(stub))?)
}

/// Given an already-bulk-fetched MGEF target, extract `"Perk to Apply"`/`"Equip Ability"` into a compact
/// [`Evidence`]. `None` if the MGEF wasn't found/failed to fetch, or has
/// neither field set — the common case, most magic effects are plain
/// damage/buff effects with nothing further to chase.
fn mgef_pass_through_evidence(mgef_target: &Value, by_fid: &Fetched) -> Option<Evidence> {
    let entry = fetched(by_fid, mgef_target)?;
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
    Some(Evidence {
        source: stub(mgef_target),
        via: Some("Base Effect".to_string()),
        detail: EvidenceDetail::PassThrough(PassThroughDetail {
            perk_to_apply,
            equip_ability,
        }),
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

fn note(text: impl Into<String>) -> EvidenceDetail {
    EvidenceDetail::Note { note: text.into() }
}

fn forward_evidence(target: &Value, by_fid: &Fetched) -> Evidence {
    let entry = match fetched(by_fid, target) {
        None => {
            return Evidence {
                source: stub(target),
                via: None,
                detail: note("fetch failed: no response"),
                hop_depth: None,
                path_chain: None,
            };
        }
        Some(e) => e,
    };
    if let Some(err) = &entry.error {
        return Evidence {
            source: stub(target),
            via: None,
            detail: note(format!("fetch failed: {err}")),
            hop_depth: None,
            path_chain: None,
        };
    }
    let fields = entry.fields.clone().unwrap_or(Value::Null);
    let mut detail = RecordDetail::default();
    if is_truthy(fields.get("Description")) {
        detail.description = Some(fields["Description"].clone());
    }
    if let Some(effects) = fields.get("Effects").and_then(Value::as_array)
        && !effects.is_empty()
    {
        let capped: Vec<Value> = effects.iter().take(12).cloned().collect();
        let truncated = effects.len().saturating_sub(capped.len());
        detail.effects = Some(capped);
        detail.effects_truncated = (truncated > 0).then_some(truncated);
    }
    let detail = if detail.description.is_none() && detail.effects.is_none() {
        note("no Description/Effects field on this record")
    } else {
        EvidenceDetail::Record(detail)
    };
    Evidence {
        source: stub(target),
        via: None,
        detail,
        hop_depth: None,
        path_chain: None,
    }
}

/// Min/max `y` across an inline-resolved curve table's `curve` points, if any.
fn curve_y_range(curve_table: &Value) -> Option<(f64, f64)> {
    let points = curve_table.get("curve")?.as_array()?;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in points {
        let Some(y) = p.get("y").and_then(Value::as_f64) else {
            continue;
        };
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    if min_y.is_finite() && max_y.is_finite() {
        Some((min_y, max_y))
    } else {
        None
    }
}

/// Summarize an EXPL record's damage / radius / force / stagger / chain
/// payload. Covers the five corpus damage shapes (per-type `Damage Types[]` +
/// curve, legacy `Data.Damage Curve Table`, `Base Weapon Damage Mult`, flat
/// `Data.Damage`, or none) plus utility fields so a JSON consumer can read
/// damage without guessing which shape is present. `pub(crate)` so
/// `esm::walk`'s EXPL digest arm reuses the same logic.
pub(crate) fn summarize_explosion(fields: &Value) -> ExplosionSummary {
    let data = fields.get("Data");
    let mut summary = ExplosionSummary::default();
    let mut damage = Vec::new();

    if let Some(d) = data {
        let inner = d.get("Inner Radius").cloned().unwrap_or(Value::Null);
        let outer = d.get("Outer Radius").cloned().unwrap_or(Value::Null);
        if !inner.is_null() || !outer.is_null() {
            summary.radius = Some([inner, outer]);
        }
        if is_truthy(d.get("Force")) {
            summary.force = Some(d["Force"].clone());
        }
        let stagger = named(d.get("Stagger"));
        if is_truthy(Some(&stagger)) {
            summary.stagger = Some(stagger);
        }
        if let Some(ipds) = d.get("Impact Data Set").filter(|v| is_ref_stub(v)) {
            summary.impact_data_set = Some(
                ipds.get("editor_id")
                    .cloned()
                    .unwrap_or_else(|| stub(ipds).get("formid").cloned().unwrap_or(Value::Null)),
            );
        }
        summary.chain = Some(
            d.pointer("/Flags1/flags")
                .and_then(Value::as_array)
                .is_some_and(|flags| flags.iter().any(|f| f.as_str() == Some("Chain"))),
        );
        summary.placed_object = d.get("Placed Object").filter(|v| is_ref_stub(v)).map(stub);
        summary.spawn_projectile = d
            .get("Spawn Projectile")
            .filter(|v| is_ref_stub(v))
            .map(stub);
    }

    let curve_row = |ct: &Value, kind: Option<Value>| ExplosionDamage {
        kind,
        curve: Some(ct.get("editor_id").cloned().unwrap_or(Value::Null)),
        range: curve_y_range(ct).map(|(lo, hi)| [lo, hi]),
        ..ExplosionDamage::default()
    };
    if let Some(types) = fields.get("Damage Types").and_then(Value::as_array) {
        for entry in types {
            let kind = Some(
                entry
                    .pointer("/Type/editor_id")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            damage.push(
                match entry.get("Curve Table").filter(|v| is_truthy(Some(*v))) {
                    Some(ct) => curve_row(ct, kind),
                    None => ExplosionDamage {
                        kind,
                        amount: is_truthy(entry.get("Amount")).then(|| entry["Amount"].clone()),
                        ..ExplosionDamage::default()
                    },
                },
            );
        }
    }
    if let Some(d) = data {
        if let Some(ct) = d.get("Damage Curve Table").filter(|v| is_truthy(Some(*v))) {
            damage.push(curve_row(ct, None));
        }
        if is_truthy(d.get("Base Weapon Damage Mult")) {
            damage.push(ExplosionDamage {
                base_weapon_mult: Some(d["Base Weapon Damage Mult"].clone()),
                ..ExplosionDamage::default()
            });
        }
        if is_truthy(d.get("Damage")) {
            damage.push(ExplosionDamage {
                flat: Some(d["Damage"].clone()),
                ..ExplosionDamage::default()
            });
        }
    }
    summary.damage = Some(damage);
    summary
}

/// Build forward evidence for a PROJ-targeting OMOD property: speed/type plus
/// the linked EXPL's radius/force/stagger/chain/damage summary when present.
fn projectile_evidence(target: &Value, proj_fields: &Value, expl_by_fid: &Fetched) -> Evidence {
    let mut detail = ProjectileDetail::default();
    if let Some(data) = proj_fields.get("Data") {
        if is_truthy(data.get("Speed")) {
            detail.speed = Some(data["Speed"].clone());
        }
        let proj_type = named(data.get("Type"));
        if is_truthy(Some(&proj_type)) {
            detail.kind = Some(proj_type);
        }
        if let Some(expl) = data.get("Explosion").filter(|v| is_ref_stub(v)) {
            detail.explosion = Some(stub(expl));
            if let Some(entry) = fetched(expl_by_fid, expl)
                && entry.error.is_none()
            {
                let expl_fields = entry.fields.as_ref().unwrap_or(&Value::Null);
                detail.summary = summarize_explosion(expl_fields);
            }
        }
    }
    Evidence {
        source: stub(target),
        via: None,
        detail: EvidenceDetail::Projectile(Box::new(detail)),
        hop_depth: None,
        path_chain: None,
    }
}

/// Synthetic evidence for a [`HopKind::TagKeyword`]: the KYWD's own Notes/Type,
/// not a reverse-chased consumer.
fn tag_keyword_evidence(target: &Value, kywd_fields: Option<&Value>, type_name: Value) -> Evidence {
    let notes = kywd_fields
        .and_then(|f| f.get("Notes"))
        .filter(|n| !n.is_null())
        .cloned()
        .unwrap_or(Value::Null);
    Evidence {
        source: stub(target),
        via: None,
        detail: EvidenceDetail::Tag(TagDetail {
            tag: true,
            notes,
            kind: type_name,
        }),
        hop_depth: None,
        path_chain: None,
    }
}

/// A successfully-fetched KYWD's decoded fields.
fn kywd_fields_from_map<'a>(by_fid: &'a Fetched, target: &Value) -> Option<&'a Value> {
    let entry = fetched(by_fid, target)?;
    if entry.error.is_some() {
        return None;
    }
    entry.fields.as_ref()
}

/// True when a KYWD's `Type.name` is a populated enum other than `"None"` —
/// those are categorically item-policy / UI tags, never SPEL/PERK gates.
fn is_populated_kywd_type(type_name: &Value) -> bool {
    is_truthy(Some(type_name)) && type_name.as_str() != Some("None")
}

/// Classify one OMOD `Data.Properties[]` row into a [`Hop`] plus an optional
/// forward/reverse fetch destination. Shared by the root's own properties and
/// by include-expanded rows so the KYWD/PERK/AVIF/PROJ/forward-type dispatch
/// lives in one place.
fn classify_property_row(
    prop: &Value,
    property_index: usize,
    source_omod: Option<Value>,
    kywd_by_fid: &Fetched,
) -> (Hop, Option<FetchDest>) {
    let prop_name = named(prop.get("Property"));
    let function = named(prop.get("Function Type"));
    let value1 = field_or_null(prop.get("Value 1"));
    let value2 = field_or_null(prop.get("Value 2"));
    let curve_table = prop
        .get("Curve Table")
        .filter(|v| is_truthy(Some(*v)))
        .cloned();

    let mut hop = Hop {
        property_index,
        property: prop_name,
        function,
        value1: value1.clone(),
        value2,
        curve_table,
        kind: HopKind::DirectProperty,
        target: None,
        resolution: None,
        source_omod,
        evidence: Vec::new(),
    };

    if !is_ref_stub(&value1) {
        return (hop, None);
    }

    let target = stub(&value1);
    let rt = target
        .get("record_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    hop.target = Some(target.clone());

    let dest = if rt == "KYWD" {
        if let Some(fields) = kywd_fields_from_map(kywd_by_fid, &target) {
            let type_name = named(fields.get("Type"));
            if is_populated_kywd_type(&type_name) {
                hop.kind = HopKind::TagKeyword;
                hop.evidence = vec![tag_keyword_evidence(&target, Some(fields), type_name)];
                return (hop, None);
            }
        }
        hop.kind = HopKind::KeywordHook;
        Some(FetchDest::Reverse(target))
    } else if rt == "PERK" {
        hop.kind = HopKind::PerkGrant;
        Some(FetchDest::Forward(target))
    } else if FORWARD_FETCH_TYPES.contains(&rt.as_str()) || rt == "PROJ" {
        // PROJ joins the forward-fetch path alongside ENCH/SPEL, but stays
        // out of FORWARD_FETCH_TYPES — that constant's evidence builder
        // assumes Effects/Description, which a PROJ lacks (see
        // projectile_evidence).
        hop.kind = HopKind::DirectProperty;
        Some(FetchDest::Forward(target))
    } else if rt == "AVIF" {
        // The one case `HopKind` alone can't tell apart from a plain direct
        // SPEL/ENCH/PROJ attachment — both are `DirectProperty`, but this
        // target is resolved by *reverse* chase (like a KeywordHook) rather
        // than forward-fetched. `hop.resolution` (set below) is what lets
        // `esm::walk`'s renderer distinguish "AV hook" from "direct
        // property" without string-matching `target.record_type`.
        hop.kind = HopKind::DirectProperty;
        Some(FetchDest::Reverse(target))
    } else {
        hop.kind = HopKind::DirectProperty;
        None
    };

    hop.resolution = match &dest {
        Some(FetchDest::Forward(_)) => Some(FetchDirection::Forward),
        Some(FetchDest::Reverse(_)) => Some(FetchDirection::Reverse),
        None => None,
    };
    (hop, dest)
}

/// Where a classified property row still needs a follow-up fetch.
enum FetchDest {
    Forward(Value),
    Reverse(Value),
}

/// Collect `(properties, source_omod)` batches for the root plus a bounded
/// BFS over the `Data.Includes[]` that compose into it (depth ≤
/// [`OMOD_INCLUDE_MAX_DEPTH`], breadth ≤ [`OMOD_INCLUDE_ENQUEUE_CAP`] per
/// level). An OMOD whose includes are alternatives contributes only its own
/// properties.
fn collect_property_sources(
    f: &mut impl RecordSource,
    root_fields: &Value,
    root_flags: u32,
) -> anyhow::Result<Vec<(Vec<Value>, Option<Value>)>> {
    let root_properties: Vec<Value> = root_fields
        .pointer("/Data/Properties")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut sources: Vec<(Vec<Value>, Option<Value>)> = vec![(root_properties, None)];

    // The mod templates an OMOD's `Data.Includes[]` names, capped per level.
    let included = |fields: &Value| -> Vec<FormId> {
        fields
            .pointer("/Data/Includes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(OMOD_INCLUDE_ENQUEUE_CAP)
            .filter_map(|inc| stub_formid(inc.get("Mod")))
            .collect()
    };
    let mut visited: HashSet<FormId> = HashSet::new();
    let mut queue: VecDeque<(FormId, usize)> = VecDeque::new();
    if include_role(root_flags) == IncludeRole::Compose {
        for fid in included(root_fields) {
            if visited.insert(fid) {
                queue.push_back((fid, 1));
            }
        }
    }

    while let Some((fid, depth)) = queue.pop_front() {
        if depth > OMOD_INCLUDE_MAX_DEPTH {
            continue;
        }
        let fetched = f.bulk_get(&[RecordSel::FormId(fid)], ResolveDepth::Stub)?;
        let Some(entry) = fetched.into_iter().next() else {
            continue;
        };
        if entry.error.is_some() {
            continue;
        }
        let fields = entry.fields.clone().unwrap_or(Value::Null);
        let omod_stub = json!({
            "formid": fid.display(),
            "editor_id": entry.editor_id.clone().unwrap_or_default(),
            "record_type": entry.header.as_ref().map(|h| h.signature.clone()).unwrap_or_else(|| "OMOD".to_string()),
        });
        let properties: Vec<Value> = fields
            .pointer("/Data/Properties")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        sources.push((properties, Some(omod_stub)));

        let flags = entry.header.as_ref().map_or(0, |h| h.flags);
        if depth < OMOD_INCLUDE_MAX_DEPTH && include_role(flags) == IncludeRole::Compose {
            for child in included(&fields) {
                if visited.insert(child) {
                    queue.push_back((child, depth + 1));
                }
            }
        }
    }

    Ok(sources)
}

/// One reverse-`refs` call per [`CONSUMER_TYPES`] entry (SPEL, PERK) against a
/// keyword/AVIF `target_fid` — the "who reads this?" half of the chase
/// pattern. Shared verbatim by [`reverse_chase`] (which flattens every type's
/// rows into one evidence list) and `walk`'s KYWD/AVIF digest (which keeps
/// SPEL/PERK grouped under separate headers) — factored here so the `refs`
/// call sequence isn't duplicated between the two callers.
pub(crate) fn consumer_refs_by_type(
    f: &mut impl RecordSource,
    target_fid: FormId,
    depth: RefDepth,
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
    f: &mut impl RecordSource,
    target: &Value,
    depth: RefDepth,
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

    let row_fid = |row: &RefRow| crate::parse_form_id_input(&row.form_id).ok();
    let mut fids: Vec<FormId> = rows.iter().filter_map(row_fid).collect();
    dedup_sorted(&mut fids);
    let by_fid = bulk_fetch_map(f, &fids)?;

    let mut evidence = Vec::new();
    for row in &rows {
        let entry = row_fid(row).and_then(|fid| by_fid.get(&fid));
        let fields = entry.and_then(|e| e.fields.clone()).unwrap_or(Value::Null);
        let paths: Vec<Option<&str>> = match &row.field_paths {
            Some(p) if !p.is_empty() => p.iter().map(|s| Some(s.as_str())).collect(),
            _ => vec![None],
        };
        let row_value = serde_json::to_value(row).unwrap_or(Value::Null);
        for path in paths {
            let sliced = path.and_then(|p| slice_effect(&fields, p));
            let detail = match sliced {
                Some(effect) => EvidenceDetail::Effect {
                    effect: effect.clone(),
                },
                None => note(
                    "reference confirmed but the exact effect could not be isolated from the \
                     field path; inspect the full record",
                ),
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
    f: &mut impl RecordSource,
    sources: &[(usize, Vec<Value>)],
) -> anyhow::Result<Vec<(usize, Evidence)>> {
    let all_targets: Vec<Value> = sources
        .iter()
        .flat_map(|(_, effects)| mgef_targets_in_effects_array(effects))
        .collect();
    if all_targets.is_empty() {
        return Ok(Vec::new());
    }
    let by_fid = fetch_stubs(f, &all_targets)?;

    let mut out = Vec::new();
    for (idx, effects) in sources {
        for target in mgef_targets_in_effects_array(effects) {
            if let Some(ev) = mgef_pass_through_evidence(&target, &by_fid) {
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
/// `f` is anything implementing [`RecordSource`] — normally the
/// `Database`-backed `DbSource` behind `Op::Chase` (`src/ops/analysis.rs`),
/// or a fake for tests (see `tests/chase.rs`).
pub fn chase(
    f: &mut impl RecordSource,
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
            let flags = entry.header.as_ref().map_or(0, |h| h.flags);
            return omod_chase(f, root, &fields, flags, opts);
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
/// direct-property/perk-grant/keyword-hook/tag-keyword (see the module docs)
/// and forward- or reverse-fetch whatever record carries the mechanic. The
/// mod templates it includes contribute their rows too, up to
/// [`OMOD_INCLUDE_MAX_DEPTH`] / [`OMOD_INCLUDE_ENQUEUE_CAP`], tagged with
/// [`Hop::source_omod`]; a `Mod Collection`/`Mod Selector` root lists its
/// includes as [`ChaseTree::alternatives`] instead (see [`IncludeRole`]).
/// `root_flags` is the root's record-header flags.
///
/// `pub(crate)` (rather than only reachable through [`chase`]'s dispatch) so
/// `esm::walk`'s OMOD digest can classify an already-fetched root directly —
/// see that module's `digest_node`. Building the `RootStub` is cheap and the
/// caller doesn't need to have paid for a fresh `bulk_get` first.
pub(crate) fn omod_chase(
    f: &mut impl RecordSource,
    root: RootStub,
    fields: &Value,
    root_flags: u32,
    opts: &ChaseOptions,
) -> anyhow::Result<ChaseTree> {
    let sources = collect_property_sources(f, fields, root_flags)?;

    // ---- one bulk_get for every KYWD-typed property target (Type/Notes) ----
    let kywd_targets: Vec<Value> = sources
        .iter()
        .flat_map(|(properties, _)| properties)
        .map(|prop| field_or_null(prop.get("Value 1")))
        .filter(|value1| {
            is_ref_stub(value1) && value1.get("record_type").and_then(Value::as_str) == Some("KYWD")
        })
        .collect();
    let kywd_by_fid = fetch_stubs(f, &kywd_targets)?;

    let mut hops: Vec<Hop> = Vec::new();
    let mut forward_targets: Vec<(usize, Value)> = Vec::new();
    let mut reverse_targets: Vec<(usize, Value)> = Vec::new();

    for (properties, source_omod) in &sources {
        for (i, prop) in properties.iter().enumerate() {
            let (hop, dest) = classify_property_row(prop, i, source_omod.clone(), &kywd_by_fid);
            let hop_idx = hops.len();
            match dest {
                Some(FetchDest::Forward(t)) => forward_targets.push((hop_idx, t)),
                Some(FetchDest::Reverse(t)) => reverse_targets.push((hop_idx, t)),
                None => {}
            }
            hops.push(hop);
        }
    }

    // ---- forward fetch (perk_grant + direct ENCH/SPEL/PROJ attachments) ----
    if !forward_targets.is_empty() {
        let by_fid = fetch_stubs(f, forward_targets.iter().map(|(_, t)| t))?;

        // One batched follow-up fetch for every PROJ target's Data.Explosion
        // (PROJ evidence needs the linked explosion).
        let is_proj =
            |target: &Value| target.get("record_type").and_then(Value::as_str) == Some("PROJ");
        let explosions: Vec<&Value> = forward_targets
            .iter()
            .filter(|(_, target)| is_proj(target))
            .filter_map(|(_, target)| fetched(&by_fid, target)?.fields.as_ref())
            .filter_map(|fields| fields.pointer("/Data/Explosion"))
            .collect();
        let expl_by_fid = fetch_stubs(f, explosions)?;

        for (i, target) in &forward_targets {
            if is_proj(target) {
                let proj_fields = fetched(&by_fid, target)
                    .and_then(|e| e.fields.as_ref())
                    .unwrap_or(&Value::Null);
                hops[*i].evidence = vec![projectile_evidence(target, proj_fields, &expl_by_fid)];
            } else {
                hops[*i].evidence = vec![forward_evidence(target, &by_fid)];
            }
        }

        // MGEF pass-through: if a forward-fetched target's own Effects[] carry
        // a Base Effect resolving to an MGEF with "Perk to Apply"/"Equip
        // Ability" set (the ENCH -> MGEF -> PERK "tech migration" pattern from
        // the mechanics KB), surface it as one more Evidence entry on the hop.
        let mgef_sources: Vec<(usize, Vec<Value>)> = forward_targets
            .iter()
            .filter_map(|(i, target)| {
                let rt = target
                    .get("record_type")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if rt == "PROJ" {
                    return None;
                }
                let effects = hops[*i].evidence.first()?.detail.effects()?.to_vec();
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

    // ---- demote empty KeywordHooks to TagKeyword (untyped, nothing gates) ----
    for hop in &mut hops {
        if hop.kind != HopKind::KeywordHook || !hop.evidence.is_empty() {
            continue;
        }
        let Some(target) = hop.target.clone() else {
            continue;
        };
        // Only demote when we successfully fetched the KYWD's own record —
        // without Type/Notes we can't build the synthetic tag evidence the
        // walk renderer expects, and a failed lookup leaves the hop as a
        // KeywordHook dead-end (test sources that omit KYWD bodies).
        let Some(fields) = kywd_fields_from_map(&kywd_by_fid, &target) else {
            continue;
        };
        hop.kind = HopKind::TagKeyword;
        // Reset `resolution`: it still holds `Reverse` from the discarded
        // KeywordHook reverse-chase attempt, but the hop's evidence below is
        // now the synthetic tag evidence every other TagKeyword hop carries
        // (`resolution: None`) — keep the two fields consistent.
        hop.resolution = None;
        hop.evidence = vec![tag_keyword_evidence(&target, Some(fields), json!("None"))];
    }

    let alternatives = match include_role(root_flags) {
        IncludeRole::Alternatives => include_alternatives(fields),
        IncludeRole::Compose => Vec::new(),
    };
    Ok(ChaseTree {
        root,
        hops,
        effect_hops: Vec::new(),
        alternatives,
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
    f: &mut impl RecordSource,
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
            if let Some(base) = inner.get("Base Effect")
                && is_ref_stub(base)
            {
                kind = EffectHopKind::BaseEffect;
                target = Some(stub(base));
            }
            if target.is_none() {
                for key in PERK_EFFECT_TARGET_KEYS {
                    if let Some(t) = inner.get(key)
                        && is_ref_stub(t)
                    {
                        kind = EffectHopKind::ForwardTarget;
                        target = Some(stub(t));
                        break;
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
        let by_fid = fetch_stubs(f, forward_targets.iter().map(|(_, t)| t))?;
        for (i, target) in &forward_targets {
            hops[*i].evidence = vec![forward_evidence(target, &by_fid)];
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
        if let Some(effects) = hops[*i].evidence.first().and_then(|ev| ev.detail.effects()) {
            mgef_sources.push((*i, effects.to_vec()));
        }
    }
    for (idx, ev) in mgef_pass_through(f, &mgef_sources)? {
        hops[idx].evidence.push(ev);
    }

    Ok(ChaseTree {
        root,
        hops: Vec::new(),
        effect_hops: hops,
        alternatives: Vec::new(),
    })
}

// This module has no rendering primitives — `esm::walk::render` owns all of
// them (`fmt_stub`, `summarize_effect`, ...), matching the module docs' and
// `docs/adr/0001-walk-interactive-chase-pipeline-json.md`'s "chase has no
// human text renderer of its own".

// ─── colocated unit tests for private helpers ───────────────────────────────
// `first_array_container`/`walk_path`/`named`/`is_formid_stub`/`stub` are
// private and not reachable from an external `tests/` integration crate, so
// these stay colocated (see esm/AGENTS.md's testing conventions).
#[cfg(test)]
mod tests {
    use super::*;

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
        let by_fid: Fetched = [(FormId::new(1), entry)].into_iter().collect();
        let ev = mgef_pass_through_evidence(&mgef_target, &by_fid).expect("evidence");
        let EvidenceDetail::PassThrough(pass) = &ev.detail else {
            panic!("expected pass-through evidence, got {:?}", ev.detail);
        };
        assert_eq!(pass.perk_to_apply.as_ref().unwrap()["formid"], json!("0x2"));
        assert_eq!(pass.equip_ability.as_ref().unwrap()["formid"], json!("0x3"));
    }

    /// Each detail variant's JSON is its own fields, and reads back as the
    /// same variant.
    #[test]
    fn evidence_detail_json_round_trips_per_variant() {
        let cases = [
            json!({"note": "fetch failed: no response"}),
            json!({"tag": true, "notes": "UI tag", "type": "None"}),
            json!({"effect": {"Effect": {"Base Effect": null}}}),
            json!({"description": "Grants bonus damage.", "effects": [], "effects_truncated": 3}),
            json!({"perk_to_apply": {"formid": "0x00000002"}}),
            json!({"speed": 5000.0, "type": "Missile", "explosion": {"formid": "0x3"},
                   "radius": [0.0, 256.0], "chain": false, "damage": [{"type": null, "amount": 50}]}),
        ];
        let variants = [
            "Note",
            "Tag",
            "Effect",
            "Record",
            "PassThrough",
            "Projectile",
        ];
        for (json, variant) in cases.into_iter().zip(variants) {
            let detail: EvidenceDetail = serde_json::from_value(json.clone()).unwrap();
            assert!(
                format!("{detail:?}").starts_with(variant),
                "{variant}: {detail:?}"
            );
            assert_eq!(serde_json::to_value(&detail).unwrap(), json, "{variant}");
        }
    }

    #[test]
    fn mgef_pass_through_evidence_returns_none_when_neither_field_set() {
        let mgef_target = json!({"formid": "0x1", "record_type": "MGEF"});
        let entry = ok_test_entry("0x1", json!({"Magic Effect Data": {"Data": {}}}));
        let by_fid: Fetched = [(FormId::new(1), entry)].into_iter().collect();
        assert!(mgef_pass_through_evidence(&mgef_target, &by_fid).is_none());
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
