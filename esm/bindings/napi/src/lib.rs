#![deny(clippy::all)]

use esm::ipc::RecordSel;
use esm::{Database, FormId, ResolveDepth};
use napi_derive::napi;
use std::sync::Mutex;

#[napi]
pub struct EsmDatabase {
    inner: Mutex<Database>,
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
            inner: Mutex::new(db),
        })
    }

    #[napi]
    pub fn file_info(&self) -> napi::Result<serde_json::Value> {
        let db = self.inner.lock().unwrap();
        let info = db
            .file_info()
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(serde_json::to_value(&info).unwrap())
    }

    #[napi]
    pub fn list_groups(&self) -> napi::Result<serde_json::Value> {
        let db = self.inner.lock().unwrap();
        let groups = db.list_groups();
        Ok(serde_json::to_value(&groups).unwrap())
    }

    /// Paginated record rows for the given 4-character record type signature.
    #[napi]
    pub fn list_type_records(
        &self,
        sig: String,
        offset: u32,
        limit: u32,
    ) -> napi::Result<serde_json::Value> {
        let mut db = self.inner.lock().unwrap();
        let rows = db
            .list_type_records(&sig, offset as usize, limit as usize)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(serde_json::to_value(&rows).unwrap())
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
        let db = self.inner.lock().unwrap();
        let result = db
            .record_by_formid_resolved(fid, depth)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(serde_json::to_value(&result).unwrap())
    }

    /// Decode a record by EditorID string.
    #[napi]
    pub fn record_by_edid(&self, edid: String) -> napi::Result<serde_json::Value> {
        let mut db = self.inner.lock().unwrap();
        let result = db
            .record_by_edid_resolved(&edid, ResolveDepth::Stub)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(serde_json::to_value(&result).unwrap())
    }

    /// Decode a record by FormID or EditorID (auto-detected).
    ///
    /// `resolve` controls FormID field expansion: `"none"` | `"stub"` | `"full"`.
    #[napi]
    pub fn record_by_id(&self, id: String, resolve: String) -> napi::Result<serde_json::Value> {
        let sel = RecordSel::from_input(&id)
            .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
        let depth = parse_resolve_depth(&resolve)?;
        let mut db = self.inner.lock().unwrap();
        let result = match sel {
            RecordSel::FormId(fid) => db.record_by_formid_resolved(fid, depth),
            RecordSel::Edid(edid) => db.record_by_edid_resolved(&edid, depth),
        }
        .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(serde_json::to_value(&result).unwrap())
    }

    /// Return all records that reference the given FormID hex string.
    #[napi]
    pub fn referenced_by(&self, formid: String) -> napi::Result<serde_json::Value> {
        let fid: FormId = formid
            .parse()
            .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
        let mut db = self.inner.lock().unwrap();
        let rows = db
            .referenced_by(fid)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(serde_json::to_value(&rows).unwrap())
    }

    /// Return all records that reference the given FormID or EditorID (auto-detected).
    #[napi]
    pub fn referenced_by_id(&self, id: String) -> napi::Result<serde_json::Value> {
        let sel = RecordSel::from_input(&id)
            .map_err(|e: anyhow::Error| napi::Error::from_reason(format!("{e:#}")))?;
        let mut db = self.inner.lock().unwrap();
        let fid = resolve_sel(&mut db, sel)?;
        let rows = db
            .referenced_by(fid)
            .map_err(|e| napi::Error::from_reason(format!("{e:#}")))?;
        Ok(serde_json::to_value(&rows).unwrap())
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
