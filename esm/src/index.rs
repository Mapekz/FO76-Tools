use crate::decode::{DecodeContext, ResolveDepth, decode_record};
use crate::format::Signature;
use crate::formid::{FormId, parse_formid};
use crate::reader::{
    EsmFile, RecordMeta, WalkEvent, edid_from_subrecords, inline_string_from_subrecords,
    lstring_id_from_subrecords,
};
#[cfg(test)]
use crate::rkyvcache::write_section;
use crate::rkyvcache::{
    CacheSig, Section, map_section_if_present, section_path_for_spec, write_and_remap,
};
// `SectionKind`/`section_path_for` (the explicit-kind form `section_path_for_spec`
// replaces at every production call site — see that function's doc comment)
// are only still needed by this module's own tests, which build paths for a
// kind that deliberately doesn't match the type being mapped, to prove
// `Section::map` rejects the mismatch.
#[cfg(test)]
use crate::rkyvcache::{SectionKind, section_path_for};
use crate::schema::Schema;
use crate::strings::Localization;
use crate::tree::TreeIndex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

// Bump whenever any section's cached on-disk data changes, whether that's a
// layout change (fields added/removed/reordered) or a content change (the
// same fields now hold different derived values). Content changes are
// invisible to `*_LAYOUT_FINGERPRINT`, which only folds in the archived
// type's `size_of`/`align_of` — the version bump is the only thing that
// catches those. All five sections (`tree`/`forms`/`edid`/`search`/`xref`)
// share this one constant, so a bump rebuilds all five even when only one
// changed.
pub(crate) const CACHE_VERSION: u32 = 15;

/// Per-record data stored in the lazy search index.
///
/// For **localized** ESMs the name and description are stored as lstring IDs
/// (`full_id`, `desc_id`), resolved to text at query time via the active
/// [`Localization`] table.  For **non-localized** ESMs the inline text is
/// stored directly (`full_text`, `desc_text`) so no localization BA2 is needed.
///
/// rkyv-archived directly (no serde derives) — nothing in this crate
/// serde-encodes a bare `SearchMeta` once `search_index` leaves the bincode
/// blob (Stage 6); it only ever travels inside [`SearchSection`] via
/// [`write_section`]/[`Section::map`], same reasoning `018d7a8` documents for
/// dropping `RecordMeta`'s serde derives.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

/// One combined rkyv section: the sorted FormID→[`RecordMeta`] table plus
/// the type directory, always built and written together (mirrors how
/// they're already always rebuilt together in `build_tree_and_forms`/
/// `build_type_index`) — see `rkyvcache.rs` for the section mechanics this
/// is built on, and `tree.rs`'s `TreeIndex` for the prior-stage type this
/// one's plumbing mirrors.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct FormsSection {
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
/// `Section::map` for the `forms` section — see `Index::build`/
/// `build_tree_and_forms` below.
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

/// Binds the `forms` section's archived type to its kind and layout
/// fingerprint — see [`crate::rkyvcache::SectionSpec`]'s doc comment.
impl crate::rkyvcache::SectionSpec for rkyv::Archived<FormsSection> {
    const KIND: crate::rkyvcache::SectionKind = crate::rkyvcache::SectionKind::Forms;
    const LAYOUT_FINGERPRINT: u64 = FORMS_LAYOUT_FINGERPRINT;
}

/// One rkyv section: the EditorID → raw FormID map (`edid`). A thin
/// named wrapper around the map, matching how `018d7a8` wrapped
/// `records`/`types` in [`FormsSection`] rather than archiving a bare
/// top-level collection.
///
/// `edid_to_form`'s archived key type is `ArchivedString`, which — unlike
/// the `rend` endian-wrapper integer types (see [`XrefSection`]'s doc
/// comment) — DOES implement `Borrow<str>` (`rkyv` 0.8.17,
/// `src/string/mod.rs`), with a `Hash` impl that delegates to `str::hash`.
/// So [`Index::get_by_edid`] can call `.get(edid)` directly with a plain
/// `&str` — no key-conversion step needed here, unlike [`XrefSection`]'s
/// `u32` keys.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct EdidSection {
    edid_to_form: HashMap<String, u32>,
}

/// FNV-1a fingerprint of [`EdidSection`]'s archived layout — see
/// [`FORMS_LAYOUT_FINGERPRINT`]'s doc comment for the general pattern. No
/// other named `Archive`-derived type is reachable from `EdidSection` (its
/// key/value types are `String`/`u32`, both `rkyv`-builtin), so only
/// `EdidSection` itself needs folding in.
const EDID_LAYOUT_FINGERPRINT: u64 = {
    use crate::rkyvcache::{FNV_OFFSET_BASIS, fnv1a_u64};

    let acc = fnv1a_u64(
        FNV_OFFSET_BASIS,
        core::mem::size_of::<rkyv::Archived<EdidSection>>() as u64,
    );
    fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<EdidSection>>() as u64,
    )
};

/// Binds the `edid` section's archived type to its kind and layout
/// fingerprint — see [`crate::rkyvcache::SectionSpec`]'s doc comment.
impl crate::rkyvcache::SectionSpec for rkyv::Archived<EdidSection> {
    const KIND: crate::rkyvcache::SectionKind = crate::rkyvcache::SectionKind::Edid;
    const LAYOUT_FINGERPRINT: u64 = EDID_LAYOUT_FINGERPRINT;
}

/// One rkyv section: the raw FormID → [`SearchMeta`] map (`search`).
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct SearchSection {
    entries: HashMap<u32, SearchMeta>,
}

/// FNV-1a fingerprint of [`SearchSection`]'s archived layout — see
/// [`FORMS_LAYOUT_FINGERPRINT`]'s doc comment for the general pattern.
/// `SearchMeta` is folded in separately for the same reason `RecordMeta` is
/// folded into [`FORMS_LAYOUT_FINGERPRINT`]: it sits behind a `HashMap`
/// indirection, so a layout change to it alone would not necessarily change
/// `size_of::<Archived<SearchSection>>()`.
const SEARCH_LAYOUT_FINGERPRINT: u64 = {
    use crate::rkyvcache::{FNV_OFFSET_BASIS, fnv1a_u64};

    let acc = fnv1a_u64(
        FNV_OFFSET_BASIS,
        core::mem::size_of::<rkyv::Archived<SearchSection>>() as u64,
    );
    let acc = fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<SearchSection>>() as u64,
    );
    let acc = fnv1a_u64(
        acc,
        core::mem::size_of::<rkyv::Archived<SearchMeta>>() as u64,
    );
    fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<SearchMeta>>() as u64,
    )
};

/// Binds the `search` section's archived type to its kind and layout
/// fingerprint — see [`crate::rkyvcache::SectionSpec`]'s doc comment.
impl crate::rkyvcache::SectionSpec for rkyv::Archived<SearchSection> {
    const KIND: crate::rkyvcache::SectionKind = crate::rkyvcache::SectionKind::Search;
    const LAYOUT_FINGERPRINT: u64 = SEARCH_LAYOUT_FINGERPRINT;
}

/// One rkyv section: the raw referencee FormID → raw referencing FormIDs map
/// (`xref`).
///
/// `refs`'s archived key type is the endian-wrapped `rkyv::rend::u32_le`,
/// not a bare `u32` — `rend`'s newtypes only get the blanket `Borrow<T> for
/// T`, never `Borrow<u32>` for `u32_le`, so [`Index::get_xref`] has to
/// convert the lookup key to the exact archived key type first, the same
/// `let key: rkyv::rend::u64_le = offset.into();` treatment
/// `TreeView::group_idx_at_offset` already established for `offset_map`'s
/// `u64` keys.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct XrefSection {
    refs: HashMap<u32, Vec<u32>>,
}

/// FNV-1a fingerprint of [`XrefSection`]'s archived layout — see
/// [`FORMS_LAYOUT_FINGERPRINT`]'s doc comment for the general pattern. No
/// other named `Archive`-derived type is reachable from `XrefSection` (its
/// key/value types are `u32`/`Vec<u32>`, both `rkyv`-builtin), so only
/// `XrefSection` itself needs folding in.
const XREF_LAYOUT_FINGERPRINT: u64 = {
    use crate::rkyvcache::{FNV_OFFSET_BASIS, fnv1a_u64};

    let acc = fnv1a_u64(
        FNV_OFFSET_BASIS,
        core::mem::size_of::<rkyv::Archived<XrefSection>>() as u64,
    );
    fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<XrefSection>>() as u64,
    )
};

/// Binds the `xref` section's archived type to its kind and layout
/// fingerprint — see [`crate::rkyvcache::SectionSpec`]'s doc comment.
impl crate::rkyvcache::SectionSpec for rkyv::Archived<XrefSection> {
    const KIND: crate::rkyvcache::SectionKind = crate::rkyvcache::SectionKind::Xref;
    const LAYOUT_FINGERPRINT: u64 = XREF_LAYOUT_FINGERPRINT;
}

/// # Not `Clone`; `Debug` is hand-written
///
/// Every field below is a [`Section`], which wraps a `Mmap` — `Mmap`
/// implements neither trait. Nothing in this crate needs `Index: Clone`
/// (`Database`, which owns one, doesn't derive it either; `Registry` shares
/// `Database` instances via `Arc<Mutex<Database>>`, cloning the `Arc`, never
/// the `Database`/`Index` itself). `Debug` is implemented manually below,
/// summarizing each section as just its mapped/absent state — the shape
/// `Result`/`Option` helpers that require `T: Debug` (e.g. `.unwrap_err()`
/// on a `Result<Index, _>`) need — rather than requiring `Section<A>: Debug`
/// for all five.
pub struct Index {
    pub(crate) path: PathBuf,
    /// Eager, all-or-nothing alongside `forms` — see [`Index::build`].
    pub(crate) tree: Section<rkyv::Archived<TreeIndex>>,
    /// Eager, all-or-nothing alongside `tree` — see [`Index::build`].
    pub(crate) forms: Section<rkyv::Archived<FormsSection>>,
    /// Lazy, independently optional — see [`crate::Database::ensure_edid_index`]
    /// and [`Index::build`]'s doc comment for the cross-process warm-reuse
    /// property this field (and `search`/`xref` below) preserves. Building
    /// it needs `Database`'s other fields (the mmap'd ESM), which is why
    /// that method lives on `Database`, not here — `Index` only holds the
    /// section once built.
    pub(crate) edid: Section<rkyv::Archived<EdidSection>>,
    /// Lazy, independently optional — see [`crate::Database::ensure_search_index`].
    pub(crate) search: Section<rkyv::Archived<SearchSection>>,
    /// Lazy, independently optional — see [`crate::Database::ensure_xref_index`].
    pub(crate) xref: Section<rkyv::Archived<XrefSection>>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index")
            .field("path", &self.path)
            .field("tree_mapped", &self.tree.is_mapped())
            .field("forms_mapped", &self.forms.is_mapped())
            .field("edid_mapped", &self.edid.is_mapped())
            .field("search_mapped", &self.search.is_mapped())
            .field("xref_mapped", &self.xref.is_mapped())
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
    /// Load the on-disk cache for `esm`, building whatever pieces are
    /// missing or stale.
    ///
    /// `tree`/`forms` are eager and all-or-nothing (unchanged since Stage
    /// 5): both must independently map against the same [`CacheSig`], or
    /// both are rebuilt together from a fresh ESM walk (see
    /// [`build_tree_and_forms`]).
    ///
    /// `edid`/`search`/`xref` are lazy and independently optional — each is
    /// opportunistically mapped on its own right here. This is what gives
    /// this crate its cross-process warm-reuse property for the three lazy
    /// indexes: before Stage 6, all three lived in the same bincode
    /// `CacheFile` blob as `form_index`/`tree`, so if process A called
    /// `Database::ensure_edid_index` (building it and persisting the whole
    /// blob), process B starting later and loading that blob got
    /// `edid_index: Some(...)` for free, purely because the whole blob
    /// decoded in one shot. Now that each of the three lives in its own
    /// independent section file (the whole point of sectioning —
    /// `ensure_edid_index` only ever writes the `edid` section, never a
    /// shared blob), that property has to be reconstructed explicitly:
    /// `Section::map` already degrades a missing/stale/corrupt file to
    /// `Section::Absent` on its own (never an `Err` for that reason), so
    /// mapping all three here — unconditionally, no extra "is it there"
    /// branch — is sufficient. If process A already called the matching
    /// `ensure_*_index` and its write landed on disk, process B's
    /// `Index::build` (this function) picks up an already-`Mapped` section
    /// here, and the matching `ensure_*_index` call later becomes a no-op
    /// (its `is_mapped()` early-return). If not, the field is
    /// `Section::Absent`, exactly like `Index::empty`, ready for a later
    /// `ensure_*_index` call to build it. Any one of the three being absent
    /// never affects the other two, or `tree`/`forms`, or causes this
    /// function to fail or rebuild anything else.
    pub fn build(esm: &EsmFile) -> anyhow::Result<Self> {
        let sig = CacheSig::read(&esm.path)?;

        let tree_section = Section::<rkyv::Archived<TreeIndex>>::map(
            &section_path_for_spec::<rkyv::Archived<TreeIndex>>(&esm.path)?,
            sig,
            CACHE_VERSION,
        )?;
        let forms_section = Section::<rkyv::Archived<FormsSection>>::map(
            &section_path_for_spec::<rkyv::Archived<FormsSection>>(&esm.path)?,
            sig,
            CACHE_VERSION,
        )?;
        let (tree_section, forms_section) = if tree_section.is_mapped() && forms_section.is_mapped()
        {
            (tree_section, forms_section)
        } else {
            build_tree_and_forms(esm, sig)?
        };

        // Independently-optional lazy indexes — see this function's doc
        // comment above for why no extra "leave absent" branch is needed:
        // `Section::map` already IS that branch, uniformly, for all three.
        let edid_section = Section::<rkyv::Archived<EdidSection>>::map(
            &section_path_for_spec::<rkyv::Archived<EdidSection>>(&esm.path)?,
            sig,
            CACHE_VERSION,
        )?;
        let search_section = Section::<rkyv::Archived<SearchSection>>::map(
            &section_path_for_spec::<rkyv::Archived<SearchSection>>(&esm.path)?,
            sig,
            CACHE_VERSION,
        )?;
        let xref_section = Section::<rkyv::Archived<XrefSection>>::map(
            &section_path_for_spec::<rkyv::Archived<XrefSection>>(&esm.path)?,
            sig,
            CACHE_VERSION,
        )?;

        Ok(Index {
            path: esm.path.clone(),
            tree: tree_section,
            forms: forms_section,
            edid: edid_section,
            search: search_section,
            xref: xref_section,
        })
    }

    /// Create an index with every section absent.
    ///
    /// The index holds no records and must not be persisted to disk — it is
    /// the same starting state a fresh cache is in before [`Index::build`]
    /// (or a lazy `ensure_*_index` call) populates any section, and every
    /// accessor must degrade to its empty-equivalent rather than panic when
    /// handed one. Exercised directly by this module's absent-state
    /// regression tests.
    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            tree: Section::Absent,
            forms: Section::Absent,
            edid: Section::Absent,
            search: Section::Absent,
            xref: Section::Absent,
        }
    }

    /// Resolve a FormID to its [`RecordMeta`]. Owned, not borrowed — cheap
    /// since `RecordMeta` is `Copy`. `None` if the forms section is absent
    /// (no cache built yet) or `form_id` isn't present.
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
    /// section is absent (no cache built yet).
    pub fn contains(&self, form_id: FormId) -> bool {
        self.forms.get().is_some_and(|f| {
            f.records
                .binary_search_by_key(&form_id.raw(), |entry| entry.0.to_native())
                .is_ok()
        })
    }

    /// Total number of records in the form index. `0` if the forms section
    /// is absent (no cache built yet).
    pub fn len(&self) -> usize {
        self.forms.get().map_or(0, |f| f.records.len())
    }

    pub fn is_empty(&self) -> bool {
        self.forms.get().is_none_or(|f| f.records.is_empty())
    }

    /// Iterate every FormID present in the form index, in on-disk
    /// (FormID-sorted) order. Empty iterator if the forms section is absent
    /// (no cache built yet).
    pub fn iter_form_ids(&self) -> impl Iterator<Item = FormId> + '_ {
        let records: &[_] = self.forms.get().map_or(&[][..], |f| f.records.as_slice());
        records.iter().map(|entry| FormId::new(entry.0.to_native()))
    }

    /// Iterate every `(FormId, RecordMeta)` pair, in on-disk (FormID-sorted)
    /// order. Internal helper for [`build_edid_section`]/
    /// [`build_search_section`], which — unlike [`Self::iter_form_ids`]
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

    /// Resolve an EditorID to its FormID via the lazy EditorID index. `None`
    /// if the index hasn't been built yet ([`crate::Database::ensure_edid_index`])
    /// or `edid` isn't present. See [`EdidSection`]'s doc comment for why
    /// this looks the key up directly with `&str` (no archived-key
    /// conversion needed, unlike [`Self::get_xref`]).
    ///
    /// `pub(crate)`, not `pub`: reachable only through
    /// [`crate::Database`]'s own methods, which always call
    /// `ensure_edid_index` first — see that method's doc comment for why
    /// this and [`Self::get_xref`]/[`Self::iter_search`] are not part of
    /// this crate's public surface (a caller reaching this directly,
    /// without ensuring first, would silently read "not found" for an
    /// EditorID that's actually present but just not indexed yet).
    pub(crate) fn get_by_edid(&self, edid: &str) -> Option<FormId> {
        let section = self.edid.get()?;
        section
            .edid_to_form
            .get(edid)
            .map(|raw| FormId::new(raw.to_native()))
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
    /// `build_type_index`/`build_tree_and_forms`). Alloc-free beyond the
    /// returned iterator itself — does not fetch each record's
    /// [`RecordMeta`]. Empty iterator if the forms section is absent or the
    /// type has no records.
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

    /// Return the list of FormIDs that reference the given FormID. Empty
    /// `Vec` if the xref section is absent (not yet built —
    /// [`crate::Database::ensure_xref_index`]) or `form_id` has no
    /// referencers.
    ///
    /// `pub(crate)` — see [`Self::get_by_edid`]'s doc comment for why this
    /// and [`Self::iter_search`] are not part of the public surface: every
    /// real caller reaches this through [`crate::Database`]'s own
    /// ensure-then-get wrapper, which can't forget the `ensure` half because
    /// there's only one method to call.
    pub(crate) fn get_xref(&self, form_id: FormId) -> Vec<FormId> {
        let Some(section) = self.xref.get() else {
            return Vec::new();
        };
        // See `XrefSection`'s doc comment for why the lookup key must be
        // converted to the archived key type first.
        let key: rkyv::rend::u32_le = form_id.raw().into();
        section
            .refs
            .get(&key)
            .map(|v| v.iter().map(|id| FormId::new(id.to_native())).collect())
            .unwrap_or_default()
    }

    /// Iterate the lazy search index (if already built) as borrowed
    /// [`SearchRef`] views — empty iterator if not yet built. `pub(crate)` —
    /// see [`Self::get_by_edid`]'s doc comment.
    pub(crate) fn iter_search(&self) -> impl Iterator<Item = (FormId, SearchRef<'_>)> + '_ {
        self.search.get().into_iter().flat_map(|s| {
            s.entries.iter().map(|(id, meta)| {
                (
                    FormId::new(id.to_native()),
                    SearchRef {
                        editor_id: meta.editor_id.as_ref().map(|s| s.as_str()),
                        full_id: meta.full_id.as_ref().map(|v| v.to_native()),
                        desc_id: meta.desc_id.as_ref().map(|v| v.to_native()),
                        full_text: meta.full_text.as_ref().map(|s| s.as_str()),
                        desc_text: meta.desc_text.as_ref().map(|s| s.as_str()),
                    },
                )
            })
        })
    }

    /// Borrowed view over the GRUP tree — replaces the removed `pub tree`
    /// field. See [`crate::tree::TreeView`]. `Section::get` already returns
    /// `Option<&Archived<TreeIndex>>`, exactly what `TreeView::new` expects —
    /// `Section::Absent` (no cache built yet, or a corrupt/foreign file that
    /// degraded to absent) degrades to an empty-equivalent `TreeView` rather
    /// than a panic.
    pub fn tree(&self) -> crate::tree::TreeView<'_> {
        crate::tree::TreeView::new(self.tree.get())
    }
}

/// Build the `edid` section's data (EditorID → FormID) by scanning every
/// record's own EDID subrecord. The data-only half of
/// [`crate::Database::ensure_edid_index`], which owns the surrounding
/// acquire/recheck/write/publish protocol (via
/// [`crate::rkyvcache::write_and_remap`]) — this only computes the map,
/// ticking `lease` once per record scanned.
pub(crate) fn build_edid_section(
    index: &Index,
    esm: &EsmFile,
    lease: &mut crate::progress::BuildLease,
) -> anyhow::Result<EdidSection> {
    let mut edid_to_form: HashMap<String, u32> = HashMap::new();
    for (i, (form_id, meta)) in index.iter_all().enumerate() {
        lease.tick(i as u64);
        let rec = esm.parse_record_at(meta.offset)?;
        if let Some(edid) = edid_from_subrecords(&rec.subrecords) {
            edid_to_form.insert(edid, form_id.raw());
        }
    }
    Ok(EdidSection { edid_to_form })
}

/// Build the `search` section's data (EditorID/name/description per
/// record) — the data-only half of [`crate::Database::ensure_search_index`];
/// see [`build_edid_section`]'s doc comment for the split this mirrors.
///
/// For **localized** ESMs the FULL/DESC lstring IDs are stored (resolved to
/// text at query time). For **non-localized** ESMs the inline string text
/// is stored directly.
pub(crate) fn build_search_section(
    index: &Index,
    esm: &EsmFile,
    is_localized: bool,
    lease: &mut crate::progress::BuildLease,
) -> anyhow::Result<SearchSection> {
    let mut entries: HashMap<u32, SearchMeta> = HashMap::new();
    for (i, (form_id, meta)) in index.iter_all().enumerate() {
        lease.tick(i as u64);
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
            entries.insert(
                form_id.raw(),
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
    Ok(SearchSection { entries })
}

/// Build the `xref` section's data (referencee FormID → referencing
/// FormIDs) — the data-only half of [`crate::Database::ensure_xref_index`];
/// see [`build_edid_section`]'s doc comment for the split this mirrors.
/// This is the most expensive of the three lazy builds (a full schema
/// decode of every record, not just an EDID/name lookup).
///
/// Walks every record, decodes it with `ResolveDepth::None` (so FormID
/// fields come out as `"0x........"` hex strings), harvests those strings,
/// and inverts them into a referencee→referencers map.
pub(crate) fn build_xref_section(
    index: &Index,
    esm: &EsmFile,
    schema: &Schema,
    is_localized: bool,
    localization: Option<&Localization>,
    curves: Option<&crate::curves::CurveIndex>,
    lease: &mut crate::progress::BuildLease,
) -> anyhow::Result<XrefSection> {
    let mut xref: HashMap<u32, Vec<u32>> = HashMap::new();
    esm.walk_records(|meta| {
        lease.tick(meta.offset);
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
            // A target is kept if it's either a real indexed record or
            // one of the ~228 engine-hardcoded FormIDs (`crate::hardcoded`,
            // e.g. AVIF `DamageRecieved`/`KillStreak`) — hardcoded forms
            // have no backing record by design, so without this fallback
            // every real reference to one was silently dropped while the
            // index was built (issue #27). `index.contains` still runs
            // first and short-circuits for the overwhelming majority of
            // targets, so the 228-entry binary search only fires on an
            // index miss — matching `hardcoded::lookup`'s own "consult
            // only as a fallback" contract.
            //
            // This is a bounded, curated allowlist, not a relaxation of
            // the existence check itself: `harvest_formids` collects
            // every `0x…`-shaped string in the decoded JSON, including
            // values from misdecoded bytes, and `index.contains` is what
            // keeps that garbage out. `0x0` (NULL) in particular appears
            // dozens of times among PERK effects alone and stays
            // correctly excluded — it is below the hardcoded table's
            // `0x1A` floor. A few other harvested low FormIDs also fall
            // outside both the index and the table (e.g. `0x14`, just
            // under that floor, and a couple just above the table's
            // `0x39B` ceiling); those stay dropped too — undocumented
            // engine internals this table doesn't cover.
            let target_exists =
                index.contains(target) || crate::hardcoded::lookup(target).is_some();
            if target != referencer && target_exists && seen.insert(target) {
                xref.entry(target.raw()).or_default().push(referencer.raw());
            }
        }
        Ok(())
    })?;
    Ok(XrefSection { refs: xref })
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

/// The eager, all-or-nothing pair [`build_tree_and_forms`] rebuilds and
/// [`Index::build`] maps — factored into a named alias per clippy's
/// `type_complexity` lint.
type TreeAndFormsSections = (
    Section<rkyv::Archived<TreeIndex>>,
    Section<rkyv::Archived<FormsSection>>,
);

/// Build `tree`/`forms` fresh from a full ESM walk, write each to its own
/// rkyv section, then drop the owned data and map both straight back in —
/// the write→drop→re-map protocol `9e7c160`/`018d7a8` established. Called
/// from [`Index::build`] whenever either section fails to map against the
/// current [`CacheSig`] (`sig`, computed once by the caller) — `tree`/`forms`
/// are eager and all-or-nothing, so this always rebuilds and returns both
/// together, never just one.
///
/// Acquires a [`crate::progress::BuildLease`] for the duration via
/// [`crate::progress::BuildLease::acquire_or_recheck`] (see `Index::build`'s
/// call site, which holds no lock of its own before calling in — the lock
/// lives entirely inside this function so the common warm-path
/// `Section::map` check in `Index::build` never pays for it). The recheck
/// closure re-maps both sections immediately after the lease is granted:
/// another process may have finished building while this call was blocked
/// waiting for the lock, in which case [`crate::progress::Acquired::AlreadyBuilt`]
/// returns the now-mapped sections without this function doing any real
/// work — structurally, not just by convention, since a [`crate::progress::BuildLease`]
/// only exists in the [`crate::progress::Acquired::NeedsBuild`] arm below.
fn build_tree_and_forms(esm: &EsmFile, sig: CacheSig) -> anyhow::Result<TreeAndFormsSections> {
    let tree_path = section_path_for_spec::<rkyv::Archived<TreeIndex>>(&esm.path)?;
    let forms_path = section_path_for_spec::<rkyv::Archived<FormsSection>>(&esm.path)?;
    let total = esm.data().len() as u64;

    // Single stage, not two: `tree` and `forms` are both derived from one
    // shared `walk_structure` pass below, so there is only one counting
    // stage to report.
    //
    // The `map_section_if_present` pair below is a SECOND on-disk check of
    // the exact two files `Index::build`'s caller already mapped once, just
    // above, before deciding both weren't mapped and calling in here — not
    // a redundant repeat of that check, but the one that closes the TOCTOU
    // window between that first check and this call actually acquiring the
    // advisory build lock: another process can finish building both
    // sections in that gap, and this recheck is what lets this call return
    // the just-finished sections (`Acquired::AlreadyBuilt`) instead of
    // racing a second walk of the whole ESM. See `crate::Database::
    // build_lazy_section`'s doc comment for the same shape on the three lazy
    // single-section builds.
    let mut lease = match crate::progress::BuildLease::acquire_or_recheck(
        &esm.path,
        crate::progress::BuildStage::Forms,
        1,
        1,
        total,
        || {
            let tree_recheck = map_section_if_present::<rkyv::Archived<TreeIndex>>(
                &tree_path,
                sig,
                CACHE_VERSION,
            )?;
            let forms_recheck = map_section_if_present::<rkyv::Archived<FormsSection>>(
                &forms_path,
                sig,
                CACHE_VERSION,
            )?;
            Ok(tree_recheck.zip(forms_recheck))
        },
    )? {
        crate::progress::Acquired::AlreadyBuilt(sections) => return Ok(sections),
        crate::progress::Acquired::NeedsBuild(lease) => lease,
    };

    // One structural walk feeds both builders inline: each `WalkEvent`
    // updates the tree arena (`TreeBuilder`, the same per-event state
    // machine `TreeIndex::build_with_tick` drives on its own) and, for
    // `WalkEvent::Record`, also inserts a `RecordMeta` into the forms table
    // — derived straight from the event's own fields, no second header parse
    // needed (the pre-unification version above re-parsed each record's
    // header a second time here just to recover `form_id`, since
    // `walk_records`'s `RecordMeta` didn't carry it).
    let mut form_index = HashMap::new();
    let mut tree_builder = crate::tree::TreeBuilder::new();
    esm.walk_structure(|event| {
        let offset = match &event {
            WalkEvent::GroupStart { offset, .. } => *offset,
            WalkEvent::GroupEnd { offset } => *offset,
            WalkEvent::Record(r) => r.offset,
        };
        lease.tick(offset);
        if let WalkEvent::Record(sr) = &event {
            form_index.insert(
                sr.form_id,
                RecordMeta {
                    offset: sr.offset,
                    signature: sr.signature,
                    flags: sr.flags,
                    form_version: sr.form_version,
                },
            );
        }
        tree_builder.push_event(&event);
        Ok(())
    })?;
    let tree = tree_builder.finish();
    let type_index = build_type_index(&form_index);

    // Both sections are written back-to-back below with no further counting
    // pass in between — report the whole write phase as "writing" rather
    // than threading a further stage transition through a phase that's
    // CPU/IO-bound, not iteration-counted, and short relative to the walk
    // above.
    lease.writing();

    // Write the freshly-built tree to its own rkyv section, then drop the
    // owned value and map it straight back in — there is exactly one code
    // path for how `Index` ever reads tree data afterwards (through the
    // archived section via `Index::tree()`), never a second one that keeps
    // the owned `TreeIndex` around. `write_and_remap` is the one helper
    // every section build (this one and the three lazy `ensure_*_index`
    // builds in `lib.rs`) uses for this write→drop→re-map→ensure-mapped
    // sequence.
    let tree_section = write_and_remap(&tree_path, sig, CACHE_VERSION, tree)?;

    // Build the combined forms section (sorted FormID table + type
    // directory) from the local owned `form_index`/`type_index`, write it,
    // drop the owned data, and map it straight back in — same protocol as
    // `tree` above.
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
    let forms_section = write_and_remap(&forms_path, sig, CACHE_VERSION, forms_data)?;

    Ok((tree_section, forms_section))
}

/// Which of the five rkyv cache sections are present and valid for
/// `esm_path` — one bucket per [`crate::progress::BuildStage`] (the same
/// five names as [`SectionKind`], see that type's doc comment).
#[derive(Debug, Clone)]
pub struct CacheInventory {
    pub present: Vec<crate::progress::BuildStage>,
    pub missing: Vec<crate::progress::BuildStage>,
}

impl CacheInventory {
    pub fn is_empty(&self) -> bool {
        self.present.is_empty()
    }

    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Inspect `esm_cache/`'s five sections for `esm_path` via the same O(1)
/// header check [`Index::build`] uses (`Section::map`: magic/format/kind/
/// cache_version/layout_fingerprint/ESM identity stamp) — but as a **pure
/// read**, deliberately without `Index::build`'s `build_tree_and_forms`
/// fallback. This must never trigger a build: `esm cache status` calls it
/// while another process may hold the build lock, and it has to answer
/// instantly regardless. Doesn't mmap the ESM itself — `CacheSig::read`
/// only needs `fs::metadata`.
pub fn cache_inventory(esm_path: &std::path::Path) -> anyhow::Result<CacheInventory> {
    let sig = CacheSig::read(esm_path)?;
    let mut present = Vec::new();
    let mut missing = Vec::new();

    let mut bucket = |stage: crate::progress::BuildStage, mapped: bool| {
        if mapped {
            present.push(stage);
        } else {
            missing.push(stage);
        }
    };

    bucket(
        crate::progress::BuildStage::Forms,
        Section::<rkyv::Archived<FormsSection>>::map(
            &section_path_for_spec::<rkyv::Archived<FormsSection>>(esm_path)?,
            sig,
            CACHE_VERSION,
        )?
        .is_mapped(),
    );
    bucket(
        crate::progress::BuildStage::Tree,
        Section::<rkyv::Archived<TreeIndex>>::map(
            &section_path_for_spec::<rkyv::Archived<TreeIndex>>(esm_path)?,
            sig,
            CACHE_VERSION,
        )?
        .is_mapped(),
    );
    bucket(
        crate::progress::BuildStage::Edid,
        Section::<rkyv::Archived<EdidSection>>::map(
            &section_path_for_spec::<rkyv::Archived<EdidSection>>(esm_path)?,
            sig,
            CACHE_VERSION,
        )?
        .is_mapped(),
    );
    bucket(
        crate::progress::BuildStage::Search,
        Section::<rkyv::Archived<SearchSection>>::map(
            &section_path_for_spec::<rkyv::Archived<SearchSection>>(esm_path)?,
            sig,
            CACHE_VERSION,
        )?
        .is_mapped(),
    );
    bucket(
        crate::progress::BuildStage::Xref,
        Section::<rkyv::Archived<XrefSection>>::map(
            &section_path_for_spec::<rkyv::Archived<XrefSection>>(esm_path)?,
            sig,
            CACHE_VERSION,
        )?
        .is_mapped(),
    );

    Ok(CacheInventory { present, missing })
}

/// Regression test for [`crate::rkyvcache::SectionSpec`]'s core promise:
/// each of the five sections' `KIND`/`LAYOUT_FINGERPRINT` pairing, chosen
/// once in the `impl SectionSpec` block next to that section's own type
/// definition, actually matches the named `_LAYOUT_FINGERPRINT` constant and
/// `SectionKind` variant this module's doc comments say it should. This
/// `match` is exhaustive with no wildcard arm — adding a sixth
/// [`crate::progress::BuildStage`] variant without a matching arm here (and
/// in [`BuildStage::label`]/[`BuildStage::unit`] in `progress.rs`) is a
/// compile error, not a silently-uncovered case.
#[cfg(test)]
fn section_spec_fingerprint_for(kind: SectionKind) -> u64 {
    use crate::rkyvcache::SectionSpec;
    match kind {
        SectionKind::Tree => <rkyv::Archived<TreeIndex> as SectionSpec>::LAYOUT_FINGERPRINT,
        SectionKind::Forms => <rkyv::Archived<FormsSection> as SectionSpec>::LAYOUT_FINGERPRINT,
        SectionKind::Edid => <rkyv::Archived<EdidSection> as SectionSpec>::LAYOUT_FINGERPRINT,
        SectionKind::Search => <rkyv::Archived<SearchSection> as SectionSpec>::LAYOUT_FINGERPRINT,
        SectionKind::Xref => <rkyv::Archived<XrefSection> as SectionSpec>::LAYOUT_FINGERPRINT,
    }
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
    use std::fs;
    use std::path::Path;

    /// Asserts the pairing [`crate::rkyvcache::SectionSpec`] binds — that
    /// every section's `KIND`/`LAYOUT_FINGERPRINT` (chosen once, in the
    /// `impl SectionSpec` block next to that section's own type) actually
    /// matches this module's own `_LAYOUT_FINGERPRINT` constants — for all
    /// five sections, via the exhaustive `match` in
    /// [`section_spec_fingerprint_for`]. Guards against a wrong
    /// `(SectionKind, CACHE_VERSION, LAYOUT_FINGERPRINT)` pairing compiling
    /// fine while silently forcing a section to rebuild on every open:
    /// there is exactly one place per section where the pairing is chosen,
    /// and this test proves each of those five places agrees with the named
    /// constant its own doc comments claim.
    #[test]
    fn section_spec_pairing_matches_named_fingerprint_constants() {
        assert_eq!(
            section_spec_fingerprint_for(SectionKind::Tree),
            crate::tree::TREE_LAYOUT_FINGERPRINT
        );
        assert_eq!(
            section_spec_fingerprint_for(SectionKind::Forms),
            FORMS_LAYOUT_FINGERPRINT
        );
        assert_eq!(
            section_spec_fingerprint_for(SectionKind::Edid),
            EDID_LAYOUT_FINGERPRINT
        );
        assert_eq!(
            section_spec_fingerprint_for(SectionKind::Search),
            SEARCH_LAYOUT_FINGERPRINT
        );
        assert_eq!(
            section_spec_fingerprint_for(SectionKind::Xref),
            XREF_LAYOUT_FINGERPRINT
        );

        // All five discriminants distinct — a copy-paste that gave two
        // sections the same `SectionKind` would let one section's on-disk
        // file silently collide with (and be misread as) another's.
        use crate::rkyvcache::SectionSpec;
        let mut kinds = vec![
            <rkyv::Archived<TreeIndex> as SectionSpec>::KIND,
            <rkyv::Archived<FormsSection> as SectionSpec>::KIND,
            <rkyv::Archived<EdidSection> as SectionSpec>::KIND,
            <rkyv::Archived<SearchSection> as SectionSpec>::KIND,
            <rkyv::Archived<XrefSection> as SectionSpec>::KIND,
        ];
        kinds.sort_by_key(|k| *k as u32);
        kinds.dedup();
        assert_eq!(kinds.len(), 5, "every section must have a distinct KIND");
    }

    /// Arbitrary, test-only `cache_version` — these tests exercise the
    /// `write_section`/`Section::map` mechanics against real section types,
    /// not this module's real `CACHE_VERSION` (which stays private to
    /// `Index::build`/`build_tree_and_forms`).
    const TEST_CACHE_VERSION: u32 = 5252;

    /// Distinct, non-colliding synthetic ESM path per test — same precedent
    /// as `rkyvcache.rs`'s `test_path`/`tree.rs`'s round-trip test, suffixed
    /// with pid+nonce so parallel/sequential `cargo test` runs never collide.
    /// Section paths are derived from this via the real [`section_path_for`],
    /// same as production code, so these tests exercise the actual shared
    /// `esm_cache/` layout rather than a hand-rolled stand-in.
    fn test_esm_path(name: &str) -> PathBuf {
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("esm_index_test_{name}_{pid}_{nonce}.esm"))
    }

    /// Build a `FormsSection` from `form_index` (mirroring
    /// `build_tree_and_forms`'s own conversion), write it via
    /// `write_section`, and map it back via `Section::map`. Shared
    /// boilerplate for every forms-section test below.
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
        write_section(path, sig, TEST_CACHE_VERSION, &data).expect("write forms section");
        let section = Section::<rkyv::Archived<FormsSection>>::map(path, sig, TEST_CACHE_VERSION)
            .expect("map forms section");
        assert!(section.is_mapped(), "freshly written section must map back");
        section
    }

    /// Write `edid_to_form` via `write_section` and map it back — shared
    /// boilerplate for the edid-section tests below, mirroring
    /// [`write_and_map_forms`].
    fn write_and_map_edid(
        edid_to_form: &HashMap<String, u32>,
        path: &Path,
    ) -> Section<rkyv::Archived<EdidSection>> {
        let data = EdidSection {
            edid_to_form: edid_to_form.clone(),
        };
        let sig = CacheSig {
            size: 123_456,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 0,
        };
        write_section(path, sig, TEST_CACHE_VERSION, &data).expect("write edid section");
        let section = Section::<rkyv::Archived<EdidSection>>::map(path, sig, TEST_CACHE_VERSION)
            .expect("map edid section");
        assert!(section.is_mapped(), "freshly written section must map back");
        section
    }

    /// Write `entries` via `write_section` and map it back — shared
    /// boilerplate for the search-section tests below, mirroring
    /// [`write_and_map_forms`].
    fn write_and_map_search(
        entries: &HashMap<u32, SearchMeta>,
        path: &Path,
    ) -> Section<rkyv::Archived<SearchSection>> {
        let data = SearchSection {
            entries: entries.clone(),
        };
        let sig = CacheSig {
            size: 123_456,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 0,
        };
        write_section(path, sig, TEST_CACHE_VERSION, &data).expect("write search section");
        let section = Section::<rkyv::Archived<SearchSection>>::map(path, sig, TEST_CACHE_VERSION)
            .expect("map search section");
        assert!(section.is_mapped(), "freshly written section must map back");
        section
    }

    /// Write `refs` via `write_section` and map it back — shared boilerplate
    /// for the xref-section tests below, mirroring [`write_and_map_forms`].
    fn write_and_map_xref(
        refs: &HashMap<u32, Vec<u32>>,
        path: &Path,
    ) -> Section<rkyv::Archived<XrefSection>> {
        let data = XrefSection { refs: refs.clone() };
        let sig = CacheSig {
            size: 123_456,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 0,
        };
        write_section(path, sig, TEST_CACHE_VERSION, &data).expect("write xref section");
        let section = Section::<rkyv::Archived<XrefSection>>::map(path, sig, TEST_CACHE_VERSION)
            .expect("map xref section");
        assert!(section.is_mapped(), "freshly written section must map back");
        section
    }

    fn index_over(forms: Section<rkyv::Archived<FormsSection>>) -> Index {
        Index {
            path: PathBuf::from("/tmp/test.esm"),
            tree: Section::Absent,
            forms,
            edid: Section::Absent,
            search: Section::Absent,
            xref: Section::Absent,
        }
    }

    fn index_with_edid(edid: Section<rkyv::Archived<EdidSection>>) -> Index {
        Index {
            path: PathBuf::from("/tmp/test.esm"),
            tree: Section::Absent,
            forms: Section::Absent,
            edid,
            search: Section::Absent,
            xref: Section::Absent,
        }
    }

    fn index_with_search(search: Section<rkyv::Archived<SearchSection>>) -> Index {
        Index {
            path: PathBuf::from("/tmp/test.esm"),
            tree: Section::Absent,
            forms: Section::Absent,
            edid: Section::Absent,
            search,
            xref: Section::Absent,
        }
    }

    fn index_with_xref(xref: Section<rkyv::Archived<XrefSection>>) -> Index {
        Index {
            path: PathBuf::from("/tmp/test.esm"),
            tree: Section::Absent,
            forms: Section::Absent,
            edid: Section::Absent,
            search: Section::Absent,
            xref,
        }
    }

    /// Verify that `records_by_type` (through the real `forms` rkyv
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

        let path = section_path_for(
            &test_esm_path("records_by_type_sorted_and_stable"),
            SectionKind::Forms,
        )
        .unwrap();
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

        let path = section_path_for(
            &test_esm_path("forms_round_trip_through_rkyv_section"),
            SectionKind::Forms,
        )
        .unwrap();
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

    /// Regression test: every accessor over an absent forms section
    /// (`Index::empty`'s state, or a cache that hasn't been built yet) must
    /// answer with its empty-equivalent rather than panicking. Mirrors
    /// `tree.rs`'s `tree_view_absent_state_never_panics`; exercised via
    /// `Index::empty` itself (rather than a bare `Section::Absent` built by
    /// hand) since that's the real state a section starts in before its
    /// first build.
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

    /// Round-trip an `EdidSection` built from a small synthetic map through
    /// `write_section` + `Section::map`, and assert `get_by_edid` agrees
    /// with the original for every present EditorID plus an absent one.
    /// Mirrors `forms_round_trip_through_rkyv_section`.
    #[test]
    fn edid_round_trip_through_rkyv_section() {
        let mut edid_to_form: HashMap<String, u32> = HashMap::new();
        edid_to_form.insert("WeapAssaultRifle".to_string(), 0x0000_0010);
        edid_to_form.insert("ArmoVaultSuit".to_string(), 0x0000_0020);
        edid_to_form.insert("NpcRaider01".to_string(), 0x0000_0030);

        let path = section_path_for(
            &test_esm_path("edid_round_trip_through_rkyv_section"),
            SectionKind::Edid,
        )
        .unwrap();
        let index = index_with_edid(write_and_map_edid(&edid_to_form, &path));

        for (edid, &raw) in &edid_to_form {
            assert_eq!(
                index.get_by_edid(edid),
                Some(FormId::new(raw)),
                "edid {edid:?} must resolve to its original FormID"
            );
        }
        assert_eq!(index.get_by_edid("DoesNotExist"), None);

        let _ = fs::remove_file(&path);
    }

    /// Regression test: `get_by_edid` over an absent edid section
    /// (`Index::empty`'s state, or an index that hasn't called
    /// `ensure_edid_index` yet) must return `None` rather than panicking.
    #[test]
    fn edid_absent_state_never_panics() {
        let index = Index::empty(PathBuf::from("/tmp/fo76_edid_absent_test.esm"));
        assert_eq!(index.get_by_edid("Anything"), None);
    }

    /// Round-trip a `SearchSection` built from a small synthetic map through
    /// `write_section` + `Section::map`, and assert `has_search_index`/
    /// `iter_search` agree with the original — including both the
    /// localized-ESM shape (`full_id`/`desc_id`, no inline text) and the
    /// non-localized shape (`full_text`/`desc_text`, no lstring IDs) in the
    /// same map, plus a FormID with no entry simply not appearing.
    #[test]
    fn search_round_trip_through_rkyv_section() {
        let mut entries: HashMap<u32, SearchMeta> = HashMap::new();
        entries.insert(
            0x0000_0010,
            SearchMeta {
                editor_id: Some("WeapAssaultRifle".to_string()),
                full_id: Some(1234),
                desc_id: None,
                full_text: None,
                desc_text: None,
            },
        );
        entries.insert(
            0x0000_0020,
            SearchMeta {
                editor_id: Some("ArmoVaultSuit".to_string()),
                full_id: None,
                desc_id: None,
                full_text: Some("Vault-Tec jumpsuit".to_string()),
                desc_text: Some("A sturdy jumpsuit.".to_string()),
            },
        );

        let path = section_path_for(
            &test_esm_path("search_round_trip_through_rkyv_section"),
            SectionKind::Search,
        )
        .unwrap();
        let index = index_with_search(write_and_map_search(&entries, &path));

        assert!(index.search.is_mapped());
        let results: HashMap<FormId, SearchRef<'_>> = index.iter_search().collect();
        assert_eq!(results.len(), 2);

        let a = results[&FormId::new(0x0000_0010)];
        assert_eq!(a.editor_id, Some("WeapAssaultRifle"));
        assert_eq!(a.full_id, Some(1234));
        assert_eq!(a.desc_id, None);
        assert_eq!(a.full_text, None);
        assert_eq!(a.desc_text, None);

        let b = results[&FormId::new(0x0000_0020)];
        assert_eq!(b.editor_id, Some("ArmoVaultSuit"));
        assert_eq!(b.full_id, None);
        assert_eq!(b.full_text, Some("Vault-Tec jumpsuit"));
        assert_eq!(b.desc_text, Some("A sturdy jumpsuit."));

        assert!(!results.contains_key(&FormId::new(0xDEAD_BEEF)));

        let _ = fs::remove_file(&path);
    }

    /// Regression test: `has_search_index`/`iter_search` over an absent
    /// search section must answer with their empty-equivalent rather than
    /// panicking.
    #[test]
    fn search_absent_state_never_panics() {
        let index = Index::empty(PathBuf::from("/tmp/fo76_search_absent_test.esm"));
        assert!(!index.search.is_mapped());
        assert_eq!(index.iter_search().count(), 0);
    }

    /// Round-trip an `XrefSection` built from a small synthetic map through
    /// `write_section` + `Section::map`, and assert `get_xref` agrees with
    /// the original for every present FormID plus an absent one.
    #[test]
    fn xref_round_trip_through_rkyv_section() {
        let mut refs: HashMap<u32, Vec<u32>> = HashMap::new();
        refs.insert(0x0000_0010, vec![0x0000_0020, 0x0000_0030]);
        refs.insert(0x0000_0040, vec![0x0000_0050]);

        let path = section_path_for(
            &test_esm_path("xref_round_trip_through_rkyv_section"),
            SectionKind::Xref,
        )
        .unwrap();
        let index = index_with_xref(write_and_map_xref(&refs, &path));

        assert_eq!(
            index.get_xref(FormId::new(0x0000_0010)),
            vec![FormId::new(0x0000_0020), FormId::new(0x0000_0030)]
        );
        assert_eq!(
            index.get_xref(FormId::new(0x0000_0040)),
            vec![FormId::new(0x0000_0050)]
        );
        assert_eq!(index.get_xref(FormId::new(0xDEAD_BEEF)), Vec::new());

        let _ = fs::remove_file(&path);
    }

    /// Regression test: `get_xref` over an absent xref section must return
    /// an empty `Vec` rather than panicking.
    #[test]
    fn xref_absent_state_never_panics() {
        let index = Index::empty(PathBuf::from("/tmp/fo76_xref_absent_test.esm"));
        assert_eq!(index.get_xref(FormId::new(1)), Vec::new());
    }

    /// Build a minimal synthetic ESM (TES4 header + one top-level WEAP GRUP
    /// containing one WEAP record) for
    /// [`cross_process_warm_reuse_picks_up_prebuilt_lazy_indexes`] below.
    /// Same byte-level conventions as `tree.rs`'s `build_nested_tree_esm`,
    /// duplicated here rather than shared for the same reason that module's
    /// own comment documents: this `#[cfg(test)]` block compiles inside the
    /// `esm` crate itself, with no visibility into `tree.rs`'s private test
    /// helpers or the separate `tests/` integration-test crate's
    /// `tests/common`.
    fn build_minimal_warm_reuse_esm() -> Vec<u8> {
        fn record(sig: &[u8; 4], form_id: u32) -> Vec<u8> {
            let mut r = Vec::with_capacity(24);
            r.extend_from_slice(sig);
            r.extend_from_slice(&0u32.to_le_bytes()); // data_size
            r.extend_from_slice(&0u32.to_le_bytes()); // flags
            r.extend_from_slice(&form_id.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes()); // vcs1
            r.extend_from_slice(&0u16.to_le_bytes()); // form_version
            r.extend_from_slice(&0u16.to_le_bytes()); // vcs2
            r
        }
        fn grup(label: u32, group_type: i32, body: &[u8]) -> Vec<u8> {
            let group_size = (24 + body.len()) as u32;
            let mut g = Vec::with_capacity(group_size as usize);
            g.extend_from_slice(b"GRUP");
            g.extend_from_slice(&group_size.to_le_bytes());
            g.extend_from_slice(&label.to_le_bytes());
            g.extend_from_slice(&group_type.to_le_bytes());
            g.extend_from_slice(&0u32.to_le_bytes()); // stamp
            g.extend_from_slice(&0u32.to_le_bytes()); // unknown
            g.extend_from_slice(body);
            g
        }

        let mut buf = Vec::new();
        // TES4 header (24 B, data_size = 0).
        buf.extend_from_slice(b"TES4");
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());

        let weap1 = record(b"WEAP", 1);
        let weap_grup = grup(u32::from_le_bytes(*b"WEAP"), 0, &weap1);

        buf.extend_from_slice(&weap_grup);
        buf
    }

    /// The single most important test in this task: proves the
    /// cross-process warm-reuse property [`Index::build`]'s doc comment
    /// promises for `edid`/`search`/`xref` actually holds once each lives in
    /// its own independent section file rather than one shared bincode
    /// blob. Writes all three sections directly via `write_section` —
    /// simulating "some earlier process already called
    /// `ensure_edid_index`/`ensure_search_index`/`ensure_xref_index` and
    /// persisted the result" — then builds a BRAND NEW `Index` via
    /// `Index::build` for the very same (synthetic) ESM and checks every
    /// accessor sees the pre-built data WITHOUT this test ever calling any
    /// `ensure_*_index` method on that fresh `Index`.
    #[test]
    fn cross_process_warm_reuse_picks_up_prebuilt_lazy_indexes() {
        let bytes = build_minimal_warm_reuse_esm();
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir();
        let esm_path = tmp.join(format!("fo76_warm_reuse_test_{pid}_{nonce}.esm"));
        fs::write(&esm_path, &bytes).expect("write synthetic esm");

        let tree_path = section_path_for(&esm_path, SectionKind::Tree).unwrap();
        let forms_path = section_path_for(&esm_path, SectionKind::Forms).unwrap();
        let edid_path = section_path_for(&esm_path, SectionKind::Edid).unwrap();
        let search_path = section_path_for(&esm_path, SectionKind::Search).unwrap();
        let xref_path = section_path_for(&esm_path, SectionKind::Xref).unwrap();

        // "Process A": open once via the real entry point. tree/forms are
        // eager, so this builds them fresh; edid/search/xref are lazy and
        // nothing has called any ensure_*_index yet, so all three start
        // absent.
        let esm = EsmFile::open(&esm_path).expect("open synthetic esm");
        let first = Index::build(&esm).expect("first Index::build (cold)");
        assert!(first.tree.is_mapped(), "sanity: tree built eagerly");
        assert!(first.forms.is_mapped(), "sanity: forms built eagerly");
        assert!(!first.edid.is_mapped(), "sanity: edid starts absent");
        assert!(!first.search.is_mapped(), "sanity: search starts absent");
        assert!(!first.xref.is_mapped(), "sanity: xref starts absent");
        drop(first);
        assert!(
            tree_path.exists(),
            "sanity: tree section written by process A"
        );
        assert!(
            forms_path.exists(),
            "sanity: forms section written by process A"
        );

        // "Process A, later": persist edid/search/xref directly via
        // write_section — exactly the write half of what
        // ensure_edid_index/ensure_search_index/ensure_xref_index would have
        // done, without actually calling them (keeps this test independent
        // of decode.rs/schema.rs plumbing those methods need).
        let sig = CacheSig::read(&esm.path).expect("read cache sig");

        let mut edid_to_form = HashMap::new();
        edid_to_form.insert("PrebuiltWeapon".to_string(), 0x0000_0001);
        write_section(
            &edid_path,
            sig,
            CACHE_VERSION,
            &EdidSection { edid_to_form },
        )
        .expect("write edid section");

        let mut entries = HashMap::new();
        entries.insert(
            0x0000_0001,
            SearchMeta {
                editor_id: Some("PrebuiltWeapon".to_string()),
                full_id: None,
                desc_id: None,
                full_text: Some("A pre-built weapon".to_string()),
                desc_text: None,
            },
        );
        write_section(&search_path, sig, CACHE_VERSION, &SearchSection { entries })
            .expect("write search section");

        let mut refs = HashMap::new();
        refs.insert(0x0000_0001, vec![0x0000_0099]);
        write_section(&xref_path, sig, CACHE_VERSION, &XrefSection { refs })
            .expect("write xref section");

        // "Process B": a BRAND NEW Index::build call for the same ESM. This
        // is the whole point of the test — it must see edid/search/xref
        // already mapped from the files just written above, without this
        // test EVER calling ensure_edid_index/ensure_search_index/
        // ensure_xref_index.
        let second = Index::build(&esm).expect("second Index::build (warm)");
        assert!(second.tree.is_mapped());
        assert!(second.forms.is_mapped());
        assert!(
            second.edid.is_mapped(),
            "process B must see process A's prebuilt edid section"
        );
        assert!(
            second.search.is_mapped(),
            "process B must see process A's prebuilt search section"
        );
        assert!(
            second.xref.is_mapped(),
            "process B must see process A's prebuilt xref section"
        );

        assert_eq!(
            second.get_by_edid("PrebuiltWeapon"),
            Some(FormId::new(0x0000_0001)),
            "process B's get_by_edid must see process A's prebuilt mapping"
        );
        assert!(second.search.is_mapped());
        let found: Vec<_> = second
            .iter_search()
            .filter(|(id, _)| *id == FormId::new(0x0000_0001))
            .collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.editor_id, Some("PrebuiltWeapon"));
        assert_eq!(found[0].1.full_text, Some("A pre-built weapon"));
        assert_eq!(
            second.get_xref(FormId::new(0x0000_0001)),
            vec![FormId::new(0x0000_0099)],
            "process B's get_xref must see process A's prebuilt reverse-reference edge"
        );

        let _ = fs::remove_file(&esm_path);
        // `esm_cache/` is a directory SHARED by every ESM in this parent
        // directory (by design), so removing only this test's own files —
        // never the whole directory — is required, not just tidier.
        let _ = fs::remove_file(&tree_path);
        let _ = fs::remove_file(&forms_path);
        let _ = fs::remove_file(&edid_path);
        let _ = fs::remove_file(&search_path);
        let _ = fs::remove_file(&xref_path);
    }
}
