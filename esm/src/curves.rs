//! Curve table resolution for Fallout 76 CURV records.
//!
//! Builds an index from CURV FormID → parsed curve points by reading
//! CURV records from an ESM and loading the JSON point data from the
//! Startup BA2 archive.

use crate::{ba2::Ba2Archive, formid::FormId, reader::EsmFile};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

/// A single point in a curve table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
}

/// A parsed curve table: EditorID, source path, and interpolatable points.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Curve {
    pub edid: String,
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

/// Index of CURV FormID → parsed curve.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CurveIndex {
    by_formid: HashMap<u32, Curve>,
}

impl CurveIndex {
    /// Look up a curve by FormID.
    pub fn get(&self, id: FormId) -> Option<&Curve> {
        self.by_formid.get(&id.raw())
    }

    /// Build the curve index from a live ESM + Startup BA2.
    pub fn build(
        esm: &EsmFile,
        index: &crate::index::Index,
        ba2_path: &Path,
    ) -> Result<CurveIndex> {
        let ba2 = Ba2Archive::open(ba2_path)
            .with_context(|| format!("opening Startup BA2: {}", ba2_path.display()))?;

        let curv_records = index.records_by_type("CURV");
        let mut by_formid = HashMap::new();

        for (form_id, meta) in curv_records {
            let parsed = match esm.parse_record_at(meta.offset) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Warning: failed to parse CURV {}: {}", form_id.display(), e);
                    continue;
                }
            };

            let edid = crate::reader::edid_from_subrecords(&parsed.subrecords).unwrap_or_default();

            // Find the path subrecord — try CRVE first, then JASF
            let path_sub = parsed
                .subrecords
                .iter()
                .find(|s| s.signature.as_str() == "CRVE" || s.signature.as_str() == "JASF");
            let Some(path_sub) = path_sub else {
                continue;
            };

            // Read as NUL-terminated string
            let raw = &path_sub.data;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            let curv_path = match std::str::from_utf8(&raw[..end]) {
                Ok(s) => s.to_owned(),
                Err(_) => continue,
            };

            // Map to BA2 internal path
            let internal = ba2_internal_path(&curv_path);

            // Read and parse curve JSON
            let points = match ba2.read(&internal) {
                Ok(bytes) => parse_curve_json(&bytes).unwrap_or_default(),
                Err(_) => {
                    // Path not in this BA2 — skip silently (common)
                    continue;
                }
            };

            by_formid.insert(
                form_id.raw(),
                Curve {
                    edid,
                    path: curv_path,
                    points,
                },
            );
        }

        Ok(CurveIndex { by_formid })
    }
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
    } else if let Some(inner) = v.get("curve").or_else(|| v.get("points")) {
        inner.as_array()?.to_vec()
    } else {
        return None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ba2_path_mapping() {
        assert_eq!(
            ba2_internal_path(r"Weapons\Weap_10mmSMGDMG.json"),
            "misc/curvetables/json/weapons/weap_10mmsmgdmg.json"
        );
        assert_eq!(
            ba2_internal_path("Creatures/Weapon/Damage_Universal_Tier24.json"),
            "misc/curvetables/json/creatures/weapon/damage_universal_tier24.json"
        );
    }

    #[test]
    fn eval_clamp_low() {
        let pts = vec![
            CurvePoint { x: 1.0, y: 10.0 },
            CurvePoint { x: 10.0, y: 100.0 },
        ];
        assert_eq!(eval(&pts, 0.0), Some(10.0)); // clamp to first
    }

    #[test]
    fn eval_clamp_high() {
        let pts = vec![
            CurvePoint { x: 1.0, y: 10.0 },
            CurvePoint { x: 10.0, y: 100.0 },
        ];
        assert_eq!(eval(&pts, 20.0), Some(100.0)); // clamp to last
    }

    #[test]
    fn eval_interpolation() {
        let pts = vec![
            CurvePoint { x: 0.0, y: 0.0 },
            CurvePoint { x: 10.0, y: 100.0 },
        ];
        assert_eq!(eval(&pts, 5.0), Some(50.0));
    }

    #[test]
    fn eval_empty() {
        assert_eq!(eval(&[], 5.0), None);
    }

    #[test]
    fn eval_duplicate_x() {
        let pts = vec![
            CurvePoint { x: 5.0, y: 42.0 },
            CurvePoint { x: 5.0, y: 99.0 }, // duplicate x
        ];
        // Should return first y value (a.y when b.x == a.x)
        assert!(eval(&pts, 5.0).is_some());
    }
}
