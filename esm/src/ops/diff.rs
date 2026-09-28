//! The two-database `diff` op.

use crate::Database;
use crate::diff::{DiffOptions, DiffResult, diff_databases_with};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Diff this ESM against the one at `b`. Needs two databases, so it runs
/// through [`crate::host::Host::run`] (or [`diff`] directly) rather than the
/// single-database dispatcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct DiffArgs {
    pub b: PathBuf,
    pub record_type: Option<String>,
    /// Body-detail / noise-suppression / type-exclusion controls (see
    /// [`DiffOptions`]).
    #[serde(default)]
    pub options: DiffOptions,
}

/// Diff two open databases, limited to `record_type` when set — the body of
/// `Op::Diff`, shared by [`crate::host::Host::run`], the CLI's
/// source-override path and the N-API binding.
pub fn diff(db_a: &Database, db_b: &Database, args: &DiffArgs) -> anyhow::Result<DiffResult> {
    let mut options = args.options.clone();
    if args.record_type.is_some() {
        options.only_type = args.record_type.clone();
    }
    diff_databases_with(db_a, db_b, &options)
}

pub(super) fn needs_two_databases(_db: &Database, _args: &DiffArgs) -> anyhow::Result<DiffResult> {
    anyhow::bail!("diff needs two ESMs; run it through `Host::run`")
}
