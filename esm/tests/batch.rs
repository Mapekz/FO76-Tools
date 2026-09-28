//! `esm batch`: one JSON request per stdin line in, one response envelope
//! per stdout line out, in order, with bad lines answered rather than fatal.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn answers_each_request_line_in_order() {
    let esm_path = common::unique_temp_path("batch").with_extension("esm");
    std::fs::write(&esm_path, common::make_minimal_esm()).unwrap();
    let esm = esm_path.display().to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_esm"))
        .arg("batch")
        .env("ESM_NO_PROGRESS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn esm batch");
    {
        let mut stdin = child.stdin.take().unwrap();
        let file_info = serde_json::json!({"esm": esm, "op": {"op": "file_info"}});
        writeln!(stdin, "{file_info}").unwrap();
        writeln!(stdin, "not json").unwrap();
        writeln!(stdin).unwrap();
        let missing = serde_json::json!({"esm": esm, "op": {"op": "record",
            "sel": {"kind": "form_id", "value": 0x7FFF_FFFFu32}, "depth": "none"}});
        writeln!(stdin, "{missing}").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());

    let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "blank lines get no answer: {lines:?}");
    assert_eq!(lines[0]["status"], "ok");
    assert_eq!(lines[1]["status"], "err");
    assert!(
        lines[1]["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid request")
    );
    assert_eq!(lines[2]["status"], "err");

    let _ = esm::progress::clear_cache(&esm_path);
    let _ = std::fs::remove_file(&esm_path);
}
