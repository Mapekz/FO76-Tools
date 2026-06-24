//! On-disk constants and (de)serialization for the BA2 (BTDX) binary format.
//!
//! Both the 24-byte file header and the 36-byte per-file GNRL record are
//! represented as plain byte arrays — no derive macros, just explicit LE reads
//! and writes so the layout is crystal-clear and testable.

use anyhow::{bail, Result};

// ── Magic / type tags ────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"BTDX";
pub const TAG_GNRL: &[u8; 4] = b"GNRL";
pub const TAG_DX10: &[u8; 4] = b"DX10";

// ── Size constants ───────────────────────────────────────────────────────────

pub const HEADER_SIZE: usize = 24;
pub const RECORD_SIZE: usize = 36;

// ── Field constants (ground-truthed against SeventySix - Localization.ba2) ──

/// Every GNRL file record carries this flags value.
pub const RECORD_FLAGS: u32 = 0x0010_0100;
/// Sentinel padding at the end of every GNRL record.
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
    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let mut archive_type = [0u8; 4];
    archive_type.copy_from_slice(&data[8..12]);
    let file_count = u32::from_le_bytes(data[12..16].try_into().unwrap());
    let name_table_offset = u64::from_le_bytes(data[16..24].try_into().unwrap());
    Ok(Header {
        version,
        archive_type,
        file_count,
        name_table_offset,
    })
}

/// Serialize a header to exactly 24 bytes.
pub fn write_header(version: u32, file_count: u32, name_table_offset: u64) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];
    buf[0..4].copy_from_slice(MAGIC);
    buf[4..8].copy_from_slice(&version.to_le_bytes());
    buf[8..12].copy_from_slice(TAG_GNRL);
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
    let name_hash = u32::from_le_bytes(data[base..base + 4].try_into().unwrap());
    let mut ext = [0u8; 4];
    ext.copy_from_slice(&data[base + 4..base + 8]);
    let dir_hash = u32::from_le_bytes(data[base + 8..base + 12].try_into().unwrap());
    let flags = u32::from_le_bytes(data[base + 12..base + 16].try_into().unwrap());
    let data_offset = u64::from_le_bytes(data[base + 16..base + 24].try_into().unwrap());
    let packed_size = u32::from_le_bytes(data[base + 24..base + 28].try_into().unwrap());
    let unpacked_size = u32::from_le_bytes(data[base + 28..base + 32].try_into().unwrap());
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let bytes = write_header(1, 42, 0xDEAD_BEEF_1234_5678);
        assert_eq!(&bytes[0..4], MAGIC);
        assert_eq!(&bytes[8..12], TAG_GNRL);
        let hdr = read_header(&bytes).unwrap();
        assert_eq!(hdr.version, 1);
        assert_eq!(hdr.file_count, 42);
        assert_eq!(hdr.name_table_offset, 0xDEAD_BEEF_1234_5678);
        assert_eq!(&hdr.archive_type, TAG_GNRL);
    }

    #[test]
    fn record_round_trip() {
        let r = Record {
            name_hash: 0x1234_5678,
            ext: *b"txt\0",
            dir_hash: 0xABCD_EF01,
            flags: RECORD_FLAGS,
            data_offset: 0x0000_0100_0000_0000,
            packed_size: 0,
            unpacked_size: 1024,
        };
        let bytes = write_record(&r);
        // flags at offset 12
        assert_eq!(
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            RECORD_FLAGS
        );
        // padding at offset 32
        assert_eq!(
            u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
            PADDING
        );
        let r2 = read_record(&bytes, 0);
        assert_eq!(r2.name_hash, r.name_hash);
        assert_eq!(r2.ext, r.ext);
        assert_eq!(r2.dir_hash, r.dir_hash);
        assert_eq!(r2.flags, RECORD_FLAGS);
        assert_eq!(r2.data_offset, r.data_offset);
        assert_eq!(r2.packed_size, r.packed_size);
        assert_eq!(r2.unpacked_size, r.unpacked_size);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = write_header(1, 0, 24);
        bytes[0] = b'X';
        assert!(read_header(&bytes).is_err());
    }

    #[test]
    fn too_small_rejected() {
        assert!(read_header(&[0u8; 10]).is_err());
    }
}
