//! Concurrent `esm` processes against an ESM with no cache yet: each one's
//! missing sections are built by a detached `esm cache build` under the
//! shared build lease, and every caller gets the same answer.

mod common;

use std::process::{Command, Stdio};

#[test]
fn concurrent_cold_callers_all_succeed_with_identical_output() {
    let esm_path = common::unique_temp_path("cold_cache").with_extension("esm");
    std::fs::write(&esm_path, common::make_xref_esm()).unwrap();

    let children: Vec<_> = (0..6)
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_esm"))
                .arg("--esm")
                .arg(&esm_path)
                .args(["refs", "0x00000001", "--json"])
                .env("ESM_NO_PROGRESS", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn esm refs")
        })
        .collect();
    let outputs: Vec<_> = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect();

    for out in &outputs {
        assert!(
            out.status.success(),
            "esm refs failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, outputs[0].stdout);
    }
    let rows: serde_json::Value = serde_json::from_slice(&outputs[0].stdout).unwrap();
    assert!(
        rows.to_string().contains("0x00000002"),
        "the WEAP at 0x2 references 0x1: {rows}"
    );

    let _ = esm::progress::clear_cache(&esm_path);
    let _ = std::fs::remove_file(&esm_path);
}
