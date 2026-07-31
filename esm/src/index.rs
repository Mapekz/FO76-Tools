use crate::decode::{DecodeContext, ResolveDepth, decode_record};
use crate::format::Signature;
use crate::formid::{FormId, parse_formid};
use crate::reader::{
    EsmFile, RecordMeta, edid_from_subrecords, inline_string_from_subrecords,
    lstring_id_from_subrecords,
};
use crate::rkyvcache::{CacheSig, Section, SectionKind, write_section};
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

// Bumped 9 -> 10: fixed the VMAD Object-property FormID offset (decode.rs
// `read_scalar` base type 1) — it was reading the wrong 4 bytes of the 8-byte
// union for the common `obj_format == 2` case, so xref edges sourced from
// script properties (e.g. a MGEF's "apply effect" script pointing at its
// SPEL) were silently dropped. Bump forces a rebuild so `refs` picks up the
// now-correct FormIDs.
//
// Bumped 10 -> 11: TERM's VMAD (wbVMADFragmentedPERK per xEdit) was decoded
// with the generic `decode_vmad`, which stops after the base scripts array
// and never parses the fragment tail — so a terminal's prize `Form_*` script
// properties (e.g. a prize terminal's shirt/weapon grants) were invisible to
// `harvest_formids` and dropped from `refs`. Now dispatched to
// `decode_vmad_perk`. Bump forces a rebuild so `refs` picks up the newly
// decoded TERM tail FormIDs.
//
// Bumped 11 -> 12: no layout change, but the encoder did. bincode 1 -> 2 moved
// the default integer encoding from fixed-width to varint, so a v11 file and a
// v12 file with identical contents are different bytes. `try_load_cache`
// already treats a decode error as "no usable cache", so this bump is
// defence-in-depth: it rejects old files by version rather than relying on the
// new decoder to fail on old bytes, which a dense binary format could
// conceivably mis-parse without erroring.
//
// Bumped 12 -> 13: `TreeIndex` moved out of the bincode `CacheFile` blob into
// its own rkyv-backed `.esm.tree` section (see `rkyvcache.rs` / `tree.rs`'s
// `TreeView`) — `CacheFile` lost its `tree` field, changing the bincode byte
// shape. `try_load_cache` already treats a decode error as "no usable cache",
// but a removed field (rather than a changed type) is exactly the kind of
// change a length-prefixed/varint format like bincode 2 could conceivably
// mis-parse into a structurally different-but-plausible `CacheFile` without
// erroring, so this bump rejects a pre-this-change `.esm.idx` by version
// check rather than leaving that to chance. This section's own
// `.esm.tree` file carries its own independent version/layout-fingerprint
// check (see `Section::map`), gated on this same `CACHE_VERSION` constant —
// see `try_load_cache`/`build_fresh` below.
//
// Bumped 13 -> 14: `form_index` (the FormID -> `RecordMeta` table) and the
// derived type directory (previously `type_index`, rebuilt from
// `form_index` on every single load via `build_type_index`) moved out of the
// bincode `CacheFile` blob into their own combined rkyv-backed `.esm.forms`
// section (see `rkyvcache.rs` / this file's `FormsSection`,
// `try_load_cache`, `build_fresh`) — `CacheFile` lost its `form_index`
// field, changing the bincode byte shape the same way Stage 4's `tree`
// removal did above. `form_index` held ~5.64M entries and was the bulk of
// this crate's cold-load cost; it and the type directory are now read
// zero-copy via `rkyv::access_unchecked`, the same mechanism `tree` already
// used. This section's own `.esm.forms` file carries its own independent
// version/layout-fingerprint check (see `Section::map`), gated on this same
// `CACHE_VERSION` constant — see `try_load_cache`/`build_fresh` below.
const CACHE_VERSION: u32 = 14;

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
    edid_index: Option<HashMap<String, u32>>,
    xref_index: Option<HashMap<u32, Vec<u32>>>,
    search_index: Option<HashMap<u32, SearchMeta>>,
}

/// One combined rkyv section: the sorted FormID→[`RecordMeta`] table plus
/// the type directory, always built and written together (mirrors how
/// they're already always rebuilt together in `build_fresh`/
/// `build_type_index`) — see `rkyvcache.rs` for the section mechanics this
/// is built on, and `tree.rs`'s `TreeIndex` for the prior-stage type this
/// one's plumbing mirrors.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct FormsSection {
    /// Sorted ascending by `.0` (the raw FormID `u32`) — binary-searchable.
    /// A sorted `Vec`, not a `HashMap`: an `ArchivedHashMap<u32, RecordMeta>`
    /// measures ~37.7 B/entry (Swiss-table control bytes + empty slots) vs.
    /// a sorted `Vec`'s ~28 B/entry at 5.64M entries — a ~55 MB difference
    /// for no behavioral benefit, since lookup is O(log n) either way.
    records: Vec<(u32, RecordMeta)>,
    /// Type signature (raw `[u8; 4]` bytes, not the `Signature` newtype
    /// directly — see the module-level note on that choice) -> sorted
    /// FormIDs of that type. Only ~178 distinct keys in practice, so —
    /// unlike `records` — the HashMap-vs-Vec density argument doesn't
    /// apply here; a plain `HashMap` is simplest and the entry count makes
    /// any overhead irrelevant.
    types: HashMap<[u8; 4], Vec<u32>>,
}

/// FNV-1a fingerprint of this section's archived layout, folding
/// `size_of`/`align_of` per [`crate::rkyvcache::fnv1a_u64`]'s doc comment.
/// Passed as the `layout_fingerprint` argument to `write_section`/
/// `Section::map` for the `.esm.forms` section — see `try_load_cache`/
/// `build_fresh` below.
///
/// `RecordMeta` is folded in separately from `FormsSection` itself because
/// it sits behind a `Vec` indirection (`records: Vec<(u32, RecordMeta)>`) —
/// a layout change to `RecordMeta` (e.g. one driven by a layout change to
/// `Signature`, which it embeds inline) would not change
/// `size_of::<Archived<FormsSection>>()` itself, the same reasoning
/// `tree.rs`'s `TREE_LAYOUT_FINGERPRINT` documents for folding in
/// `GroupEntry`/`ChildRef` alongside `TreeIndex`.
const FORMS_LAYOUT_FINGERPRINT: u64 = {
    use crate::rkyvcache::{FNV_OFFSET_BASIS, fnv1a_u64};

    let acc = fnv1a_u64(
        FNV_OFFSET_BASIS,
        core::mem::size_of::<rkyv::Archived<FormsSection>>() as u64,
    );
    let acc = fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<FormsSection>>() as u64,
    );
    let acc = fnv1a_u64(
        acc,
        core::mem::size_of::<rkyv::Archived<RecordMeta>>() as u64,
    );
    fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<RecordMeta>>() as u64,
    )
};

/// # Not `Clone`; `Debug` is hand-written
///
/// `tree`/`forms`'s `Section`s wrap a `Mmap`, which implements neither
/// trait. Nothing in this crate needs `Index: Clone` (`Database`, which owns
/// one, doesn't derive it either; `Registry` shares `Database` instances via
/// `Arc<Mutex<Database>>`, cloning the `Arc`, never the `Database`/`Index`
/// itself) — but at least one existing test does format an `Index`-carrying
/// `Result` via `unwrap_err`, so `Debug` is implemented manually below,
/// summarizing `tree`/`forms` as just their mapped/absent state rather than
/// requiring `Section<A>: Debug`.
pub struct Index {
    pub path: PathBuf,
    edid_index: Option<HashMap<String, FormId>>,
    tree: Section<rkyv::Archived<TreeIndex>>,
    forms: Section<rkyv::Archived<FormsSection>>,
    cache_path: PathBuf,
    xref_index: Option<HashMap<FormId, Vec<FormId>>>,
    search_index: Option<HashMap<FormId, SearchMeta>>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index")
            .field("path", &self.path)
            .field("forms_mapped", &self.forms.is_mapped())
            .field("edid_index_built", &self.edid_index.is_some())
            .field("tree_mapped", &self.tree.is_mapped())
            .field("cache_path", &self.cache_path)
            .field("xref_index_built", &self.xref_index.is_some())
            .field("search_index_built", &self.search_index.is_some())
            .finish()
    }
}

/// Borrowed view over one search entry. Exists so [`crate::Database::search`]
/// never has to clone/allocate just to test whether an entry matches — every
/// field is a borrow (or a plain `Copy` scalar), valid for as long as the
/// [`Index`] this came from.
#[derive(Debug, Clone, Copy)]
pub struct SearchRef<'a> {
    pub editor_id: Option<&'a str>,
    pub full_id: Option<u32>,
    pub desc_id: Option<u32>,
    pub full_text: Option<&'a str>,
    pub desc_text: Option<&'a str>,
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
            edid_index: None,
            tree: Section::Absent,
            forms: Section::Absent,
            cache_path,
            xref_index: None,
            search_index: None,
        }
    }

    /// Resolve a FormID to its [`RecordMeta`]. Owned, not borrowed — cheap
    /// since `RecordMeta` is `Copy`. `None` if the forms section is absent
    /// (lite mode / no cache built yet) or `form_id` isn't present.
    pub fn get_by_formid(&self, form_id: FormId) -> Option<RecordMeta> {
        let records = self.forms.get()?.records.as_slice();
        let idx = records
            .binary_search_by_key(&form_id.raw(), |entry| entry.0.to_native())
            .ok()?;
        Some(owned_record_meta(&records[idx].1))
    }

    /// Alloc-free type lookup: resolve a FormID to just its [`Signature`]
    /// without fetching the whole [`RecordMeta`] — this is the accessor
    /// [`crate::Database::search`]'s type-filter branch relies on to avoid
    /// paying for the rest of `RecordMeta` when only the type tag is needed.
    pub fn signature_of(&self, form_id: FormId) -> Option<Signature> {
        let records = self.forms.get()?.records.as_slice();
        let idx = records
            .binary_search_by_key(&form_id.raw(), |entry| entry.0.to_native())
            .ok()?;
        Some(Signature(records[idx].1.signature.0))
    }

    /// Whether `form_id` is present in the form index. `false` if the forms
    /// section is absent (lite mode / no cache built yet).
    pub fn contains(&self, form_id: FormId) -> bool {
        self.forms.get().is_some_and(|f| {
            f.records
                .binary_search_by_key(&form_id.raw(), |entry| entry.0.to_native())
                .is_ok()
        })
    }

    /// Total number of records in the form index. `0` if the forms section
    /// is absent (lite mode / no cache built yet).
    pub fn len(&self) -> usize {
        self.forms.get().map_or(0, |f| f.records.len())
    }

    pub fn is_empty(&self) -> bool {
        self.forms.get().is_none_or(|f| f.records.is_empty())
    }

    /// Iterate every FormID present in the form index, in on-disk
    /// (FormID-sorted) order. Empty iterator if the forms section is absent
    /// (lite mode / no cache built yet).
    pub fn iter_form_ids(&self) -> impl Iterator<Item = FormId> + '_ {
        let records: &[_] = self.forms.get().map_or(&[][..], |f| f.records.as_slice());
        records.iter().map(|entry| FormId::new(entry.0.to_native()))
    }

    /// Iterate every `(FormId, RecordMeta)` pair, in on-disk (FormID-sorted)
    /// order. Internal helper for [`Self::ensure_edid_index`]/
    /// [`Self::ensure_search_index`], which — unlike [`Self::iter_form_ids`]
    /// — need the full `RecordMeta` (specifically `.offset`) alongside each
    /// FormID to parse the record. Empty iterator if the forms section is
    /// absent, same degrade-gracefully contract as every other accessor
    /// here.
    fn iter_all(&self) -> impl Iterator<Item = (FormId, RecordMeta)> + '_ {
        let records: &[_] = self.forms.get().map_or(&[][..], |f| f.records.as_slice());
        records.iter().map(|entry| {
            (
                FormId::new(entry.0.to_native()),
                owned_record_meta(&entry.1),
            )
        })
    }

    pub fn get_by_edid(&self, edid: &str) -> Option<FormId> {
        self.edid_index.as_ref()?.get(edid).copied()
    }

    /// Iterate every distinct record-type [`Signature`] present in the
    /// file — replaces "collect every record's signature into a `HashSet`"
    /// callers, since the type directory's keys are already that distinct
    /// set. Empty iterator if the forms section is absent.
    pub fn signatures(&self) -> impl Iterator<Item = Signature> + '_ {
        self.forms
            .get()
            .into_iter()
            .flat_map(|f| f.types.keys().map(|bytes| Signature(*bytes)))
    }

    /// Number of records of the given 4-character type signature, without
    /// materializing a full `Vec`/iterator of them. `0` if the forms section
    /// is absent or the type has no records.
    pub fn count_by_type(&self, sig: &str) -> usize {
        let key = Signature::from_slice(sig.as_bytes()).0;
        self.forms
            .get()
            .and_then(|f| f.types.get(&key))
            .map_or(0, |ids| ids.len())
    }

    /// FormIDs of the given 4-character type signature, sorted (per
    /// `build_type_index`/`build_fresh`). Alloc-free beyond the returned
    /// iterator itself — does not fetch each record's [`RecordMeta`]. Empty
    /// iterator if the forms section is absent or the type has no records.
    pub fn form_ids_by_type(&self, sig: &str) -> impl ExactSizeIterator<Item = FormId> + '_ {
        let key = Signature::from_slice(sig.as_bytes()).0;
        let ids: &[_] = self
            .forms
            .get()
            .and_then(|f| f.types.get(&key))
            .map_or(&[][..], |v| v.as_slice());
        ids.iter().map(|id| FormId::new(id.to_native()))
    }

    /// `(FormId, RecordMeta)` pairs for every record of the given
    /// 4-character type signature, FormID-sorted. Owned pairs — cheap since
    /// `RecordMeta` is `Copy`. Empty iterator if the forms section is absent
    /// or the type has no records.
    pub fn records_by_type(&self, sig: &str) -> impl Iterator<Item = (FormId, RecordMeta)> + '_ {
        let key = Signature::from_slice(sig.as_bytes()).0;
        let ids: &[_] = self
            .forms
            .get()
            .and_then(|f| f.types.get(&key))
            .map_or(&[][..], |v| v.as_slice());
        ids.iter().filter_map(move |id| {
            let form_id = FormId::new(id.to_native());
            self.get_by_formid(form_id).map(|meta| (form_id, meta))
        })
    }

    pub fn ensure_edid_index(&mut self, esm: &EsmFile) -> anyhow::Result<()> {
        if self.edid_index.is_some() {
            return Ok(());
        }
        let mut edid_index = HashMap::new();
        for (form_id, meta) in self.iter_all() {
            let rec = esm.parse_record_at(meta.offset)?;
            if let Some(edid) = edid_from_subrecords(&rec.subrecords) {
                edid_index.insert(edid, form_id);
            }
        }
        self.edid_index = Some(edid_index);
        self.save_cache(esm)?;
        Ok(())
    }

    fn save_cache(&self, esm: &EsmFile) -> anyhow::Result<()> {
        // Don't persist an empty (lite) index — it would overwrite a valid cache
        // with an empty one.
        if self.is_empty() {
            return Ok(());
        }
        let meta = fs::metadata(&esm.path)?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let dur = mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

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
            edid_index,
            xref_index,
            search_index,
        };

        let encoded = bincode::serde::encode_to_vec(&cache, bincode::config::standard())?;
        // Write to a sidecar temp file first, then rename atomically so a crash
        // mid-write cannot leave a partial (corrupt) cache at the real path.
        let tmp_path = unique_tmp_path(&self.cache_path)?;
        let write_result: anyhow::Result<()> = (|| {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
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
        // Reborrow immutably up front (same pattern the pre-section version
        // used for `form_index`) so the closure below can query the forms
        // section without conflicting with `self.xref_index = ...` after
        // `walk_records` returns.
        let index = &*self;
        let mut xref: HashMap<FormId, Vec<FormId>> = HashMap::new();
        esm.walk_records(|meta| {
            let rec = match esm.parse_record_at(meta.offset) {
                Ok(r) => r,
                Err(_) => return Ok(()),
            };
            let referencer = rec.header.form_id;
            if !index.contains(referencer) {
                return Ok(());
            }
            let ctx = DecodeContext::for_record(
                schema,
                rec.header.form_version,
                is_localized,
                localization,
                curves,
                ResolveDepth::None,
                None,
            );
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
                if target != referencer && index.contains(target) && seen.insert(target) {
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
    pub fn get_xref(&self, form_id: FormId) -> Vec<FormId> {
        self.xref_index
            .as_ref()
            .and_then(|m| m.get(&form_id))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether the lazy search index has been built yet.
    pub fn has_search_index(&self) -> bool {
        self.search_index.is_some()
    }

    /// Iterate the lazy search index (if already built) as borrowed
    /// [`SearchRef`] views — empty iterator if not yet built.
    pub fn iter_search(&self) -> impl Iterator<Item = (FormId, SearchRef<'_>)> + '_ {
        self.search_index.iter().flat_map(|m| {
            m.iter().map(|(id, meta)| {
                (
                    *id,
                    SearchRef {
                        editor_id: meta.editor_id.as_deref(),
                        full_id: meta.full_id,
                        desc_id: meta.desc_id,
                        full_text: meta.full_text.as_deref(),
                        desc_text: meta.desc_text.as_deref(),
                    },
                )
            })
        })
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
        for (form_id, meta) in self.iter_all() {
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
                    form_id,
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

    /// Borrowed view over the GRUP tree — replaces the removed `pub tree`
    /// field. See [`crate::tree::TreeView`]. `Section::get` already returns
    /// `Option<&Archived<TreeIndex>>`, exactly what `TreeView::new` expects —
    /// `Section::Absent` (lite mode, or no cache built yet) degrades to an
    /// empty-equivalent `TreeView` rather than a panic.
    pub fn tree(&self) -> crate::tree::TreeView<'_> {
        crate::tree::TreeView::new(self.tree.get())
    }
}

fn build_type_index(form_index: &HashMap<FormId, RecordMeta>) -> HashMap<Signature, Vec<FormId>> {
    let mut type_index: HashMap<Signature, Vec<FormId>> = HashMap::new();
    for (id, meta) in form_index {
        type_index.entry(meta.signature).or_default().push(*id);
    }
    for ids in type_index.values_mut() {
        ids.sort_by_key(|id| id.raw());
    }
    type_index
}

/// Convert one archived `RecordMeta` entry back to owned — hand-rolled
/// rather than `rkyv::Deserialize`, the same reasoning `tree.rs`'s
/// `owned_child_ref` documents: `RecordMeta` is small, has no allocations to
/// speak of, and this avoids threading a fallible deserializer through what
/// is otherwise an infallible conversion.
fn owned_record_meta(archived: &rkyv::Archived<RecordMeta>) -> RecordMeta {
    RecordMeta {
        offset: archived.offset.to_native(),
        signature: Signature(archived.signature.0),
        flags: archived.flags.to_native(),
        form_version: archived.form_version.to_native(),
    }
}

/// Build a unique temp path next to `base`, e.g. `SeventySix.esm.idx.tmp.<16 hex>`.
pub(crate) fn unique_tmp_path(base: &Path) -> anyhow::Result<PathBuf> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)?;
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    let parent = base
        .parent()
        .ok_or_else(|| anyhow::anyhow!("base path has no parent: {}", base.display()))?;
    let mut name = base
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("base path has no file name: {}", base.display()))?
        .to_os_string();
    name.push(".tmp.");
    name.push(hex);
    Ok(parent.join(name))
}

fn cache_path_for(esm_path: &Path) -> PathBuf {
    let mut p = esm_path.to_path_buf();
    p.set_extension("esm.idx");
    p
}

/// Return the `.esm.tree` rkyv-section path for a given ESM path — same
/// `set_extension` pattern as [`cache_path_for`] (and `mindex.rs`'s
/// `midx_path_for`).
fn tree_path_for(esm_path: &Path) -> PathBuf {
    let mut p = esm_path.to_path_buf();
    p.set_extension("esm.tree");
    p
}

/// Return the `.esm.forms` rkyv-section path for a given ESM path — same
/// `set_extension` pattern as [`cache_path_for`]/[`tree_path_for`].
fn forms_path_for(esm_path: &Path) -> PathBuf {
    let mut p = esm_path.to_path_buf();
    p.set_extension("esm.forms");
    p
}

fn try_load_cache(esm: &EsmFile) -> anyhow::Result<Option<Index>> {
    let cache_path = cache_path_for(&esm.path);
    if !cache_path.exists() {
        return Ok(None);
    }
    let meta = fs::metadata(&esm.path)?;
    // Reject obviously oversized cache files before reading them into RAM.
    // A legitimate .esm.idx is a bincode-serialized HashMap of ~5.6M records
    // and typically stays well under 300 MiB; anything above 1 GiB is suspect.
    let cache_meta = fs::metadata(&cache_path)?;
    if cache_meta.len() > 1024 * 1024 * 1024 {
        anyhow::bail!(
            "cache file suspiciously large ({}B), refusing to load",
            cache_meta.len()
        );
    }
    let bytes = fs::read(&cache_path)?;
    let cache: CacheFile =
        match bincode::serde::decode_from_slice(&bytes, bincode::config::standard()) {
            Ok((c, _)) => c,
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

    // Since Stage 4/5, `TreeIndex` and `form_index`+the type directory no
    // longer live in the bincode `CacheFile` blob above — they're their own
    // rkyv-backed `.esm.tree`/`.esm.forms` sections. The cache as a whole is
    // only usable if ALL THREE pieces load/validate against the same ESM
    // identity; any one failing means a full rebuild of everything, exactly
    // matching this function's pre-existing "any piece invalid -> rebuild
    // everything" behavior (now checking three things instead of one).
    let sig = CacheSig {
        size: meta.len(),
        mtime_secs: dur.as_secs(),
        mtime_nanos: dur.subsec_nanos(),
    };
    let tree_section = Section::<rkyv::Archived<TreeIndex>>::map(
        &tree_path_for(&esm.path),
        SectionKind::Tree,
        sig,
        CACHE_VERSION,
        crate::tree::TREE_LAYOUT_FINGERPRINT,
    )?;
    if !tree_section.is_mapped() {
        return Ok(None);
    }
    let forms_section = Section::<rkyv::Archived<FormsSection>>::map(
        &forms_path_for(&esm.path),
        SectionKind::Forms,
        sig,
        CACHE_VERSION,
        FORMS_LAYOUT_FINGERPRINT,
    )?;
    if !forms_section.is_mapped() {
        return Ok(None);
    }

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
        edid_index,
        tree: tree_section,
        forms: forms_section,
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
    let type_index = build_type_index(&form_index);

    let cache_path = cache_path_for(&esm.path);
    let tree_path = tree_path_for(&esm.path);
    let forms_path = forms_path_for(&esm.path);
    let sig = CacheSig::read(&esm.path)?;

    // Opportunistically write the compact mmap index alongside the .idx so
    // that `Database::open_lite` / `--mmap-index` paths are always ready.
    // Must happen here, using the LOCAL, still-owned `form_index` map:
    // `Index` no longer keeps an owned `HashMap<FormId, RecordMeta>` field
    // for a later call site to reach through (it now holds only the mapped
    // `.esm.forms` section) — so this has to run before `form_index` is
    // consumed/dropped below, not after `Index` is constructed like the
    // pre-this-task code did. `mindex.rs` itself is untouched by this task
    // (see the scope note in the commit this belongs to) — it keeps working
    // as a second, independent FormID index built from this same data.
    if let Err(e) = crate::mindex::build_from_form_index_and_save(&form_index, &esm.path) {
        log::warn!("failed to write .esm.midx: {e}");
    }

    // Write the freshly-built tree to its own rkyv section, then drop the
    // owned value and map it straight back in — there is exactly one code
    // path for how `Index` ever reads tree data afterwards (through the
    // archived section via `Index::tree()`), never a second one that keeps
    // the owned `TreeIndex` around.
    write_section(
        &tree_path,
        SectionKind::Tree,
        sig,
        CACHE_VERSION,
        crate::tree::TREE_LAYOUT_FINGERPRINT,
        &tree,
    )?;
    drop(tree);
    let tree_section = Section::<rkyv::Archived<TreeIndex>>::map(
        &tree_path,
        SectionKind::Tree,
        sig,
        CACHE_VERSION,
        crate::tree::TREE_LAYOUT_FINGERPRINT,
    )?;
    anyhow::ensure!(
        tree_section.is_mapped(),
        "just-written tree section at {} failed to map back — this should never happen",
        tree_path.display()
    );

    // Build the combined forms section (sorted FormID table + type
    // directory) from the local owned `form_index`/`type_index`, write it,
    // drop the owned data, and map it straight back in — same
    // write→drop→re-map protocol as `tree` above.
    let mut records: Vec<(u32, RecordMeta)> = form_index
        .iter()
        .map(|(id, meta)| (id.raw(), *meta))
        .collect();
    records.sort_unstable_by_key(|(raw_id, _)| *raw_id);
    let types: HashMap<[u8; 4], Vec<u32>> = type_index
        .into_iter()
        .map(|(sig, ids)| (sig.0, ids.into_iter().map(|id| id.raw()).collect()))
        .collect();
    drop(form_index);
    let forms_data = FormsSection { records, types };
    write_section(
        &forms_path,
        SectionKind::Forms,
        sig,
        CACHE_VERSION,
        FORMS_LAYOUT_FINGERPRINT,
        &forms_data,
    )?;
    drop(forms_data);
    let forms_section = Section::<rkyv::Archived<FormsSection>>::map(
        &forms_path,
        SectionKind::Forms,
        sig,
        CACHE_VERSION,
        FORMS_LAYOUT_FINGERPRINT,
    )?;
    anyhow::ensure!(
        forms_section.is_mapped(),
        "just-written forms section at {} failed to map back — this should never happen",
        forms_path.display()
    );

    let index = Index {
        path: esm.path.clone(),
        edid_index: None,
        tree: tree_section,
        forms: forms_section,
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
            if (s.starts_with("0x") || s.starts_with("0X"))
                && let Ok(fid) = parse_formid(s)
            {
                out.push(fid);
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

    /// Arbitrary, test-only `cache_version` — these tests exercise the
    /// `write_section`/`Section::map` mechanics against a real `FormsSection`,
    /// not this module's real `CACHE_VERSION` (which stays private to
    /// `try_load_cache`/`build_fresh`).
    const TEST_CACHE_VERSION: u32 = 5252;

    /// Distinct, non-colliding temp `.esm.forms` path per test — same
    /// precedent as `rkyvcache.rs`'s `test_path`/`tree.rs`'s round-trip test,
    /// suffixed with pid+nonce so parallel/sequential `cargo test` runs never
    /// collide on the same file.
    fn test_forms_path(name: &str) -> PathBuf {
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("esm_index_test_{name}_{pid}_{nonce}.esm.forms"))
    }

    /// Build a `FormsSection` from `form_index` (mirroring `build_fresh`'s
    /// own conversion), write it via `write_section`, and map it back via
    /// `Section::map`. Shared boilerplate for every forms-section test below.
    fn write_and_map_forms(
        form_index: &HashMap<FormId, RecordMeta>,
        path: &Path,
    ) -> Section<rkyv::Archived<FormsSection>> {
        let type_index = build_type_index(form_index);
        let mut records: Vec<(u32, RecordMeta)> = form_index
            .iter()
            .map(|(id, meta)| (id.raw(), *meta))
            .collect();
        records.sort_unstable_by_key(|(raw_id, _)| *raw_id);
        let types: HashMap<[u8; 4], Vec<u32>> = type_index
            .into_iter()
            .map(|(sig, ids)| (sig.0, ids.into_iter().map(|id| id.raw()).collect()))
            .collect();
        let data = FormsSection { records, types };

        let sig = CacheSig {
            size: 123_456,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 0,
        };
        write_section(
            path,
            SectionKind::Forms,
            sig,
            TEST_CACHE_VERSION,
            FORMS_LAYOUT_FINGERPRINT,
            &data,
        )
        .expect("write forms section");
        let section = Section::<rkyv::Archived<FormsSection>>::map(
            path,
            SectionKind::Forms,
            sig,
            TEST_CACHE_VERSION,
            FORMS_LAYOUT_FINGERPRINT,
        )
        .expect("map forms section");
        assert!(section.is_mapped(), "freshly written section must map back");
        section
    }

    fn index_over(forms: Section<rkyv::Archived<FormsSection>>) -> Index {
        Index {
            path: PathBuf::from("/tmp/test.esm"),
            edid_index: None,
            tree: Section::Absent,
            forms,
            cache_path: PathBuf::from("/tmp/test.esm.idx"),
            xref_index: None,
            search_index: None,
        }
    }

    /// Verify that `records_by_type` (through the real `.esm.forms` rkyv
    /// section, not an in-memory stand-in) returns a deterministic sorted
    /// order on repeated calls, and that `count_by_type`/`form_ids_by_type`/
    /// `signatures` all agree with it.
    #[test]
    fn records_by_type_sorted_and_stable() {
        let weap1 = FormId::new(0x0000_0010);
        let weap2 = FormId::new(0x0000_0005);
        let npc_ = FormId::new(0x0000_0020);

        let mut form_index = HashMap::new();
        form_index.insert(
            weap1,
            RecordMeta {
                offset: 0,
                signature: Signature::from_slice(b"WEAP"),
                flags: 0,
                form_version: 155,
            },
        );
        form_index.insert(
            weap2,
            RecordMeta {
                offset: 100,
                signature: Signature::from_slice(b"WEAP"),
                flags: 0,
                form_version: 155,
            },
        );
        form_index.insert(
            npc_,
            RecordMeta {
                offset: 200,
                signature: Signature::from_slice(b"NPC_"),
                flags: 0,
                form_version: 155,
            },
        );

        let path = test_forms_path("records_by_type_sorted_and_stable");
        let index = index_over(write_and_map_forms(&form_index, &path));

        // First call
        let first: Vec<(FormId, RecordMeta)> = index.records_by_type("WEAP").collect();
        // Second call — must return same order
        let second: Vec<(FormId, RecordMeta)> = index.records_by_type("WEAP").collect();

        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        // Pre-sorted by FormId::raw() ascending: weap2 (0x05) < weap1 (0x10)
        assert_eq!(first[0].0, weap2);
        assert_eq!(first[1].0, weap1);
        assert_eq!(first[0].0, second[0].0, "order must be stable across calls");
        assert_eq!(first[1].0, second[1].0, "order must be stable across calls");

        // count_by_type / form_ids_by_type must agree with records_by_type
        // without materializing the full RecordMeta for each entry.
        assert_eq!(index.count_by_type("WEAP"), 2);
        let weap_ids: Vec<FormId> = index.form_ids_by_type("WEAP").collect();
        assert_eq!(weap_ids, vec![weap2, weap1]);

        // NPC_ should return exactly one record
        let npc_records: Vec<(FormId, RecordMeta)> = index.records_by_type("NPC_").collect();
        assert_eq!(npc_records.len(), 1);
        assert_eq!(npc_records[0].0, npc_);
        assert_eq!(index.count_by_type("NPC_"), 1);

        // Unknown type returns empty
        assert_eq!(index.records_by_type("XXXX").count(), 0);
        assert_eq!(index.count_by_type("XXXX"), 0);

        // signatures() enumerates the distinct types present, alloc-free vs.
        // a full form_index scan.
        let mut sigs: Vec<String> = index.signatures().map(|s| s.as_str().to_owned()).collect();
        sigs.sort_unstable();
        assert_eq!(sigs, vec!["NPC_".to_string(), "WEAP".to_string()]);

        let _ = fs::remove_file(&path);
    }

    /// Round-trip a `FormsSection` built from a small synthetic set of
    /// records (3 distinct signatures) through `write_section` +
    /// `Section::map`, and assert every accessor (`get_by_formid`,
    /// `signature_of`, `contains`, `len`/`is_empty`, `iter_form_ids`,
    /// `signatures`, `count_by_type`, `form_ids_by_type`, `records_by_type`)
    /// agrees with the original in-memory data — including a FormID that's
    /// absent and a signature with zero records. Mirrors `tree.rs`'s
    /// `tree_round_trip_through_rkyv_section`.
    #[test]
    fn forms_round_trip_through_rkyv_section() {
        let weap1 = FormId::new(0x0000_0010);
        let weap2 = FormId::new(0x0000_0005);
        let npc_ = FormId::new(0x0000_0020);
        let armo1 = FormId::new(0x0000_0030);

        let mut form_index: HashMap<FormId, RecordMeta> = HashMap::new();
        form_index.insert(
            weap1,
            RecordMeta {
                offset: 10,
                signature: Signature::from_slice(b"WEAP"),
                flags: 0,
                form_version: 155,
            },
        );
        form_index.insert(
            weap2,
            RecordMeta {
                offset: 20,
                signature: Signature::from_slice(b"WEAP"),
                flags: 1,
                form_version: 155,
            },
        );
        form_index.insert(
            npc_,
            RecordMeta {
                offset: 30,
                signature: Signature::from_slice(b"NPC_"),
                flags: 4,
                form_version: 131,
            },
        );
        form_index.insert(
            armo1,
            RecordMeta {
                offset: 40,
                signature: Signature::from_slice(b"ARMO"),
                flags: 0,
                form_version: 131,
            },
        );

        let path = test_forms_path("forms_round_trip_through_rkyv_section");
        let index = index_over(write_and_map_forms(&form_index, &path));

        // get_by_formid / signature_of / contains agree with the original
        // for every present FormID.
        for (&id, meta) in &form_index {
            let got = index
                .get_by_formid(id)
                .unwrap_or_else(|| panic!("formid {id:?} must be present"));
            assert_eq!(got.offset, meta.offset);
            assert_eq!(got.signature, meta.signature);
            assert_eq!(got.flags, meta.flags);
            assert_eq!(got.form_version, meta.form_version);
            assert_eq!(index.signature_of(id), Some(meta.signature));
            assert!(index.contains(id));
        }

        // An absent FormID: get_by_formid/signature_of/contains all agree
        // it's not present.
        let missing = FormId::new(0xDEAD_BEEF);
        assert!(index.get_by_formid(missing).is_none());
        assert_eq!(index.signature_of(missing), None);
        assert!(!index.contains(missing));

        // len / is_empty
        assert_eq!(index.len(), 4);
        assert!(!index.is_empty());

        // iter_form_ids() — same set as the original, order-independent.
        let mut ids: Vec<FormId> = index.iter_form_ids().collect();
        ids.sort_by_key(|id| id.raw());
        let mut expected_ids: Vec<FormId> = form_index.keys().copied().collect();
        expected_ids.sort_by_key(|id| id.raw());
        assert_eq!(ids, expected_ids);

        // signatures() — the 3 distinct types present.
        let mut sigs: Vec<String> = index.signatures().map(|s| s.as_str().to_owned()).collect();
        sigs.sort_unstable();
        assert_eq!(
            sigs,
            vec!["ARMO".to_string(), "NPC_".to_string(), "WEAP".to_string()]
        );

        // count_by_type / form_ids_by_type / records_by_type agree for a
        // multi-record type...
        assert_eq!(index.count_by_type("WEAP"), 2);
        let weap_ids: Vec<FormId> = index.form_ids_by_type("WEAP").collect();
        assert_eq!(weap_ids, vec![weap2, weap1], "sorted ascending by FormID");
        assert_eq!(index.form_ids_by_type("WEAP").len(), 2);
        let weap_records: Vec<(FormId, RecordMeta)> = index.records_by_type("WEAP").collect();
        assert_eq!(
            weap_records.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            weap_ids
        );

        // ...a single-record type...
        assert_eq!(index.count_by_type("ARMO"), 1);
        assert_eq!(
            index.form_ids_by_type("ARMO").collect::<Vec<_>>(),
            vec![armo1]
        );

        // ...and a signature with ZERO records: count_by_type returns 0, and
        // form_ids_by_type/records_by_type return empty iterators rather
        // than panicking.
        assert_eq!(index.count_by_type("XXXX"), 0);
        assert_eq!(index.form_ids_by_type("XXXX").len(), 0);
        assert_eq!(index.form_ids_by_type("XXXX").count(), 0);
        assert_eq!(index.records_by_type("XXXX").count(), 0);

        let _ = fs::remove_file(&path);
    }

    /// Regression test for lite-mode behavior: every accessor over an
    /// absent forms section (`Index::empty`'s state, or a cache that hasn't
    /// been built yet) must answer with its empty-equivalent rather than
    /// panicking. Mirrors `tree.rs`'s `tree_view_absent_state_never_panics`;
    /// exercised via `Index::empty` itself (rather than a bare
    /// `Section::Absent` built by hand) since that's the real production
    /// path this guards — see `Database::open_lite` in `lib.rs`.
    #[test]
    fn forms_absent_state_never_panics() {
        let index = Index::empty(PathBuf::from("/tmp/fo76_forms_absent_test.esm"));

        assert!(index.get_by_formid(FormId::new(1)).is_none());
        assert_eq!(index.signature_of(FormId::new(1)), None);
        assert!(!index.contains(FormId::new(1)));
        assert_eq!(index.len(), 0);
        assert!(index.is_empty());
        assert_eq!(index.iter_form_ids().count(), 0);
        assert_eq!(index.signatures().count(), 0);
        assert_eq!(index.count_by_type("WEAP"), 0);
        assert_eq!(index.form_ids_by_type("WEAP").len(), 0);
        assert_eq!(index.form_ids_by_type("WEAP").count(), 0);
        assert_eq!(index.records_by_type("WEAP").count(), 0);
    }

    #[test]
    fn unique_tmp_path_differs_and_same_parent() -> anyhow::Result<()> {
        let base = PathBuf::from("/tmp/SeventySix.esm.idx");
        let p1 = unique_tmp_path(&base)?;
        let p2 = unique_tmp_path(&base)?;
        assert_ne!(p1, p2);
        assert_eq!(p1.parent(), base.parent());
        Ok(())
    }
}
