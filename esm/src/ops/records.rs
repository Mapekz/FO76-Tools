//! Record lookups: header info, single and bulk decoded records, raw
//! subrecord dumps, and type listings, filters and searches.

use super::{NoArgs, RecordSel, resolve_sel};
use crate::{Database, FilterOp, FormId, RecordRow, ResolveDepth, SearchField};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) fn file_info(db: &Database, _args: &NoArgs) -> anyhow::Result<crate::reader::FileInfo> {
    db.file_info()
}

/// Decode one record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecordArgs {
    pub sel: RecordSel,
    pub depth: ResolveDepth,
}

pub(super) fn record(db: &Database, args: &RecordArgs) -> anyhow::Result<crate::RecordResult> {
    record_resolved(db, &args.sel, args.depth)
}

/// Decode several records in one request. Each selector is resolved and
/// decoded independently (see [`BulkRecordEntry`]): one bad FormID/EditorID
/// produces an error entry for that selector only, it does not fail the
/// whole call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecordBulkArgs {
    pub sels: Vec<RecordSel>,
    pub depth: ResolveDepth,
}

pub(super) fn record_bulk(
    db: &Database,
    args: &RecordBulkArgs,
) -> anyhow::Result<Vec<BulkRecordEntry>> {
    Ok(args
        .sels
        .iter()
        .map(|sel| bulk_record_entry(db, sel, args.depth))
        .collect())
}

/// Dump one record's subrecords as hex, undecoded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecordRawArgs {
    pub sel: RecordSel,
}

pub(super) fn record_raw(db: &Database, args: &RecordRawArgs) -> anyhow::Result<RawRecordView> {
    let form_id = resolve_sel(db, &args.sel)?;
    let rec = db
        .record_raw(form_id)
        .map_err(|e| explain_hardcoded_miss(form_id, e))?;
    Ok(raw_record_view(&rec))
}

/// One page of the records of type `sig`, with EditorIDs and display names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ListTypeRecordsArgs {
    pub sig: String,
    pub offset: usize,
    pub limit: usize,
}

pub(super) fn list_type_records(
    db: &Database,
    args: &ListTypeRecordsArgs,
) -> anyhow::Result<Vec<RecordRow>> {
    db.list_type_records(&args.sig, args.offset, args.limit)
}

/// Filter records of type `sig` by a predicate against their decoded field
/// body. See [`crate::Database::filter_type_records`] for path/operator semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct FilterTypeRecordsArgs {
    pub sig: String,
    pub path: Option<String>,
    // Named `filter_op` (not `op`) to avoid colliding with `Op`'s own
    // `#[serde(tag = "op")]` wire discriminant.
    pub filter_op: FilterOp,
    pub value: Option<String>,
    pub limit: usize,
}

pub(super) fn filter_type_records(
    db: &Database,
    args: &FilterTypeRecordsArgs,
) -> anyhow::Result<crate::FilterResult> {
    db.filter_type_records(
        &args.sig,
        args.path.as_deref(),
        args.filter_op,
        args.value.as_deref(),
        args.limit,
    )
}

/// Union of all dot-notation field paths observed across a decoded sample
/// of a type's records — see [`crate::Database::list_type_field_paths`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ListTypeFieldPathsArgs {
    pub sig: String,
}

pub(super) fn list_type_field_paths(
    db: &Database,
    args: &ListTypeFieldPathsArgs,
) -> anyhow::Result<Vec<String>> {
    db.list_type_field_paths(&args.sig)
}

/// Wildcard search over EditorIDs and/or display names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct SearchArgs {
    pub pattern: String,
    pub types: Vec<String>,
    pub field: SearchField,
    pub limit: usize,
}

pub(super) fn search(db: &Database, args: &SearchArgs) -> anyhow::Result<Vec<RecordRow>> {
    if args.pattern.is_empty() {
        bail!("search pattern must not be empty (use \"*\" to match all records)");
    }
    let types: Vec<String> = args
        .types
        .iter()
        .map(|t| {
            let up = t.to_uppercase();
            if up.len() != 4 {
                bail!("record type '{}' must be a 4-character signature", t);
            }
            Ok(up)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    db.search(&args.pattern, &types, args.field, args.limit)
}

pub(super) fn record_resolved(
    db: &Database,
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
/// called from `DatabaseResolver::stub`/`decode_full` (`src/database.rs`), which
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
pub(super) fn bulk_record_entry(
    db: &Database,
    sel: &RecordSel,
    depth: ResolveDepth,
) -> BulkRecordEntry {
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
