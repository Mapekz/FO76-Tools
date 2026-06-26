//! Multi-ESM registry: lazily opens and caches [`Database`] instances.

use crate::Database;
use anyhow::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Lazily opened ESM databases keyed by canonical path.
pub struct Registry {
    inner: Mutex<HashMap<PathBuf, Arc<Mutex<Database>>>>,
    /// When true, eagerly build the edid + search indexes on open (daemon behaviour).
    auto_warm: bool,
    /// When true, also eagerly build the xref index (slow, opt-in).
    pub warm_xref: bool,
}

impl Registry {
    /// New registry without auto-warm — used by `LocalBackend` for short-lived processes.
    /// Lazy indexes are still built on demand when an op needs them.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            auto_warm: false,
            warm_xref: false,
        }
    }

    /// New registry for the daemon: auto-warms edid + search on open, and optionally xref.
    pub fn with_warm_xref(warm_xref: bool) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            auto_warm: true,
            warm_xref,
        }
    }

    /// Canonicalize `path`, open the ESM if not already cached, warm indexes,
    /// and return a shared handle.
    ///
    /// The outer map lock is held only long enough to fetch or insert the
    /// `Arc`; the inner `Database` lock is acquired afterward so different ESMs
    /// never serialize on each other.
    pub fn get_or_open(&self, path: &Path) -> anyhow::Result<Arc<Mutex<Database>>> {
        Ok(self.get_or_open_with_key(path)?.1)
    }

    /// Like [`Self::get_or_open`], but also returns the canonical cache key.
    pub fn get_or_open_with_key(
        &self,
        path: &Path,
    ) -> anyhow::Result<(PathBuf, Arc<Mutex<Database>>)> {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()))?;

        if let Some(existing) = {
            let map = self.inner.lock().unwrap();
            map.get(&canonical).cloned()
        } {
            self.warm_indexes(&existing)?;
            return Ok((canonical, existing));
        }

        let db = Database::open(&canonical)?;
        let opened = Arc::new(Mutex::new(db));

        let arc = {
            let mut map = self.inner.lock().unwrap();
            if let Some(existing) = map.get(&canonical) {
                existing.clone()
            } else {
                map.insert(canonical.clone(), opened.clone());
                opened
            }
        };

        self.warm_indexes(&arc)?;

        Ok((canonical, arc))
    }

    fn warm_indexes(&self, db_arc: &Arc<Mutex<Database>>) -> anyhow::Result<()> {
        if !self.auto_warm && !self.warm_xref {
            return Ok(());
        }
        let mut db = db_arc.lock().unwrap();
        let crate::Database {
            esm,
            index,
            is_localized,
            schema,
            localization,
            curves,
            ..
        } = &mut *db;
        index.ensure_edid_index(esm)?;
        index.ensure_search_index(esm, *is_localized)?;
        if self.warm_xref {
            index.ensure_xref_index(
                esm,
                schema,
                *is_localized,
                localization.as_ref(),
                curves.as_ref(),
            )?;
        }
        Ok(())
    }

    /// List resident ESM paths and their record counts (for daemon status).
    pub fn list_resident(&self) -> Vec<ResidentInfo> {
        let map = self.inner.lock().unwrap();
        map.iter()
            .map(|(path, db_arc)| {
                let db = db_arc.lock().unwrap();
                ResidentInfo {
                    path: path.clone(),
                    record_count: db.index.form_index.len(),
                }
            })
            .collect()
    }

    /// Drop all cached databases (used on daemon shutdown).
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    /// Pre-insert a database for unit/integration tests (skips open + warm).
    pub fn insert_for_test(&self, path: PathBuf, db: Arc<Mutex<Database>>) {
        let canonical = path.canonicalize().unwrap_or(path);
        self.inner.lock().unwrap().insert(canonical, db);
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of one ESM held in the registry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResidentInfo {
    pub path: PathBuf,
    pub record_count: usize,
}
