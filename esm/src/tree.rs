//! Tree navigation over the hierarchical GRUP structure of ESM files.
//!
//! ESM records live in a tree of GRUPs (top-level type groups, world cells,
//! etc.). This module provides a flat arena (`TreeIndex`) built by one
//! structural scan of the file, cached alongside the form index, and a
//! presentation layer (`GroupLabel`, `GroupNode`, `RecordStub`, `GroupChild`)
//! for browsing.

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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct GroupEntry {
    pub group_type: i32,
    pub label: u32,
    pub start: u64,
    pub end: u64,
    pub depth: u32,
    pub parent: Option<usize>,
    pub children: Vec<ChildRef>,
}

/// An internal reference to a direct child of a GRUP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ChildRef {
    Group(usize),
    Record {
        form_id: u32,
        offset: u64,
        sig: [u8; 4],
    },
}

/// The cached structural tree of the ESM file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TreeIndex {
    pub(crate) roots: Vec<usize>,
    pub(crate) groups: Vec<GroupEntry>,
    /// Map from GRUP start offset to arena index for O(1) lookup.
    #[serde(default)]
    pub(crate) offset_map: std::collections::HashMap<u64, usize>,
}

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
                        parent,
                        children: Vec::new(),
                    });
                    tree.offset_map.insert(offset, idx);
                    // Link as child of parent or as a root
                    if let Some(parent_idx) = parent {
                        tree.groups[parent_idx].children.push(ChildRef::Group(idx));
                    } else {
                        tree.roots.push(idx);
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
    pub(crate) fn group_node(&self, idx: usize) -> GroupNode {
        let entry = &self.groups[idx];
        GroupNode {
            group_type: entry.group_type,
            label: Self::decode_label(entry.group_type, entry.label),
            child_count: entry.children.len(),
            offset: entry.start,
        }
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
}
