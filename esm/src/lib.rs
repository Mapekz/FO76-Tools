// Unsafe code is limited to the functions that allow it: memory maps and
// rkyv access over cache sections.
#![deny(unsafe_code)]

pub mod ba2;
pub mod chase;
pub mod compress;
pub mod ctda;
pub mod curves;
pub mod database;
pub mod decode;
pub mod diff;
pub mod discover;
pub(crate) mod fields;
pub mod filter;
pub mod format;
pub mod formid;
pub mod hardcoded;
pub mod host;
pub mod index;
pub mod logging;
pub mod lvli;
pub mod ops;
pub mod progress;
pub mod query;
pub mod reader;
pub mod refs;
mod rkyvcache;
pub mod schema;
pub mod source;
pub mod strings;
#[cfg(test)]
pub(crate) mod testkit;
pub mod tree;
pub mod walk;
pub mod wildcard;

pub use decode::{FormIdRefResolver, FormIdStub, ResolveDepth};
pub use diff::{BodyDetail, DiffOptions, DiffResult, RecordDiff, RecordStub, RefName};
pub use formid::{FormId, FormIdBase};
pub use index::{CacheInventory, SearchMeta, cache_inventory};
pub use ops::{
    BulkRecordEntry, CoverageReport, Markers, Op, RawRecordView, RawSubrecordView, RefList,
    RefPathNode, RefRow, Request, Response,
};
pub use reader::RecordMeta;

// Re-export tree types.
pub use tree::{GroupChild, GroupLabel, GroupNode, TreeIndex, TreeRecordStub};

pub use database::{Database, DatabaseResolver, ListEntry, RecordResult, RecordRow, SearchField};
pub use filter::{FilterOp, FilterResult};
pub use formid::{looks_like_formid, parse_form_id_input};
pub use refs::seeds::{
    CarrierKind, CarrierTag, Carriers, EntryPointSpec, OmodPropertySpec, PropScope,
};
