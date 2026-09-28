//! The decoder's typed value tree.
//!
//! Decoding builds a [`Node`] tree; JSON exists only at the boundary
//! ([`Node::into_json`]). Values whose rendering depends on the caller's
//! `--resolve` depth or on loaded side data — FormID references and localized
//! string ids — stay typed in the tree and are resolved while rendering, so
//! consumers that walk the tree read FormIDs and integers directly instead of
//! parsing them back out of strings.

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use super::{DecodeContext, hex, markers, render_formid};
use crate::formid::FormId;
use crate::strings::StringKind;

/// A struct's decoded fields, in decode order.
pub type Fields = IndexMap<String, Node>;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    Int(i64),
    /// An `f32` game value; renders without f32→f64 widening noise.
    Float(f32),
    Str(String),
    /// An enum-formatted integer with a known name: `{"value", "name"}`.
    Enum {
        value: i64,
        name: String,
    },
    /// A flags-formatted integer: `{"value": "0x…", "flags": [set names]}`.
    Flags {
        value: u64,
        set: Vec<String>,
    },
    /// A FormID reference. `curve` marks a field whose valid refs include a
    /// curve-table leaf, which inlines curve points at every resolve depth.
    FormId {
        id: FormId,
        curve: bool,
    },
    /// A localized string id, looked up in `kind`'s table when rendered.
    LString {
        id: u32,
        kind: StringKind,
    },
    /// Opaque bytes: `{"hex"}`.
    Bytes(Vec<u8>),
    /// Bytes the schema couldn't decode: `{"hex"?, "_raw": true, "reason"?}`.
    Raw {
        bytes: Option<Vec<u8>>,
        reason: Option<String>,
    },
    Struct(Fields),
    Array(Vec<Node>),
}

impl Node {
    /// A struct node from `(key, value)` pairs, in order.
    pub fn obj<const N: usize>(pairs: [(&str, Node); N]) -> Node {
        Node::Struct(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    pub fn str(s: impl Into<String>) -> Node {
        Node::Str(s.into())
    }

    pub fn int(v: impl Into<i64>) -> Node {
        Node::Int(v.into())
    }

    /// `{"hex", "_raw": true}`.
    pub fn raw(bytes: &[u8]) -> Node {
        Node::Raw {
            bytes: Some(bytes.to_vec()),
            reason: None,
        }
    }

    /// `{"hex", "_raw": true, "reason"}`, or `{"_raw": true, "reason"}`
    /// without bytes.
    pub fn raw_reason(bytes: Option<&[u8]>, reason: impl Into<String>) -> Node {
        Node::Raw {
            bytes: bytes.map(<[u8]>::to_vec),
            reason: Some(reason.into()),
        }
    }

    pub fn as_struct(&self) -> Option<&Fields> {
        match self {
            Node::Struct(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Node]> {
        match self {
            Node::Array(items) => Some(items),
            _ => None,
        }
    }

    /// A struct field by name.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_struct()?.get(key)
    }

    /// Render the tree to its boundary JSON, resolving FormIDs and localized
    /// strings through `ctx`.
    pub fn into_json(self, ctx: &DecodeContext<'_>) -> Value {
        match self {
            Node::Null => Value::Null,
            Node::Bool(b) => Value::Bool(b),
            Node::Int(v) => json!(v),
            Node::Float(f) => super::json_f32(f),
            Node::Str(s) => Value::String(s),
            Node::Enum { value, name } => json!({"value": value, "name": name}),
            Node::Flags { value, set } => json!({"value": format!("0x{value:X}"), "flags": set}),
            Node::FormId { id, curve } => render_formid(ctx, curve, id),
            Node::LString { id, kind } => {
                match ctx.localization.and_then(|loc| loc.lookup(kind, id)) {
                    Some(text) => json!(text),
                    None => json!({
                        "lstring_id": format!("0x{id:08X}"),
                        (markers::UNRESOLVED): true
                    }),
                }
            }
            Node::Bytes(bytes) => json!({"hex": hex::encode(&bytes)}),
            Node::Raw { bytes, reason } => {
                let mut map = Map::new();
                if let Some(bytes) = bytes {
                    map.insert("hex".into(), json!(hex::encode(&bytes)));
                }
                map.insert(markers::RAW.into(), json!(true));
                if let Some(reason) = reason {
                    map.insert("reason".into(), json!(reason));
                }
                Value::Object(map)
            }
            Node::Struct(fields) => Value::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, v.into_json(ctx)))
                    .collect(),
            ),
            Node::Array(items) => {
                Value::Array(items.into_iter().map(|v| v.into_json(ctx)).collect())
            }
        }
    }

    /// Render a copy of this node; see [`Node::into_json`].
    pub fn to_json(&self, ctx: &DecodeContext<'_>) -> Value {
        self.clone().into_json(ctx)
    }
}

impl From<bool> for Node {
    fn from(b: bool) -> Node {
        Node::Bool(b)
    }
}

impl From<i32> for Node {
    fn from(v: i32) -> Node {
        Node::Int(v.into())
    }
}

/// Insert `value` under `key`; if `key` is taken, under `"key 2"`, `"key 3"`, …
///
/// Schema patterns reuse one `wbXxx` definition for two slots of a struct
/// (e.g. MGEF's two `wbActorValue` fields), and the second must not clobber
/// the first.
pub(crate) fn insert_unique(fields: &mut Fields, key: String, value: Node) {
    if !fields.contains_key(&key) {
        fields.insert(key, value);
        return;
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{key} {n}");
        if !fields.contains_key(&candidate) {
            fields.insert(candidate, value);
            return;
        }
        n += 1;
    }
}
