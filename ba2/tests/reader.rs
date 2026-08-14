//! Integration tests for `ba2::reader` — archive opening, entry reading, and
//! error-path coverage.

mod common;

use ba2::compress::{Codec, MAX_DECOMP_SIZE};
use ba2::format::{RECORD_FLAGS, Record, write_header, write_record};
use ba2::hash::hash_path;
use ba2::reader::Ba2Archive;
use ba2::{ArchiveKind, WriteOptions, write_ba2};
use common::TestTexture;
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

// ── Happy-path reads ──────────────────────────────────────────────────────

#[test]
fn open_and_read_stored_entries() {
    let entries: &[(&str, &[u8])] = &[
        ("interface/test.txt", b"hello world"),
        ("data/config.bin", b"\x00\x01\x02\x03"),
    ];
    let tmp = common::make_test_archive(entries);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    assert_eq!(archive.list().len(), 2);

    let txt = archive.read("interface/test.txt", Codec::Auto).unwrap();
    assert_eq!(txt, b"hello world");

    // `read` is case-insensitive.
    let bin = archive.read("DATA/CONFIG.BIN", Codec::Auto).unwrap();
    assert_eq!(bin, b"\x00\x01\x02\x03");
}

#[test]
fn missing_entry_returns_error() {
    let entries: &[(&str, &[u8])] = &[("foo/bar.txt", b"data")];
    let tmp = common::make_test_archive(entries);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    assert!(archive.read("foo/missing.txt", Codec::Auto).is_err());
}

/// An empty archive (0 files) can be opened and produces an empty entry list.
#[test]
fn open_empty_archive() {
    let tmp = common::make_test_archive(&[]);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    assert_eq!(archive.list().len(), 0);
}

// ── Compressed-entry reads ────────────────────────────────────────────────

#[test]
fn read_lz4_compressed_entry() {
    // Use write_ba2 to produce a real LZ4-compressed archive.
    let data: Vec<u8> = b"FO76 compressed blob! ".repeat(50).to_vec();

    let src_dir = TempDir::new().unwrap();
    let src_file = src_dir.path().join("payload.bin");
    std::fs::write(&src_file, &data).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("data/payload.bin".to_string(), src_file)];
    let opts = WriteOptions {
        codec: Codec::Lz4,
        ..Default::default()
    };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let entry = &archive.list()[0];
    assert!(entry.is_compressed(), "entry should be LZ4-compressed");

    let out_data = archive.read("data/payload.bin", Codec::Auto).unwrap();
    assert_eq!(out_data, data);
}

#[test]
fn read_zlib_compressed_entry() {
    let data: Vec<u8> = b"FO4 zlib payload xxxxxxxx ".repeat(50).to_vec();

    let src_dir = TempDir::new().unwrap();
    let src_file = src_dir.path().join("payload.bin");
    std::fs::write(&src_file, &data).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("data/payload.bin".to_string(), src_file)];
    let opts = WriteOptions {
        codec: Codec::Zlib,
        ..Default::default()
    };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let entry = &archive.list()[0];
    assert!(entry.is_compressed(), "entry should be zlib-compressed");

    let out_data = archive.read("data/payload.bin", Codec::Auto).unwrap();
    assert_eq!(out_data, data);
}

// ── Decompression-bomb cap (real read path) ───────────────────────────────

/// A crafted GNRL record with a tiny compressed blob but a declared
/// `unpacked_size` past `MAX_DECOMP_SIZE` — the shape a corrupt or malicious
/// archive would use to trigger an unbounded allocation on decompress.
/// `Ba2Archive::read` must error instead of attempting the allocation.
#[test]
fn read_rejects_oversized_declared_unpacked_size() {
    let path = "data/bomb.bin";
    let (name_hash, dir_hash, ext) = hash_path(path);
    // Small "compressed" payload — irrelevant, since the size cap must fire
    // before any decompressor call is made.
    let payload: &[u8] = b"tiny";

    let data_start = 24u64 + 36; // header + one record
    let name_table_offset = data_start + payload.len() as u64;

    let mut buf = Vec::new();
    buf.extend_from_slice(&write_header(1, ArchiveKind::Gnrl, 1, name_table_offset));
    let record = Record {
        name_hash,
        ext,
        dir_hash,
        flags: RECORD_FLAGS,
        data_offset: data_start,
        packed_size: payload.len() as u32, // nonzero => compressed, decompress path taken
        unpacked_size: (MAX_DECOMP_SIZE + 1) as u32, // crafted oversized declared size
    };
    buf.extend_from_slice(&write_record(&record));
    buf.extend_from_slice(payload);
    let name = path.to_lowercase();
    buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
    buf.extend_from_slice(name.as_bytes());

    let tmp = NamedTempFile::new().unwrap();
    {
        let mut f = tmp.reopen().unwrap();
        f.write_all(&buf).unwrap();
    }

    let archive = Ba2Archive::open(tmp.path()).unwrap();
    let result = archive.read(path, Codec::Lz4);
    let err = result.expect_err(
        "expected the decompression-bomb cap to reject a crafted oversized unpacked_size",
    );
    // `read`'s top-level error wraps the cap error with `.with_context(...)` —
    // check the full cause chain, not just the outermost Display.
    assert!(
        err.chain().any(|c| c.to_string().contains("exceeds limit")),
        "expected 'exceeds limit' somewhere in the error chain, got: {err:?}"
    );
}

// ── Error branches on open ────────────────────────────────────────────────

/// Build the 24-byte raw header bytes for an otherwise valid archive header.
fn make_raw_header(
    version: u32,
    archive_type: &[u8; 4],
    file_count: u32,
    nt_offset: u64,
) -> Vec<u8> {
    let mut v = Vec::with_capacity(24);
    v.extend_from_slice(b"BTDX");
    v.extend_from_slice(&version.to_le_bytes());
    v.extend_from_slice(archive_type);
    v.extend_from_slice(&file_count.to_le_bytes());
    v.extend_from_slice(&nt_offset.to_le_bytes());
    v
}

fn write_tmp(data: &[u8]) -> NamedTempFile {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(data).unwrap();
    tmp.flush().unwrap();
    tmp
}

#[test]
fn opens_dx10() {
    // Empty DX10 archives open cleanly, just like empty GNRL ones.
    let header = make_raw_header(1, b"DX10", 0, 24);
    let tmp = write_tmp(&header);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    assert_eq!(archive.kind(), ArchiveKind::Dx10);
    assert_eq!(archive.list().len(), 0);
}

#[test]
fn rejects_bad_version() {
    let header = make_raw_header(2, b"GNRL", 0, 24);
    let tmp = write_tmp(&header);
    let err = Ba2Archive::open(tmp.path());
    assert!(err.is_err(), "unsupported version must be rejected");
}

#[test]
fn rejects_non_gnrl_type() {
    let header = make_raw_header(1, b"XXXX", 0, 24);
    let tmp = write_tmp(&header);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "unknown archive type must be rejected"
    );
}

#[test]
fn rejects_records_past_eof() {
    // Claim 100 file records but provide only the header (24 bytes).
    // records_end = 24 + 100*36 = 3624 > 24 → bail.
    let header = make_raw_header(1, b"GNRL", 100, 99_999);
    let tmp = write_tmp(&header);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "records extending past EOF must be rejected"
    );
}

#[test]
fn rejects_nametable_offset_out_of_range() {
    // 0 records, but name_table_offset is absurdly large.
    let mut data = make_raw_header(1, b"GNRL", 0, 999_999);
    data.extend_from_slice(&[0u8; 10]); // some extra bytes, but not 999999 of them
    let tmp = write_tmp(&data);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "out-of-range name table offset must be rejected"
    );
}

#[test]
fn rejects_truncated_name_length_prefix() {
    // 1 record, name table starts at the right offset, but only 1 byte follows
    // (the length prefix needs 2 bytes).
    let entry_data = b"x";
    let data_start = 24u64 + 36; // header + 1 record
    let nt_offset = data_start + 1; // 1 byte of entry data

    let mut buf = make_raw_header(1, b"GNRL", 1, nt_offset);

    // Record: minimal fields; only packed_size=0 and unpacked_size=1 matter here.
    use ba2::format::{RECORD_FLAGS, Record, write_record};
    use ba2::hash::hash_path;
    let (name_hash, dir_hash, ext) = hash_path("a.txt");
    let r = Record {
        name_hash,
        ext,
        dir_hash,
        flags: RECORD_FLAGS,
        data_offset: data_start,
        packed_size: 0,
        unpacked_size: 1,
    };
    buf.extend_from_slice(&write_record(&r));
    buf.extend_from_slice(entry_data); // 1 byte of data
    buf.push(0xAB); // only 1 byte for the name-table length prefix (needs 2)

    let tmp = write_tmp(&buf);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "truncated name-length prefix must be rejected"
    );
}

#[test]
fn rejects_truncated_name_bytes() {
    // Name-table length prefix claims 100 chars but there are 0 name bytes.
    let entry_data = b"x";
    let data_start = 24u64 + 36;
    let nt_offset = data_start + 1;

    let mut buf = make_raw_header(1, b"GNRL", 1, nt_offset);

    use ba2::format::{RECORD_FLAGS, Record, write_record};
    use ba2::hash::hash_path;
    let (name_hash, dir_hash, ext) = hash_path("a.txt");
    let r = Record {
        name_hash,
        ext,
        dir_hash,
        flags: RECORD_FLAGS,
        data_offset: data_start,
        packed_size: 0,
        unpacked_size: 1,
    };
    buf.extend_from_slice(&write_record(&r));
    buf.extend_from_slice(entry_data);
    buf.extend_from_slice(&100u16.to_le_bytes()); // claims 100-char name
    // …but writes 0 name bytes

    let tmp = write_tmp(&buf);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "truncated name string must be rejected"
    );
}

// ── read() error branch ───────────────────────────────────────────────────

#[test]
fn read_data_out_of_range() {
    // Build an archive where unpacked_size is enormous so the data extent
    // exceeds the file size.  open() must succeed (it doesn't validate data
    // offsets), but read() must fail.
    let entry_data = b"tiny";
    let data_start = 24u64 + 36;
    let nt_offset = data_start + entry_data.len() as u64;

    let mut buf = make_raw_header(1, b"GNRL", 1, nt_offset);

    use ba2::format::{RECORD_FLAGS, Record, write_record};
    use ba2::hash::hash_path;
    let (name_hash, dir_hash, ext) = hash_path("data/x.bin");
    let r = Record {
        name_hash,
        ext,
        dir_hash,
        flags: RECORD_FLAGS,
        data_offset: data_start,
        packed_size: 0,
        unpacked_size: u32::MAX, // claims 4 GiB, but actual data is 4 bytes
    };
    buf.extend_from_slice(&write_record(&r));
    buf.extend_from_slice(entry_data);
    let name = "data\\x.bin";
    buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
    buf.extend_from_slice(name.as_bytes());

    let tmp = write_tmp(&buf);
    let archive = Ba2Archive::open(tmp.path()).unwrap(); // open should succeed
    assert!(
        archive.read("data/x.bin", Codec::Auto).is_err(),
        "read() must fail when data extent exceeds file size"
    );
}

// ── DX10 ─────────────────────────────────────────────────────────────────────

#[test]
fn dx10_multi_chunk_reassembly_order() {
    let tex = TestTexture {
        path: "textures/multi.dds",
        dxgi_format: 77, // BC3_UNORM
        width: 8,
        height: 8,
        mip_count: 2,
        cubemap: false,
        chunks: &[(0, 0, &[0xAAu8; 16]), (1, 1, &[0xBBu8; 4])],
    };
    let tmp = common::make_test_texture_archive(&[tex]);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    assert_eq!(archive.kind(), ArchiveKind::Dx10);

    let data = archive.read("textures/multi.dds", Codec::Auto).unwrap();
    let mut expected = ba2::dds::synth_header(77, 8, 8, 2, false).unwrap();
    expected.extend_from_slice(&[0xAAu8; 16]);
    expected.extend_from_slice(&[0xBBu8; 4]);
    assert_eq!(
        data, expected,
        "chunks must be decompressed and concatenated in on-disk order, after the synthesized header"
    );
}

#[test]
fn dx10_stored_chunk_reads_raw() {
    // TestTexture chunks are always stored (packed_size == 0) — verify that
    // path explicitly, since it's the common case in this test file.
    let tex = TestTexture {
        path: "textures/stored.dds",
        dxgi_format: 71,
        width: 4,
        height: 4,
        mip_count: 1,
        cubemap: false,
        chunks: &[(0, 0, &[0x11u8; 8])],
    };
    let tmp = common::make_test_texture_archive(&[tex]);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    assert!(!archive.list()[0].is_compressed());
    let data = archive.read("textures/stored.dds", Codec::Auto).unwrap();
    assert!(data.ends_with(&[0x11u8; 8]));
}

#[test]
fn dx10_cubemap_flag_decoded() {
    let tex = TestTexture {
        path: "textures/shared/cubemaps/test.dds",
        dxgi_format: 71,
        width: 4,
        height: 4,
        mip_count: 1,
        cubemap: true,
        chunks: &[(0, 0, &[0x01u8; 8])],
    };
    let tmp = common::make_test_texture_archive(&[tex]);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    let t = archive.list()[0].texture().unwrap();
    assert!(t.cubemap);

    let data = archive
        .read("textures/shared/cubemaps/test.dds", Codec::Auto)
        .unwrap();
    let caps2 = u32::from_le_bytes(data[112..116].try_into().unwrap());
    assert_eq!(
        caps2, 0x0000_FE00,
        "DDSCAPS2_CUBEMAP | DDSCAPS2_CUBEMAP_ALLFACES must be set"
    );
}

#[test]
fn dx10_unknown_dxgi_format_errors_at_read_not_open() {
    let tex = TestTexture {
        path: "textures/unknown.dds",
        dxgi_format: 255, // not one of the 15 formats FO76 ships
        width: 4,
        height: 4,
        mip_count: 1,
        cubemap: false,
        chunks: &[(0, 0, &[0u8; 8])],
    };
    let tmp = common::make_test_texture_archive(&[tex]);
    let archive = Ba2Archive::open(tmp.path()).unwrap(); // open doesn't validate dxgi_format
    assert!(archive.read("textures/unknown.dds", Codec::Auto).is_err());
}

#[test]
fn dx10_chunk_decompressed_length_mismatch_errors() {
    use ba2::compress::compress_zlib;
    use ba2::format::{
        TEX_CHUNK_SIZE, TEX_RECORD_SIZE, TexChunk, TexRecord, write_tex_chunk, write_tex_record,
    };
    use ba2::hash::hash_path;

    let payload = b"zlib payload for a length-mismatch test";
    let compressed = compress_zlib(payload).unwrap();
    let (name_hash, dir_hash, ext) = hash_path("textures/bad.dds");

    let data_start = 24u64 + TEX_RECORD_SIZE as u64 + TEX_CHUNK_SIZE as u64;
    let nt_offset = data_start + compressed.len() as u64;

    let mut buf = make_raw_header(1, b"DX10", 1, nt_offset);
    let tex = TexRecord {
        name_hash,
        ext,
        dir_hash,
        chunk_count: 1,
        height: 4,
        width: 4,
        mip_count: 1,
        dxgi_format: 71,
        cubemap: false,
        tile_mode: 0x08,
    };
    buf.extend_from_slice(&write_tex_record(&tex));
    let chunk = TexChunk {
        data_offset: data_start,
        packed_size: compressed.len() as u32,
        // Deliberately wrong: the payload decompresses to `payload.len()`.
        unpacked_size: payload.len() as u32 + 5,
        mip_first: 0,
        mip_last: 0,
    };
    buf.extend_from_slice(&write_tex_chunk(&chunk));
    buf.extend_from_slice(&compressed);
    let name = "textures\\bad.dds";
    buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
    buf.extend_from_slice(name.as_bytes());

    let tmp = write_tmp(&buf);
    let archive = Ba2Archive::open(tmp.path()).unwrap();
    assert!(
        archive.read("textures/bad.dds", Codec::Auto).is_err(),
        "a chunk decompressing to a different length than its unpacked_size must error"
    );
}

#[test]
fn dx10_rejects_chunk_header_size_mismatch() {
    use ba2::format::{TexRecord, write_tex_record};

    let tex = TexRecord {
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
    let mut tex_bytes = write_tex_record(&tex);
    tex_bytes[14..16].copy_from_slice(&99u16.to_le_bytes()); // should always be 24

    let mut buf = make_raw_header(1, b"DX10", 1, 48);
    buf.extend_from_slice(&tex_bytes);

    let tmp = write_tmp(&buf);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "unexpected chunk_header_size must be rejected"
    );
}

#[test]
fn dx10_rejects_truncated_chunk_area() {
    use ba2::format::{TexRecord, write_tex_record};

    // Claims 5 chunks but the file ends right after the texture header.
    let tex = TexRecord {
        name_hash: 0,
        ext: *b"dds\0",
        dir_hash: 0,
        chunk_count: 5,
        height: 4,
        width: 4,
        mip_count: 1,
        dxgi_format: 71,
        cubemap: false,
        tile_mode: 0x08,
    };
    let mut buf = make_raw_header(1, b"DX10", 1, 48);
    buf.extend_from_slice(&write_tex_record(&tex));

    let tmp = write_tmp(&buf);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "texture records extending past EOF must be rejected"
    );
}

#[test]
fn dx10_rejects_records_extending_past_name_table() {
    use ba2::format::{TexChunk, TexRecord, write_tex_chunk, write_tex_record};

    let tex = TexRecord {
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
    let chunk = TexChunk {
        data_offset: 72,
        packed_size: 0,
        unpacked_size: 8,
        mip_first: 0,
        mip_last: 0,
    };
    // The full record area (header + tex record + chunk) ends at byte 72,
    // but the name table is declared to start at byte 30 — inside it.
    let mut buf = make_raw_header(1, b"DX10", 1, 30);
    buf.extend_from_slice(&write_tex_record(&tex));
    buf.extend_from_slice(&write_tex_chunk(&chunk));

    let tmp = write_tmp(&buf);
    assert!(
        Ba2Archive::open(tmp.path()).is_err(),
        "texture records extending past the declared name table start must be rejected"
    );
}
