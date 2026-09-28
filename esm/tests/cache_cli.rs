//! `esm cache build` and `esm cache status` on the string and curve source
//! sections, over a synthetic ESM folder.

mod common;

use common::{tes4_header, unique_temp_path};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A folder holding one empty `Test.esm`.
fn esm_folder(stem: &str) -> PathBuf {
    let dir = unique_temp_path(stem).with_extension("");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Test.esm"), tes4_header()).unwrap();
    dir
}

fn esm(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esm"))
        .arg("--esm")
        .arg(dir.join("Test.esm"))
        .args(args)
        .env("ESM_NO_PROGRESS", "1")
        .output()
        .expect("run esm")
}

fn status(dir: &Path) -> serde_json::Value {
    let out = esm(dir, &["cache", "status", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn a_requested_source_section_that_fails_to_build_fails_the_command() {
    let dir = esm_folder("cache_bad_strings");
    std::fs::create_dir_all(dir.join("strings")).unwrap();
    std::fs::write(dir.join("strings/Test_en.strings"), [1, 2, 3]).unwrap();

    let out = esm(&dir, &["cache", "build", "--section", "lstrings"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lstrings section failed to build"),
        "{stderr}"
    );
    assert_eq!(
        status(&dir)["sections"]["lstrings"],
        serde_json::json!(false)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_requested_source_section_with_no_source_fails_the_command() {
    let dir = esm_folder("cache_no_curves");
    let out = esm(&dir, &["cache", "build", "--section", "curves"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("curves has no source"), "{stderr}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn status_reports_the_source_sections() {
    let dir = esm_folder("cache_status_sources");
    let curves = dir.join("misc/curvetables/json/sub");
    std::fs::create_dir_all(&curves).unwrap();

    let out = esm(&dir, &["cache", "build"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let built = status(&dir);
    assert_eq!(built["state"], "complete", "{built}");
    assert_eq!(built["sections"]["curves"], serde_json::json!(true));
    assert_eq!(
        built["sections"]["lstrings"],
        serde_json::Value::Null,
        "no string source"
    );

    // A curve file arriving leaves the curves section out of date.
    std::fs::write(curves.join("new.json"), "[]").unwrap();
    let stale = status(&dir);
    assert_eq!(stale["state"], "partial", "{stale}");
    assert_eq!(stale["sections"]["curves"], serde_json::json!(false));
    std::fs::remove_dir_all(&dir).ok();
}
