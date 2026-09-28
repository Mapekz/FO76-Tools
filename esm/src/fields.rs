//! Readers for decoded record JSON shared by chase, walk and the drop table:
//! schema enums, FormID stubs at [`crate::ResolveDepth::Stub`], condition
//! rows, and Python-style truthiness.

use serde_json::Value;

use crate::FormId;

/// Extract the human name from a `{"value":.., "name":..}` schema enum
/// object, or return the value unchanged if it isn't wrapped that way.
pub(crate) fn named(field: Option<&Value>) -> Value {
    match field {
        Some(Value::Object(map)) => map
            .get("name")
            .cloned()
            .unwrap_or_else(|| field.cloned().unwrap_or(Value::Null)),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// Python-truthiness for a JSON value (`None`/`0`/`""`/`[]`/`{}`/`false` are
/// falsy, as in a bare Python `if x:`).
pub(crate) fn is_truthy(v: Option<&Value>) -> bool {
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

/// A decoded FormID reference at [`crate::ResolveDepth::Stub`] is a
/// `{"formid", "editor_id", "record_type"}` object.
pub(crate) fn is_ref_stub(v: &Value) -> bool {
    matches!(v, Value::Object(map) if map.contains_key("formid"))
}

/// The FormID of a reference stub.
pub(crate) fn stub_formid(v: Option<&Value>) -> Option<FormId> {
    let obj = v?.as_object()?;
    let s = obj.get("formid")?.as_str()?;
    crate::parse_form_id_input(s).ok()
}

pub(crate) fn dedup_sorted(fids: &mut Vec<FormId>) {
    fids.sort_by_key(|f| f.0);
    fids.dedup();
}

/// Pull the flat condition rows out of a SPEL/ENCH/ALCH/MGEF-style
/// `Conditions` node; LVLI entries decode `Conditions` into the same shape.
pub(crate) fn flatten_condition_rows(node: &Value) -> Vec<Value> {
    node.get("Conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.pointer("/Condition/Condition Data").cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn named_extracts_name_from_enum_object() {
        let v = json!({"value": 31, "name": "Keywords"});
        assert_eq!(named(Some(&v)), json!("Keywords"));
    }

    #[test]
    fn named_passes_through_non_enum_values() {
        assert_eq!(named(Some(&json!(1.5))), json!(1.5));
        assert_eq!(named(None), Value::Null);
    }

    #[test]
    fn is_ref_stub_detects_formid_key() {
        assert!(is_ref_stub(
            &json!({"formid": "0x123", "record_type": "KYWD"})
        ));
        assert!(!is_ref_stub(&json!(1.5)));
    }

    #[test]
    fn is_truthy_matches_python_semantics() {
        assert!(!is_truthy(None));
        assert!(!is_truthy(Some(&Value::Null)));
        assert!(!is_truthy(Some(&json!(0))));
        assert!(!is_truthy(Some(&json!(""))));
        assert!(!is_truthy(Some(&json!([]))));
        assert!(!is_truthy(Some(&json!({}))));
        assert!(is_truthy(Some(&json!(1))));
        assert!(is_truthy(Some(&json!("x"))));
    }
}
