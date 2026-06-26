//! Integration tests for `ba2::extract` — extract_all, extract_one, filtering.

mod common;

use ba2::compress::Codec;
use ba2::extract::{extract_all, extract_one, ExtractOptions};
use tempfile::TempDir;

// ── extract_all ───────────────────────────────────────────────────────────

#[test]
fn extract_all_writes_files() {
    let content_a = b"file A content";
    let content_b = b"file B content";
    let entries: &[(&str, &[u8])] = &[("dir/a.txt", content_a), ("dir/b.txt", content_b)];

    let tmp = common::make_test_archive(entries);
    let archive = ba2::reader::Ba2Archive::open(tmp.path()).unwrap();
    let out = TempDir::new().unwrap();
    let opts = ExtractOptions::default();
    let count = extract_all(&archive, out.path(), &opts).unwrap();

    assert_eq!(count, 2);
    assert_eq!(
        std::fs::read(out.path().join("dir/a.txt")).unwrap(),
        content_a
    );
    assert_eq!(
        std::fs::read(out.path().join("dir/b.txt")).unwrap(),
        content_b
    );
}

#[test]
fn extract_all_with_glob_filter() {
    let entries: &[(&str, &[u8])] = &[
        ("strings/main.strings", b"string data"),
        ("interface/hud.swf", b"swf data"),
    ];
    let tmp = common::make_test_archive(entries);
    let archive = ba2::reader::Ba2Archive::open(tmp.path()).unwrap();
    let out = TempDir::new().unwrap();

    // Build a GlobSet matching only "strings/*".
    let glob = globset::Glob::new("strings/*").unwrap();
    let mut builder = globset::GlobSetBuilder::new();
    builder.add(glob);
    let gs = builder.build().unwrap();

    let opts = ExtractOptions {
        codec: Codec::Auto,
        filter: Some(gs),
    };
    let count = extract_all(&archive, out.path(), &opts).unwrap();

    assert_eq!(count, 1, "only the strings/* entry should be extracted");
    assert!(out.path().join("strings/main.strings").exists());
    assert!(!out.path().join("interface/hud.swf").exists());
}

// ── extract_one ───────────────────────────────────────────────────────────

#[test]
fn extract_one_writes_named_file() {
    let entries: &[(&str, &[u8])] = &[
        ("strings/en.strings", b"english strings"),
        ("strings/de.strings", b"german strings"),
    ];
    let tmp = common::make_test_archive(entries);
    let archive = ba2::reader::Ba2Archive::open(tmp.path()).unwrap();
    let out = TempDir::new().unwrap();

    let dest = extract_one(&archive, "strings/en.strings", out.path(), Codec::Auto).unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), b"english strings");
    // The other entry must not have been extracted.
    assert!(!out.path().join("strings/de.strings").exists());
}

#[test]
fn extract_one_is_case_insensitive() {
    let entries: &[(&str, &[u8])] = &[("interface/HUD.swf", b"swf bytes")];
    let tmp = common::make_test_archive(entries);
    let archive = ba2::reader::Ba2Archive::open(tmp.path()).unwrap();
    let out = TempDir::new().unwrap();

    // Archive stores the name lowercased; the caller may pass mixed case.
    let dest = extract_one(&archive, "INTERFACE/hud.swf", out.path(), Codec::Auto).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"swf bytes");
}

#[test]
fn extract_one_missing_returns_error() {
    let entries: &[(&str, &[u8])] = &[("foo/bar.txt", b"data")];
    let tmp = common::make_test_archive(entries);
    let archive = ba2::reader::Ba2Archive::open(tmp.path()).unwrap();
    let out = TempDir::new().unwrap();

    assert!(
        extract_one(&archive, "foo/nonexistent.txt", out.path(), Codec::Auto).is_err(),
        "extract_one for a missing entry must return an error"
    );
}
