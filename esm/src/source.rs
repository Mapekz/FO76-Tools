//! Where chase, walk and the drop table get records and reverse references.
//!
//! [`RecordSource`] keeps their traversal logic free of I/O: the database
//! implements it in `ops::analysis`, and [`MemorySource`] answers from
//! records supplied up front (tests use it with no ESM involved). Records
//! arrive as [`Resolved`] trees, so traversal reads FormIDs typed.

use std::collections::HashMap;

use serde_json::Value;

use crate::ops::{RecordSel, RefDepth};
use crate::reader::RecordHeaderInfo;
use crate::{FormId, RefList, Resolved};

/// One record a [`RecordSource`] fetched for a selector: the record, or why
/// the selector didn't resolve.
#[derive(Debug, Clone)]
pub struct SourceRecord {
    /// The selector, rendered for display (see [`RecordSel::display`]).
    pub sel: String,
    pub header: Option<RecordHeaderInfo>,
    pub editor_id: Option<String>,
    pub fields: Option<Resolved>,
    /// Set instead of the others when the selector didn't resolve.
    pub error: Option<String>,
}

impl SourceRecord {
    /// An entry for a selector that didn't resolve.
    pub fn failed(sel: String, error: String) -> Self {
        SourceRecord {
            sel,
            header: None,
            editor_id: None,
            fields: None,
            error: Some(error),
        }
    }
}

/// A bulk record fetch and a reverse-reference walk (`Op::ReferencedBy`).
/// `bulk_get` returns exactly one entry per selector, in selector order (an
/// error entry for one that doesn't resolve): callers pair results with
/// their selectors by position.
pub trait RecordSource {
    fn bulk_get(&mut self, sels: &[RecordSel]) -> anyhow::Result<Vec<SourceRecord>>;

    fn refs(
        &mut self,
        target: FormId,
        depth: RefDepth,
        limit: usize,
        type_filter: &str,
        paths: bool,
    ) -> anyhow::Result<RefList>;
}

/// Batch-fetch `fids` and return them keyed by FormID.
pub(crate) fn bulk_fetch_map(
    f: &mut impl RecordSource,
    fids: &[FormId],
) -> anyhow::Result<HashMap<FormId, SourceRecord>> {
    if fids.is_empty() {
        return Ok(HashMap::new());
    }
    let sels: Vec<RecordSel> = fids.iter().map(|fid| RecordSel::FormId(*fid)).collect();
    let entries = f.bulk_get(&sels)?;
    Ok(fids.iter().copied().zip(entries).collect())
}

/// A [`RecordSource`] over records and reverse-reference lists supplied up
/// front. An unknown selector gets an error entry, and `refs` answers with the list inserted
/// for its `(target, type filter)` pair (empty otherwise), truncated to
/// `limit` like the database's.
#[derive(Debug, Default)]
pub struct MemorySource {
    records: HashMap<FormId, SourceRecord>,
    refs: HashMap<(FormId, String), RefList>,
}

impl MemorySource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a record with header `signature`/`flags`, its EditorID and its
    /// decoded fields as `--resolve stub` JSON (read with
    /// [`Resolved::from_stub_json`]).
    pub fn insert(
        &mut self,
        formid: FormId,
        signature: &str,
        editor_id: &str,
        flags: u32,
        fields: Value,
    ) {
        self.insert_resolved(
            formid,
            signature,
            editor_id,
            flags,
            Resolved::from_stub_json(&fields),
        );
    }

    /// [`Self::insert`] with the fields already a [`Resolved`] tree.
    pub fn insert_resolved(
        &mut self,
        formid: FormId,
        signature: &str,
        editor_id: &str,
        flags: u32,
        fields: Resolved,
    ) {
        self.records.insert(
            formid,
            SourceRecord {
                sel: formid.display(),
                header: Some(RecordHeaderInfo {
                    signature: signature.to_string(),
                    form_id: formid,
                    flags,
                    form_version: 0,
                    data_size: 0,
                    offset: 0,
                }),
                editor_id: Some(editor_id.to_string()),
                fields: Some(fields),
                error: None,
            },
        );
    }

    /// Answer `refs(target, .., type_filter, ..)` with `list`.
    pub fn insert_refs(&mut self, target: FormId, type_filter: &str, list: RefList) {
        self.refs.insert((target, type_filter.to_string()), list);
    }

    fn lookup(&self, sel: &RecordSel) -> Option<&SourceRecord> {
        let by_edid = |edid: &str| {
            self.records
                .values()
                .find(|e| e.editor_id.as_deref() == Some(edid))
        };
        match sel {
            RecordSel::FormId(fid) => self.records.get(fid),
            RecordSel::Edid(edid) => by_edid(edid),
            RecordSel::Auto(token) => crate::parse_form_id_input(token)
                .ok()
                .and_then(|fid| self.records.get(&fid))
                .or_else(|| by_edid(token)),
            // Entry-point and OMOD-property selectors name refs seeds, not
            // records.
            RecordSel::EntryPoint(_) | RecordSel::OmodProperty(_) => None,
        }
    }
}

impl RecordSource for MemorySource {
    fn bulk_get(&mut self, sels: &[RecordSel]) -> anyhow::Result<Vec<SourceRecord>> {
        Ok(sels
            .iter()
            .map(|sel| {
                let display = sel.display();
                match self.lookup(sel) {
                    Some(entry) => SourceRecord {
                        sel: display,
                        ..entry.clone()
                    },
                    None => SourceRecord::failed(display.clone(), format!("not found: {display}")),
                }
            })
            .collect())
    }

    fn refs(
        &mut self,
        target: FormId,
        _depth: RefDepth,
        limit: usize,
        type_filter: &str,
        _paths: bool,
    ) -> anyhow::Result<RefList> {
        let mut list = self
            .refs
            .get(&(target, type_filter.to_string()))
            .cloned()
            .unwrap_or_else(|| RefList {
                target: target.display(),
                ..Default::default()
            });
        if limit > 0 && list.rows.len() > limit {
            list.rows.truncate(limit);
            list.capped = true;
        }
        Ok(list)
    }
}
