//! Integration tests for `ba2::format` — binary (de)serialization.
//!
//! These tests exercise only the public API so they live here rather than
//! inline.  They pin exact byte offsets and field values; any regression in
//! the on-disk layout will show up immediately.

use ba2::format::{
    read_header, read_record, write_header, write_record, Record, HEADER_SIZE, MAGIC, PADDING,
    RECORD_FLAGS, RECORD_SIZE, TAG_GNRL, VERSION,
};

// ── Header ───────────────────────────────────────────────────────────────────

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
fn bad_magic_rejected() {
    let mut bytes = write_header(1, 0, HEADER_SIZE as u64);
    bytes[0] = b'X';
    assert!(read_header(&bytes).is_err(), "corrupted magic must be rejected");
}

#[test]
fn too_small_rejected() {
    assert!(read_header(&[0u8; 10]).is_err(), "slice shorter than HEADER_SIZE must be rejected");
}

/// Pin the exact byte position of every field in the 24-byte header.
#[test]
fn write_header_byte_layout() {
    let bytes = write_header(VERSION, 99, 0x0102_0304_0506_0708);
    // [0..4]  magic
    assert_eq!(&bytes[0..4], b"BTDX", "magic at [0..4]");
    // [4..8]  version (LE u32)
    assert_eq!(&bytes[4..8], &VERSION.to_le_bytes(), "version at [4..8]");
    // [8..12] archive type
    assert_eq!(&bytes[8..12], b"GNRL", "archive_type at [8..12]");
    // [12..16] file count (LE u32)
    assert_eq!(&bytes[12..16], &99u32.to_le_bytes(), "file_count at [12..16]");
    // [16..24] name_table_offset (LE u64)
    assert_eq!(
        &bytes[16..24],
        &0x0102_0304_0506_0708u64.to_le_bytes(),
        "name_table_offset at [16..24]"
    );
    assert_eq!(bytes.len(), HEADER_SIZE);
}

// ── Record ───────────────────────────────────────────────────────────────────

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
    assert_eq!(
        u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        RECORD_FLAGS,
        "flags at [12..16]"
    );
    assert_eq!(
        u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
        PADDING,
        "padding at [32..36]"
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

/// Pin the exact byte position of EVERY field in the 36-byte record.
#[test]
fn write_record_byte_layout() {
    let r = Record {
        name_hash: 0x1234_5678,
        ext: *b"bin\0",
        dir_hash: 0xABCD_EF01,
        flags: RECORD_FLAGS,
        data_offset: 0x0000_0100_0000_0000,
        packed_size: 256,
        unpacked_size: 1024,
    };
    let bytes = write_record(&r);
    assert_eq!(bytes.len(), RECORD_SIZE);
    // [0..4]   name_hash (LE u32)
    assert_eq!(&bytes[0..4], &0x1234_5678u32.to_le_bytes(), "name_hash at [0..4]");
    // [4..8]   ext ([u8;4])
    assert_eq!(&bytes[4..8], b"bin\0", "ext at [4..8]");
    // [8..12]  dir_hash (LE u32)
    assert_eq!(&bytes[8..12], &0xABCD_EF01u32.to_le_bytes(), "dir_hash at [8..12]");
    // [12..16] flags (LE u32)
    assert_eq!(&bytes[12..16], &RECORD_FLAGS.to_le_bytes(), "flags at [12..16]");
    // [16..24] data_offset (LE u64)
    assert_eq!(
        &bytes[16..24],
        &0x0000_0100_0000_0000u64.to_le_bytes(),
        "data_offset at [16..24]"
    );
    // [24..28] packed_size (LE u32)
    assert_eq!(&bytes[24..28], &256u32.to_le_bytes(), "packed_size at [24..28]");
    // [28..32] unpacked_size (LE u32)
    assert_eq!(&bytes[28..32], &1024u32.to_le_bytes(), "unpacked_size at [28..32]");
    // [32..36] padding sentinel
    assert_eq!(&bytes[32..36], &PADDING.to_le_bytes(), "padding at [32..36]");
}

/// Verify that `read_record(data, base)` reads from `base`, not from 0.
#[test]
fn read_record_at_nonzero_base() {
    // 8 garbage bytes followed by a valid record.
    let prefix = [0xFFu8; 8];
    let r = Record {
        name_hash: 0xDEAD_BEEF,
        ext: *b"bin\0",
        dir_hash: 0xCAFE_BABE,
        flags: RECORD_FLAGS,
        data_offset: 0x0000_0001_0000_0000,
        packed_size: 0,
        unpacked_size: 512,
    };
    let mut buf = prefix.to_vec();
    buf.extend_from_slice(&write_record(&r));

    let r2 = read_record(&buf, 8); // base offset = length of prefix
    assert_eq!(r2.name_hash, r.name_hash, "name_hash must be read from base+0");
    assert_eq!(r2.dir_hash, r.dir_hash, "dir_hash must be read from base+8");
    assert_eq!(r2.data_offset, r.data_offset, "data_offset must be read from base+16");
    assert_eq!(r2.unpacked_size, r.unpacked_size, "unpacked_size must be read from base+28");
}
