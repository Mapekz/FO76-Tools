//! Concurrent `esm` processes against an ESM with no cache yet: each one's
//! missing sections are built by a detached `esm cache build` under the
//! shared build lease, each section once, and every caller gets the same
//! answer. A build outlives the process that started it.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A folder with a cold `Test.esm` and a source for every cache section: an
/// empty set of string tables and an empty curve-table tree.
fn cold_folder(stem: &str) -> PathBuf {
    let dir = common::unique_temp_path(stem).with_extension("");
    std::fs::create_dir_all(dir.join("strings")).unwrap();
    std::fs::create_dir_all(dir.join("misc/curvetables/json")).unwrap();
    std::fs::write(dir.join("Test.esm"), common::make_xref_esm()).unwrap();
    for ext in ["strings", "dlstrings", "ilstrings"] {
        // count 0, data size 0
        std::fs::write(dir.join(format!("strings/Test_en.{ext}")), [0u8; 8]).unwrap();
    }
    dir
}

/// `esm --esm <dir>/Test.esm <args>`, logging each section it (or a build
/// it delegates) publishes to `dir/build.log`.
fn esm(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_esm"));
    cmd.arg("--esm")
        .arg(dir.join("Test.esm"))
        .args(args)
        .env("ESM_NO_PROGRESS", "1")
        .env("ESM_BUILD_LOG", dir.join("build.log"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// How many times each section was published.
fn builds(dir: &Path) -> BTreeMap<String, usize> {
    let log = std::fs::read_to_string(dir.join("build.log")).unwrap_or_default();
    let mut counts = BTreeMap::new();
    for label in log.lines() {
        *counts.entry(label.to_string()).or_default() += 1;
    }
    counts
}

fn all_succeed(children: Vec<Child>) -> Vec<Vec<u8>> {
    children
        .into_iter()
        .map(|c| {
            let out = c.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "esm failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        })
        .collect()
}

fn cleanup(dir: &Path) {
    let _ = esm::progress::clear_cache(&dir.join("Test.esm"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn concurrent_cold_callers_all_succeed_with_identical_output() {
    let dir = cold_folder("cold_cache_refs");
    let children = (0..6)
        .map(|_| {
            esm(&dir, &["refs", "0x00000001", "--json"])
                .spawn()
                .expect("spawn esm refs")
        })
        .collect();
    let outputs = all_succeed(children);
    assert!(outputs.iter().all(|o| *o == outputs[0]));
    let rows: serde_json::Value = serde_json::from_slice(&outputs[0]).unwrap();
    assert!(
        rows.to_string().contains("0x00000002"),
        "the WEAP at 0x2 references 0x1: {rows}"
    );
    let built = builds(&dir);
    for section in ["forms", "tree", "xref", "lstrings", "curves"] {
        assert_eq!(built.get(section), Some(&1), "{section}: {built:?}");
    }
    cleanup(&dir);
}

#[test]
fn concurrent_cold_cache_builds_build_each_section_once() {
    let dir = cold_folder("cold_cache_build");
    let children = (0..6)
        .map(|_| {
            esm(&dir, &["cache", "build"])
                .spawn()
                .expect("spawn esm cache build")
        })
        .collect();
    all_succeed(children);
    let built = builds(&dir);
    let sections: Vec<&str> = esm::progress::BuildStage::SECTIONS
        .iter()
        .map(|s| s.label())
        .collect();
    assert_eq!(
        built,
        sections.iter().map(|s| (s.to_string(), 1)).collect(),
        "every section built exactly once"
    );
    cleanup(&dir);
}

/// Killing the caller's whole process group while its cold build waits
/// leaves the build running: the detached builder, in its own session,
/// finishes and publishes the section. Linux-only: it finds the builder
/// through `/proc`.
#[cfg(target_os = "linux")]
#[test]
fn a_cold_build_outlives_its_killed_caller() {
    use std::os::unix::process::CommandExt;

    let dir = cold_folder("cold_cache_kill");
    let esm_path = dir.join("Test.esm");
    // Hold the build lease, so the detached builder waits on it.
    let lease =
        esm::progress::BuildLease::acquire(&esm_path, esm::progress::BuildStage::Tree, 0).unwrap();
    let mut caller = esm(&dir, &["list", "--type", "WEAP", "--json"]);
    caller.process_group(0);
    let mut caller = caller.spawn().expect("spawn esm list");

    let deadline = Instant::now() + Duration::from_secs(20);
    while !builder_running(&esm_path) {
        assert!(Instant::now() < deadline, "no detached builder started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let killed = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", caller.id())])
        .status()
        .unwrap();
    assert!(killed.success());
    let _ = caller.wait();
    drop(lease);

    while builds(&dir).get("tree") != Some(&1) {
        assert!(
            Instant::now() < deadline,
            "the build didn't finish: {:?}",
            builds(&dir)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    cleanup(&dir);
}

/// Whether a detached `esm cache build` for `esm_path` is running (read
/// from Linux's `/proc`).
#[cfg(target_os = "linux")]
fn builder_running(esm_path: &Path) -> bool {
    let needle = format!("{}\0cache\0build\0", esm_path.display());
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
        .any(|cmdline| String::from_utf8_lossy(&cmdline).contains(&needle))
}
