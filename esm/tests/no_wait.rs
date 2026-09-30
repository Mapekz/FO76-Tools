//! `--no-wait` refuses to wait on a cache build another process holds, for
//! every command that could otherwise block on one: a query exits 75,
//! `cache build` exits 75, and `batch` answers the request with an `err`
//! envelope instead of blocking the stream.

mod common;

use esm::progress::{BuildLease, BuildStage};
use std::io::Write;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Wait for `child`, killing it and failing the test if it is still blocked
/// after a generous deadline — i.e. it waited on the lease.
fn finish(mut child: Child, what: &str) -> Output {
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().ok();
            panic!("{what} blocked on the held build lease despite --no-wait");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

fn esm(args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_esm"));
    cmd.arg("--no-wait")
        .args(args)
        .env("ESM_NO_PROGRESS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

#[test]
fn no_wait_never_blocks_on_a_running_build() {
    let esm_path = common::unique_temp_path("no_wait").with_extension("esm");
    std::fs::write(&esm_path, common::make_minimal_esm()).unwrap();
    let canonical = esm::discover::resolve_esm_path(&esm_path).unwrap();
    let esm_arg = esm_path.to_str().unwrap();
    let lease = BuildLease::acquire(&canonical, BuildStage::Forms, 10).unwrap();

    let info = finish(esm(&["--esm", esm_arg, "info"]).spawn().unwrap(), "info");
    assert_eq!(info.status.code(), Some(75));

    let build = esm(&["--esm", esm_arg, "cache", "build", "--section", "forms"])
        .spawn()
        .unwrap();
    let build = finish(build, "cache build");
    assert_eq!(build.status.code(), Some(75));

    let mut batch = esm(&["batch"]).spawn().unwrap();
    {
        let mut stdin = batch.stdin.take().unwrap();
        let file_info = serde_json::json!({"esm": esm_arg, "op": {"op": "file_info"}});
        writeln!(stdin, "{file_info}").unwrap();
    }
    let batch = finish(batch, "batch");
    assert!(batch.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8(batch.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "one envelope per request: {lines:?}");
    assert_eq!(lines[0]["status"], "err");
    assert!(
        lines[0]["error"].as_str().unwrap().contains("being built"),
        "{lines:?}"
    );

    drop(lease);
    let _ = esm::progress::clear_cache(&esm_path);
    let _ = std::fs::remove_file(&esm_path);
}
