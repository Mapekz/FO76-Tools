//! Integration tests for `ba2::writer` — round-trips and layout properties.

use ba2::compress::Codec;
use ba2::hash::hash_path;
use ba2::reader::Ba2Archive;
use ba2::{ArchiveKind, WriteOptions, write_ba2};
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
    let opts = WriteOptions {
        kind: ArchiveKind::Gnrl,
        codec,
        min_shrink_ratio: 1.0,
    };
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
    write_ba2(
        out.path(),
        &files,
        &WriteOptions {
            kind: ArchiveKind::Gnrl,
            codec: Codec::Lz4,
            min_shrink_ratio: 1.0,
        },
    )
    .unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let entry_c = &archive.list()[0];
    let entry_i = &archive.list()[1];

    assert!(
        entry_c.is_compressed(),
        "compressible file must be LZ4-compressed"
    );
    assert!(
        !entry_i.is_compressed(),
        "incompressible 1-byte file must be stored"
    );

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

// ── DX10 create ───────────────────────────────────────────────────────────

/// Build an in-memory `.dds` file: a synthesized header followed by
/// deterministic (non-zero, distinguishable) filler mip data of exactly
/// `mip_data_len` bytes.
fn synthetic_dds(
    dxgi_format: u8,
    width: u16,
    height: u16,
    mip_count: u8,
    cubemap: bool,
    mip_data_len: usize,
) -> Vec<u8> {
    let mut dds = ba2::dds::synth_header(dxgi_format, width, height, mip_count, cubemap).unwrap();
    dds.extend((0..mip_data_len).map(|i| (i % 251) as u8));
    dds
}

/// A single-chunk (small, below the 512x512 chunking threshold) BC1 texture
/// round-trips byte-for-byte through DX10 create → open → read.
#[test]
fn dx10_create_small_texture_round_trips() {
    // 64x64 BC1_UNORM, 1 mip: mip0 size = 64*64*4/8 = 2048 bytes.
    let dds = synthetic_dds(71, 64, 64, 1, false, 2048);

    let src_dir = TempDir::new().unwrap();
    let src = src_dir.path().join("small.dds");
    std::fs::write(&src, &dds).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("textures/small.dds".to_string(), src)];
    let opts = WriteOptions {
        kind: ArchiveKind::Dx10,
        codec: Codec::Store,
        ..Default::default()
    };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    assert_eq!(archive.kind(), ArchiveKind::Dx10);
    assert_eq!(archive.list().len(), 1);
    let t = archive.list()[0].texture().unwrap();
    assert_eq!(t.dxgi_format, 71);
    assert_eq!(t.width, 64);
    assert_eq!(t.height, 64);
    assert_eq!(t.mip_count, 1);
    assert!(!t.cubemap);
    assert_eq!(
        t.chunks.len(),
        1,
        "below the 512x512 area threshold: 1 chunk"
    );

    let round_tripped = archive.read("textures/small.dds", Codec::Auto).unwrap();
    assert_eq!(
        round_tripped, dds,
        "extracted .dds must match the source byte-for-byte"
    );
}

/// A texture large enough to be split into multiple chunks (the exact
/// 1024x1024, 11-mip, 3-chunk shape ground-truthed against a real FO76
/// archive entry) round-trips, and each chunk's mip range is assigned
/// correctly.
#[test]
fn dx10_create_multi_chunk_round_trips() {
    // mip sizes for BC1_UNORM (bpp 4) at 1024x1024, 11 mips, with the 4x4
    // minimum block clamp on the tail mips: 524288 + 131072 + 43704.
    let total_mip_len = 524288 + 131072 + 43704;
    let dds = synthetic_dds(71, 1024, 1024, 11, false, total_mip_len);

    let src_dir = TempDir::new().unwrap();
    let src = src_dir.path().join("large.dds");
    std::fs::write(&src, &dds).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("textures/large.dds".to_string(), src)];
    let opts = WriteOptions {
        kind: ArchiveKind::Dx10,
        codec: Codec::Store,
        ..Default::default()
    };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let t = archive.list()[0].texture().unwrap();
    assert_eq!(
        t.chunks.len(),
        3,
        "1024x1024/11 mips must split into 3 chunks"
    );
    assert_eq!((t.chunks[0].mip_first, t.chunks[0].mip_last), (0, 0));
    assert_eq!((t.chunks[1].mip_first, t.chunks[1].mip_last), (1, 1));
    assert_eq!((t.chunks[2].mip_first, t.chunks[2].mip_last), (2, 10));
    assert_eq!(t.chunks[0].unpacked_size, 524288);
    assert_eq!(t.chunks[1].unpacked_size, 131072);
    assert_eq!(t.chunks[2].unpacked_size, 43704);

    let round_tripped = archive.read("textures/large.dds", Codec::Auto).unwrap();
    assert_eq!(round_tripped, dds);
}

/// A zlib-compressed multi-chunk texture also round-trips (chunks compress
/// and decompress independently).
#[test]
fn dx10_create_zlib_compressed_round_trips() {
    let total_mip_len = 524288 + 131072 + 43704;
    let dds = synthetic_dds(71, 1024, 1024, 11, false, total_mip_len);

    let src_dir = TempDir::new().unwrap();
    let src = src_dir.path().join("large.dds");
    std::fs::write(&src, &dds).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("textures/large.dds".to_string(), src)];
    let opts = WriteOptions {
        kind: ArchiveKind::Dx10,
        codec: Codec::Zlib,
        ..Default::default()
    };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let entry = &archive.list()[0];
    assert!(entry.is_compressed());

    let round_tripped = archive.read("textures/large.dds", Codec::Auto).unwrap();
    assert_eq!(round_tripped, dds);
}

/// Cubemaps are never chunked, regardless of size.
#[test]
fn dx10_create_cubemap_is_single_chunk() {
    // Large enough that a non-cubemap texture would chunk, but cubemap: 1 chunk.
    let dds = synthetic_dds(71, 2048, 2048, 1, true, 4096);

    let src_dir = TempDir::new().unwrap();
    let src = src_dir.path().join("cube.dds");
    std::fs::write(&src, &dds).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("textures/cube.dds".to_string(), src)];
    let opts = WriteOptions {
        kind: ArchiveKind::Dx10,
        codec: Codec::Store,
        ..Default::default()
    };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let t = archive.list()[0].texture().unwrap();
    assert!(t.cubemap);
    assert_eq!(t.chunks.len(), 1);

    let round_tripped = archive.read("textures/cube.dds", Codec::Auto).unwrap();
    assert_eq!(round_tripped, dds);
}

/// A DXT10-extension format (BC7, no legacy FourCC) round-trips its dxgi_format.
#[test]
fn dx10_create_dxt10_extension_format_round_trips() {
    // 32x32 BC7_UNORM, 1 mip: mip0 size = 32*32*8/8 = 1024 bytes.
    let dds = synthetic_dds(98, 32, 32, 1, false, 1024);

    let src_dir = TempDir::new().unwrap();
    let src = src_dir.path().join("bc7.dds");
    std::fs::write(&src, &dds).unwrap();

    let out = NamedTempFile::new().unwrap();
    let files = vec![("textures/bc7.dds".to_string(), src)];
    let opts = WriteOptions {
        kind: ArchiveKind::Dx10,
        codec: Codec::Store,
        ..Default::default()
    };
    write_ba2(out.path(), &files, &opts).unwrap();

    let archive = Ba2Archive::open(out.path()).unwrap();
    let t = archive.list()[0].texture().unwrap();
    assert_eq!(t.dxgi_format, 98);

    let round_tripped = archive.read("textures/bc7.dds", Codec::Auto).unwrap();
    assert_eq!(round_tripped, dds);
}
