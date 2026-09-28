//! [`Database`]: an open ESM with its index, schema, strings and curves,
//! and the record lookups, listings, searches and filters over it.

use crate::decode::{DecodeContext, decode_record, decode_record_node};
use crate::filter::{FILTER_SCAN_CAP, collect_field_paths, effective_take, predicate_matches};
use crate::filter::{FilterOp, FilterResult};
use crate::formid::FormId;
use crate::index::Index;
use crate::reader::RecordMeta;
use crate::reader::{EsmFile, FileInfo, ParsedRecord, RecordHeaderInfo, edid_from_subrecords};
use crate::refs::seeds::{
    CarrierKind, CarrierTag, Carriers, EntryPointSpec, OmodPropertySpec, PropScope,
};
use crate::refs::seeds::{
    OmodPropertySel, entry_point_name_matches, enum_id_name, omod_property_name_matches,
};
use crate::schema::Schema;
use crate::strings::{Localization, StringKind};
use crate::tree::ChildRef;
use crate::tree::{GroupChild, GroupNode};
use crate::wildcard::wildcard_match;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// Primary interface to a Fallout 76 ESM file.
///
/// Holds a memory-mapped ESM, a FormID/EditorID index, the embedded field
/// schema, and an optional localization table loaded from the sibling BA2.
pub struct Database {
    pub(crate) esm: EsmFile,
    pub(crate) index: Index,
    pub(crate) schema: Schema,
    /// Whether the ESM's TES4 header has the Localized flag set. Stays
    /// `pub` (unlike its siblings below): the CLI's `diff` command
    /// (`src/bin/cli/diff.rs`) reads it across the binary/library crate boundary.
    pub is_localized: bool,
    /// Resolved string tables, if a localization BA2 was found or supplied.
    pub(crate) localization: Option<Localization>,
    /// Optional curve index built from Startup BA2. When present, FormID fields
    /// whose `valid_refs` includes `"CURV"` have their curve data inlined.
    pub(crate) curves: Option<crate::curves::CurveIndex>,
    /// Per-record-type memoized decode, populated lazily by `filter_type_records`
    /// and `list_type_field_paths`. In-memory only — never persisted, no
    /// CACHE_VERSION bump (these are ephemeral, rebuilt each time the Database
    /// is opened; `tree`/`GroupLabel`/`TreeRecordStub` in `tree.rs` are the only
    /// precedent for presentation-layer types, and this is analogous — it's not
    /// part of any of `Index`'s persisted rkyv sections at all).
    filter_cache: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<TypeSample>>>,
}

/// The memoized decode of one record type: its total record count and the
/// first [`FILTER_SCAN_CAP`] records, decoded.
struct TypeSample {
    total: usize,
    entries: Vec<FilterCacheEntry>,
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
/// is intentionally not derived for TS export; see esm-viewer/AGENTS.md.
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
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

impl Database {
    /// Open an ESM file or data folder.
    ///
    /// When `path` is a **directory**, it is scanned for exactly one `.esm`
    /// file; zero or multiple ESMs produce a clear error.  When `path` is a
    /// **file**, it is used directly. Either way the ESM path is canonicalized
    /// first ([`crate::discover::resolve_esm_path`]), so every caller that
    /// names the same file (through a folder, a relative path or a symlink)
    /// shares one `esm_cache/`, one build lock and one progress heartbeat.
    ///
    /// After locating the ESM, sibling sources are loaded automatically when
    /// present (missing sources are silently skipped; load failures are
    /// reported through `log::warn!` and do not abort):
    ///
    /// - **Strings**: loose `strings/<stem>_<locale>.{strings,…}` or
    ///   `<stem>_<locale>.strings` in the folder, else any
    ///   `*localization*.ba2` in the folder.
    /// - **Curves**: `misc/curvetables/json/` or `curvetables/json/` in the
    ///   folder, else any `*startup*.ba2` in the folder.
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let esm_path = crate::discover::resolve_esm_path(path.as_ref())?;
        let resolved = crate::discover::resolve_sources(&esm_path, "en")?;

        let esm = EsmFile::open(&resolved.esm)?;
        let index = Index::build(&esm)?;
        let schema = Schema::load_embedded().context("load embedded schema")?;

        let localization = resolved.strings.as_ref().and_then(|src| {
            Localization::cached(&esm.path, src, &resolved.locale, &resolved.loose_prefix)
                .inspect_err(|e| log::warn!("failed to load string tables: {e:#}"))
                .ok()
        });

        let curves = resolved.curves.as_ref().and_then(|src| {
            crate::curves::CurveIndex::cached(&esm, &index, src)
                .inspect_err(|e| log::warn!("failed to load curve tables: {e:#}"))
                .ok()
        });

        let is_localized = esm.file_info().map(|i| i.is_localized).unwrap_or(false);

        Ok(Database {
            esm,
            index,
            schema,
            is_localized,
            localization,
            curves,
            filter_cache: Default::default(),
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
    // mmap'd ESM (plus the schema, localization and curves for `xref`'s full
    // decode) that only `Database` holds. The three functions actually
    // reachable from index.rs are `build_edid_section`/`build_search_section`/
    // `build_xref_section` — this crate-internal data/orchestration split
    // keeps each section's construction logic colocated with its type in
    // `index.rs`, while the shared acquire/recheck/write/publish protocol
    // lives once, in `rkyvcache::map_or_build`.

    /// Build the lazy EditorID index on first call, writing it to its own
    /// `edid` section so a later call — in this process (the `is_mapped()`
    /// early-return below) or a fresh one (see [`Index::build`]'s doc
    /// comment) — reuses it rather than rebuilding. See
    /// [`crate::rkyvcache::map_or_build`] for the shared acquire/recheck/publish
    /// protocol this and its two siblings below delegate to.
    pub fn ensure_edid_index(&self) -> anyhow::Result<()> {
        let total = self.index.len() as u64;
        self.index.edid.get_or_build(|| {
            crate::rkyvcache::map_or_build(
                &self.esm.path,
                total,
                |_| true,
                |lease| crate::index::build_edid_section(&self.index, &self.esm, lease),
            )
        })?;
        Ok(())
    }

    /// Build the lazy search index (EditorID + name/description) on first
    /// call, then cache it to its own `search` section. See
    /// [`crate::rkyvcache::map_or_build`] for the acquire/recheck protocol this
    /// shares.
    pub fn ensure_search_index(&self) -> anyhow::Result<()> {
        let total = self.index.len() as u64;
        self.index.search.get_or_build(|| {
            crate::rkyvcache::map_or_build(
                &self.esm.path,
                total,
                |_| true,
                |lease| {
                    crate::index::build_search_section(
                        &self.index,
                        &self.esm,
                        self.is_localized,
                        lease,
                    )
                },
            )
        })?;
        Ok(())
    }

    /// Build the reverse-reference (`xref`) index on first call, then cache
    /// it to its own `xref` section. The most expensive of the three lazy
    /// builds (a full schema decode of every record). See
    /// [`crate::rkyvcache::map_or_build`] for the acquire/recheck protocol this
    /// shares.
    pub fn ensure_xref_index(&self) -> anyhow::Result<()> {
        let total = self.esm.data().len() as u64;
        self.index.xref.get_or_build(|| {
            crate::rkyvcache::map_or_build(
                &self.esm.path,
                total,
                |_| true,
                |lease| {
                    crate::index::build_xref_section(
                        &self.index,
                        &self.esm,
                        &self.schema,
                        self.is_localized,
                        self.localization.as_ref(),
                        self.curves.as_ref(),
                        lease,
                    )
                },
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
    // relying on a "call ensure first" convention, those three
    // Index accessors are `pub(crate)` and reachable only through the
    // wrappers below, each of which ensures internally — there is no way to
    // read a lazy index's data from within this crate without going through
    // a call that guarantees it is built first.

    /// Referencers of `form_id`, building the `xref` index first if needed.
    /// Never silently answers "no referencers" for an index that just
    /// hasn't been built yet — see the module note above.
    fn xref_lookup(&self, form_id: FormId) -> anyhow::Result<Vec<FormId>> {
        self.ensure_xref_index()?;
        Ok(self.index.get_xref(form_id))
    }

    /// Resolve an EditorID to its FormID, building the `edid` index first if
    /// needed. `Ok(None)` means "index built, EditorID genuinely absent" —
    /// never conflated with "index not built yet" the way a bare
    /// `Index::get_by_edid` call without an `ensure_edid_index` first would
    /// be.
    fn resolve_edid_indexed(&self, edid: &str) -> anyhow::Result<Option<FormId>> {
        self.ensure_edid_index()?;
        Ok(self.index.get_by_edid(edid))
    }

    /// Resolve a FormID to its [`RecordMeta`] via the full `Index` HashMap.
    pub(crate) fn get_formid_meta(&self, form_id: FormId) -> anyhow::Result<RecordMeta> {
        self.index
            .get_by_formid(form_id)
            .with_context(|| format!("FormID {} not found", form_id))
    }

    pub fn record_by_formid(&self, form_id: FormId) -> anyhow::Result<RecordResult> {
        let meta = self.get_formid_meta(form_id)?;
        self.record_at_meta_with_depth(&meta, crate::decode::ResolveDepth::None)
    }

    pub fn record_by_edid(&self, edid: &str) -> anyhow::Result<RecordResult> {
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
        &self,
        pattern: &str,
        types: &[String],
        field: SearchField,
        limit: usize,
    ) -> anyhow::Result<Vec<RecordRow>> {
        // No `assert!`-after-ensure needed here: `ensure_search_index`'s only
        // two return paths either propagate an `Err` or leave `search`
        // mapped — see its doc comment.
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
        &self,
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
    pub fn referenced_by(&self, form_id: FormId) -> anyhow::Result<Vec<RecordRow>> {
        let mut out = Vec::new();
        for referencer in self.referencers(form_id)? {
            if let Some(row) = self.record_row_for(referencer)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// The FormIDs of the indexed records that reference `form_id`, in
    /// [`Database::referenced_by`]'s order, without building their rows.
    pub(crate) fn referencers(&self, form_id: FormId) -> anyhow::Result<Vec<FormId>> {
        Ok(self
            .xref_lookup(form_id)?
            .into_iter()
            .filter(|&id| self.index.get_by_formid(id).is_some())
            .collect())
    }

    /// Build a [`RecordRow`] (resolved type/EditorID/name) for an arbitrary
    /// FormID already present in the index — `None` if it isn't. Shared by
    /// [`Database::referenced_by`] (each referencer row) and
    /// [`refs::referenced_by_enriched`]'s carrier/seed rows.
    pub(crate) fn record_row_for(&self, form_id: FormId) -> anyhow::Result<Option<RecordRow>> {
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
                    result.push(GroupChild::Record(crate::tree::TreeRecordStub {
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
    pub fn record_stub_at(&self, offset: u64) -> anyhow::Result<crate::tree::TreeRecordStub> {
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

        Ok(crate::tree::TreeRecordStub {
            form_id: FormId(hdr.form_id).display(),
            editor_id,
            record_type: hdr.signature.to_string(),
            offset,
        })
    }

    /// Decode an already-parsed record at the given resolution depth. Shared
    /// by `record_at_meta_with_depth` and `DatabaseResolver::leaf_inline`
    /// (the latter always at `ResolveDepth::None`, since a value-bearing
    /// leaf's own fields never need further reference-following) so a leaf
    /// lookup never re-parses a record `DatabaseResolver::stub` already
    /// parsed.
    fn decode_parsed(
        &self,
        parsed: &crate::reader::ParsedRecord,
        depth: crate::decode::ResolveDepth,
    ) -> Value {
        self.decode_parsed_with(parsed, depth, |_| {})
    }

    /// [`Self::decode_parsed`], handing the typed tree to `inspect` before it
    /// is rendered.
    fn decode_parsed_with(
        &self,
        parsed: &crate::reader::ParsedRecord,
        depth: crate::decode::ResolveDepth,
        inspect: impl FnOnce(&crate::decode::node::Node),
    ) -> Value {
        let resolver: Option<DatabaseResolver<'_>> = if depth != crate::decode::ResolveDepth::None {
            Some(DatabaseResolver::new(self, 2))
        } else {
            None
        };
        let ctx = self
            .decode_env(
                depth,
                resolver
                    .as_ref()
                    .map(|r| r as &dyn crate::decode::FormIdRefResolver),
            )
            .for_record(parsed.header.form_version);
        let node = self.node_parsed(&ctx, parsed);
        inspect(&node);
        node.into_json(&ctx)
    }

    /// [`Self::record_at_meta_with_depth`] plus every FormID the record's
    /// schema-typed fields reference ([`FormId::is_null`] excluded), in tree order
    /// with duplicates removed.
    pub(crate) fn record_at_meta_with_refs(
        &self,
        meta: &crate::reader::RecordMeta,
        depth: crate::decode::ResolveDepth,
    ) -> anyhow::Result<(RecordResult, Vec<FormId>)> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&parsed.subrecords);
        let mut refs = Vec::new();
        let fields = self.decode_parsed_with(&parsed, depth, |node| {
            let mut seen = std::collections::HashSet::new();
            node.for_each_formid(&mut |id| {
                if !id.is_null() && seen.insert(id) {
                    refs.push(id);
                }
            });
        });
        Ok((
            RecordResult {
                header: parsed.header,
                editor_id,
                fields,
            },
            refs,
        ))
    }

    /// Decode the record at `meta` for the analysis layers: rendered at
    /// [`crate::ResolveDepth::Stub`] with its FormID references kept typed
    /// (see [`crate::Resolved`]). Returns the parsed header and EditorID
    /// alongside.
    pub(crate) fn record_resolved_at_meta(
        &self,
        meta: &crate::reader::RecordMeta,
    ) -> anyhow::Result<(
        crate::reader::RecordHeaderInfo,
        Option<String>,
        crate::Resolved,
    )> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&parsed.subrecords);
        let resolver = DatabaseResolver::new(self, 2);
        let ctx = self
            .decode_env(crate::decode::ResolveDepth::Stub, Some(&resolver))
            .for_record(parsed.header.form_version);
        let fields = self.node_parsed(&ctx, &parsed).into_resolved(&ctx);
        Ok((parsed.header, editor_id, fields))
    }

    /// [`Self::record_at_meta_with_depth`] plus the typed tree its fields
    /// render.
    pub(crate) fn record_at_meta_with_node(
        &self,
        meta: &crate::reader::RecordMeta,
        depth: crate::decode::ResolveDepth,
    ) -> anyhow::Result<(RecordResult, crate::decode::node::Node)> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&parsed.subrecords);
        let mut tree = None;
        let fields = self.decode_parsed_with(&parsed, depth, |node| tree = Some(node.clone()));
        let tree = tree.context("decoder produced no tree")?;
        Ok((
            RecordResult {
                header: parsed.header,
                editor_id,
                fields,
            },
            tree,
        ))
    }

    /// Decode an already-parsed record into its typed tree under `ctx`.
    fn node_parsed(
        &self,
        ctx: &DecodeContext<'_>,
        parsed: &crate::reader::ParsedRecord,
    ) -> crate::decode::node::Node {
        let mut node = decode_record_node(ctx, &parsed.header.signature, &parsed.subrecords);
        // CURV records only carry a path to an external curve-points JSON file
        // (see schema `JSON File Path[/2]`) — inline the parsed points too, so a
        // plain `get` on a CURV record doesn't require a second out-of-band read
        // of that file. Referencing records already get this via `render_formid`
        // (`decode/mod.rs`); this covers the CURV record itself.
        if parsed.header.signature == "CURV"
            && let Some(curve) = self
                .curves
                .as_ref()
                .and_then(|curves| curves.get(parsed.header.form_id))
            && let crate::decode::node::Node::Struct(fields) = &mut node
        {
            fields.insert("Curve".to_string(), crate::decode::curve_points_node(curve));
        }
        node
    }

    /// Decode the record at `meta` into its typed tree, with no FormID
    /// resolver. Returns the parsed header and EditorID alongside.
    pub(crate) fn record_node_at_meta(
        &self,
        meta: &crate::reader::RecordMeta,
    ) -> anyhow::Result<(crate::reader::ParsedRecord, crate::decode::node::Node)> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let ctx = self.plain_ctx(parsed.header.form_version);
        let node = self.node_parsed(&ctx, &parsed);
        Ok((parsed, node))
    }

    /// This database's decode environment, rendering FormIDs at `depth`
    /// through `resolver`.
    pub(crate) fn decode_env<'a>(
        &'a self,
        depth: crate::decode::ResolveDepth,
        resolver: Option<&'a dyn crate::decode::FormIdRefResolver>,
    ) -> crate::decode::DecodeEnv<'a> {
        crate::decode::DecodeEnv {
            schema: &self.schema,
            is_localized: self.is_localized,
            localization: self.localization.as_ref(),
            curves: self.curves.as_ref(),
            types: Some(&self.index),
            resolve_depth: depth,
            resolver,
        }
    }

    /// A decode context for this database with no FormID resolver
    /// (`ResolveDepth::None`).
    pub(crate) fn plain_ctx(&self, form_version: u16) -> DecodeContext<'_> {
        self.decode_env(crate::decode::ResolveDepth::None, None)
            .for_record(form_version)
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
        let fields = self.decode_parsed(&parsed, depth);
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
    /// Only resolves against real ESM records — unlike `ops::resolve_sel`
    /// (the path every serving surface uses), this does not fall back to
    /// `crate::hardcoded`'s engine-hardcoded EditorID table. Prefer
    /// `ops::resolve_sel` + [`Self::record_by_formid_resolved`] for that
    /// broader precedence-aware resolution; this method stays as a narrower
    /// public building block rather than duplicating that fallback here.
    pub fn record_by_edid_resolved(
        &self,
        edid: &str,
        depth: crate::decode::ResolveDepth,
    ) -> anyhow::Result<RecordResult> {
        let form_id = self
            .resolve_edid_indexed(edid)?
            .with_context(|| format!("EditorID '{}' not found", edid))?;
        self.record_by_formid_resolved(form_id, depth)
    }

    /// Decode `referencer` and return every path within its body where a
    /// FormID field references `target`. Backs `refs --paths`: best-effort —
    /// returns an empty vec if `referencer` can't be located or decoded, or if
    /// no FormID field holds `target`.
    pub fn formid_reference_paths(&self, referencer: FormId, target: FormId) -> Vec<String> {
        let Some(meta) = self.index.get_by_formid(referencer) else {
            return Vec::new();
        };
        let Ok((_, node)) = self.record_node_at_meta(&meta) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        node.formid_paths(target, "", &mut out);
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
        let Ok((_, tree)) = self.record_node_at_meta(&meta) else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        tree.for_each_formid(&mut |f| {
            if f != node && !f.is_null() && seen.insert(f) {
                out.push(f);
            }
        });
        out
    }

    /// The memoized decode of record type `sig` (already uppercased): decoded
    /// on first access, at most [`FILTER_SCAN_CAP`] records, then shared by
    /// [`Database::filter_type_records`], [`Database::list_type_field_paths`],
    /// [`Database::perks_by_entry_point`] and [`Database::omods_by_property`].
    fn type_sample(&self, sig: &str) -> anyhow::Result<std::sync::Arc<TypeSample>> {
        if let Some(sample) = self.lock_filter_cache().get(sig) {
            return Ok(sample.clone());
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
            let ctx = self.plain_ctx(parsed.header.form_version);
            let fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
            entries.push(FilterCacheEntry {
                form_id,
                editor_id,
                offset,
                fields,
            });
        }

        let sample = std::sync::Arc::new(TypeSample { total, entries });
        Ok(self
            .lock_filter_cache()
            .entry(sig.to_string())
            .or_insert(sample)
            .clone())
    }

    fn lock_filter_cache(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, std::sync::Arc<TypeSample>>>
    {
        self.filter_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Filter records of type `sig` by a predicate against their decoded
    /// `fields` JSON body. See [`FilterOp`] and [`predicate_matches`] for the
    /// path syntax and operator semantics.
    ///
    /// `path` of `None`/empty deep-scans every value in the record. `limit`
    /// of `0` means no limit. Decoding itself is capped at [`FILTER_SCAN_CAP`]
    /// records per type — see [`FilterResult::scan_capped`].
    pub fn filter_type_records(
        &self,
        sig: &str,
        path: Option<&str>,
        op: FilterOp,
        value: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<FilterResult> {
        let sig = sig.to_uppercase();
        let sample = self.type_sample(&sig)?;
        let (total, entries) = (sample.total, &sample.entries);
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
    pub fn list_type_field_paths(&self, sig: &str) -> anyhow::Result<Vec<String>> {
        const MAX_PATHS: usize = 5000;
        let sig = sig.to_uppercase();
        let sample = self.type_sample(&sig)?;
        let entries = &sample.entries;

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
    /// Reuses the `type_sample("PERK")` memoized decode (shared with
    /// [`Database::filter_type_records`]), so repeat lookups after the first
    /// are effectively free.
    ///
    /// Returns `(label, seeds)`: `label` is a human-readable description of
    /// what matched — e.g. `"entry point 39 (Mod Percent Blocked)"` or
    /// `"entry point 'Mod VATS*' (14 matched: 43 Mod VATS Attack Damage, …)"`
    /// — meant for [`ops::RefList::target`]; `seeds` are `(FormId, tags)`
    /// pairs tagging each carrier with the entry points it matched.
    pub fn perks_by_entry_point(
        &self,
        spec: &EntryPointSpec,
    ) -> anyhow::Result<(String, Carriers)> {
        let sample = self.type_sample("PERK")?;
        let entries = &sample.entries;

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
    /// Reuses the `type_sample("OMOD")` memoized decode (shared with
    /// [`Database::filter_type_records`]), so repeat lookups after the first
    /// are effectively free.
    ///
    /// Returns `(label, seeds)`: `label` is a human-readable description of
    /// what matched — e.g. `"OMOD property weap:0 (Speed)"` or
    /// `"OMOD property 'Enchantments' (3 matched: weap:65 Enchantments, \
    /// armo:0 Enchantments, npc:3 Enchantments)"` — meant for
    /// [`ops::RefList::target`]; `seeds` are `(FormId, tags)` pairs tagging
    /// each carrier with the properties it matched.
    pub fn omods_by_property(&self, spec: &OmodPropertySpec) -> anyhow::Result<(String, Carriers)> {
        let sample = self.type_sample("OMOD")?;
        let entries = &sample.entries;

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

    /// `stub(id)` serialized to JSON, with a value-bearing-leaf inline
    /// applied on top when one exists. Shared by `decode_full`'s two
    /// stub-shaped fallbacks (depth limit, index miss) so `--resolve full`
    /// is never *less* informative than `--resolve stub` for a GLOB/CURV
    /// reference reached at hop >= 1.
    fn stub_or_leaf_value(&self, id: FormId) -> Option<Value> {
        use crate::decode::FormIdRefResolver;
        let stub = self.stub(id)?;
        if let Some(inlined) = self.leaf_inline(id, &stub.record_type) {
            return Some(inlined);
        }
        serde_json::to_value(&stub).ok()
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

    fn leaf_inline(&self, id: FormId, record_type: &str) -> Option<Value> {
        match crate::decode::leaf_values::lookup(record_type)? {
            crate::decode::leaf_values::InlineSource::CurveIndex => {
                let curve = self.db.curves.as_ref()?.get(id)?;
                Some(crate::decode::curve_inline(id, curve))
            }
            crate::decode::leaf_values::InlineSource::Fields(keys) => {
                // Decode the target at `ResolveDepth::None`: no resolver, so
                // no recursion and no depth-budget interaction. `stub()`
                // already parsed this record's bytes (header + subrecord
                // split) to build the plain stub — this reuses that parse,
                // paying only the incremental cost of a schema decode over a
                // record with at most a handful of members (see the
                // `leaf_values` module doc for the measured cost).
                let meta = self.db.get_formid_meta(id).ok()?;
                let parsed = self.db.esm.parse_record_at(meta.offset).ok()?;
                let fields = self
                    .db
                    .decode_parsed(&parsed, crate::decode::ResolveDepth::None);
                let fields_obj = fields.as_object()?;
                let mut map = crate::decode::stub_map(&self.stub(id)?);
                let mut any = false;
                for key in *keys {
                    if let Some(v) = fields_obj.get(*key) {
                        map.insert((*key).to_string(), v.clone());
                        any = true;
                    }
                }
                any.then(|| Value::Object(map))
            }
        }
    }

    fn decode_full(&self, id: FormId) -> Option<Value> {
        if self.remaining == 0 {
            // At depth limit — fall back to stub
            return self.stub_or_leaf_value(id);
        }
        let Ok(meta) = self.db.get_formid_meta(id) else {
            // Index miss — fall back to the hardcoded-form table, same as `stub`.
            // There's no further record to recurse into, so this returns the
            // same stub-shaped JSON `stub()` would (matching the existing
            // depth-limit fallback above).
            return self.stub_or_leaf_value(id);
        };
        let parsed = self.db.esm.parse_record_at(meta.offset).ok()?;
        let editor_id = crate::reader::edid_from_subrecords(&parsed.subrecords);
        let record_type = parsed.header.signature.clone();
        // Build a nested DecodeContext with depth decremented
        let nested_resolver = DatabaseResolver {
            db: self.db,
            remaining: self.remaining - 1,
        };
        let ctx = self
            .db
            .decode_env(crate::decode::ResolveDepth::Full, Some(&nested_resolver))
            .for_record(parsed.header.form_version);
        let fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
        Some(serde_json::json!({
            "formid": id.display(),
            "editor_id": editor_id,
            "record_type": record_type,
            "fields": fields,
        }))
    }
}
