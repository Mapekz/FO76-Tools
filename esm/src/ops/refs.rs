//! Reverse-reference ops: who references a record, and how two records
//! connect.

use super::{RecordSel, resolve_sel};
use crate::refs::{
    RefSeeds, find_ref_path, referenced_by_enriched, referenced_by_enriched_multi,
    resolve_ref_seeds,
};
use crate::{CarrierTag, Database};
use serde::{Deserialize, Serialize};

/// Walk the reverse-reference graph from a record (or from every carrier of
/// an entry point or OMOD property) out to `depth` hops.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ReferencedByArgs {
    pub sel: RecordSel,
    pub limit: usize,
    /// How far the reverse-reference walk goes (default: direct references).
    #[serde(default)]
    pub depth: super::RefDepth,
    /// Narrow rows to referencing records of this 4-character type
    /// signature (e.g. `"OMOD"`); case-insensitive. Applied during the walk
    /// itself: non-matching nodes are still traversed so deeper hops stay
    /// reachable, only excluded from the emitted rows/limit/total.
    #[serde(default)]
    pub type_filter: Option<String>,
    /// Annotate each emitted row with the JSON field path(s) inside it
    /// that reference its direct predecessor in the hop chain. Opt-in —
    /// requires decoding every emitted row, unlike the default walk.
    #[serde(default)]
    pub paths: bool,
    /// Row ordering applied before `limit` truncation. `Depth` yields a
    /// breadth-first prefix under `limit` instead of a FormID-lexical slice.
    #[serde(default)]
    pub sort: RefSort,
}

pub(super) fn referenced_by(db: &Database, args: &ReferencedByArgs) -> anyhow::Result<RefList> {
    match resolve_ref_seeds(db, &args.sel)? {
        RefSeeds::Direct(target) => referenced_by_enriched(
            db,
            target,
            args.depth,
            args.limit,
            args.type_filter.as_deref(),
            args.paths,
            args.sort,
        ),
        RefSeeds::Carriers { label, seeds } => referenced_by_enriched_multi(
            db,
            &seeds,
            label,
            args.depth,
            args.limit,
            args.type_filter.as_deref(),
            args.paths,
            args.sort,
        ),
    }
}

/// Find one connecting chain of reverse-reference hops between two
/// records — see [`find_ref_path`]. Distinct from `ReferencedBy`: that
/// enumerates the *entire* reverse-reference graph out to a depth; this
/// answers "how (if at all) is A connected to B" directly, via a
/// bidirectional search that never materializes the full closure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefPathArgs {
    pub from: RecordSel,
    pub to: RecordSel,
    /// Combined hop-count ceiling across both search directions
    /// (0 = [`crate::refs::DEFAULT_MAX_PATH_HOPS`]).
    #[serde(default)]
    pub max_hops: usize,
    /// Annotate each hop with the JSON field path(s) inside it that
    /// reference the previous hop. Opt-in — requires decoding every hop on
    /// the chain, unlike the default search.
    #[serde(default)]
    pub paths: bool,
}

pub(super) fn ref_path(
    db: &Database,
    args: &RefPathArgs,
) -> anyhow::Result<crate::refs::RefPathResult> {
    let from = resolve_sel(db, &args.from)?;
    let to = resolve_sel(db, &args.to)?;
    find_ref_path(db, from, to, args.max_hops, args.paths)
}

/// Row ordering for a [`Op::ReferencedBy`] walk, applied inside the op in
/// `refs::referenced_by_walk` before `limit` truncation (sorting after
/// truncation would be meaningless — the truncation has already happened).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
pub enum RefSort {
    /// Sort by FormID ascending — the default when the field is omitted.
    #[default]
    Formid,
    /// Sort by `(depth, form_id)` — under `--limit`, this yields a
    /// breadth-first prefix of the walk instead of a FormID-lexical slice.
    Depth,
}

/// One node on the hop chain from the lookup target to a result record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefPathNode {
    pub form_id: String,
    pub record_type: Option<String>,
    pub editor_id: Option<String>,
}

/// One referencer row enriched with record type (refs command output).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(try_from = "RefRowJson")]
pub struct RefRow {
    pub form_id: String,
    /// `form_id`, typed, for in-process consumers (chase, walk). Not
    /// serialized; a row read back from JSON gets it from `form_id`.
    #[serde(skip)]
    #[cfg_attr(test, ts(skip))]
    pub id: crate::FormId,
    pub record_type: Option<String>,
    pub editor_id: Option<String>,
    pub name: Option<String>,
    pub offset: u64,
    /// Hop distance from the lookup target (1 = direct reference).
    pub depth: usize,
    /// Intermediate nodes on the path from target to this record.
    /// Empty when depth = 1 (direct reference).
    ///
    /// In an entry-point walk (`emit_seeds`), `path[0]` is the originating
    /// carrier, so depth-1 rows already have a non-empty path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<RefPathNode>,
    /// JSON field path(s) inside this record's decoded body where it
    /// references its direct predecessor in the hop chain (the walk target
    /// itself, for depth = 1 rows) — e.g.
    /// `"Effects[2].Conditions[0].Parameter 1"`. `None` unless `--paths` was
    /// requested: computing this requires decoding the full record, so it's
    /// opt-in and left absent on the default fast walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_paths: Option<Vec<String>>,
    /// Carrier tags the originating carrier matched, when the walk was seeded
    /// by a virtual selector (e.g. entry-point). On a `depth: 0` row these are
    /// the carrier's own matches; on deeper rows they are inherited from the
    /// carrier the BFS reached this record through (`path[0]`). Empty for a
    /// single-target walk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<CarrierTag>,
}

/// A [`RefRow`] as JSON carries it: everything but the typed `id`, which
/// reading it back derives from `form_id`.
#[derive(Deserialize)]
struct RefRowJson {
    form_id: String,
    record_type: Option<String>,
    editor_id: Option<String>,
    name: Option<String>,
    offset: u64,
    depth: usize,
    #[serde(default)]
    path: Vec<RefPathNode>,
    #[serde(default)]
    field_paths: Option<Vec<String>>,
    #[serde(default)]
    tags: Vec<CarrierTag>,
}

impl TryFrom<RefRowJson> for RefRow {
    type Error = String;

    fn try_from(row: RefRowJson) -> Result<Self, String> {
        let id = crate::parse_form_id_input(&row.form_id)
            .map_err(|e| format!("reference row form_id {:?}: {e}", row.form_id))?;
        Ok(RefRow {
            form_id: row.form_id,
            id,
            record_type: row.record_type,
            editor_id: row.editor_id,
            name: row.name,
            offset: row.offset,
            depth: row.depth,
            path: row.path,
            field_paths: row.field_paths,
            tags: row.tags,
        })
    }
}

/// Referenced-by result with total count and optional cap flag.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefList {
    pub target: String,
    pub rows: Vec<RefRow>,
    pub total: usize,
    pub capped: bool,
    /// Total depth-0 carrier rows before `--limit` truncation. Set only for
    /// entry-point (multi-seed) walks; used by the CLI capped-output note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier_total: Option<usize>,
    /// Total distinct tag ids across all seeds. Set only for carrier-seeded
    /// walks (e.g. entry-point); used by the CLI capped-output note. `None`
    /// for a plain single-target walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_total: Option<usize>,
    /// The raw `depth` this walk was asked for, before clamping — lets a
    /// caller detect that its request was silently adjusted. `0` means the
    /// caller asked for an unbounded walk (see [`DEFAULT_MAX_DEPTH`]).
    #[serde(default)]
    pub requested_depth: usize,
    /// The `max_depth` this walk actually used, post-clamp. `None` when
    /// `requested_depth == 0` (unbounded — there is no fixed cap to report).
    #[serde(default)]
    pub effective_depth: Option<usize>,
    /// True when the BFS discovered nodes at `effective_depth` that were
    /// never expanded further — this result is a genuine subset of the full
    /// reverse-reference graph, not its complete closure, regardless of
    /// `capped`/`--limit`.
    #[serde(default)]
    pub depth_capped: bool,
    /// Count of newly-discovered nodes at `effective_depth` that were not
    /// expanded (see `depth_capped`). Zero whenever `depth_capped` is false.
    #[serde(default)]
    pub frontier_remaining: usize,
    /// Row count per hop depth, index = depth, computed before `--limit`
    /// truncation (so this reflects the full walk, not just what's shown).
    /// Index 0 is always the carrier-row count (0 for a single-target walk).
    #[serde(default)]
    pub per_depth_totals: Vec<usize>,
    /// The deepest depth present in `rows` after `--limit` truncation — lets
    /// a truncated result state precisely "you only got hops 1..=N".
    #[serde(default)]
    pub shown_max_depth: usize,
}

#[cfg(test)]
mod ref_row_tests {
    use super::*;

    #[test]
    fn a_row_read_back_from_json_keeps_its_typed_id() {
        let row = RefRow {
            form_id: crate::FormId(0x0050_0030).display(),
            id: crate::FormId(0x0050_0030),
            depth: 1,
            ..RefRow::default()
        };
        let back: RefRow = serde_json::from_value(serde_json::to_value(&row).unwrap()).unwrap();
        assert_eq!(back.id, row.id);
        let bad = serde_json::json!({"form_id": "nope", "record_type": null, "editor_id": null,
            "name": null, "offset": 0, "depth": 1});
        assert!(serde_json::from_value::<RefRow>(bad).is_err());
    }
}
