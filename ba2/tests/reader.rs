//! Integration tests for `ba2::reader` — archive opening, entry reading, and
//! error-path coverage.

mod common;

use ba2::compress::Codec;
use ba2::reader::Ba2Archive;
use ba2::{write_ba2, WriteOptions};
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
    let opts = WriteOptions { codec: Codec::Lz4, ..Default::default() };
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
    let opts = WriteOptions { codec: Codec::Zlib, ..Default::default() };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let entry = &archive.list()[0];
    assert!(entry.is_compressed(), "entry should be zlib-compressed");

    let out_data = archive.read("data/payload.bin", Codec::Auto).unwrap();
    assert_eq!(out_data, data);
}

// ── Error branches on open ────────────────────────────────────────────────

/// Build the 24-byte raw header bytes for an otherwise valid archive header.
fn make_raw_header(version: u32, archive_type: &[u8; 4], file_count: u32, nt_offset: u64) -> Vec<u8> {
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
fn rejects_dx10() {
    // DX10 (texture) archives are not supported.
    let header = make_raw_header(1, b"DX10", 0, 24);
    let tmp = write_tmp(&header);
    let result = Ba2Archive::open(tmp.path());
    assert!(result.is_err(), "DX10 archive must be rejected");
    // Verify the error message mentions DX10 or texture so it's diagnosable.
    let msg = result.err().unwrap().to_string();
    assert!(msg.contains("DX10") || msg.contains("texture"), "error should mention DX10: {}", msg);
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
    assert!(Ba2Archive::open(tmp.path()).is_err(), "unknown archive type must be rejected");
}

#[test]
fn rejects_records_past_eof() {
    // Claim 100 file records but provide only the header (24 bytes).
    // records_end = 24 + 100*36 = 3624 > 24 → bail.
    let header = make_raw_header(1, b"GNRL", 100, 99_999);
    let tmp = write_tmp(&header);
    assert!(Ba2Archive::open(tmp.path()).is_err(), "records extending past EOF must be rejected");
}

#[test]
fn rejects_nametable_offset_out_of_range() {
    // 0 records, but name_table_offset is absurdly large.
    let mut data = make_raw_header(1, b"GNRL", 0, 999_999);
    data.extend_from_slice(&[0u8; 10]); // some extra bytes, but not 999999 of them
    let tmp = write_tmp(&data);
    assert!(Ba2Archive::open(tmp.path()).is_err(), "out-of-range name table offset must be rejected");
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
    use ba2::format::{write_record, Record, RECORD_FLAGS};
    use ba2::hash::hash_path;
    let (name_hash, dir_hash, ext) = hash_path("a.txt");
    let r = Record { name_hash, ext, dir_hash, flags: RECORD_FLAGS, data_offset: data_start, packed_size: 0, unpacked_size: 1 };
    buf.extend_from_slice(&write_record(&r));
    buf.extend_from_slice(entry_data); // 1 byte of data
    buf.push(0xAB); // only 1 byte for the name-table length prefix (needs 2)

    let tmp = write_tmp(&buf);
    assert!(Ba2Archive::open(tmp.path()).is_err(), "truncated name-length prefix must be rejected");
}

#[test]
fn rejects_truncated_name_bytes() {
    // Name-table length prefix claims 100 chars but there are 0 name bytes.
    let entry_data = b"x";
    let data_start = 24u64 + 36;
    let nt_offset = data_start + 1;

    let mut buf = make_raw_header(1, b"GNRL", 1, nt_offset);

    use ba2::format::{write_record, Record, RECORD_FLAGS};
    use ba2::hash::hash_path;
    let (name_hash, dir_hash, ext) = hash_path("a.txt");
    let r = Record { name_hash, ext, dir_hash, flags: RECORD_FLAGS, data_offset: data_start, packed_size: 0, unpacked_size: 1 };
    buf.extend_from_slice(&write_record(&r));
    buf.extend_from_slice(entry_data);
    buf.extend_from_slice(&100u16.to_le_bytes()); // claims 100-char name
    // …but writes 0 name bytes

    let tmp = write_tmp(&buf);
    assert!(Ba2Archive::open(tmp.path()).is_err(), "truncated name string must be rejected");
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

    use ba2::format::{write_record, Record, RECORD_FLAGS};
    use ba2::hash::hash_path;
    let (name_hash, dir_hash, ext) = hash_path("data/x.bin");
    let r = Record {
        name_hash, ext, dir_hash,
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
