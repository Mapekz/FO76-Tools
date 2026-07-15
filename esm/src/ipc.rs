//! Wire types and the canonical `dispatch` function shared by CLI, daemon, and N-API.

use crate::diff::{diff_databases_with, DiffOptions};
use crate::registry::Registry;
use crate::{Database, FilterOp, FormId, ResolveDepth, SearchField};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;

/// Default maximum recursion depth for the reverse-reference walk.
pub const DEFAULT_MAX_DEPTH: usize = 6;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Referenced-by result with total count and optional cap flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefList {
    pub target: String,
    pub rows: Vec<RefRow>,
    pub total: usize,
    pub capped: bool,
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
            let same_db = key_a == key_b || std::sync::Arc::ptr_eq(&arc_a, &arc_b);
            if same_db {
                let db = arc_a.lock().unwrap();
                // same_db means no added records — enrich_added_sources is a no-op.
                return diff_locked(&db, &db, options, record_type);
            }
            // Lock in key order (deadlock-safe).
            if key_a < key_b {
                let db_a = arc_a.lock().unwrap();
                let db_b = arc_b.lock().unwrap();
                diff_locked(&db_a, &db_b, options, record_type)
            } else {
                let db_b = arc_b.lock().unwrap();
                let db_a = arc_a.lock().unwrap();
                diff_locked(&db_a, &db_b, options, record_type)
            }
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
        } => {
            let target = resolve_sel(db, sel)?;
            let ref_list =
                referenced_by_enriched(db, target, *depth, *limit, type_filter.as_deref(), *paths)?;
            Ok(serde_json::to_value(&ref_list)?)
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
            if let Some(fid) = formid_attempt {
                if db.get_formid_meta(fid).is_ok() {
                    return Ok(fid);
                }
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

/// Post-lock part of a database diff: run [`diff_databases_with`] and apply
/// the optional record-type filter, once both `Database` locks are already
/// held. Shared by `dispatch_inner`'s `Diff` arm (which locks two handles via
/// a [`Registry`]'s key ordering) and the N-API binding's `diff` method
/// (which locks two separate `Arc<Mutex<Database>>` handles ordered by raw
/// pointer address) — each keeps its own lock-acquisition code, since only
/// the registry path has a `Registry` to source canonical keys from.
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
/// `[1, DEFAULT_MAX_DEPTH]`; passing 0 is treated as 1.
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
) -> anyhow::Result<RefList> {
    let max_depth = depth.clamp(1, DEFAULT_MAX_DEPTH);
    let type_filter = match type_filter {
        Some(t) => {
            if t.len() != 4 {
                bail!("record type '{}' must be a 4-character signature", t);
            }
            Some(t.to_uppercase())
        }
        None => None,
    };

    // `seen` is both the dedup set for emitted results and the BFS visited set.
    // Seeding with `target` prevents the target itself from appearing as its
    // own result and breaks any self-referential cycles.
    let mut seen: HashSet<FormId> = HashSet::new();
    seen.insert(target);

    // Queue entries: (node_to_expand, path_of_intermediate_hops_leading_to_it).
    // The path does NOT include the target or the node itself; it holds the
    // nodes that were emitted at earlier depths on the way here.
    let mut queue: VecDeque<(FormId, Vec<RefPathNode>)> = VecDeque::new();
    queue.push_back((target, Vec::new()));

    let mut rows: Vec<RefRow> = Vec::new();

    while let Some((current, path_here)) = queue.pop_front() {
        for r in db.referenced_by(current)? {
            let fid = match crate::parse_form_id_input(&r.form_id) {
                Ok(f) => f,
                Err(_) => continue,
            };
            if !seen.insert(fid) {
                continue; // already emitted via a shorter or equal-length path
            }

            let record_type = db.index.get_by_formid(fid).map(|m| m.signature.clone());
            let hop_depth = path_here.len() + 1;

            let type_matches = match &type_filter {
                Some(f) => record_type.as_deref() == Some(f.as_str()),
                None => true,
            };

            if type_matches {
                let field_paths = if include_paths {
                    Some(db.formid_reference_paths(fid, current))
                } else {
                    None
                };
                rows.push(RefRow {
                    form_id: r.form_id.clone(),
                    record_type: record_type.clone(),
                    editor_id: r.editor_id.clone(),
                    name: r.name.clone(),
                    offset: r.offset,
                    depth: hop_depth,
                    path: path_here.clone(),
                    field_paths,
                });
            }

            if hop_depth < max_depth {
                let mut new_path = path_here.clone();
                new_path.push(RefPathNode {
                    form_id: r.form_id,
                    record_type,
                    editor_id: r.editor_id,
                });
                queue.push_back((fid, new_path));
            }
        }
    }

    rows.sort_by_key(|r| {
        crate::parse_form_id_input(&r.form_id)
            .map(|f| f.0)
            .unwrap_or(u32::MAX)
    });

    let total = rows.len();
    let capped = limit > 0 && total > limit;
    let limited: Vec<RefRow> = if limit > 0 {
        rows.into_iter().take(limit).collect()
    } else {
        rows
    };

    Ok(RefList {
        target: target.display(),
        rows: limited,
        total,
        capped,
    })
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
