use crate::formid::FormId;
use crate::reader::{edid_from_subrecords, lstring_id_from_subrecords, EsmFile, RecordMeta};
use crate::tree::TreeIndex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const CACHE_VERSION: u32 = 2;

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
}

#[derive(Debug, Clone)]
pub struct Index {
    pub path: PathBuf,
    pub form_index: HashMap<FormId, RecordMeta>,
    edid_index: Option<HashMap<String, FormId>>,
    pub tree: TreeIndex,
    cache_path: PathBuf,
}

impl Index {
    pub fn build(esm: &EsmFile) -> anyhow::Result<Self> {
        if let Some(cached) = try_load_cache(esm)? {
            return Ok(cached);
        }
        build_fresh(esm)
    }

    pub fn get_by_formid(&self, form_id: FormId) -> Option<&RecordMeta> {
        self.form_index.get(&form_id)
    }

    pub fn get_by_edid(&self, edid: &str) -> Option<FormId> {
        self.edid_index.as_ref()?.get(edid).copied()
    }

    pub fn records_by_type(&self, sig: &str) -> Vec<(FormId, &RecordMeta)> {
        let mut out: Vec<_> = self
            .form_index
            .iter()
            .filter(|(_, m)| m.signature == sig)
            .map(|(id, m)| (*id, m))
            .collect();
        out.sort_by_key(|(id, _)| id.raw());
        out
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

        let cache = CacheFile {
            version: CACHE_VERSION,
            path: esm.path.to_string_lossy().into_owned(),
            size: meta.len(),
            mtime_secs: dur.as_secs(),
            mtime_nanos: dur.subsec_nanos(),
            form_index,
            edid_index,
            tree: self.tree.clone(),
        };

        let encoded = bincode::serialize(&cache)?;
        let mut file = fs::File::create(&self.cache_path)?;
        file.write_all(&encoded)?;
        Ok(())
    }
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

    let form_index = cache
        .form_index
        .into_iter()
        .map(|(k, v)| (FormId::new(k), v))
        .collect();
    let edid_index = cache
        .edid_index
        .map(|m| m.into_iter().map(|(k, v)| (k, FormId::new(v))).collect());

    Ok(Some(Index {
        path: esm.path.clone(),
        form_index,
        edid_index,
        tree: cache.tree,
        cache_path,
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

    let cache_path = cache_path_for(&esm.path);
    let index = Index {
        path: esm.path.clone(),
        form_index,
        edid_index: None,
        tree,
        cache_path,
    };
    index.save_cache(esm)?;
    Ok(index)
}

pub fn full_name_for_record(esm: &EsmFile, meta: &RecordMeta) -> anyhow::Result<Option<u32>> {
    let rec = esm.parse_record_at(meta.offset)?;
    Ok(lstring_id_from_subrecords(&rec.subrecords, "FULL"))
}
