//! Integration tests for `ba2::format` — binary (de)serialization.
//!
//! These tests exercise only the public API so they live here rather than
//! inline.  They pin exact byte offsets and field values; any regression in
//! the on-disk layout will show up immediately.

use ba2::format::{
    ArchiveKind, HEADER_SIZE, MAGIC, PADDING, RECORD_FLAGS, RECORD_SIZE, Record, TAG_DX10,
    TAG_GNRL, TEX_CHUNK_HEADER_SIZE, TEX_CHUNK_SIZE, TEX_FLAG_CUBEMAP, TEX_RECORD_SIZE, TexChunk,
    TexRecord, VERSION, read_header, read_record, read_tex_chunk, read_tex_record, write_header,
    write_record, write_tex_chunk, write_tex_record,
};

// ── Header ───────────────────────────────────────────────────────────────────

#[test]
fn header_round_trip() {
    let bytes = write_header(1, ArchiveKind::Gnrl, 42, 0xDEAD_BEEF_1234_5678);
    assert_eq!(&bytes[0..4], MAGIC);
    assert_eq!(&bytes[8..12], TAG_GNRL);
    let hdr = read_header(&bytes).unwrap();
    assert_eq!(hdr.version, 1);
    assert_eq!(hdr.file_count, 42);
    assert_eq!(hdr.name_table_offset, 0xDEAD_BEEF_1234_5678);
    assert_eq!(&hdr.archive_type, TAG_GNRL);
}

#[test]
fn dx10_header_round_trip() {
    let bytes = write_header(1, ArchiveKind::Dx10, 7, 0x1234);
    assert_eq!(&bytes[8..12], TAG_DX10);
    let hdr = read_header(&bytes).unwrap();
    assert_eq!(&hdr.archive_type, TAG_DX10);
    assert_eq!(
        ArchiveKind::from_tag(&hdr.archive_type).unwrap(),
        ArchiveKind::Dx10
    );
}

#[test]
fn archive_kind_from_tag_rejects_unknown() {
    assert!(ArchiveKind::from_tag(b"XXXX").is_err());
}

#[test]
fn bad_magic_rejected() {
    let mut bytes = write_header(1, ArchiveKind::Gnrl, 0, HEADER_SIZE as u64);
    bytes[0] = b'X';
    assert!(
        read_header(&bytes).is_err(),
        "corrupted magic must be rejected"
    );
}

#[test]
fn too_small_rejected() {
    assert!(
        read_header(&[0u8; 10]).is_err(),
        "slice shorter than HEADER_SIZE must be rejected"
    );
}

/// Pin the exact byte position of every field in the 24-byte header.
#[test]
fn write_header_byte_layout() {
    let bytes = write_header(VERSION, ArchiveKind::Gnrl, 99, 0x0102_0304_0506_0708);
    // [0..4]  magic
    assert_eq!(&bytes[0..4], b"BTDX", "magic at [0..4]");
    // [4..8]  version (LE u32)
    assert_eq!(&bytes[4..8], &VERSION.to_le_bytes(), "version at [4..8]");
    // [8..12] archive type
    assert_eq!(&bytes[8..12], b"GNRL", "archive_type at [8..12]");
    // [12..16] file count (LE u32)
    assert_eq!(
        &bytes[12..16],
        &99u32.to_le_bytes(),
        "file_count at [12..16]"
    );
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
    assert_eq!(
        &bytes[0..4],
        &0x1234_5678u32.to_le_bytes(),
        "name_hash at [0..4]"
    );
    // [4..8]   ext ([u8;4])
    assert_eq!(&bytes[4..8], b"bin\0", "ext at [4..8]");
    // [8..12]  dir_hash (LE u32)
    assert_eq!(
        &bytes[8..12],
        &0xABCD_EF01u32.to_le_bytes(),
        "dir_hash at [8..12]"
    );
    // [12..16] flags (LE u32)
    assert_eq!(
        &bytes[12..16],
        &RECORD_FLAGS.to_le_bytes(),
        "flags at [12..16]"
    );
    // [16..24] data_offset (LE u64)
    assert_eq!(
        &bytes[16..24],
        &0x0000_0100_0000_0000u64.to_le_bytes(),
        "data_offset at [16..24]"
    );
    // [24..28] packed_size (LE u32)
    assert_eq!(
        &bytes[24..28],
        &256u32.to_le_bytes(),
        "packed_size at [24..28]"
    );
    // [28..32] unpacked_size (LE u32)
    assert_eq!(
        &bytes[28..32],
        &1024u32.to_le_bytes(),
        "unpacked_size at [28..32]"
    );
    // [32..36] padding sentinel
    assert_eq!(
        &bytes[32..36],
        &PADDING.to_le_bytes(),
        "padding at [32..36]"
    );
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
    assert_eq!(
        r2.name_hash, r.name_hash,
        "name_hash must be read from base+0"
    );
    assert_eq!(r2.dir_hash, r.dir_hash, "dir_hash must be read from base+8");
    assert_eq!(
        r2.data_offset, r.data_offset,
        "data_offset must be read from base+16"
    );
    assert_eq!(
        r2.unpacked_size, r.unpacked_size,
        "unpacked_size must be read from base+28"
    );
}

// ── DX10 texture record ─────────────────────────────────────────────────────

/// Pin the exact byte position of every field in the 24-byte DX10 texture header.
#[test]
fn write_tex_record_byte_layout() {
    let r = TexRecord {
        name_hash: 0x1234_5678,
        ext: *b"dds\0",
        dir_hash: 0xABCD_EF01,
        chunk_count: 3,
        height: 1024,
        width: 2048,
        mip_count: 12,
        dxgi_format: 84, // BC5_SNORM
        cubemap: true,
        tile_mode: 0x08,
    };
    let bytes = write_tex_record(&r);
    assert_eq!(bytes.len(), TEX_RECORD_SIZE);
    assert_eq!(
        &bytes[0..4],
        &0x1234_5678u32.to_le_bytes(),
        "name_hash at [0..4]"
    );
    assert_eq!(&bytes[4..8], b"dds\0", "ext at [4..8]");
    assert_eq!(
        &bytes[8..12],
        &0xABCD_EF01u32.to_le_bytes(),
        "dir_hash at [8..12]"
    );
    assert_eq!(bytes[12], 0, "unk8 at [12] is always 0");
    assert_eq!(bytes[13], 3, "chunk_count at [13]");
    assert_eq!(
        &bytes[14..16],
        &TEX_CHUNK_HEADER_SIZE.to_le_bytes(),
        "chunk_header_size at [14..16]"
    );
    assert_eq!(&bytes[16..18], &1024u16.to_le_bytes(), "height at [16..18]");
    assert_eq!(&bytes[18..20], &2048u16.to_le_bytes(), "width at [18..20]");
    assert_eq!(bytes[20], 12, "mip_count at [20]");
    assert_eq!(bytes[21], 84, "dxgi_format at [21]");
    assert_eq!(bytes[22], TEX_FLAG_CUBEMAP, "cubemap byte at [22]");
    assert_eq!(bytes[23], 0x08, "tile_mode at [23]");

    let r2 = read_tex_record(&bytes, 0).unwrap();
    assert_eq!(r2.name_hash, r.name_hash);
    assert_eq!(r2.ext, r.ext);
    assert_eq!(r2.dir_hash, r.dir_hash);
    assert_eq!(r2.chunk_count, r.chunk_count);
    assert_eq!(r2.height, r.height);
    assert_eq!(r2.width, r.width);
    assert_eq!(r2.mip_count, r.mip_count);
    assert_eq!(r2.dxgi_format, r.dxgi_format);
    assert_eq!(r2.cubemap, r.cubemap);
    assert_eq!(r2.tile_mode, r.tile_mode);
}

#[test]
fn read_tex_record_rejects_bad_chunk_header_size() {
    let r = TexRecord {
        name_hash: 0,
        ext: *b"dds\0",
        dir_hash: 0,
        chunk_count: 1,
        height: 4,
        width: 4,
        mip_count: 1,
        dxgi_format: 71,
        cubemap: false,
        tile_mode: 0x08,
    };
    let mut bytes = write_tex_record(&r);
    bytes[14..16].copy_from_slice(&99u16.to_le_bytes());
    assert!(
        read_tex_record(&bytes, 0).is_err(),
        "unexpected chunk_header_size must be rejected"
    );
}

#[test]
fn read_tex_record_at_nonzero_base() {
    let prefix = [0xFFu8; 8];
    let r = TexRecord {
        name_hash: 0xDEAD_BEEF,
        ext: *b"dds\0",
        dir_hash: 0xCAFE_BABE,
        chunk_count: 2,
        height: 512,
        width: 512,
        mip_count: 10,
        dxgi_format: 71,
        cubemap: false,
        tile_mode: 0x08,
    };
    let mut buf = prefix.to_vec();
    buf.extend_from_slice(&write_tex_record(&r));
    let r2 = read_tex_record(&buf, 8).unwrap();
    assert_eq!(r2.name_hash, r.name_hash);
    assert_eq!(r2.width, r.width);
    assert_eq!(r2.height, r.height);
}

// ── DX10 chunk record ────────────────────────────────────────────────────────

/// Pin the exact byte position of every field in the 24-byte DX10 chunk record.
#[test]
fn write_tex_chunk_byte_layout() {
    let c = TexChunk {
        data_offset: 0x0000_0100_0000_0000,
        packed_size: 12345,
        unpacked_size: 65536,
        mip_first: 2,
        mip_last: 10,
    };
    let bytes = write_tex_chunk(&c);
    assert_eq!(bytes.len(), TEX_CHUNK_SIZE);
    assert_eq!(
        &bytes[0..8],
        &0x0000_0100_0000_0000u64.to_le_bytes(),
        "data_offset at [0..8]"
    );
    assert_eq!(
        &bytes[8..12],
        &12345u32.to_le_bytes(),
        "packed_size at [8..12]"
    );
    assert_eq!(
        &bytes[12..16],
        &65536u32.to_le_bytes(),
        "unpacked_size at [12..16]"
    );
    assert_eq!(&bytes[16..18], &2u16.to_le_bytes(), "mip_first at [16..18]");
    assert_eq!(&bytes[18..20], &10u16.to_le_bytes(), "mip_last at [18..20]");
    assert_eq!(
        &bytes[20..24],
        &PADDING.to_le_bytes(),
        "sentinel at [20..24]"
    );

    let c2 = read_tex_chunk(&bytes, 0);
    assert_eq!(c2.data_offset, c.data_offset);
    assert_eq!(c2.packed_size, c.packed_size);
    assert_eq!(c2.unpacked_size, c.unpacked_size);
    assert_eq!(c2.mip_first, c.mip_first);
    assert_eq!(c2.mip_last, c.mip_last);
}
