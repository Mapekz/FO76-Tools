//! Registry of level-keyed curve fields: the (record signature, field path)
//! pairs whose Curve Table x-axis is verified to be player/creature level,
//! so `walk --level` can evaluate them.
//!
//! Membership is by *verified x-domain*, not by field name. Two curve
//! fields are deliberately ABSENT and must stay absent:
//!
//! - COBJ `Curve Table` — keyed on component *count*, already evaluated at
//!   the count by the derived component `Quantity` (`decode::derived`).
//! - LVLI `Minimim Level Curve Table` (schema typo, preserved verbatim) —
//!   an item-quality-tier index (0-3 with 99/100 sentinels), not level.
//!   Flagged rather than evaluated by `lvli::resolve_min_level` — see that
//!   function's doc comment for the full reasoning. Do not duplicate or
//!   regress that existing behavior with this new mechanism.

use serde_json::Value;

/// How a [`LevelCurveField`]'s x-axis is confirmed to be level.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AxisGuard {
    /// x is level unconditionally.
    Always,
    /// x is level only when the named sibling field on the same JSON object
    /// is absent, null, or the literal string "None" — otherwise that
    /// sibling names the real (non-level) axis, and the row should render
    /// an axis note instead of an evaluated number.
    ///
    /// Only ENCH/SPEL/ALCH `Effects[]` need this. Verified: every
    /// `Actor Value: None` effect curve is level-domained; every named-AV
    /// one is domained on its own axis (tier counters, SPECIAL stats, a
    /// 0..40000 legendary-caps curve). Evaluating a caps curve at level 50
    /// would print a confident, wrong number — exactly what
    /// `lvli::resolve_min_level`'s existing carve-out already refuses to
    /// do for its own case.
    SiblingIsNoneOrAbsent(&'static str),
}

/// One level-keyed curve field registration. See module docs for the
/// membership test and [`LEVEL_KEYED_CURVES`] for the verified table.
pub(crate) struct LevelCurveField {
    /// Record signature, e.g. `"WEAP"`.
    pub sig: &'static str,
    /// Dot path from the decoded fields root; a `"[]"` segment iterates an
    /// array (at most one per path — see [`navigate`]).
    pub path: &'static str,
    /// Fallback label when there's no per-row label sibling.
    pub label: &'static str,
    /// Sibling key on the same row whose value labels the row (its
    /// `editor_id` if an object, else its string value) — `None` means use
    /// `label` (with an array index suffix when the row came from array
    /// iteration, to disambiguate multiple rows).
    pub label_from: Option<&'static str>,
    pub guard: AxisGuard,
}

pub(crate) const LEVEL_KEYED_CURVES: &[LevelCurveField] = &[
    LevelCurveField {
        sig: "WEAP",
        path: "Damage Curve",
        label: "damage",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "WEAP",
        path: "Min Durability Curve",
        label: "durability min",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "WEAP",
        path: "Max Durability Curve",
        label: "durability max",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "WEAP",
        path: "Condition Loss Scale",
        label: "condition loss",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "WEAP",
        path: "Bash Condition Loss Scale",
        label: "bash condition loss",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "WEAP",
        path: "Damage Types[].Curve Table",
        label: "damage type",
        label_from: Some("Type"),
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "NPC_",
        path: "Properties[].Curve Table",
        label: "property",
        label_from: Some("Actor Value"),
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "RACE",
        path: "Properties[].Curve Table",
        label: "property",
        label_from: Some("Actor Value"),
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "RACE",
        path: "EWS Actor Cost Curve",
        label: "EWS actor cost",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "ARMO",
        path: "Resistances[].Curve Table",
        label: "resistance",
        label_from: Some("Type"),
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "ARMO",
        path: "Durability Min",
        label: "durability min",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "ARMO",
        path: "Durability Max",
        label: "durability max",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "ARMO",
        path: "Condition Damage Scale Factor",
        label: "condition loss",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "EXPL",
        path: "Data.Damage Curve Table",
        label: "damage",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "EXPL",
        path: "Damage Types[].Curve Table",
        label: "damage type",
        label_from: Some("Type"),
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "ALCH",
        path: "Effect Data.Health",
        label: "food health",
        label_from: None,
        guard: AxisGuard::Always,
    },
    LevelCurveField {
        sig: "ENCH",
        path: "Effects[].Effect.Curve Table",
        label: "effect",
        label_from: None,
        guard: AxisGuard::SiblingIsNoneOrAbsent("Actor Value"),
    },
    LevelCurveField {
        sig: "SPEL",
        path: "Effects[].Effect.Curve Table",
        label: "effect",
        label_from: None,
        guard: AxisGuard::SiblingIsNoneOrAbsent("Actor Value"),
    },
    LevelCurveField {
        sig: "ALCH",
        path: "Effects[].Effect.Curve Table",
        label: "effect",
        label_from: None,
        guard: AxisGuard::SiblingIsNoneOrAbsent("Actor Value"),
    },
];

/// Look up the single [`LevelCurveField`] registered for `(sig, path)` — used
/// by `walk::digest_magic_item` to reuse this module's guard rule for its own
/// per-effect `curve_at_level` field rather than re-deriving it.
pub(crate) fn field_for(sig: &str, path: &str) -> Option<&'static LevelCurveField> {
    LEVEL_KEYED_CURVES
        .iter()
        .find(|f| f.sig == sig && f.path == path)
}

/// Apply an [`AxisGuard`] to `row` (the JSON object that owns both the curve
/// field and the guard's sibling key, if any): `None` means "evaluation is
/// permitted", `Some(axis)` means "suppressed — this is the real axis name".
pub(crate) fn guard_axis(guard: &AxisGuard, row: &Value) -> Option<String> {
    match guard {
        AxisGuard::Always => None,
        AxisGuard::SiblingIsNoneOrAbsent(key) => match row.get(*key) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s == "None" => None,
            Some(Value::Object(m))
                if m.get("editor_id").and_then(Value::as_str) == Some("None") =>
            {
                None
            }
            Some(other) => Some(axis_label(other)),
        },
    }
}

/// Render a non-level axis value (a resolved ref stub's `editor_id`, or a
/// bare string/number) as the text `render_level_curves` shows the reader.
fn axis_label(v: &Value) -> String {
    match v {
        Value::Object(m) => m
            .get("editor_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "?".to_string()),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One matched curve leaf from [`navigate`]: the fully-resolved field path
/// (array segments carry their real index), the leaf value itself, the row
/// object that owns it (for `label_from`/guard sibling lookups), and the
/// array index the row was reached at, if any (used to disambiguate a
/// static label across multiple rows — see [`LevelCurveField::label_from`]).
struct MatchedLeaf<'a> {
    field_path: String,
    leaf: &'a Value,
    row: &'a Value,
    array_index: Option<usize>,
}

/// Walk `path` (dot-separated, at most one `"[]"`-suffixed segment for one
/// level of array iteration — see [`LevelCurveField::path`]) against
/// `fields`, returning every matched leaf. Intentionally minimal: it does
/// not support nested arrays, wildcards, or any JSONPath feature beyond
/// what this crate's verified field paths actually need.
fn navigate<'a>(fields: &'a Value, path: &str) -> Vec<MatchedLeaf<'a>> {
    struct Frame<'a> {
        path: String,
        value: &'a Value,
        array_index: Option<usize>,
    }

    let segments: Vec<&str> = path.split('.').collect();
    let Some((last, init)) = segments.split_last() else {
        return Vec::new();
    };

    let mut frames = vec![Frame {
        path: String::new(),
        value: fields,
        array_index: None,
    }];

    for seg in init {
        let mut next = Vec::new();
        if let Some(key) = seg.strip_suffix("[]") {
            for f in &frames {
                let Some(arr) = f.value.get(key).and_then(Value::as_array) else {
                    continue;
                };
                for (i, elem) in arr.iter().enumerate() {
                    let p = if f.path.is_empty() {
                        format!("{key}[{i}]")
                    } else {
                        format!("{}.{}[{}]", f.path, key, i)
                    };
                    next.push(Frame {
                        path: p,
                        value: elem,
                        array_index: Some(i),
                    });
                }
            }
        } else {
            for f in &frames {
                let Some(child) = f.value.get(*seg) else {
                    continue;
                };
                let p = if f.path.is_empty() {
                    (*seg).to_string()
                } else {
                    format!("{}.{}", f.path, seg)
                };
                next.push(Frame {
                    path: p,
                    value: child,
                    array_index: f.array_index,
                });
            }
        }
        frames = next;
    }

    let mut out = Vec::new();
    for f in frames {
        let Some(leaf) = f.value.get(*last) else {
            continue;
        };
        let field_path = if f.path.is_empty() {
            (*last).to_string()
        } else {
            format!("{}.{}", f.path, last)
        };
        out.push(MatchedLeaf {
            field_path,
            leaf,
            row: f.value,
            array_index: f.array_index,
        });
    }
    out
}

fn resolve_label(entry: &LevelCurveField, m: &MatchedLeaf<'_>) -> String {
    if let Some(key) = entry.label_from {
        if let Some(v) = m.row.get(key) {
            match v {
                Value::Object(o) => {
                    if let Some(e) = o.get("editor_id").and_then(Value::as_str) {
                        return e.to_string();
                    }
                }
                Value::String(s) => return s.clone(),
                _ => {}
            }
        }
        return entry.label.to_string();
    }
    match m.array_index {
        Some(i) => format!("{}[{i}]", entry.label),
        None => entry.label.to_string(),
    }
}

/// One evaluated level-keyed curve found on a decoded record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct LevelCurveRow {
    /// e.g. `"Properties[2].Curve Table"`.
    pub field: String,
    /// Resolved label (sibling value's editor_id/string, or the field's
    /// static label).
    pub label: String,
    /// The CURV record's own editor_id, if the resolved reference stub
    /// carries one.
    pub curve_edid: Option<String>,
    /// `eval(points, level)` result; `None` when the guard suppressed
    /// evaluation.
    pub value: Option<f64>,
    /// Set only when `value` is `None`: the sibling value naming the real
    /// axis.
    pub axis: Option<String>,
}

/// Every level-keyed curve on a decoded record's `fields`, evaluated at
/// `level`, for the given record `sig`nature. Empty vec for signatures with
/// no rows in [`LEVEL_KEYED_CURVES`].
///
/// A matched leaf with no curve loaded/present at all (`points_from_json`
/// returns `None`, or `Some(vec![])` — a curve reference that resolved but
/// carries zero points) produces no row: this is the normal case for the
/// overwhelming majority of fields on the overwhelming majority of records,
/// not a decode failure worth flagging.
pub fn eval_level_curves(sig: &str, fields: &Value, level: f32) -> Vec<LevelCurveRow> {
    let mut out = Vec::new();
    for entry in LEVEL_KEYED_CURVES.iter().filter(|e| e.sig == sig) {
        for m in navigate(fields, entry.path) {
            let Some(points) = crate::curves::points_from_json(m.leaf) else {
                continue;
            };
            if points.is_empty() {
                continue;
            }
            let curve_edid = m
                .leaf
                .get("editor_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let label = resolve_label(entry, &m);
            let (value, axis) = match guard_axis(&entry.guard, m.row) {
                None => (crate::curves::eval(&points, level).map(f64::from), None),
                Some(axis) => (None, Some(axis)),
            };
            out.push(LevelCurveRow {
                field: m.field_path,
                label,
                curve_edid,
                value,
                axis,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evaluates_plain_top_level_curve_field() {
        let fields = json!({
            "Damage Curve": {
                "formid": "0x1", "editor_id": "CT_Test", "curve_path": "x.json",
                "curve": [{"x": 1.0, "y": 10.0}, {"x": 50.0, "y": 100.0}],
            }
        });
        let rows = eval_level_curves("WEAP", &fields, 50.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field, "Damage Curve");
        assert_eq!(rows[0].label, "damage");
        assert_eq!(rows[0].value, Some(100.0));
        assert_eq!(rows[0].axis, None);
    }

    #[test]
    fn array_iteration_produces_one_row_per_element_with_index_in_path() {
        let fields = json!({
            "Properties": [
                {"Actor Value": {"formid": "0x1", "editor_id": "Health", "record_type": "AVIF"}, "Curve Table": {
                    "curve": [{"x": 0.0, "y": 1.0}, {"x": 10.0, "y": 2.0}],
                }},
                {"Actor Value": {"formid": "0x2", "editor_id": "DamageResist", "record_type": "AVIF"}, "Curve Table": {
                    "curve": [{"x": 0.0, "y": 5.0}, {"x": 10.0, "y": 6.0}],
                }},
            ]
        });
        let rows = eval_level_curves("NPC_", &fields, 10.0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].field, "Properties[0].Curve Table");
        assert_eq!(rows[0].label, "Health");
        assert_eq!(rows[0].value, Some(2.0));
        assert_eq!(rows[1].field, "Properties[1].Curve Table");
        assert_eq!(rows[1].label, "DamageResist");
        assert_eq!(rows[1].value, Some(6.0));
    }

    #[test]
    fn armo_resistances_label_from_type_not_actor_value() {
        let fields = json!({
            "Resistances": [
                {"Type": {"formid": "0x1", "editor_id": "dtEnergy", "record_type": "DMGT"}, "Curve Table": {
                    "curve": [{"x": 0.0, "y": 1.0}],
                }},
            ]
        });
        let rows = eval_level_curves("ARMO", &fields, 5.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "dtEnergy");
    }

    #[test]
    fn missing_intermediate_key_yields_no_rows() {
        let fields = json!({"SomeOtherKey": 1});
        let rows = eval_level_curves("EXPL", &fields, 10.0);
        assert!(rows.is_empty());
    }

    #[test]
    fn null_curve_table_value_yields_no_row() {
        let fields = json!({
            "Resistances": [
                {"Type": {"formid": "0x1", "editor_id": "dtEnergy"}, "Curve Table": null},
            ]
        });
        let rows = eval_level_curves("ARMO", &fields, 5.0);
        assert!(rows.is_empty());
    }

    #[test]
    fn empty_curve_points_yields_no_row() {
        let fields = json!({"Damage Curve": {"curve": []}});
        let rows = eval_level_curves("WEAP", &fields, 5.0);
        assert!(rows.is_empty());
    }

    #[test]
    fn guard_permits_evaluation_when_actor_value_absent() {
        let row = json!({"Curve Table": {"curve": [{"x": 0.0, "y": 1.0}]}});
        assert_eq!(
            guard_axis(&AxisGuard::SiblingIsNoneOrAbsent("Actor Value"), &row),
            None
        );
    }

    #[test]
    fn guard_permits_evaluation_when_actor_value_null() {
        let row = json!({"Actor Value": null, "Curve Table": {"curve": [{"x": 0.0, "y": 1.0}]}});
        assert_eq!(
            guard_axis(&AxisGuard::SiblingIsNoneOrAbsent("Actor Value"), &row),
            None
        );
    }

    #[test]
    fn guard_permits_evaluation_when_actor_value_literal_none_string() {
        let row = json!({"Actor Value": "None"});
        assert_eq!(
            guard_axis(&AxisGuard::SiblingIsNoneOrAbsent("Actor Value"), &row),
            None
        );
    }

    #[test]
    fn guard_permits_evaluation_when_actor_value_literal_none_stub() {
        let row = json!({"Actor Value": {"editor_id": "None"}});
        assert_eq!(
            guard_axis(&AxisGuard::SiblingIsNoneOrAbsent("Actor Value"), &row),
            None
        );
    }

    #[test]
    fn guard_suppresses_evaluation_and_names_axis_when_actor_value_named() {
        let row = json!({"Actor Value": {"formid": "0x1", "editor_id": "Perception", "record_type": "AVIF"}});
        assert_eq!(
            guard_axis(&AxisGuard::SiblingIsNoneOrAbsent("Actor Value"), &row),
            Some("Perception".to_string())
        );
    }

    #[test]
    fn ench_effects_guard_end_to_end_via_eval_level_curves() {
        let fields = json!({
            "Effects": [
                {"Effect": {
                    "Actor Value": null,
                    "Curve Table": {"editor_id": "CT_Level", "curve": [{"x": 1.0, "y": 10.0}, {"x": 50.0, "y": 100.0}]},
                }},
                {"Effect": {
                    "Actor Value": {"formid": "0x1", "editor_id": "Perception", "record_type": "AVIF"},
                    "Curve Table": {"editor_id": "CT_Caps", "curve": [{"x": 0.0, "y": 1.0}, {"x": 40000.0, "y": 5.0}]},
                }},
            ]
        });
        let rows = eval_level_curves("ENCH", &fields, 50.0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].field, "Effects[0].Effect.Curve Table");
        assert_eq!(rows[0].label, "effect[0]");
        assert_eq!(rows[0].value, Some(100.0));
        assert_eq!(rows[0].axis, None);
        assert_eq!(rows[1].field, "Effects[1].Effect.Curve Table");
        assert_eq!(rows[1].label, "effect[1]");
        assert_eq!(rows[1].value, None);
        assert_eq!(rows[1].axis, Some("Perception".to_string()));
    }

    #[test]
    fn cobj_curve_table_is_not_in_the_allowlist() {
        let fields = json!({
            "Components": [{"Count": 3, "Curve Table": {"curve": [{"x": 1.0, "y": 1.0}, {"x": 5.0, "y": 5.0}]}}],
        });
        let rows = eval_level_curves("COBJ", &fields, 10.0);
        assert!(
            rows.is_empty(),
            "COBJ has no LEVEL_KEYED_CURVES rows at all"
        );
    }

    #[test]
    fn lvli_minimim_level_curve_table_is_not_in_the_allowlist() {
        let fields = json!({
            "Minimim Level Curve Table": {"curve": [{"x": 0.0, "y": 1.0}, {"x": 3.0, "y": 4.0}]},
        });
        let rows = eval_level_curves("LVLI", &fields, 50.0);
        assert!(
            rows.is_empty(),
            "LVLI has no LEVEL_KEYED_CURVES rows at all"
        );
    }
}
