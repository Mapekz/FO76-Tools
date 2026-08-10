//! Integration tests for `src/discover.rs`'s `resolve_sources`/`resolve_esm_path` —
//! previously 0 tests despite `resolve_esm_path` being load-bearing: every
//! `esm_cache/` consumer (`Registry`, the CLI's progress watcher,
//! `backend.rs`'s `building_progress`) must key off the exact canonical path
//! it returns, not the raw, possibly-relative-or-folder input a caller
//! passed in (see that function's doc comment). These tests exercise the
//! three input shapes its doc comment claims are equivalent: a file, a
//! folder, and a relative path.

mod common;

use common::make_minimal_esm;
use esm::discover::{resolve_esm_path, resolve_sources};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// Collision-free temp directory for one test's fixtures, disambiguated by
/// pid + a per-process counter (mirrors `tests/common::unique_temp_path`'s
/// scheme) so parallel/sequential `cargo test` runs never collide.
fn fixture_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "esm_discover_test_{name}_{}_{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn write_esm(path: &std::path::Path) {
    let mut f = std::fs::File::create(path).expect("create fixture esm");
    f.write_all(&make_minimal_esm()).expect("write fixture esm");
}

/// A **file** input resolves to itself, canonicalized — the simplest case,
/// and the one every other case is checked against.
#[test]
fn file_input_resolves_to_itself_canonicalized() {
    let dir = fixture_dir("file_input");
    let esm_path = dir.join("SeventySix.esm");
    write_esm(&esm_path);

    let resolved = resolve_esm_path(&esm_path).expect("resolve file input");
    assert_eq!(resolved, esm_path.canonicalize().unwrap());

    // `resolve_sources` (the two-step resolution `resolve_esm_path` builds
    // on) must agree on the pre-canonicalization ESM path too — a file
    // input is used directly, no folder scan.
    let sources = resolve_sources(&esm_path, "en").expect("resolve_sources file input");
    assert_eq!(sources.esm, esm_path);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A **folder** input containing exactly one `.esm` resolves to that file,
/// canonicalized — the shape `esm --esm <folder>` documents as supported,
/// and the one `backend.rs`'s `building_progress` fix (Stage C) exists to
/// keep working against a cold daemon.
#[test]
fn folder_input_resolves_to_the_esm_file_inside_it() {
    let dir = fixture_dir("folder_input");
    let esm_path = dir.join("SeventySix.esm");
    write_esm(&esm_path);

    let resolved = resolve_esm_path(&dir).expect("resolve folder input");
    assert_eq!(
        resolved,
        esm_path.canonicalize().unwrap(),
        "a folder input must resolve to the single .esm file inside it"
    );

    let sources = resolve_sources(&dir, "en").expect("resolve_sources folder input");
    assert_eq!(sources.esm, esm_path);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A folder containing zero or multiple `.esm` files is a clear error, not
/// a silent pick — `resolve_esm_path`'s doc comment and `find_single_esm`
/// promise this; regression-guard it here since nothing else in this test
/// file exercises the failure path.
#[test]
fn folder_input_with_no_esm_file_is_an_error() {
    let dir = fixture_dir("folder_input_empty");
    assert!(resolve_esm_path(&dir).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folder_input_with_multiple_esm_files_is_an_error() {
    let dir = fixture_dir("folder_input_ambiguous");
    write_esm(&dir.join("SeventySix.esm"));
    write_esm(&dir.join("Other.esm"));
    let err = resolve_esm_path(&dir).expect_err("ambiguous folder must error");
    // `resolve_esm_path` wraps the underlying `find_single_esm` error with
    // an outer `.with_context(...)` (see `resolve_sources`), so the specific
    // "multiple .esm files" message is further down the anyhow chain, not
    // in `Display`'s top-level message — `{:#}` prints the full chain.
    let full = format!("{err:#}");
    assert!(
        full.contains("multiple .esm files"),
        "error chain should name the ambiguity, got: {full}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A **relative** path input canonicalizes to the same absolute path as the
/// equivalent absolute-path input — the third input shape
/// `resolve_esm_path`'s doc comment names alongside "folder" and "symlink"
/// as differing from the canonical form. Built relative to the crate root
/// (`cargo test`'s working directory) rather than via `std::env::set_current_dir`,
/// which would race every other test in this binary over one process-global
/// value.
#[test]
fn relative_path_input_canonicalizes_to_the_same_path_as_absolute() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rel_dir = PathBuf::from("target").join(format!(
        "esm_discover_test_relative_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let abs_dir = manifest_dir.join(&rel_dir);
    std::fs::create_dir_all(&abs_dir).expect("create relative fixture dir");
    let rel_esm = rel_dir.join("SeventySix.esm");
    let abs_esm = abs_dir.join("SeventySix.esm");
    write_esm(&abs_esm);

    // `cargo test` runs test binaries with the package root as the working
    // directory, so this relative path is meaningful without ever touching
    // process-wide CWD state.
    let resolved_relative = resolve_esm_path(&rel_esm).expect("resolve relative input");
    let resolved_absolute = resolve_esm_path(&abs_esm).expect("resolve absolute input");

    assert_eq!(
        resolved_relative, resolved_absolute,
        "a relative-path input must canonicalize to the same absolute path \
         as the equivalent absolute-path input"
    );
    assert!(resolved_relative.is_absolute());

    let _ = std::fs::remove_dir_all(&abs_dir);
}

/// Idempotency guarantee `resolve_sources`'s doc comment states explicitly:
/// a **file** input is never scanned for siblings of its parent directory —
/// only a directory input triggers the single-ESM folder scan. Regression
/// guard for that guarantee using a directory that would itself be
/// ambiguous (two `.esm` files) if it were ever scanned — a file input
/// pointing at one of them must still resolve cleanly.
#[test]
fn file_input_is_not_scanned_even_when_its_directory_would_be_ambiguous() {
    let dir = fixture_dir("file_input_ambiguous_dir");
    let esm_path = dir.join("SeventySix.esm");
    write_esm(&esm_path);
    write_esm(&dir.join("Other.esm"));

    let resolved = resolve_esm_path(&esm_path).expect("file input must not trigger folder scan");
    assert_eq!(resolved, esm_path.canonicalize().unwrap());

    let _ = std::fs::remove_dir_all(&dir);
}
