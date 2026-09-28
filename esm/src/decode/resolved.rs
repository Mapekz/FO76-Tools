//! Record bodies for the analysis layers (chase, walk, the drop table).
//!
//! A [`Resolved`] tree is a decoded record rendered at
//! [`super::ResolveDepth::Stub`], except that every FormID reference stays a
//! [`Resolved::Ref`] carrying its [`FormId`] beside what it renders as. The
//! analysis layers read reference identity from [`Resolved::ref_id`] and
//! [`Resolved::stub_id`], never from a rendered string, and
//! [`Resolved::to_json`] renders exactly what `--resolve stub` prints.

use indexmap::IndexMap;
use serde_json::{Number, Value};

use super::node::Node;
use super::{DecodeContext, render_formid};
use crate::formid::FormId;

/// A struct's fields, in decode order.
pub type ResolvedFields = IndexMap<String, Resolved>;

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Resolved>),
    Object(ResolvedFields),
    /// A non-null FormID reference. `body` is what it renders as: the stub
    /// object (`formid`/`editor_id`/`record_type`, plus a value-bearing
    /// leaf's payload such as a GLOB's `Value` or a curve's points) when the
    /// target resolves, the bare hex FormID string when it doesn't.
    Ref {
        id: FormId,
        body: Box<Resolved>,
    },
}

static NULL: Resolved = Resolved::Null;

impl Resolved {
    /// A reference to `id` that renders as `rendered`; a `null` rendering
    /// (the null FormID) is [`Resolved::Null`].
    pub fn reference(id: FormId, rendered: Value) -> Resolved {
        if rendered.is_null() {
            return Resolved::Null;
        }
        Resolved::Ref {
            id,
            body: Box::new(Resolved::plain(rendered)),
        }
    }

    /// A reference to `id` whose target doesn't resolve: it renders as the
    /// bare hex FormID.
    pub fn unresolved(id: FormId) -> Resolved {
        Resolved::reference(id, Value::String(id.display()))
    }

    /// `value` as a tree with no references: strings and objects stay what
    /// they are.
    pub fn plain(value: Value) -> Resolved {
        match value {
            Value::Null => Resolved::Null,
            Value::Bool(b) => Resolved::Bool(b),
            Value::Number(n) => Resolved::Number(n),
            Value::String(s) => Resolved::String(s),
            Value::Array(items) => {
                Resolved::Array(items.into_iter().map(Resolved::plain).collect())
            }
            Value::Object(map) => Resolved::Object(
                map.into_iter()
                    .map(|(k, v)| (k, Resolved::plain(v)))
                    .collect(),
            ),
        }
    }

    /// Read record fields supplied as `--resolve stub` JSON (a
    /// [`crate::source::MemorySource`]'s records): an object with a `formid`
    /// FormID string is a resolved reference stub. An unresolved reference
    /// can't be told apart from any other string here; build it with
    /// [`Resolved::unresolved`].
    pub fn from_stub_json(value: &Value) -> Resolved {
        match value {
            Value::Object(map) => {
                let id = map
                    .get("formid")
                    .and_then(Value::as_str)
                    .and_then(|s| crate::parse_form_id_input(s).ok());
                match id {
                    Some(id) => Resolved::Ref {
                        id,
                        body: Box::new(Resolved::plain(value.clone())),
                    },
                    None => Resolved::Object(
                        map.iter()
                            .map(|(k, v)| (k.clone(), Resolved::from_stub_json(v)))
                            .collect(),
                    ),
                }
            }
            Value::Array(items) => {
                Resolved::Array(items.iter().map(Resolved::from_stub_json).collect())
            }
            other => Resolved::plain(other.clone()),
        }
    }

    /// The value a reference renders as; any other node itself.
    fn target(&self) -> &Resolved {
        match self {
            Resolved::Ref { body, .. } => body,
            other => other,
        }
    }

    /// The FormID of a reference, resolved or not.
    pub fn ref_id(&self) -> Option<FormId> {
        match self {
            Resolved::Ref { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// The FormID of a reference whose target resolved to a stub.
    pub fn stub_id(&self) -> Option<FormId> {
        match self {
            Resolved::Ref { id, body } if matches!(**body, Resolved::Object(_)) => Some(*id),
            _ => None,
        }
    }

    /// Whether this is a reference whose target resolved to a stub.
    pub fn is_stub(&self) -> bool {
        self.stub_id().is_some()
    }

    /// An object field, looked up through a reference's stub.
    pub fn get(&self, key: &str) -> Option<&Resolved> {
        match self.target() {
            Resolved::Object(map) => map.get(key),
            _ => None,
        }
    }

    /// The value at a JSON Pointer (RFC 6901), as `serde_json::Value::pointer`.
    pub fn pointer(&self, pointer: &str) -> Option<&Resolved> {
        if pointer.is_empty() {
            return Some(self);
        }
        let rest = pointer.strip_prefix('/')?;
        let mut cur = self;
        for token in rest.split('/') {
            let token = token.replace("~1", "/").replace("~0", "~");
            cur = match cur.target() {
                Resolved::Object(map) => map.get(&token)?,
                Resolved::Array(items) => {
                    if token.starts_with('+') || (token.len() > 1 && token.starts_with('0')) {
                        return None;
                    }
                    items.get(token.parse::<usize>().ok()?)?
                }
                _ => return None,
            };
        }
        Some(cur)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.target() {
            Resolved::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self.target() {
            Resolved::Number(n) => n.as_f64(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self.target() {
            Resolved::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self.target() {
            Resolved::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.target() {
            Resolved::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Resolved>> {
        match self.target() {
            Resolved::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&ResolvedFields> {
        match self.target() {
            Resolved::Object(map) => Some(map),
            _ => None,
        }
    }

    /// The elements of an array; empty for anything else.
    pub fn items(&self) -> &[Resolved] {
        self.as_array().map_or(&[], Vec::as_slice)
    }

    pub fn is_null(&self) -> bool {
        matches!(self.target(), Resolved::Null)
    }

    pub fn is_object(&self) -> bool {
        matches!(self.target(), Resolved::Object(_))
    }

    /// The JSON this renders as: `--resolve stub` output.
    pub fn to_json(&self) -> Value {
        match self {
            Resolved::Null => Value::Null,
            Resolved::Bool(b) => Value::Bool(*b),
            Resolved::Number(n) => Value::Number(n.clone()),
            Resolved::String(s) => Value::String(s.clone()),
            Resolved::Array(items) => Value::Array(items.iter().map(Resolved::to_json).collect()),
            Resolved::Object(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
            }
            Resolved::Ref { body, .. } => body.to_json(),
        }
    }
}

impl std::ops::Index<&str> for Resolved {
    type Output = Resolved;

    /// An object field, or `null` when absent (as `serde_json::Value`).
    fn index(&self, key: &str) -> &Resolved {
        self.get(key).unwrap_or(&NULL)
    }
}

impl Node {
    /// Render the tree for the analysis layers: as [`Node::into_json`] at
    /// `ctx`'s resolve depth, keeping every FormID reference typed (see
    /// [`Resolved`]).
    pub fn into_resolved(self, ctx: &DecodeContext<'_>) -> Resolved {
        match self {
            Node::FormId { id, curve } => Resolved::reference(id, render_formid(ctx, curve, id)),
            Node::Struct(fields) => Resolved::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, v.into_resolved(ctx)))
                    .collect(),
            ),
            Node::Array(items) => {
                Resolved::Array(items.into_iter().map(|v| v.into_resolved(ctx)).collect())
            }
            other => Resolved::plain(other.into_json(ctx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_stub_reads_like_its_json_and_keeps_its_id() {
        let stub =
            json!({"formid": "0x00000010", "editor_id": "E", "record_type": "GLOB", "Value": 2.5});
        let r = Resolved::from_stub_json(&json!({"G": stub, "n": 1}));
        let g = r.get("G").unwrap();
        assert_eq!(g.stub_id(), Some(FormId(0x10)));
        assert_eq!(g.get("Value").and_then(Resolved::as_f64), Some(2.5));
        assert_eq!(r.to_json(), json!({"G": stub, "n": 1}));
    }

    #[test]
    fn an_unresolved_reference_is_a_ref_but_not_a_stub() {
        let r = Resolved::unresolved(FormId(0xDEAD02));
        assert_eq!(r.ref_id(), Some(FormId(0xDEAD02)));
        assert_eq!(r.stub_id(), None);
        assert_eq!(r.as_str(), Some("0x00DEAD02"));
        assert_eq!(r.to_json(), json!("0x00DEAD02"));
    }

    #[test]
    fn pointer_and_index_match_serde_json() {
        let v = json!({"a/b": [{"c": 1}], "x": {"y": null}});
        let r = Resolved::from_stub_json(&v);
        for p in [
            "",
            "/a~1b",
            "/a~1b/0/c",
            "/a~1b/01",
            "/x/y",
            "/missing",
            "/a~1b/5",
        ] {
            assert_eq!(
                r.pointer(p).map(Resolved::to_json),
                v.pointer(p).cloned(),
                "{p}"
            );
        }
        assert!(r["missing"].is_null());
    }
}
