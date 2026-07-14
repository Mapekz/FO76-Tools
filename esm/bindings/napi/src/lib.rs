#![deny(clippy::all)]

use esm::ipc::RecordSel;
use esm::{Database, FormId, ResolveDepth, SearchField};
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
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        esm::ipc::dispatch_op(&mut db, &esm::ipc::Op::FileInfo)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
    }

    #[napi]
    pub fn list_groups(&self) -> napi::Result<serde_json::Value> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        esm::ipc::dispatch_op(&mut db, &esm::ipc::Op::ListGroups)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
        let op = esm::ipc::Op::ListTypeRecords {
            sig,
            offset: offset as usize,
            limit: limit as usize,
        };
        esm::ipc::dispatch_op(&mut db, &op).map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
        let field = esm::query::search_field(Some(&field), SearchField::Both)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let op = esm::ipc::Op::Search {
            pattern,
            types,
            field,
            limit: limit as usize,
        };
        esm::ipc::dispatch_op(&mut db, &op).map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
        let op =
            esm::query::filter_op(&op).map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let wire_op = esm::ipc::Op::FilterTypeRecords {
            sig,
            path,
            filter_op: op,
            value,
            limit: limit as usize,
        };
        esm::ipc::dispatch_op(&mut db, &wire_op)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
    }

    /// List every dot-notation field path observed across a (possibly capped)
    /// decoded sample of a type's records — for filter-panel autocomplete.
    #[napi]
    pub fn list_type_field_paths(&self, sig: String) -> napi::Result<serde_json::Value> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        esm::ipc::dispatch_op(&mut db, &esm::ipc::Op::ListTypeFieldPaths { sig })
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
    }

    /// List direct children of the top-level GRUP with the given record type signature.
    #[napi]
    pub fn list_type_children(
        &self,
        sig: String,
        offset: u32,
        limit: u32,
    ) -> napi::Result<serde_json::Value> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let op = esm::ipc::Op::ListTypeChildren {
            sig,
            offset: offset as usize,
            limit: limit as usize,
        };
        esm::ipc::dispatch_op(&mut db, &op).map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let wire_op = esm::ipc::Op::ListGroupChildren {
            group_offset: group_offset as u64,
            offset: offset as usize,
            limit: limit as usize,
        };
        esm::ipc::dispatch_op(&mut db, &wire_op)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
        let depth = esm::query::resolve_depth(Some(&resolve), ResolveDepth::None)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        let mut db = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let op = esm::ipc::Op::Record {
            sel: RecordSel::FormId(fid),
            depth,
        };
        esm::ipc::dispatch_op(&mut db, &op).map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
            let depth = esm::query::resolve_depth(Some(&resolve), ResolveDepth::None)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let op = esm::ipc::Op::Record {
                sel: RecordSel::Edid(edid),
                depth,
            };
            esm::ipc::dispatch_op(&mut db, &op)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
            let depth = esm::query::resolve_depth(Some(&resolve), ResolveDepth::None)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let op = esm::ipc::Op::Record { sel, depth };
            esm::ipc::dispatch_op(&mut db, &op)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
            let walk_depth = esm::query::clamp_ref_depth(depth.map(|d| d as usize));
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let op = esm::ipc::Op::ReferencedBy {
                sel,
                limit: usize::MAX,
                depth: walk_depth,
                type_filter: None,
                paths: false,
            };
            esm::ipc::dispatch_op(&mut db, &op)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
            let op = esm::ipc::Op::RecordRaw { sel };
            esm::ipc::dispatch_op(&mut db, &op)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
            let mut db = inner
                .lock()
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            let op = esm::ipc::Op::Coverage {
                record_type,
                sample: sample as usize,
            };
            esm::ipc::dispatch_op(&mut db, &op)
                .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
        })
        .await
        .map_err(|e| napi::Error::from_reason(format!("join error: {e}")))?
    }

    /// Compare this database (treated as the "old"/base snapshot) against
    /// `other` (the "new" snapshot). `record_type` (optional 4-char sig)
    /// restricts the diff to one type; `bodies` is "none"|"stub"|"full"
    /// (detail level for added/removed record bodies); `suppress_noise` strips
    /// known-noisy fields (placement/CELL-precombine) from `changed` records;
    /// `exclude_types` omits matching signatures from added/removed/changed
    /// entirely.
    #[napi]
    pub async fn diff(
        &self,
        other: &EsmDatabase,
        record_type: Option<String>,
        bodies: String,
        suppress_noise: bool,
        exclude_types: Vec<String>,
    ) -> napi::Result<serde_json::Value> {
        let bodies = esm::query::body_detail(Some(&bodies), esm::diff::BodyDetail::Full)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        let arc_a = self.inner.clone();
        let arc_b = other.inner.clone();
        tokio::task::spawn_blocking(move || {
            let options = esm::query::diff_options(bodies, suppress_noise, exclude_types);
            // Lock ordering doesn't have a `Registry`'s canonical keys to compare here
            // (unlike `dispatch_inner`'s `Diff` arm) — order by raw `Arc` pointer address
            // instead, which is just as deadlock-safe as long as it's used consistently.
            let same_db = Arc::ptr_eq(&arc_a, &arc_b);
            if same_db {
                let db = arc_a
                    .lock()
                    .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                esm::ipc::diff_locked(&db, &db, &options, &record_type)
            } else if (Arc::as_ptr(&arc_a) as usize) < (Arc::as_ptr(&arc_b) as usize) {
                let db_a = arc_a
                    .lock()
                    .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                let db_b = arc_b
                    .lock()
                    .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                esm::ipc::diff_locked(&db_a, &db_b, &options, &record_type)
            } else {
                let db_b = arc_b
                    .lock()
                    .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                let db_a = arc_a
                    .lock()
                    .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                esm::ipc::diff_locked(&db_a, &db_b, &options, &record_type)
            }
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))
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
