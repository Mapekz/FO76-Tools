//! `curve` subcommand handler — ad-hoc lookup/sum over any `CURV` (Curve
//! Table) record, by editor ID or FormID.
//!
//! The actual result-construction logic lives in the library
//! (`esm::curves::curve_query`), so any surface can call it. This module is a
//! thin CLI wrapper: build selectors from `targets` (mirroring `query::cmd_get`'s
//! auto-detect-FormID-vs-EditorID convention), fetch them in one
//! `Op::RecordBulk` round-trip regardless of target count (CURV points are
//! already inlined into every `Op::Record`/`RecordBulk` response at every
//! resolve depth — no dedicated `Op` needed), call the shared library
//! function, then print.

use esm::curves::curve_query;
use esm::ipc::{BulkRecordEntry, Op, RecordSel};
use esm::{FormIdBase, ResolveDepth};
use std::path::Path;

use crate::Backend;
use crate::output::print_json;

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_curve(
    backend: &mut Backend,
    file: &Path,
    targets: Vec<String>,
    at: Vec<f32>,
    sum: Option<Vec<f32>>,
    step: f32,
    json: bool,
    pretty: bool,
    base: FormIdBase,
) -> anyhow::Result<()> {
    let sels: Vec<RecordSel> = targets
        .iter()
        .map(|t| RecordSel::from_input_with(t, base))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let v = backend.run(
        file,
        Op::RecordBulk {
            sels,
            depth: ResolveDepth::None,
        },
    )?;
    let entries: Vec<BulkRecordEntry> = serde_json::from_value(v)?;

    // clap's `conflicts_with`/`requires` already enforce --at/--sum mutual
    // exclusion and --step requiring --sum for this binary; `curve_query`
    // re-checks --at/--sum itself for a library caller with no clap.
    let sum = match sum {
        Some(pair) if pair.len() == 2 => Some((pair[0], pair[1])),
        Some(_) => anyhow::bail!("--sum takes exactly two values: FROM TO"),
        None => None,
    };

    let result = curve_query(&entries, &at, sum, step);
    print_json(&result, pretty || !json);
    Ok(())
}
