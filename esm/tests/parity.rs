//! Cross-surface parity regression guard.
//!
//! `src/ipc.rs` documents `dispatch_op` as "the one query-dispatch surface
//! shared by the daemon, CLI, HTTP/MCP server, and N-API bindings" — but nothing
//! previously asserted that the `Registry`-backed `dispatch` path (what the
//! daemon/CLI/HTTP-MCP server actually call) and the direct `dispatch_op` path
//! (what N-API calls against an already-open `Database`) agree on the exact
//! JSON produced for the same op. This test drives a handful of representative
//! ops through both paths against the same synthetic ESM and asserts the
//! resulting `serde_json::Value`s are equal — the regression guard that would
//! have caught N-API/CLI drifting from the canonical dispatch path.

mod common;

use common::{make_xref_esm, unique_temp_path};
use esm::ipc::{dispatch, dispatch_op, Op, RecordSel, Request, Response};
use esm::registry::Registry;
use esm::{Database, FormId, ResolveDepth, SearchField};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write [`make_xref_esm`]'s buffer (a WEAP(1) target + a WEAP(2) referencer
/// whose `YNAM`/`ZNAM` both point at it) to a unique temp path and hand back a
/// fresh `Registry` pointed at nothing yet — mirrors `tests/ipc.rs`'s
/// `open_test_db` convention, but keeps the ESM un-opened so each op-parity
/// check below opens it fresh via both paths.
fn setup() -> (PathBuf, Registry) {
    let buf = make_xref_esm();
    let path = unique_temp_path("parity");
    let mut f = std::fs::File::create(&path).expect("create temp esm");
    f.write_all(&buf).expect("write temp esm");
    (path, Registry::new())
}

/// Run `op` through both dispatch surfaces against the same synthetic ESM at
/// `path` and assert they produce identical JSON:
///
/// - `dispatch(reg, req)` — the `Registry`-backed path the daemon (and, via
///   `LocalBackend`, the CLI) actually calls.
/// - `dispatch_op(&mut db, &op)` — the direct path N-API calls against a
///   `Database` it already holds locked (see `bindings/napi/src/lib.rs`).
fn assert_parity(path: &Path, reg: &Registry, op: Op) {
    let req = Request {
        esm: path.to_path_buf(),
        op: op.clone(),
    };
    let via_registry = match dispatch(reg, &req) {
        Response::Ok { data } => data,
        Response::Err { error } => panic!("dispatch (registry path) failed for {op:?}: {error}"),
    };

    let mut db = Database::open(path).expect("open db directly for dispatch_op path");
    let via_direct = dispatch_op(&mut db, &op)
        .unwrap_or_else(|e| panic!("dispatch_op failed for {op:?}: {e:#}"));

    assert_eq!(
        via_registry, via_direct,
        "dispatch vs dispatch_op produced different JSON for {op:?}"
    );
}

#[test]
fn file_info_parity() {
    let (path, reg) = setup();
    assert_parity(&path, &reg, Op::FileInfo);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn record_parity() {
    let (path, reg) = setup();
    assert_parity(
        &path,
        &reg,
        Op::Record {
            sel: RecordSel::FormId(FormId(1)),
            depth: ResolveDepth::None,
        },
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn record_parity_with_stub_resolve() {
    // FormId(2) carries YNAM/ZNAM references to FormId(1) — exercise the
    // resolver-attached decode path (`ResolveDepth::Stub`), not just the bare
    // hex-output path `record_parity` above covers.
    let (path, reg) = setup();
    assert_parity(
        &path,
        &reg,
        Op::Record {
            sel: RecordSel::FormId(FormId(2)),
            depth: ResolveDepth::Stub,
        },
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn search_parity() {
    let (path, reg) = setup();
    assert_parity(
        &path,
        &reg,
        Op::Search {
            pattern: "*".to_string(),
            types: vec!["WEAP".to_string()],
            field: SearchField::Both,
            limit: 0,
        },
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn list_groups_parity() {
    let (path, reg) = setup();
    assert_parity(&path, &reg, Op::ListGroups);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn referenced_by_parity() {
    let (path, reg) = setup();
    assert_parity(
        &path,
        &reg,
        Op::ReferencedBy {
            sel: RecordSel::FormId(FormId(1)),
            limit: 0,
            depth: 1,
            type_filter: None,
            paths: false,
        },
    );
    let _ = std::fs::remove_file(&path);
}
