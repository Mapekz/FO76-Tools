//! `info` / `tree` / `coverage` subcommand handlers — simple read-and-print
//! commands that don't warrant their own module.

use esm::ops::Op;
use esm::{CoverageReport, FormIdBase, Markers};
use serde_json::Value;
use std::path::Path;

use crate::Backend;
use crate::output::{print_json, render_form_id};

/// Rewrite `tree`'s identity FormID labels into `base`, in place. `tree`'s
/// JSON (`Op::ListGroups`/`Op::ListTypeChildren`) is always a flat array of
/// `GroupNode`/`GroupChild` objects — never recursively nested (children are
/// fetched per level, not embedded) — so one pass over the top-level array
/// (plus each node's own `label` sub-object) covers every FormID:
/// `GroupLabel::FormId.form_id`, `GroupLabel::CellChildren.cell`, and
/// `GroupChild::Record`'s top-level `form_id`. See `src/tree.rs`.
fn convert_tree_json(v: &mut Value, base: FormIdBase) {
    let Value::Array(nodes) = v else { return };
    for node in nodes.iter_mut() {
        if let Some(Value::String(s)) = node.get_mut("form_id") {
            *s = render_form_id(s, base);
        }
        if let Some(label) = node.get_mut("label").and_then(Value::as_object_mut) {
            for key in ["form_id", "cell"] {
                if let Some(Value::String(s)) = label.get_mut(key) {
                    *s = render_form_id(s, base);
                }
            }
        }
    }
}

pub(crate) fn cmd_info(backend: &mut Backend, file: &Path) -> anyhow::Result<()> {
    let info: esm::reader::FileInfo =
        serde_json::from_value(backend.run(file, Op::FileInfo(esm::ops::NoArgs {}))?)?;
    println!("File: {}", file.display());
    println!("Version: {}", info.version);
    println!("Record count: {}", info.record_count);
    println!("Next Object ID: 0x{:08X}", info.next_object_id);
    println!("Flags: 0x{:08X}", info.flags);
    println!("ESM: {}", info.is_esm);
    println!("Localized: {}", info.is_localized);
    if let Some(a) = &info.author {
        println!("Author: {}", a);
    }
    if let Some(d) = &info.description {
        println!("Description: {}", d);
    }
    if !info.masters.is_empty() {
        println!("Masters:");
        for m in &info.masters {
            println!("  - {}", m);
        }
    }
    Ok(())
}

pub(crate) fn cmd_tree(
    backend: &mut Backend,
    file: &Path,
    record_type: Option<&str>,
    offset: usize,
    limit: usize,
    pretty: bool,
    base: FormIdBase,
) -> anyhow::Result<()> {
    let mut v = if let Some(sig) = record_type {
        backend.run(
            file,
            Op::ListTypeChildren(esm::ops::ListTypeChildrenArgs {
                sig: sig.to_string(),
                offset,
                limit,
            }),
        )?
    } else {
        backend.run(file, Op::ListGroups(esm::ops::NoArgs {}))?
    };
    convert_tree_json(&mut v, base);
    print_json(&v, pretty);
    Ok(())
}

pub(crate) fn cmd_coverage(
    backend: &mut Backend,
    file: &Path,
    record_type: Option<&str>,
    sample: usize,
    as_json: bool,
    gate: bool,
) -> anyhow::Result<()> {
    let v = backend.run(
        file,
        Op::Coverage(esm::ops::CoverageArgs {
            record_type: record_type.map(|s| s.to_string()),
            sample,
        }),
    )?;
    let report: CoverageReport = serde_json::from_value(v)?;

    if as_json {
        print_json(&serde_json::to_value(&report)?, true);
    } else {
        let mut rows: Vec<(&String, &Markers)> = report.by_type.iter().collect();
        rows.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(b.0)));

        let row = |sig: &str, m: &Markers| {
            println!(
                "{:<6}  {:>10}  {:>12}  {:>9}  {:>8}  {:>8}  {:>10}  {:>8}  {:>13}",
                sig,
                m.records,
                m.raw_fallback,
                m.malformed,
                m.trailing,
                m.unmapped,
                m.unresolved,
                m.unknown_record,
                m.unknown_bytes
            )
        };
        println!(
            "{:<6}  {:>10}  {:>12}  {:>9}  {:>8}  {:>8}  {:>10}  {:>8}  {:>13}",
            "SIG",
            "records",
            "raw_fallback",
            "malformed",
            "trailing",
            "unmapped",
            "unresolved",
            "unknown",
            "unknown_bytes"
        );
        println!("{}", "-".repeat(105));
        for (sig, m) in &rows {
            if m.total() > 0 || record_type.is_some() {
                row(sig, m);
            }
        }
        println!("{}", "-".repeat(105));
        let totals = &report.totals;
        row("TOTAL", totals);
        if totals.total() == 0 {
            println!("\n✓ Zero coverage markers — all records fully decoded.");
        }
    }

    // Gate on decode and schema coverage only — not `unresolved`, which indicates
    // missing localization BA2 strings rather than a decode failure.
    if gate {
        let totals = &report.totals;
        let mut failures = Vec::new();
        if totals.raw_fallback > 0 {
            failures.push(format!("{} raw_fallback", totals.raw_fallback));
        }
        if totals.malformed > 0 {
            failures.push(format!("{} malformed", totals.malformed));
        }
        if totals.trailing > 0 {
            failures.push(format!("{} trailing", totals.trailing));
        }
        if totals.unmapped > 0 {
            failures.push(format!("{} unmapped", totals.unmapped));
        }
        if totals.unknown_record > 0 {
            failures.push(format!("{} unknown_record", totals.unknown_record));
        }
        if !failures.is_empty() {
            anyhow::bail!("gate check failed: {}", failures.join(", "));
        }
    }
    Ok(())
}
