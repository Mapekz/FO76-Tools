#![deny(clippy::all)]

use esm::ipc::RecordSel;
use esm::{Database, FilterOp, FormId, ResolveDepth, SearchField};
use napi_derive::napi;
use std::sync::{Arc, Mutex};

#[napi]
pub struct EsmDatabase {
    inner: Arc<Mutex<Database>>,
}

#[napi]
impl EsmDatabase {
    /// Open an ESM file asynchronously (blocks on mmap + index build).
    #[napi(factory)]
    pub async fn open_database(path: String) -> napi::Result<EsmDatabase> {
        let db = tokio::task::spawn_blocking(move || Database::open(&path))
            .await
            .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(EsmDatabase {
            inner: Arc::new(Mutex::new(db)),
        })
    }

    #[napi]
    pub fn file_info(&self) -> napi::Result<serde_json::Value> {
        let db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let info = db
            .file_info()
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&info).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    #[napi]
    pub fn list_groups(&self) -> napi::Result<serde_json::Value> {
        let db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let groups = db.list_groups();
        serde_json::to_value(&groups).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Paginated record rows for the given 4-character record type signature.
    #[napi]
    pub fn list_type_records(
        &self,
        sig: String,
        offset: u32,
        limit: u32,
    ) -> napi::Result<serde_json::Value> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let rows = db
            .list_type_records(&sig, offset as usize, limit as usize)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&rows).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Search records by EditorID and/or display name using a `*`-wildcard pattern.
    ///
    /// `types` restricts the search to the given 4-character record-type
    /// signatures (empty = all types). `field` is one of `"edid"` | `"name"` |
    /// `"both"`. `limit` caps the number of results (`0` = no limit).
    #[napi]
    pub fn search(
        &self,
        pattern: String,
        types: Vec<String>,
        field: String,
        limit: u32,
    ) -> napi::Result<serde_json::Value> {
        let field = parse_search_field(&field)?;
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let rows = db
            .search(&pattern, &types, field, limit as usize)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&rows).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Filter records of type `sig` by a predicate against their decoded
    /// field body. `path` is a dot-separated path (`"[]"` segments fan out
    /// over arrays); `None`/empty deep-scans every field. `op` is one of
    /// `"exists"` | `"eq"` | `"contains"` | `"gt"` | `"lt"` | `"gte"` | `"lte"`.
    /// `limit` caps the number of returned rows (`0` = no limit).
    #[napi]
    pub fn filter_type_records(
        &self,
        sig: String,
        path: Option<String>,
        op: String,
        value: Option<String>,
        limit: u32,
    ) -> napi::Result<serde_json::Value> {
        let op = parse_filter_op(&op)?;
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let result = db
            .filter_type_records(&sig, path.as_deref(), op, value.as_deref(), limit as usize)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&result).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// List every dot-notation field path observed across a (possibly capped)
    /// decoded sample of a type's records — for filter-panel autocomplete.
    #[napi]
    pub fn list_type_field_paths(&self, sig: String) -> napi::Result<serde_json::Value> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let paths = db
            .list_type_field_paths(&sig)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&paths).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// List direct children of the top-level GRUP with the given record type signature.
    #[napi]
    pub fn list_type_children(
        &self,
        sig: String,
        offset: u32,
        limit: u32,
    ) -> napi::Result<serde_json::Value> {
        let db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let children = db
            .list_type_children(&sig, offset as usize, limit as usize)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&children).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// List direct children of an arbitrary GRUP by its own header offset — used for
    /// recursive descent below the top level (e.g. into a worldspace's exterior blocks,
    /// then into a block's cells). `group_offset` is passed as `f64`/JS `number` rather
    /// than a `u64`/BigInt: GRUP offsets fit exactly within f64's safe-integer range for
    /// any realistic ESM file size, and this keeps the JS side free of BigInt handling.
    #[napi]
    pub fn list_group_children(
        &self,
        group_offset: f64,
        offset: u32,
        limit: u32,
    ) -> napi::Result<serde_json::Value> {
        let db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let children = db
            .list_group_children(group_offset as u64, offset as usize, limit as usize)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&children).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Decode a record by FormID hex string (e.g. "0x0000463F").
    ///
    /// `resolve` controls FormID field expansion: `"none"` | `"stub"` | `"full"`.
    #[napi]
    pub fn record_by_formid(
        &self,
        formid: String,
        resolve: String,
    ) -> napi::Result<serde_json::Value> {
        let fid: FormId = formid
            .parse()
            .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
        let depth = parse_resolve_depth(&resolve)?;
        let db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let result = db
            .record_by_formid_resolved(fid, depth)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        serde_json::to_value(&result).map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Decode a record by EditorID string.
    ///
    /// `resolve` controls FormID field expansion: `"none"` | `"stub"` | `"full"`.
    #[napi]
    pub async fn record_by_edid(
        &self,
        edid: String,
        resolve: String,
    ) -> napi::Result<serde_json::Value> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let depth = parse_resolve_depth(&resolve)?;
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let result = db
                .record_by_edid_resolved(&edid, depth)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            serde_json::to_value(&result).map_err(|e| napi::Error::from_reason(format!("{e}")))
        })
        .await
        .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
    }

    /// Decode a record by FormID or EditorID (auto-detected).
    ///
    /// `resolve` controls FormID field expansion: `"none"` | `"stub"` | `"full"`.
    #[napi]
    pub async fn record_by_id(
        &self,
        id: String,
        resolve: String,
    ) -> napi::Result<serde_json::Value> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let sel = RecordSel::from_input(&id)
                .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
            let depth = parse_resolve_depth(&resolve)?;
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let result = match sel {
                RecordSel::FormId(fid) => db.record_by_formid_resolved(fid, depth),
                RecordSel::Edid(edid) => db.record_by_edid_resolved(&edid, depth),
            }
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            serde_json::to_value(&result).map_err(|e| napi::Error::from_reason(format!("{e}")))
        })
        .await
        .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
    }

    /// Return all records that reference the given FormID hex string.
    #[napi]
    pub async fn referenced_by(&self, formid: String) -> napi::Result<serde_json::Value> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let fid: FormId = formid
                .parse()
                .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let rows = db
                .referenced_by(fid)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            serde_json::to_value(&rows).map_err(|e| napi::Error::from_reason(format!("{e}")))
        })
        .await
        .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
    }

    /// Return all records that reference the given FormID or EditorID (auto-detected).
    ///
    /// `depth` controls the reverse-reference walk depth (default 1 = direct refs only,
    /// capped at DEFAULT_MAX_DEPTH = 6). Each returned row includes its hop `depth` and
    /// an intermediate-node `path` array (empty for depth-1 results).
    #[napi]
    pub async fn referenced_by_id(
        &self,
        id: String,
        depth: Option<u32>,
    ) -> napi::Result<serde_json::Value> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let sel = RecordSel::from_input(&id)
                .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let fid = resolve_sel(&mut db, sel)?;
            let walk_depth = depth
                .map(|d| (d as usize).clamp(1, esm::ipc::DEFAULT_MAX_DEPTH))
                .unwrap_or(1);
            let list = esm::ipc::referenced_by_enriched(&mut db, fid, walk_depth, usize::MAX)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            serde_json::to_value(&list).map_err(|e| napi::Error::from_reason(format!("{e}")))
        })
        .await
        .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
    }

    /// Hex/subrecord dump of a record, by FormID or EditorID (auto-detected).
    #[napi]
    pub async fn record_raw(&self, id: String) -> napi::Result<serde_json::Value> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let sel = RecordSel::from_input(&id)
                .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let fid = resolve_sel(&mut db, sel)?;
            let rec = db
                .record_raw(fid)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            let view = esm::ipc::raw_record_view(&rec);
            serde_json::to_value(&view).map_err(|e| napi::Error::from_reason(format!("{e}")))
        })
        .await
        .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
    }

    /// Decode-coverage report: per-type counts of _unknown_record/_raw/_unmapped/
    /// _unresolved markers. `record_type` (4-char sig, optional) restricts to one
    /// type; `sample` caps records decoded per type (0 = unlimited).
    #[napi]
    pub async fn coverage_report(
        &self,
        record_type: Option<String>,
        sample: u32,
    ) -> napi::Result<serde_json::Value> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let report = esm::ipc::coverage_report(&db, record_type.as_deref(), sample as usize)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            serde_json::to_value(&report).map_err(|e| napi::Error::from_reason(format!("{e}")))
        })
        .await
        .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
    }
}

/// Parse a FormID hex string to its display form.
#[napi]
pub fn parse_form_id(s: String) -> napi::Result<String> {
    let fid: FormId = s
        .parse()
        .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
    Ok(fid.display())
}

fn parse_resolve_depth(s: &str) -> napi::Result<ResolveDepth> {
    match s {
        "none" => Ok(ResolveDepth::None),
        "stub" => Ok(ResolveDepth::Stub),
        "full" => Ok(ResolveDepth::Full),
        other => Err(napi::Error::from_reason(format!(
            "unknown resolve depth '{other}'; expected none|stub|full"
        ))),
    }
}

fn parse_search_field(s: &str) -> napi::Result<SearchField> {
    match s {
        "edid" => Ok(SearchField::Edid),
        "name" => Ok(SearchField::Name),
        "both" => Ok(SearchField::Both),
        other => Err(napi::Error::from_reason(format!(
            "unknown search field '{other}'; expected edid|name|both"
        ))),
    }
}

fn parse_filter_op(s: &str) -> napi::Result<FilterOp> {
    match s {
        "exists" => Ok(FilterOp::Exists),
        "eq" => Ok(FilterOp::Eq),
        "contains" => Ok(FilterOp::Contains),
        "gt" => Ok(FilterOp::Gt),
        "lt" => Ok(FilterOp::Lt),
        "gte" => Ok(FilterOp::Gte),
        "lte" => Ok(FilterOp::Lte),
        other => Err(napi::Error::from_reason(format!(
            "unknown filter op '{other}'; expected exists|eq|contains|gt|lt|gte|lte"
        ))),
    }
}

fn resolve_sel(db: &mut Database, sel: RecordSel) -> napi::Result<FormId> {
    match sel {
        RecordSel::FormId(fid) => Ok(fid),
        RecordSel::Edid(edid) => {
            db.index
                .ensure_edid_index(&db.esm)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            db.index
                .get_by_edid(&edid)
                .ok_or_else(|| napi::Error::from_reason(format!("EditorID '{}' not found", edid)))
        }
    }
}
