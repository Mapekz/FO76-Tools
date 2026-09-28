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

use crate::fields::{dedup_sorted, is_truthy, is_truthy_json, named};
use crate::ops::{RecordSel, RefDepth};
use crate::source::{RecordSource, SourceRecord, bulk_fetch_map};
use crate::{FormId, RefList, RefRow, Resolved};
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
    /// `omod`'s FormID, for in-process consumers (walk). Not serialized.
    #[serde(skip)]
    #[cfg_attr(test, ts(skip))]
    pub id: FormId,
    pub minimum_level: u64,
}

/// The alternatives a `Mod Collection`/`Mod Selector` OMOD's decoded
/// `fields` include, in order.
pub(crate) fn include_alternatives(fields: &Resolved) -> Vec<IncludeAlternative> {
    fields
        .pointer("/Data/Includes")
        .map(Resolved::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|inc| {
            let omod = inc.get("Mod")?;
            let id = omod.stub_id()?;
            let minimum_level = inc
                .get("Minimum Level")
                .and_then(Resolved::as_u64)
                .unwrap_or(0);
            Some(IncludeAlternative {
                omod: omod.to_json(),
                id,
                minimum_level,
            })
        })
        .collect()
}

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
    /// `target`'s FormID, for in-process consumers (walk). Not serialized.
    #[serde(skip)]
    #[cfg_attr(test, ts(skip))]
    pub target_id: Option<FormId>,
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

// ─── record-body helpers ────────────────────────────────────────────────────

fn field_or_null(field: Option<&Resolved>) -> Value {
    field.map_or(Value::Null, Resolved::to_json)
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

/// Descend into a decoded record's `fields` along a dot/`[N]` path.
fn walk_path<'v>(fields: &'v Resolved, path: &str) -> Option<&'v Resolved> {
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

fn slice_effect<'v>(fields: &'v Resolved, path: &str) -> Option<&'v Resolved> {
    let container = first_array_container(path)?;
    walk_path(fields, &container)
}

/// A reference the chase follows: the target's FormID, and its stub as the
/// output shows it (`{"formid", "editor_id", "record_type"}`).
#[derive(Debug, Clone)]
struct Target {
    id: FormId,
    stub: Value,
}

impl Target {
    /// The target of a reference that resolved to a stub.
    fn of(reference: &Resolved) -> Option<Target> {
        let id = reference.stub_id()?;
        Some(Target {
            id,
            stub: json!({
                "formid": id.display(),
                "editor_id": field_or_null(reference.get("editor_id")),
                "record_type": field_or_null(reference.get("record_type")),
            }),
        })
    }

    fn record_type(&self) -> &str {
        self.stub
            .get("record_type")
            .and_then(Value::as_str)
            .unwrap_or("")
    }
}

/// A reverse-reference row's stub, in the same shape as [`Target::stub`].
fn row_stub(row: &RefRow) -> Value {
    json!({
        "formid": row.form_id,
        "editor_id": row.editor_id,
        "record_type": row.record_type,
    })
}

/// Scan an `Effects[]` array for `Effect."Base Effect"` reference stubs
/// whose `record_type` is `"MGEF"` (present on SPEL/ALCH/ENCH-shaped Effects;
/// absent on PERK's Ability/Quest/Item union — this check is naturally a
/// no-op there, no type-specific gating needed), deduped by FormID.
fn mgef_targets_in_effects_array(effects: &[Resolved]) -> Vec<Target> {
    let mut seen: HashSet<FormId> = HashSet::new();
    let mut out = Vec::new();
    for entry in effects {
        let Some(target) = entry
            .get("Effect")
            .and_then(Resolved::as_object)
            .and_then(|inner| inner.get("Base Effect"))
            .and_then(Target::of)
        else {
            continue;
        };
        if target.record_type() == "MGEF" && seen.insert(target.id) {
            out.push(target);
        }
    }
    out
}

/// Records fetched by FormID (see [`fetch_targets`]).
type Fetched = HashMap<FormId, SourceRecord>;

/// Fetch, once each, the records these targets name.
fn fetch_targets<'t>(
    f: &mut impl RecordSource,
    targets: impl IntoIterator<Item = &'t Target>,
) -> anyhow::Result<Fetched> {
    let mut fids: Vec<FormId> = targets.into_iter().map(|t| t.id).collect();
    dedup_sorted(&mut fids);
    bulk_fetch_map(f, &fids)
}

/// The fetched record a target names.
fn fetched<'a>(by_fid: &'a Fetched, target: &Target) -> Option<&'a SourceRecord> {
    by_fid.get(&target.id)
}

/// Given an already-bulk-fetched MGEF target, extract `"Perk to Apply"`/`"Equip Ability"` into a compact
/// [`Evidence`]. `None` if the MGEF wasn't found/failed to fetch, or has
/// neither field set — the common case, most magic effects are plain
/// damage/buff effects with nothing further to chase.
fn mgef_pass_through_evidence(mgef_target: &Target, by_fid: &Fetched) -> Option<Evidence> {
    let entry = fetched(by_fid, mgef_target)?;
    let fields = entry.fields.as_ref()?;
    let perk_to_apply = walk_path(fields, "Magic Effect Data.Data.Perk to Apply")
        .filter(|v| is_truthy(Some(*v)))
        .map(Resolved::to_json);
    let equip_ability = walk_path(fields, "Magic Effect Data.Data.Equip Ability")
        .filter(|v| is_truthy(Some(*v)))
        .map(Resolved::to_json);
    if perk_to_apply.is_none() && equip_ability.is_none() {
        return None;
    }
    Some(Evidence {
        source: mgef_target.stub.clone(),
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

fn note(text: impl Into<String>) -> EvidenceDetail {
    EvidenceDetail::Note { note: text.into() }
}

/// Cap on the `Effects[]` rows a forward-fetched record's evidence shows.
const EVIDENCE_EFFECTS_CAP: usize = 12;

fn forward_evidence(target: &Target, by_fid: &Fetched) -> Evidence {
    let evidence = |detail| Evidence {
        source: target.stub.clone(),
        via: None,
        detail,
        hop_depth: None,
        path_chain: None,
    };
    let Some(entry) = fetched(by_fid, target) else {
        return evidence(note("fetch failed: no response"));
    };
    if let Some(err) = &entry.error {
        return evidence(note(format!("fetch failed: {err}")));
    }
    let fields = entry.fields.as_ref().unwrap_or(&Resolved::Null);
    let mut detail = RecordDetail::default();
    if is_truthy(fields.get("Description")) {
        detail.description = Some(fields["Description"].to_json());
    }
    let effects = fields
        .get("Effects")
        .map(Resolved::items)
        .unwrap_or_default();
    if !effects.is_empty() {
        let capped: Vec<Value> = effects
            .iter()
            .take(EVIDENCE_EFFECTS_CAP)
            .map(Resolved::to_json)
            .collect();
        let truncated = effects.len().saturating_sub(capped.len());
        detail.effects = Some(capped);
        detail.effects_truncated = (truncated > 0).then_some(truncated);
    }
    evidence(
        if detail.description.is_none() && detail.effects.is_none() {
            note("no Description/Effects field on this record")
        } else {
            EvidenceDetail::Record(detail)
        },
    )
}

/// The `Effects[]` rows [`forward_evidence`] shows for `target`.
fn forward_effects<'a>(target: &Target, by_fid: &'a Fetched) -> &'a [Resolved] {
    let effects = fetched(by_fid, target)
        .filter(|entry| entry.error.is_none())
        .and_then(|entry| entry.fields.as_ref())
        .and_then(|fields| fields.get("Effects"))
        .map(Resolved::items)
        .unwrap_or_default();
    &effects[..effects.len().min(EVIDENCE_EFFECTS_CAP)]
}

/// Min/max `y` across an inline-resolved curve table's `curve` points, if any.
fn curve_y_range(curve_table: &Resolved) -> Option<(f64, f64)> {
    let points = curve_table.get("curve")?.as_array()?;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in points {
        let Some(y) = p.get("y").and_then(Resolved::as_f64) else {
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
pub(crate) fn summarize_explosion(fields: &Resolved) -> ExplosionSummary {
    let data = fields.get("Data");
    let mut summary = ExplosionSummary::default();
    let mut damage = Vec::new();

    if let Some(d) = data {
        let inner = field_or_null(d.get("Inner Radius"));
        let outer = field_or_null(d.get("Outer Radius"));
        if !inner.is_null() || !outer.is_null() {
            summary.radius = Some([inner, outer]);
        }
        if is_truthy(d.get("Force")) {
            summary.force = Some(d["Force"].to_json());
        }
        let stagger = named(d.get("Stagger"));
        if is_truthy_json(Some(&stagger)) {
            summary.stagger = Some(stagger);
        }
        if let Some(ipds) = d.get("Impact Data Set").and_then(Target::of) {
            summary.impact_data_set = Some(match &ipds.stub["editor_id"] {
                Value::Null => json!(ipds.id.display()),
                edid => edid.clone(),
            });
        }
        summary.chain = Some(
            d.pointer("/Flags1/flags")
                .and_then(Resolved::as_array)
                .is_some_and(|flags| flags.iter().any(|f| f.as_str() == Some("Chain"))),
        );
        summary.placed_object = d.get("Placed Object").and_then(Target::of).map(|t| t.stub);
        summary.spawn_projectile = d
            .get("Spawn Projectile")
            .and_then(Target::of)
            .map(|t| t.stub);
    }

    let curve_row = |ct: &Resolved, kind: Option<Value>| ExplosionDamage {
        kind,
        curve: Some(field_or_null(ct.get("editor_id"))),
        range: curve_y_range(ct).map(|(lo, hi)| [lo, hi]),
        ..ExplosionDamage::default()
    };
    for entry in fields
        .get("Damage Types")
        .map(Resolved::items)
        .unwrap_or_default()
    {
        let kind = Some(field_or_null(entry.pointer("/Type/editor_id")));
        damage.push(
            match entry.get("Curve Table").filter(|v| is_truthy(Some(*v))) {
                Some(ct) => curve_row(ct, kind),
                None => ExplosionDamage {
                    kind,
                    amount: is_truthy(entry.get("Amount")).then(|| entry["Amount"].to_json()),
                    ..ExplosionDamage::default()
                },
            },
        );
    }
    if let Some(d) = data {
        if let Some(ct) = d.get("Damage Curve Table").filter(|v| is_truthy(Some(*v))) {
            damage.push(curve_row(ct, None));
        }
        if is_truthy(d.get("Base Weapon Damage Mult")) {
            damage.push(ExplosionDamage {
                base_weapon_mult: Some(d["Base Weapon Damage Mult"].to_json()),
                ..ExplosionDamage::default()
            });
        }
        if is_truthy(d.get("Damage")) {
            damage.push(ExplosionDamage {
                flat: Some(d["Damage"].to_json()),
                ..ExplosionDamage::default()
            });
        }
    }
    summary.damage = Some(damage);
    summary
}

/// Build forward evidence for a PROJ-targeting OMOD property: speed/type plus
/// the linked EXPL's radius/force/stagger/chain/damage summary when present.
fn projectile_evidence(target: &Target, proj_fields: &Resolved, expl_by_fid: &Fetched) -> Evidence {
    let mut detail = ProjectileDetail::default();
    if let Some(data) = proj_fields.get("Data") {
        if is_truthy(data.get("Speed")) {
            detail.speed = Some(data["Speed"].to_json());
        }
        let proj_type = named(data.get("Type"));
        if is_truthy_json(Some(&proj_type)) {
            detail.kind = Some(proj_type);
        }
        if let Some(expl) = data.get("Explosion").and_then(Target::of) {
            if let Some(entry) = fetched(expl_by_fid, &expl)
                && entry.error.is_none()
            {
                let expl_fields = entry.fields.as_ref().unwrap_or(&Resolved::Null);
                detail.summary = summarize_explosion(expl_fields);
            }
            detail.explosion = Some(expl.stub);
        }
    }
    Evidence {
        source: target.stub.clone(),
        via: None,
        detail: EvidenceDetail::Projectile(Box::new(detail)),
        hop_depth: None,
        path_chain: None,
    }
}

/// Synthetic evidence for a [`HopKind::TagKeyword`]: the KYWD's own Notes/Type,
/// not a reverse-chased consumer.
fn tag_keyword_evidence(
    target: &Target,
    kywd_fields: Option<&Resolved>,
    type_name: Value,
) -> Evidence {
    let notes = kywd_fields
        .and_then(|f| f.get("Notes"))
        .filter(|n| !n.is_null())
        .map_or(Value::Null, Resolved::to_json);
    Evidence {
        source: target.stub.clone(),
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
fn kywd_fields_from_map<'a>(by_fid: &'a Fetched, target: &Target) -> Option<&'a Resolved> {
    let entry = fetched(by_fid, target)?;
    if entry.error.is_some() {
        return None;
    }
    entry.fields.as_ref()
}

/// True when a KYWD's `Type.name` is a populated enum other than `"None"` —
/// those are categorically item-policy / UI tags, never SPEL/PERK gates.
fn is_populated_kywd_type(type_name: &Value) -> bool {
    is_truthy_json(Some(type_name)) && type_name.as_str() != Some("None")
}

/// Classify one OMOD `Data.Properties[]` row into a [`Hop`] plus an optional
/// forward/reverse fetch destination. Shared by the root's own properties and
/// by include-expanded rows so the KYWD/PERK/AVIF/PROJ/forward-type dispatch
/// lives in one place.
fn classify_property_row(
    prop: &Resolved,
    property_index: usize,
    source_omod: Option<Value>,
    kywd_by_fid: &Fetched,
) -> (Hop, Option<FetchDest>) {
    let value1 = prop.get("Value 1");
    let mut hop = Hop {
        property_index,
        property: named(prop.get("Property")),
        function: named(prop.get("Function Type")),
        value1: field_or_null(value1),
        value2: field_or_null(prop.get("Value 2")),
        curve_table: prop
            .get("Curve Table")
            .filter(|v| is_truthy(Some(*v)))
            .map(Resolved::to_json),
        kind: HopKind::DirectProperty,
        target: None,
        target_id: None,
        resolution: None,
        source_omod,
        evidence: Vec::new(),
    };

    let Some(target) = value1.and_then(Target::of) else {
        return (hop, None);
    };
    let rt = target.record_type().to_string();
    hop.target = Some(target.stub.clone());
    hop.target_id = Some(target.id);

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
    Forward(Target),
    Reverse(Target),
}

/// Collect `(properties, source_omod)` batches for the root plus a bounded
/// BFS over the `Data.Includes[]` that compose into it (depth ≤
/// [`OMOD_INCLUDE_MAX_DEPTH`], breadth ≤ [`OMOD_INCLUDE_ENQUEUE_CAP`] per
/// level). An OMOD whose includes are alternatives contributes only its own
/// properties.
fn collect_property_sources(
    f: &mut impl RecordSource,
    root_fields: &Resolved,
    root_flags: u32,
) -> anyhow::Result<Vec<(Vec<Resolved>, Option<Value>)>> {
    let properties = |fields: &Resolved| -> Vec<Resolved> {
        fields
            .pointer("/Data/Properties")
            .map(Resolved::items)
            .unwrap_or_default()
            .to_vec()
    };
    let mut sources: Vec<(Vec<Resolved>, Option<Value>)> = vec![(properties(root_fields), None)];

    // The mod templates an OMOD's `Data.Includes[]` names, capped per level.
    let included = |fields: &Resolved| -> Vec<FormId> {
        fields
            .pointer("/Data/Includes")
            .map(Resolved::items)
            .unwrap_or_default()
            .iter()
            .take(OMOD_INCLUDE_ENQUEUE_CAP)
            .filter_map(|inc| inc.get("Mod").and_then(Resolved::stub_id))
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
        let fetched = f.bulk_get(&[RecordSel::FormId(fid)])?;
        let Some(entry) = fetched.into_iter().next() else {
            continue;
        };
        if entry.error.is_some() {
            continue;
        }
        let fields = entry.fields.unwrap_or(Resolved::Null);
        let omod_stub = json!({
            "formid": fid.display(),
            "editor_id": entry.editor_id.unwrap_or_default(),
            "record_type": entry.header.as_ref().map(|h| h.signature.clone()).unwrap_or_else(|| "OMOD".to_string()),
        });
        sources.push((properties(&fields), Some(omod_stub)));

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
    target: &Target,
    depth: RefDepth,
    limit: usize,
) -> anyhow::Result<Vec<Evidence>> {
    let rows: Vec<RefRow> = consumer_refs_by_type(f, target.id, depth, limit)?
        .into_iter()
        .flat_map(|(_, ref_list)| ref_list.rows)
        .collect();
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut fids: Vec<FormId> = rows.iter().map(|row| row.id).collect();
    dedup_sorted(&mut fids);
    let by_fid = bulk_fetch_map(f, &fids)?;

    let mut evidence = Vec::new();
    for row in &rows {
        let fields = by_fid
            .get(&row.id)
            .and_then(|e| e.fields.as_ref())
            .unwrap_or(&Resolved::Null);
        let paths: Vec<Option<&str>> = match &row.field_paths {
            Some(p) if !p.is_empty() => p.iter().map(|s| Some(s.as_str())).collect(),
            _ => vec![None],
        };
        for path in paths {
            let sliced = path.and_then(|p| slice_effect(fields, p));
            let detail = match sliced {
                Some(effect) => EvidenceDetail::Effect {
                    effect: effect.to_json(),
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
                source: row_stub(row),
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
/// effects, or a root's own Base-Effect-shaped entry). Returns
/// `(index, Evidence)` pairs for the caller to push onto the right hop's
/// evidence list; a source with no MGEF carrying a pass-through field
/// contributes nothing.
fn mgef_pass_through(
    f: &mut impl RecordSource,
    sources: &[(usize, Vec<Resolved>)],
) -> anyhow::Result<Vec<(usize, Evidence)>> {
    let all_targets: Vec<Target> = sources
        .iter()
        .flat_map(|(_, effects)| mgef_targets_in_effects_array(effects))
        .collect();
    if all_targets.is_empty() {
        return Ok(Vec::new());
    }
    let by_fid = fetch_targets(f, &all_targets)?;

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

fn build_root_stub(entry: &SourceRecord, fields: &Resolved) -> RootStub {
    RootStub {
        formid: entry.header.as_ref().map(|h| h.form_id.display()),
        record_type: entry.header.as_ref().map(|h| h.signature.clone()),
        editor_id: entry.editor_id.clone(),
        name: fields
            .get("Name")
            .filter(|v| is_truthy(Some(*v)))
            .map(Resolved::to_json),
        description: fields
            .get("Description")
            .filter(|v| is_truthy(Some(*v)))
            .map(Resolved::to_json),
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
    let entries = f.bulk_get(std::slice::from_ref(&selector))?;
    let entry = entries
        .into_iter()
        .next()
        .with_context(|| format!("bulk_get returned no entries for {selector_display:?}"))?;
    if let Some(err) = &entry.error {
        bail!("failed to resolve {selector_display:?}: {err}");
    }

    let fields = entry.fields.clone().unwrap_or(Resolved::Null);
    let record_type = fields.get("_record_type").and_then(Resolved::as_str);
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
    fields: &Resolved,
    root_flags: u32,
    opts: &ChaseOptions,
) -> anyhow::Result<ChaseTree> {
    let sources = collect_property_sources(f, fields, root_flags)?;

    // ---- one bulk_get for every KYWD-typed property target (Type/Notes) ----
    let kywd_targets: Vec<Target> = sources
        .iter()
        .flat_map(|(properties, _)| properties)
        .filter_map(|prop| prop.get("Value 1").and_then(Target::of))
        .filter(|target| target.record_type() == "KYWD")
        .collect();
    let kywd_by_fid = fetch_targets(f, &kywd_targets)?;

    let mut hops: Vec<Hop> = Vec::new();
    let mut forward_targets: Vec<(usize, Target)> = Vec::new();
    let mut reverse_targets: Vec<(usize, Target)> = Vec::new();

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
        let by_fid = fetch_targets(f, forward_targets.iter().map(|(_, t)| t))?;

        // One batched follow-up fetch for every PROJ target's Data.Explosion
        // (PROJ evidence needs the linked explosion).
        let is_proj = |target: &Target| target.record_type() == "PROJ";
        let explosions: Vec<Target> = forward_targets
            .iter()
            .filter(|(_, target)| is_proj(target))
            .filter_map(|(_, target)| fetched(&by_fid, target)?.fields.as_ref())
            .filter_map(|fields| fields.pointer("/Data/Explosion").and_then(Target::of))
            .collect();
        let expl_by_fid = fetch_targets(f, &explosions)?;

        for (i, target) in &forward_targets {
            if is_proj(target) {
                let proj_fields = fetched(&by_fid, target)
                    .and_then(|e| e.fields.as_ref())
                    .unwrap_or(&Resolved::Null);
                hops[*i].evidence = vec![projectile_evidence(target, proj_fields, &expl_by_fid)];
            } else {
                hops[*i].evidence = vec![forward_evidence(target, &by_fid)];
            }
        }

        // MGEF pass-through: if a forward-fetched target's own Effects[] carry
        // a Base Effect resolving to an MGEF with "Perk to Apply"/"Equip
        // Ability" set (the ENCH -> MGEF -> PERK "tech migration" pattern from
        // the mechanics KB), surface it as one more Evidence entry on the hop.
        let mgef_sources: Vec<(usize, Vec<Resolved>)> = forward_targets
            .iter()
            .filter(|(_, target)| !is_proj(target))
            .map(|(i, target)| (*i, forward_effects(target, &by_fid).to_vec()))
            .filter(|(_, effects)| !effects.is_empty())
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
        let (Some(id), Some(stub)) = (hop.target_id, hop.target.clone()) else {
            continue;
        };
        let target = Target { id, stub };
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
    fields: &Resolved,
) -> anyhow::Result<ChaseTree> {
    let effects = fields
        .get("Effects")
        .map(Resolved::items)
        .unwrap_or_default();

    let mut hops: Vec<EffectHop> = Vec::with_capacity(effects.len());
    let mut forward_targets: Vec<(usize, Target)> = Vec::new();

    for (i, entry) in effects.iter().enumerate() {
        let inner_obj = entry.get("Effect").and_then(Resolved::as_object);

        let mut kind = EffectHopKind::NoTarget;
        let mut target: Option<Target> = None;

        if let Some(inner) = inner_obj {
            if let Some(base) = inner.get("Base Effect").and_then(Target::of) {
                kind = EffectHopKind::BaseEffect;
                target = Some(base);
            }
            if target.is_none()
                && let Some(t) = PERK_EFFECT_TARGET_KEYS
                    .iter()
                    .find_map(|key| inner.get(*key).and_then(Target::of))
            {
                kind = EffectHopKind::ForwardTarget;
                target = Some(t);
            }
        }

        if kind == EffectHopKind::ForwardTarget
            && let Some(t) = &target
        {
            forward_targets.push((i, t.clone()));
        }

        hops.push(EffectHop {
            effect_index: i,
            kind,
            effect: entry.to_json(),
            target: target.map(|t| t.stub),
            evidence: Vec::new(),
        });
    }

    // ---- MGEF pass-through sources: the root's own Base-Effect-shaped
    // entries, plus any forward-fetched target's own Effects[] (e.g. a PERK
    // rank granting a SPEL Ability whose Base Effect -> MGEF carries "Perk
    // to Apply"). ----
    let mut mgef_sources: Vec<(usize, Vec<Resolved>)> = hops
        .iter()
        .filter(|hop| hop.kind == EffectHopKind::BaseEffect)
        .map(|hop| (hop.effect_index, vec![effects[hop.effect_index].clone()]))
        .collect();

    // ---- forward fetch (PERK's Ability/Quest/Spell/Item/Leveled Item targets): 1 bulk call ----
    if !forward_targets.is_empty() {
        let by_fid = fetch_targets(f, forward_targets.iter().map(|(_, t)| t))?;
        for (i, target) in &forward_targets {
            hops[*i].evidence = vec![forward_evidence(target, &by_fid)];
            let effects = forward_effects(target, &by_fid);
            if !effects.is_empty() {
                mgef_sources.push((*i, effects.to_vec()));
            }
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

    fn r(v: Value) -> Resolved {
        Resolved::from_stub_json(&v)
    }

    fn target(v: Value) -> Target {
        Target::of(&r(v)).expect("a reference stub")
    }

    #[test]
    fn walk_path_descends_through_objects_and_arrays() {
        let fields = r(json!({
            "Effects": [
                {"Effect": {"Base Effect": {"formid": "0x1"}}},
                {"Effect": {"Base Effect": {"formid": "0x2"}}},
            ]
        }));
        let found = walk_path(&fields, "Effects[1].Effect.Base Effect");
        assert_eq!(found.and_then(Resolved::stub_id), Some(FormId(2)));
    }

    #[test]
    fn walk_path_returns_none_on_missing_key_or_out_of_range_index() {
        let fields = r(json!({"Effects": [{"a": 1}]}));
        assert_eq!(walk_path(&fields, "Effects[5].a"), None);
        assert_eq!(walk_path(&fields, "Missing.Key"), None);
    }

    #[test]
    fn slice_effect_returns_the_containing_array_element() {
        let fields = r(json!({
            "Effects": [
                {"Effect": {"x": 1}},
                {"Effect": {"x": 2, "Conditions": {"Conditions": []}}},
            ]
        }));
        let sliced = slice_effect(
            &fields,
            "Effects[1].Effect.Conditions.Conditions[0].Parameter 1",
        );
        assert_eq!(
            sliced.map(Resolved::to_json),
            Some(json!({"Effect": {"x": 2, "Conditions": {"Conditions": []}}}))
        );
    }

    #[test]
    fn target_and_row_stubs_share_one_shape() {
        let t =
            target(json!({"formid": "0x1", "editor_id": "e", "record_type": "KYWD", "Value": 1}));
        assert_eq!(t.id, FormId(1));
        assert_eq!(
            t.stub,
            json!({"formid": "0x00000001", "editor_id": "e", "record_type": "KYWD"})
        );
        let row = RefRow {
            form_id: "0x00000002".into(),
            id: FormId(2),
            editor_id: Some("f".into()),
            record_type: Some("SPEL".into()),
            ..RefRow::default()
        };
        assert_eq!(
            row_stub(&row),
            json!({"formid": "0x00000002", "editor_id": "f", "record_type": "SPEL"})
        );
    }

    #[test]
    fn an_unresolved_reference_is_not_a_target() {
        assert!(Target::of(&Resolved::unresolved(FormId(0x10))).is_none());
    }

    #[test]
    fn mgef_targets_in_effects_array_finds_mgef_base_effect() {
        let effects = vec![r(json!({
            "Effect": {"Base Effect": {"formid": "0x1", "editor_id": "e", "record_type": "MGEF"}}
        }))];
        let targets = mgef_targets_in_effects_array(&effects);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, FormId(1));
    }

    #[test]
    fn mgef_targets_in_effects_array_dedupes_repeated_mgef() {
        let effects = vec![
            r(json!({"Effect": {"Base Effect": {"formid": "0x1", "record_type": "MGEF"}}})),
            r(json!({"Effect": {"Base Effect": {"formid": "0x1", "record_type": "MGEF"}}})),
        ];
        assert_eq!(mgef_targets_in_effects_array(&effects).len(), 1);
    }

    #[test]
    fn mgef_targets_in_effects_array_no_ops_on_perk_shaped_entries() {
        // PERK's Ability-shaped effect has no "Base Effect" key at all.
        let effects = vec![r(json!({
            "Effect": {"Ability": {"formid": "0x1", "record_type": "SPEL"}}
        }))];
        assert!(mgef_targets_in_effects_array(&effects).is_empty());
    }

    #[test]
    fn mgef_targets_in_effects_array_ignores_non_mgef_base_effect() {
        let effects = vec![r(json!({
            "Effect": {"Base Effect": {"formid": "0x1", "record_type": "SPEL"}}
        }))];
        assert!(mgef_targets_in_effects_array(&effects).is_empty());
    }

    #[test]
    fn mgef_pass_through_evidence_extracts_perk_to_apply_and_equip_ability() {
        let mgef_target = target(json!({"formid": "0x1", "editor_id": "e", "record_type": "MGEF"}));
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
        let mgef_target = target(json!({"formid": "0x1", "record_type": "MGEF"}));
        let entry = ok_test_entry("0x1", json!({"Magic Effect Data": {"Data": {}}}));
        let by_fid: Fetched = [(FormId::new(1), entry)].into_iter().collect();
        assert!(mgef_pass_through_evidence(&mgef_target, &by_fid).is_none());
    }

    fn ok_test_entry(sel: &str, fields: Value) -> SourceRecord {
        SourceRecord {
            sel: sel.to_string(),
            header: None,
            editor_id: None,
            fields: Some(r(fields)),
            error: None,
        }
    }
}
