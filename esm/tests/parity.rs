//! Cross-surface parity regression guard.
//!
//! `Host::run` (what the CLI and `esm batch` call) and a direct `dispatch_op`
//! against an already-open `Database` must produce the same JSON for the same
//! op. This drives representative ops through both against the same
//! synthetic ESM and asserts the resulting `serde_json::Value`s are equal.

mod common;

use common::{make_xref_esm, unique_temp_path};
use esm::diff::DiffOptions;
use esm::host::Host;
use esm::ops::{Op, RecordSel, dispatch_op, run_diff};
use esm::{Database, FormId, ResolveDepth, SearchField};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write [`make_xref_esm`]'s buffer (a WEAP(1) target + a WEAP(2) referencer
/// whose `YNAM`/`ZNAM` both point at it) to a unique temp path and hand back a
/// fresh `Host` that hasn't opened it yet.
fn setup() -> (PathBuf, Host) {
    let buf = make_xref_esm();
    let path = unique_temp_path("parity");
    let mut f = std::fs::File::create(&path).expect("create temp esm");
    f.write_all(&buf).expect("write temp esm");
    (path, Host::new())
}

/// Run `op` through `Host::run` and through `dispatch_op` on a directly
/// opened `Database`, and assert identical JSON.
fn assert_parity(path: &Path, host: &Host, op: Op) {
    let via_host = host
        .run(path, &op)
        .unwrap_or_else(|e| panic!("Host::run failed for {op:?}: {e:#}"));
    let db = Database::open(path).expect("open db directly for dispatch_op path");
    let via_direct =
        dispatch_op(&db, &op).unwrap_or_else(|e| panic!("dispatch_op failed for {op:?}: {e:#}"));
    assert_eq!(
        via_host, via_direct,
        "Host::run vs dispatch_op produced different JSON for {op:?}"
    );
}

/// `Op::Diff` needs two databases, so diff parity is checked between
/// `Host::run` and [`run_diff`] on two directly opened databases.
fn assert_diff_parity(
    path_a: &Path,
    path_b: &Path,
    host: &Host,
    options: &DiffOptions,
    record_type: &Option<String>,
) {
    let op = Op::Diff {
        b: path_b.to_path_buf(),
        record_type: record_type.clone(),
        options: options.clone(),
    };
    let via_host = host
        .run(path_a, &op)
        .unwrap_or_else(|e| panic!("Host::run diff failed: {e:#}"));
    let db_a = Database::open(path_a).expect("open path_a");
    let db_b = Database::open(path_b).expect("open path_b");
    let via_direct = run_diff(&db_a, &db_b, options, record_type)
        .unwrap_or_else(|e| panic!("run_diff failed: {e:#}"));
    assert_eq!(
        via_host, via_direct,
        "Host::run vs run_diff produced different JSON for {path_a:?} vs {path_b:?}"
    );
}

#[test]
fn file_info_parity() {
    let (path, host) = setup();
    assert_parity(&path, &host, Op::FileInfo);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn record_parity() {
    let (path, host) = setup();
    assert_parity(
        &path,
        &host,
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
    let (path, host) = setup();
    assert_parity(
        &path,
        &host,
        Op::Record {
            sel: RecordSel::FormId(FormId(2)),
            depth: ResolveDepth::Stub,
        },
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn search_parity() {
    let (path, host) = setup();
    assert_parity(
        &path,
        &host,
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
    let (path, host) = setup();
    assert_parity(&path, &host, Op::ListGroups);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn referenced_by_parity() {
    let (path, host) = setup();
    assert_parity(
        &path,
        &host,
        Op::ReferencedBy {
            sel: RecordSel::FormId(FormId(1)),
            limit: 0,
            depth: 1,
            type_filter: None,
            paths: false,
            sort: esm::ops::RefSort::Formid,
        },
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn diff_parity_distinct_paths() {
    let (path_a, host) = setup();
    let buf = make_xref_esm();
    let path_b = unique_temp_path("parity-diff-b");
    let mut f = std::fs::File::create(&path_b).expect("create second temp esm");
    f.write_all(&buf).expect("write second temp esm");
    assert_diff_parity(&path_a, &path_b, &host, &DiffOptions::default(), &None);
    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

#[test]
fn diff_parity_same_database() {
    // Both diff operands are the same open database.
    let (path, host) = setup();
    assert_diff_parity(&path, &path, &host, &DiffOptions::default(), &None);
    let _ = std::fs::remove_file(&path);
}
