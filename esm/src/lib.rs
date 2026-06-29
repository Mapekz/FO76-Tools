pub mod ba2;
pub mod backend;
pub mod compress;
pub mod ctda;
pub mod curves;
pub mod decode;
pub mod diff;
pub mod discover;
pub mod format;
pub mod formid;
pub mod index;
pub mod ipc;
pub mod mindex;
pub mod reader;
pub mod registry;
pub mod schema;
pub mod strings;
pub mod tree;
pub mod wildcard;

use crate::decode::{decode_record, DecodeContext};
use crate::formid::parse_formid;
use crate::index::Index;
use crate::reader::{edid_from_subrecords, EsmFile, FileInfo, ParsedRecord, RecordHeaderInfo};
use crate::schema::Schema;
use crate::strings::{Localization, StringKind};
use crate::tree::ChildRef;
use crate::wildcard::wildcard_match;
use anyhow::{bail, Context};
pub use decode::{FormIdRefResolver, FormIdStub, ResolveDepth};
pub use diff::{apply_type_filter, DiffResult, RecordDiff, RecordStub, RefName};
pub use formid::FormId;
pub use index::SearchMeta;
pub use ipc::{
    CoverageReport, Markers, Op, RawRecordView, RawSubrecordView, RefList, RefPathNode, RefRow,
    Request, Response,
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
    pub esm: EsmFile,
    pub index: Index,
    pub schema: Schema,
    /// Whether the ESM's TES4 header has the Localized flag set.
    pub is_localized: bool,
    /// Resolved string tables, if a localization BA2 was found or supplied.
    pub localization: Option<Localization>,
    /// Optional curve index built from Startup BA2. When present, FormID fields
    /// whose `valid_refs` includes `"CURV"` have their curve data inlined.
    pub curves: Option<crate::curves::CurveIndex>,
    /// Optional zero-copy mmap'd form index, set only in [`Database::open_lite`].
    ///
    /// When present, [`record_by_formid`](Self::record_by_formid) and related
    /// methods use binary search over this table instead of the HashMap in
    /// `index.form_index`.  The full index (`index`) is empty in this mode.
    pub mmap_index: Option<crate::mindex::MmapFormIndex>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordResult {
    pub header: RecordHeaderInfo,
    pub editor_id: Option<String>,
    pub fields: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEntry {
    pub form_id: String,
    pub editor_id: Option<String>,
    pub full_lstring_id: Option<String>,
}

/// A tree row combining FormID, record type, EditorID, and resolved translated name.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
                match Localization::from_ba2(ba2_path, &resolved.locale) {
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
            mmap_index: None,
        })
    }

    /// Open an ESM file or folder in **lite mode**: mmap the ESM and load the
    /// compact `.esm.midx` binary index (building it from an ESM walk if absent).
    ///
    /// When `path` is a directory, it is scanned for exactly one `.esm` file.
    ///
    /// Compared to [`Database::open`], this skips the ~280 MiB `.esm.idx`
    /// bincode load entirely — startup is typically sub-second even cold.
    /// The trade-off is that only FormID-based lookups (`record_by_formid`,
    /// `record_raw`, `record_by_formid_resolved`) are supported.  Operations
    /// that require the full index (EditorID lookup, `list`, `search`, `refs`,
    /// `tree`) return an error directing the caller to use the warm daemon.
    ///
    /// Use `--mmap-index` on the CLI or `ESM_MMAP_INDEX=1` to activate this
    /// path.
    pub fn open_lite(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let esm_path = crate::discover::resolve_sources(path, "en")?.esm;
        let esm = EsmFile::open(&esm_path)?;
        let mmap_index = crate::mindex::MmapFormIndex::load_or_build(&esm)
            .with_context(|| format!("build mmap index for {}", esm_path.display()))?;
        let schema = Schema::load_embedded().context("load embedded schema")?;
        let is_localized = esm.file_info().map(|i| i.is_localized).unwrap_or(false);
        Ok(Database {
            esm,
            index: Index::empty(esm_path),
            schema,
            is_localized,
            localization: None,
            curves: None,
            mmap_index: Some(mmap_index),
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

    /// Resolve a FormID to its [`RecordMeta`], consulting the mmap index in
    /// lite mode or the full HashMap in normal mode.
    fn get_formid_meta(&self, form_id: FormId) -> anyhow::Result<RecordMeta> {
        if let Some(ref midx) = self.mmap_index {
            midx.get_by_formid(form_id)
                .with_context(|| format!("FormID {} not found", form_id))
        } else {
            self.index
                .get_by_formid(form_id)
                .cloned()
                .with_context(|| format!("FormID {} not found", form_id))
        }
    }

    /// Bail with a helpful message when a full-index operation is attempted in
    /// lite mode (opened via [`Database::open_lite`] / `--mmap-index`).
    fn check_not_lite(&self, op: &str) -> anyhow::Result<()> {
        if self.mmap_index.is_some() {
            bail!(
                "{op} requires the full index; start the warm daemon \
                 (`esm daemon start`) or remove --mmap-index"
            );
        }
        Ok(())
    }

    pub fn record_by_formid(&mut self, form_id: FormId) -> anyhow::Result<RecordResult> {
        let meta = self.get_formid_meta(form_id)?;
        self.record_at_meta(&meta)
    }

    pub fn record_by_edid(&mut self, edid: &str) -> anyhow::Result<RecordResult> {
        self.check_not_lite("EditorID lookup")?;
        self.index.ensure_edid_index(&self.esm)?;
        let form_id = self
            .index
            .get_by_edid(edid)
            .with_context(|| format!("EditorID '{}' not found", edid))?;
        self.record_by_formid(form_id)
    }

    pub fn list_by_type(&self, sig: &str, limit: usize) -> anyhow::Result<Vec<ListEntry>> {
        self.check_not_lite("list_by_type")?;
        if sig.len() != 4 {
            bail!("record type must be a 4-character signature");
        }
        let records = self.index.records_by_type(sig);
        let mut out = Vec::new();
        for (form_id, meta) in records.into_iter().take(limit) {
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
        self.check_not_lite("search")?;
        self.index
            .ensure_search_index(&self.esm, self.is_localized)?;

        let type_filter: Option<HashSet<&str>> = if types.is_empty() {
            None
        } else {
            Some(types.iter().map(|s| s.as_str()).collect())
        };

        let search_index = self
            .index
            .search_index()
            .expect("search_index must be populated after ensure_search_index");

        // Collect matching entries. HashMap order is nondeterministic, so we
        // accumulate into a Vec and sort by FormID before returning.
        let mut matches: Vec<(u32, RecordRow)> = Vec::new();

        for (form_id, smeta) in search_index {
            // Type filter.
            if let Some(ref filter) = type_filter {
                let sig = self
                    .index
                    .get_by_formid(*form_id)
                    .map(|m| m.signature.as_str())
                    .unwrap_or("");
                if !filter.contains(sig) {
                    continue;
                }
            }

            // Resolve display name: lstring ID for localized ESMs,
            // inline text for non-localized ESMs.
            let name: Option<String> = smeta
                .full_id
                .and_then(|id| {
                    self.localization
                        .as_ref()
                        .and_then(|l| l.lookup(StringKind::Strings, id))
                        .map(|s| s.to_owned())
                })
                .or_else(|| smeta.full_text.clone());

            // Resolve description: lstring ID for localized ESMs,
            // inline text for non-localized ESMs.
            let desc: Option<String> = smeta
                .desc_id
                .and_then(|id| {
                    self.localization
                        .as_ref()
                        .and_then(|l| l.lookup(StringKind::Strings, id))
                        .map(|s| s.to_owned())
                })
                .or_else(|| smeta.desc_text.clone());

            // Check if this record matches the pattern for the requested field.
            let matched = match field {
                SearchField::Edid => smeta
                    .editor_id
                    .as_deref()
                    .map(|e| wildcard_match(pattern, e))
                    .unwrap_or(false),
                SearchField::Name => {
                    name.as_deref()
                        .map(|n| wildcard_match(pattern, n))
                        .unwrap_or(false)
                        || desc
                            .as_deref()
                            .map(|d| wildcard_match(pattern, d))
                            .unwrap_or(false)
                }
                SearchField::Both => {
                    smeta
                        .editor_id
                        .as_deref()
                        .map(|e| wildcard_match(pattern, e))
                        .unwrap_or(false)
                        || name
                            .as_deref()
                            .map(|n| wildcard_match(pattern, n))
                            .unwrap_or(false)
                        || desc
                            .as_deref()
                            .map(|d| wildcard_match(pattern, d))
                            .unwrap_or(false)
                }
            };

            if !matched {
                continue;
            }

            let meta = self.index.get_by_formid(*form_id);
            let offset = meta.map(|m| m.offset).unwrap_or(0);
            let record_type = meta.map(|m| m.signature.clone());

            matches.push((
                form_id.raw(),
                RecordRow {
                    form_id: form_id.display(),
                    record_type,
                    editor_id: smeta.editor_id.clone(),
                    name,
                    offset,
                },
            ));
        }

        matches.sort_by_key(|(raw, _)| *raw);

        let mut out: Vec<RecordRow> = matches.into_iter().map(|(_, row)| row).collect();
        if limit > 0 && out.len() > limit {
            out.truncate(limit);
        }
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
        self.check_not_lite("list_type_records")?;
        if sig.len() != 4 {
            bail!("record type must be a 4-character signature");
        }
        let records: Vec<(FormId, u64, String)> = self
            .index
            .records_by_type(sig)
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(fid, meta)| (fid, meta.offset, meta.signature.clone()))
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
    /// persisted to the `.esm.idx` cache so subsequent calls are instant.
    pub fn referenced_by(&mut self, form_id: FormId) -> anyhow::Result<Vec<RecordRow>> {
        self.check_not_lite("referenced_by")?;
        self.index.ensure_xref_index(
            &self.esm,
            &self.schema,
            self.is_localized,
            self.localization.as_ref(),
            self.curves.as_ref(),
        )?;
        let referencers: Vec<FormId> = self.index.get_xref(form_id).to_vec();
        let mut out = Vec::new();
        for referencer in referencers {
            let meta = match self.index.get_by_formid(referencer) {
                Some(m) => m.clone(),
                None => continue,
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
            out.push(RecordRow {
                form_id: referencer.display(),
                record_type: Some(meta.signature.clone()),
                editor_id,
                name,
                offset: meta.offset,
            });
        }
        Ok(out)
    }

    pub fn record_raw(&self, form_id: FormId) -> anyhow::Result<ParsedRecord> {
        let meta = self.get_formid_meta(form_id)?;
        self.esm.parse_record_at(meta.offset)
    }

    /// List all top-level (group_type == 0) GRUPs in file order.
    pub fn list_groups(&self) -> Vec<GroupNode> {
        self.index
            .tree
            .roots
            .iter()
            .map(|&idx| self.index.tree.group_node(idx))
            .collect()
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
        let group_idx = self.index.tree.roots.iter().copied().find(|&idx| {
            let entry = &self.index.tree.groups[idx];
            matches!(
                TreeIndex::decode_label(entry.group_type, entry.label),
                crate::tree::GroupLabel::RecordType { ref sig } if sig == &sig_upper
            )
        });

        let Some(group_idx) = group_idx else {
            return Ok(Vec::new());
        };

        // Collect the paginated child slice (avoid holding borrow into mutable self below)
        let children_slice: Vec<ChildRef> = {
            let entry = &self.index.tree.groups[group_idx];
            let start = offset.min(entry.children.len());
            let end = (offset + limit).min(entry.children.len());
            entry.children[start..end].to_vec()
        };

        let mut result = Vec::new();
        for child in children_slice {
            match child {
                ChildRef::Group(idx) => {
                    result.push(GroupChild::Group(self.index.tree.group_node(idx)));
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
                        form_id: FormId(form_id),
                        editor_id,
                        record_type,
                        offset: rec_offset,
                    }));
                }
            }
        }
        Ok(result)
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
            form_id: FormId(hdr.form_id),
            editor_id,
            record_type: hdr.signature.to_string(),
            offset,
        })
    }

    pub fn record_at_meta(&self, meta: &crate::reader::RecordMeta) -> anyhow::Result<RecordResult> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&parsed.subrecords);
        let ctx = DecodeContext {
            schema: &self.schema,
            form_version: parsed.header.form_version,
            is_localized: self.is_localized,
            localization: self.localization.as_ref(),
            curves: self.curves.as_ref(),
            resolve_depth: crate::decode::ResolveDepth::None,
            resolver: None,
            outer_struct: None,
            record_signature: None,
            record_edid_char: None,
            scope_min_doc_index: None,
            scope_max_doc_index: None,
        };
        let fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
        Ok(RecordResult {
            header: parsed.header,
            editor_id,
            fields,
        })
    }

    fn record_at_meta_with_depth(
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
        let ctx = DecodeContext {
            schema: &self.schema,
            form_version: parsed.header.form_version,
            is_localized: self.is_localized,
            localization: self.localization.as_ref(),
            curves: self.curves.as_ref(),
            resolve_depth: depth,
            resolver: resolver
                .as_ref()
                .map(|r| r as &dyn crate::decode::FormIdRefResolver),
            outer_struct: None,
            record_signature: None,
            record_edid_char: None,
            scope_min_doc_index: None,
            scope_max_doc_index: None,
        };
        let fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
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
    pub fn record_by_edid_resolved(
        &mut self,
        edid: &str,
        depth: crate::decode::ResolveDepth,
    ) -> anyhow::Result<RecordResult> {
        self.check_not_lite("EditorID lookup")?;
        self.index.ensure_edid_index(&self.esm)?;
        let form_id = self
            .index
            .get_by_edid(edid)
            .with_context(|| format!("EditorID '{}' not found", edid))?;
        self.record_by_formid_resolved(form_id, depth)
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
        let meta = self.db.get_formid_meta(id).ok()?;
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
        let meta = self.db.get_formid_meta(id).ok()?;
        let parsed = self.db.esm.parse_record_at(meta.offset).ok()?;
        let editor_id = crate::reader::edid_from_subrecords(&parsed.subrecords);
        let record_type = parsed.header.signature.clone();
        // Build a nested DecodeContext with depth decremented
        let nested_resolver = DatabaseResolver {
            db: self.db,
            remaining: self.remaining - 1,
        };
        let ctx = DecodeContext {
            schema: &self.db.schema,
            form_version: parsed.header.form_version,
            is_localized: self.db.is_localized,
            localization: self.db.localization.as_ref(),
            curves: self.db.curves.as_ref(),
            resolve_depth: crate::decode::ResolveDepth::Full,
            resolver: Some(&nested_resolver),
            outer_struct: None,
            record_signature: None,
            record_edid_char: None,
            scope_min_doc_index: None,
            scope_max_doc_index: None,
        };
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
/// EditorID. Note that short all-hex EditorIDs (e.g. `cafe`) are read as
/// FormIDs; an explicit `--edid` flag disambiguates those cases.
pub fn looks_like_formid(s: &str) -> bool {
    let s = s.trim();
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    !body.is_empty() && body.len() <= 8 && body.bytes().all(|b| b.is_ascii_hexdigit())
}
