use crate::compress::decompress_record_data;
use crate::format::{
    GroupHeader, RecordHeader, Signature, Subrecord, SubrecordHeader, COMPRESSED_FLAG, GRUP_SIG,
    HEADER_SIZE, SUBRECORD_HEADER_SIZE, TES4_SIG, XXXX_SIG,
};
use crate::formid::FormId;
use anyhow::{bail, Context};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

/// A record reference emitted by [`EsmFile::walk_structure`].
#[derive(Debug, Clone)]
pub struct StructuralRecord {
    pub form_id: FormId,
    pub record_type: String,
    pub offset: u64,
}

/// Event emitted by [`EsmFile::walk_structure`] as it traverses the GRUP tree.
pub enum WalkEvent {
    /// A GRUP node has been entered.
    GroupStart {
        offset: u64,
        group_type: i32,
        label: u32,
        group_size: u32,
    },
    /// A GRUP node has been fully traversed (all children emitted).
    GroupEnd { offset: u64 },
    /// A regular record (non-GRUP) was encountered.
    Record(StructuralRecord),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: PathBuf,
    pub version: f32,
    pub record_count: u32,
    pub next_object_id: u32,
    pub author: Option<String>,
    pub description: Option<String>,
    pub masters: Vec<String>,
    pub flags: u32,
    pub is_esm: bool,
    pub is_localized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordMeta {
    pub offset: u64,
    pub signature: String,
    pub flags: u32,
    pub form_version: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordHeaderInfo {
    pub signature: String,
    pub form_id: FormId,
    pub flags: u32,
    pub form_version: u16,
    pub data_size: u32,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct OwnedSubrecord {
    pub signature: Signature,
    pub data: Vec<u8>,
    /// Position of this subrecord within its parent record's subrecord list
    /// (0-based). Used by the decoder to resolve cross-signature ordering
    /// when `stop_before` is set on an `rarray` schema member.
    pub doc_index: usize,
}

#[derive(Debug, Clone)]
pub struct ParsedRecord {
    pub header: RecordHeaderInfo,
    pub subrecords: Vec<OwnedSubrecord>,
}

pub struct EsmFile {
    pub mmap: Mmap,
    pub path: PathBuf,
}

impl EsmFile {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: We hold the file open for the lifetime of `Mmap`; no other process
        // is expected to truncate the file while it is mapped.
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(EsmFile { mmap, path })
    }

    pub fn data(&self) -> &[u8] {
        &self.mmap
    }

    pub fn file_info(&self) -> anyhow::Result<FileInfo> {
        parse_tes4_header(self.data())
    }

    pub fn record_header_at(&self, offset: u64) -> anyhow::Result<RecordHeaderInfo> {
        let data = self.data();
        if offset as usize + HEADER_SIZE as usize > data.len() {
            bail!("record offset out of range");
        }
        let hdr = RecordHeader::parse(&data[offset as usize..])?;
        Ok(RecordHeaderInfo {
            signature: hdr.signature.to_string(),
            form_id: FormId::new(hdr.form_id),
            flags: hdr.flags,
            form_version: hdr.form_version,
            data_size: hdr.data_size,
            offset,
        })
    }

    pub fn parse_record_at(&self, offset: u64) -> anyhow::Result<ParsedRecord> {
        let data = self.data();
        let header = self.record_header_at(offset)?;
        let hdr = RecordHeader::parse(&data[offset as usize..])?;
        let data_start = offset as usize + HEADER_SIZE as usize;
        let data_end = data_start + hdr.data_size as usize;
        if data_end > data.len() {
            bail!("record data out of range");
        }
        let raw = &data[data_start..data_end];
        let payload = if hdr.is_compressed() {
            decompress_record_data(raw)?
        } else {
            raw.to_vec()
        };
        let subrecords = parse_subrecords_owned(&payload)?;
        Ok(ParsedRecord { header, subrecords })
    }

    /// Returns the decompressed data payload bytes for the record at `offset`.
    /// Header bytes are excluded; for compressed records the zlib payload is decompressed.
    pub fn record_payload_at(&self, offset: u64) -> anyhow::Result<Vec<u8>> {
        let data = self.data();
        let hdr = RecordHeader::parse(&data[offset as usize..])?;
        let data_start = offset as usize + HEADER_SIZE as usize;
        let data_end = data_start + hdr.data_size as usize;
        if data_end > data.len() {
            anyhow::bail!("record data out of range");
        }
        let raw = &data[data_start..data_end];
        if hdr.flags & COMPRESSED_FLAG != 0 {
            decompress_record_data(raw)
        } else {
            Ok(raw.to_vec())
        }
    }

    pub fn walk_records<F>(&self, mut f: F) -> anyhow::Result<()>
    where
        F: FnMut(RecordMeta) -> anyhow::Result<()>,
    {
        let data = self.data();
        if data.len() < HEADER_SIZE as usize {
            bail!("file too small");
        }
        let sig = Signature::from_slice(&data[0..4]);
        if sig.0 != TES4_SIG {
            bail!("expected TES4 record at start");
        }
        let tes4 = RecordHeader::parse(&data[0..HEADER_SIZE as usize])?;
        let start = HEADER_SIZE + tes4.data_size as u64;
        let end = data.len() as u64;
        walk_container(data, start, end, &mut f)
    }

    /// Walk the GRUP/record tree emitting [`WalkEvent`]s.
    ///
    /// Skips the TES4 file header record and begins from the first top-level
    /// GRUP. Does not modify or call into the existing [`walk_records`] path.
    ///
    /// [`walk_records`]: EsmFile::walk_records
    pub fn walk_structure<F>(&self, mut f: F) -> anyhow::Result<()>
    where
        F: FnMut(WalkEvent) -> anyhow::Result<()>,
    {
        let data = self.data();
        if data.len() < HEADER_SIZE as usize {
            bail!("file too small for walk_structure");
        }
        let tes4 = RecordHeader::parse(&data[0..HEADER_SIZE as usize])?;
        let start = HEADER_SIZE as usize + tes4.data_size as usize;
        let end = data.len();
        self.walk_structure_container(data, start, end, &mut f)
    }

    fn walk_structure_container<F>(
        &self,
        data: &[u8],
        mut pos: usize,
        end: usize,
        f: &mut F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(WalkEvent) -> anyhow::Result<()>,
    {
        while pos + HEADER_SIZE as usize <= end {
            // Need at least 4 bytes to identify GRUP vs record
            if pos + 4 > data.len() {
                break;
            }
            if data[pos..pos + 4] == GRUP_SIG {
                // GRUP header: sig(4) + group_size(4) + label(4) + group_type(4) + stamp(4) + unknown(4)
                let gh = GroupHeader::parse(&data[pos..])?;
                let group_size = gh.group_size as usize;
                // group_size includes the 24-byte header itself
                let group_end = pos.saturating_add(group_size);
                if group_end > end {
                    bail!("GRUP extends beyond container at offset {}", pos);
                }
                f(WalkEvent::GroupStart {
                    offset: pos as u64,
                    group_type: gh.group_type,
                    label: gh.label,
                    group_size: gh.group_size,
                })?;
                // Walk children (after the 24-byte GRUP header)
                self.walk_structure_container(data, pos + HEADER_SIZE as usize, group_end, f)?;
                f(WalkEvent::GroupEnd { offset: pos as u64 })?;
                pos = group_end;
            } else {
                // Regular record header
                let rh = RecordHeader::parse(&data[pos..])?;
                let record_end = pos
                    .saturating_add(HEADER_SIZE as usize)
                    .saturating_add(rh.data_size as usize);
                if record_end > end {
                    break;
                }
                f(WalkEvent::Record(StructuralRecord {
                    form_id: FormId::new(rh.form_id),
                    record_type: rh.signature.to_string(),
                    offset: pos as u64,
                }))?;
                pos = record_end;
            }
        }
        Ok(())
    }
}

fn walk_container<F>(data: &[u8], mut pos: u64, end: u64, f: &mut F) -> anyhow::Result<()>
where
    F: FnMut(RecordMeta) -> anyhow::Result<()>,
{
    while pos + HEADER_SIZE <= end {
        let slice = &data[pos as usize..];
        if slice.starts_with(&GRUP_SIG) {
            let gh = GroupHeader::parse(&slice[..HEADER_SIZE as usize])?;
            let group_end = pos + gh.group_size as u64;
            if group_end > end {
                bail!("group extends past container");
            }
            walk_container(data, pos + HEADER_SIZE, group_end, f)?;
            pos = group_end;
        } else {
            let rh = RecordHeader::parse(&slice[..HEADER_SIZE as usize])?;
            f(RecordMeta {
                offset: pos,
                signature: rh.signature.to_string(),
                flags: rh.flags,
                form_version: rh.form_version,
            })?;
            pos += rh.total_size();
        }
    }
    Ok(())
}

pub fn parse_subrecords_owned(data: &[u8]) -> anyhow::Result<Vec<OwnedSubrecord>> {
    parse_subrecords(data).map(|subs| {
        subs.into_iter()
            .enumerate()
            .map(|(doc_index, s)| OwnedSubrecord {
                signature: s.signature,
                data: s.data.to_vec(),
                doc_index,
            })
            .collect()
    })
}

pub fn parse_subrecords(data: &[u8]) -> anyhow::Result<Vec<Subrecord<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut pending_size: Option<usize> = None;

    while pos + SUBRECORD_HEADER_SIZE <= data.len() {
        let hdr = SubrecordHeader::parse(&data[pos..])?;
        let sig = hdr.signature;
        pos += SUBRECORD_HEADER_SIZE;

        if sig.0 == XXXX_SIG && hdr.size == 4 && pos + 4 <= data.len() {
            pending_size =
                Some(
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                        as usize,
                );
            pos += 4;
            continue;
        }

        let mut size = hdr.size as usize;
        if size == 0 {
            if let Some(real) = pending_size.take() {
                size = real;
            }
        } else {
            pending_size = None;
        }

        let end = pos.saturating_add(size).min(data.len());
        out.push(Subrecord {
            signature: sig,
            data: &data[pos..end],
        });
        pos = end;
    }
    Ok(out)
}

fn read_zstring(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

pub fn parse_tes4_header(data: &[u8]) -> anyhow::Result<FileInfo> {
    if data.len() < HEADER_SIZE as usize {
        bail!("file too small for TES4");
    }
    let hdr = RecordHeader::parse(&data[0..HEADER_SIZE as usize])?;
    if hdr.signature.0 != TES4_SIG {
        bail!("not a TES4 header");
    }
    let start = HEADER_SIZE as usize;
    let end = start + hdr.data_size as usize;
    let payload = if hdr.is_compressed() {
        decompress_record_data(&data[start..end])?
    } else {
        data[start..end].to_vec()
    };
    let subs = parse_subrecords(&payload)?;

    let mut version = 0.0f32;
    let mut record_count = 0u32;
    let mut next_object_id = 0u32;
    let mut author = None;
    let mut description = None;
    let mut masters = Vec::new();
    let mut in_master = false;

    for sr in &subs {
        match sr.signature.as_str() {
            "HEDR" if sr.data.len() >= 12 => {
                version = f32::from_le_bytes(sr.data[0..4].try_into()?);
                record_count = u32::from_le_bytes(sr.data[4..8].try_into()?);
                next_object_id = u32::from_le_bytes(sr.data[8..12].try_into()?);
            }
            "CNAM" => author = Some(read_zstring(sr.data)),
            "SNAM" => description = Some(read_zstring(sr.data)),
            "MAST" => {
                in_master = true;
                masters.push(read_zstring(sr.data));
            }
            "DATA" if in_master => {
                in_master = false;
            }
            _ => {}
        }
    }

    Ok(FileInfo {
        path: PathBuf::new(),
        version,
        record_count,
        next_object_id,
        author,
        description,
        masters,
        flags: hdr.flags,
        is_esm: hdr.flags & 0x01 != 0,
        is_localized: hdr.flags & 0x80 != 0,
    })
}

pub fn edid_from_subrecords(subs: &[OwnedSubrecord]) -> Option<String> {
    subs.iter()
        .find(|s| s.signature.as_str() == "EDID")
        .map(|s| read_zstring(&s.data))
}

pub fn lstring_id_from_subrecords(subs: &[OwnedSubrecord], sig: &str) -> Option<u32> {
    subs.iter()
        .find(|s| s.signature.as_str() == sig)
        .filter(|s| s.data.len() >= 4)
        .map(|s| u32::from_le_bytes(s.data[0..4].try_into().unwrap()))
}

/// Read a subrecord's data as an inline NUL-terminated string (for
/// non-localized ESMs), stripping an optional `<ID=XXXXXXXX>` prefix.
///
/// Returns `None` if the subrecord is absent or its data is empty.
pub fn inline_string_from_subrecords(subs: &[OwnedSubrecord], sig: &str) -> Option<String> {
    let sr = subs.iter().find(|s| s.signature.as_str() == sig)?;
    if sr.data.is_empty() {
        return None;
    }
    let nul_end = sr
        .data
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(sr.data.len());
    if nul_end == 0 {
        return None;
    }
    let s = String::from_utf8_lossy(&sr.data[..nul_end]);
    // Strip the optional `<ID=XXXXXXXX>` reference marker.
    let text = if s.starts_with("<ID=") {
        if let Some(close) = s.find('>') {
            let remainder = s[close + 1..].trim_start();
            if remainder.is_empty() {
                return None;
            }
            remainder.to_string()
        } else {
            s.into_owned()
        }
    } else {
        s.into_owned()
    };
    Some(text)
}

#[cfg(test)]
mod structural_tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal ESM byte buffer:
    /// - TES4 record: 24 bytes, data_size=0
    /// - GRUP: 24 bytes header + 2 × 24-byte child records = 72 bytes total (group_size=72)
    /// - 2 WEAP records (data_size=0 each)
    fn make_minimal_esm() -> Vec<u8> {
        let mut buf = Vec::new();

        // TES4 header: sig=TES4, data_size=0, flags=0, form_id=0, vcs1=0, form_version=0, vcs2=0
        buf.extend_from_slice(b"TES4"); // signature
        buf.extend_from_slice(&0u32.to_le_bytes()); // data_size = 0
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&0u32.to_le_bytes()); // form_id
        buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
        buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
        buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2
                                                    // TES4 data_size=0, so no payload bytes

        // GRUP header: sig=GRUP, group_size=72, label=WEAP, group_type=0, stamp=0, unknown=0
        // group_size = 24 (header) + 24 (rec1) + 24 (rec2) = 72
        let group_size: u32 = 72;
        let label = u32::from_le_bytes(*b"WEAP");
        buf.extend_from_slice(b"GRUP"); // signature
        buf.extend_from_slice(&group_size.to_le_bytes()); // group_size
        buf.extend_from_slice(&label.to_le_bytes()); // label
        buf.extend_from_slice(&0i32.to_le_bytes()); // group_type = 0 (top-level)
        buf.extend_from_slice(&0u32.to_le_bytes()); // stamp
        buf.extend_from_slice(&0u32.to_le_bytes()); // unknown

        // WEAP record 1: sig=WEAP, data_size=0, flags=0, form_id=1, vcs1=0, form_version=0, vcs2=0
        buf.extend_from_slice(b"WEAP");
        buf.extend_from_slice(&0u32.to_le_bytes()); // data_size
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&1u32.to_le_bytes()); // form_id = 1
        buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
        buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
        buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2

        // WEAP record 2: form_id = 2
        buf.extend_from_slice(b"WEAP");
        buf.extend_from_slice(&0u32.to_le_bytes()); // data_size
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&2u32.to_le_bytes()); // form_id = 2
        buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
        buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
        buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2

        buf
    }

    #[test]
    fn walk_structure_events_sequence() {
        let buf = make_minimal_esm();

        // Write to a temp file so EsmFile::open can mmap it
        let tmp_path = std::env::temp_dir().join("fo76_esm_test_walk_structure.esm");
        {
            let mut f = std::fs::File::create(&tmp_path).expect("create temp file");
            f.write_all(&buf).expect("write");
        }

        let esm = EsmFile::open(&tmp_path).expect("open");

        let mut events = Vec::new();
        esm.walk_structure(|ev| {
            match &ev {
                WalkEvent::GroupStart {
                    group_type, label, ..
                } => {
                    events.push(format!("GroupStart(type={},label={})", group_type, label));
                }
                WalkEvent::GroupEnd { .. } => {
                    events.push("GroupEnd".to_string());
                }
                WalkEvent::Record(r) => {
                    events.push(format!("Record({},{})", r.record_type, r.form_id.0));
                }
            }
            Ok(())
        })
        .expect("walk_structure");

        let _ = std::fs::remove_file(&tmp_path);

        assert_eq!(
            events.len(),
            4,
            "expected GroupStart, Record, Record, GroupEnd; got {:?}",
            events
        );
        assert!(
            events[0].starts_with("GroupStart"),
            "first event is GroupStart"
        );
        assert_eq!(events[1], "Record(WEAP,1)");
        assert_eq!(events[2], "Record(WEAP,2)");
        assert_eq!(events[3], "GroupEnd");
    }
}
