//! Curve table resolution for Fallout 76 CURV records.
//!
//! Builds an index from CURV FormID → parsed curve points by reading
//! CURV records from an ESM and loading the JSON point data from the
//! Startup BA2 archive.

use crate::discover::CurvesSrc;
use crate::rkyvcache::{ArchiveBuf, SectionKind, SectionSpec};
use crate::{ba2::Ba2Archive, formid::FormId, reader::EsmFile};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single point in a curve table.
#[derive(Debug, Clone, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
}

/// A parsed curve table: EditorID, source path, and interpolatable points.
#[derive(Debug, Clone, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize)]
pub struct Curve {
    pub edid: Option<String>,
    pub path: String,
    pub points: Vec<CurvePoint>,
}

impl Curve {
    /// Linearly interpolate y at x, clamped to the curve's domain.
    /// Returns None only for an empty curve.
    pub fn eval(&self, x: f32) -> Option<f32> {
        eval(&self.points, x)
    }
}

/// Linear interpolation of y at x over sorted curve points.
///
/// Mirrors `interpolate` in `tools/curvelib.py` exactly: any change to
/// clamping/edge-case semantics here should be ported there too.
pub fn eval(points: &[CurvePoint], x: f32) -> Option<f32> {
    if points.is_empty() {
        return None;
    }
    if x <= points[0].x {
        return Some(points[0].y);
    }
    let last = points.last().unwrap();
    if x >= last.x {
        return Some(last.y);
    }
    // Find bracketing pair
    for w in points.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if x >= a.x && x <= b.x {
            if (b.x - a.x).abs() < f32::EPSILON {
                return Some(a.y);
            }
            let t = (x - a.x) / (b.x - a.x);
            return Some(a.y + t * (b.y - a.y));
        }
    }
    Some(last.y)
}

/// Extract curve points from an already-decoded JSON value, handling both
/// shapes produced elsewhere in this codebase: a CURV record's own decoded
/// fields, which carry a top-level `"Curve"` key (see
/// `Database::record_at_meta_with_depth`'s CURV-record inline), and a
/// resolved curve-reference stub, which carries a lowercase `"curve"` key
/// (see `decode::resolve_formid`'s `InlineSource::CurveIndex` branch).
///
/// Returns `None` only when *neither* key is present — the signal callers
/// use to distinguish "curve tables not loaded" from "loaded, but genuinely
/// zero points" (`Some(vec![])`, when the key is present but its array is
/// empty). Points are **not** re-sorted here: they are already sorted by the
/// time they reach this JSON (`parse_curve_json` sorts at index-build time),
/// so re-sorting here would silently mask a real upstream regression instead
/// of surfacing it.
pub fn points_from_json(v: &serde_json::Value) -> Option<Vec<CurvePoint>> {
    let raw = v.get("Curve").or_else(|| v.get("curve"))?;
    let points = raw
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let x = p.get("x").and_then(serde_json::Value::as_f64)? as f32;
                    let y = p.get("y").and_then(serde_json::Value::as_f64)? as f32;
                    Some(CurvePoint { x, y })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(points)
}

/// Sum [`eval`] over x stepping from `start` to `end` inclusive.
///
/// Steps are *counted* (`n = round((end - start) / step)`) rather than accumulated in
/// a `while x <= end` loop, so float drift can't cause the loop to overshoot
/// or miss the last point. Accumulates in `f64` for precision on large sums.
pub fn sum_range(points: &[CurvePoint], start: f32, end: f32, step: f32) -> Result<f64> {
    if step <= 0.0 || !step.is_finite() {
        anyhow::bail!("step must be > 0 and finite, got {step}");
    }
    if end < start {
        anyhow::bail!("end ({end}) must be >= start ({start})");
    }

    let n = ((end - start) / step).round() as i64;
    let mut total = 0.0_f64;
    for i in 0..=n {
        let x = start + i as f32 * step;
        if let Some(y) = eval(points, x) {
            total += f64::from(y);
        }
    }
    Ok(total)
}

impl ArchivedCurve {
    pub fn edid(&self) -> Option<&str> {
        self.edid.as_ref().map(|s| s.as_str())
    }

    pub fn path(&self) -> &str {
        self.path.as_str()
    }

    pub fn points(&self) -> Vec<CurvePoint> {
        self.points
            .iter()
            .map(|p| CurvePoint {
                x: p.x.to_native(),
                y: p.y.to_native(),
            })
            .collect()
    }

    pub fn eval(&self, x: f32) -> Option<f32> {
        eval(&self.points(), x)
    }
}

/// Every CURV record's curve, sorted by FormID, plus a stamp of the curve
/// files it was read from — the `curves` cache section.
#[derive(rkyv::Archive, rkyv::Serialize)]
pub(crate) struct CurvesSection {
    source: u64,
    ids: Vec<u32>,
    curves: Vec<Curve>,
}

impl CurvesSection {
    fn new(source: u64, mut entries: Vec<(u32, Curve)>) -> Self {
        entries.sort_by_key(|(id, _)| *id);
        entries.dedup_by_key(|(id, _)| *id);
        let (ids, curves) = entries.into_iter().unzip();
        CurvesSection {
            source,
            ids,
            curves,
        }
    }
}

/// Version of this section's archived layout, stored in its file header so
/// a cache written by a build with another layout is rebuilt. Bump it when
/// the layout golden test in this module fails.
const CURVES_LAYOUT_FINGERPRINT: u64 = 1;

impl SectionSpec for ArchivedCurvesSection {
    const KIND: SectionKind = SectionKind::Curves;
    const LAYOUT_FINGERPRINT: u64 = CURVES_LAYOUT_FINGERPRINT;
}

/// Index of CURV FormID → parsed curve.
pub struct CurveIndex {
    section: ArchiveBuf<ArchivedCurvesSection>,
}

impl std::fmt::Debug for CurveIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurveIndex")
            .field("curves", &self.section.get().ids.len())
            .finish()
    }
}

impl CurveIndex {
    /// Look up a curve by FormID.
    pub fn get(&self, id: FormId) -> Option<&ArchivedCurve> {
        let section = self.section.get();
        let i = section
            .ids
            .binary_search_by_key(&id.raw(), |x| x.to_native())
            .ok()?;
        section.curves.get(i)
    }

    /// Build an index in memory from `(form_id, Curve)` pairs, with no ESM or
    /// curve files involved.
    pub fn from_curves(curves: impl IntoIterator<Item = (FormId, Curve)>) -> Result<CurveIndex> {
        let entries = curves.into_iter().map(|(id, c)| (id.raw(), c)).collect();
        Ok(CurveIndex {
            section: ArchiveBuf::serialize(&CurvesSection::new(0, entries))?,
        })
    }

    /// Build the curve index from a live ESM + loose curve JSON files.
    ///
    /// `misc_dir` is the `misc/` directory extracted from a Startup BA2
    /// (curve files live at `<misc_dir>/curvetables/json/<path>`).
    pub fn build_from_dir(
        esm: &EsmFile,
        index: &crate::index::Index,
        misc_dir: &Path,
    ) -> Result<CurveIndex> {
        let section = CurvesSection::new(0, read_loose(esm, index, misc_dir)?);
        Ok(CurveIndex {
            section: ArchiveBuf::serialize(&section)?,
        })
    }

    /// Build the curve index from a live ESM + Startup BA2.
    pub fn build(
        esm: &EsmFile,
        index: &crate::index::Index,
        ba2_path: &Path,
    ) -> Result<CurveIndex> {
        let section = CurvesSection::new(0, read_ba2(esm, index, ba2_path)?);
        Ok(CurveIndex {
            section: ArchiveBuf::serialize(&section)?,
        })
    }

    /// The curves `Database::open` discovered for `esm`, served from the
    /// `curves` cache section: read and published on first use, then mapped.
    /// A section built from a different curve source is rebuilt. A loose
    /// `curvetables/json/` directory is stamped by its own metadata, so
    /// rewriting a curve file inside it in place is not detected.
    pub(crate) fn cached(
        esm: &EsmFile,
        index: &crate::index::Index,
        src: &CurvesSrc,
    ) -> Result<CurveIndex> {
        let source_path = match src {
            CurvesSrc::LooseBase(base) => base.join("curvetables/json"),
            CurvesSrc::Ba2(path) => path.clone(),
        };
        let stamp = crate::rkyvcache::source_stamp(&[source_path], "curves")?;
        let section = crate::rkyvcache::map_or_build::<CurvesSection>(
            &esm.path,
            index.count_by_type("CURV") as u64,
            |cached| cached.source.to_native() == stamp,
            |_lease| {
                let entries = match src {
                    CurvesSrc::LooseBase(base) => read_loose(esm, index, base)?,
                    CurvesSrc::Ba2(path) => read_ba2(esm, index, path)?,
                };
                Ok(CurvesSection::new(stamp, entries))
            },
        )?;
        Ok(CurveIndex {
            section: ArchiveBuf::Mapped(section),
        })
    }
}

fn read_loose(
    esm: &EsmFile,
    index: &crate::index::Index,
    misc_dir: &Path,
) -> Result<Vec<(u32, Curve)>> {
    let json_root = misc_dir.join("curvetables/json");
    if !json_root.is_dir() {
        anyhow::bail!(
            "curves directory missing curvetables/json/: {}",
            json_root.display()
        );
    }
    Ok(read_curves(esm, index, |curv_path| {
        let normalized = curv_path.replace('\\', "/").to_lowercase();
        std::fs::read(json_root.join(normalized)).ok()
    }))
}

fn read_ba2(
    esm: &EsmFile,
    index: &crate::index::Index,
    ba2_path: &Path,
) -> Result<Vec<(u32, Curve)>> {
    let ba2 = Ba2Archive::open(ba2_path)
        .with_context(|| format!("opening Startup BA2: {}", ba2_path.display()))?;
    Ok(read_curves(esm, index, |curv_path| {
        ba2.read(&ba2_internal_path(curv_path)).ok()
    }))
}

/// Pair every CURV record with its curve file: the record's CRVE (or JASF)
/// path is handed to `read`, and a record whose file `read` cannot supply is
/// skipped.
fn read_curves(
    esm: &EsmFile,
    index: &crate::index::Index,
    mut read: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Vec<(u32, Curve)> {
    let mut entries = Vec::new();
    for (form_id, meta) in index.records_by_type("CURV") {
        let parsed = match esm.parse_record_at(meta.offset) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("failed to parse CURV {}: {}", form_id.display(), e);
                continue;
            }
        };
        let edid = crate::reader::edid_from_subrecords(&parsed.subrecords);
        let Some(path_sub) = parsed
            .subrecords
            .iter()
            .find(|s| s.signature.as_str() == "CRVE" || s.signature.as_str() == "JASF")
        else {
            continue;
        };
        let raw = &path_sub.data;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let Ok(curv_path) = std::str::from_utf8(&raw[..end]) else {
            continue;
        };
        let Some(bytes) = read(curv_path) else {
            continue;
        };
        let points = parse_curve_json(&bytes).unwrap_or_default();
        entries.push((
            form_id.raw(),
            Curve {
                edid,
                path: curv_path.to_owned(),
                points,
            },
        ));
    }
    entries
}

/// Map a CURV path string to a BA2 internal path.
pub fn ba2_internal_path(curv_path: &str) -> String {
    let normalized = curv_path.replace('\\', "/");
    format!("misc/curvetables/json/{}", normalized).to_lowercase()
}

fn parse_curve_json(bytes: &[u8]) -> Option<Vec<CurvePoint>> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;

    // Support both top-level array and {"curve": [...]} wrapper
    let arr = if v.is_array() {
        v.as_array()?.to_vec()
    } else {
        let inner = v.get("curve").or_else(|| v.get("points"))?;
        inner.as_array()?.to_vec()
    };

    let mut points: Vec<CurvePoint> = arr
        .iter()
        .filter_map(|p| {
            let x = p.get("x").and_then(|v| v.as_f64())? as f32;
            let y = p.get("y").and_then(|v| v.as_f64())? as f32;
            Some(CurvePoint { x, y })
        })
        .collect();

    // Sort by x to ensure correct interpolation
    points.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    Some(points)
}

// ─── `esm curve` query logic ────────────────────────────────────────────────

/// Build the JSON result for the `esm curve` CLI command (`bin/cli/curve.rs`)
/// from an already-resolved `Op::RecordBulk` response — the query logic
/// itself, kept at library level so any surface can call it.
///
/// `entries` must be the `BulkRecordEntry` list a single `Op::RecordBulk`
/// call produced over the caller's selectors, in the same order — the
/// single-vs-multi wrapping below relies on `entries.len()` matching the
/// original target count.
///
/// `at` and `sum` are mutually exclusive. Clap already enforces this at the
/// CLI layer (`conflicts_with`), but this function re-checks it defensively
/// since a library caller with no clap can't rely on that for free.
///
/// - Exactly one entry: returns that entry's own result object directly, no
///   `"sel"` wrapper key (matches `esm get`'s single-target convention).
/// - More than one entry: returns a JSON array, each object tagged with its
///   own `"sel"` key so callers can correlate results back to selectors.
pub fn curve_query(
    entries: &[crate::ops::BulkRecordEntry],
    at: &[f32],
    sum: Option<(f32, f32)>,
    step: f32,
) -> serde_json::Value {
    if !at.is_empty() && sum.is_some() {
        return serde_json::json!({"error": "--at and --sum are mutually exclusive"});
    }

    let mut results: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| curve_entry_result(entry, at, sum, step))
        .collect();

    if entries.len() == 1 {
        return results.pop().unwrap_or(serde_json::Value::Null);
    }

    for (result, entry) in results.iter_mut().zip(entries) {
        if let serde_json::Value::Object(map) = result {
            map.insert("sel".to_string(), serde_json::json!(entry.sel));
        }
    }
    serde_json::Value::Array(results)
}

/// One entry's result — everything `curve_query` needs except the `"sel"`
/// wrapping, which only the multi-target case adds (see `curve_query`).
fn curve_entry_result(
    entry: &crate::ops::BulkRecordEntry,
    at: &[f32],
    sum: Option<(f32, f32)>,
    step: f32,
) -> serde_json::Value {
    if let Some(err) = &entry.error {
        return serde_json::json!({"error": err});
    }
    let Some(fields) = &entry.fields else {
        return serde_json::json!({
            "error": format!("'{}' did not resolve to a record", entry.sel)
        });
    };
    let record_type = fields
        .get("_record_type")
        .and_then(serde_json::Value::as_str);
    if record_type != Some("Curve Table") {
        return serde_json::json!({
            "error": format!(
                "'{}' resolved to a {}, not a Curve Table",
                entry.sel,
                record_type.unwrap_or("?")
            )
        });
    }

    let Some(points) = points_from_json(fields) else {
        return serde_json::json!({
            "error": format!(
                "'{}': curve tables not loaded — is `misc/curvetables/json/` present next to \
                 the ESM?",
                entry.sel
            )
        });
    };
    if points.is_empty() {
        return serde_json::json!({
            "error": format!("'{}' is a Curve Table record but has no points", entry.sel)
        });
    }

    let editor_id = entry.editor_id.clone().unwrap_or_default();

    if let Some((start, end)) = sum {
        return match sum_range(&points, start, end, step) {
            Ok(total) => serde_json::json!({
                "editor_id": editor_id,
                "start": crate::decode::json_f32(start),
                "end": crate::decode::json_f32(end),
                "step": crate::decode::json_f32(step),
                "sum": total,
            }),
            Err(e) => serde_json::json!({"error": e.to_string()}),
        };
    }

    if !at.is_empty() {
        let values: Vec<serde_json::Value> = at
            .iter()
            .map(|&x| {
                let y = eval(&points, x).unwrap_or(0.0);
                serde_json::json!({"x": crate::decode::json_f32(x), "y": crate::decode::json_f32(y)})
            })
            .collect();
        return serde_json::json!({"editor_id": editor_id, "at": values});
    }

    let curve_path = fields
        .get("JSON File Path")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            fields
                .get("JSON File Path 2")
                .and_then(serde_json::Value::as_str)
        });
    let point_values: Vec<serde_json::Value> = points
        .iter()
        .map(|p| serde_json::json!({"x": crate::decode::json_f32(p.x), "y": crate::decode::json_f32(p.y)}))
        .collect();
    match curve_path {
        Some(path) => serde_json::json!({
            "editor_id": editor_id,
            "curve_path": path,
            "points": point_values,
        }),
        None => serde_json::json!({
            "editor_id": editor_id,
            "points": point_values,
        }),
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// Pins the curves section's archived layout (see
    /// `crate::rkyvcache::assert_archived_layout`).
    #[test]
    fn curves_section_layout_is_pinned() {
        let curve = Curve {
            edid: Some("CT_Test".into()),
            path: "Test.json".into(),
            points: vec![
                CurvePoint { x: 1.0, y: 10.0 },
                CurvePoint { x: 50.0, y: 50.0 },
            ],
        };
        let section = CurvesSection::new(7, vec![(0x10, curve)]);
        crate::rkyvcache::assert_archived_layout("curves", &section, 0xe67efa43fe07698b);
    }
}
