pub mod compress;
pub mod decode;
pub mod diff;
pub mod format;
pub mod formid;
pub mod index;
pub mod reader;
pub mod schema;

use crate::decode::{decode_record, DecodeContext};
use crate::formid::parse_formid;
use crate::index::Index;
use crate::reader::{edid_from_subrecords, EsmFile, FileInfo, ParsedRecord, RecordHeaderInfo};
use crate::schema::Schema;
use anyhow::{bail, Context};
pub use diff::{DiffResult, RecordDiff, RecordStub};
pub use formid::FormId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub struct Database {
    pub esm: EsmFile,
    pub index: Index,
    pub schema: Schema,
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
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let esm = EsmFile::open(path)?;
        let index = Index::build(&esm)?;
        let schema = Schema::load_embedded().context("load embedded schema")?;
        Ok(Database { esm, index, schema })
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

    pub(crate) fn record_at_meta(
        &self,
        meta: &crate::reader::RecordMeta,
    ) -> anyhow::Result<RecordResult> {
        let parsed = self.esm.parse_record_at(meta.offset)?;
        let editor_id = edid_from_subrecords(&parsed.subrecords);
        let ctx = DecodeContext {
            schema: &self.schema,
            form_version: parsed.header.form_version,
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
