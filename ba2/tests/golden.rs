//! Writer goldens: fixed inputs must produce byte-identical archives. A
//! digest changes only when the writer's output changes; update the constant
//! when that change is intended.

use ba2::compress::Codec;
use ba2::{ArchiveKind, WriteOptions, write_ba2};
use tempfile::{NamedTempFile, TempDir};

/// FNV-1a 64.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

/// Write `files` (archive path, contents) with `kind`/`codec` and return the
/// archive's digest.
fn archive_digest(files: &[(&str, Vec<u8>)], kind: ArchiveKind, codec: Codec) -> u64 {
    let src = TempDir::new().unwrap();
    let inputs: Vec<(String, std::path::PathBuf)> = files
        .iter()
        .enumerate()
        .map(|(i, (name, bytes))| {
            let path = src.path().join(format!("{i}.bin"));
            std::fs::write(&path, bytes).unwrap();
            (name.to_string(), path)
        })
        .collect();
    let out = NamedTempFile::new().unwrap();
    let opts = WriteOptions {
        kind,
        codec,
        ..Default::default()
    };
    write_ba2(out.path(), &inputs, &opts).unwrap();
    fnv(&std::fs::read(out.path()).unwrap())
}

fn gnrl_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("meshes/a.nif", b"alpha content 1234567890".repeat(40)),
        (
            "Textures/Sub/B.dds",
            (0..3000u32).map(|i| (i * 7 % 256) as u8).collect(),
        ),
        ("strings/c.strings", Vec::new()),
        ("scripts/d.pex", b"x".to_vec()),
    ]
}

fn dds(dxgi_format: u8, width: u16, height: u16, mips: u8, cube: bool, len: usize) -> Vec<u8> {
    let mut dds = ba2::dds::synth_header(dxgi_format, width, height, mips, cube).unwrap();
    dds.extend((0..len).map(|i| (i % 251) as u8));
    dds
}

fn dx10_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        // 64x64 BC1, one mip: a single chunk.
        ("textures/small_d.dds", dds(71, 64, 64, 1, false, 2048)),
        // 1024x1024 BC1 with mips: split into several chunks.
        (
            "textures/large_d.dds",
            dds(71, 1024, 1024, 11, false, 699_064),
        ),
        // 64x64 BC1 cubemap, one mip.
        ("textures/cube_e.dds", dds(71, 64, 64, 1, true, 2048 * 6)),
    ]
}

#[test]
fn gnrl_store_archive_is_unchanged() {
    assert_eq!(
        archive_digest(&gnrl_files(), ArchiveKind::Gnrl, Codec::Store),
        0x3483d4b1b8c9af8f
    );
}

#[test]
fn gnrl_lz4_archive_is_unchanged() {
    assert_eq!(
        archive_digest(&gnrl_files(), ArchiveKind::Gnrl, Codec::Lz4),
        0xaa29d3fde8583c71
    );
}

#[test]
fn gnrl_zlib_archive_is_unchanged() {
    assert_eq!(
        archive_digest(&gnrl_files(), ArchiveKind::Gnrl, Codec::Zlib),
        0xe8b555bca1114e6d
    );
}

#[test]
fn dx10_zlib_archive_is_unchanged() {
    assert_eq!(
        archive_digest(&dx10_files(), ArchiveKind::Dx10, Codec::Zlib),
        0xf6e63239b4dea925
    );
}
