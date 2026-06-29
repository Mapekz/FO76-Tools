use crate::decode::{decode_record, DecodeContext, ResolveDepth};
use crate::formid::{parse_formid, FormId};
use crate::reader::{
    edid_from_subrecords, inline_string_from_subrecords, lstring_id_from_subrecords, EsmFile,
    RecordMeta,
};
use crate::schema::Schema;
use crate::strings::Localization;
use crate::tree::TreeIndex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const CACHE_VERSION: u32 = 9;

/// Per-record data stored in the lazy search index.
///
/// For **localized** ESMs the name and description are stored as lstring IDs
/// (`full_id`, `desc_id`), resolved to text at query time via the active
/// [`Localization`] table.  For **non-localized** ESMs the inline text is
/// stored directly (`full_text`, `desc_text`) so no localization BA2 is needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMeta {
    /// EditorID of the record, if present.
    pub editor_id: Option<String>,
    /// FULL (display name) LString ID for localized ESMs.
    pub full_id: Option<u32>,
    /// DESC (description) LString ID for localized ESMs.
    pub desc_id: Option<u32>,
    /// FULL inline text for non-localized ESMs.
    pub full_text: Option<String>,
    /// DESC inline text for non-localized ESMs.
    pub desc_text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    path: String,
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    form_index: HashMap<u32, RecordMeta>,
    edid_index: Option<HashMap<String, u32>>,
    tree: TreeIndex,
    xref_index: Option<HashMap<u32, Vec<u32>>>,
    search_index: Option<HashMap<u32, SearchMeta>>,
}

#[derive(Debug, Clone)]
pub struct Index {
    pub path: PathBuf,
    pub form_index: HashMap<FormId, RecordMeta>,
    edid_index: Option<HashMap<String, FormId>>,
    pub tree: TreeIndex,
    cache_path: PathBuf,
    xref_index: Option<HashMap<FormId, Vec<FormId>>>,
    search_index: Option<HashMap<FormId, SearchMeta>>,
    type_index: HashMap<String, Vec<FormId>>,
}

impl Index {
    pub fn build(esm: &EsmFile) -> anyhow::Result<Self> {
        if let Some(cached) = try_load_cache(esm)? {
            return Ok(cached);
        }
        build_fresh(esm)
    }

    /// Create an empty index for use with [`crate::Database::open_lite`].
    ///
    /// The index holds no records and must not be persisted to disk — it exists
    /// only as a structural placeholder when the mmap form index is used for
    /// lookups.
    pub fn empty(path: PathBuf) -> Self {
        let cache_path = {
            let mut p = path.clone();
            p.set_extension("esm.idx");
            p
        };
        Self {
            path,
            form_index: HashMap::new(),
            edid_index: None,
            tree: crate::tree::TreeIndex::default(),
            cache_path,
            xref_index: None,
            search_index: None,
            type_index: HashMap::new(),
        }
    }

    pub fn get_by_formid(&self, form_id: FormId) -> Option<&RecordMeta> {
        self.form_index.get(&form_id)
    }

    pub fn get_by_edid(&self, edid: &str) -> Option<FormId> {
        self.edid_index.as_ref()?.get(edid).copied()
    }

    pub fn records_by_type(&self, sig: &str) -> Vec<(FormId, &RecordMeta)> {
        self.type_index
            .get(sig)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.form_index.get(id).map(|m| (*id, m)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn ensure_edid_index(&mut self, esm: &EsmFile) -> anyhow::Result<()> {
        if self.edid_index.is_some() {
            return Ok(());
        }
        let mut edid_index = HashMap::new();
        for (form_id, meta) in &self.form_index {
            let rec = esm.parse_record_at(meta.offset)?;
            if let Some(edid) = edid_from_subrecords(&rec.subrecords) {
                edid_index.insert(edid, *form_id);
            }
        }
        self.edid_index = Some(edid_index);
        self.save_cache(esm)?;
        Ok(())
    }

    fn save_cache(&self, esm: &EsmFile) -> anyhow::Result<()> {
        // Don't persist an empty (lite) index — it would overwrite a valid cache
        // with an empty one.
        if self.form_index.is_empty() {
            return Ok(());
        }
        let meta = fs::metadata(&esm.path)?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let dur = mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

        let form_index: HashMap<u32, RecordMeta> = self
            .form_index
            .iter()
            .map(|(k, v)| (k.raw(), v.clone()))
            .collect();
        let edid_index = self.edid_index.as_ref().map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.raw()))
                .collect::<HashMap<_, _>>()
        });
        let xref_index = self.xref_index.as_ref().map(|m| {
            m.iter()
                .map(|(k, v)| (k.raw(), v.iter().map(|f| f.raw()).collect::<Vec<_>>()))
                .collect::<HashMap<_, _>>()
        });
        let search_index = self.search_index.as_ref().map(|m| {
            m.iter()
                .map(|(k, v)| (k.raw(), v.clone()))
                .collect::<HashMap<_, _>>()
        });

        let cache = CacheFile {
            version: CACHE_VERSION,
            path: esm.path.to_string_lossy().into_owned(),
            size: meta.len(),
            mtime_secs: dur.as_secs(),
            mtime_nanos: dur.subsec_nanos(),
            form_index,
            edid_index,
            tree: self.tree.clone(),
            xref_index,
            search_index,
        };

        let encoded = bincode::serialize(&cache)?;
        // Write to a sidecar temp file first, then rename atomically so a crash
        // mid-write cannot leave a partial (corrupt) cache at the real path.
        let tmp_path = self.cache_path.with_extension("tmp");
        let write_result: anyhow::Result<()> = (|| {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(&encoded)?;
            Ok(())
        })();
        match write_result {
            Ok(()) => fs::rename(&tmp_path, &self.cache_path).map_err(Into::into),
            Err(e) => {
                let _ = fs::remove_file(&tmp_path); // best-effort cleanup
                Err(e)
            }
        }
    }

    /// Build the reverse-reference index on first call, then cache it to disk.
    ///
    /// Walks every record, decodes it with `ResolveDepth::None` (so FormID
    /// fields come out as `"0x........"` hex strings), harvests those strings,
    /// and inverts them into a referencee→referencers map.
    pub fn ensure_xref_index(
        &mut self,
        esm: &EsmFile,
        schema: &Schema,
        is_localized: bool,
        localization: Option<&Localization>,
        curves: Option<&crate::curves::CurveIndex>,
    ) -> anyhow::Result<()> {
        if self.xref_index.is_some() {
            return Ok(());
        }
        let form_index = &self.form_index;
        let mut xref: HashMap<FormId, Vec<FormId>> = HashMap::new();
        esm.walk_records(|meta| {
            let rec = match esm.parse_record_at(meta.offset) {
                Ok(r) => r,
                Err(_) => return Ok(()),
            };
            let referencer = rec.header.form_id;
            if !form_index.contains_key(&referencer) {
                return Ok(());
            }
            let ctx = DecodeContext {
                schema,
                form_version: rec.header.form_version,
                is_localized,
                localization,
                curves,
                resolve_depth: ResolveDepth::None,
                resolver: None,
                outer_struct: None,
                record_signature: None,
                record_edid_char: None,
                scope_min_doc_index: None,
                scope_max_doc_index: None,
            };
            let fields = decode_record(&ctx, &rec.header.signature, &rec.subrecords);
            let mut refs = Vec::new();
            harvest_formids(&fields, &mut refs);
            // Dedup within this record: a single record may reference the same
            // target FormID multiple times (e.g. the same FormID in two
            // separate subrecords, or repeated array entries).  We want each
            // referencing record to appear exactly once per target, regardless
            // of how many times it references it internally.
            let mut seen = HashSet::new();
            for target in refs {
                if target != referencer && form_index.contains_key(&target) && seen.insert(target) {
                    xref.entry(target).or_default().push(referencer);
                }
            }
            Ok(())
        })?;
        self.xref_index = Some(xref);
        self.save_cache(esm)?;
        Ok(())
    }

    /// Return the list of FormIDs that reference the given FormID.
    pub fn get_xref(&self, form_id: FormId) -> &[FormId] {
        self.xref_index
            .as_ref()
            .and_then(|m| m.get(&form_id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Return the lazy search index (if already built).
    pub fn search_index(&self) -> Option<&HashMap<FormId, SearchMeta>> {
        self.search_index.as_ref()
    }

    /// Build the search index on first call, then cache it to disk.
    ///
    /// Iterates every record, extracting the EditorID and name/description
    /// fields.  For **localized** ESMs the FULL and DESC lstring IDs are
    /// stored (resolved to text at query time).  For **non-localized** ESMs
    /// the inline string text is stored directly.
    ///
    /// The result is persisted to the `.esm.idx` cache so subsequent
    /// invocations load in microseconds rather than seconds.
    pub fn ensure_search_index(&mut self, esm: &EsmFile, is_localized: bool) -> anyhow::Result<()> {
        if self.search_index.is_some() {
            return Ok(());
        }
        let mut search_index: HashMap<FormId, SearchMeta> = HashMap::new();
        for (form_id, meta) in &self.form_index {
            let rec = match esm.parse_record_at(meta.offset) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let editor_id = edid_from_subrecords(&rec.subrecords);
            let (full_id, full_text, desc_id, desc_text) = if is_localized {
                (
                    lstring_id_from_subrecords(&rec.subrecords, "FULL"),
                    None,
                    lstring_id_from_subrecords(&rec.subrecords, "DESC"),
                    None,
                )
            } else {
                (
                    None,
                    inline_string_from_subrecords(&rec.subrecords, "FULL"),
                    None,
                    inline_string_from_subrecords(&rec.subrecords, "DESC"),
                )
            };
            // Only store records that have at least one searchable field.
            if editor_id.is_some()
                || full_id.is_some()
                || full_text.is_some()
                || desc_id.is_some()
                || desc_text.is_some()
            {
                search_index.insert(
                    *form_id,
                    SearchMeta {
                        editor_id,
                        full_id,
                        desc_id,
                        full_text,
                        desc_text,
                    },
                );
            }
        }
        self.search_index = Some(search_index);
        self.save_cache(esm)?;
        Ok(())
    }
}

fn build_type_index(form_index: &HashMap<FormId, RecordMeta>) -> HashMap<String, Vec<FormId>> {
    let mut type_index: HashMap<String, Vec<FormId>> = HashMap::new();
    for (id, meta) in form_index {
        type_index
            .entry(meta.signature.clone())
            .or_default()
            .push(*id);
    }
    for ids in type_index.values_mut() {
        ids.sort_by_key(|id| id.raw());
    }
    type_index
}

fn cache_path_for(esm_path: &Path) -> PathBuf {
    let mut p = esm_path.to_path_buf();
    p.set_extension("esm.idx");
    p
}

fn try_load_cache(esm: &EsmFile) -> anyhow::Result<Option<Index>> {
    let cache_path = cache_path_for(&esm.path);
    if !cache_path.exists() {
        return Ok(None);
    }
    let meta = fs::metadata(&esm.path)?;
    // Reject obviously oversized cache files before reading them into RAM.
    // A legitimate .esm.idx is a bincode-serialized HashMap of ~100k records
    // and typically stays well under 300 MiB; anything above 1 GiB is suspect.
    let cache_meta = fs::metadata(&cache_path)?;
    if cache_meta.len() > 1024 * 1024 * 1024 {
        anyhow::bail!(
            "cache file suspiciously large ({}B), refusing to load",
            cache_meta.len()
        );
    }
    let bytes = fs::read(&cache_path)?;
    let cache: CacheFile = match bincode::deserialize(&bytes) {
        Ok(c) => c,
        Err(_) => return Ok(None), // stale or incompatible cache format
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let dur = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    if cache.version != CACHE_VERSION
        || cache.path != esm.path.to_string_lossy()
        || cache.size != meta.len()
        || cache.mtime_secs != dur.as_secs()
        || cache.mtime_nanos != dur.subsec_nanos()
    {
        return Ok(None);
    }

    let form_index: HashMap<FormId, RecordMeta> = cache
        .form_index
        .into_iter()
        .map(|(k, v)| (FormId::new(k), v))
        .collect();
    let edid_index = cache
        .edid_index
        .map(|m| m.into_iter().map(|(k, v)| (k, FormId::new(v))).collect());
    let xref_index = cache.xref_index.map(|m| {
        m.into_iter()
            .map(|(k, v)| (FormId::new(k), v.into_iter().map(FormId::new).collect()))
            .collect()
    });
    let search_index = cache
        .search_index
        .map(|m| m.into_iter().map(|(k, v)| (FormId::new(k), v)).collect());
    let type_index = build_type_index(&form_index);

    Ok(Some(Index {
        path: esm.path.clone(),
        form_index,
        edid_index,
        tree: cache.tree,
        xref_index,
        search_index,
        cache_path,
        type_index,
    }))
}

fn build_fresh(esm: &EsmFile) -> anyhow::Result<Index> {
    let mut form_index = HashMap::new();
    esm.walk_records(|meta| {
        let data = esm.data();
        let rh = crate::format::RecordHeader::parse(&data[meta.offset as usize..])?;
        let form_id = FormId::new(rh.form_id);
        form_index.insert(form_id, meta);
        Ok(())
    })?;

    let tree = TreeIndex::build(esm)?;
    let type_index = build_type_index(&form_index);

    let cache_path = cache_path_for(&esm.path);
    let index = Index {
        path: esm.path.clone(),
        form_index,
        edid_index: None,
        tree,
        xref_index: None,
        search_index: None,
        cache_path,
        type_index,
    };
    index.save_cache(esm)?;

    // Opportunistically write the compact mmap index alongside the .idx so
    // that `Database::open_lite` / `--mmap-index` paths are always ready.
    if let Err(e) = crate::mindex::build_from_form_index_and_save(&index.form_index, &esm.path) {
        eprintln!("Warning: failed to write .esm.midx: {e}");
    }

    Ok(index)
}

pub fn full_name_for_record(esm: &EsmFile, meta: &RecordMeta) -> anyhow::Result<Option<u32>> {
    let rec = esm.parse_record_at(meta.offset)?;
    Ok(lstring_id_from_subrecords(&rec.subrecords, "FULL"))
}

/// Recursively walk a decoded JSON value and collect every string that looks
/// like a FormID hex literal (`"0x........"`).
fn harvest_formids(val: &Value, out: &mut Vec<FormId>) {
    match val {
        Value::String(s) => {
            if s.starts_with("0x") || s.starts_with("0X") {
                if let Ok(fid) = parse_formid(s) {
                    out.push(fid);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                harvest_formids(v, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                if !k.starts_with('_') {
                    harvest_formids(v, out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Verify that `try_load_cache` rejects a cache file whose on-disk size
    /// exceeds 1 GiB without reading the file into RAM.
    ///
    /// The cache file is created as a sparse file (no actual disk allocation for
    /// the hole), so the test completes quickly on any POSIX filesystem.
    /// The ESM stub is a 4-byte file — `EsmFile::open` only needs a non-empty
    /// file it can mmap; the content is irrelevant here.
    #[test]
    fn try_load_cache_rejects_oversized_cache_file() -> anyhow::Result<()> {
        let tmp_dir = std::env::temp_dir();
        let pid = std::process::id();
        let esm_path = tmp_dir.join(format!("fo76_idx_size_test_{pid}.esm"));
        let cache_path = {
            let mut p = esm_path.clone();
            p.set_extension("esm.idx");
            p
        };

        // Minimal non-empty ESM stub for mmap.
        {
            let mut f = fs::File::create(&esm_path)?;
            f.write_all(b"TEST")?;
        }

        // Sparse file > 1 GiB — the OS allocates no physical blocks for the hole.
        {
            let f = fs::File::create(&cache_path)?;
            f.set_len(1024 * 1024 * 1024 + 1)?;
        }

        let esm = crate::reader::EsmFile::open(&esm_path)?;
        let result = try_load_cache(&esm);

        let _ = fs::remove_file(&esm_path);
        let _ = fs::remove_file(&cache_path);

        assert!(
            result.is_err(),
            "expected error for oversized cache file, got Ok"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("suspiciously large"),
            "unexpected error message: {msg}"
        );
        Ok(())
    }

    /// Verify that `records_by_type` uses the pre-built type_index and returns
    /// a deterministic sorted order on repeated calls.
    #[test]
    fn records_by_type_sorted_and_stable() {
        use crate::reader::RecordMeta;

        let weap1 = FormId::new(0x0000_0010);
        let weap2 = FormId::new(0x0000_0005);
        let npc_ = FormId::new(0x0000_0020);

        let mut form_index = HashMap::new();
        form_index.insert(
            weap1,
            RecordMeta {
                offset: 0,
                signature: "WEAP".into(),
                flags: 0,
                form_version: 155,
            },
        );
        form_index.insert(
            weap2,
            RecordMeta {
                offset: 100,
                signature: "WEAP".into(),
                flags: 0,
                form_version: 155,
            },
        );
        form_index.insert(
            npc_,
            RecordMeta {
                offset: 200,
                signature: "NPC_".into(),
                flags: 0,
                form_version: 155,
            },
        );

        let type_index = build_type_index(&form_index);
        let cache_path = std::path::PathBuf::from("/tmp/test.esm.idx");
        let index = Index {
            path: std::path::PathBuf::from("/tmp/test.esm"),
            form_index,
            edid_index: None,
            tree: crate::tree::TreeIndex::default(),
            cache_path,
            xref_index: None,
            search_index: None,
            type_index,
        };

        // First call
        let first = index.records_by_type("WEAP");
        // Second call — must return same order
        let second = index.records_by_type("WEAP");

        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        // Pre-sorted by FormId::raw() ascending: weap2 (0x05) < weap1 (0x10)
        assert_eq!(first[0].0, weap2);
        assert_eq!(first[1].0, weap1);
        assert_eq!(first[0].0, second[0].0, "order must be stable across calls");
        assert_eq!(first[1].0, second[1].0, "order must be stable across calls");

        // NPC_ should return exactly one record
        let npc_records = index.records_by_type("NPC_");
        assert_eq!(npc_records.len(), 1);
        assert_eq!(npc_records[0].0, npc_);

        // Unknown type returns empty
        assert!(index.records_by_type("XXXX").is_empty());
    }
}
