//! `esm batch`: answer a stream of `Op` requests over stdin/stdout.
//!
//! Each input line is one JSON [`esm::ops::Request`] (`{"esm": <path>,
//! "op": {...}}`); each gets exactly one output line, the matching
//! [`esm::ops::Response`] envelope (`{"status": "ok", "data": ...}` or
//! `{"status": "err", "error": ...}`), in order. Databases stay open for the
//! life of the process, so a script that owns one `esm batch` child pays the
//! open cost once per ESM instead of once per call. With `--no-wait`, a
//! request whose ESM has a cache build already running is answered with an
//! `err` envelope instead of waiting for it. The process exits when stdin
//! closes.

use esm::ops::{Request, Response};
use std::io::{BufRead, Write};

pub(crate) fn cmd_batch(no_wait: bool) -> anyhow::Result<()> {
    let host = esm::host::Host::new();
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let esms = crate::op_esms(&request.esm, &request.op);
                match no_wait.then(|| crate::build_in_progress(&esms)).flatten() {
                    Some(progress) => Response::Err {
                        error: crate::build_in_progress_message(&progress),
                    },
                    None => Response::from_result(crate::progress_ui::watched(&esms, || {
                        host.run(&request.esm, &request.op)
                    })),
                }
            }
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
