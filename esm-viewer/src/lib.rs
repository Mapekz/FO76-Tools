pub mod ba2;
pub mod compress;
pub mod decode;
pub mod diff;
pub mod format;
pub mod formid;
pub mod index;
pub mod reader;
pub mod schema;
pub mod strings;
pub mod tree;

use crate::decode::{decode_record, DecodeContext};
use crate::formid::parse_formid;
use crate::index::Index;
use crate::reader::{edid_from_subrecords, EsmFile, FileInfo, ParsedRecord, RecordHeaderInfo};
use crate::schema::Schema;
use crate::strings::Localization;
use crate::tree::ChildRef;
use anyhow::{bail, Context};
pub use diff::{DiffResult, RecordDiff, RecordStub};
pub use formid::FormId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    /// Resolved string tables, if a localization BA2 was found or supplied.
    pub localization: Option<Localization>,
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

impl Database {
    /// Open an ESM file.
    ///
    /// If a `SeventySix - Localization.ba2` file is found next to the ESM, it is
    /// loaded silently.  Failures to load the BA2 produce a `stderr` warning but
    /// do not abort.
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let esm = EsmFile::open(path)?;
        let index = Index::build(&esm)?;
        let schema = Schema::load_embedded().context("load embedded schema")?;

        // Auto-detect sibling localization BA2.
        let sibling_ba2 = path.with_file_name("SeventySix - Localization.ba2");
        let localization = if sibling_ba2.exists() {
            match Localization::from_ba2(&sibling_ba2, "en", "seventysix") {
                Ok(loc) => Some(loc),
                Err(e) => {
                    eprintln!("Warning: failed to load localization: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Database {
            esm,
            index,
            schema,
            localization,
        })
    }

    /// Replace (or set) the localization tables used for LString resolution.
    pub fn set_localization(&mut self, loc: Localization) {
        self.localization = Some(loc);
    }

    pub fn file_info(&self) -> anyhow::Result<FileInfo> {
        let mut info = self.esm.file_info()?;
        info.path = self.esm.path.clone();
        Ok(info)
    }

    pub fn record_by_formid(&mut self, form_id: FormId) -> anyhow::Result<RecordResult> {
        let meta = self
            .index
            .get_by_formid(form_id)
            .with_context(|| format!("FormID {} not found", form_id))?
            .clone();
        self.record_at_meta(&meta)
    }

    pub fn record_by_edid(&mut self, edid: &str) -> anyhow::Result<RecordResult> {
        self.index.ensure_edid_index(&self.esm)?;
        let form_id = self
            .index
            .get_by_edid(edid)
            .with_context(|| format!("EditorID '{}' not found", edid))?;
        self.record_by_formid(form_id)
    }

    pub fn list_by_type(&mut self, sig: &str, limit: usize) -> anyhow::Result<Vec<ListEntry>> {
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

    pub fn record_raw(&mut self, form_id: FormId) -> anyhow::Result<ParsedRecord> {
        let meta = self
            .index
            .get_by_formid(form_id)
            .with_context(|| format!("FormID {} not found", form_id))?
            .clone();
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
        &mut self,
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

    pub(crate) fn record_at_meta(
        &self,
        meta: &crate::reader::RecordMeta,
    ) -> anyhow::Result<RecordResult> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&parsed.subrecords);
        let ctx = DecodeContext {
            schema: &self.schema,
            form_version: parsed.header.form_version,
            localization: self.localization.as_ref(),
        };
        let fields = decode_record(&ctx, &parsed.header.signature, &parsed.subrecords);
        Ok(RecordResult {
            header: parsed.header,
            editor_id,
            fields,
        })
    }
}

pub fn parse_form_id_input(s: &str) -> anyhow::Result<FormId> {
    parse_formid(s)
}
