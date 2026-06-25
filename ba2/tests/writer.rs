//! Integration tests for `ba2::writer` — round-trips and layout properties.

use ba2::compress::Codec;
use ba2::hash::hash_path;
use ba2::reader::Ba2Archive;
use ba2::{write_ba2, WriteOptions};
use tempfile::{NamedTempFile, TempDir};

// ── Helpers ───────────────────────────────────────────────────────────────

fn round_trip(codec: Codec) -> (Vec<u8>, Vec<u8>) {
    let content_a = b"alpha content 1234567890".repeat(4);
    let content_b = b"beta  content ABCDEFGHIJ".repeat(4);

    let src_dir = TempDir::new().unwrap();
    let file_a = src_dir.path().join("a.txt");
    let file_b = src_dir.path().join("b.bin");
    std::fs::write(&file_a, &content_a).unwrap();
    std::fs::write(&file_b, &content_b).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![
        ("data/a.txt".to_string(), file_a),
        ("data/b.bin".to_string(), file_b),
    ];
    let opts = WriteOptions { codec, min_shrink_ratio: 1.0 };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    assert_eq!(archive.list().len(), 2);

    // Verify names and hashes.
    let entry_a = &archive.list()[0];
    let entry_b = &archive.list()[1];
    assert_eq!(entry_a.name, "data\\a.txt");
    assert_eq!(entry_b.name, "data\\b.bin");

    let (nh, dh, ext) = hash_path("data/a.txt");
    assert_eq!(entry_a.name_hash, nh);
    assert_eq!(entry_a.dir_hash, dh);
    assert_eq!(entry_a.ext, ext);

    let out_a = archive.read("data/a.txt", Codec::Auto).unwrap();
    let out_b = archive.read("data/b.bin", Codec::Auto).unwrap();
    (out_a, out_b)
}

// ── Codec round-trips ─────────────────────────────────────────────────────

#[test]
fn store_round_trip() {
    let content_a = b"alpha content 1234567890".repeat(4);
    let content_b = b"beta  content ABCDEFGHIJ".repeat(4);
    let (a, b) = round_trip(Codec::Store);
    assert_eq!(a, content_a.to_vec());
    assert_eq!(b, content_b.to_vec());
}

#[test]
fn lz4_round_trip() {
    let content_a = b"alpha content 1234567890".repeat(4);
    let content_b = b"beta  content ABCDEFGHIJ".repeat(4);
    let (a, b) = round_trip(Codec::Lz4);
    assert_eq!(a, content_a.to_vec());
    assert_eq!(b, content_b.to_vec());
}

#[test]
fn zlib_round_trip() {
    let content_a = b"alpha content 1234567890".repeat(4);
    let content_b = b"beta  content ABCDEFGHIJ".repeat(4);
    let (a, b) = round_trip(Codec::Zlib);
    assert_eq!(a, content_a.to_vec());
    assert_eq!(b, content_b.to_vec());
}

// ── Edge cases ────────────────────────────────────────────────────────────

/// An empty file list must produce a valid empty archive.
#[test]
fn empty_file_list() {
    let out = NamedTempFile::new().unwrap();
    write_ba2(out.path(), &[], &WriteOptions::default()).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    assert_eq!(archive.list().len(), 0);
    assert_eq!(archive.header.file_count, 0);
}

/// An archive with a mix of compressible and incompressible files should have
/// the right `is_compressed()` state for each entry, and both must round-trip.
#[test]
fn mixed_compress_and_store() {
    // 500 identical bytes compress well; 1 byte is too small to compress.
    let compressible: Vec<u8> = vec![0x41u8; 500]; // "AAAAAA..." — very compressible
    let incompressible: Vec<u8> = vec![0xFFu8; 1]; // 1 byte — LZ4 overhead makes it larger

    let src_dir = TempDir::new().unwrap();
    let file_c = src_dir.path().join("c.bin");
    let file_i = src_dir.path().join("i.bin");
    std::fs::write(&file_c, &compressible).unwrap();
    std::fs::write(&file_i, &incompressible).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![
        ("test/c.bin".to_string(), file_c),
        ("test/i.bin".to_string(), file_i),
    ];
    write_ba2(out.path(), &files, &WriteOptions { codec: Codec::Lz4, min_shrink_ratio: 1.0 })
        .unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let entry_c = &archive.list()[0];
    let entry_i = &archive.list()[1];

    assert!(entry_c.is_compressed(), "compressible file must be LZ4-compressed");
    assert!(!entry_i.is_compressed(), "incompressible 1-byte file must be stored");

    let data_c = archive.read("test/c.bin", Codec::Auto).unwrap();
    let data_i = archive.read("test/i.bin", Codec::Auto).unwrap();
    assert_eq!(data_c, compressible);
    assert_eq!(data_i, incompressible);
}

/// Forward-slash archive paths are normalised to backslash in the output.
#[test]
fn forward_slash_paths_normalised() {
    let content = b"slash test";
    let src_dir = TempDir::new().unwrap();
    let src = src_dir.path().join("file.txt");
    std::fs::write(&src, content).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("some/dir/file.txt".to_string(), src)];
    write_ba2(out.path(), &files, &WriteOptions::default()).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    assert_eq!(archive.list()[0].name, "some\\dir\\file.txt");
    // read() normalises forward-slash input, so both forms work.
    assert_eq!(
        archive.read("some/dir/file.txt", Codec::Auto).unwrap(),
        content.to_vec()
    );
}
