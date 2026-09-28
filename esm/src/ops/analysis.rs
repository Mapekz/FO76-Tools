//! The mechanism digests: `walk`, `chase` and LVLI drop tables.

use super::records::{bulk_record_entry, record_resolved};
use super::{BulkRecordEntry, RecordSel, RefDepth, RefList, RefSort};
use crate::{Database, FormId, ResolveDepth, SearchField};
use anyhow::bail;
use serde::{Deserialize, Serialize};

/// Interactive record digest — the op behind `esm walk` (see
/// [`crate::walk`]). The BFS and per-node digest computation run entirely
/// in-process, one request regardless of how many nodes the walk visits.
/// `want_refs` mirrors the CLI's `--refs` flag: when true and the root
/// resolved, one extra unfiltered reverse-reference walk runs on the root and
/// is folded into [`crate::walk::WalkResult::refs`]. When the root selector
/// doesn't resolve, `not_found.matches` is filled in by one in-process
/// [`Database::search`] call (see
/// `docs/adr/0001-walk-interactive-chase-pipeline-json.md`'s section on where
/// the computation runs).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct WalkArgs {
    pub sel: RecordSel,
    /// `None` = [`crate::walk::default_depth`] for the root's type.
    #[serde(default)]
    pub depth: Option<usize>,
    pub ref_limit: usize,
    pub level: f32,
    pub want_refs: bool,
}

pub(super) fn walk(db: &Database, args: &WalkArgs) -> anyhow::Result<crate::walk::WalkResult> {
    let opts = crate::walk::WalkOptions {
        depth: args.depth,
        ref_limit: args.ref_limit,
        level: args.level,
    };
    let mut result = crate::walk::walk(&mut DbSource { db }, args.sel.clone(), &opts)?;
    if let Some(nf) = result.not_found.as_mut() {
        nf.matches = db.search(&nf.target, &[], SearchField::Both, 10)?;
    } else if args.want_refs
        && let Some(root) = result.nodes.first()
    {
        let root_fid = crate::parse_form_id_input(&root.formid)?;
        let ref_list = crate::refs::referenced_by_enriched(
            db,
            root_fid,
            RefDepth::DIRECT,
            0,
            None,
            false,
            RefSort::Formid,
        )?;
        result.refs = Some(crate::walk::build_refs_digest(&ref_list.rows));
    }
    Ok(result)
}

/// Pipeline evidence contract — the op behind `esm chase` (see
/// [`crate::chase`]). Always emits the classified `ChaseTree`; hard-errors on
/// a selector that doesn't resolve to one of the five accepted root types
/// (OMOD/PERK/SPEL/ALCH/ENCH).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ChaseArgs {
    pub sel: RecordSel,
    pub depth: RefDepth,
    pub ref_limit: usize,
}

pub(super) fn chase(db: &Database, args: &ChaseArgs) -> anyhow::Result<crate::chase::ChaseTree> {
    // Each keyword's consumers are one reverse walk; an unbounded one per
    // keyword is never what a chase wants.
    if args.depth == RefDepth::Unbounded {
        bail!(
            "chase depth must be 1..={}; only refs walks unbounded",
            crate::ops::DEFAULT_MAX_DEPTH
        );
    }
    let opts = crate::chase::ChaseOptions {
        depth: args.depth,
        ref_limit: args.ref_limit,
    };
    crate::chase::chase(&mut DbSource { db }, args.sel.clone(), &opts)
}

/// LVLI drop-probability table — the `Op` form of
/// [`crate::lvli::drop_table`], reachable standalone (not only via
/// `Op::Walk`'s LVLI digest). Hard-errors on a selector that doesn't resolve
/// to an LVLI record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct DropTableArgs {
    pub sel: RecordSel,
    pub level: f32,
    pub max_depth: usize,
    pub strict: bool,
}

pub(super) fn drop_table(
    db: &Database,
    args: &DropTableArgs,
) -> anyhow::Result<crate::lvli::DropTable> {
    let result = record_resolved(db, &args.sel, ResolveDepth::Stub)?;
    if result.header.signature != "LVLI" {
        bail!(
            "{:?} resolves to a {:?} record — drop-table only supports LVLI selectors",
            args.sel.display(),
            result.header.signature
        );
    }
    let opts = crate::lvli::DropOptions {
        level: args.level,
        max_depth: args.max_depth,
        strict: args.strict,
        tree_depth: 0,
    };
    crate::lvli::drop_table(
        &mut DbSource { db },
        result.header.form_id,
        &result.fields,
        &opts,
    )
}

/// [`crate::source::RecordSource`] over an open `Database`, so the walk,
/// chase and drop-table traversals fetch records and reverse references
/// directly.
struct DbSource<'a> {
    db: &'a Database,
}

impl crate::source::RecordSource for DbSource<'_> {
    fn bulk_get(
        &mut self,
        sels: &[RecordSel],
        depth: ResolveDepth,
    ) -> anyhow::Result<Vec<BulkRecordEntry>> {
        Ok(sels
            .iter()
            .map(|sel| bulk_record_entry(self.db, sel, depth))
            .collect())
    }

    fn refs(
        &mut self,
        target: FormId,
        depth: RefDepth,
        limit: usize,
        type_filter: &str,
        paths: bool,
    ) -> anyhow::Result<RefList> {
        crate::refs::referenced_by_enriched(
            self.db,
            target,
            depth,
            limit,
            Some(type_filter),
            paths,
            RefSort::Formid,
        )
    }
}
