//! Field-path predicates for [`crate::Database::filter_type_records`].

use crate::database::RecordRow;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Comparison operator for [`Database::filter_type_records`].
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    /// True iff the path resolves to a present value (even `null`). No `value` used.
    Exists,
    /// Numeric equality if both sides parse as numbers; otherwise a
    /// case-insensitive exact string match.
    Eq,
    /// Case-insensitive substring match; deep-scans if the resolved value is
    /// itself an object or array.
    Contains,
    /// Numeric greater-than.
    Gt,
    /// Numeric less-than.
    Lt,
    /// Numeric greater-than-or-equal.
    Gte,
    /// Numeric less-than-or-equal.
    Lte,
}

/// Maximum number of records of a single type decoded and cached by
/// [`Database::type_sample`]. Types like REFR/NAVM/LAND can have tens
/// or hundreds of thousands of records; a full schema-driven decode of all of
/// them is meaningfully more expensive than the cheap header/EDID scans
/// `ensure_xref_index`/`ensure_search_index` already do at full-file scale.
/// `records_by_type` is FormID-sorted, so this is a stable, deterministic
/// subset rather than an arbitrary truncation.
pub(crate) const FILTER_SCAN_CAP: usize = 20_000;

/// `limit == 0` means "unlimited" — the shared convention every `limit`
/// parameter in this crate's public query API follows (`list_by_type`,
/// `list_type_records`, `search`, `filter_type_records`), restated
/// identically at each call site before this helper existed. Feed the
/// result to `Iterator::take`/`Vec::truncate`: `usize::MAX` items is
/// effectively "don't stop early" for any realistic in-memory collection.
pub(crate) fn effective_take(limit: usize) -> usize {
    if limit == 0 { usize::MAX } else { limit }
}

/// Evaluate a filter predicate against a decoded record's `fields` JSON body.
///
/// `path` is a dot-separated sequence of segments navigating into `fields`
/// (schema-driven key names, e.g. `"Data.Damage"`). A segment of `"[]"` means
/// "the current value must be a JSON array; recurse into every element for
/// the remaining path, matching if ANY element satisfies it". An empty/`None`
/// path means "deep-scan every value anywhere in the record, matching if ANY
/// value anywhere satisfies the operator".
pub(crate) fn predicate_matches(
    fields: &Value,
    path: Option<&str>,
    op: FilterOp,
    value: Option<&str>,
) -> bool {
    let path = path.map(str::trim).filter(|p| !p.is_empty());
    match path {
        None => deep_scan_matches(fields, op, value),
        Some(p) => {
            let segments: Vec<&str> = p.split('.').collect();
            navigate_matches(fields, &segments, op, value)
        }
    }
}

/// Walk `segments` into `current`, applying `[]` array-wildcard fan-out, and
/// test the operator once the path is exhausted. Returns `false` if the path
/// doesn't exist in the JSON (e.g. an object without the requested key).
fn navigate_matches(current: &Value, segments: &[&str], op: FilterOp, value: Option<&str>) -> bool {
    match segments.split_first() {
        None => op_matches(current, op, value),
        Some((&"[]", rest)) => match current {
            Value::Array(items) => items
                .iter()
                .any(|item| navigate_matches(item, rest, op, value)),
            _ => false,
        },
        Some((seg, rest)) => match current {
            Value::Object(map) => match map.get(*seg) {
                Some(next) => navigate_matches(next, rest, op, value),
                None => false,
            },
            _ => false,
        },
    }
}

/// Test the operator against a value reached via explicit path navigation.
/// `Contains` deep-scans when the terminal value is itself a container.
fn op_matches(current: &Value, op: FilterOp, value: Option<&str>) -> bool {
    match op {
        FilterOp::Exists => true,
        FilterOp::Contains => match current {
            Value::Object(_) | Value::Array(_) => deep_scan_matches(current, op, value),
            _ => value_matches(current, op, value),
        },
        _ => value_matches(current, op, value),
    }
}

/// Recurse through every value anywhere in `v` (objects' values, array
/// elements, and scalars), matching if ANY value satisfies the operator.
fn deep_scan_matches(v: &Value, op: FilterOp, value: Option<&str>) -> bool {
    if value_matches(v, op, value) {
        return true;
    }
    match v {
        Value::Object(map) => map.values().any(|vv| deep_scan_matches(vv, op, value)),
        Value::Array(items) => items.iter().any(|vv| deep_scan_matches(vv, op, value)),
        _ => false,
    }
}

/// Scalar-only operator test: containers never match directly here — the
/// caller's recursion (`deep_scan_matches`/`op_matches`) is responsible for
/// visiting a container's children.
fn value_matches(current: &Value, op: FilterOp, value: Option<&str>) -> bool {
    if matches!(current, Value::Object(_) | Value::Array(_)) {
        return false;
    }
    match op {
        FilterOp::Exists => true,
        FilterOp::Eq => eq_matches(current, value),
        FilterOp::Contains => match value {
            Some(needle) => stringify_scalar(current)
                .map(|s| s.to_lowercase().contains(&needle.to_lowercase()))
                .unwrap_or(false),
            None => false,
        },
        FilterOp::Gt | FilterOp::Lt | FilterOp::Gte | FilterOp::Lte => {
            numeric_matches(current, op, value)
        }
    }
}

/// Render a scalar JSON value as its natural display text (strings as raw
/// content, not JSON-quoted). Returns `None` for objects/arrays.
fn stringify_scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".to_string()),
        Value::Object(_) | Value::Array(_) => None,
    }
}

fn eq_matches(current: &Value, value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    if let Value::Number(n) = current
        && let (Some(cur_f), Ok(val_f)) = (n.as_f64(), value.parse::<f64>())
    {
        return cur_f == val_f;
    }
    match stringify_scalar(current) {
        Some(s) => s.eq_ignore_ascii_case(value),
        None => false,
    }
}

fn numeric_matches(current: &Value, op: FilterOp, value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let Ok(val_f) = value.parse::<f64>() else {
        return false;
    };
    let cur_f = match current {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    };
    let Some(cur_f) = cur_f else {
        return false;
    };
    match op {
        FilterOp::Gt => cur_f > val_f,
        FilterOp::Lt => cur_f < val_f,
        FilterOp::Gte => cur_f >= val_f,
        FilterOp::Lte => cur_f <= val_f,
        _ => false,
    }
}

/// Collect every dot-notation field path present in `v` into `out`, capping
/// defensively once `out` reaches `cap` entries. Array levels collapse to a
/// literal `"[]"` segment regardless of index.
pub(crate) fn collect_field_paths(v: &Value, prefix: &str, out: &mut HashSet<String>, cap: usize) {
    if out.len() >= cap {
        return;
    }
    match v {
        Value::Object(map) => {
            for (k, vv) in map {
                let next = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                out.insert(next.clone());
                collect_field_paths(vv, &next, out, cap);
                if out.len() >= cap {
                    return;
                }
            }
        }
        Value::Array(items) => {
            let next = if prefix.is_empty() {
                "[]".to_string()
            } else {
                format!("{prefix}.[]")
            };
            out.insert(next.clone());
            for item in items {
                collect_field_paths(item, &next, out, cap);
                if out.len() >= cap {
                    return;
                }
            }
        }
        _ => {}
    }
}

/// Result envelope for [`Database::filter_type_records`] — reports both
/// whether the requested `limit` truncated the match list, and whether the
/// underlying decode itself was capped (see [`FILTER_SCAN_CAP`]) for a huge
/// type, so callers can honestly report "N of M possible matches, based on
/// the first K of L total records of this type" rather than silently
/// under-covering.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct FilterResult {
    pub rows: Vec<RecordRow>,
    /// Total matches found within the scanned set (may exceed rows.len() if `limit` truncated).
    pub matched: usize,
    /// How many records of this type were actually decoded and tested.
    pub scanned: usize,
    /// Total records of this type that exist in the file.
    pub total: usize,
    /// True if rows.len() < matched (the match list itself was truncated by `limit`).
    pub capped: bool,
    /// True if scanned < total (the decode pass itself stopped at FILTER_SCAN_CAP).
    pub scan_capped: bool,
}

#[cfg(test)]
mod tests {
    use super::{FilterOp, predicate_matches};
    use serde_json::json;

    #[test]
    fn simple_top_level_eq() {
        let fields = json!({ "EditorID": "TestWeapon" });
        assert!(predicate_matches(
            &fields,
            Some("EditorID"),
            FilterOp::Eq,
            Some("testweapon")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("EditorID"),
            FilterOp::Eq,
            Some("other")
        ));
    }

    #[test]
    fn simple_top_level_contains() {
        let fields = json!({ "Name": "Combat Rifle" });
        assert!(predicate_matches(
            &fields,
            Some("Name"),
            FilterOp::Contains,
            Some("rifle")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Name"),
            FilterOp::Contains,
            Some("shotgun")
        ));
    }

    #[test]
    fn simple_top_level_gt() {
        let fields = json!({ "Value": 50 });
        assert!(predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Gt,
            Some("10")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Gt,
            Some("100")
        ));
    }

    #[test]
    fn nested_dot_path_navigation() {
        let fields = json!({ "Data": { "Damage": 25, "Weight": 5.5 } });
        assert!(predicate_matches(
            &fields,
            Some("Data.Damage"),
            FilterOp::Eq,
            Some("25")
        ));
        assert!(predicate_matches(
            &fields,
            Some("Data.Weight"),
            FilterOp::Lt,
            Some("10")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Data.Missing"),
            FilterOp::Exists,
            None
        ));
    }

    #[test]
    fn array_wildcard_matches_any_element() {
        let fields = json!({
            "Components": [
                { "Component": "Steel", "Count": 2 },
                { "Component": "Wood", "Count": 1 },
            ]
        });
        assert!(predicate_matches(
            &fields,
            Some("Components.[].Component"),
            FilterOp::Eq,
            Some("Wood")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Components.[].Component"),
            FilterOp::Eq,
            Some("Aluminum")
        ));
    }

    #[test]
    fn empty_path_deep_scan() {
        let fields = json!({
            "Data": { "Damage": 25 },
            "Keywords": ["WeapTypeRifle", "Craftable"],
        });
        // Deep-scan finds a nested value anywhere in the tree.
        assert!(predicate_matches(&fields, None, FilterOp::Eq, Some("25")));
        assert!(predicate_matches(
            &fields,
            Some(""),
            FilterOp::Contains,
            Some("rifle")
        ));
        assert!(!predicate_matches(
            &fields,
            None,
            FilterOp::Eq,
            Some("nope")
        ));
    }

    #[test]
    fn exists_on_present_but_null_field() {
        let fields = json!({ "Optional": null });
        assert!(predicate_matches(
            &fields,
            Some("Optional"),
            FilterOp::Exists,
            None
        ));
    }

    #[test]
    fn exists_on_genuinely_missing_field() {
        let fields = json!({ "Other": 1 });
        assert!(!predicate_matches(
            &fields,
            Some("Missing"),
            FilterOp::Exists,
            None
        ));
    }

    #[test]
    fn numeric_eq_matches_string_value_against_json_number() {
        let fields = json!({ "Value": 50.0 });
        assert!(predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Eq,
            Some("50")
        ));
    }

    #[test]
    fn contains_matches_substring_of_stringified_number() {
        let fields = json!({ "Code": 1234 });
        assert!(predicate_matches(
            &fields,
            Some("Code"),
            FilterOp::Contains,
            Some("23")
        ));
    }

    #[test]
    fn gt_wrong_type_does_not_match() {
        let fields = json!({ "Name": "not a number" });
        assert!(!predicate_matches(
            &fields,
            Some("Name"),
            FilterOp::Gt,
            Some("10")
        ));
    }

    #[test]
    fn gt_unparseable_value_does_not_match() {
        let fields = json!({ "Value": 50 });
        assert!(!predicate_matches(
            &fields,
            Some("Value"),
            FilterOp::Gt,
            Some("not-a-number")
        ));
    }

    #[test]
    fn contains_on_object_deep_scans_nested_values() {
        let fields = json!({
            "Data": { "Nested": { "Label": "SpecialSteel" } }
        });
        assert!(predicate_matches(
            &fields,
            Some("Data"),
            FilterOp::Contains,
            Some("steel")
        ));
        assert!(!predicate_matches(
            &fields,
            Some("Data"),
            FilterOp::Contains,
            Some("wood")
        ));
    }
}
