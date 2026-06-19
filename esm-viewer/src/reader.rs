use crate::compress::decompress_record_data;
use crate::format::{
    GroupHeader, RecordHeader, Signature, Subrecord, SubrecordHeader, GRUP_SIG, HEADER_SIZE,
    SUBRECORD_HEADER_SIZE, TES4_SIG, XXXX_SIG,
};
use crate::formid::FormId;
use anyhow::{bail, Context};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

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
            .map(|s| OwnedSubrecord {
                signature: s.signature,
                data: s.data.to_vec(),
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
            pending_size = Some(u32::from_le_bytes([
                data[pos],
                data[pos + 1],
                data[pos + 2],
                data[pos + 3],
            ]) as usize);
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
