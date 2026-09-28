//! Schema decode coverage: marker counts per record type.

use crate::Database;
use crate::decode::node::{Node, RawReason};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Decode every record (or a sample of each type) and count decode markers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct CoverageArgs {
    pub record_type: Option<String>,
    pub sample: usize,
}

pub(super) fn coverage(db: &Database, args: &CoverageArgs) -> anyhow::Result<CoverageReport> {
    coverage_report(db, args.record_type.as_deref(), args.sample)
}

/// Counts of schema-coverage markers per record type.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Markers {
    pub unknown_record: u64,
    /// `_raw` values the schema couldn't decode: members the extractor
    /// couldn't model, and unions with no variant for their bytes.
    pub raw_fallback: u64,
    /// `_raw` values whose bytes don't fit their declared layout (truncated
    /// VMAD, short CTDA, inconsistent Model Information).
    pub malformed: u64,
    /// `_trailing` markers: struct subrecords with bytes left after their
    /// fields.
    pub trailing: u64,
    pub unmapped: u64,
    pub unresolved: u64,
    /// `_raw` values the schema itself declares unknown (xEdit `wbUnknown`).
    /// Informational: not a coverage gap.
    pub unknown_bytes: u64,
    pub records: u64,
}

impl Markers {
    /// Every marker that means the decode is incomplete (all but
    /// `unknown_bytes`).
    pub fn total(&self) -> u64 {
        self.unknown_record
            + self.raw_fallback
            + self.malformed
            + self.trailing
            + self.unmapped
            + self.unresolved
    }

    pub fn add(&mut self, other: &Markers) {
        self.unknown_record += other.unknown_record;
        self.raw_fallback += other.raw_fallback;
        self.malformed += other.malformed;
        self.trailing += other.trailing;
        self.unmapped += other.unmapped;
        self.unresolved += other.unresolved;
        self.unknown_bytes += other.unknown_bytes;
        self.records += other.records;
    }
}

/// Coverage audit report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct CoverageReport {
    pub by_type: BTreeMap<String, Markers>,
    pub totals: Markers,
}

/// Count the markers in a record's decoded tree.
fn count_markers(db: &Database, node: &Node, m: &mut Markers) {
    use crate::decode::markers;
    match node {
        Node::Struct(fields) => {
            for (key, child) in fields {
                if key == markers::UNKNOWN_RECORD {
                    m.unknown_record += 1;
                } else if key == markers::UNMAPPED {
                    if let Node::Struct(by_sig) = child {
                        m.unmapped += by_sig
                            .values()
                            .filter_map(Node::as_array)
                            .map(|subs| subs.len() as u64)
                            .sum::<u64>();
                    }
                } else {
                    count_markers(db, child, m);
                }
            }
        }
        Node::Array(items) => items.iter().for_each(|item| count_markers(db, item, m)),
        Node::Raw { reason, .. } => match reason {
            RawReason::Unknown => m.unknown_bytes += 1,
            RawReason::Unmodelled(_) | RawReason::UnresolvedUnion => m.raw_fallback += 1,
            RawReason::Malformed(_) => m.malformed += 1,
            RawReason::Trailing => m.trailing += 1,
        },
        Node::LString { id, kind } => {
            let text = db
                .localization
                .as_ref()
                .and_then(|loc| loc.lookup(*kind, *id));
            if text.is_none() {
                m.unresolved += 1;
            }
        }
        _ => {}
    }
}

pub fn coverage_report(
    db: &Database,
    record_type: Option<&str>,
    sample: usize,
) -> anyhow::Result<CoverageReport> {
    // `signatures()` iterates the pre-built type_index's keys — already the
    // distinct set of signatures present, no HashSet dedup needed and no
    // 5.64M-record form_index scan (down from that to 178 entries).
    let mut all_sigs: Vec<String> = db
        .index
        .signatures()
        .map(|s| s.as_str().to_owned())
        .collect();
    all_sigs.sort();

    if let Some(rt) = record_type {
        let rt_upper = rt.to_uppercase();
        all_sigs.retain(|s| *s == rt_upper);
        if all_sigs.is_empty() {
            bail!("no records of type '{}' found", rt);
        }
    }

    let mut by_type: BTreeMap<String, Markers> = BTreeMap::new();

    for sig in &all_sigs {
        let metas: Vec<crate::reader::RecordMeta> = db
            .index
            .records_by_type(sig)
            .map(|(_, m)| m)
            .take(if sample == 0 { usize::MAX } else { sample })
            .collect();

        let mut type_markers = Markers::default();
        for meta in &metas {
            match db.record_node_at_meta(meta) {
                Ok((_, node)) => {
                    type_markers.records += 1;
                    count_markers(db, &node, &mut type_markers);
                }
                Err(e) => {
                    eprintln!("Warning: failed to decode {} record: {}", sig, e);
                }
            }
        }
        by_type.insert(sig.clone(), type_markers);
    }

    let totals = by_type.values().fold(Markers::default(), |mut acc, m| {
        acc.add(m);
        acc
    });

    Ok(CoverageReport { by_type, totals })
}
