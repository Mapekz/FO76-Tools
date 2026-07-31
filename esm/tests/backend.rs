//! Exercises `RemoteBackend` over a real TCP socket — no existing test does this.
//! `tests/parity.rs`/`tests/ipc.rs` cover `dispatch`/`LocalBackend` only, and
//! `src/bin/server.rs`'s router tests use `tower::ServiceExt::oneshot` in-process
//! (auth/routing, never real body sizes). A hand-rolled stub HTTP/1.1 server stands in
//! for `esm-server` here so these tests need no game data, no `--features server`
//! build, and never touch the global `esm-daemon.json` discovery file or spawn lock —
//! `RemoteBackend::new` is constructed directly against the stub's loopback port.
//!
//! Covers issue #26 (bulk `get` over the daemon hard-fails past ureq's 10 MiB response
//! cap) and its chunking complement in `RemoteBackend::run`.

use esm::ResolveDepth;
use esm::backend::{QueryBackend, RemoteBackend};
use esm::formid::FormId;
use esm::ipc::{Op, RecordSel};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

/// Per-request callback: given the parsed JSON body the client sent, return the JSON
/// body to send back as the HTTP response (typically the full `{"status":"ok","data":
/// ...}` envelope `RemoteBackend` expects — see `esm::ipc::Response`).
type Handler = Box<dyn Fn(&serde_json::Value) -> serde_json::Value + Send + 'static>;

/// Start a minimal HTTP/1.1 server on an OS-assigned loopback port, on a dedicated
/// thread for the remainder of the test process (no shutdown handshake — the thread
/// just blocks in `accept()` once the test is done, which is fine for a short-lived
/// test binary). Every request's parsed JSON body is appended to the returned `Vec`
/// (guarded by the `Mutex`) so tests can assert how many round-trips happened and what
/// each one carried — e.g. "was a 1300-selector bulk get actually split into 3 calls."
fn spawn_stub_server(handler: Handler) -> (u16, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let port = listener.local_addr().expect("local_addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_thread = Arc::clone(&seen);

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(body) = read_request_body(&mut stream) else {
                continue;
            };
            let req_json: serde_json::Value =
                serde_json::from_slice(&body).expect("stub server: request body is valid JSON");
            seen_thread.lock().unwrap().push(req_json.clone());
            let resp_json = handler(&req_json);
            write_json_response(&mut stream, &resp_json);
        }
    });

    (port, seen)
}

/// Read one HTTP/1.1 request off `stream` and return its body bytes (per
/// `Content-Length`), discarding the request line and headers. Good enough for
/// `ureq::post(..).send_json(..)`, which always sends a known-length body.
fn read_request_body(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None; // peer closed before headers finished
        }
        let line = line.trim_end();
        if line.is_empty() {
            break; // blank line ends the header block
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

fn write_json_response(stream: &mut TcpStream, body: &serde_json::Value) {
    let body = serde_json::to_vec(body).expect("serialize stub response");
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

/// The exact failure this issue reports: a response past ureq 3's hard-coded 10 MiB
/// `Body::read_json` default. Fails on `main` with
/// "json: the response body is larger than request limit: 10485760 …"; must succeed
/// once `RemoteBackend` routes through `read_json_unlimited`.
#[test]
fn remote_backend_accepts_response_over_ureq_default_cap() {
    // ~1 KB per entry * 11,000 ≈ 11 MB of serialized JSON — comfortably past the 10 MiB
    // (10,485,760 byte) cap named in the issue.
    let big: Vec<String> = (0..11_000)
        .map(|i| format!("{i}-{}", "x".repeat(1000)))
        .collect();
    let (port, _seen) = spawn_stub_server(Box::new(
        move |_req| serde_json::json!({ "status": "ok", "data": big }),
    ));

    let mut backend = RemoteBackend::new("127.0.0.1", port, "token".to_string());
    let data = backend
        .run(Path::new("/nonexistent.esm"), Op::FileInfo)
        .expect("an 11 MB response must not be rejected as too large");

    assert_eq!(data.as_array().expect("data is an array").len(), 11_000);
}

/// A bulk `get` over the chunk threshold must be split into multiple `/op`
/// round-trips, one per `chunks(bulk_chunk)` slice, and the per-chunk result arrays
/// must be concatenated back in selector order — see `RemoteBackend::run_bulk_chunked`.
#[test]
fn remote_backend_chunks_bulk_record_requests() {
    let (port, seen) = spawn_stub_server(Box::new(|req| {
        let sels = req["op"]["sels"].as_array().expect("sels array");
        let entries: Vec<serde_json::Value> = sels
            .iter()
            .map(|s| serde_json::json!({ "sel": s["value"] }))
            .collect();
        serde_json::json!({ "status": "ok", "data": entries })
    }));

    let mut backend =
        RemoteBackend::new("127.0.0.1", port, "token".to_string()).with_bulk_chunk(512);
    let sels: Vec<RecordSel> = (0..1300u32).map(|i| RecordSel::FormId(FormId(i))).collect();
    let data = backend
        .run(
            Path::new("/nonexistent.esm"),
            Op::RecordBulk {
                sels,
                depth: ResolveDepth::None,
            },
        )
        .expect("chunked bulk get must still succeed as one logical call");

    let requests = seen.lock().unwrap();
    assert_eq!(
        requests.len(),
        3,
        "1300 selectors at chunk=512 must be 3 round-trips"
    );
    let chunk_lens: Vec<usize> = requests
        .iter()
        .map(|r| r["op"]["sels"].as_array().unwrap().len())
        .collect();
    assert_eq!(chunk_lens, vec![512, 512, 276]);

    let entries = data.as_array().expect("merged result is a JSON array");
    assert_eq!(entries.len(), 1300);
    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(
            entry["sel"],
            serde_json::json!(i as u32),
            "entry {i} must correlate back to the selector at that position"
        );
    }
}

/// Below the chunk threshold, the existing single-request wire shape is untouched.
#[test]
fn remote_backend_does_not_chunk_below_threshold() {
    let (port, seen) = spawn_stub_server(Box::new(|req| {
        let sels = req["op"]["sels"].as_array().expect("sels array");
        let entries: Vec<serde_json::Value> = sels
            .iter()
            .map(|s| serde_json::json!({ "sel": s["value"] }))
            .collect();
        serde_json::json!({ "status": "ok", "data": entries })
    }));

    let mut backend =
        RemoteBackend::new("127.0.0.1", port, "token".to_string()).with_bulk_chunk(512);
    let sels: Vec<RecordSel> = (0..500u32).map(|i| RecordSel::FormId(FormId(i))).collect();
    let data = backend
        .run(
            Path::new("/nonexistent.esm"),
            Op::RecordBulk {
                sels,
                depth: ResolveDepth::None,
            },
        )
        .expect("unchunked bulk get succeeds");

    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "500 selectors at chunk=512 must stay a single request"
    );
    assert_eq!(data.as_array().expect("data is an array").len(), 500);
}
