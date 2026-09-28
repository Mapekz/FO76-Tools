//! Every operation a surface can run against an open database: the [`Op`]
//! enum, one `Args` struct and one function per op (grouped by family in the
//! submodules), and [`run`], the dispatcher the CLI, `esm batch` and N-API all
//! reach through [`crate::host::Host::run`].
//!
//! Adding an op: write its `Args` struct and `fn(&Database, &Args) ->
//! Result<Output>` in the family module, then add one line to the `ops!`
//! list below. The wire enum, the dispatcher and the TypeScript `Op` and
//! `OpOutput` types all follow from that line.

mod analysis;
mod coverage;
mod diff;
mod records;
mod refs;
mod sel;
mod tree;

pub use analysis::{ChaseArgs, DropTableArgs, WalkArgs};
pub use coverage::{CoverageArgs, CoverageReport, Markers, coverage_report};
pub use diff::{DiffArgs, diff};
pub use records::{
    BulkRecordEntry, FilterTypeRecordsArgs, ListTypeFieldPathsArgs, ListTypeRecordsArgs,
    RawRecordView, RawSubrecordView, RecordArgs, RecordBulkArgs, RecordRawArgs, SearchArgs,
    raw_record_view,
};
pub use refs::{RefList, RefPathArgs, RefPathNode, RefRow, RefSort, ReferencedByArgs};
pub use sel::{RecordSel, resolve_sel};
pub use tree::{ListGroupChildrenArgs, ListTypeChildrenArgs, RecordStubAtArgs};

use crate::Database;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Default maximum recursion depth for the reverse-reference walk.
pub const DEFAULT_MAX_DEPTH: usize = 8;

/// How far a reverse-reference walk goes. On the wire it is a hop count,
/// with 0 meaning unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, type = "number"))]
pub enum RefDepth {
    /// Up to this many hops (as asked; walks clamp it to
    /// `1..=`[`DEFAULT_MAX_DEPTH`], so 0 is one hop).
    Hops(usize),
    /// No hop cap: can take minutes on hub-heavy graphs.
    Unbounded,
}

impl RefDepth {
    /// Direct references only.
    pub const DIRECT: RefDepth = RefDepth::Hops(1);

    pub fn from_hops(hops: usize) -> RefDepth {
        if hops == 0 {
            RefDepth::Unbounded
        } else {
            RefDepth::Hops(hops)
        }
    }

    /// The hop count as asked (0 = unbounded; `Hops(0)` asks for one hop).
    pub fn requested(self) -> usize {
        match self {
            RefDepth::Hops(n) => n.max(1),
            RefDepth::Unbounded => 0,
        }
    }

    /// The hop cap a walk uses: clamped to `1..=`[`DEFAULT_MAX_DEPTH`],
    /// `None` when unbounded.
    pub fn max_hops(self) -> Option<usize> {
        match self {
            RefDepth::Hops(n) => Some(n.clamp(1, DEFAULT_MAX_DEPTH)),
            RefDepth::Unbounded => None,
        }
    }
}

impl Default for RefDepth {
    fn default() -> Self {
        RefDepth::DIRECT
    }
}

impl Serialize for RefDepth {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.requested() as u64)
    }
}

impl<'de> Deserialize<'de> for RefDepth {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        usize::deserialize(d).map(RefDepth::from_hops)
    }
}

/// A request to execute one operation against an ESM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub esm: PathBuf,
    pub op: Op,
}

/// Success or error envelope `esm batch` answers each request with.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { data: Value },
    Err { error: String },
}

impl Response {
    pub fn from_result(result: anyhow::Result<Value>) -> Self {
        match result {
            Ok(data) => Response::Ok { data },
            Err(e) => Response::Err {
                error: format!("{:#}", e),
            },
        }
    }
}

/// The arguments of an op that takes none.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct NoArgs {}

/// Declares every op once: its wire tag (the `snake_case` of its variant),
/// its arguments, its output type, and the function that runs it. The CLI's
/// arguments are declared beside it, not derived from it (see
/// `docs/adr/0017-cli-arguments-are-declared-beside-the-registry.md`).
macro_rules! ops {
    ($( $tag:ident => $variant:ident($args:ty) -> $out:ty = $run:path; )*) => {
        /// Every operation routable through [`run`] / [`crate::host::Host::run`].
        /// On the wire, `{"op": "<tag>", ...args}`.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[cfg_attr(test, derive(ts_rs::TS))]
        #[cfg_attr(test, ts(export))]
        #[serde(tag = "op", rename_all = "snake_case")]
        pub enum Op {
            $( $variant($args), )*
        }

        impl Op {
            /// The wire tag, e.g. `"record_bulk"`.
            pub fn tag(&self) -> &'static str {
                match self {
                    $( Op::$variant(_) => stringify!($tag), )*
                }
            }
        }

        /// Run a single-database op and serialize its output. Each op's
        /// result is bound to its declared output type, so a declaration
        /// that disagrees with its function (and would export a wrong
        /// TypeScript `OpOutput`) doesn't compile.
        pub fn run(db: &Database, op: &Op) -> anyhow::Result<Value> {
            match op {
                $( Op::$variant(args) => {
                    let out: $out = $run(db, args)?;
                    Ok(serde_json::to_value(out)?)
                } )*
            }
        }

        /// Each op's output type, keyed by wire tag — exported to TypeScript
        /// as `OpOutput` so a caller of `run(op)` gets its result type.
        #[cfg(test)]
        #[derive(ts_rs::TS)]
        #[ts(export)]
        #[allow(dead_code)]
        struct OpOutput {
            $( $tag: $out, )*
        }

        #[cfg(test)]
        const TAGS: &[&str] = &[$( stringify!($tag), )*];
    };
}

ops! {
    file_info => FileInfo(NoArgs) -> crate::reader::FileInfo = records::file_info;
    record => Record(RecordArgs) -> crate::RecordResult = records::record;
    record_bulk => RecordBulk(RecordBulkArgs) -> Vec<BulkRecordEntry> = records::record_bulk;
    record_raw => RecordRaw(RecordRawArgs) -> RawRecordView = records::record_raw;
    list_type_records => ListTypeRecords(ListTypeRecordsArgs) -> Vec<crate::RecordRow> = records::list_type_records;
    filter_type_records => FilterTypeRecords(FilterTypeRecordsArgs) -> crate::FilterResult = records::filter_type_records;
    list_type_field_paths => ListTypeFieldPaths(ListTypeFieldPathsArgs) -> Vec<String> = records::list_type_field_paths;
    search => Search(SearchArgs) -> Vec<crate::RecordRow> = records::search;
    referenced_by => ReferencedBy(ReferencedByArgs) -> RefList = refs::referenced_by;
    ref_path => RefPath(RefPathArgs) -> crate::refs::RefPathResult = refs::ref_path;
    walk => Walk(WalkArgs) -> crate::walk::WalkResult = analysis::walk;
    chase => Chase(ChaseArgs) -> crate::chase::ChaseTree = analysis::chase;
    drop_table => DropTable(DropTableArgs) -> crate::lvli::DropTable = analysis::drop_table;
    list_groups => ListGroups(NoArgs) -> Vec<crate::GroupNode> = tree::list_groups;
    list_type_children => ListTypeChildren(ListTypeChildrenArgs) -> Vec<crate::GroupChild> = tree::list_type_children;
    list_group_children => ListGroupChildren(ListGroupChildrenArgs) -> Vec<crate::GroupChild> = tree::list_group_children;
    record_stub_at => RecordStubAt(RecordStubAtArgs) -> crate::tree::TreeRecordStub = tree::record_stub_at;
    coverage => Coverage(CoverageArgs) -> CoverageReport = coverage::coverage;
    diff => Diff(DiffArgs) -> crate::diff::DiffResult = diff::needs_two_databases;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each `ops!` tag must be exactly the serde tag of its variant.
    #[test]
    fn every_tag_is_the_serde_tag_of_its_variant() {
        for tag in TAGS {
            let err = serde_json::from_value::<Op>(serde_json::json!({ "op": tag }))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(
                !err.contains("unknown variant"),
                "`{tag}` is not a serde tag of Op: {err}"
            );
        }
        let file_info =
            serde_json::from_value::<Op>(serde_json::json!({"op": "file_info"})).unwrap();
        assert_eq!(file_info.tag(), "file_info");
        assert_eq!(
            serde_json::to_value(&file_info).unwrap(),
            serde_json::json!({"op": "file_info"})
        );
    }
}
