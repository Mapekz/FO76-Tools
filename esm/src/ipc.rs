//! Wire types and the canonical `dispatch` function shared by CLI, daemon, and N-API.

use crate::diff::diff_databases;
use crate::registry::Registry;
use crate::{Database, FormId, ResolveDepth, SearchField};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

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

/// Record selector: FormID or EditorID.
///
/// Adjacently tagged so primitive-newtype variants (FormId wraps u32, Edid wraps String)
/// survive JSON round-trips. Internally-tagged enums cannot serialize non-map payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RecordSel {
    FormId(FormId),
    Edid(String),
}

impl RecordSel {
    /// Build a selector from a single user-supplied token, auto-detecting whether
    /// it denotes a FormID (numeric/hex) or an EditorID via [`crate::looks_like_formid`].
    pub fn from_input(s: &str) -> anyhow::Result<RecordSel> {
        if crate::looks_like_formid(s) {
            Ok(RecordSel::FormId(crate::parse_form_id_input(s)?))
        } else {
            Ok(RecordSel::Edid(s.to_string()))
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
    RecordRaw {
        sel: RecordSel,
    },
    ListByType {
        sig: String,
        limit: usize,
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
    },
    ListGroups,
    ListTypeChildren {
        sig: String,
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
    },
    /// Walk reverse references through leveled lists to terminal drop sources.
    Sources {
        sel: RecordSel,
        #[serde(default)]
        max_depth: Option<usize>,
    },
    /// Daemon lifecycle: no ESM path required (ignored).
    Shutdown,
}

// ─── Shared DTOs (lifted from CLI) ──────────────────────────────────────────

/// One referencer row enriched with record type (refs command output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefRow {
    pub form_id: String,
    pub record_type: Option<String>,
    pub editor_id: Option<String>,
    pub name: Option<String>,
    pub offset: u64,
}

/// Referenced-by result with total count and optional cap flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefList {
    pub target: String,
    pub rows: Vec<RefRow>,
    pub total: usize,
    pub capped: bool,
}

/// Hex dump view of a raw record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRecordView {
    pub header: crate::reader::RecordHeaderInfo,
    pub subrecords: Vec<RawSubrecordView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSubrecordView {
    pub signature: String,
    pub size: usize,
    pub hex: String,
}

/// Counts of schema-coverage markers per record type.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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
        Op::Diff { b, record_type } => {
            let (key_a, arc_a) = reg.get_or_open_with_key(&req.esm)?;
            let (key_b, arc_b) = reg.get_or_open_with_key(b)?;
            let same_db = key_a == key_b || std::sync::Arc::ptr_eq(&arc_a, &arc_b);
            let mut result = if same_db {
                let db = arc_a.lock().unwrap();
                diff_databases(&db, &db)?
            } else if key_a < key_b {
                let db_a = arc_a.lock().unwrap();
                let db_b = arc_b.lock().unwrap();
                diff_databases(&db_a, &db_b)?
            } else {
                let db_b = arc_b.lock().unwrap();
                let db_a = arc_a.lock().unwrap();
                diff_databases(&db_a, &db_b)?
            };
            if let Some(sig) = record_type {
                let sig = sig.to_uppercase();
                result.added.retain(|s| s.record_type == sig);
                result.removed.retain(|s| s.record_type == sig);
                result.changed.retain(|d| d.stub.record_type == sig);
            }
            Ok(serde_json::to_value(&result)?)
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
            let result = record_resolved(db, sel, *depth)?;
            Ok(serde_json::json!({
                "header": result.header,
                "editor_id": result.editor_id,
                "fields": result.fields
            }))
        }
        Op::RecordRaw { sel } => {
            let form_id = resolve_sel(db, sel)?;
            let rec = db.record_raw(form_id)?;
            let view = RawRecordView {
                header: rec.header,
                subrecords: rec
                    .subrecords
                    .iter()
                    .map(|sr| RawSubrecordView {
                        signature: sr.signature.to_string(),
                        size: sr.data.len(),
                        hex: sr.data.iter().map(|b| format!("{:02x}", b)).collect(),
                    })
                    .collect(),
            };
            Ok(serde_json::to_value(&view)?)
        }
        Op::ListByType { sig, limit } => {
            let entries = db.list_by_type(sig, *limit)?;
            Ok(serde_json::to_value(&entries)?)
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
        Op::ReferencedBy { sel, limit } => {
            let target = resolve_sel(db, sel)?;
            let ref_list = referenced_by_enriched(db, target, *limit)?;
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
        Op::Sources { sel, max_depth } => {
            let target = resolve_sel(db, sel)?;
            let opts = crate::sources::SourcesOptions {
                max_depth: max_depth.unwrap_or(crate::sources::DEFAULT_MAX_DEPTH),
            };
            let list = crate::sources::sources_of(db, target, &opts)?;
            Ok(serde_json::to_value(&list)?)
        }
    }
}

fn resolve_sel(db: &mut Database, sel: &RecordSel) -> anyhow::Result<FormId> {
    match sel {
        RecordSel::FormId(fid) => Ok(*fid),
        RecordSel::Edid(edid) => {
            db.index.ensure_edid_index(&db.esm)?;
            db.index
                .get_by_edid(edid)
                .ok_or_else(|| anyhow::anyhow!("EditorID '{}' not found", edid))
        }
    }
}

fn record_resolved(
    db: &mut Database,
    sel: &RecordSel,
    depth: ResolveDepth,
) -> anyhow::Result<crate::RecordResult> {
    match sel {
        RecordSel::FormId(fid) => {
            if depth != ResolveDepth::None {
                db.record_by_formid_resolved(*fid, depth)
            } else {
                db.record_by_formid(*fid)
            }
        }
        RecordSel::Edid(edid) => {
            if depth != ResolveDepth::None {
                db.record_by_edid_resolved(edid, depth)
            } else {
                db.record_by_edid(edid)
            }
        }
    }
}

pub fn referenced_by_enriched(
    db: &mut Database,
    target: FormId,
    limit: usize,
) -> anyhow::Result<RefList> {
    let mut rows = db.referenced_by(target)?;
    rows.sort_by_key(|r| {
        crate::parse_form_id_input(&r.form_id)
            .map(|f| f.0)
            .unwrap_or(u32::MAX)
    });

    let enriched: Vec<RefRow> = rows
        .into_iter()
        .map(|r| {
            let record_type = crate::parse_form_id_input(&r.form_id)
                .ok()
                .and_then(|fid| db.index.get_by_formid(fid))
                .map(|m| m.signature.clone());
            RefRow {
                form_id: r.form_id,
                record_type,
                editor_id: r.editor_id,
                name: r.name,
                offset: r.offset,
            }
        })
        .collect();

    let total = enriched.len();
    let capped = limit > 0 && total > limit;
    let limited: Vec<RefRow> = if limit > 0 {
        enriched.into_iter().take(limit).collect()
    } else {
        enriched
    };

    Ok(RefList {
        target: target.display(),
        rows: limited,
        total,
        capped,
    })
}

fn count_markers(v: &Value, m: &mut Markers) {
    match v {
        Value::Object(obj) => {
            if obj.get("_unknown_record") == Some(&Value::Bool(true)) {
                m.unknown_record += 1;
            }
            if obj.get("_raw") == Some(&Value::Bool(true)) && obj.contains_key("reason") {
                m.raw_fallback += 1;
            }
            if obj.get("_unresolved") == Some(&Value::Bool(true)) {
                m.unresolved += 1;
            }
            if let Some(Value::Object(unmapped)) = obj.get("_unmapped") {
                for subs in unmapped.values() {
                    if let Value::Array(arr) = subs {
                        m.unmapped += arr.len() as u64;
                    }
                }
            }
            for (key, child) in obj {
                if key == "_unmapped" {
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
            match db.record_at_meta(meta) {
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
