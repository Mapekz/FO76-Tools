//! Integration tests for `ba2::hash` — Bethesda CRC-32 and path hashing.
//!
//! Hash values are ground-truthed against real FO76 archive entries
//! (SeventySix - Startup.ba2 and SeventySix - Localization.ba2).

use ba2::hash::{beth_crc, hash_path};

// ── beth_crc ─────────────────────────────────────────────────────────────────

/// Golden-value regression pin for `beth_crc`.
///
/// The Bethesda CRC variant uses poly `0xEDB88320`, **init 0**, and **no
/// final XOR**.  This differs from standard CRC-32 (init `0xFFFF_FFFF`,
/// final XOR `0xFFFF_FFFF`, which gives `0x3610_A686` for "hello").
///
/// The expected value below was determined empirically from the algorithm
/// and must not change — any change indicates the algorithm was accidentally
/// altered, which would break hash matching against real game archives.
#[test]
fn beth_crc_golden_value() {
    let got = beth_crc(b"hello");
    assert_eq!(
        got, 0xF032_519B,
        "beth_crc(b\"hello\") regressed: got 0x{:08X}, expected 0xF032519B — \
        if the algorithm intentionally changed, update this pin AND re-verify \
        against real archive hashes",
        got
    );
    // Must differ from standard CRC-32 of "hello".
    assert_ne!(got, 0x3610_A686, "must NOT equal standard CRC-32");
}

#[test]
fn beth_crc_empty() {
    // init=0 and no bytes → result is 0.
    assert_eq!(beth_crc(b""), 0);
}

// ── hash_path — existing ground-truth vectors ─────────────────────────────

/// These vectors were read directly from the parsed sample archives.
#[test]
fn root_file_dir_hash_zero() {
    let (_, dir_hash, ext) = hash_path("archive-lists.txt");
    assert_eq!(dir_hash, 0, "root file must have dir_hash == 0");
    assert_eq!(&ext, b"txt\0");
}

#[test]
fn root_file_name_hash() {
    // Verified against `archive-lists.txt` entry in SeventySix - Startup.ba2.
    let (name_hash, _, _) = hash_path("archive-lists.txt");
    assert_eq!(name_hash, 0x26551af7);
}

#[test]
fn extension_truncated_to_4_bytes() {
    // "dlstrings" → "dlst"
    let (_, _, ext) = hash_path("strings/nw_de.dlstrings");
    assert_eq!(&ext, b"dlst");

    // "ilstrings" → "ilst"
    let (_, _, ext2) = hash_path("strings/nw_de.ilstrings");
    assert_eq!(&ext2, b"ilst");

    // "strings" → "stri"
    let (_, _, ext3) = hash_path("strings/seventysix_en.strings");
    assert_eq!(&ext3, b"stri");
}

#[test]
fn slash_and_backslash_equivalent() {
    let a = hash_path("interface/translate_de.txt");
    let b = hash_path("interface\\translate_de.txt");
    assert_eq!(a, b);
}

#[test]
fn case_insensitive() {
    let a = hash_path("Interface/Translate_DE.txt");
    let b = hash_path("interface/translate_de.txt");
    assert_eq!(a, b);
}

#[test]
fn no_extension_file() {
    // File with no dot: stem is the whole filename, ext is all zeros.
    let (_, _, ext) = hash_path("somedir/noext");
    assert_eq!(ext, [0u8; 4]);
}

// ── hash_path — edge cases ────────────────────────────────────────────────

/// A trailing dot produces an empty extension (the stem is the full filename
/// including the dot-less part after the last dot, which is the empty string).
#[test]
fn hash_path_trailing_dot() {
    let (_, _, ext) = hash_path("dir/file.");
    assert_eq!(ext, [0u8; 4], "trailing dot → empty extension");
}

/// Only the last dot separates the extension.
#[test]
fn hash_path_multiple_dots() {
    let (_, _, ext) = hash_path("dir/file.tar.gz");
    assert_eq!(&ext, b"gz\0\0", "extension is only the segment after the last dot");
}

/// Empty string must not panic.
#[test]
fn hash_path_empty_string() {
    // dir="" and stem="" → both hash empty bytes.  ext=[0;4].
    let (name_hash, dir_hash, ext) = hash_path("");
    assert_eq!(name_hash, beth_crc(b""), "empty stem hashes empty bytes");
    assert_eq!(dir_hash, beth_crc(b""), "empty dir hashes empty bytes");
    assert_eq!(ext, [0u8; 4]);
}
