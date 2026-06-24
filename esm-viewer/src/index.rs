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
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const CACHE_VERSION: u32 = 7;

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
        let mut file = fs::File::create(&self.cache_path)?;
        file.write_all(&encoded)?;
        Ok(())
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
                record_edid_char: None,
            };
            let fields = decode_record(&ctx, &rec.header.signature, &rec.subrecords);
            let mut refs = Vec::new();
            harvest_formids(&fields, &mut refs);
            for target in refs {
                if target != referencer && form_index.contains_key(&target) {
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
    let xref_index = cache.xref_index.map(|m| {
        m.into_iter()
            .map(|(k, v)| (FormId::new(k), v.into_iter().map(FormId::new).collect()))
            .collect()
    });
    let search_index = cache
        .search_index
        .map(|m| m.into_iter().map(|(k, v)| (FormId::new(k), v)).collect());

    Ok(Some(Index {
        path: esm.path.clone(),
        form_index,
        edid_index,
        tree: cache.tree,
        xref_index,
        search_index,
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
        xref_index: None,
        search_index: None,
        cache_path,
    };
    index.save_cache(esm)?;
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
            for v in map.values() {
                harvest_formids(v, out);
            }
        }
        _ => {}
    }
}
