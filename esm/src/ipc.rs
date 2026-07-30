//! Wire types and the canonical `dispatch` function shared by CLI, daemon, and N-API.

use crate::diff::{DiffOptions, diff_databases_with};
use crate::registry::Registry;
use crate::{
    Database, EntryPointRef, EntryPointSpec, FilterOp, FormId, RecordRow, ResolveDepth, SearchField,
};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    /// A PERK "Entry Point" name or numeric id (see [`EntryPointSpec::parse`]),
    /// resolving to every PERK that carries it rather than a single record.
    /// Only meaningful for [`Op::ReferencedBy`] (via [`resolve_ref_seeds`]) —
    /// `resolve_sel`, which every other `Op` uses, rejects it. Never produced
    /// by [`RecordSel::from_input`]/[`RecordSel::from_parts`]; constructed
    /// only by the CLI's explicit `--entry-point`/`--ep` flag.
    EntryPoint(String),
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
        /// (0 = [`DEFAULT_MAX_PATH_HOPS`]).
        #[serde(default)]
        max_hops: usize,
        /// Annotate each hop with the JSON field path(s) inside it that
        /// reference the previous hop. Opt-in — requires decoding every
        /// hop on the chain, unlike the default search.
        #[serde(default)]
        paths: bool,
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
/// [`referenced_by_walk`] before `limit` truncation (sorting after
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
    /// Entry points the originating carrier matched, when the walk was seeded
    /// by an entry-point selector. On a `depth: 0` row these are the carrier's
    /// own matches; on deeper rows they are inherited from the carrier the BFS
    /// reached this record through (`path[0]`). Empty for a single-target walk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_points: Vec<EntryPointRef>,
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
    /// Total distinct entry-point ids across all seeds. Set only for
    /// entry-point walks; used by the CLI capped-output note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point_total: Option<usize>,
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
            let rec = db.record_raw(form_id)?;
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
            db.index.ensure_edid_index(&db.esm)?;
            db.index
                .get_by_edid(edid)
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
            // Defense-in-depth: lite mode (`--mmap-index`) has no EditorID
            // index. The CLI already refuses `Auto` selectors in that mode
            // (see `mmap_index_supports` in `src/bin/cli.rs`), so this
            // shouldn't be reachable in practice, but bail with the same
            // message `record_by_edid_resolved` uses rather than panicking
            // or producing a confusing miss if it ever is.
            db.check_not_lite("EditorID lookup")?;
            db.index.ensure_edid_index(&db.esm)?;
            if let Some(fid) = db.index.get_by_edid(token) {
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
    }
}

fn record_resolved(
    db: &mut Database,
    sel: &RecordSel,
    depth: ResolveDepth,
) -> anyhow::Result<crate::RecordResult> {
    // `record_by_formid_resolved`/`record_by_edid_resolved` already collapse to
    // an unresolved decode when `depth == ResolveDepth::None`, so there's no
    // separate "unresolved" path to special-case here.
    match sel {
        RecordSel::FormId(fid) => db.record_by_formid_resolved(*fid, depth),
        RecordSel::Edid(edid) => db.record_by_edid_resolved(edid, depth),
        RecordSel::Auto(_) => {
            // Delegate to `resolve_sel` for the FormID-then-EditorID fallback
            // logic rather than duplicating it here.
            let fid = resolve_sel(db, sel)?;
            db.record_by_formid_resolved(fid, depth)
        }
        RecordSel::EntryPoint(token) => bail!(
            "entry-point selector '{token}' is only valid for refs \
             (Op::ReferencedBy) — it doesn't resolve to a single record"
        ),
    }
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

/// Walk reverse references from `target` up to `depth` hops using BFS.
///
/// A `depth` of 1 (the default) returns the same set as the old single-level
/// lookup.  Higher values follow the reverse-reference graph breadth-first,
/// visiting each node at most once (cycle-safe).  `depth` is clamped to
/// `[1, DEFAULT_MAX_DEPTH]`; `depth == 0` requests an unbounded walk instead
/// (no fixed hop cap — see [`RefList::effective_depth`]).
///
/// Each `RefRow` carries:
/// - `depth`: hop distance from `target` (1 = direct referencer).
/// - `path`: intermediate nodes between `target` and this row; empty for
///   depth-1 rows (and omitted from serialized JSON when empty).
///
/// `type_filter`, if set, must be a 4-character record-type signature
/// (case-insensitive); only rows of that type are emitted. The filter is
/// applied to *emission*, not traversal — the walk still expands through
/// non-matching nodes so a matching node further away stays reachable, and
/// `limit`/`total`/`capped` are computed against the filtered set.
///
/// `include_paths`, if true, decodes every emitted row's record body and
/// annotates it with [`RefRow::field_paths`] (see
/// [`Database::formid_reference_paths`]) — opt-in because it requires a full
/// decode per row, unlike the rest of this walk.
pub fn referenced_by_enriched(
    db: &mut Database,
    target: FormId,
    depth: usize,
    limit: usize,
    type_filter: Option<&str>,
    include_paths: bool,
    sort: RefSort,
) -> anyhow::Result<RefList> {
    let (rows, stats) = referenced_by_walk(
        db,
        &[(target, Vec::new())],
        false,
        depth,
        limit,
        type_filter,
        include_paths,
        sort,
    )?;
    Ok(RefList {
        target: target.display(),
        rows,
        total: stats.total,
        capped: stats.capped,
        carrier_total: None,
        entry_point_total: None,
        requested_depth: stats.requested_depth,
        effective_depth: stats.effective_depth,
        depth_capped: stats.depth_capped,
        frontier_remaining: stats.frontier_remaining,
        per_depth_totals: stats.per_depth_totals,
        shown_max_depth: stats.shown_max_depth,
    })
}

/// Multi-seed reverse-reference walk: every entry in `seeds` is emitted as
/// its own `depth: 0` "carrier" row — unlike [`referenced_by_enriched`],
/// whose single target is only a BFS root and never appears in the output —
/// then the BFS proceeds from all seeds at once, so a record referencing two
/// different seeds is still only emitted once. Used by [`resolve_ref_seeds`]'s
/// entry-point path (see [`crate::EntryPointSpec`]).
///
/// `seeds` carries per-carrier entry-point tags that are copied onto every
/// descendant row (and unioned on equal-depth re-reaches). Caller order is
/// preserved end-to-end — do not re-sort here; see
/// [`Database::perks_by_entry_point`].
#[allow(clippy::too_many_arguments)]
pub fn referenced_by_enriched_multi(
    db: &mut Database,
    seeds: &[(FormId, Vec<EntryPointRef>)],
    label: String,
    depth: usize,
    limit: usize,
    type_filter: Option<&str>,
    include_paths: bool,
    sort: RefSort,
) -> anyhow::Result<RefList> {
    let entry_point_total = seeds
        .iter()
        .flat_map(|(_, tags)| tags.iter().map(|ep| ep.id))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let (rows, stats) = referenced_by_walk(
        db,
        seeds,
        true,
        depth,
        limit,
        type_filter,
        include_paths,
        sort,
    )?;
    Ok(RefList {
        target: label,
        rows,
        total: stats.total,
        capped: stats.capped,
        carrier_total: Some(stats.carrier_total),
        entry_point_total: Some(entry_point_total),
        requested_depth: stats.requested_depth,
        effective_depth: stats.effective_depth,
        depth_capped: stats.depth_capped,
        frontier_remaining: stats.frontier_remaining,
        per_depth_totals: stats.per_depth_totals,
        shown_max_depth: stats.shown_max_depth,
    })
}

/// Shared BFS behind [`referenced_by_enriched`] / [`referenced_by_enriched_multi`].
///
/// `seeds` are the BFS roots, optionally tagged with entry points. When
/// `emit_seeds` is true, each seed is also emitted as its own `depth: 0` row
/// *before* the BFS-found referencer rows, and the queue is seeded with the
/// carrier's own [`RefPathNode`] so descendants' `path`/`VIA` trace back to
/// that carrier. When false (the legacy single-target path), seeds are only
/// BFS roots with empty paths — exactly as `target` always was.
///
/// Seed order is preserved (stable-dedup by FormID only). Callers that care
/// about display/attribution order — notably
/// [`Database::perks_by_entry_point`] — must sort before calling.
///
/// Returns `(rows, stats)` — see [`WalkStats`].
#[allow(clippy::too_many_arguments)]
fn referenced_by_walk(
    db: &mut Database,
    seeds: &[(FormId, Vec<EntryPointRef>)],
    emit_seeds: bool,
    depth: usize,
    limit: usize,
    type_filter: Option<&str>,
    include_paths: bool,
    sort: RefSort,
) -> anyhow::Result<(Vec<RefRow>, WalkStats)> {
    let requested_depth = depth;
    // `depth == 0` requests an unbounded walk (no fixed hop cap); any other
    // value clamps to `[1, DEFAULT_MAX_DEPTH]` as before.
    let max_depth = if depth == 0 {
        usize::MAX
    } else {
        depth.clamp(1, DEFAULT_MAX_DEPTH)
    };
    let effective_depth = if depth == 0 { None } else { Some(max_depth) };
    let type_filter = match type_filter {
        Some(t) => {
            if t.len() != 4 {
                bail!("record type '{}' must be a 4-character signature", t);
            }
            Some(t.to_uppercase())
        }
        None => None,
    };
    let type_matches = |record_type: &Option<String>| match &type_filter {
        Some(f) => record_type.as_deref() == Some(f.as_str()),
        None => true,
    };

    // Stable-dedup by FormID, preserving first-occurrence order. Do NOT
    // re-sort by form_id — caller order drives carrier display grouping and
    // BFS attribution priority.
    let mut seen_seed = HashSet::new();
    let seeds: Vec<(FormId, Vec<EntryPointRef>)> = seeds
        .iter()
        .filter(|(f, _)| seen_seed.insert(*f))
        .cloned()
        .collect();
    let seed_tags: HashMap<FormId, Vec<EntryPointRef>> =
        seeds.iter().map(|(f, tags)| (*f, tags.clone())).collect();
    let seed_ids: Vec<FormId> = seeds.iter().map(|(f, _)| *f).collect();

    // `seen` is both the dedup set for emitted referencer rows and the BFS
    // visited set. Seeding with every entry in `seeds` prevents a seed from
    // appearing as another seed's own referencer and breaks self-referential
    // cycles.
    let mut seen: HashSet<FormId> = seed_ids.iter().copied().collect();

    // Queue entries: (node_to_expand, originating_carrier, path).
    // In EP mode (`emit_seeds`), path[0] is the carrier itself; hop_depth
    // subtracts 1 so direct referencers still report depth 1. In Direct mode
    // the origin is None and the path starts empty (legacy behavior).
    let mut queue: VecDeque<(FormId, Option<FormId>, Vec<RefPathNode>)> = VecDeque::new();
    let mut seed_rows: Vec<RefRow> = Vec::new();

    if emit_seeds {
        for &seed in &seed_ids {
            let Some(row) = db.record_row_for(seed)? else {
                continue;
            };
            let seed_node = RefPathNode {
                form_id: row.form_id.clone(),
                record_type: row.record_type.clone(),
                editor_id: row.editor_id.clone(),
            };
            if type_matches(&row.record_type) {
                seed_rows.push(RefRow {
                    form_id: row.form_id,
                    record_type: row.record_type,
                    editor_id: row.editor_id,
                    name: row.name,
                    offset: row.offset,
                    depth: 0,
                    path: Vec::new(),
                    field_paths: None,
                    entry_points: seed_tags.get(&seed).cloned().unwrap_or_default(),
                });
            }
            // Type-filtered carriers still contribute a RefPathNode so their
            // descendants' path/VIA remain attributable.
            queue.push_back((seed, Some(seed), vec![seed_node]));
        }
    } else {
        for &seed in &seed_ids {
            queue.push_back((seed, None, Vec::new()));
        }
    }

    let carrier_total = seed_rows.len();
    let mut rows: Vec<RefRow> = Vec::new();
    // FormId → index into `rows` for equal-depth entry-point tag unions.
    let mut emitted: HashMap<FormId, usize> = HashMap::new();
    // Newly-discovered nodes at `max_depth` that were not expanded further —
    // the unexplored BFS frontier. See `RefList::depth_capped`.
    let mut frontier_remaining: usize = 0;

    while let Some((current, origin, path_here)) = queue.pop_front() {
        for r in db.referenced_by(current)? {
            let fid = match crate::parse_form_id_input(&r.form_id) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let hop_depth = path_here.len() + 1 - usize::from(emit_seeds);

            if !seen.insert(fid) {
                // Equal-depth re-reach: union entry_points onto the already-
                // emitted row without changing its path/VIA (first-reach wins
                // for the path). Deeper re-reaches and seed self-hits are
                // ignored as before.
                if let Some(&idx) = emitted.get(&fid)
                    && rows[idx].depth == hop_depth
                    && let Some(origin_fid) = origin
                {
                    merge_entry_points(
                        &mut rows[idx].entry_points,
                        seed_tags
                            .get(&origin_fid)
                            .map(|v| v.as_slice())
                            .unwrap_or(&[]),
                    );
                }
                continue;
            }

            let record_type = db.index.get_by_formid(fid).map(|m| m.signature.clone());

            if type_matches(&record_type) {
                let field_paths = if include_paths {
                    Some(db.formid_reference_paths(fid, current))
                } else {
                    None
                };
                let entry_points = origin
                    .and_then(|o| seed_tags.get(&o).cloned())
                    .unwrap_or_default();
                let idx = rows.len();
                rows.push(RefRow {
                    form_id: r.form_id.clone(),
                    record_type: record_type.clone(),
                    editor_id: r.editor_id.clone(),
                    name: r.name.clone(),
                    offset: r.offset,
                    depth: hop_depth,
                    path: path_here.clone(),
                    field_paths,
                    entry_points,
                });
                emitted.insert(fid, idx);
            }

            if hop_depth < max_depth {
                let mut new_path = path_here.clone();
                new_path.push(RefPathNode {
                    form_id: r.form_id,
                    record_type,
                    editor_id: r.editor_id,
                });
                queue.push_back((fid, origin, new_path));
            } else {
                frontier_remaining += 1;
            }
        }
    }

    let form_id_key = |r: &RefRow| {
        crate::parse_form_id_input(&r.form_id)
            .map(|f| f.0)
            .unwrap_or(u32::MAX)
    };
    match sort {
        RefSort::Formid => rows.sort_by_key(form_id_key),
        RefSort::Depth => rows.sort_by_key(|r| (r.depth, form_id_key(r))),
    }

    let mut all_rows = seed_rows;
    all_rows.append(&mut rows);

    let max_depth_seen = all_rows.iter().map(|r| r.depth).max().unwrap_or(0);
    let mut per_depth_totals = vec![0usize; max_depth_seen + 1];
    for r in &all_rows {
        per_depth_totals[r.depth] += 1;
    }

    let total = all_rows.len();
    let capped = limit > 0 && total > limit;
    let limited: Vec<RefRow> = if limit > 0 {
        all_rows.into_iter().take(limit).collect()
    } else {
        all_rows
    };
    let shown_max_depth = limited.iter().map(|r| r.depth).max().unwrap_or(0);

    Ok((
        limited,
        WalkStats {
            total,
            capped,
            carrier_total,
            requested_depth,
            effective_depth,
            depth_capped: frontier_remaining > 0,
            frontier_remaining,
            per_depth_totals,
            shown_max_depth,
        },
    ))
}

/// Non-row outcome of one [`referenced_by_walk`] call — the shared "how did
/// this walk go" facts both [`referenced_by_enriched`] and
/// [`referenced_by_enriched_multi`] copy into their own `RefList` (each adds
/// its own `target`/`entry_point_total` on top).
struct WalkStats {
    total: usize,
    capped: bool,
    /// Depth-0 rows that survived the type filter. 0 when `emit_seeds` is
    /// false.
    carrier_total: usize,
    requested_depth: usize,
    effective_depth: Option<usize>,
    depth_capped: bool,
    frontier_remaining: usize,
    per_depth_totals: Vec<usize>,
    shown_max_depth: usize,
}

/// Merge `incoming` into `dst` by entry-point id, keeping sorted+deduped.
fn merge_entry_points(dst: &mut Vec<EntryPointRef>, incoming: &[EntryPointRef]) {
    if incoming.is_empty() {
        return;
    }
    dst.extend(incoming.iter().cloned());
    dst.sort();
    dst.dedup_by(|a, b| a.id == b.id);
}

/// Default `--max-hops` for [`find_ref_path`] when the caller passes 0.
pub const DEFAULT_MAX_PATH_HOPS: usize = 12;

/// Node-visit ceiling for [`find_ref_path`]'s bidirectional search, combined
/// across both frontiers — a backstop against a disconnected/near-total
/// closure search still trying to enumerate hundreds of thousands of nodes
/// one at a time. When hit, the answer is genuinely "don't know" (see
/// [`RefPathResult::budget_exhausted`]), not "definitely no path".
const REF_PATH_NODE_BUDGET: usize = 200_000;

/// One node on a [`RefPathResult`] chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefPathHop {
    pub form_id: String,
    pub record_type: Option<String>,
    pub editor_id: Option<String>,
    pub name: Option<String>,
    /// JSON field path(s) inside *this* hop's own decoded body where it
    /// references the previous hop in the chain (the one closer to
    /// `RefPathResult::from`). `None` unless `paths` was requested; always
    /// absent on the first hop (`from` itself has no predecessor to point at).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_paths: Option<Vec<String>>,
}

/// Outcome of [`find_ref_path`] — a chain of reverse-reference hops
/// connecting `from` to `to` (`from` first, `to` last), or a report of why
/// none was found.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefPathResult {
    pub from: String,
    pub to: String,
    /// `Some(chain)` when a path was found within `max_hops`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<Vec<RefPathHop>>,
    /// `chain.len() - 1` (0 when `from == to`). `None` alongside `chain: None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hops: Option<usize>,
    /// True when [`REF_PATH_NODE_BUDGET`] was exhausted before a path was
    /// found or definitively ruled out within `max_hops` — the search result
    /// is inconclusive, not a confirmed "no path exists".
    pub budget_exhausted: bool,
}

/// Bidirectional BFS connecting `from` to `to` via the reverse-reference
/// relation — the same relation [`referenced_by_walk`] follows one-directionally
/// (`n` is connected to `m` when `m` is in `referenced_by(n)`, i.e. `m`'s body
/// contains `n`'s FormID).
///
/// A plain one-directional walk from `from` must expand *every* referencer at
/// every hop, which balloons on hub-heavy graphs (CELL/REFR nodes routinely
/// have thousands of referencers) long before it reaches a `to` with a small
/// number of hops between them. This function instead grows two frontiers at
/// once and expands whichever is smaller each round:
/// - **back** (from `from`): expands via [`Database::referenced_by`] — who
///   references this node. Exactly [`referenced_by_walk`]'s own direction.
/// - **fwd** (from `to`): expands via [`Database::outgoing_formids`] — what
///   this node's own body references. This discovers the *predecessor* of a
///   node in the same reverse-reference chain (if `X`'s body contains `Y`'s
///   FormID, then `X` is a referencer of `Y`, i.e. `X` is one hop closer to
///   `from` than `Y` is) — cheap because it's bounded by each node's own
///   field count, not by how many other records happen to reference it.
///
/// The two frontiers meet at a node `M` such that `from` reaches `M` via some
/// number of back-hops and `to` reaches `M` via some number of fwd-hops;
/// splicing those two partial chains at `M` yields a complete `from..=to`
/// chain in the same hop-by-hop shape [`referenced_by_walk`]'s `path`/`VIA`
/// already uses.
///
/// `max_hops` is the combined hop-count ceiling (0 = [`DEFAULT_MAX_PATH_HOPS`]).
pub fn find_ref_path(
    db: &mut Database,
    from: FormId,
    to: FormId,
    max_hops: usize,
    include_paths: bool,
) -> anyhow::Result<RefPathResult> {
    let max_hops = if max_hops == 0 {
        DEFAULT_MAX_PATH_HOPS
    } else {
        max_hops
    };

    if from == to {
        let hop = ref_path_hop(db, from, None)?;
        return Ok(RefPathResult {
            from: from.display(),
            to: to.display(),
            chain: Some(vec![hop]),
            hops: Some(0),
            budget_exhausted: false,
        });
    }

    // parent maps: back_parent[n] = the node one hop closer to `from` that
    // discovered `n` (n is a referencer of back_parent[n]); fwd_parent[n] =
    // the node one hop closer to `to` that discovered `n` (fwd_parent[n]'s
    // body references n, i.e. fwd_parent[n] is a referencer of n).
    let mut back_parent: HashMap<FormId, FormId> = HashMap::new();
    let mut fwd_parent: HashMap<FormId, FormId> = HashMap::new();
    back_parent.insert(from, from);
    fwd_parent.insert(to, to);
    let mut back_frontier = vec![from];
    let mut fwd_frontier = vec![to];
    let mut back_depth = 0;
    let mut fwd_depth = 0;
    // +2 for the two seeds themselves.
    let mut visited = 2;

    loop {
        if back_depth + fwd_depth >= max_hops {
            return Ok(RefPathResult {
                from: from.display(),
                to: to.display(),
                chain: None,
                hops: None,
                budget_exhausted: false,
            });
        }
        if back_frontier.is_empty() && fwd_frontier.is_empty() {
            return Ok(RefPathResult {
                from: from.display(),
                to: to.display(),
                chain: None,
                hops: None,
                budget_exhausted: false,
            });
        }

        // Expand whichever frontier is smaller (an empty frontier — the
        // other side ran dry — never gets picked over a non-empty one).
        let expand_back = !back_frontier.is_empty()
            && (fwd_frontier.is_empty() || back_frontier.len() <= fwd_frontier.len());

        let mut next_frontier = Vec::new();
        if expand_back {
            for &node in &back_frontier {
                for r in db.referenced_by(node)? {
                    let Ok(fid) = crate::parse_form_id_input(&r.form_id) else {
                        continue;
                    };
                    if back_parent.contains_key(&fid) {
                        continue;
                    }
                    back_parent.insert(fid, node);
                    visited += 1;
                    if fwd_parent.contains_key(&fid) {
                        return build_ref_path_result(
                            db,
                            from,
                            to,
                            fid,
                            &back_parent,
                            &fwd_parent,
                            include_paths,
                        );
                    }
                    if visited >= REF_PATH_NODE_BUDGET {
                        return Ok(RefPathResult {
                            from: from.display(),
                            to: to.display(),
                            chain: None,
                            hops: None,
                            budget_exhausted: true,
                        });
                    }
                    next_frontier.push(fid);
                }
            }
            back_frontier = next_frontier;
            back_depth += 1;
        } else {
            for &node in &fwd_frontier {
                for fid in db.outgoing_formids(node) {
                    if fwd_parent.contains_key(&fid) {
                        continue;
                    }
                    fwd_parent.insert(fid, node);
                    visited += 1;
                    if back_parent.contains_key(&fid) {
                        return build_ref_path_result(
                            db,
                            from,
                            to,
                            fid,
                            &back_parent,
                            &fwd_parent,
                            include_paths,
                        );
                    }
                    if visited >= REF_PATH_NODE_BUDGET {
                        return Ok(RefPathResult {
                            from: from.display(),
                            to: to.display(),
                            chain: None,
                            hops: None,
                            budget_exhausted: true,
                        });
                    }
                    next_frontier.push(fid);
                }
            }
            fwd_frontier = next_frontier;
            fwd_depth += 1;
        }
    }
}

/// Splice the two partial chains at meeting node `meet` into one complete
/// `from..=to` chain and decode each hop. See [`find_ref_path`] for the
/// direction convention `back_parent`/`fwd_parent` follow.
fn build_ref_path_result(
    db: &mut Database,
    from: FormId,
    to: FormId,
    meet: FormId,
    back_parent: &HashMap<FormId, FormId>,
    fwd_parent: &HashMap<FormId, FormId>,
    include_paths: bool,
) -> anyhow::Result<RefPathResult> {
    let mut nodes = vec![meet];
    let mut cur = meet;
    while cur != from {
        cur = back_parent[&cur];
        nodes.push(cur);
    }
    nodes.reverse(); // [from, ..., meet]

    let mut cur = meet;
    while cur != to {
        cur = fwd_parent[&cur];
        nodes.push(cur);
    }
    // nodes is now [from, ..., meet, ..., to].

    let hops = nodes.len() - 1;
    let mut chain = Vec::with_capacity(nodes.len());
    for (i, &n) in nodes.iter().enumerate() {
        let predecessor = if i > 0 { Some(nodes[i - 1]) } else { None };
        chain.push(ref_path_hop(db, n, predecessor.filter(|_| include_paths))?);
    }

    Ok(RefPathResult {
        from: from.display(),
        to: to.display(),
        chain: Some(chain),
        hops: Some(hops),
        budget_exhausted: false,
    })
}

/// Decode one chain node into a [`RefPathHop`], annotating `field_paths`
/// (this node's own reference to `predecessor`, its immediate neighbor
/// closer to `from`) when `predecessor` is `Some`.
fn ref_path_hop(
    db: &mut Database,
    node: FormId,
    predecessor: Option<FormId>,
) -> anyhow::Result<RefPathHop> {
    let row = db.record_row_for(node)?.unwrap_or_else(|| RecordRow {
        form_id: node.display(),
        record_type: None,
        editor_id: None,
        name: None,
        offset: 0,
    });
    let field_paths = predecessor.map(|p| db.formid_reference_paths(node, p));
    Ok(RefPathHop {
        form_id: row.form_id,
        record_type: row.record_type,
        editor_id: row.editor_id,
        name: row.name,
        field_paths,
    })
}

/// Seeds resolved from a [`RecordSel`] for [`Op::ReferencedBy`] — either a
/// single direct target (today's behavior, unchanged) or every PERK carrying
/// a matched entry point (see [`crate::EntryPointSpec`]).
enum RefSeeds {
    /// A resolved FormID/EditorID/Auto target. The target is only a BFS
    /// root — [`referenced_by_enriched`] never emits it as a row.
    Direct(FormId),
    /// One or more PERK entry-point carriers, each emitted as its own
    /// `depth: 0` row by [`referenced_by_enriched_multi`].
    Carriers {
        label: String,
        seeds: crate::EntryPointCarriers,
    },
}

/// Resolve a [`RecordSel`] to BFS seeds for [`Op::ReferencedBy`] specifically
/// — the one place [`RecordSel::EntryPoint`] is handled, and the one place
/// an EditorID lookup miss falls back to an entry-point name match (so a
/// bare positional token like `'Mod Percent Blocked'` — parsed as
/// [`RecordSel::Edid`] by [`RecordSel::from_input`], since it isn't
/// FormID-shaped — resolves without needing the explicit `--entry-point`
/// flag). Every other `Op` uses [`resolve_sel`], which does not have this
/// fallback and rejects `RecordSel::EntryPoint` outright.
fn resolve_ref_seeds(db: &mut Database, sel: &RecordSel) -> anyhow::Result<RefSeeds> {
    match sel {
        RecordSel::EntryPoint(token) => {
            let spec = EntryPointSpec::parse(token)?;
            let (label, seeds) = db.perks_by_entry_point(&spec)?;
            Ok(RefSeeds::Carriers { label, seeds })
        }
        RecordSel::Edid(edid) => match resolve_sel(db, sel) {
            Ok(fid) => Ok(RefSeeds::Direct(fid)),
            Err(edid_err) => {
                // Unlike the explicit `--entry-point` path above, a parse
                // failure here (e.g. `edid` happens to look hex-prefixed,
                // which `RecordSel::Edid` shouldn't produce in practice) is
                // just another way to *not* be an entry point — fold it into
                // the same "neither interpretation matched" message rather
                // than surfacing `EntryPointSpec::parse`'s FormID-specific
                // wording, which would be confusing here.
                let carriers = EntryPointSpec::parse(edid)
                    .ok()
                    .and_then(|spec| db.perks_by_entry_point(&spec).ok())
                    .filter(|(_, seeds)| !seeds.is_empty());
                match carriers {
                    Some((label, seeds)) => Ok(RefSeeds::Carriers { label, seeds }),
                    None => bail!(
                        "'{edid}' did not resolve as EditorID ({edid_err:#}) or as a \
                         PERK entry point (no carriers matched)"
                    ),
                }
            }
        },
        _ => Ok(RefSeeds::Direct(resolve_sel(db, sel)?)),
    }
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
    let mut all_sigs: Vec<String> = db
        .index
        .form_index
        .values()
        .map(|m| m.signature.clone())
        .collect::<HashSet<_>>()
        .into_iter()
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
            .into_iter()
            .map(|(_, m)| m.clone())
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
