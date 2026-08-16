pub mod ba2;
pub mod backend;
pub mod chase;
pub mod compress;
pub mod ctda;
pub mod curves;
pub mod decode;
pub mod diff;
pub mod discover;
pub mod format;
pub mod formid;
pub mod hardcoded;
pub mod index;
pub mod ipc;
pub mod lvli;
pub mod progress;
pub mod query;
pub mod reader;
pub mod refs;
pub mod registry;
mod rkyvcache;
pub mod schema;
pub mod strings;
pub mod tree;
pub mod walk;
pub mod wildcard;

use crate::decode::{DecodeContext, decode_record};
use crate::formid::parse_formid;
use crate::index::Index;
use crate::reader::{EsmFile, FileInfo, ParsedRecord, RecordHeaderInfo, edid_from_subrecords};
use crate::schema::Schema;
use crate::strings::{Localization, StringKind};
use crate::tree::ChildRef;
use crate::wildcard::wildcard_match;
use anyhow::{Context, bail};
pub use decode::{FormIdRefResolver, FormIdStub, ResolveDepth};
pub use diff::{
    BodyDetail, DiffOptions, DiffResult, RecordDiff, RecordStub, RefName, apply_type_filter,
};
pub use formid::FormId;
pub use index::{CacheInventory, SearchMeta, cache_inventory};
pub use ipc::{
    BulkRecordEntry, CoverageReport, Markers, Op, RawRecordView, RawSubrecordView, RefList,
    RefPathNode, RefRow, Request, Response,
};
pub use reader::RecordMeta;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

// Re-export tree types. The tree module's RecordStub is distinct from
// diff::RecordStub (different form_id representation and purpose), so it is
// exported under the alias TreeRecordStub to avoid a name collision.
pub use tree::{GroupChild, GroupLabel, GroupNode, RecordStub as TreeRecordStub, TreeIndex};

/// Primary interface to a Fallout 76 ESM file.
///
/// Holds a memory-mapped ESM, a FormID/EditorID index, the embedded field
/// schema, and an optional localization table loaded from the sibling BA2.
pub struct Database {
    pub(crate) esm: EsmFile,
    pub(crate) index: Index,
    pub(crate) schema: Schema,
    /// Whether the ESM's TES4 header has the Localized flag set. Stays
    /// `pub` (unlike its siblings below) — `src/bin/cli.rs`'s `diff` command
    /// reads it directly across the bin/lib crate boundary.
    pub is_localized: bool,
    /// Resolved string tables, if a localization BA2 was found or supplied.
    pub(crate) localization: Option<Localization>,
    /// Optional curve index built from Startup BA2. When present, FormID fields
    /// whose `valid_refs` includes `"CURV"` have their curve data inlined.
    pub(crate) curves: Option<crate::curves::CurveIndex>,
    /// Per-record-type memoized decode, populated lazily by `filter_type_records`
    /// and `list_type_field_paths`. In-memory only — never persisted, no
    /// CACHE_VERSION bump (these are ephemeral, rebuilt each time the Database
    /// is opened; `tree`/`GroupLabel`/`RecordStub` in `tree.rs` are the only
    /// precedent for presentation-layer types, and this is analogous — it's not
    /// part of any of `Index`'s persisted rkyv sections at all).
    filter_cache: std::collections::HashMap<String, (usize, Vec<FilterCacheEntry>)>,
}

/// One memoized, fully-decoded record used by [`Database::filter_type_records`]
/// and [`Database::list_type_field_paths`].
struct FilterCacheEntry {
    form_id: FormId,
    editor_id: Option<String>,
    offset: u64,
    fields: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecordResult {
    pub header: RecordHeaderInfo,
    pub editor_id: Option<String>,
    #[cfg_attr(test, ts(type = "Record<string, unknown>"))]
    pub fields: Value,
}

/// Presentation type for the CLI's own `list_by_type` printing — does not cross
/// the N-API boundary (no napi binding calls `Database::list_by_type`), so it
/// is intentionally not derived for TS export; see esm-viewer/CLAUDE.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEntry {
    pub form_id: String,
    pub editor_id: Option<String>,
    pub full_lstring_id: Option<String>,
}

/// A tree row combining FormID, record type, EditorID, and resolved translated name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecordRow {
    pub form_id: String,
    pub record_type: Option<String>,
    pub editor_id: Option<String>,
    pub name: Option<String>,
    pub offset: u64,
}

/// Which fields to match against in [`Database::search`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
    /// Match only the EditorID.
    Edid,
    /// Match only the display name (FULL) and description (DESC).
    Name,
    /// Match EditorID **or** display name / description (default).
    Both,
}

/// Comparison operator for [`Database::filter_type_records`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    /// True iff the path resolves to a present value (even `null`). No `value` used.
    Exists,
    /// Numeric equality if both sides parse as numbers; otherwise a
    /// case-insensitive exact string match.
    Eq,
    /// Case-insensitive substring match; deep-scans if the resolved value is
    /// itself an object or array.
    Contains,
    /// Numeric greater-than.
    Gt,
    /// Numeric less-than.
    Lt,
    /// Numeric greater-than-or-equal.
    Gte,
    /// Numeric less-than-or-equal.
    Lte,
}

/// Maximum number of records of a single type decoded and cached by
/// [`Database::ensure_filter_cache`]. Types like REFR/NAVM/LAND can have tens
/// or hundreds of thousands of records; a full schema-driven decode of all of
/// them is meaningfully more expensive than the cheap header/EDID scans
/// `ensure_xref_index`/`ensure_search_index` already do at full-file scale.
/// `records_by_type` is FormID-sorted, so this is a stable, deterministic
/// subset rather than an arbitrary truncation.
const FILTER_SCAN_CAP: usize = 20_000;

/// `limit == 0` means "unlimited" — the shared convention every `limit`
/// parameter in this crate's public query API follows (`list_by_type`,
/// `list_type_records`, `search`, `filter_type_records`), restated
/// identically at each call site before this helper existed. Feed the
/// result to `Iterator::take`/`Vec::truncate`: `usize::MAX` items is
/// effectively "don't stop early" for any realistic in-memory collection.
fn effective_take(limit: usize) -> usize {
    if limit == 0 { usize::MAX } else { limit }
}

/// Evaluate a filter predicate against a decoded record's `fields` JSON body.
///
/// `path` is a dot-separated sequence of segments navigating into `fields`
/// (schema-driven key names, e.g. `"Data.Damage"`). A segment of `"[]"` means
/// "the current value must be a JSON array; recurse into every element for
/// the remaining path, matching if ANY element satisfies it". An empty/`None`
/// path means "deep-scan every value anywhere in the record, matching if ANY
/// value anywhere satisfies the operator".
fn predicate_matches(
    fields: &Value,
    path: Option<&str>,
    op: FilterOp,
    value: Option<&str>,
) -> bool {
    let path = path.map(str::trim).filter(|p| !p.is_empty());
    match path {
        None => deep_scan_matches(fields, op, value),
        Some(p) => {
            let segments: Vec<&str> = p.split('.').collect();
            navigate_matches(fields, &segments, op, value)
        }
    }
}

/// Walk `segments` into `current`, applying `[]` array-wildcard fan-out, and
/// test the operator once the path is exhausted. Returns `false` if the path
/// doesn't exist in the JSON (e.g. an object without the requested key).
fn navigate_matches(current: &Value, segments: &[&str], op: FilterOp, value: Option<&str>) -> bool {
    match segments.split_first() {
        None => op_matches(current, op, value),
        Some((&"[]", rest)) => match current {
            Value::Array(items) => items
                .iter()
                .any(|item| navigate_matches(item, rest, op, value)),
            _ => false,
        },
        Some((seg, rest)) => match current {
            Value::Object(map) => match map.get(*seg) {
                Some(next) => navigate_matches(next, rest, op, value),
                None => false,
            },
            _ => false,
        },
    }
}

/// Test the operator against a value reached via explicit path navigation.
/// `Contains` deep-scans when the terminal value is itself a container.
fn op_matches(current: &Value, op: FilterOp, value: Option<&str>) -> bool {
    match op {
        FilterOp::Exists => true,
        FilterOp::Contains => match current {
            Value::Object(_) | Value::Array(_) => deep_scan_matches(current, op, value),
            _ => value_matches(current, op, value),
        },
        _ => value_matches(current, op, value),
    }
}

/// Recurse through every value anywhere in `v` (objects' values, array
/// elements, and scalars), matching if ANY value satisfies the operator.
fn deep_scan_matches(v: &Value, op: FilterOp, value: Option<&str>) -> bool {
    if value_matches(v, op, value) {
        return true;
    }
    match v {
        Value::Object(map) => map.values().any(|vv| deep_scan_matches(vv, op, value)),
        Value::Array(items) => items.iter().any(|vv| deep_scan_matches(vv, op, value)),
        _ => false,
    }
}

/// Scalar-only operator test: containers never match directly here — the
/// caller's recursion (`deep_scan_matches`/`op_matches`) is responsible for
/// visiting a container's children.
fn value_matches(current: &Value, op: FilterOp, value: Option<&str>) -> bool {
    if matches!(current, Value::Object(_) | Value::Array(_)) {
        return false;
    }
    match op {
        FilterOp::Exists => true,
        FilterOp::Eq => eq_matches(current, value),
        FilterOp::Contains => match value {
            Some(needle) => stringify_scalar(current)
                .map(|s| s.to_lowercase().contains(&needle.to_lowercase()))
                .unwrap_or(false),
            None => false,
        },
        FilterOp::Gt | FilterOp::Lt | FilterOp::Gte | FilterOp::Lte => {
            numeric_matches(current, op, value)
        }
    }
}

/// Render a scalar JSON value as its natural display text (strings as raw
/// content, not JSON-quoted). Returns `None` for objects/arrays.
fn stringify_scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".to_string()),
        Value::Object(_) | Value::Array(_) => None,
    }
}

fn eq_matches(current: &Value, value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    if let Value::Number(n) = current
        && let (Some(cur_f), Ok(val_f)) = (n.as_f64(), value.parse::<f64>())
    {
        return cur_f == val_f;
    }
    match stringify_scalar(current) {
        Some(s) => s.eq_ignore_ascii_case(value),
        None => false,
    }
}

fn numeric_matches(current: &Value, op: FilterOp, value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let Ok(val_f) = value.parse::<f64>() else {
        return false;
    };
    let cur_f = match current {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    };
    let Some(cur_f) = cur_f else {
        return false;
    };
    match op {
        FilterOp::Gt => cur_f > val_f,
        FilterOp::Lt => cur_f < val_f,
        FilterOp::Gte => cur_f >= val_f,
        FilterOp::Lte => cur_f <= val_f,
        _ => false,
    }
}

/// Collect every dot-notation field path present in `v` into `out`, capping
/// defensively once `out` reaches `cap` entries. Array levels collapse to a
/// literal `"[]"` segment regardless of index.
fn collect_field_paths(v: &Value, prefix: &str, out: &mut HashSet<String>, cap: usize) {
    if out.len() >= cap {
        return;
    }
    match v {
        Value::Object(map) => {
            for (k, vv) in map {
                let next = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                out.insert(next.clone());
                collect_field_paths(vv, &next, out, cap);
                if out.len() >= cap {
                    return;
                }
            }
        }
        Value::Array(items) => {
            let next = if prefix.is_empty() {
                "[]".to_string()
            } else {
                format!("{prefix}.[]")
            };
            out.insert(next.clone());
            for item in items {
                collect_field_paths(item, &next, out, cap);
                if out.len() >= cap {
                    return;
                }
            }
        }
        _ => {}
    }
}

/// Walk a decoded record body collecting every JSON path where a leaf string
/// value equals `target` (a FormID hex string, e.g. `"0x0004FE3D"`).
///
/// Unlike [`collect_field_paths`] (which collapses every array level to a
/// literal `"[]"` segment for a type-level path union), array elements here
/// are indexed (`Key[N]`) — the point is the exact location(s) within one
/// specific decoded record, e.g. `Effects[2].Conditions[0].Parameter 1`. Backs
/// [`Database::formid_reference_paths`] (`refs --paths`).
fn collect_formid_paths(v: &Value, target: &str, prefix: String, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s == target => out.push(prefix),
        Value::Object(map) => {
            for (k, vv) in map {
                let next = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                collect_formid_paths(vv, target, next, out);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect_formid_paths(item, target, format!("{prefix}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// A raw decoded FormID field renders as exactly `"0x"` + 8 hex digits (see
/// `FormId::display`) — strict enough that no other decoded string field
/// (names, EditorIDs, enum labels) can collide with it, so no target
/// comparison is needed the way [`collect_formid_paths`] needs one.
fn looks_like_decoded_formid(s: &str) -> bool {
    s.len() == 10 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Collect every string value in `v` that looks like a raw decoded FormID
/// (see [`looks_like_decoded_formid`]), in traversal order with duplicates
/// kept — callers that need a deduplicated set should dedupe on the parsed
/// [`FormId`], not this raw string list. Backs [`Database::outgoing_formids`].
fn collect_all_formid_values(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if looks_like_decoded_formid(s) => out.push(s.clone()),
        Value::Object(map) => {
            for vv in map.values() {
                collect_all_formid_values(vv, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_all_formid_values(item, out);
            }
        }
        _ => {}
    }
}

/// Result envelope for [`Database::filter_type_records`] — reports both
/// whether the requested `limit` truncated the match list, and whether the
/// underlying decode itself was capped (see [`FILTER_SCAN_CAP`]) for a huge
/// type, so callers can honestly report "N of M possible matches, based on
/// the first K of L total records of this type" rather than silently
/// under-covering.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct FilterResult {
    pub rows: Vec<RecordRow>,
    /// Total matches found within the scanned set (may exceed rows.len() if `limit` truncated).
    pub matched: usize,
    /// How many records of this type were actually decoded and tested.
    pub scanned: usize,
    /// Total records of this type that exist in the file.
    pub total: usize,
    /// True if rows.len() < matched (the match list itself was truncated by `limit`).
    pub capped: bool,
    /// True if scanned < total (the decode pass itself stopped at FILTER_SCAN_CAP).
    pub scan_capped: bool,
}

/// A PERK "Entry Point" selector for [`Database::perks_by_entry_point`] —
/// either the enum's numeric id (some ids carry no name at all, e.g. one
/// that only `mod_custom_V63-BERTHA_Perk` uses) or a name pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryPointSpec {
    Id(u16),
    /// Case-insensitive exact match, unless `name` contains `*`, in which
    /// case it's matched via [`crate::wildcard::wildcard_match`] (same
    /// matcher `Database::search` uses).
    Name(String),
}

/// Kind of virtual-seed selector that produced a [`CarrierTag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub enum CarrierKind {
    EntryPoint,
    OmodProperty,
}

/// One tag a virtual-seed carrier matched under a selector (e.g. a PERK
/// entry point under an [`EntryPointSpec`]).
///
/// Carried on [`ipc::RefRow::tags`] so every reverse-ref row in a
/// carrier-seeded walk (such as `--entry-point`/`--ep`) can name which
/// hook(s) it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct CarrierTag {
    pub kind: CarrierKind,
    pub id: u16,
    pub name: Option<String>,
    /// Enum-space qualifier for a kind that has one (OMOD property scope:
    /// "weap"/"armo"/"npc"). `None` for `CarrierKind::EntryPoint`, which has
    /// no scope concept.
    pub scope: Option<String>,
}

/// Carrier records from a virtual selector (e.g. [`Database::perks_by_entry_point`]),
/// each tagged with the match(es) that selected it.
pub type Carriers = Vec<(FormId, Vec<CarrierTag>)>;

impl EntryPointSpec {
    /// Parse a CLI/MCP token: an all-ASCII-digit token is a numeric id,
    /// everything else is a name pattern — except a `0x`/`0X`-prefixed
    /// token, which is rejected outright rather than silently becoming a
    /// (never-matching) name pattern: it's unambiguously someone passing a
    /// FormID to `--entry-point` by mistake, and entry points are only ever
    /// selected by name or by their small decimal id, never hex.
    pub fn parse(s: &str) -> anyhow::Result<EntryPointSpec> {
        let trimmed = s.trim();
        if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            bail!(
                "'{trimmed}' looks like a FormID, not a PERK entry-point name or \
                 numeric id; use the positional target or --formid for a FormID lookup"
            );
        }
        if !trimmed.is_empty()
            && trimmed.bytes().all(|b| b.is_ascii_digit())
            && let Ok(id) = trimmed.parse::<u16>()
        {
            return Ok(EntryPointSpec::Id(id));
        }
        Ok(EntryPointSpec::Name(trimmed.to_string()))
    }

    fn display(&self) -> String {
        match self {
            EntryPointSpec::Id(id) => id.to_string(),
            EntryPointSpec::Name(n) => format!("'{n}'"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropScope {
    Weap,
    Armo,
    Npc,
}

impl PropScope {
    /// The `Data.Form Type` enum name this scope's OMODs decode to.
    fn form_type_name(self) -> &'static str {
        match self {
            PropScope::Weap => "Weapon",
            PropScope::Armo => "Armor",
            PropScope::Npc => "Non-player character",
        }
    }

    fn tag_str(self) -> &'static str {
        match self {
            PropScope::Weap => "weap",
            PropScope::Armo => "armo",
            PropScope::Npc => "npc",
        }
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        if prefix.eq_ignore_ascii_case("weap") || prefix.eq_ignore_ascii_case("weapon") {
            Some(PropScope::Weap)
        } else if prefix.eq_ignore_ascii_case("armo") || prefix.eq_ignore_ascii_case("armor") {
            Some(PropScope::Armo)
        } else if prefix.eq_ignore_ascii_case("npc") || prefix.eq_ignore_ascii_case("npc_") {
            Some(PropScope::Npc)
        } else {
            None
        }
    }

    fn from_form_type_name(name: &str) -> Option<Self> {
        [PropScope::Weap, PropScope::Armo, PropScope::Npc]
            .into_iter()
            .find(|scope| name == scope.form_type_name())
    }
}

/// An OMOD Property selector for [`Database::omods_by_property`] — optionally
/// scoped to the weapon, armor, or NPC property enum space, then selected by
/// numeric id or name pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmodPropertySpec {
    scope: Option<PropScope>,
    sel: OmodPropertySel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OmodPropertySel {
    Id(u16),
    Name(String),
}

impl OmodPropertySpec {
    /// Parse a CLI/MCP token. Numeric ids require a form-type scope because
    /// each OMOD property enum space assigns different meanings to the same
    /// number. Names may be scoped or may fan out across all three spaces.
    pub fn parse(s: &str) -> anyhow::Result<OmodPropertySpec> {
        let trimmed = s.trim();
        if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            bail!(
                "'{trimmed}' looks like a FormID, not an OMOD property name or \
                 numeric id; use the positional target or --formid for a FormID lookup"
            );
        }

        let (scope, rest) = match trimmed.split_once(':') {
            Some((prefix, rest)) => match PropScope::from_prefix(prefix) {
                Some(scope) => (Some(scope), rest),
                None => (None, trimmed),
            },
            None => (None, trimmed),
        };
        let sel = if !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
            && let Ok(id) = rest.parse::<u16>()
        {
            if scope.is_none() {
                bail!("property ids are per-form-type; use weap:<id>, armo:<id>, or npc:<id>");
            }
            OmodPropertySel::Id(id)
        } else {
            OmodPropertySel::Name(rest.to_string())
        };

        Ok(OmodPropertySpec { scope, sel })
    }

    fn display(&self) -> String {
        match (self.scope, &self.sel) {
            (Some(scope), OmodPropertySel::Id(id)) => format!("{}:{id}", scope.tag_str()),
            (Some(scope), OmodPropertySel::Name(name)) => {
                format!("{}:'{name}'", scope.tag_str())
            }
            (None, OmodPropertySel::Id(id)) => id.to_string(),
            (None, OmodPropertySel::Name(name)) => format!("'{name}'"),
        }
    }
}

/// `true` if `name` satisfies `pattern` per [`EntryPointSpec::Name`]'s
/// matching rule: exact case-insensitive unless `pattern` contains `*`.
/// Not [`crate::wildcard::wildcard_match`] alone — that matcher treats a
/// `*`-free pattern as a *substring* search, which would make `--ep 'Mod
/// Weapon Attack Damage'` also hit unrelated entry points like `Mod Weapon
/// DMG Bonus Mult`-adjacent names sharing a prefix.
fn entry_point_name_matches(pattern: &str, name: &str) -> bool {
    if pattern.contains('*') {
        wildcard_match(pattern, name)
    } else {
        pattern.eq_ignore_ascii_case(name)
    }
}

/// `true` if `name` satisfies `pattern` per [`OmodPropertySpec`]'s matching
/// rule: exact case- and whitespace-insensitive unless `pattern` contains
/// `*`, in which case the same glob rule applies to whitespace-stripped forms.
/// Not [`crate::wildcard::wildcard_match`] alone — that matcher treats a
/// `*`-free pattern as a substring search, which is wrong here too.
fn omod_property_name_matches(pattern: &str, name: &str) -> bool {
    let pattern: String = pattern.chars().filter(|c| !c.is_whitespace()).collect();
    let name: String = name.chars().filter(|c| !c.is_whitespace()).collect();
    if pattern.contains('*') {
        wildcard_match(&pattern, &name)
    } else {
        pattern.eq_ignore_ascii_case(&name)
    }
}

/// Extract `(numeric id, name)` from a decoded enum value. Handles both the
/// resolved `{value, name}` shape and the bare-int fallback `format_int` in
/// `decode/mod.rs` emits when an id falls outside its enum's name table.
fn enum_id_name(v: &Value) -> Option<(u16, Option<&str>)> {
    match v {
        Value::Object(o) => {
            let id = o.get("value")?.as_u64()?;
            Some((
                u16::try_from(id).ok()?,
                o.get("name").and_then(Value::as_str),
            ))
        }
        Value::Number(n) => Some((u16::try_from(n.as_u64()?).ok()?, None)),
        _ => None,
    }
}

impl Database {
    /// Open an ESM file or data folder.
    ///
    /// When `path` is a **directory**, it is scanned for exactly one `.esm`
    /// file; zero or multiple ESMs produce a clear error.  When `path` is a
    /// **file**, it is used directly.
    ///
    /// After locating the ESM, sibling sources are loaded automatically when
    /// present (missing sources are silently skipped; load failures print a
    /// warning to stderr but do not abort):
    ///
    /// - **Strings**: loose `strings/<stem>_<locale>.{strings,…}` or
    ///   `<stem>_<locale>.strings` in the folder, else any
    ///   `*localization*.ba2` in the folder.
    /// - **Curves**: `misc/curvetables/json/` or `curvetables/json/` in the
    ///   folder, else any `*startup*.ba2` in the folder.
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let resolved = crate::discover::resolve_sources(path, "en")?;

        let esm = EsmFile::open(&resolved.esm)?;
        let index = Index::build(&esm)?;
        let schema = Schema::load_embedded().context("load embedded schema")?;

        let localization = match resolved.strings {
            Some(crate::discover::StringsSrc::Ba2(ref ba2_path)) => {
                match Localization::from_ba2(ba2_path, &resolved.locale, &resolved.loose_prefix) {
                    Ok(loc) => Some(loc),
                    Err(e) => {
                        log::warn!("failed to load localization from BA2: {}", e);
                        None
                    }
                }
            }
            Some(crate::discover::StringsSrc::Loose(ref dir)) => {
                match Localization::from_loose_files(dir, &resolved.locale, &resolved.loose_prefix)
                {
                    Ok(loc) => Some(loc),
                    Err(e) => {
                        log::warn!("failed to load localization from loose files: {}", e);
                        None
                    }
                }
            }
            None => None,
        };

        let curves = match resolved.curves {
            Some(crate::discover::CurvesSrc::LooseBase(ref base)) => {
                match crate::curves::CurveIndex::build_from_dir(&esm, &index, base) {
                    Ok(ci) => Some(ci),
                    Err(e) => {
                        log::warn!("failed to load curves from loose dir: {}", e);
                        None
                    }
                }
            }
            Some(crate::discover::CurvesSrc::Ba2(ref ba2_path)) => {
                match crate::curves::CurveIndex::build(&esm, &index, ba2_path) {
                    Ok(ci) => Some(ci),
                    Err(e) => {
                        log::warn!("failed to load curves from BA2: {}", e);
                        None
                    }
                }
            }
            None => None,
        };

        let is_localized = esm.file_info().map(|i| i.is_localized).unwrap_or(false);

        Ok(Database {
            esm,
            index,
            schema,
            is_localized,
            localization,
            curves,
            filter_cache: std::collections::HashMap::new(),
        })
    }

    /// Replace (or set) the localization tables used for LString resolution.
    pub fn set_localization(&mut self, loc: Localization) {
        self.localization = Some(loc);
    }

    /// Load and build the curve index from a Startup BA2 archive.
    ///
    /// Once loaded, any `formid` field with `"CURV"` in its `valid_refs` will
    /// have the curve's path and point data inlined in the decoded output.
    pub fn load_curves(&mut self, ba2_path: &Path) -> anyhow::Result<()> {
        let curves = crate::curves::CurveIndex::build(&self.esm, &self.index, ba2_path)?;
        self.curves = Some(curves);
        Ok(())
    }

    /// Load and build the curve index from a loose `misc/` directory.
    ///
    /// `misc_dir` is the extracted `misc/` folder from a Startup BA2
    /// (`misc_dir/curvetables/json/` must contain the JSON files).
    pub fn load_curves_from_dir(&mut self, misc_dir: &Path) -> anyhow::Result<()> {
        let curves = crate::curves::CurveIndex::build_from_dir(&self.esm, &self.index, misc_dir)?;
        self.curves = Some(curves);
        Ok(())
    }

    /// Parse the record at the given offset in the mmap'd ESM file.
    pub fn parse_record_at(&self, offset: u64) -> anyhow::Result<crate::reader::ParsedRecord> {
        self.esm.parse_record_at(offset)
    }

    /// Returns the localization string tables, if loaded.
    pub fn localization(&self) -> Option<&Localization> {
        self.localization.as_ref()
    }

    /// Returns whether any enrichment (localization or curves) is available.
    pub fn has_enrichment(&self) -> bool {
        self.localization.is_some() || self.curves.is_some()
    }

    pub fn file_info(&self) -> anyhow::Result<FileInfo> {
        let mut info = self.esm.file_info()?;
        info.path = self.esm.path.clone();
        Ok(info)
    }

    // ── Lazy index builders ─────────────────────────────────────────────
    //
    // `Index` keeps the data (the five `Section`s) and the pure reads over
    // it; `Database` owns building it, since building a section needs the
    // mmap'd ESM (plus the schema/localization/curves for `xref`'s full
    // decode) that only `Database` holds. The three functions actually
    // reachable from index.rs are `build_edid_section`/`build_search_section`/
    // `build_xref_section` — this crate-internal data/orchestration split
    // keeps each section's construction logic colocated with its type in
    // `index.rs`, while the shared acquire/recheck/write/publish protocol
    // lives once, in `build_lazy_section` below.

    /// The acquire/recheck/build/publish skeleton shared by all three lazy
    /// single-section builds (`ensure_edid_index`/`ensure_search_index`/
    /// `ensure_xref_index`) — each call site differs only in *what* it
    /// builds (a `build_*_section` closure over `&self.index`/`&self.esm`
    /// plus whatever else that section's build needs) and the tick-count
    /// `total` that closure's progress reporting is denominated in; the
    /// surrounding protocol is identical and lives here once.
    ///
    /// `T`'s archived type is the section itself: [`crate::rkyvcache::SectionSpec`]
    /// (ADR 0007) supplies both the on-disk file
    /// ([`crate::rkyvcache::section_path_for_spec`]) and the
    /// [`crate::progress::BuildStage`] this call's progress heartbeat
    /// reports under, so — unlike the pre-Stage-C shape this replaced, where
    /// `section_path_for`'s explicit [`crate::rkyvcache::SectionKind`]
    /// argument and the generic `Section<Archived<_>>` type parameter were
    /// two independently-suppliable values that had to be kept in sync by
    /// convention — there is only one place a caller can go wrong: passing
    /// the wrong `T`.
    ///
    /// Uses [`crate::progress::BuildLease::acquire_or_recheck`], which folds
    /// the "did another process finish this while I waited for the lock"
    /// recheck into the acquire call itself. That recheck's
    /// [`crate::rkyvcache::map_section_if_present`] call is a SECOND
    /// on-disk validation of the same section this method's caller already
    /// checked once via the cheap in-memory `is_mapped()` early return
    /// (`ensure_edid_index` etc., before calling in here) — not a redundant
    /// repeat of that check, but the one that closes the TOCTOU window
    /// between "found not yet mapped in this process's `Index`" and
    /// "actually acquired the advisory build lock": another process (or a
    /// concurrent call in this one) can finish the exact same build in that
    /// gap, and this recheck is what lets that caller return the
    /// just-finished section instead of racing a second build of the same
    /// data. There is no code path that obtains a live
    /// [`crate::progress::BuildLease`] without this recheck having already
    /// run and found the section still missing.
    fn build_lazy_section<T>(
        &self,
        total: u64,
        build: impl FnOnce(&mut crate::progress::BuildLease) -> anyhow::Result<T>,
    ) -> anyhow::Result<crate::rkyvcache::Section<rkyv::Archived<T>>>
    where
        T: rkyv::Archive,
        rkyv::Archived<T>: crate::rkyvcache::SectionSpec,
        T: for<'a> rkyv::Serialize<
                rkyv::api::high::HighSerializer<
                    rkyv::ser::writer::IoWriter<std::io::BufWriter<std::fs::File>>,
                    rkyv::ser::allocator::ArenaHandle<'a>,
                    rkyv::rancor::Error,
                >,
            >,
    {
        let stage = <rkyv::Archived<T> as crate::rkyvcache::SectionSpec>::KIND;
        let sig = crate::rkyvcache::CacheSig::read(&self.esm.path)?;
        let path = crate::rkyvcache::section_path_for_spec::<rkyv::Archived<T>>(&self.esm.path)?;

        let mut lease = match crate::progress::BuildLease::acquire_or_recheck(
            &self.esm.path,
            stage,
            1,
            1,
            total,
            || {
                crate::rkyvcache::map_section_if_present::<rkyv::Archived<T>>(
                    &path,
                    sig,
                    crate::index::CACHE_VERSION,
                )
            },
        )? {
            crate::progress::Acquired::AlreadyBuilt(section) => return Ok(section),
            crate::progress::Acquired::NeedsBuild(lease) => lease,
        };

        let data = build(&mut lease)?;
        lease.writing();
        crate::rkyvcache::write_and_remap(&path, sig, crate::index::CACHE_VERSION, data)
    }

    /// Build the lazy EditorID index on first call, writing it to its own
    /// `edid` section so a later call — in this process (the `is_mapped()`
    /// early-return below) or a fresh one (see [`Index::build`]'s doc
    /// comment) — reuses it rather than rebuilding. See
    /// [`Self::build_lazy_section`] for the shared acquire/recheck/publish
    /// protocol this and its two siblings below delegate to.
    pub fn ensure_edid_index(&mut self) -> anyhow::Result<()> {
        if self.index.edid.is_mapped() {
            return Ok(());
        }
        let total = self.index.len() as u64;
        self.index.edid = self.build_lazy_section(total, |lease| {
            crate::index::build_edid_section(&self.index, &self.esm, lease)
        })?;
        Ok(())
    }

    /// Build the lazy search index (EditorID + name/description) on first
    /// call, then cache it to its own `search` section. See
    /// [`Self::build_lazy_section`] for the acquire/recheck protocol this
    /// shares.
    pub fn ensure_search_index(&mut self) -> anyhow::Result<()> {
        if self.index.search.is_mapped() {
            return Ok(());
        }
        let total = self.index.len() as u64;
        self.index.search = self.build_lazy_section(total, |lease| {
            crate::index::build_search_section(&self.index, &self.esm, self.is_localized, lease)
        })?;
        Ok(())
    }

    /// Build the reverse-reference (`xref`) index on first call, then cache
    /// it to its own `xref` section. The most expensive of the three lazy
    /// builds (a full schema decode of every record). See
    /// [`Self::build_lazy_section`] for the acquire/recheck protocol this
    /// shares.
    pub fn ensure_xref_index(&mut self) -> anyhow::Result<()> {
        if self.index.xref.is_mapped() {
            return Ok(());
        }
        let total = self.esm.data().len() as u64;
        self.index.xref = self.build_lazy_section(total, |lease| {
            crate::index::build_xref_section(
                &self.index,
                &self.esm,
                &self.schema,
                self.is_localized,
                self.localization.as_ref(),
                self.curves.as_ref(),
                lease,
            )
        })?;
        Ok(())
    }

    // ── Ensure-then-get, collapsed to one call each ─────────────────────
    //
    // `Index::get_xref`/`iter_search`/`get_by_edid` silently return
    // empty/`None` when their section hasn't been built yet — a real
    // "no referencers"/"not found" answer is indistinguishable from "the
    // index isn't built yet", so a caller that forgets the matching
    // `ensure_*_index` call gets a wrong answer with no error. Rather than
    // documenting "call ensure first" as a convention (the pre-Stage-C
    // shape, enforced three different ways: an `assert!` after
    // `ensure_search_index`, nothing at all before `get_xref`, and separate
    // `.expect(...)` sites for the filter-cache trio below), those three
    // Index accessors are `pub(crate)` and reachable only through the
    // wrappers below, each of which ensures internally — there is no way to
    // read a lazy index's data from within this crate without going through
    // a call that guarantees it is built first.

    /// Referencers of `form_id`, building the `xref` index first if needed.
    /// Never silently answers "no referencers" for an index that just
    /// hasn't been built yet — see the module note above.
    fn xref_lookup(&mut self, form_id: FormId) -> anyhow::Result<Vec<FormId>> {
        self.ensure_xref_index()?;
        Ok(self.index.get_xref(form_id))
    }

    /// Resolve an EditorID to its FormID, building the `edid` index first if
    /// needed. `Ok(None)` means "index built, EditorID genuinely absent" —
    /// never conflated with "index not built yet" the way a bare
    /// `Index::get_by_edid` call without an `ensure_edid_index` first would
    /// be.
    fn resolve_edid_indexed(&mut self, edid: &str) -> anyhow::Result<Option<FormId>> {
        self.ensure_edid_index()?;
        Ok(self.index.get_by_edid(edid))
    }

    /// Resolve a FormID to its [`RecordMeta`] via the full `Index` HashMap.
    fn get_formid_meta(&self, form_id: FormId) -> anyhow::Result<RecordMeta> {
        self.index
            .get_by_formid(form_id)
            .with_context(|| format!("FormID {} not found", form_id))
    }

    pub fn record_by_formid(&mut self, form_id: FormId) -> anyhow::Result<RecordResult> {
        let meta = self.get_formid_meta(form_id)?;
        self.record_at_meta_with_depth(&meta, crate::decode::ResolveDepth::None)
    }

    pub fn record_by_edid(&mut self, edid: &str) -> anyhow::Result<RecordResult> {
        let form_id = self
            .resolve_edid_indexed(edid)?
            .with_context(|| format!("EditorID '{}' not found", edid))?;
        self.record_by_formid(form_id)
    }

    pub fn list_by_type(&self, sig: &str, limit: usize) -> anyhow::Result<Vec<ListEntry>> {
        if sig.len() != 4 {
            bail!("record type must be a 4-character signature");
        }
        let records = self.index.records_by_type(sig);
        let mut out = Vec::new();
        for (form_id, meta) in records.take(effective_take(limit)) {
            let rec = self.esm.parse_record_at(meta.offset)?;
            let editor_id = edid_from_subrecords(&rec.subrecords);
            let full_lstring_id =
                crate::reader::lstring_id_from_subrecords(&rec.subrecords, "FULL")
                    .map(|id| format!("0x{:08X}", id));
            out.push(ListEntry {
                form_id: form_id.display(),
                editor_id,
                full_lstring_id,
            });
        }
        Ok(out)
    }

    /// Search records by EditorID and/or display name using a wildcard pattern.
    ///
    /// `pattern` supports `*` as a multi-character wildcard. A plain string
    /// (no `*`) is treated as a case-insensitive substring match. An empty
    /// pattern or bare `"*"` matches everything.
    ///
    /// `types` restricts the search to the given 4-character record-type
    /// signatures (uppercase). An empty slice searches all record types.
    ///
    /// `field` controls which fields are compared: [`SearchField::Edid`],
    /// [`SearchField::Name`] (FULL + DESC), or [`SearchField::Both`].
    ///
    /// `limit` caps the number of results; pass `0` for no limit.
    ///
    /// Results are sorted by FormID for deterministic output.  When the result
    /// count equals a non-zero `limit`, the caller should indicate to the user
    /// that output was capped.
    ///
    /// Name search requires the localization BA2 to be loaded — if absent,
    /// only EditorID matching produces results.  For non-localized ESMs,
    /// names are inline strings and will not match via the lstring-ID path;
    /// EditorID search still works for those files.
    pub fn search(
        &mut self,
        pattern: &str,
        types: &[String],
        field: SearchField,
        limit: usize,
    ) -> anyhow::Result<Vec<RecordRow>> {
        // No `assert!`-after-ensure needed here (unlike the pre-Stage-C
        // shape this replaced): `ensure_search_index`'s only two return
        // paths either propagate an `Err` or leave `search` mapped — see
        // its doc comment.
        self.ensure_search_index()?;

        let type_filter: Option<HashSet<&str>> = if types.is_empty() {
            None
        } else {
            Some(types.iter().map(|s| s.as_str()).collect())
        };

        // Collect matching entries. HashMap order is nondeterministic, so we
        // accumulate into a Vec and sort by FormID before returning.
        let mut matches: Vec<(u32, RecordRow)> = Vec::new();

        for (form_id, sref) in self.index.iter_search() {
            // Type filter — alloc-free: only the record's Signature is
            // needed here, not the whole RecordMeta.
            if let Some(ref filter) = type_filter {
                let sig = self.index.signature_of(form_id);
                let sig_str = sig.as_ref().map(|s| s.as_str()).unwrap_or("");
                if !filter.contains(sig_str) {
                    continue;
                }
            }

            // Resolve display name/description as borrowed &str first — no
            // allocation yet. SearchField::Edid never reads either, so skip
            // the localization lookup entirely in that case.
            let name_str: Option<&str> = if field == SearchField::Edid {
                None
            } else {
                sref.full_id
                    .and_then(|id| {
                        self.localization
                            .as_ref()
                            .and_then(|l| l.lookup(StringKind::Strings, id))
                    })
                    .or(sref.full_text)
            };
            let desc_str: Option<&str> = if field == SearchField::Edid {
                None
            } else {
                sref.desc_id
                    .and_then(|id| {
                        self.localization
                            .as_ref()
                            .and_then(|l| l.lookup(StringKind::Strings, id))
                    })
                    .or(sref.desc_text)
            };

            // Check if this record matches the pattern for the requested
            // field — tested entirely against borrowed &str data, no
            // allocation regardless of outcome.
            let matched = match field {
                SearchField::Edid => sref
                    .editor_id
                    .map(|e| wildcard_match(pattern, e))
                    .unwrap_or(false),
                SearchField::Name => {
                    name_str
                        .map(|n| wildcard_match(pattern, n))
                        .unwrap_or(false)
                        || desc_str
                            .map(|d| wildcard_match(pattern, d))
                            .unwrap_or(false)
                }
                SearchField::Both => {
                    sref.editor_id
                        .map(|e| wildcard_match(pattern, e))
                        .unwrap_or(false)
                        || name_str
                            .map(|n| wildcard_match(pattern, n))
                            .unwrap_or(false)
                        || desc_str
                            .map(|d| wildcard_match(pattern, d))
                            .unwrap_or(false)
                }
            };

            if !matched {
                continue;
            }

            // Only now build the owned `name` — the one of the two that
            // actually crosses into the outgoing RecordRow (RecordRow has no
            // description field; desc_str above only ever needed to be
            // borrowed for the match test).
            let name: Option<String> = name_str.map(|s| s.to_owned());

            let meta = self.index.get_by_formid(form_id);
            let offset = meta.map(|m| m.offset).unwrap_or(0);
            let record_type = meta.map(|m| m.signature.as_str().to_owned());

            matches.push((
                form_id.raw(),
                RecordRow {
                    form_id: form_id.display(),
                    record_type,
                    editor_id: sref.editor_id.map(|s| s.to_owned()),
                    name,
                    offset,
                },
            ));
        }

        matches.sort_by_key(|(raw, _)| *raw);

        let mut out: Vec<RecordRow> = matches.into_iter().map(|(_, row)| row).collect();
        out.truncate(effective_take(limit));
        Ok(out)
    }

    /// List records of the given 4-character type signature with pagination.
    ///
    /// Returns FormID, EditorID, and resolved translated name (from the
    /// localization BA2 when available) for each record.
    pub fn list_type_records(
        &mut self,
        sig: &str,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<RecordRow>> {
        if sig.len() != 4 {
            bail!("record type must be a 4-character signature");
        }
        let records: Vec<(FormId, u64, String)> = self
            .index
            .records_by_type(sig)
            .skip(offset)
            .take(effective_take(limit))
            .map(|(fid, meta)| (fid, meta.offset, meta.signature.as_str().to_owned()))
            .collect();
        let mut out = Vec::new();
        for (form_id, rec_offset, record_type) in records {
            let rec = self.esm.parse_record_at(rec_offset)?;
            let editor_id = edid_from_subrecords(&rec.subrecords);
            let name =
                crate::reader::lstring_id_from_subrecords(&rec.subrecords, "FULL").and_then(|id| {
                    self.localization
                        .as_ref()
                        .and_then(|l| l.lookup(crate::strings::StringKind::Strings, id))
                        .map(|s| s.to_owned())
                });
            out.push(RecordRow {
                form_id: form_id.display(),
                record_type: Some(record_type),
                editor_id,
                name,
                offset: rec_offset,
            });
        }
        Ok(out)
    }

    /// Return the list of records that reference `form_id`, with FormID,
    /// EditorID, and resolved name for each.
    ///
    /// The reverse-reference index is built lazily on the first call and
    /// persisted to its own `xref` rkyv section so subsequent calls —
    /// in this process or a fresh one — are instant.
    pub fn referenced_by(&mut self, form_id: FormId) -> anyhow::Result<Vec<RecordRow>> {
        let referencers = self.xref_lookup(form_id)?;
        let mut out = Vec::new();
        for referencer in referencers {
            if let Some(row) = self.record_row_for(referencer)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Build a [`RecordRow`] (resolved type/EditorID/name) for an arbitrary
    /// FormID already present in the index — `None` if it isn't. Shared by
    /// [`Database::referenced_by`] (each referencer row) and
    /// [`refs::referenced_by_enriched`]'s carrier/seed rows.
    fn record_row_for(&mut self, form_id: FormId) -> anyhow::Result<Option<RecordRow>> {
        let Some(meta) = self.index.get_by_formid(form_id) else {
            return Ok(None);
        };
        let rec = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&rec.subrecords);
        let name =
            crate::reader::lstring_id_from_subrecords(&rec.subrecords, "FULL").and_then(|id| {
                self.localization
                    .as_ref()
                    .and_then(|l| l.lookup(crate::strings::StringKind::Strings, id))
                    .map(|s| s.to_owned())
            });
        Ok(Some(RecordRow {
            form_id: form_id.display(),
            record_type: Some(meta.signature.as_str().to_owned()),
            editor_id,
            name,
            offset: meta.offset,
        }))
    }

    pub fn record_raw(&self, form_id: FormId) -> anyhow::Result<ParsedRecord> {
        let meta = self.get_formid_meta(form_id)?;
        self.esm.parse_record_at(meta.offset)
    }

    /// List all top-level (group_type == 0) GRUPs in file order.
    pub fn list_groups(&self) -> Vec<GroupNode> {
        let tree = self.index.tree();
        tree.roots().map(|idx| tree.group_node(idx)).collect()
    }

    /// List direct children of the top-level GRUP with the given record type signature.
    ///
    /// Returns an empty vec if the group doesn't exist. Applies `offset`/`limit`
    /// for pagination over children.
    pub fn list_type_children(
        &self,
        sig: &str,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<GroupChild>> {
        let sig_upper = sig.to_uppercase();

        // Find the top-level group with this record-type signature
        let group_idx = self.index.tree().find_root_by_type(&sig_upper);

        let Some(group_idx) = group_idx else {
            return Ok(Vec::new());
        };

        Ok(self.group_children_at(group_idx, offset, limit))
    }

    /// List direct children of an arbitrary GRUP by its own header offset (for recursive
    /// descent below the top level — e.g. into a worldspace's exterior blocks, then into
    /// a block's cells). Returns an empty vec if no GRUP starts at that offset.
    pub fn list_group_children(
        &self,
        group_offset: u64,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<GroupChild>> {
        let Some(group_idx) = self.index.tree().group_idx_at_offset(group_offset) else {
            return Ok(Vec::new());
        };
        Ok(self.group_children_at(group_idx, offset, limit))
    }

    /// Paginate and materialize the children of the GRUP at arena index `group_idx`.
    ///
    /// Infallible: pagination clamps to the child count, and `record_stub_at`
    /// failures already collapse to `None` editor_ids rather than propagating.
    fn group_children_at(&self, group_idx: usize, offset: usize, limit: usize) -> Vec<GroupChild> {
        let tree = self.index.tree();
        // Collect the paginated child slice (avoid holding borrow into mutable self below)
        let children_slice: Vec<ChildRef> = tree.children(group_idx, offset, limit);

        let mut result = Vec::new();
        for child in children_slice {
            match child {
                ChildRef::Group(idx) => {
                    // `ChildRef::Group` stores its arena index as `u32` (Stage
                    // 4 pinned every `TreeIndex`-adjacent stored index to
                    // `u32` for portable rkyv layout — see `tree.rs`), while
                    // `TreeView::group_node` keeps `usize` at the in-memory
                    // API boundary. Lossless widening cast, not a narrowing
                    // one.
                    result.push(GroupChild::Group(tree.group_node(idx as usize)));
                }
                ChildRef::Record {
                    form_id,
                    offset: rec_offset,
                    sig: rec_sig,
                } => {
                    // Try cheap stub read to get EDID from the first subrecord
                    let editor_id = self
                        .record_stub_at(rec_offset)
                        .ok()
                        .and_then(|s| s.editor_id);
                    let record_type = String::from_utf8_lossy(&rec_sig)
                        .trim_end_matches('\0')
                        .to_string();
                    result.push(GroupChild::Record(crate::tree::RecordStub {
                        form_id: FormId(form_id).display(),
                        editor_id,
                        record_type,
                        offset: rec_offset,
                    }));
                }
            }
        }
        result
    }

    /// Cheap header-only read at a file offset — no field decode.
    ///
    /// Attempts to read the EDID from the first subrecord when the record is not
    /// compressed. Falls back to `None` editor_id without panicking.
    pub fn record_stub_at(&self, offset: u64) -> anyhow::Result<crate::tree::RecordStub> {
        let data = self.esm.data();
        if offset as usize + crate::format::HEADER_SIZE as usize > data.len() {
            anyhow::bail!("record offset {} out of range", offset);
        }
        let hdr = crate::format::RecordHeader::parse(&data[offset as usize..])?;

        // Attempt to read EDID (first subrecord) for non-compressed records
        let editor_id = if hdr.flags & crate::format::COMPRESSED_FLAG == 0 {
            let sub_start = offset as usize + crate::format::HEADER_SIZE as usize;
            if sub_start + crate::format::SUBRECORD_HEADER_SIZE <= data.len() {
                let sub_hdr = crate::format::SubrecordHeader::parse(&data[sub_start..])?;
                if sub_hdr.signature.as_str() == "EDID" {
                    let data_start = sub_start + crate::format::SUBRECORD_HEADER_SIZE;
                    let data_end = data_start
                        .saturating_add(sub_hdr.size as usize)
                        .min(data.len());
                    let raw = &data[data_start..data_end];
                    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    String::from_utf8(raw[..end].to_vec()).ok()
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(crate::tree::RecordStub {
            form_id: FormId(hdr.form_id).display(),
            editor_id,
            record_type: hdr.signature.to_string(),
            offset,
        })
    }

    /// Decode a record at `meta`'s offset with the given resolution depth.
    /// `ResolveDepth::None` decodes with no FormID-reference resolver — the one
    /// codepath used by every unresolved-decode call site (coverage scans,
    /// unchanged-side diff decodes, plain `record_by_formid`/`record_by_edid`).
    pub(crate) fn record_at_meta_with_depth(
        &self,
        meta: &crate::reader::RecordMeta,
        depth: crate::decode::ResolveDepth,
    ) -> anyhow::Result<RecordResult> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&parsed.subrecords);
        let resolver: Option<DatabaseResolver<'_>> = if depth != crate::decode::ResolveDepth::None {
            Some(DatabaseResolver::new(self, 2))
        } else {
            None
        };
        let ctx = DecodeContext::for_record(
            &self.schema,
            parsed.header.form_version,
            self.is_localized,
            self.localization.as_ref(),
            self.curves.as_ref(),
            depth,
            resolver
                .as_ref()
                .map(|r| r as &dyn crate::decode::FormIdRefResolver),
        );
        let mut fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
        // CURV records only carry a path to an external curve-points JSON file
        // (see schema `JSON File Path[/2]`) — inline the parsed points too, so a
        // plain `get` on a CURV record doesn't require a second out-of-band read
        // of that file. Referencing records already get this via `resolve_formid`
        // (decode.rs); this covers the CURV record itself.
        if parsed.header.signature == "CURV"
            && let Some(curve) = self
                .curves
                .as_ref()
                .and_then(|curves| curves.get(parsed.header.form_id))
            && let Value::Object(map) = &mut fields
        {
            map.insert(
                "Curve".to_string(),
                crate::decode::curve_points_value(curve),
            );
        }
        Ok(RecordResult {
            header: parsed.header,
            editor_id,
            fields,
        })
    }

    /// Decode a record by FormID with the given resolution depth.
    pub fn record_by_formid_resolved(
        &self,
        form_id: FormId,
        depth: crate::decode::ResolveDepth,
    ) -> anyhow::Result<RecordResult> {
        let meta = self.get_formid_meta(form_id)?;
        self.record_at_meta_with_depth(&meta, depth)
    }

    /// Decode a record by EditorID with the given resolution depth.
    ///
    /// Only resolves against real ESM records — unlike `ipc::resolve_sel`
    /// (the CLI/daemon/N-API serving path), this does not fall back to
    /// `crate::hardcoded`'s engine-hardcoded EditorID table. Prefer
    /// `ipc::resolve_sel` + [`Self::record_by_formid_resolved`] for that
    /// broader precedence-aware resolution; this method stays as a narrower
    /// public building block rather than duplicating that fallback here.
    pub fn record_by_edid_resolved(
        &mut self,
        edid: &str,
        depth: crate::decode::ResolveDepth,
    ) -> anyhow::Result<RecordResult> {
        let form_id = self
            .resolve_edid_indexed(edid)?
            .with_context(|| format!("EditorID '{}' not found", edid))?;
        self.record_by_formid_resolved(form_id, depth)
    }

    /// Decode `referencer` (no FormID resolver — `ResolveDepth::None`, plain
    /// hex output) and return every JSON path within its body where `target`
    /// appears as a raw FormID string. Backs `refs --paths`: best-effort —
    /// returns an empty vec if `referencer` can't be located or decoded, or if
    /// `target` never appears as a literal field value (e.g. it's only
    /// reachable indirectly, such as through curve-table inlining).
    pub fn formid_reference_paths(&self, referencer: FormId, target: FormId) -> Vec<String> {
        let Some(meta) = self.index.get_by_formid(referencer) else {
            return Vec::new();
        };
        let Ok(result) = self.record_at_meta_with_depth(&meta, crate::decode::ResolveDepth::None)
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        collect_formid_paths(&result.fields, &target.display(), String::new(), &mut out);
        out
    }

    /// Decode `node` (no FormID resolver — plain hex output) and return every
    /// distinct FormID its body references, deduplicated and excluding
    /// `node` itself. Backs [`refs::find_ref_path`]'s forward-search
    /// direction: unlike [`Database::referenced_by`] (which asks the reverse
    /// index "who points at `node`"), this walks `node`'s *own* outgoing
    /// references — cheap and bounded by `node`'s own field count, useful
    /// when searching from an endpoint with unknown-but-possibly-huge
    /// incoming fan-out.
    pub fn outgoing_formids(&self, node: FormId) -> Vec<FormId> {
        let Some(meta) = self.index.get_by_formid(node) else {
            return Vec::new();
        };
        let Ok(result) = self.record_at_meta_with_depth(&meta, crate::decode::ResolveDepth::None)
        else {
            return Vec::new();
        };
        let mut raw = Vec::new();
        collect_all_formid_values(&result.fields, &mut raw);
        let mut seen = std::collections::HashSet::new();
        raw.into_iter()
            .filter_map(|s| crate::parse_form_id_input(&s).ok())
            .filter(|&f| f != node && seen.insert(f))
            .collect()
    }

    /// Populate `self.filter_cache` for `sig` (already uppercased) on first
    /// access, decoding at most [`FILTER_SCAN_CAP`] records. No-op if already
    /// cached. Used by [`Database::filter_type_records`] and
    /// [`Database::list_type_field_paths`].
    fn ensure_filter_cache(&mut self, sig: &str) -> anyhow::Result<()> {
        if self.filter_cache.contains_key(sig) {
            return Ok(());
        }

        // `count_by_type` looks up the type_index directly, avoiding a full
        // materialization of every (FormId, RecordMeta) pair just to measure
        // `total` — `records_by_type` itself is still walked below, but only
        // up to `FILTER_SCAN_CAP`.
        let total = self.index.count_by_type(sig);
        let records: Vec<(FormId, u64)> = self
            .index
            .records_by_type(sig)
            .take(FILTER_SCAN_CAP)
            .map(|(fid, meta)| (fid, meta.offset))
            .collect();

        let mut entries = Vec::with_capacity(records.len());
        for (form_id, offset) in records {
            let parsed = self.esm.parse_record_at(offset)?;
            let editor_id = edid_from_subrecords(&parsed.subrecords);
            let ctx = DecodeContext::for_record(
                &self.schema,
                parsed.header.form_version,
                self.is_localized,
                self.localization.as_ref(),
                self.curves.as_ref(),
                crate::decode::ResolveDepth::None,
                None,
            );
            let fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
            entries.push(FilterCacheEntry {
                form_id,
                editor_id,
                offset,
                fields,
            });
        }

        self.filter_cache.insert(sig.to_string(), (total, entries));
        Ok(())
    }

    /// [`Self::ensure_filter_cache`] for `sig`, then borrow the entry it
    /// guarantees is now present — the same ensure-then-get shape
    /// [`Self::xref_lookup`]/[`Self::resolve_edid_indexed`] use for the
    /// lazy `Index` sections, applied here to the four call sites that used
    /// to each repeat `self.ensure_filter_cache(&sig)?` followed by a
    /// `self.filter_cache.get(&sig).expect("populated by
    /// ensure_filter_cache")` — a panic (not a recoverable error) if that
    /// invariant were ever violated. Folding both steps into one call still
    /// can't silently skip the ensure, and turns what would be a process
    /// crash into a normal propagated `Err` via `.context()`, matching this
    /// crate's `anyhow::Result` convention instead of being the one
    /// `.expect()`-shaped exception to it.
    fn filter_cache_entries(
        &mut self,
        sig: &str,
    ) -> anyhow::Result<&(usize, Vec<FilterCacheEntry>)> {
        self.ensure_filter_cache(sig)?;
        self.filter_cache.get(sig).with_context(|| {
            format!("filter cache entry for '{sig}' missing immediately after ensure_filter_cache — this should never happen")
        })
    }

    /// Filter records of type `sig` by a predicate against their decoded
    /// `fields` JSON body. See [`FilterOp`] and [`predicate_matches`] for the
    /// path syntax and operator semantics.
    ///
    /// `path` of `None`/empty deep-scans every value in the record. `limit`
    /// of `0` means no limit. Decoding itself is capped at [`FILTER_SCAN_CAP`]
    /// records per type — see [`FilterResult::scan_capped`].
    pub fn filter_type_records(
        &mut self,
        sig: &str,
        path: Option<&str>,
        op: FilterOp,
        value: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<FilterResult> {
        let sig = sig.to_uppercase();
        let (total, entries) = self.filter_cache_entries(&sig)?;
        let total = *total;
        let scanned = entries.len();

        let mut matches: Vec<&FilterCacheEntry> = entries
            .iter()
            .filter(|e| predicate_matches(&e.fields, path, op, value))
            .collect();
        matches.sort_by_key(|e| e.form_id.raw());

        let matched = matches.len();
        let capped = limit > 0 && matched > limit;
        let rows: Vec<RecordRow> = matches
            .into_iter()
            .take(effective_take(limit))
            .map(|e| RecordRow {
                form_id: e.form_id.display(),
                record_type: Some(sig.clone()),
                editor_id: e.editor_id.clone(),
                name: None,
                offset: e.offset,
            })
            .collect();

        Ok(FilterResult {
            rows,
            matched,
            scanned,
            total,
            capped,
            scan_capped: scanned < total,
        })
    }

    /// Union of all dot-notation field paths observed across the (possibly
    /// capped) decoded sample of a type's records — for filter-panel
    /// autocomplete. Array levels collapse to a literal `"[]"` segment
    /// regardless of index (all elements of an array share the same
    /// predicate-path shape). Sorted, deduped, capped defensively at a few
    /// thousand entries against pathological records.
    pub fn list_type_field_paths(&mut self, sig: &str) -> anyhow::Result<Vec<String>> {
        const MAX_PATHS: usize = 5000;
        let sig = sig.to_uppercase();
        let (_, entries) = self.filter_cache_entries(&sig)?;

        let mut paths: HashSet<String> = HashSet::new();
        for entry in entries {
            if paths.len() >= MAX_PATHS {
                break;
            }
            collect_field_paths(&entry.fields, "", &mut paths, MAX_PATHS);
        }
        let mut out: Vec<String> = paths.into_iter().collect();
        out.sort();
        out.truncate(MAX_PATHS);
        Ok(out)
    }

    /// Carrier PERKs for an entry point (see [`EntryPointSpec`]) — the
    /// reverse of `get`/`walk`'s "what entry point does this perk carry?".
    /// Deduped: a perk with several effects on the same entry point, or
    /// matching several effects under a glob, appears once.
    ///
    /// Seeds are sorted by `(primary entry-point id, form_id)`, where
    /// "primary" is the smallest matched id on that carrier. That order
    /// drives both the per-EP carrier grouping in table output and the
    /// BFS's first-reach attribution priority in
    /// `refs::referenced_by_walk` (earlier seeds win equal-depth ties for
    /// `path`/`VIA`; equal-depth `tags` are unioned).
    ///
    /// Reuses the `ensure_filter_cache("PERK")` memoized decode (shared with
    /// [`Database::filter_type_records`]), so repeat lookups after the first
    /// are effectively free.
    ///
    /// Returns `(label, seeds)`: `label` is a human-readable description of
    /// what matched — e.g. `"entry point 39 (Mod Percent Blocked)"` or
    /// `"entry point 'Mod VATS*' (14 matched: 43 Mod VATS Attack Damage, …)"`
    /// — meant for [`ipc::RefList::target`]; `seeds` are `(FormId, tags)`
    /// pairs tagging each carrier with the entry points it matched.
    pub fn perks_by_entry_point(
        &mut self,
        spec: &EntryPointSpec,
    ) -> anyhow::Result<(String, Carriers)> {
        let (_, entries) = self.filter_cache_entries("PERK")?;

        let mut seeds: Carriers = Vec::new();
        let mut matched: std::collections::BTreeSet<(u16, Option<String>)> = Default::default();
        for entry in entries {
            let Some(effects) = entry.fields.get("Effects").and_then(Value::as_array) else {
                continue;
            };
            let mut tags: Vec<CarrierTag> = Vec::new();
            for effect in effects {
                let Some(ep) = effect.pointer("/Effect/Entry Point/Entry Point") else {
                    continue;
                };
                let Some((id, name)) = enum_id_name(ep) else {
                    continue;
                };
                let is_match = match spec {
                    EntryPointSpec::Id(want) => id == *want,
                    EntryPointSpec::Name(pat) => {
                        name.is_some_and(|n| entry_point_name_matches(pat, n))
                    }
                };
                if is_match {
                    let name = name.map(str::to_string);
                    matched.insert((id, name.clone()));
                    tags.push(CarrierTag {
                        kind: CarrierKind::EntryPoint,
                        id,
                        name,
                        scope: None,
                    });
                }
            }
            if !tags.is_empty() {
                tags.sort();
                tags.dedup_by(|a, b| a.id == b.id);
                seeds.push((entry.form_id, tags));
            }
        }
        seeds.sort_by_key(|(f, tags)| (tags.first().map(|t| t.id).unwrap_or(0), f.raw()));

        let label = match matched.len() {
            0 => format!("entry point {} (no match)", spec.display()),
            1 => {
                let (id, name) = matched.iter().next().expect("len == 1");
                match name {
                    Some(n) => format!("entry point {id} ({n})"),
                    None => format!("entry point {id} (unnamed)"),
                }
            }
            n => {
                let legend: Vec<String> = matched
                    .iter()
                    .map(|(id, name)| match name {
                        Some(n) => format!("{id} {n}"),
                        None => id.to_string(),
                    })
                    .collect();
                format!(
                    "entry point {} ({n} matched: {})",
                    spec.display(),
                    legend.join(", ")
                )
            }
        };

        Ok((label, seeds))
    }

    /// Carrier OMODs for a property (see [`OmodPropertySpec`]) — the reverse
    /// of `get`/`walk`'s "what properties does this OMOD declare?". Deduped:
    /// an OMOD with the same matched property row more than once appears once
    /// with one copy of that tag.
    ///
    /// Seeds are sorted by `(primary property id, form_id)`, where "primary"
    /// is the smallest matched id on that carrier. That order drives both the
    /// per-property carrier grouping in table output and the BFS's first-reach
    /// attribution priority in `refs::referenced_by_walk` (earlier seeds win
    /// equal-depth ties for `path`/`VIA`; equal-depth `tags` are unioned).
    ///
    /// Reuses the `ensure_filter_cache("OMOD")` memoized decode (shared with
    /// [`Database::filter_type_records`]), so repeat lookups after the first
    /// are effectively free.
    ///
    /// Returns `(label, seeds)`: `label` is a human-readable description of
    /// what matched — e.g. `"OMOD property weap:0 (Speed)"` or
    /// `"OMOD property 'Enchantments' (3 matched: weap:65 Enchantments, \
    /// armo:0 Enchantments, npc:3 Enchantments)"` — meant for
    /// [`ipc::RefList::target`]; `seeds` are `(FormId, tags)` pairs tagging
    /// each carrier with the properties it matched.
    pub fn omods_by_property(
        &mut self,
        spec: &OmodPropertySpec,
    ) -> anyhow::Result<(String, Carriers)> {
        let (_, entries) = self.filter_cache_entries("OMOD")?;

        let mut seeds: Carriers = Vec::new();
        let mut matched: std::collections::BTreeSet<(PropScope, u16, Option<String>)> =
            Default::default();
        for entry in entries {
            let Some(form_type_name) = entry
                .fields
                .pointer("/Data/Form Type/name")
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(scope) = PropScope::from_form_type_name(form_type_name) else {
                continue;
            };
            if spec.scope.is_some_and(|want| want != scope) {
                continue;
            }
            let Some(properties) = entry
                .fields
                .pointer("/Data/Properties")
                .and_then(Value::as_array)
            else {
                continue;
            };

            let mut tags: Vec<CarrierTag> = Vec::new();
            for property in properties {
                let Some(value) = property.get("Property") else {
                    continue;
                };
                let Some((id, name)) = enum_id_name(value) else {
                    continue;
                };
                let is_match = match &spec.sel {
                    OmodPropertySel::Id(want) => id == *want,
                    OmodPropertySel::Name(pattern) => {
                        name.is_some_and(|name| omod_property_name_matches(pattern, name))
                    }
                };
                if is_match {
                    let name = name.map(str::to_string);
                    matched.insert((scope, id, name.clone()));
                    tags.push(CarrierTag {
                        kind: CarrierKind::OmodProperty,
                        id,
                        name,
                        scope: Some(scope.tag_str().to_string()),
                    });
                }
            }
            if !tags.is_empty() {
                tags.sort();
                tags.dedup();
                seeds.push((entry.form_id, tags));
            }
        }
        seeds.sort_by_key(|(form_id, tags)| {
            (tags.first().map(|tag| tag.id).unwrap_or(0), form_id.raw())
        });

        let label = match matched.len() {
            0 => format!("OMOD property {} (no match)", spec.display()),
            1 => {
                let (scope, id, name) = matched.iter().next().expect("len == 1");
                match name {
                    Some(name) => {
                        format!("OMOD property {}:{id} ({name})", scope.tag_str())
                    }
                    None => format!("OMOD property {}:{id} (unnamed)", scope.tag_str()),
                }
            }
            n => {
                let legend: Vec<String> = matched
                    .iter()
                    .map(|(scope, id, name)| match name {
                        Some(name) => format!("{}:{id} {name}", scope.tag_str()),
                        None => format!("{}:{id}", scope.tag_str()),
                    })
                    .collect();
                format!(
                    "OMOD property {} ({n} matched: {})",
                    spec.display(),
                    legend.join(", ")
                )
            }
        };

        Ok((label, seeds))
    }
}

/// Adapter that wraps a [`Database`] and implements [`FormIdRefResolver`].
///
/// Uses only `&self` methods on `Database` — read-only record access via `esm`.
pub struct DatabaseResolver<'a> {
    db: &'a Database,
    /// Remaining recursion depth for `Full` resolution.
    remaining: u8,
}

impl<'a> DatabaseResolver<'a> {
    pub fn new(db: &'a Database, remaining: u8) -> Self {
        Self { db, remaining }
    }
}

impl<'a> crate::decode::FormIdRefResolver for DatabaseResolver<'a> {
    fn stub(&self, id: FormId) -> Option<crate::decode::FormIdStub> {
        let Ok(meta) = self.db.get_formid_meta(id) else {
            // Index miss — this may be a hardcoded engine form (e.g. AVIF `Kill
            // Streak` at 0x399) that has no record in the ESM at all. Real
            // records always win; this fallback only fires when the index
            // lookup itself fails.
            let form = crate::hardcoded::lookup(id)?;
            return Some(crate::decode::FormIdStub {
                formid: id.display(),
                editor_id: form.editor_id.clone(),
                record_type: form.record_type.clone(),
            });
        };
        let parsed = self.db.esm.parse_record_at(meta.offset).ok()?;
        let editor_id = crate::reader::edid_from_subrecords(&parsed.subrecords);
        let record_type = parsed.header.signature.clone();
        Some(crate::decode::FormIdStub {
            formid: id.display(),
            editor_id,
            record_type,
        })
    }

    fn decode_full(&self, id: FormId) -> Option<Value> {
        if self.remaining == 0 {
            // At depth limit — fall back to stub
            return self.stub(id).and_then(|s| serde_json::to_value(&s).ok());
        }
        let Ok(meta) = self.db.get_formid_meta(id) else {
            // Index miss — fall back to the hardcoded-form table, same as `stub`.
            // There's no further record to recurse into, so this returns the
            // same stub-shaped JSON `stub()` would (matching the existing
            // depth-limit fallback above).
            return self.stub(id).and_then(|s| serde_json::to_value(&s).ok());
        };
        let parsed = self.db.esm.parse_record_at(meta.offset).ok()?;
        let editor_id = crate::reader::edid_from_subrecords(&parsed.subrecords);
        let record_type = parsed.header.signature.clone();
        // Build a nested DecodeContext with depth decremented
        let nested_resolver = DatabaseResolver {
            db: self.db,
            remaining: self.remaining - 1,
        };
        let ctx = DecodeContext::for_record(
            &self.db.schema,
            parsed.header.form_version,
            self.db.is_localized,
            self.db.localization.as_ref(),
            self.db.curves.as_ref(),
            crate::decode::ResolveDepth::Full,
            Some(&nested_resolver),
        );
        let fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
        Some(serde_json::json!({
            "formid": id.display(),
            "editor_id": editor_id,
            "record_type": record_type,
            "fields": fields,
        }))
    }
}

pub fn parse_form_id_input(s: &str) -> anyhow::Result<FormId> {
    parse_formid(s)
}

/// Heuristic: returns `true` if `s` looks like a FormID literal (a `0x`-prefixed
/// hex value, or a bare run of only hex digits up to 8 chars — which also covers
/// pure-decimal input like `18000`) rather than an EditorID.
///
/// Used to auto-route ambiguous CLI/server input to the right lookup. Anything
/// with non-hex characters, or longer than 8 hex digits, is treated as an
/// EditorID. Short all-hex EditorIDs (e.g. `cafe`) are read as FormIDs; an
/// explicit `--edid` flag disambiguates those cases.
pub fn looks_like_formid(s: &str) -> bool {
    let s = s.trim();
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    !body.is_empty() && body.len() <= 8 && body.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod filter_predicate_tests {
    use super::{FilterOp, predicate_matches};
    use serde_json::json;

    #[test]
    fn simple_top_level_eq() {
        let fields = json!({ "EditorID": "TestWeapon" });
        assert!(predicate_matches(
            &fields,
            Some("EditorID"),
            FilterOp::Eq,
            Some("testweapon")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("EditorID"),
            FilterOp::Eq,
            Some("other")
        ));
    }

    #[test]
    fn simple_top_level_contains() {
        let fields = json!({ "Name": "Combat Rifle" });
        assert!(predicate_matches(
            &fields,
            Some("Name"),
            FilterOp::Contains,
            Some("rifle")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Name"),
            FilterOp::Contains,
            Some("shotgun")
        ));
    }

    #[test]
    fn simple_top_level_gt() {
        let fields = json!({ "Value": 50 });
        assert!(predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Gt,
            Some("10")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Gt,
            Some("100")
        ));
    }

    #[test]
    fn nested_dot_path_navigation() {
        let fields = json!({ "Data": { "Damage": 25, "Weight": 5.5 } });
        assert!(predicate_matches(
            &fields,
            Some("Data.Damage"),
            FilterOp::Eq,
            Some("25")
        ));
        assert!(predicate_matches(
            &fields,
            Some("Data.Weight"),
            FilterOp::Lt,
            Some("10")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Data.Missing"),
            FilterOp::Exists,
            None
        ));
    }

    #[test]
    fn array_wildcard_matches_any_element() {
        let fields = json!({
            "Components": [
                { "Component": "Steel", "Count": 2 },
                { "Component": "Wood", "Count": 1 },
            ]
        });
        assert!(predicate_matches(
            &fields,
            Some("Components.[].Component"),
            FilterOp::Eq,
            Some("Wood")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Components.[].Component"),
            FilterOp::Eq,
            Some("Aluminum")
        ));
    }

    #[test]
    fn empty_path_deep_scan() {
        let fields = json!({
            "Data": { "Damage": 25 },
            "Keywords": ["WeapTypeRifle", "Craftable"],
        });
        // Deep-scan finds a nested value anywhere in the tree.
        assert!(predicate_matches(&fields, None, FilterOp::Eq, Some("25")));
        assert!(predicate_matches(
            &fields,
            Some(""),
            FilterOp::Contains,
            Some("rifle")
        ));
        assert!(!predicate_matches(
            &fields,
            None,
            FilterOp::Eq,
            Some("nope")
        ));
    }

    #[test]
    fn exists_on_present_but_null_field() {
        let fields = json!({ "Optional": null });
        assert!(predicate_matches(
            &fields,
            Some("Optional"),
            FilterOp::Exists,
            None
        ));
    }

    #[test]
    fn exists_on_genuinely_missing_field() {
        let fields = json!({ "Other": 1 });
        assert!(!predicate_matches(
            &fields,
            Some("Missing"),
            FilterOp::Exists,
            None
        ));
    }

    #[test]
    fn numeric_eq_matches_string_value_against_json_number() {
        let fields = json!({ "Value": 50.0 });
        assert!(predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Eq,
            Some("50")
        ));
    }

    #[test]
    fn contains_matches_substring_of_stringified_number() {
        let fields = json!({ "Code": 1234 });
        assert!(predicate_matches(
            &fields,
            Some("Code"),
            FilterOp::Contains,
            Some("23")
        ));
    }

    #[test]
    fn gt_wrong_type_does_not_match() {
        let fields = json!({ "Name": "not a number" });
        assert!(!predicate_matches(
            &fields,
            Some("Name"),
            FilterOp::Gt,
            Some("10")
        ));
    }

    #[test]
    fn gt_unparseable_value_does_not_match() {
        let fields = json!({ "Value": 50 });
        assert!(!predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Gt,
            Some("not-a-number")
        ));
    }

    #[test]
    fn contains_on_object_deep_scans_nested_values() {
        let fields = json!({
            "Data": { "Nested": { "Label": "SpecialSteel" } }
        });
        assert!(predicate_matches(
            &fields,
            Some("Data"),
            FilterOp::Contains,
            Some("steel")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Data"),
            FilterOp::Contains,
            Some("wood")
        ));
    }
}
