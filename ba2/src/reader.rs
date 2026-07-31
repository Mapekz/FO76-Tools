//! BA2 archive reader for Fallout 76/FO4 General (GNRL) and DX10 (texture)
//! archives.
//!
//! Ported and extended from `esm-parser/src/ba2.rs`.  Changes vs. the original:
//! - `Ba2Entry` exposes `name_hash`, `dir_hash`, `ext`, and per-kind data for
//!   display.
//! - `read()` is codec-aware: it sniffs the first two bytes to detect zlib vs
//!   LZ4, and accepts an explicit `Codec` override.
//! - DX10 texture archives are read and their entries' DDS headers
//!   synthesized on `read()`; see [`crate::dds`].
//! - Version != 1 causes an error rather than a warning.

use crate::compress::{Codec, decompress};
use crate::dds;
pub use crate::format::TexChunk;
use crate::format::{
    ArchiveKind, HEADER_SIZE, Header, RECORD_SIZE, TEX_CHUNK_SIZE, TEX_RECORD_SIZE, VERSION,
    read_header, read_record, read_tex_chunk, read_tex_record,
};
use anyhow::{Context, Result, bail};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

/// Per-kind payload for a [`Ba2Entry`].
#[derive(Debug, Clone)]
pub enum EntryData {
    /// A GNRL entry: one blob, optionally compressed.
    Gnrl {
        flags: u32,
        data_offset: u64,
        /// Compressed size; 0 means the data is stored uncompressed.
        packed_size: u32,
        unpacked_size: u32,
    },
    /// A DX10 texture entry: dimensions/format plus its mip chunks.
    Texture(TextureInfo),
}

/// Dimensions, format, and mip-chunk layout of a DX10 texture entry.
#[derive(Debug, Clone)]
pub struct TextureInfo {
    pub width: u16,
    pub height: u16,
    pub mip_count: u8,
    pub dxgi_format: u8,
    pub cubemap: bool,
    pub tile_mode: u8,
    /// Mip chunks in on-disk order (chunk 0 first).
    pub chunks: Vec<TexChunk>,
}

/// A single file entry in a BA2 archive (either GNRL or DX10).
#[derive(Debug, Clone)]
pub struct Ba2Entry {
    /// Lowercase path as stored in the name table (backslash-separated).
    pub name: String,
    pub name_hash: u32,
    pub dir_hash: u32,
    pub ext: [u8; 4],
    pub data: EntryData,
}

impl Ba2Entry {
    /// True when this entry's blob (GNRL) or any of its chunks (DX10) is compressed.
    pub fn is_compressed(&self) -> bool {
        match &self.data {
            EntryData::Gnrl { packed_size, .. } => *packed_size != 0,
            EntryData::Texture(t) => t.chunks.iter().any(|c| c.packed_size != 0),
        }
    }

    /// On-disk compressed size: the GNRL blob's `packed_size`, or the sum of
    /// a texture's chunk `packed_size`s (each 0 counts as its `unpacked_size`,
    /// matching how `is_compressed` treats a stored chunk).
    pub fn packed_size(&self) -> u64 {
        match &self.data {
            EntryData::Gnrl { packed_size, .. } => *packed_size as u64,
            EntryData::Texture(t) => t
                .chunks
                .iter()
                .map(|c| if c.packed_size == 0 { c.unpacked_size } else { c.packed_size } as u64)
                .sum(),
        }
    }

    /// Decompressed size: the GNRL blob's `unpacked_size`, or a texture's
    /// full synthesized `.dds` file size (header + all mip chunks).
    pub fn unpacked_size(&self) -> u64 {
        match &self.data {
            EntryData::Gnrl { unpacked_size, .. } => *unpacked_size as u64,
            EntryData::Texture(t) => {
                let header_len =
                    dds::synth_header(t.dxgi_format, t.width, t.height, t.mip_count, t.cubemap)
                        .map(|h| h.len() as u64)
                        .unwrap_or(0);
                header_len + t.chunks.iter().map(|c| c.unpacked_size as u64).sum::<u64>()
            }
        }
    }

    /// This entry's texture metadata, if it is a DX10 entry.
    pub fn texture(&self) -> Option<&TextureInfo> {
        match &self.data {
            EntryData::Texture(t) => Some(t),
            EntryData::Gnrl { .. } => None,
        }
    }
}

/// An open BA2 archive (GNRL or DX10), memory-mapped for zero-copy reads.
pub struct Ba2Archive {
    mmap: Mmap,
    pub entries: Vec<Ba2Entry>,
    by_name: HashMap<String, usize>,
    pub header: Header,
    kind: ArchiveKind,
}

impl Ba2Archive {
    /// Open and parse a BTDX BA2 archive (GNRL or DX10) at `path`.
    ///
    /// Returns an error for unsupported archive types/versions, and for
    /// truncated or otherwise malformed files.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file =
            File::open(path).with_context(|| format!("failed to open BA2: {}", path.display()))?;
        // SAFETY: We hold `file` open for the entire lifetime of `mmap`.
        // No other process is expected to truncate the file while it is mapped.
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &*mmap;

        let header = read_header(data)?;

        if header.version != VERSION {
            bail!(
                "unsupported BA2 version {} (only version {} is supported)",
                header.version,
                VERSION
            );
        }
        let kind = ArchiveKind::from_tag(&header.archive_type)?;

        let file_count = header.file_count as usize;
        let nt_start = header.name_table_offset as usize;
        // Use strict greater-than so an empty archive (nt_start == data.len(),
        // zero entries to read) is accepted rather than incorrectly rejected.
        if nt_start > data.len() {
            bail!("BA2 name table offset out of range");
        }

        let entries = match kind {
            ArchiveKind::Gnrl => Self::parse_gnrl_entries(data, file_count, nt_start)?,
            ArchiveKind::Dx10 => Self::parse_dx10_entries(data, file_count, nt_start)?,
        };

        let mut by_name: HashMap<String, usize> = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            by_name.insert(e.name.clone(), i);
        }

        Ok(Ba2Archive {
            mmap,
            entries,
            by_name,
            header,
            kind,
        })
    }

    /// Parse `count` name-table entries starting at `nt_start`, in record
    /// order. Names are normalised to lowercase, backslash-separated —
    /// consistent whether the archive stores `/` (DX10) or `\` (GNRL).
    fn parse_names(data: &[u8], nt_start: usize, count: usize) -> Result<Vec<String>> {
        let mut pos = nt_start;
        let mut names = Vec::with_capacity(count);
        for i in 0..count {
            if pos + 2 > data.len() {
                bail!("BA2 name table entry {} truncated (no length prefix)", i);
            }
            let name_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            if pos + name_len > data.len() {
                bail!("BA2 name table entry {} string bytes out of range", i);
            }
            let name = String::from_utf8_lossy(&data[pos..pos + name_len])
                .to_lowercase()
                .replace('/', "\\");
            pos += name_len;
            names.push(name);
        }
        Ok(names)
    }

    fn parse_gnrl_entries(
        data: &[u8],
        file_count: usize,
        nt_start: usize,
    ) -> Result<Vec<Ba2Entry>> {
        let records_start = HEADER_SIZE;
        let records_end = records_start
            .checked_add(file_count * RECORD_SIZE)
            .ok_or_else(|| anyhow::anyhow!("BA2 file-count overflow"))?;
        if records_end > data.len() {
            bail!("BA2 file records extend past end of file");
        }

        let mut raw: Vec<crate::format::Record> = Vec::with_capacity(file_count);
        for i in 0..file_count {
            let base = records_start + i * RECORD_SIZE;
            raw.push(read_record(data, base));
        }

        let names = Self::parse_names(data, nt_start, file_count)?;

        Ok(raw
            .into_iter()
            .zip(names)
            .map(|(r, name)| Ba2Entry {
                name,
                name_hash: r.name_hash,
                dir_hash: r.dir_hash,
                ext: r.ext,
                data: EntryData::Gnrl {
                    flags: r.flags,
                    data_offset: r.data_offset,
                    packed_size: r.packed_size,
                    unpacked_size: r.unpacked_size,
                },
            })
            .collect())
    }

    /// Parse DX10 texture entries. Unlike GNRL, the record area has a
    /// variable stride per entry (`TEX_RECORD_SIZE + TEX_CHUNK_SIZE *
    /// chunk_count`), so it is walked sequentially with a per-step bounds
    /// check rather than one bulk check up front.
    fn parse_dx10_entries(
        data: &[u8],
        file_count: usize,
        nt_start: usize,
    ) -> Result<Vec<Ba2Entry>> {
        let mut pos = HEADER_SIZE;
        let mut raw: Vec<(crate::format::TexRecord, Vec<TexChunk>)> =
            Vec::with_capacity(file_count);
        for i in 0..file_count {
            if pos + TEX_RECORD_SIZE > data.len() {
                bail!("BA2 texture record {} truncated", i);
            }
            let tex =
                read_tex_record(data, pos).with_context(|| format!("BA2 texture record {}", i))?;
            pos += TEX_RECORD_SIZE;

            let chunk_count = tex.chunk_count as usize;
            let mut chunks = Vec::with_capacity(chunk_count);
            for c in 0..chunk_count {
                if pos + TEX_CHUNK_SIZE > data.len() {
                    bail!("BA2 texture {} chunk {} truncated", i, c);
                }
                chunks.push(read_tex_chunk(data, pos));
                pos += TEX_CHUNK_SIZE;
            }
            raw.push((tex, chunks));
        }
        if pos > nt_start {
            bail!("BA2 texture records extend past the name table");
        }

        let names = Self::parse_names(data, nt_start, file_count)?;

        Ok(raw
            .into_iter()
            .zip(names)
            .map(|((r, chunks), name)| Ba2Entry {
                name,
                name_hash: r.name_hash,
                dir_hash: r.dir_hash,
                ext: r.ext,
                data: EntryData::Texture(TextureInfo {
                    width: r.width,
                    height: r.height,
                    mip_count: r.mip_count,
                    dxgi_format: r.dxgi_format,
                    cubemap: r.cubemap,
                    tile_mode: r.tile_mode,
                    chunks,
                }),
            })
            .collect())
    }

    /// The archive's on-disk type (GNRL or DX10).
    pub fn kind(&self) -> ArchiveKind {
        self.kind
    }

    /// Return all file entries in the archive.
    pub fn list(&self) -> &[Ba2Entry] {
        &self.entries
    }

    /// Extract and decompress a named file.
    ///
    /// `name` is matched case-insensitively.  `codec` controls decompression:
    /// `Auto` (default) sniffs each blob for zlib vs LZ4. For DX10 entries,
    /// returns a complete synthesized `.dds` file (header + concatenated,
    /// decompressed mip chunks).
    pub fn read(&self, name: &str, codec: Codec) -> Result<Vec<u8>> {
        // Names in the archive are lowercased and backslash-separated.
        // Normalise the caller's input to match.
        let name_lower = name.to_lowercase().replace('/', "\\");
        let &idx = self
            .by_name
            .get(&name_lower)
            .ok_or_else(|| anyhow::anyhow!("file not found in BA2: {}", name))?;
        let entry = &self.entries[idx];
        let data = &*self.mmap;

        match &entry.data {
            EntryData::Gnrl {
                data_offset,
                packed_size,
                unpacked_size,
                ..
            } => Self::read_blob(
                data,
                entry,
                *data_offset,
                *packed_size,
                *unpacked_size,
                codec,
            ),
            EntryData::Texture(t) => {
                let mut out =
                    dds::synth_header(t.dxgi_format, t.width, t.height, t.mip_count, t.cubemap)
                        .with_context(|| {
                            format!("failed to synthesize DDS header for '{}'", entry.name)
                        })?;
                for (i, chunk) in t.chunks.iter().enumerate() {
                    let mip = Self::read_blob(
                        data,
                        entry,
                        chunk.data_offset,
                        chunk.packed_size,
                        chunk.unpacked_size,
                        codec,
                    )
                    .with_context(|| format!("chunk {} of '{}'", i, entry.name))?;
                    if mip.len() != chunk.unpacked_size as usize {
                        bail!(
                            "'{}' chunk {} decompressed to {} bytes, expected {}",
                            entry.name,
                            i,
                            mip.len(),
                            chunk.unpacked_size
                        );
                    }
                    out.extend_from_slice(&mip);
                }
                Ok(out)
            }
        }
    }

    /// Read and decompress a single stored blob (a GNRL entry's data, or one
    /// DX10 chunk) at `data_offset`/`packed_size`/`unpacked_size`.
    fn read_blob(
        data: &[u8],
        entry: &Ba2Entry,
        data_offset: u64,
        packed_size: u32,
        unpacked_size: u32,
        codec: Codec,
    ) -> Result<Vec<u8>> {
        let start = data_offset as usize;
        let stored_len = if packed_size == 0 {
            unpacked_size as usize
        } else {
            packed_size as usize
        };

        if start.saturating_add(stored_len) > data.len() {
            bail!("BA2 entry '{}' data out of range", entry.name);
        }
        let raw = &data[start..start + stored_len];

        if packed_size == 0 {
            // Stored uncompressed.
            Ok(raw.to_vec())
        } else {
            decompress(raw, unpacked_size, codec)
                .with_context(|| format!("decompression failed for '{}'", entry.name))
        }
    }
}
