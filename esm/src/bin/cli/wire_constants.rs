//! `esm dump-wire-constants` — the Rust-side facts `tools/esm_gateway.py`
//! hand-mirrors, printed as JSON. See `cmd_dump_wire_constants`'s doc and
//! `tools/regen_wire_constants.py`, which consumes this to (re)write the
//! checked-in `tools/wire_constants.py`.
//!
//! Two guards keep that checked-in mirror honest: this module's `tests` fail
//! under plain `cargo test` when it no longer matches what the Rust side
//! emits, and CI regenerates it and fails on any `git diff`.

use clap::Args as _;
use esm::backend::{
    CONNECT_TIMEOUT, DAEMON_FILENAME, HEALTH_POLL_INTERVAL, HEALTH_POLL_MAX,
    OP_TIMEOUT_DEFAULT_SECS,
};
use esm::ipc::{DEFAULT_MAX_DEPTH, Op, RecordSel, RefSort};
use esm::{DiffOptions, FilterOp, FormId, ResolveDepth, SearchField};
use std::path::PathBuf;

use crate::DiffArgs;

/// One representative value per [`Op`] variant, purely so
/// [`op_wire_names`] has something to serialize and pattern-match against.
/// Field values are otherwise arbitrary/empty — nothing here is ever
/// dispatched.
fn sample_ops() -> Vec<Op> {
    vec![
        Op::FileInfo,
        Op::Record {
            sel: RecordSel::FormId(FormId::new(0)),
            depth: ResolveDepth::default(),
        },
        Op::RecordBulk {
            sels: vec![],
            depth: ResolveDepth::default(),
        },
        Op::RecordRaw {
            sel: RecordSel::FormId(FormId::new(0)),
        },
        Op::ListByType {
            sig: String::new(),
            limit: 0,
        },
        Op::ListTypeRecords {
            sig: String::new(),
            offset: 0,
            limit: 0,
        },
        Op::FilterTypeRecords {
            sig: String::new(),
            path: None,
            filter_op: FilterOp::Exists,
            value: None,
            limit: 0,
        },
        Op::ListTypeFieldPaths { sig: String::new() },
        Op::Search {
            pattern: String::new(),
            types: vec![],
            field: SearchField::Both,
            limit: 0,
        },
        Op::ReferencedBy {
            sel: RecordSel::FormId(FormId::new(0)),
            limit: 0,
            depth: 0,
            type_filter: None,
            paths: false,
            sort: RefSort::Formid,
        },
        Op::RefPath {
            from: RecordSel::FormId(FormId::new(0)),
            to: RecordSel::FormId(FormId::new(0)),
            max_hops: 0,
            paths: false,
        },
        Op::Walk {
            sel: RecordSel::FormId(FormId::new(0)),
            depth: 0,
            ref_limit: 0,
            level: 0.0,
            want_refs: false,
        },
        Op::Chase {
            sel: RecordSel::FormId(FormId::new(0)),
            depth: 0,
            ref_limit: 0,
        },
        Op::DropTable {
            sel: RecordSel::FormId(FormId::new(0)),
            level: 0.0,
            max_depth: 0,
            strict: false,
        },
        Op::ListGroups,
        Op::ListTypeChildren {
            sig: String::new(),
            offset: 0,
            limit: 0,
        },
        Op::ListGroupChildren {
            group_offset: 0,
            offset: 0,
            limit: 0,
        },
        Op::RecordStubAt { offset: 0 },
        Op::Coverage {
            record_type: None,
            sample: 0,
        },
        Op::Diff {
            b: PathBuf::new(),
            record_type: None,
            options: DiffOptions::default(),
        },
        Op::Shutdown,
    ]
}

/// Exists purely as a compile-time completeness guard on [`sample_ops`]:
/// this `match` has no wildcard arm, so adding, removing, or renaming an
/// [`Op`] variant fails to compile here until `sample_ops` (and this match)
/// are updated — `cmd_dump_wire_constants`'s `op_names` list can't
/// silently fall out of sync with the real enum. Never actually called for
/// its behavior (every arm is a no-op); `sample_ops`' construction is what
/// does the real work, this only proves it's exhaustive.
#[allow(dead_code)]
fn assert_op_variant_covered(op: &Op) {
    match op {
        Op::FileInfo => {}
        Op::Record { .. } => {}
        Op::RecordBulk { .. } => {}
        Op::RecordRaw { .. } => {}
        Op::ListByType { .. } => {}
        Op::ListTypeRecords { .. } => {}
        Op::FilterTypeRecords { .. } => {}
        Op::ListTypeFieldPaths { .. } => {}
        Op::Search { .. } => {}
        Op::ReferencedBy { .. } => {}
        Op::RefPath { .. } => {}
        Op::Walk { .. } => {}
        Op::Chase { .. } => {}
        Op::DropTable { .. } => {}
        Op::ListGroups => {}
        Op::ListTypeChildren { .. } => {}
        Op::ListGroupChildren { .. } => {}
        Op::RecordStubAt { .. } => {}
        Op::Coverage { .. } => {}
        Op::Diff { .. } => {}
        Op::Shutdown => {}
    }
}

/// The `op` wire-tag string (`#[serde(tag = "op", rename_all =
/// "snake_case")]`) for every [`Op`] variant, derived from a real
/// `serde_json::to_value` round-trip over [`sample_ops`] rather than
/// hand-typed — a rename that changes serde's actual output would change
/// this list too, not just the doc comment claiming it.
fn op_wire_names() -> Vec<String> {
    sample_ops()
        .iter()
        .map(|op| {
            assert_op_variant_covered(op);
            let value = serde_json::to_value(op).expect("Op always serializes");
            value
                .get("op")
                .and_then(|t| t.as_str())
                .expect("tagged Op enum always has an \"op\" field")
                .to_string()
        })
        .collect()
}

/// Long-form flag names `esm --local diff` accepts, introspected from
/// `DiffArgs`' own `clap::Args` impl (the same struct `Commands::Diff`
/// carries) rather than hand-typed — a renamed/added/removed `#[arg(...)]`
/// changes this list automatically, no separate string table to forget to
/// update.
fn diff_flag_names() -> Vec<String> {
    let cmd = DiffArgs::augment_args(clap::Command::new("diff"));
    let mut names: Vec<String> = cmd
        .get_arguments()
        .filter_map(|a| a.get_long().map(|s| format!("--{s}")))
        .collect();
    names.sort();
    names
}

/// The whole wire-constants payload as JSON — the single value both
/// [`cmd_dump_wire_constants`] and the drift test below read, so neither can
/// check a different set of facts than the other emits.
fn wire_constants_json() -> serde_json::Value {
    // Worked examples of FormId::display()'s "0x{:08X}" format, keyed by the
    // raw u32 (as a decimal string, since JSON object keys must be strings)
    // -- lets the Python side assert its own hex formatting against real
    // Rust output instead of just trusting the doc comment describing it.
    let mut form_id_display_examples = serde_json::Map::new();
    for raw in [0u32, 0x0000463F, 0x00ABCDEF, 0xFFFFFFFF] {
        form_id_display_examples.insert(raw.to_string(), FormId::new(raw).display().into());
    }

    serde_json::json!({
        "daemon_filename": DAEMON_FILENAME,
        "connect_timeout_secs": CONNECT_TIMEOUT.as_secs_f64(),
        "health_poll_interval_secs": HEALTH_POLL_INTERVAL.as_secs_f64(),
        "health_poll_max_secs": HEALTH_POLL_MAX.as_secs_f64(),
        "op_timeout_secs": OP_TIMEOUT_DEFAULT_SECS as f64,
        "default_max_depth": DEFAULT_MAX_DEPTH,
        "op_names": op_wire_names(),
        "form_id_display_examples": form_id_display_examples,
        "diff_flags": diff_flag_names(),
    })
}

/// `esm dump-wire-constants` — prints the Rust-side facts
/// `tools/esm_gateway.py` hand-mirrors as JSON. See that module's docstring
/// and `tools/regen_wire_constants.py`, which consumes this to (re)write
/// the checked-in `tools/wire_constants.py`.
pub(crate) fn cmd_dump_wire_constants() -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&wire_constants_json())?);
    Ok(())
}

/// Drift guard on the checked-in `tools/wire_constants.py`: it must already
/// say what [`wire_constants_json`] currently emits, so a new `Op` variant or
/// `DiffArgs` flag fails `cargo test` here rather than only in CI's
/// regen-and-`git diff` step. Lives in this module because `op_wire_names`,
/// `diff_flag_names`, and `DiffArgs` are private to the `esm` binary and
/// unreachable from `tests/`. Reads the file directly instead of re-rendering
/// it, so the Python renderer's exact formatting stays Python's business.
#[cfg(test)]
mod tests {
    use super::wire_constants_json;

    const STALE: &str =
        "tools/wire_constants.py is stale -- run `python3 tools/regen_wire_constants.py`";

    fn source() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tools/wire_constants.py"
        ))
        .expect("checked-in tools/wire_constants.py is readable")
    }

    /// The text after `= ` on the `NAME: ty = <value>` line `render()` writes.
    fn scalar<'a>(src: &'a str, name: &str) -> &'a str {
        src.lines()
            .find(|line| line.starts_with(name) && line.contains(" = "))
            .unwrap_or_else(|| panic!("{name} missing from wire_constants.py -- {STALE}"))
            .split_once(" = ")
            .expect("line was matched on \" = \"")
            .1
    }

    /// The one-per-line items `render()` writes between a collection's opening
    /// line and its closing brace, trailing comma stripped but quotes left on
    /// so callers compare against the literal Python spelling.
    fn entries(src: &str, open: &str, close: &str) -> Vec<String> {
        let body = src
            .split_once(open)
            .unwrap_or_else(|| panic!("{open} missing from wire_constants.py -- {STALE}"))
            .1;
        let body = body
            .split_once(close)
            .unwrap_or_else(|| panic!("unterminated {open} in wire_constants.py"))
            .0;
        body.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| line.trim_end_matches(',').to_string())
            .collect()
    }

    /// Scalars compare numerically, never as text: Python's `repr()` renders
    /// `2.0` where Rust's `Display` renders `2`, and replicating that spelling
    /// would test the formatting rather than the value.
    #[test]
    fn scalars_match_the_rust_source() {
        let json = wire_constants_json();
        let src = source();

        assert_eq!(
            scalar(&src, "DAEMON_FILENAME").trim_matches('\''),
            json["daemon_filename"].as_str().unwrap(),
            "{STALE}"
        );

        for (py_name, json_key) in [
            ("CONNECT_TIMEOUT_SECS", "connect_timeout_secs"),
            ("HEALTH_POLL_INTERVAL_SECS", "health_poll_interval_secs"),
            ("HEALTH_POLL_MAX_SECS", "health_poll_max_secs"),
            ("OP_TIMEOUT_SECS", "op_timeout_secs"),
        ] {
            let got: f64 = scalar(&src, py_name)
                .parse()
                .unwrap_or_else(|_| panic!("{py_name} is not a float literal -- {STALE}"));
            assert_eq!(got, json[json_key].as_f64().unwrap(), "{py_name}: {STALE}");
        }

        let depth: u64 = scalar(&src, "DEFAULT_MAX_DEPTH")
            .parse()
            .unwrap_or_else(|_| panic!("DEFAULT_MAX_DEPTH is not an int literal -- {STALE}"));
        assert_eq!(
            depth,
            json["default_max_depth"].as_u64().unwrap(),
            "{STALE}"
        );
    }

    /// Order matters as well as membership: `render()` emits these in the
    /// order the Rust side produces them, so CI's `git diff --exit-code`
    /// fails on a reordering too.
    #[test]
    fn collections_match_the_rust_source() {
        let json = wire_constants_json();
        let src = source();

        // Quoted the way Python's repr() spells these -- every value here is
        // an ASCII identifier or flag, so single quotes and no escaping.
        let expect_strs = |key: &str| -> Vec<String> {
            json[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| format!("'{}'", v.as_str().unwrap()))
                .collect()
        };

        assert_eq!(
            entries(&src, "OP_NAMES: frozenset[str] = frozenset({", "})"),
            expect_strs("op_names"),
            "OP_NAMES: {STALE}"
        );
        assert_eq!(
            entries(&src, "DIFF_FLAGS: frozenset[str] = frozenset({", "})"),
            expect_strs("diff_flags"),
            "DIFF_FLAGS: {STALE}"
        );

        let examples: Vec<String> = json["form_id_display_examples"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(raw, shown)| format!("'{raw}': '{}'", shown.as_str().unwrap()))
            .collect();
        assert_eq!(
            entries(&src, "FORM_ID_DISPLAY_EXAMPLES: dict[str, str] = {", "\n}"),
            examples,
            "FORM_ID_DISPLAY_EXAMPLES: {STALE}"
        );
    }
}
