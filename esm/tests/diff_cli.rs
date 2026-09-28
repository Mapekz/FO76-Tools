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

#[test]
fn a_cache_build_in_progress_is_reported_on_stderr() {
    let (a, b) = (
        snapshot("diff_progress_a", false),
        snapshot("diff_progress_b", false),
    );
    let esm_b = std::fs::canonicalize(b.join("SeventySix.esm")).unwrap();
    let lease = esm::progress::BuildLease::acquire(&esm_b, esm::progress::BuildStage::Forms, 1)
        .expect("hold side b's build lease");
    let child = Command::new(env!("CARGO_BIN_EXE_esm"))
        .args(["diff", "--json"])
        .arg(&a)
        .arg(&b)
        .env_remove("ESM_NO_PROGRESS")
        .env("ESM_PROGRESS_GRACE_MS", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn esm diff");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    drop(lease);
    let out = child.wait_with_output().expect("wait for esm diff");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "diff failed: {stderr}");
    assert!(
        stderr.contains("building index cache"),
        "diff reports the build it waits on: {stderr}"
    );
    cleanup(&[&a, &b]);
}
