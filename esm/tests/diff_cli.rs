//! `esm diff` as a command: each side's string tables come from its own
//! snapshot folder, and a localized side without them fails loudly.

mod common;

use std::path::PathBuf;
use std::process::Command;

/// A snapshot folder holding `common::make_minimal_esm()` as
/// `SeventySix.esm`, with the TES4 Localized flag set or not.
fn snapshot(name: &str, localized: bool) -> PathBuf {
    let dir = common::unique_temp_path(name).with_extension("d");
    std::fs::create_dir_all(&dir).unwrap();
    let mut esm = common::make_minimal_esm();
    if localized {
        esm[8] |= 0x80; // TES4 record flags: Localized
    }
    std::fs::write(dir.join("SeventySix.esm"), esm).unwrap();
    dir
}

fn diff(a: &PathBuf, b: &PathBuf) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_esm"))
        .args(["diff", "--json"])
        .arg(a)
        .arg(b)
        .env("ESM_NO_PROGRESS", "1")
        .output()
        .expect("spawn esm diff")
}

fn cleanup(dirs: &[&PathBuf]) {
    for dir in dirs {
        let _ = esm::progress::clear_cache(&dir.join("SeventySix.esm"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_localized_side_without_string_tables_fails_loudly() {
    let (a, b) = (snapshot("diff_loc_a", true), snapshot("diff_loc_b", true));
    let out = diff(&a, &b);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected a failure, got: {stderr}");
    assert!(
        stderr.contains("strings"),
        "stderr names the missing tables: {stderr}"
    );
    cleanup(&[&a, &b]);
}

#[test]
fn unlocalized_sides_need_no_string_tables() {
    let (a, b) = (
        snapshot("diff_plain_a", false),
        snapshot("diff_plain_b", false),
    );
    let out = diff(&a, &b);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["changed"], serde_json::json!([]));
    cleanup(&[&a, &b]);
}
