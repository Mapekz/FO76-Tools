//! Readers for decoded record bodies shared by chase, walk and the drop
//! table: schema enums, condition rows, and Python-style truthiness — over
//! the [`Resolved`] records they traverse, and over the JSON they render
//! (`walk`'s text renderer reads its own digests back).

use serde_json::Value;

use crate::{FormId, Resolved};

/// The human name of a `{"value":.., "name":..}` schema enum object, or the
/// value itself if it isn't wrapped that way, rendered.
pub(crate) fn named(field: Option<&Resolved>) -> Value {
    match field {
        Some(v) if v.is_object() => v.get("name").unwrap_or(v).to_json(),
        Some(other) => other.to_json(),
        None => Value::Null,
    }
}

/// [`named`] over rendered JSON.
pub(crate) fn named_json(field: Option<&Value>) -> Value {
    match field {
        Some(Value::Object(map)) => map
            .get("name")
            .cloned()
            .unwrap_or_else(|| field.cloned().unwrap_or(Value::Null)),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// Python-truthiness for a value (`None`/`0`/`""`/`[]`/`{}`/`false` are
/// falsy, as in a bare Python `if x:`). A reference is truthy.
pub(crate) fn is_truthy(v: Option<&Resolved>) -> bool {
    match v {
        None | Some(Resolved::Null) => false,
        Some(Resolved::Ref { .. }) => true,
        Some(Resolved::Bool(b)) => *b,
        Some(Resolved::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Resolved::String(s)) => !s.is_empty(),
        Some(Resolved::Array(a)) => !a.is_empty(),
        Some(Resolved::Object(o)) => !o.is_empty(),
    }
}

/// [`is_truthy`] over rendered JSON.
pub(crate) fn is_truthy_json(v: Option<&Value>) -> bool {
    match v {
        None => false,
        Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// Whether rendered JSON is a reference stub
/// (`{"formid", "editor_id", "record_type"}`).
pub(crate) fn is_stub_json(v: &Value) -> bool {
    matches!(v, Value::Object(map) if map.contains_key("formid"))
}

pub(crate) fn dedup_sorted(fids: &mut Vec<FormId>) {
    fids.sort_by_key(|f| f.0);
    fids.dedup();
}

/// Pull the flat condition rows out of a SPEL/ENCH/ALCH/MGEF-style
/// `Conditions` node; LVLI entries decode `Conditions` into the same shape.
pub(crate) fn flatten_condition_rows(node: &Resolved) -> Vec<Resolved> {
    node.get("Conditions")
        .map(Resolved::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item.pointer("/Condition/Condition Data").cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r(v: Value) -> Resolved {
        Resolved::from_stub_json(&v)
    }

    #[test]
    fn named_extracts_name_from_enum_object() {
        assert_eq!(
            named(Some(&r(json!({"value": 31, "name": "Keywords"})))),
            json!("Keywords")
        );
        assert_eq!(
            named_json(Some(&json!({"value": 31, "name": "Keywords"}))),
            json!("Keywords")
        );
    }

    #[test]
    fn named_passes_through_non_enum_values() {
        assert_eq!(named(Some(&r(json!(1.5)))), json!(1.5));
        assert_eq!(named(None), Value::Null);
    }

    #[test]
    fn is_stub_json_detects_formid_key() {
        assert!(is_stub_json(
            &json!({"formid": "0x123", "record_type": "KYWD"})
        ));
        assert!(!is_stub_json(&json!(1.5)));
    }

    #[test]
    fn is_truthy_matches_python_semantics() {
        for (v, want) in [
            (json!(null), false),
            (json!(0), false),
            (json!(""), false),
            (json!([]), false),
            (json!({}), false),
            (json!(1), true),
            (json!("x"), true),
        ] {
            assert_eq!(is_truthy(Some(&r(v.clone()))), want, "{v}");
            assert_eq!(is_truthy_json(Some(&v)), want, "{v}");
        }
        assert!(!is_truthy(None));
        assert!(is_truthy(Some(&Resolved::unresolved(FormId(0x10)))));
    }
}
