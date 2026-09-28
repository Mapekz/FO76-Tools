//! `esm batch`: answer a stream of `Op` requests over stdin/stdout.
//!
//! Each input line is one JSON [`esm::ops::Request`] (`{"esm": <path>,
//! "op": {...}}`); each gets exactly one output line, the matching
//! [`esm::ops::Response`] envelope (`{"status": "ok", "data": ...}` or
//! `{"status": "err", "error": ...}`), in order. Databases stay open for the
//! life of the process, so a script that owns one `esm batch` child pays the
//! open cost once per ESM instead of once per call. The process exits when
//! stdin closes.

use esm::ops::{Request, Response};
use std::io::{BufRead, Write};

pub(crate) fn cmd_batch() -> anyhow::Result<()> {
    let host = esm::host::Host::new();
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => Response::from_result(host.run(&request.esm, &request.op)),
            Err(e) => Response::Err {
                error: format!("invalid request: {e}"),
            },
        };
        serde_json::to_writer(&mut out, &response)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    Ok(())
}
