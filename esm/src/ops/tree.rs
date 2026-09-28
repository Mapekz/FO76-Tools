//! GRUP-tree browsing ops.

use super::NoArgs;
use crate::{Database, GroupChild, GroupNode};
use serde::{Deserialize, Serialize};

pub(super) fn list_groups(db: &Database, _args: &NoArgs) -> anyhow::Result<Vec<GroupNode>> {
    Ok(db.list_groups())
}

/// One page of the top-level group for record type `sig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ListTypeChildrenArgs {
    pub sig: String,
    pub offset: usize,
    pub limit: usize,
}

pub(super) fn list_type_children(
    db: &Database,
    args: &ListTypeChildrenArgs,
) -> anyhow::Result<Vec<GroupChild>> {
    db.list_type_children(&args.sig, args.offset, args.limit)
}

/// One page of the direct children of an arbitrary GRUP, by its own header
/// offset — see [`crate::Database::list_group_children`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ListGroupChildrenArgs {
    pub group_offset: u64,
    pub offset: usize,
    pub limit: usize,
}

pub(super) fn list_group_children(
    db: &Database,
    args: &ListGroupChildrenArgs,
) -> anyhow::Result<Vec<GroupChild>> {
    db.list_group_children(args.group_offset, args.offset, args.limit)
}

/// Lightweight record header at a file offset — see
/// [`crate::Database::record_stub_at`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecordStubAtArgs {
    pub offset: u64,
}

pub(super) fn record_stub_at(
    db: &Database,
    args: &RecordStubAtArgs,
) -> anyhow::Result<crate::tree::TreeRecordStub> {
    db.record_stub_at(args.offset)
}
