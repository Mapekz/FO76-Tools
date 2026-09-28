//! Schema decode coverage: marker counts per record type.

use crate::{Database, ResolveDepth};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    pub raw_fallback: u64,
    pub unmapped: u64,
    pub unresolved: u64,
    pub records: u64,
}

impl Markers {
    pub fn total(&self) -> u64 {
        self.unknown_record + self.raw_fallback + self.unmapped + self.unresolved
    }

    pub fn add(&mut self, other: &Markers) {
        self.unknown_record += other.unknown_record;
        self.raw_fallback += other.raw_fallback;
        self.unmapped += other.unmapped;
        self.unresolved += other.unresolved;
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

fn count_markers(v: &Value, m: &mut Markers) {
    use crate::decode::markers;
    match v {
        Value::Object(obj) => {
            if obj.get(markers::UNKNOWN_RECORD) == Some(&Value::Bool(true)) {
                m.unknown_record += 1;
            }
            if obj.get(markers::RAW) == Some(&Value::Bool(true)) && obj.contains_key("reason") {
                m.raw_fallback += 1;
            }
            if obj.get(markers::UNRESOLVED) == Some(&Value::Bool(true)) {
                m.unresolved += 1;
            }
            if let Some(Value::Object(unmapped)) = obj.get(markers::UNMAPPED) {
                for subs in unmapped.values() {
                    if let Value::Array(arr) = subs {
                        m.unmapped += arr.len() as u64;
                    }
                }
            }
            for (key, child) in obj {
                if key == markers::UNMAPPED {
                    continue;
                }
                count_markers(child, m);
            }
        }
        Value::Array(arr) => {
            for child in arr {
                count_markers(child, m);
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
            match db.record_at_meta_with_depth(meta, ResolveDepth::None) {
                Ok(result) => {
                    type_markers.records += 1;
                    let mut rec_markers = Markers::default();
                    count_markers(&result.fields, &mut rec_markers);
                    type_markers.add(&rec_markers);
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
