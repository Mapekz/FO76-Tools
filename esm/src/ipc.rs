//! Wire types and the canonical `dispatch` function shared by CLI, daemon, and N-API.

use crate::diff::{DiffOptions, diff_databases_with};
use crate::refs::{
    RefSeeds, find_ref_path, referenced_by_enriched, referenced_by_enriched_multi,
    resolve_ref_seeds,
};
use crate::registry::Registry;
use crate::{CarrierTag, Database, FilterOp, FormId, ResolveDepth, SearchField};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Default maximum recursion depth for the reverse-reference walk.
pub const DEFAULT_MAX_DEPTH: usize = 8;

// ─── Wire types ─────────────────────────────────────────────────────────────

/// A request to execute one operation against an ESM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub esm: PathBuf,
    pub op: Op,
}

/// Success or error envelope returned by the daemon `/op` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { data: Value },
    Err { error: String },
}

impl Response {
    pub fn from_result(result: anyhow::Result<Value>) -> Self {
        match result {
            Ok(data) => Response::Ok { data },
            Err(e) => Response::Err {
                error: format!("{:#}", e),
            },
        }
    }
}

/// Record selector: FormID, EditorID, or an ambiguous bare token that could be
/// either (see [`RecordSel::Auto`]).
///
/// Adjacently tagged so primitive-newtype variants (FormId wraps u32, Edid wraps String)
/// survive JSON round-trips. Internally-tagged enums cannot serialize non-map payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RecordSel {
    FormId(FormId),
    Edid(String),
    /// A bare token with no explicit `0x` prefix that nonetheless *looks*
    /// like a FormID per [`crate::looks_like_formid`] (e.g. `"18000"`,
    /// `"cafe"`) — resolution tries the FormID interpretation first, then
    /// falls back to an EditorID lookup (see [`resolve_sel`]). This exists
    /// because `looks_like_formid` is a syntactic heuristic that also
    /// matches plenty of real, numeric-looking EditorIDs; an explicit `0x`
    /// prefix (or an explicit `--formid`/`--edid` flag) is unambiguous and
    /// stays `FormId`/`Edid` directly, never `Auto`.
    Auto(String),
    /// A PERK "Entry Point" name or numeric id (see [`crate::EntryPointSpec::parse`]),
    /// resolving to every PERK that carries it rather than a single record.
    /// Only meaningful for [`Op::ReferencedBy`] (via [`resolve_ref_seeds`]) —
    /// `resolve_sel`, which every other `Op` uses, rejects it. Never produced
    /// by [`RecordSel::from_input`]/[`RecordSel::from_parts`]; constructed
    /// only by the CLI's explicit `--entry-point`/`--ep` flag or the MCP
    /// `entry_point` argument.
    EntryPoint(String),
    /// An OMOD Property name or scoped numeric id (see
    /// [`crate::OmodPropertySpec::parse`]), resolving to every OMOD that declares it
    /// rather than a single record. Only meaningful for
    /// [`Op::ReferencedBy`] (via [`resolve_ref_seeds`]) — `resolve_sel`,
    /// which every other `Op` uses, rejects it. Never produced by
    /// [`RecordSel::from_input`]/[`RecordSel::from_parts`]; constructed only
    /// by the CLI's explicit `--omod-property`/`--prop` flag or the MCP
    /// `property` argument.
    OmodProperty(String),
}

impl RecordSel {
    /// Build a selector from a single user-supplied token, auto-detecting whether
    /// it denotes a FormID (numeric/hex) or an EditorID via [`crate::looks_like_formid`].
    ///
    /// A bare (no `0x`/`0X` prefix) formid-looking token is ambiguous — it
    /// could be a real FormID or a numeric-looking EditorID — so it becomes
    /// [`RecordSel::Auto`] rather than eagerly committing to `FormId`; an
    /// explicit `0x`-prefixed token is unambiguous and stays `FormId`.
    pub fn from_input(s: &str) -> anyhow::Result<RecordSel> {
        let trimmed = s.trim();
        let has_hex_prefix = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .is_some();
        if crate::looks_like_formid(s) {
            if has_hex_prefix {
                Ok(RecordSel::FormId(crate::parse_form_id_input(s)?))
            } else {
                Ok(RecordSel::Auto(s.to_string()))
            }
        } else {
            Ok(RecordSel::Edid(s.to_string()))
        }
    }

    /// Build a selector from explicit `--formid`/`--edid` inputs, falling back to
    /// auto-detecting a single ambiguous token (a positional CLI arg, or an MCP
    /// `"id"` argument) via [`RecordSel::from_input`]. The one parser shared by
    /// the CLI's `record_sel` and the MCP server's `sel_from_args` call sites.
    pub fn from_parts(
        formid: Option<&str>,
        edid: Option<&str>,
        target: Option<&str>,
    ) -> anyhow::Result<RecordSel> {
        if let Some(fid) = formid {
            Ok(RecordSel::FormId(crate::parse_form_id_input(fid)?))
        } else if let Some(e) = edid {
            Ok(RecordSel::Edid(e.to_string()))
        } else if let Some(t) = target {
            RecordSel::from_input(t)
        } else {
            bail!("specify a FormID/EditorID, or --formid/--edid")
        }
    }

    /// Render the selector for display/correlation purposes: a FormID hex
    /// string (`0x0000463F`) or the literal EditorID text. Used to tag each
    /// entry of a [`Op::RecordBulk`] result so callers can match a result back
    /// to the selector they requested, even on failure.
    pub fn display(&self) -> String {
        match self {
            RecordSel::FormId(fid) => fid.display(),
            RecordSel::Edid(edid) => edid.clone(),
            RecordSel::Auto(token) => token.clone(),
            RecordSel::EntryPoint(token) => token.clone(),
            RecordSel::OmodProperty(token) => token.clone(),
        }
    }
}

/// All operations routable through `dispatch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    FileInfo,
    Record {
        sel: RecordSel,
        depth: ResolveDepth,
    },
    /// Fetch and resolve multiple records in one round-trip — the bulk
    /// counterpart to `Record`. Each selector is resolved and decoded
    /// independently (see [`BulkRecordEntry`]): one bad FormID/EditorID
    /// produces an error entry for that selector only, it does not fail the
    /// whole call. A new variant rather than a `Vec<RecordSel>` on `Record`
    /// itself, so the existing single-record wire shape (and its
    /// byte-for-byte CLI output) is untouched — older clients that only know
    /// about `Record`/`RecordRaw` keep working unmodified, and newer clients
    /// opt into batching by sending this variant instead.
    RecordBulk {
        sels: Vec<RecordSel>,
        depth: ResolveDepth,
    },
    RecordRaw {
        sel: RecordSel,
    },
    ListByType {
        sig: String,
        limit: usize,
    },
    ListTypeRecords {
        sig: String,
        offset: usize,
        limit: usize,
    },
    /// Filter records of type `sig` by a predicate against their decoded field
    /// body. See [`crate::Database::filter_type_records`] for path/operator semantics.
    FilterTypeRecords {
        sig: String,
        path: Option<String>,
        // Named `filter_op` (not `op`) to avoid colliding with this enum's own
        // `#[serde(tag = "op")]` wire discriminant.
        filter_op: FilterOp,
        value: Option<String>,
        limit: usize,
    },
    /// Union of all dot-notation field paths observed across a decoded sample
    /// of a type's records — see [`crate::Database::list_type_field_paths`].
    ListTypeFieldPaths {
        sig: String,
    },
    Search {
        pattern: String,
        types: Vec<String>,
        field: SearchField,
        limit: usize,
    },
    ReferencedBy {
        sel: RecordSel,
        limit: usize,
        /// Recursion depth for the reverse-reference walk (default 1, capped at DEFAULT_MAX_DEPTH).
        #[serde(default)]
        depth: usize,
        /// Narrow rows to referencing records of this 4-character type
        /// signature (e.g. `"OMOD"`); case-insensitive. Applied server-side
        /// during the walk itself: non-matching nodes are still traversed so
        /// deeper hops stay reachable, only excluded from the emitted
        /// rows/limit/total. `None` (the wire default for older clients) = no
        /// filter.
        #[serde(default)]
        type_filter: Option<String>,
        /// Annotate each emitted row with the JSON field path(s) inside it
        /// that reference its direct predecessor in the hop chain. Opt-in —
        /// requires decoding every emitted row, unlike the default walk.
        #[serde(default)]
        paths: bool,
        /// Row ordering applied before `limit` truncation. `Formid` (the
        /// wire default for older clients) preserves today's behavior;
        /// `Depth` yields a breadth-first prefix under `--limit` instead of
        /// a FormID-lexical slice.
        #[serde(default)]
        sort: RefSort,
    },
    /// Find one connecting chain of reverse-reference hops between two
    /// records — see [`find_ref_path`]. Distinct from `ReferencedBy`: that
    /// enumerates the *entire* reverse-reference graph out to a depth;
    /// this answers "how (if at all) is A connected to B" directly, via a
    /// bidirectional search that never materializes the full closure.
    RefPath {
        from: RecordSel,
        to: RecordSel,
        /// Combined hop-count ceiling across both search directions
        /// (0 = [`crate::refs::DEFAULT_MAX_PATH_HOPS`]).
        #[serde(default)]
        max_hops: usize,
        /// Annotate each hop with the JSON field path(s) inside it that
        /// reference the previous hop. Opt-in — requires decoding every
        /// hop on the chain, unlike the default search.
        #[serde(default)]
        paths: bool,
    },
    /// Interactive record digest — the server-side counterpart to `esm walk`
    /// (see [`crate::walk`]). The BFS and per-node digest computation run
    /// entirely inside the process handling this op (daemon or `--local`),
    /// one round trip regardless of how many nodes the walk visits, not one
    /// `Op::RecordBulk` call per queue-pop. `want_refs` mirrors the CLI's
    /// `--refs` flag: when true and the root resolved, one extra unfiltered
    /// reverse-reference walk runs on the root and is folded into
    /// [`crate::walk::WalkResult::refs`]. When the root selector doesn't
    /// resolve, `not_found.matches` is filled in by one in-process
    /// [`Database::search`] call, folding the "not-found -> search fallback"
    /// into this op rather than requiring the caller to drive a second
    /// `Op::Search` (see `docs/adr/0001`'s dated amendment).
    Walk {
        sel: RecordSel,
        depth: usize,
        ref_limit: usize,
        level: f32,
        want_refs: bool,
    },
    /// Pipeline evidence contract — the server-side counterpart to
    /// `esm chase` (see [`crate::chase`]). Always emits the classified
    /// `ChaseTree`; hard-errors on a selector that doesn't resolve to one of
    /// the five accepted root types (OMOD/PERK/SPEL/ALCH/ENCH).
    Chase {
        sel: RecordSel,
        depth: usize,
        ref_limit: usize,
    },
    /// LVLI drop-probability table — the server-side counterpart to
    /// [`crate::lvli::drop_table`], reachable standalone (not only via
    /// `Op::Walk`'s LVLI digest) for MCP/N-API callers that just want the
    /// resolved odds. Hard-errors on a selector that doesn't resolve to an
    /// LVLI record.
    DropTable {
        sel: RecordSel,
        level: f32,
        max_depth: usize,
        strict: bool,
    },
    ListGroups,
    ListTypeChildren {
        sig: String,
        offset: usize,
        limit: usize,
    },
    /// List direct children of an arbitrary GRUP by its own header offset — see
    /// [`crate::Database::list_group_children`].
    ListGroupChildren {
        group_offset: u64,
        offset: usize,
        limit: usize,
    },
    /// Lightweight record header at a file offset — see [`crate::Database::record_stub_at`].
    RecordStubAt {
        offset: u64,
    },
    Coverage {
        record_type: Option<String>,
        sample: usize,
    },
    Diff {
        b: PathBuf,
        record_type: Option<String>,
        /// Body-detail / noise-suppression / type-exclusion controls (see
        /// [`DiffOptions`]). `#[serde(default)]` keeps older wire clients that
        /// never send this field compatible — they get `DiffOptions::default()`.
        #[serde(default)]
        options: DiffOptions,
    },
    /// Daemon lifecycle: no ESM path required (ignored).
    Shutdown,
}

// ─── Shared DTOs (lifted from CLI) ──────────────────────────────────────────

/// Row ordering for a [`Op::ReferencedBy`] walk, applied server-side inside
/// `refs::referenced_by_walk` before `limit` truncation (sorting after
/// truncation would be meaningless — the truncation has already happened).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
pub enum RefSort {
    /// Sort by FormID ascending — today's behavior, and the wire default for
    /// older clients that predate this field.
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
pub struct RefRow {
    pub form_id: String,
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

/// One entry of a [`Op::RecordBulk`] result: the resolved record on success,
/// or an isolated per-selector error message on failure. Mirrors the plain
/// `Op::Record` JSON shape (`header`/`editor_id`/`fields`) with a `sel` field
/// prepended so callers can correlate each entry back to the selector they
/// requested — necessary because one bad FormID/EditorID must not fail the
/// whole bulk call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct BulkRecordEntry {
    /// The selector as supplied, rendered for display — a FormID hex string
    /// (`0x0000463F`) or the literal EditorID text (see [`RecordSel::display`]).
    pub sel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<crate::reader::RecordHeaderInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "Record<string, unknown> | null"))]
    pub fields: Option<Value>,
    /// Set instead of `header`/`editor_id`/`fields` when this selector could
    /// not be resolved or decoded — the failure is isolated to this entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Hex dump view of a raw record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RawRecordView {
    pub header: crate::reader::RecordHeaderInfo,
    pub subrecords: Vec<RawSubrecordView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RawSubrecordView {
    pub signature: String,
    pub size: usize,
    pub hex: String,
}

/// Convert a raw parsed record into its hex-dump presentation view.
pub fn raw_record_view(rec: &crate::reader::ParsedRecord) -> RawRecordView {
    RawRecordView {
        header: rec.header.clone(),
        subrecords: rec
            .subrecords
            .iter()
            .map(|sr| RawSubrecordView {
                signature: sr.signature.to_string(),
                size: sr.data.len(),
                hex: sr.data.iter().map(|b| format!("{:02x}", b)).collect(),
            })
            .collect(),
    }
}

/// Counts of schema-coverage markers per record type.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Markers {
    pub unknown_record: u64,
    pub raw_fallback: u64,
    pub unmapped: u64,
    pub unresolved: u64,
    pub records: u64,
}

impl Markers {
    pub fn total(&self) -> u64 {
        self.unknown_record + self.raw_fallback + self.unmapped + self.unresolved
    }

    pub fn add(&mut self, other: &Markers) {
        self.unknown_record += other.unknown_record;
        self.raw_fallback += other.raw_fallback;
        self.unmapped += other.unmapped;
        self.unresolved += other.unresolved;
        self.records += other.records;
    }
}

/// Coverage audit report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct CoverageReport {
    pub by_type: BTreeMap<String, Markers>,
    pub totals: Markers,
}

// ─── Dispatch ───────────────────────────────────────────────────────────────

/// Execute `req` against the registry, returning a [`Response`].
pub fn dispatch(reg: &Registry, req: &Request) -> Response {
    Response::from_result(dispatch_inner(reg, req))
}

fn dispatch_inner(reg: &Registry, req: &Request) -> anyhow::Result<Value> {
    match &req.op {
        Op::Shutdown => Ok(Value::Null),
        Op::Diff {
            b,
            record_type,
            options,
        } => {
            let (key_a, arc_a) = reg.get_or_open_with_key(&req.esm)?;
            let (key_b, arc_b) = reg.get_or_open_with_key(b)?;
            diff_pair(&arc_a, &arc_b, Some((&key_a, &key_b)), options, record_type)
        }
        _ => {
            let arc = reg.get_or_open(&req.esm)?;
            let mut db = arc.lock().unwrap();
            dispatch_op(&mut db, &req.op)
        }
    }
}

// ─── in-process ChaseFetcher adapter ────────────────────────────────────────

/// In-process [`crate::chase::ChaseFetcher`] adapter over an already-open
/// `Database`. `Op::Walk`/`Op::Chase`/`Op::DropTable` use this so
/// `crate::walk::walk`'s BFS and `crate::chase::chase`'s classifier run
/// entirely inside the process already holding the lock on `db` — no
/// serialization, no round-trip, and `walk`'s one-node-per-queue-pop
/// `bulk_get` costs no HTTP hop per BFS node.
struct DbFetcher<'a> {
    db: &'a mut Database,
}

impl crate::chase::ChaseFetcher for DbFetcher<'_> {
    fn bulk_get(
        &mut self,
        sels: &[RecordSel],
        depth: ResolveDepth,
    ) -> anyhow::Result<Vec<BulkRecordEntry>> {
        Ok(sels
            .iter()
            .map(|sel| bulk_record_entry(self.db, sel, depth))
            .collect())
    }

    fn refs(
        &mut self,
        target: FormId,
        depth: usize,
        limit: usize,
        type_filter: &str,
        paths: bool,
    ) -> anyhow::Result<RefList> {
        crate::refs::referenced_by_enriched(
            self.db,
            target,
            depth,
            limit,
            Some(type_filter),
            paths,
            RefSort::Formid,
        )
    }
}

/// Execute a single `Op` against an already-open `Database`.
pub fn dispatch_op(db: &mut Database, op: &Op) -> anyhow::Result<Value> {
    match op {
        Op::Shutdown => Ok(Value::Null),
        Op::FileInfo => {
            let info = db.file_info()?;
            Ok(serde_json::to_value(&info)?)
        }
        Op::Record { sel, depth } => {
            // `RecordResult`'s `Serialize` impl produces the exact same
            // `{header, editor_id, fields}` shape a hand-built `json!` would
            // (no serde renames, no optional-field skipping) — this is the one
            // authoritative shape both the CLI/daemon and N-API bindings read.
            let result = record_resolved(db, sel, *depth)?;
            Ok(serde_json::to_value(&result)?)
        }
        Op::RecordBulk { sels, depth } => {
            let entries: Vec<BulkRecordEntry> = sels
                .iter()
                .map(|sel| bulk_record_entry(db, sel, *depth))
                .collect();
            Ok(serde_json::to_value(&entries)?)
        }
        Op::RecordRaw { sel } => {
            let form_id = resolve_sel(db, sel)?;
            let rec = db
                .record_raw(form_id)
                .map_err(|e| explain_hardcoded_miss(form_id, e))?;
            let view = raw_record_view(&rec);
            Ok(serde_json::to_value(&view)?)
        }
        Op::ListByType { sig, limit } => {
            let entries = db.list_by_type(sig, *limit)?;
            Ok(serde_json::to_value(&entries)?)
        }
        Op::ListTypeRecords { sig, offset, limit } => {
            let rows = db.list_type_records(sig, *offset, *limit)?;
            Ok(serde_json::to_value(&rows)?)
        }
        Op::FilterTypeRecords {
            sig,
            path,
            filter_op,
            value,
            limit,
        } => {
            let result =
                db.filter_type_records(sig, path.as_deref(), *filter_op, value.as_deref(), *limit)?;
            Ok(serde_json::to_value(&result)?)
        }
        Op::ListTypeFieldPaths { sig } => {
            let paths = db.list_type_field_paths(sig)?;
            Ok(serde_json::to_value(&paths)?)
        }
        Op::Search {
            pattern,
            types,
            field,
            limit,
        } => {
            if pattern.is_empty() {
                bail!("search pattern must not be empty (use \"*\" to match all records)");
            }
            let types: Vec<String> = types
                .iter()
                .map(|t| {
                    let up = t.to_uppercase();
                    if up.len() != 4 {
                        bail!("record type '{}' must be a 4-character signature", t);
                    }
                    Ok(up)
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let results = db.search(pattern, &types, *field, *limit)?;
            Ok(serde_json::to_value(&results)?)
        }
        Op::ReferencedBy {
            sel,
            limit,
            depth,
            type_filter,
            paths,
            sort,
        } => {
            let ref_list = match resolve_ref_seeds(db, sel)? {
                RefSeeds::Direct(target) => referenced_by_enriched(
                    db,
                    target,
                    *depth,
                    *limit,
                    type_filter.as_deref(),
                    *paths,
                    *sort,
                )?,
                RefSeeds::Carriers { label, seeds } => referenced_by_enriched_multi(
                    db,
                    &seeds,
                    label,
                    *depth,
                    *limit,
                    type_filter.as_deref(),
                    *paths,
                    *sort,
                )?,
            };
            Ok(serde_json::to_value(&ref_list)?)
        }
        Op::Walk {
            sel,
            depth,
            ref_limit,
            level,
            want_refs,
        } => {
            let opts = crate::walk::WalkOptions {
                depth: *depth,
                ref_limit: *ref_limit,
                level: *level,
            };
            let mut result = {
                let mut fetcher = DbFetcher { db: &mut *db };
                crate::walk::walk(&mut fetcher, sel.clone(), &opts)?
            };
            if let Some(nf) = result.not_found.as_mut() {
                nf.matches = db.search(&nf.target, &[], SearchField::Both, 10)?;
            } else if *want_refs && let Some(root) = result.nodes.first() {
                let root_fid = crate::parse_form_id_input(&root.formid)?;
                let ref_list = crate::refs::referenced_by_enriched(
                    db,
                    root_fid,
                    1,
                    0,
                    None,
                    false,
                    RefSort::Formid,
                )?;
                result.refs = Some(crate::walk::build_refs_digest(&ref_list.rows));
            }
            Ok(serde_json::to_value(&result)?)
        }
        Op::Chase {
            sel,
            depth,
            ref_limit,
        } => {
            let opts = crate::chase::ChaseOptions {
                depth: *depth,
                ref_limit: *ref_limit,
            };
            let mut fetcher = DbFetcher { db: &mut *db };
            let tree = crate::chase::chase(&mut fetcher, sel.clone(), &opts)?;
            Ok(serde_json::to_value(&tree)?)
        }
        Op::DropTable {
            sel,
            level,
            max_depth,
            strict,
        } => {
            let result = record_resolved(db, sel, ResolveDepth::Stub)?;
            if result.header.signature != "LVLI" {
                bail!(
                    "{:?} resolves to a {:?} record — drop-table only supports LVLI selectors",
                    sel.display(),
                    result.header.signature
                );
            }
            let opts = crate::lvli::DropOptions {
                level: *level,
                max_depth: *max_depth,
                strict: *strict,
            };
            let mut fetcher = DbFetcher { db: &mut *db };
            let table = crate::lvli::drop_table(
                &mut fetcher,
                result.header.form_id,
                &result.fields,
                &opts,
            )?;
            Ok(serde_json::to_value(&table)?)
        }
        Op::RefPath {
            from,
            to,
            max_hops,
            paths,
        } => {
            let from = resolve_sel(db, from)?;
            let to = resolve_sel(db, to)?;
            let result = find_ref_path(db, from, to, *max_hops, *paths)?;
            Ok(serde_json::to_value(&result)?)
        }
        Op::ListGroups => {
            let groups = db.list_groups();
            Ok(serde_json::to_value(&groups)?)
        }
        Op::ListTypeChildren { sig, offset, limit } => {
            let children = db.list_type_children(sig, *offset, *limit)?;
            Ok(serde_json::to_value(&children)?)
        }
        Op::ListGroupChildren {
            group_offset,
            offset,
            limit,
        } => {
            let children = db.list_group_children(*group_offset, *offset, *limit)?;
            Ok(serde_json::to_value(&children)?)
        }
        Op::RecordStubAt { offset } => {
            let stub = db.record_stub_at(*offset)?;
            Ok(serde_json::to_value(&stub)?)
        }
        Op::Coverage {
            record_type,
            sample,
        } => {
            let report = coverage_report(db, record_type.as_deref(), *sample)?;
            Ok(serde_json::to_value(&report)?)
        }
        Op::Diff { .. } => {
            bail!("Diff must be dispatched via registry with two ESM paths");
        }
    }
}

/// Resolve a [`RecordSel`] to a concrete [`FormId`], looking up the EditorID
/// index when needed. The one canonical selector-resolution used by every
/// serving surface (daemon, CLI, N-API) — do not reimplement this locally.
pub fn resolve_sel(db: &mut Database, sel: &RecordSel) -> anyhow::Result<FormId> {
    match sel {
        RecordSel::FormId(fid) => Ok(*fid),
        RecordSel::Edid(edid) => {
            db.ensure_edid_index()?;
            // Real ESM records take precedence — only consult the
            // engine-hardcoded table (`crate::hardcoded`) once the real
            // index has already missed, per its own fallback-only contract.
            db.index
                .get_by_edid(edid)
                .or_else(|| crate::hardcoded::lookup_by_editor_id(edid))
                .ok_or_else(|| anyhow::anyhow!("EditorID '{}' not found", edid))
        }
        RecordSel::Auto(token) => {
            // Try the FormID interpretation first — byte-identical to today's
            // behavior when it actually resolves to a present record. Only
            // fall back to an EditorID lookup when that fails, so a real
            // FormID never gets silently redirected to an unrelated
            // same-named EditorID.
            let formid_attempt = crate::parse_form_id_input(token).ok();
            if let Some(fid) = formid_attempt
                && db.get_formid_meta(fid).is_ok()
            {
                return Ok(fid);
            }
            db.ensure_edid_index()?;
            if let Some(fid) = db.index.get_by_edid(token) {
                return Ok(fid);
            }
            // Both real-ESM attempts (FormID and EditorID) have now missed —
            // only then fall back to the hardcoded table, same
            // real-record-wins ordering as the `Edid` branch above.
            if let Some(fid) = crate::hardcoded::lookup_by_editor_id(token) {
                return Ok(fid);
            }
            match formid_attempt {
                Some(fid) => bail!(
                    "'{token}' did not resolve as FormID {} (not found) or as EditorID '{token}' (not found)",
                    fid.display()
                ),
                None => bail!("EditorID '{token}' not found"),
            }
        }
        RecordSel::EntryPoint(token) => bail!(
            "entry-point selector '{token}' is only valid for refs \
             (Op::ReferencedBy) — it doesn't resolve to a single record"
        ),
        RecordSel::OmodProperty(token) => bail!(
            "OMOD-property selector '{token}' is only valid for refs \
             (Op::ReferencedBy) — it doesn't resolve to a single record"
        ),
    }
}

fn record_resolved(
    db: &mut Database,
    sel: &RecordSel,
    depth: ResolveDepth,
) -> anyhow::Result<crate::RecordResult> {
    // `record_by_formid_resolved` already collapses to an unresolved decode
    // when `depth == ResolveDepth::None`, so there's no separate "unresolved"
    // path to special-case here.
    //
    // Delegate FormID/EditorID/Auto resolution to `resolve_sel` uniformly
    // (rather than `Database::record_by_edid_resolved` reimplementing the
    // EditorID lookup locally) so every selector form — not just `Auto` —
    // gets `resolve_sel`'s hardcoded-table fallback. `resolve_sel` already
    // bails with the exact same message text for `EntryPoint`, so there is
    // no separate arm needed for it either.
    let fid = resolve_sel(db, sel)?;
    db.record_by_formid_resolved(fid, depth)
        .map_err(|e| explain_hardcoded_miss(fid, e))
}

/// If `form_id` is one of the ~228 engine-hardcoded FormIDs (`crate::hardcoded`)
/// with no backing ESM record, replace a "not found" error with an
/// explanation of *why* — it's baked into the game executable, not decodable
/// data — plus a pointer at `esm refs` for its referrers. Otherwise passes
/// `err` through unchanged.
///
/// Applied only at this serving boundary (`record_resolved`, `Op::RecordRaw`
/// below), not inside `Database::get_formid_meta` itself: that method is also
/// called from `DatabaseResolver::stub`/`decode_full` (`src/lib.rs`), which
/// already do their own `hardcoded::lookup` and discard the miss error
/// entirely (`let Ok(meta) = ... else { ... }`), and from `resolve_sel`'s
/// `Auto` probe, which only checks `.is_ok()`. Building this string there
/// would cost an extra binary search plus an allocation on every unresolved
/// reference of a `--resolve stub|full` decode or `coverage`/`diff` sweep —
/// paths that can hit it millions of times — for a hint two of those three
/// callers throw away.
fn explain_hardcoded_miss(form_id: FormId, err: anyhow::Error) -> anyhow::Error {
    let Some(form) = crate::hardcoded::lookup(form_id) else {
        return err;
    };
    let named = match &form.editor_id {
        Some(edid) => format!("{} ({} '{}')", form_id.display(), form.record_type, edid),
        None => format!("{} ({})", form_id.display(), form.record_type),
    };
    anyhow::anyhow!(
        "{named} is an engine-hardcoded form: it is defined by the game \
         executable, not by a record in this ESM, so it has no fields to \
         decode. Use `esm refs {}` to list the records that reference it.",
        form_id.display()
    )
}

/// Resolve one selector of an `Op::RecordBulk` request, converting a lookup
/// failure into an `error`-carrying [`BulkRecordEntry`] instead of aborting
/// the whole batch — the per-record failure isolation that distinguishes bulk
/// `get` from N sequential single `get`s.
fn bulk_record_entry(db: &mut Database, sel: &RecordSel, depth: ResolveDepth) -> BulkRecordEntry {
    let display = sel.display();
    match record_resolved(db, sel, depth) {
        Ok(result) => BulkRecordEntry {
            sel: display,
            header: Some(result.header),
            editor_id: result.editor_id,
            fields: Some(result.fields),
            error: None,
        },
        Err(e) => BulkRecordEntry {
            sel: display,
            header: None,
            editor_id: None,
            fields: None,
            error: Some(format!("{:#}", e)),
        },
    }
}

/// Acquire locks on two `Database` handles in a deadlock-safe order, then run
/// [`diff_locked`]. When `canonical_keys` is `Some`, ordering follows the
/// registry's normalized path keys (`dispatch_inner`'s `Diff` arm and the HTTP
/// `/diff` route). When `None`, ordering falls back to raw `Arc` pointer
/// address — the scheme the N-API `diff` method can adopt later.
pub fn diff_pair(
    arc_a: &Arc<Mutex<Database>>,
    arc_b: &Arc<Mutex<Database>>,
    canonical_keys: Option<(&Path, &Path)>,
    options: &DiffOptions,
    record_type: &Option<String>,
) -> anyhow::Result<Value> {
    let same_db =
        canonical_keys.map(|(ka, kb)| ka == kb).unwrap_or(false) || Arc::ptr_eq(arc_a, arc_b);
    if same_db {
        let db = arc_a
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        // same_db means no added records — enrich_added_sources is a no-op.
        return diff_locked(&db, &db, options, record_type);
    }
    match canonical_keys {
        Some((ka, kb)) if ka < kb => {
            let db_a = arc_a
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
            let db_b = arc_b
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
            diff_locked(&db_a, &db_b, options, record_type)
        }
        Some(_) => {
            let db_b = arc_b
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
            let db_a = arc_a
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
            diff_locked(&db_a, &db_b, options, record_type)
        }
        None => {
            if Arc::as_ptr(arc_a) < Arc::as_ptr(arc_b) {
                let db_a = arc_a
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
                let db_b = arc_b
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
                diff_locked(&db_a, &db_b, options, record_type)
            } else {
                let db_b = arc_b
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
                let db_a = arc_a
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
                diff_locked(&db_a, &db_b, options, record_type)
            }
        }
    }
}

/// Post-lock part of a database diff: run [`diff_databases_with`] and apply
/// the optional record-type filter, once both `Database` locks are already
/// held. Shared by [`diff_pair`] (registry-backed and HTTP `/diff` routes) and
/// the N-API binding's `diff` method (which keeps its own lock-acquisition
/// code until it adopts [`diff_pair`]).
pub fn diff_locked(
    db_a: &Database,
    db_b: &Database,
    options: &DiffOptions,
    record_type: &Option<String>,
) -> anyhow::Result<Value> {
    let mut result = diff_databases_with(db_a, db_b, options)?;
    crate::diff::apply_type_filter(&mut result, record_type);
    Ok(serde_json::to_value(&result)?)
}

fn count_markers(v: &Value, m: &mut Markers) {
    use crate::decode::markers;
    match v {
        Value::Object(obj) => {
            if obj.get(markers::UNKNOWN_RECORD) == Some(&Value::Bool(true)) {
                m.unknown_record += 1;
            }
            if obj.get(markers::RAW) == Some(&Value::Bool(true)) && obj.contains_key("reason") {
                m.raw_fallback += 1;
            }
            if obj.get(markers::UNRESOLVED) == Some(&Value::Bool(true)) {
                m.unresolved += 1;
            }
            if let Some(Value::Object(unmapped)) = obj.get(markers::UNMAPPED) {
                for subs in unmapped.values() {
                    if let Value::Array(arr) = subs {
                        m.unmapped += arr.len() as u64;
                    }
                }
            }
            for (key, child) in obj {
                if key == markers::UNMAPPED {
                    continue;
                }
                count_markers(child, m);
            }
        }
        Value::Array(arr) => {
            for child in arr {
                count_markers(child, m);
            }
        }
        _ => {}
    }
}

pub fn coverage_report(
    db: &Database,
    record_type: Option<&str>,
    sample: usize,
) -> anyhow::Result<CoverageReport> {
    // `signatures()` iterates the pre-built type_index's keys — already the
    // distinct set of signatures present, no HashSet dedup needed and no
    // 5.64M-record form_index scan (down from that to 178 entries).
    let mut all_sigs: Vec<String> = db
        .index
        .signatures()
        .map(|s| s.as_str().to_owned())
        .collect();
    all_sigs.sort();

    if let Some(rt) = record_type {
        let rt_upper = rt.to_uppercase();
        all_sigs.retain(|s| *s == rt_upper);
        if all_sigs.is_empty() {
            bail!("no records of type '{}' found", rt);
        }
    }

    let mut by_type: BTreeMap<String, Markers> = BTreeMap::new();

    for sig in &all_sigs {
        let metas: Vec<crate::reader::RecordMeta> = db
            .index
            .records_by_type(sig)
            .map(|(_, m)| m)
            .take(if sample == 0 { usize::MAX } else { sample })
            .collect();

        let mut type_markers = Markers::default();
        for meta in &metas {
            match db.record_at_meta_with_depth(meta, ResolveDepth::None) {
                Ok(result) => {
                    type_markers.records += 1;
                    let mut rec_markers = Markers::default();
                    count_markers(&result.fields, &mut rec_markers);
                    type_markers.add(&rec_markers);
                }
                Err(e) => {
                    eprintln!("Warning: failed to decode {} record: {}", sig, e);
                }
            }
        }
        by_type.insert(sig.clone(), type_markers);
    }

    let totals = by_type.values().fold(Markers::default(), |mut acc, m| {
        acc.add(m);
        acc
    });

    Ok(CoverageReport { by_type, totals })
}

/// Convenience: open a single ESM and run one op (used by LocalBackend).
pub fn dispatch_local(path: &std::path::Path, op: &Op) -> anyhow::Result<Value> {
    let reg = Registry::new();
    let req = Request {
        esm: path.to_path_buf(),
        op: op.clone(),
    };
    dispatch_inner(&reg, &req)
}
