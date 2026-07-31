//! Tree navigation over the hierarchical GRUP structure of ESM files.
//!
//! ESM records live in a tree of GRUPs (top-level type groups, world cells,
//! etc.). This module provides a flat arena (`TreeIndex`) built by one
//! structural scan of the file, cached in its own rkyv-backed `.esm.tree`
//! section (see [`crate::rkyvcache`] and `index.rs`'s `try_load_cache`/
//! `build_fresh`), and a presentation layer (`GroupLabel`, `GroupNode`,
//! `RecordStub`, `GroupChild`) for browsing.

use crate::format::Signature;
use crate::formid::FormId;
use crate::reader::{EsmFile, WalkEvent};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// The interpreted label of a GRUP, decoded per its `group_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(test, ts(export))]
pub enum GroupLabel {
    /// group_type 0: top-level type group; label is a 4-char record signature.
    RecordType { sig: String },
    /// group_type 1 (world children), 6 (cell persistent), 7 (topic children):
    /// label is a FormID, pre-formatted as hex (e.g. "0x0000463F") — matches the
    /// `RecordRow`/`RefRow` convention of never crossing the JSON boundary as a raw
    /// numeric `FormId`.
    FormId { form_id: String },
    /// group_type 2/3: interior cell block/sub-block; label is a block number.
    InteriorBlock { block: i32 },
    /// group_type 4/5: exterior cell block/sub-block; label packs grid coords.
    ExteriorBlock { grid_y: i16, grid_x: i16 },
    /// group_type 8/9/10: cell persistent/temporary/visible-distant children.
    /// `cell` is pre-formatted hex, same rationale as `FormId` above.
    CellChildren { cell: String },
    /// Unrecognised group_type; raw label preserved.
    Raw { label: u32 },
}

/// A GRUP node in the tree (presentation form, not the cached internal form).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct GroupNode {
    pub group_type: i32,
    pub label: GroupLabel,
    pub child_count: usize,
    /// Byte offset of this GRUP's 24-byte header in the file.
    pub offset: u64,
}

/// A cheap, header-only record listing — no field decode.
///
/// Renamed to `TreeRecordStub` on the TypeScript side (`#[ts(rename)]`) to
/// avoid colliding with `diff::RecordStub`'s generated file — mirrors the
/// `RecordStub as TreeRecordStub` alias `lib.rs` already uses on the Rust side.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, rename = "TreeRecordStub"))]
pub struct RecordStub {
    /// Pre-formatted hex (e.g. "0x0000463F") — same rationale as `GroupLabel::FormId`.
    pub form_id: String,
    pub editor_id: Option<String>,
    pub record_type: String,
    pub offset: u64,
}

/// A single direct child of a GRUP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "node", rename_all = "snake_case")]
#[cfg_attr(test, ts(export))]
pub enum GroupChild {
    Group(GroupNode),
    Record(RecordStub),
}

/// One arena entry per GRUP discovered in the file. Internal/cached.
///
/// `parent`/`children` store arena indices as `u32`, not `usize` — this
/// crate's `rkyv` is pinned to 32-bit pointer width (see the compile-time
/// assert in `rkyvcache.rs`), and the convention here is to pin every
/// archived field width explicitly rather than lean on that assert as the
/// only guarantee. Arena indices never come close to `u32::MAX` in practice
/// (the real ESM has ~125K GRUPs).
#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct GroupEntry {
    pub group_type: i32,
    pub label: u32,
    pub start: u64,
    pub end: u64,
    pub depth: u32,
    pub parent: Option<u32>,
    pub children: Vec<ChildRef>,
}

/// A reference to a direct child of a GRUP — either a nested GRUP (by arena
/// index) or a record header stub. Returned by [`TreeView::children`], so it
/// is `pub` (not just `pub(crate)`) even though its fields carry no
/// presentation formatting of their own — callers pair it with
/// [`TreeView::group_node`] or a record parse to build a presentation type.
///
/// `Group`'s arena index is `u32` for the same reason as [`GroupEntry`]'s
/// `parent`/`children` fields — see that type's doc comment.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ChildRef {
    Group(u32),
    Record {
        form_id: u32,
        offset: u64,
        sig: [u8; 4],
    },
}

/// The cached structural tree of the ESM file, persisted to its own
/// rkyv-backed `.esm.tree` section (see [`crate::rkyvcache`]) rather than
/// bincode-encoded — so every stored index here is `u32`, not `usize`, for
/// the same portability reason documented on [`GroupEntry`].
#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TreeIndex {
    pub(crate) roots: Vec<u32>,
    pub(crate) groups: Vec<GroupEntry>,
    /// Map from GRUP start offset to arena index for O(1) lookup.
    pub(crate) offset_map: std::collections::HashMap<u64, u32>,
}

/// FNV-1a fingerprint of this section's archived layout (`TreeIndex`,
/// `GroupEntry`, `ChildRef`), folding `size_of`/`align_of` per
/// [`crate::rkyvcache::fnv1a_u64`]'s doc comment. Passed as the
/// `layout_fingerprint` argument to `write_section`/`Section::map` for the
/// `.esm.tree` section — see `index.rs`'s `try_load_cache`/`build_fresh`.
pub(crate) const TREE_LAYOUT_FINGERPRINT: u64 = {
    use crate::rkyvcache::{FNV_OFFSET_BASIS, fnv1a_u64};

    let acc = fnv1a_u64(
        FNV_OFFSET_BASIS,
        core::mem::size_of::<rkyv::Archived<TreeIndex>>() as u64,
    );
    let acc = fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<TreeIndex>>() as u64,
    );
    let acc = fnv1a_u64(
        acc,
        core::mem::size_of::<rkyv::Archived<GroupEntry>>() as u64,
    );
    let acc = fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<GroupEntry>>() as u64,
    );
    let acc = fnv1a_u64(acc, core::mem::size_of::<rkyv::Archived<ChildRef>>() as u64);
    fnv1a_u64(
        acc,
        core::mem::align_of::<rkyv::Archived<ChildRef>>() as u64,
    )
};

impl TreeIndex {
    /// Build the tree index from an ESM file via a structural scan.
    pub fn build(esm: &EsmFile) -> Result<TreeIndex> {
        let mut tree = TreeIndex::default();
        // Stack of arena indices of currently-open (entered but not yet exited) groups.
        let mut stack: Vec<usize> = Vec::new();

        esm.walk_structure(|event| {
            match event {
                WalkEvent::GroupStart {
                    offset,
                    group_type,
                    label,
                    group_size,
                } => {
                    let depth = stack.len() as u32;
                    let parent = stack.last().copied();
                    let idx = tree.groups.len();
                    tree.groups.push(GroupEntry {
                        group_type,
                        label,
                        start: offset,
                        end: offset + group_size as u64,
                        depth,
                        // Arena-internal, just-computed index — `as u32` is
                        // infallible in practice (see `GroupEntry`'s doc
                        // comment) and matches this function's existing style
                        // (`depth` above).
                        parent: parent.map(|p| p as u32),
                        children: Vec::new(),
                    });
                    tree.offset_map.insert(offset, idx as u32);
                    // Link as child of parent or as a root
                    if let Some(parent_idx) = parent {
                        tree.groups[parent_idx]
                            .children
                            .push(ChildRef::Group(idx as u32));
                    } else {
                        tree.roots.push(idx as u32);
                    }
                    stack.push(idx);
                }
                WalkEvent::GroupEnd { .. } => {
                    stack.pop();
                }
                WalkEvent::Record(meta) => {
                    if let Some(&parent_idx) = stack.last() {
                        // Convert the record_type string back to a 4-byte sig array
                        let sig_bytes = meta.record_type.as_bytes();
                        let mut sig = [0u8; 4];
                        let copy_len = sig_bytes.len().min(4);
                        sig[..copy_len].copy_from_slice(&sig_bytes[..copy_len]);
                        tree.groups[parent_idx].children.push(ChildRef::Record {
                            form_id: meta.form_id.0,
                            offset: meta.offset,
                            sig,
                        });
                    }
                }
            }
            Ok(())
        })?;

        Ok(tree)
    }

    /// Decode a raw `group_type` + `label` into a [`GroupLabel`].
    pub(crate) fn decode_label(group_type: i32, label: u32) -> GroupLabel {
        match group_type {
            0 => {
                let sig = Signature(label.to_le_bytes()).to_string();
                GroupLabel::RecordType { sig }
            }
            1 | 6 | 7 => GroupLabel::FormId {
                form_id: FormId(label).display(),
            },
            2 | 3 => GroupLabel::InteriorBlock {
                block: label as i32,
            },
            4 | 5 => {
                let grid_y = (label >> 16) as i16;
                let grid_x = label as i16;
                GroupLabel::ExteriorBlock { grid_y, grid_x }
            }
            8..=10 => GroupLabel::CellChildren {
                cell: FormId(label).display(),
            },
            _ => GroupLabel::Raw { label },
        }
    }

    /// Convert an arena entry to a presentation [`GroupNode`].
    ///
    /// Test-only: since Stage 4 moved [`TreeView`]'s real read path onto the
    /// archived `rkyv::Archived<TreeIndex>` (see [`TreeView::group_node`]),
    /// this direct-field-access version has no production caller left — it
    /// exists solely as the "known good" comparison side for the round-trip
    /// test below.
    #[cfg(test)]
    fn group_node(&self, idx: usize) -> GroupNode {
        let entry = &self.groups[idx];
        GroupNode {
            group_type: entry.group_type,
            label: Self::decode_label(entry.group_type, entry.label),
            child_count: entry.children.len(),
            offset: entry.start,
        }
    }

    /// Find the top-level (root) GRUP whose decoded label is
    /// `GroupLabel::RecordType { sig }` matching `sig` exactly. `sig` is
    /// compared as given — callers are expected to uppercase first (matches
    /// the convention `GroupLabel::RecordType`'s `sig` is always uppercase).
    ///
    /// Test-only for the same reason as [`Self::group_node`] above.
    #[cfg(test)]
    fn find_root_by_type(&self, sig: &str) -> Option<usize> {
        self.roots.iter().find_map(|&idx| {
            let idx = idx as usize;
            let entry = &self.groups[idx];
            matches!(
                Self::decode_label(entry.group_type, entry.label),
                GroupLabel::RecordType { sig: ref s } if s == sig
            )
            .then_some(idx)
        })
    }
}

/// Borrowed view over the GRUP tree, covering exactly the four operations
/// [`crate::Database`]'s group-listing methods need. Neither [`TreeIndex`]
/// nor its internal arena types leak through this beyond [`ChildRef`] (a
/// cheap, already-`pub` header stub with no arena internals of its own).
///
/// Wraps `Option<&Archived<TreeIndex>>` rather than a bare reference because
/// the backing [`crate::rkyvcache::Section`] can be `Section::Absent` — no
/// cache built yet, or [`crate::index::Index::empty`]'s lite mode — and every
/// existing caller in `lib.rs` (`list_groups`, `list_type_children`,
/// `list_group_children`, `group_children_at`) expects a working,
/// empty-result answer in that case rather than a panic, exactly as before
/// this type moved to an archived backing. Every method below preserves that:
/// `None` degrades to the empty-equivalent, never a panic.
#[derive(Clone, Copy)]
pub struct TreeView<'a> {
    tree: Option<&'a rkyv::Archived<TreeIndex>>,
}

impl<'a> TreeView<'a> {
    pub(crate) fn new(tree: Option<&'a rkyv::Archived<TreeIndex>>) -> Self {
        Self { tree }
    }

    /// Arena indices of every top-level (group_type == 0) GRUP, in file
    /// order. Empty iterator when no tree section is mapped (lite mode / no
    /// cache built yet).
    pub fn roots(&self) -> impl ExactSizeIterator<Item = usize> + '_ {
        self.tree
            .map_or(&[][..], |t| t.roots.as_slice())
            .iter()
            .map(|idx| idx.to_native() as usize)
    }

    /// Convert an arena entry to a presentation [`GroupNode`].
    ///
    /// # Panics
    ///
    /// Only if called while the tree section is absent — which no caller
    /// respecting the other four methods' contracts can do: [`Self::roots`]
    /// is empty and [`Self::find_root_by_type`]/[`Self::group_idx_at_offset`]/
    /// [`Self::children`] all already return nothing in that state, so there
    /// is never a real `idx` to pass here when `self.tree` is `None`.
    pub fn group_node(&self, idx: usize) -> GroupNode {
        let Some(tree) = self.tree else {
            unreachable!(
                "TreeView::group_node called while the tree section is absent \
                 (idx={idx}) — every idx-producing method on this type already \
                 returns nothing in that state, so this indicates a caller bug, \
                 not routine absent-cache behavior"
            );
        };
        let entry = &tree.groups.as_slice()[idx];
        GroupNode {
            group_type: entry.group_type.to_native(),
            label: TreeIndex::decode_label(entry.group_type.to_native(), entry.label.to_native()),
            child_count: entry.children.len(),
            offset: entry.start.to_native(),
        }
    }

    /// Find the top-level GRUP whose record-type signature matches `sig`
    /// (already uppercased by the caller). `None` if no tree section is
    /// mapped.
    pub fn find_root_by_type(&self, sig: &str) -> Option<usize> {
        let tree = self.tree?;
        tree.roots.iter().find_map(|idx| {
            let idx = idx.to_native() as usize;
            let entry = &tree.groups.as_slice()[idx];
            let label =
                TreeIndex::decode_label(entry.group_type.to_native(), entry.label.to_native());
            matches!(label, GroupLabel::RecordType { sig: ref s } if s == sig).then_some(idx)
        })
    }

    /// Arena index of the GRUP starting at byte `offset`, if any. `None` if
    /// no tree section is mapped.
    pub fn group_idx_at_offset(&self, offset: u64) -> Option<usize> {
        let tree = self.tree?;
        // `offset_map`'s archived key type is the endian-wrapped
        // `rkyv::rend::u64_le`, not a bare `u64` — `ArchivedHashMap::get`
        // requires `K: Borrow<Q>` for the lookup key `Q`, and `u64_le` has no
        // `Borrow<u64>` impl (rend's newtypes only get the blanket
        // `Borrow<T> for T`), so the lookup key must be converted to the
        // exact archived key type first rather than passed as a plain `u64`.
        let key: rkyv::rend::u64_le = offset.into();
        tree.offset_map
            .get(&key)
            .map(|idx| idx.to_native() as usize)
    }

    /// Paginate the direct children of the GRUP at arena index `idx`,
    /// clamping `offset`/`limit` to the actual child count (never panics on
    /// out-of-range pagination). Empty `Vec` if no tree section is mapped.
    pub fn children(&self, idx: usize, offset: usize, limit: usize) -> Vec<ChildRef> {
        let Some(tree) = self.tree else {
            return Vec::new();
        };
        let children = tree.groups.as_slice()[idx].children.as_slice();
        let start = offset.min(children.len());
        let end = (offset + limit).min(children.len());
        children[start..end].iter().map(owned_child_ref).collect()
    }
}

/// Convert one archived [`ChildRef`] back to the owned enum — hand-rolled
/// rather than `rkyv::Deserialize` since `ChildRef` is small, has no
/// allocations to speak of ([u8; 4] aside), and this avoids threading a
/// `rkyv::rancor::Error`/deserializer through what is otherwise an infallible
/// conversion.
fn owned_child_ref(archived: &rkyv::Archived<ChildRef>) -> ChildRef {
    match archived {
        ArchivedChildRef::Group(idx) => ChildRef::Group(idx.to_native()),
        ArchivedChildRef::Record {
            form_id,
            offset,
            sig,
        } => ChildRef::Record {
            form_id: form_id.to_native(),
            offset: offset.to_native(),
            sig: *sig,
        },
    }
}

#[cfg(test)]
// `decode_label` is `pub(crate)` and not reachable from an external `tests/`
// integration crate, so these unit tests stay colocated (two-tier convention
// documented in CLAUDE.md).
mod tests {
    use super::*;

    #[test]
    fn decode_label_record_type() {
        // group_type 0, label = b"WEAP" as little-endian u32
        let weap_label = u32::from_le_bytes(*b"WEAP");
        let decoded = TreeIndex::decode_label(0, weap_label);
        assert!(
            matches!(decoded, GroupLabel::RecordType { ref sig } if sig == "WEAP"),
            "expected RecordType{{WEAP}}, got {:?}",
            decoded
        );
    }

    #[test]
    fn decode_label_exterior_block() {
        // grid_y in high 16 bits, grid_x in low 16 bits
        let label = (3u32 << 16) | (7u32 & 0xFFFF);
        let decoded = TreeIndex::decode_label(4, label);
        assert!(
            matches!(
                decoded,
                GroupLabel::ExteriorBlock {
                    grid_y: 3,
                    grid_x: 7
                }
            ),
            "expected ExteriorBlock{{3,7}}, got {:?}",
            decoded
        );
    }

    #[test]
    fn decode_label_form_id() {
        let decoded = TreeIndex::decode_label(1, 0xDEAD_BEEF);
        assert!(
            matches!(
                decoded,
                GroupLabel::FormId { ref form_id } if form_id == "0xDEADBEEF"
            ),
            "expected FormId(\"0xDEADBEEF\"), got {:?}",
            decoded
        );
    }

    #[test]
    fn decode_label_raw_fallback() {
        let decoded = TreeIndex::decode_label(99, 12345);
        assert!(
            matches!(decoded, GroupLabel::Raw { label: 12345 }),
            "expected Raw{{12345}}, got {:?}",
            decoded
        );
    }

    #[test]
    fn decode_label_cell_children() {
        let decoded = TreeIndex::decode_label(8, 0x0001_0002);
        assert!(
            matches!(
                decoded,
                GroupLabel::CellChildren { ref cell } if cell == "0x00010002"
            ),
            "expected CellChildren(\"0x00010002\"), got {:?}",
            decoded
        );
    }

    #[test]
    fn decode_label_interior_block() {
        let decoded = TreeIndex::decode_label(2, 5);
        assert!(
            matches!(decoded, GroupLabel::InteriorBlock { block: 5 }),
            "expected InteriorBlock{{5}}, got {:?}",
            decoded
        );
    }

    // ── Stage 4: TreeIndex/TreeView through the rkyv `.esm.tree` section ────

    /// Arbitrary, test-only `cache_version` — this test exercises the
    /// `write_section`/`Section::map` mechanics in isolation, not
    /// `index.rs`'s real `CACHE_VERSION` (which is private to that module and
    /// not reachable from here).
    const TEST_CACHE_VERSION: u32 = 4242;

    /// Build a small synthetic ESM exercising nested GRUPs: a top-level WEAP
    /// GRUP containing one WEAP record plus a nested interior-block GRUP
    /// (group_type=2, label=5) with a second WEAP record, and a second
    /// top-level ARMO GRUP containing one ARMO record.
    ///
    /// Mirrors the byte-level conventions of `tests/common::make_minimal_esm`
    /// / `wrap_grup` (duplicated here rather than shared, since this module's
    /// `#[cfg(test)]` block compiles inside the `esm` crate itself and cannot
    /// see `tests/common`, which is only visible to the separate `tests/`
    /// integration-test binaries).
    fn build_nested_tree_esm() -> Vec<u8> {
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
        let weap2 = record(b"WEAP", 2);
        let nested = grup(5, 2, &weap2); // interior block, label = 5
        let mut weap_body = Vec::new();
        weap_body.extend_from_slice(&weap1);
        weap_body.extend_from_slice(&nested);
        let weap_grup = grup(u32::from_le_bytes(*b"WEAP"), 0, &weap_body);

        let armo1 = record(b"ARMO", 3);
        let armo_grup = grup(u32::from_le_bytes(*b"ARMO"), 0, &armo1);

        buf.extend_from_slice(&weap_grup);
        buf.extend_from_slice(&armo_grup);
        buf
    }

    /// Round-trip a `TreeIndex` built from a synthetic ESM through
    /// `write_section` + `Section::map`, and assert every `TreeView` method
    /// (`roots`, `group_node`, `find_root_by_type`, `group_idx_at_offset`,
    /// `children`) agrees with the original in-memory `TreeIndex` for the
    /// same queries. This is the test that actually justifies wiring
    /// `rkyv::access_unchecked` (via `Section::get`) to a real production
    /// type: it proves the writer's archive and the reader's archived-field
    /// idioms (`.to_native()`, the endian-wrapped `offset_map` key, the
    /// `ArchivedChildRef` conversion) agree with the plain in-memory
    /// structure for a tree with nested GRUPs and both `ChildRef` variants.
    #[test]
    fn tree_round_trip_through_rkyv_section() {
        use crate::rkyvcache::{CacheSig, Section, SectionKind, write_section};

        let bytes = build_nested_tree_esm();
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir();
        let esm_path = tmp.join(format!("fo76_tree_rt_test_{pid}_{nonce}.esm"));
        let tree_path = tmp.join(format!("fo76_tree_rt_test_{pid}_{nonce}.esm.tree"));
        std::fs::write(&esm_path, &bytes).expect("write synthetic esm");

        let esm = EsmFile::open(&esm_path).expect("open synthetic esm");
        let original = TreeIndex::build(&esm).expect("build tree index");
        assert_eq!(
            original.groups.len(),
            3,
            "sanity: 2 top-level + 1 nested GRUP"
        );
        assert_eq!(
            original.roots.len(),
            2,
            "sanity: WEAP + ARMO top-level roots"
        );

        let sig = CacheSig::read(&esm_path).expect("read cache sig");
        write_section(
            &tree_path,
            SectionKind::Tree,
            sig,
            TEST_CACHE_VERSION,
            TREE_LAYOUT_FINGERPRINT,
            &original,
        )
        .expect("write tree section");

        let section = Section::<rkyv::Archived<TreeIndex>>::map(
            &tree_path,
            SectionKind::Tree,
            sig,
            TEST_CACHE_VERSION,
            TREE_LAYOUT_FINGERPRINT,
        )
        .expect("map tree section");
        assert!(section.is_mapped(), "freshly written section must map back");

        let view = TreeView::new(section.get());

        // roots(): file order, matches the original exactly.
        let roots: Vec<usize> = view.roots().collect();
        let expected_roots: Vec<usize> = original.roots.iter().map(|&i| i as usize).collect();
        assert_eq!(roots, expected_roots);
        assert_eq!(view.roots().len(), 2);

        // group_node() for every arena entry must match the original.
        for idx in 0..original.groups.len() {
            let got = view.group_node(idx);
            let want = original.group_node(idx);
            assert_eq!(
                got.group_type, want.group_type,
                "group_type mismatch at {idx}"
            );
            assert_eq!(got.label, want.label, "label mismatch at {idx}");
            assert_eq!(
                got.child_count, want.child_count,
                "child_count mismatch at {idx}"
            );
            assert_eq!(got.offset, want.offset, "offset mismatch at {idx}");
        }

        // find_root_by_type()
        assert_eq!(
            view.find_root_by_type("WEAP"),
            original.find_root_by_type("WEAP")
        );
        assert_eq!(
            view.find_root_by_type("ARMO"),
            original.find_root_by_type("ARMO")
        );
        assert_eq!(view.find_root_by_type("NOPE"), None);

        // group_idx_at_offset() for every known GRUP start offset, plus a miss.
        for entry in &original.groups {
            let expected = original
                .offset_map
                .get(&entry.start)
                .copied()
                .map(|i| i as usize);
            assert_eq!(view.group_idx_at_offset(entry.start), expected);
        }
        assert_eq!(view.group_idx_at_offset(0xFFFF_FFFF), None);

        // children(): full child list per arena index, both ChildRef variants.
        for idx in 0..original.groups.len() {
            let got = view.children(idx, 0, usize::MAX);
            let want = &original.groups[idx].children;
            assert_eq!(got.len(), want.len(), "child count mismatch at {idx}");
            for (g, w) in got.iter().zip(want.iter()) {
                match (g, w) {
                    (ChildRef::Group(gi), ChildRef::Group(wi)) => assert_eq!(gi, wi),
                    (
                        ChildRef::Record {
                            form_id: gf,
                            offset: go,
                            sig: gs,
                        },
                        ChildRef::Record {
                            form_id: wf,
                            offset: wo,
                            sig: ws,
                        },
                    ) => {
                        assert_eq!(gf, wf);
                        assert_eq!(go, wo);
                        assert_eq!(gs, ws);
                    }
                    _ => panic!("child kind mismatch at group {idx}: {g:?} vs {w:?}"),
                }
            }
        }
        // Pagination clamps rather than panicking, same as the pre-rkyv version.
        assert_eq!(view.children(0, 100, 5).len(), 0);

        let _ = std::fs::remove_file(&esm_path);
        let _ = std::fs::remove_file(&tree_path);
    }

    /// Regression test for lite-mode behavior: a `TreeView` over an absent
    /// section (no `.esm.tree` written — `Index::empty`'s state, or a cache
    /// that hasn't been built yet) must answer every method with its
    /// empty-equivalent rather than panicking. `group_node` is deliberately
    /// not exercised here — see its doc comment: it is documented as
    /// unreachable in this state via any of the other four methods' own
    /// contracts, so it is not part of the "returns empty" surface being
    /// guarded here.
    #[test]
    fn tree_view_absent_state_never_panics() {
        let view = TreeView::new(None);
        assert_eq!(view.roots().count(), 0);
        assert_eq!(view.find_root_by_type("WEAP"), None);
        assert_eq!(view.group_idx_at_offset(0), None);
        assert!(view.children(0, 0, 10).is_empty());
    }
}
