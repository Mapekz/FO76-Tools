//! On-disk constants and (de)serialization for the BA2 (BTDX) binary format.
//!
//! Both the 24-byte file header and the 36-byte per-file GNRL record are
//! represented as plain byte arrays — no derive macros, just explicit LE reads
//! and writes so the layout is crystal-clear and testable. The DX10 (texture)
//! per-file header and per-chunk records follow the same convention.

use anyhow::{Result, bail};

// ── Little-endian field-read helpers ─────────────────────────────────────────
//
// Fixed-offset reads go through these so call sites are `read_u32(data, 8)`
// instead of `u32::from_le_bytes(data[8..12].try_into().unwrap())`. Offsets
// stay explicit literals at each call site; only the cast noise is removed.
// Callers are responsible for having already length-checked `data` — these
// helpers panic (via slice indexing) exactly as the inlined form did.

/// Read a little-endian `u16` from `data` at byte offset `off`.
fn read_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(data[off..off + 2].try_into().unwrap())
}

/// Read a little-endian `u32` from `data` at byte offset `off`.
fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

/// Read a little-endian `u64` from `data` at byte offset `off`.
fn read_u64(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
}

// ── Magic / type tags ────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"BTDX";
pub const TAG_GNRL: &[u8; 4] = b"GNRL";
pub const TAG_DX10: &[u8; 4] = b"DX10";

/// The archive-type tag stored in byte 8..12 of the header.
///
/// Both GNRL and DX10 archives share the same 24-byte header and trailing
/// name table; only the per-file record layout differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Gnrl,
    Dx10,
}

impl ArchiveKind {
    /// Identify the archive kind from a header's raw type tag.
    pub fn from_tag(tag: &[u8; 4]) -> Result<Self> {
        if tag == TAG_GNRL {
            Ok(ArchiveKind::Gnrl)
        } else if tag == TAG_DX10 {
            Ok(ArchiveKind::Dx10)
        } else {
            bail!(
                "unsupported BA2 archive type {:?}; expected GNRL or DX10",
                tag
            );
        }
    }

    /// The raw 4-byte tag for this kind.
    pub fn tag(self) -> &'static [u8; 4] {
        match self {
            ArchiveKind::Gnrl => TAG_GNRL,
            ArchiveKind::Dx10 => TAG_DX10,
        }
    }
}

// ── Size constants ───────────────────────────────────────────────────────────

pub const HEADER_SIZE: usize = 24;
pub const RECORD_SIZE: usize = 36;

/// Size of a DX10 per-file texture header (not counting its chunk records).
pub const TEX_RECORD_SIZE: usize = 24;
/// Size of a single DX10 per-chunk record.
pub const TEX_CHUNK_SIZE: usize = 24;
/// The on-disk `chunk_header_size` field is always this value; validated on read.
pub const TEX_CHUNK_HEADER_SIZE: u16 = 24;
/// Bit set in the texture header's `cubemap` byte when the entry is a cubemap.
pub const TEX_FLAG_CUBEMAP: u8 = 0x01;
/// Observed `tile_mode` byte for every PC-targeted FO76 texture (linear tiling).
pub const TEX_TILE_MODE_LINEAR: u8 = 0x08;

// ── Field constants (ground-truthed against SeventySix - Localization.ba2) ──

/// Every GNRL file record carries this flags value.
pub const RECORD_FLAGS: u32 = 0x0010_0100;
/// Sentinel padding at the end of every GNRL record and every DX10 chunk record.
pub const PADDING: u32 = 0xBAAD_F00D;
/// Supported archive version.
pub const VERSION: u32 = 1;

// ── Header ───────────────────────────────────────────────────────────────────

/// Parsed BA2 file header (24 bytes).
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub version: u32,
    pub archive_type: [u8; 4],
    pub file_count: u32,
    pub name_table_offset: u64,
}

/// Read a 24-byte header from the start of a mapped file.
pub fn read_header(data: &[u8]) -> Result<Header> {
    if data.len() < HEADER_SIZE {
        bail!("BA2 too small to contain a header ({} bytes)", data.len());
    }
    if &data[0..4] != MAGIC {
        bail!("not a BA2 archive (bad magic {:?})", &data[0..4]);
    }
    let version = read_u32(data, 4);
    let mut archive_type = [0u8; 4];
    archive_type.copy_from_slice(&data[8..12]);
    let file_count = read_u32(data, 12);
    let name_table_offset = read_u64(data, 16);
    Ok(Header {
        version,
        archive_type,
        file_count,
        name_table_offset,
    })
}

/// Serialize a header to exactly 24 bytes.
pub fn write_header(
    version: u32,
    kind: ArchiveKind,
    file_count: u32,
    name_table_offset: u64,
) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];
    buf[0..4].copy_from_slice(MAGIC);
    buf[4..8].copy_from_slice(&version.to_le_bytes());
    buf[8..12].copy_from_slice(kind.tag());
    buf[12..16].copy_from_slice(&file_count.to_le_bytes());
    buf[16..24].copy_from_slice(&name_table_offset.to_le_bytes());
    buf
}

// ── GNRL record ──────────────────────────────────────────────────────────────

/// Parsed GNRL file record (36 bytes).
#[derive(Debug, Clone, Copy)]
pub struct Record {
    pub name_hash: u32,
    pub ext: [u8; 4],
    pub dir_hash: u32,
    pub flags: u32,
    pub data_offset: u64,
    /// Compressed size; 0 means the data is stored uncompressed.
    pub packed_size: u32,
    pub unpacked_size: u32,
    // padding field (0xBAADF00D) is consumed on read and emitted on write, not stored.
}

/// Read a single 36-byte GNRL record from a slice at byte offset `base`.
///
/// The caller must guarantee `base + RECORD_SIZE <= data.len()` before calling.
pub fn read_record(data: &[u8], base: usize) -> Record {
    let name_hash = read_u32(data, base);
    let mut ext = [0u8; 4];
    ext.copy_from_slice(&data[base + 4..base + 8]);
    let dir_hash = read_u32(data, base + 8);
    let flags = read_u32(data, base + 12);
    let data_offset = read_u64(data, base + 16);
    let packed_size = read_u32(data, base + 24);
    let unpacked_size = read_u32(data, base + 28);
    // data[base+32..base+36] is the 0xBAADF00D padding — read past it, don't store.
    Record {
        name_hash,
        ext,
        dir_hash,
        flags,
        data_offset,
        packed_size,
        unpacked_size,
    }
}

/// Serialize a GNRL record to exactly 36 bytes.
pub fn write_record(r: &Record) -> [u8; RECORD_SIZE] {
    let mut buf = [0u8; RECORD_SIZE];
    buf[0..4].copy_from_slice(&r.name_hash.to_le_bytes());
    buf[4..8].copy_from_slice(&r.ext);
    buf[8..12].copy_from_slice(&r.dir_hash.to_le_bytes());
    buf[12..16].copy_from_slice(&r.flags.to_le_bytes());
    buf[16..24].copy_from_slice(&r.data_offset.to_le_bytes());
    buf[24..28].copy_from_slice(&r.packed_size.to_le_bytes());
    buf[28..32].copy_from_slice(&r.unpacked_size.to_le_bytes());
    buf[32..36].copy_from_slice(&PADDING.to_le_bytes());
    buf
}

// ── DX10 texture header ──────────────────────────────────────────────────────

/// Parsed DX10 per-file texture header (24 bytes, precedes `chunk_count`
/// [`TexChunk`] records).
#[derive(Debug, Clone, Copy)]
pub struct TexRecord {
    pub name_hash: u32,
    pub ext: [u8; 4],
    pub dir_hash: u32,
    pub chunk_count: u8,
    pub height: u16,
    pub width: u16,
    pub mip_count: u8,
    pub dxgi_format: u8,
    pub cubemap: bool,
    pub tile_mode: u8,
}

/// Read a 24-byte DX10 texture header from a slice at byte offset `base`.
///
/// The caller must guarantee `base + TEX_RECORD_SIZE <= data.len()` before
/// calling. Bails if the on-disk `chunk_header_size` field is not 24 — every
/// FO76 texture entry carries that value, and a mismatch means the record
/// layout does not match our assumptions rather than a decodable variant.
pub fn read_tex_record(data: &[u8], base: usize) -> Result<TexRecord> {
    let name_hash = read_u32(data, base);
    let mut ext = [0u8; 4];
    ext.copy_from_slice(&data[base + 4..base + 8]);
    let dir_hash = read_u32(data, base + 8);
    // data[base+12] is `unk8` — consumed, not stored (always 0 in observed data).
    let chunk_count = data[base + 13];
    let chunk_header_size = read_u16(data, base + 14);
    if chunk_header_size != TEX_CHUNK_HEADER_SIZE {
        bail!(
            "unexpected DX10 chunk_header_size {} (expected {})",
            chunk_header_size,
            TEX_CHUNK_HEADER_SIZE
        );
    }
    let height = read_u16(data, base + 16);
    let width = read_u16(data, base + 18);
    let mip_count = data[base + 20];
    let dxgi_format = data[base + 21];
    let cubemap = data[base + 22] & TEX_FLAG_CUBEMAP != 0;
    let tile_mode = data[base + 23];
    Ok(TexRecord {
        name_hash,
        ext,
        dir_hash,
        chunk_count,
        height,
        width,
        mip_count,
        dxgi_format,
        cubemap,
        tile_mode,
    })
}

/// Serialize a DX10 texture header to exactly 24 bytes.
pub fn write_tex_record(r: &TexRecord) -> [u8; TEX_RECORD_SIZE] {
    let mut buf = [0u8; TEX_RECORD_SIZE];
    buf[0..4].copy_from_slice(&r.name_hash.to_le_bytes());
    buf[4..8].copy_from_slice(&r.ext);
    buf[8..12].copy_from_slice(&r.dir_hash.to_le_bytes());
    buf[12] = 0; // unk8
    buf[13] = r.chunk_count;
    buf[14..16].copy_from_slice(&TEX_CHUNK_HEADER_SIZE.to_le_bytes());
    buf[16..18].copy_from_slice(&r.height.to_le_bytes());
    buf[18..20].copy_from_slice(&r.width.to_le_bytes());
    buf[20] = r.mip_count;
    buf[21] = r.dxgi_format;
    buf[22] = if r.cubemap { TEX_FLAG_CUBEMAP } else { 0 };
    buf[23] = r.tile_mode;
    buf
}

// ── DX10 chunk record ────────────────────────────────────────────────────────

/// Parsed DX10 per-chunk record (24 bytes); a texture entry has `chunk_count`
/// of these immediately following its [`TexRecord`].
#[derive(Debug, Clone, Copy)]
pub struct TexChunk {
    pub data_offset: u64,
    /// Compressed size; 0 means the chunk is stored uncompressed.
    pub packed_size: u32,
    pub unpacked_size: u32,
    pub mip_first: u16,
    pub mip_last: u16,
    // sentinel field (0xBAADF00D) is consumed on read and emitted on write, not stored.
}

/// Read a single 24-byte DX10 chunk record from a slice at byte offset `base`.
///
/// The caller must guarantee `base + TEX_CHUNK_SIZE <= data.len()` before calling.
pub fn read_tex_chunk(data: &[u8], base: usize) -> TexChunk {
    let data_offset = read_u64(data, base);
    let packed_size = read_u32(data, base + 8);
    let unpacked_size = read_u32(data, base + 12);
    let mip_first = read_u16(data, base + 16);
    let mip_last = read_u16(data, base + 18);
    // data[base+20..base+24] is the 0xBAADF00D sentinel — read past it, don't store.
    TexChunk {
        data_offset,
        packed_size,
        unpacked_size,
        mip_first,
        mip_last,
    }
}

/// Serialize a DX10 chunk record to exactly 24 bytes.
pub fn write_tex_chunk(c: &TexChunk) -> [u8; TEX_CHUNK_SIZE] {
    let mut buf = [0u8; TEX_CHUNK_SIZE];
    buf[0..8].copy_from_slice(&c.data_offset.to_le_bytes());
    buf[8..12].copy_from_slice(&c.packed_size.to_le_bytes());
    buf[12..16].copy_from_slice(&c.unpacked_size.to_le_bytes());
    buf[16..18].copy_from_slice(&c.mip_first.to_le_bytes());
    buf[18..20].copy_from_slice(&c.mip_last.to_le_bytes());
    buf[20..24].copy_from_slice(&PADDING.to_le_bytes());
    buf
}
