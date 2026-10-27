//! Resource-collector capacity checks for `walk`'s CONT and LVLI digests.
//!
//! A workshop collector is three records joined only by reverse references:
//! the CONT lists a resource AVIF in `Properties[]` beside its `CarryWeight`
//! cap, and a RESO names the same AVIF plus the LVLI it `Produce`s. Neither
//! end references the other, so a forward walk never pairs the cap with the
//! output. [`checks_for_container`] starts from the CONT and
//! [`checks_for_produce`] from the produced LVLI; both find the other end
//! with type-filtered `refs`, weigh every leaf item the list can yield in one
//! bulk fetch, and report the items heavier than the cap: production the
//! container can never store.
//!
//! An item's weight is its base record's (`Data`/`DATA`/`Armor Data`/
//! `Attributes`/top-level `Weight`); weapon mods attached at spawn aren't
//! counted.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::lvli::{DropOptions, DropRow, drop_table};
use crate::ops::RefDepth;
use crate::source::{RecordSource, bulk_fetch_map};
use crate::{FormId, Resolved};

/// AVIF `CarryWeight`: on a collector CONT, the most weight it stores.
const CARRY_WEIGHT_AV: FormId = FormId(0x0000_02DC);

/// One collector's production line, checked against its container's cap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct CollectorCheck {
    /// The collector CONT, as a `{formid, editor_id, record_type}` stub.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub container: Value,
    /// The CONT's `CarryWeight` property, in pounds.
    pub capacity: f64,
    /// The RESO pairing the container's resource AVIF with its list.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub resource: Value,
    /// The LVLI the RESO produces.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub produce: Value,
    /// `produce`, typed, for in-process consumers. Not serialized.
    #[serde(skip)]
    #[cfg_attr(test, ts(skip))]
    pub produce_id: FormId,
    /// The RESO's `Interval` GLOB stub (hours, inlined as `Value`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(type = "unknown"))]
    pub interval: Option<Value>,
    /// Distinct leaf items the list can yield.
    pub items: usize,
    /// Leaf items with no weight field (unresolved or weightless types).
    pub unweighed: usize,
    /// Items heavier than `capacity`, heaviest first.
    pub over_capacity: Vec<ProducedItem>,
}

/// A produced leaf item and its weight.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ProducedItem {
    pub formid: String,
    pub editor_id: String,
    pub record_type: String,
    pub weight: f64,
    /// Chance one production roll yields this item.
    pub p_at_least_one: f64,
}

/// Checks for a collector CONT: none unless it has a `CarryWeight` cap and
/// a RESO produces from one of its other property AVIFs. Two RESOs
/// producing the same list give one check.
pub(super) fn checks_for_container(
    f: &mut impl RecordSource,
    formid: FormId,
    editor_id: &str,
    fields: &Resolved,
    level: f32,
) -> anyhow::Result<Vec<CollectorCheck>> {
    let Some(capacity) = carry_weight(fields) else {
        return Ok(Vec::new());
    };
    let container = stub(formid, editor_id, "CONT");
    let mut checks: Vec<CollectorCheck> = Vec::new();
    for av in resource_avs(fields) {
        for reso in referencers(f, av, "RESO")? {
            let Some(reso_rec) = fetch(f, reso)? else {
                continue;
            };
            let Some(produce) = reso_rec.fields.get("Produce").and_then(Resolved::stub_id) else {
                continue;
            };
            if checks.iter().any(|c| c.produce_id == produce) {
                continue;
            }
            let Some(list) = fetch(f, produce)? else {
                continue;
            };
            let opts = DropOptions {
                level,
                ..Default::default()
            };
            let table = drop_table(f, produce, &list.fields, &opts)?;
            let weighed = weigh(f, &table.rows)?;
            checks.push(check(
                &container,
                capacity,
                &reso_rec,
                (produce, &list.editor_id),
                &weighed,
            ));
        }
    }
    Ok(checks)
}

/// Checks for an LVLI a RESO produces directly, against every CONT whose
/// `Properties[]` hold the RESO's resource AVIF, one per CONT. `rows` is
/// the list's already-computed drop table. A sublist of that list gets no
/// check.
pub(super) fn checks_for_produce(
    f: &mut impl RecordSource,
    formid: FormId,
    editor_id: &str,
    rows: &[DropRow],
) -> anyhow::Result<Vec<CollectorCheck>> {
    let resos = referencers(f, formid, "RESO")?;
    if resos.is_empty() {
        return Ok(Vec::new());
    }
    let mut weighed = None;
    let mut checks: Vec<CollectorCheck> = Vec::new();
    for reso in resos {
        let Some(reso_rec) = fetch(f, reso)? else {
            continue;
        };
        if reso_rec.fields.get("Produce").and_then(Resolved::ref_id) != Some(formid) {
            continue;
        }
        let Some(av) = reso_rec
            .fields
            .get("Actor Value")
            .and_then(Resolved::ref_id)
        else {
            continue;
        };
        let conts = referencers(f, av, "CONT")?;
        let records = bulk_fetch_map(f, &conts)?;
        for cont in conts {
            let Some(cont_fields) = records.get(&cont).and_then(|r| r.fields.as_ref()) else {
                continue;
            };
            let Some(capacity) = carry_weight(cont_fields) else {
                continue;
            };
            let container = stub(
                cont,
                records[&cont].editor_id.as_deref().unwrap_or_default(),
                "CONT",
            );
            if !resource_avs(cont_fields).contains(&av)
                || checks.iter().any(|c| c.container == container)
            {
                continue;
            }
            if weighed.is_none() {
                weighed = Some(weigh(f, rows)?);
            }
            checks.push(check(
                &container,
                capacity,
                &reso_rec,
                (formid, editor_id),
                weighed.as_deref().unwrap_or_default(),
            ));
        }
    }
    Ok(checks)
}

/// The AVIFs a CONT's `Properties[]` name, other than `CarryWeight`.
fn resource_avs(fields: &Resolved) -> Vec<FormId> {
    let mut avs: Vec<FormId> = fields
        .get("Properties")
        .map(Resolved::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.get("Actor Value").and_then(Resolved::ref_id))
        .filter(|av| *av != CARRY_WEIGHT_AV)
        .collect();
    crate::fields::dedup_sorted(&mut avs);
    avs
}

/// A drop row with its leaf record's weight, `None` when it has none.
type Weighed = (DropRow, Option<f64>);

fn check(
    container: &Value,
    capacity: f64,
    reso: &Fetched,
    (produce_id, produce_edid): (FormId, &str),
    weighed: &[Weighed],
) -> CollectorCheck {
    let mut over_capacity: Vec<ProducedItem> = weighed
        .iter()
        .filter_map(|(row, w)| w.filter(|w| *w > capacity).map(|w| (row, w)))
        .map(|(row, weight)| ProducedItem {
            formid: row.formid.clone(),
            editor_id: row.editor_id.clone(),
            record_type: row.record_type.clone(),
            weight,
            p_at_least_one: row.p_at_least_one,
        })
        .collect();
    over_capacity.sort_by(|a, b| {
        b.weight
            .total_cmp(&a.weight)
            .then_with(|| a.formid.cmp(&b.formid))
    });
    CollectorCheck {
        container: container.clone(),
        capacity,
        resource: stub(reso.id, &reso.editor_id, "RESO"),
        produce: stub(produce_id, produce_edid, "LVLI"),
        produce_id,
        interval: reso
            .fields
            .get("Interval")
            .filter(|v| v.is_stub())
            .map(Resolved::to_json),
        items: weighed.len(),
        unweighed: weighed.iter().filter(|(_, w)| w.is_none()).count(),
        over_capacity,
    }
}

/// Fetch every leaf row's record in one bulk call and read its weight.
fn weigh(f: &mut impl RecordSource, rows: &[DropRow]) -> anyhow::Result<Vec<Weighed>> {
    let ids: Vec<Option<FormId>> = rows
        .iter()
        .map(|r| crate::formid::parse_formid(&r.formid).ok())
        .collect();
    let mut fids: Vec<FormId> = ids.iter().flatten().copied().collect();
    crate::fields::dedup_sorted(&mut fids);
    let records = bulk_fetch_map(f, &fids)?;
    Ok(rows
        .iter()
        .zip(ids)
        .map(|(row, id)| {
            let weight = id
                .and_then(|id| records.get(&id))
                .and_then(|r| r.fields.as_ref())
                .and_then(item_weight);
            (row.clone(), weight)
        })
        .collect())
}

/// A record's base weight: a top-level `Weight`, else the first struct
/// field holding one (`Data`, `DATA`, `Armor Data`, `Attributes`).
fn item_weight(fields: &Resolved) -> Option<f64> {
    fields.get("Weight").and_then(Resolved::as_f64).or_else(|| {
        fields
            .as_object()?
            .values()
            .find_map(|v| v.get("Weight").and_then(Resolved::as_f64))
    })
}

/// The CONT's `CarryWeight` cap. A 0 cap is unset, not "stores nothing":
/// the live Mount Blair ore extractor (0x003CA905) has one.
fn carry_weight(fields: &Resolved) -> Option<f64> {
    fields
        .get("Properties")?
        .items()
        .iter()
        .find(|p| p.get("Actor Value").and_then(Resolved::ref_id) == Some(CARRY_WEIGHT_AV))
        .and_then(|p| p.get("Value"))
        .and_then(Resolved::as_f64)
        .filter(|cap| *cap > 0.0)
}

/// The FormIDs of `type_filter` records directly referencing `target`.
fn referencers(
    f: &mut impl RecordSource,
    target: FormId,
    type_filter: &str,
) -> anyhow::Result<Vec<FormId>> {
    Ok(f.refs(target, RefDepth::DIRECT, 0, type_filter, false)?
        .rows
        .into_iter()
        .map(|r| r.id)
        .collect())
}

struct Fetched {
    id: FormId,
    editor_id: String,
    fields: Resolved,
}

fn fetch(f: &mut impl RecordSource, id: FormId) -> anyhow::Result<Option<Fetched>> {
    let mut map: HashMap<FormId, _> = bulk_fetch_map(f, &[id])?;
    Ok(map.remove(&id).and_then(|r| {
        Some(Fetched {
            id,
            editor_id: r.editor_id.unwrap_or_default(),
            fields: r.fields?,
        })
    }))
}

fn stub(id: FormId, editor_id: &str, record_type: &str) -> Value {
    json!({"formid": id.display(), "editor_id": editor_id, "record_type": record_type})
}
