//! Which values in a record's `field_changes` are FormID references.
//!
//! A FormID field renders as a `0x…` string, and so do hashes and some
//! flag values, so the rendering alone can't say which is which. The
//! decoder's typed tree can: [`changed_refs`] walks a `json_diff` of two
//! decoded records in step with both sides' JSON and [`Node`] trees, and
//! collects the FormIDs of the typed references the diff touches.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{Map, Value};

use crate::decode::node::Node;
use crate::formid::FormId;

/// One side of a diff at some location: the rendered JSON there and the
/// typed node it rendered from.
#[derive(Clone, Copy, Default)]
pub(super) struct Side<'a> {
    json: Option<&'a Value>,
    node: Option<&'a Node>,
}

impl<'a> Side<'a> {
    pub(super) fn new(json: &'a Value, node: &'a Node) -> Self {
        Side {
            json: Some(json),
            node: Some(node),
        }
    }

    fn field(self, key: &str) -> Side<'a> {
        Side {
            json: self.json.and_then(|j| j.get(key)),
            node: self.node.and_then(|n| n.get(key)),
        }
    }

    fn element(self, index: usize) -> Side<'a> {
        Side {
            json: self.json.and_then(|j| j.get(index)),
            node: self
                .node
                .and_then(Node::as_array)
                .and_then(|items| items.get(index)),
        }
    }

    /// The location at a dot-separated field path.
    fn path(self, path: &str) -> Side<'a> {
        path.split('.').fold(self, |side, key| side.field(key))
    }

    /// The reference this location holds, if it is a FormID field.
    fn formid(self) -> Option<FormId> {
        match self.node? {
            Node::FormId { id, .. } if !id.is_null() => Some(*id),
            _ => None,
        }
    }

    /// Every reference at or under this location.
    fn all_formids(self, out: &mut HashSet<FormId>) {
        if let Some(node) = self.node {
            node.for_each_formid(&mut |id| {
                if !id.is_null() {
                    out.insert(id);
                }
            });
        }
    }
}

/// The FormIDs of the typed references `diff` (a `json_diff` of `a` and
/// `b`, possibly pruned by noise suppression) touches on either side.
pub(super) fn changed_refs(diff: &Value, a: Side<'_>, b: Side<'_>) -> HashSet<FormId> {
    let mut out = HashSet::new();
    walk(diff, a, b, &mut out);
    out
}

fn is_leaf_change(map: &Map<String, Value>) -> bool {
    map.len() == 2 && map.contains_key("from") && map.contains_key("to")
}

fn walk(diff: &Value, a: Side<'_>, b: Side<'_>, out: &mut HashSet<FormId>) {
    let Value::Object(map) = diff else {
        return;
    };
    if is_leaf_change(map) {
        a.all_formids(out);
        b.all_formids(out);
        return;
    }
    if let Some(Value::Object(envelope)) = map.get("_array_diff") {
        walk_array(envelope, a, b, out);
        return;
    }
    // A reference that renders as an object (a curve inlined onto its
    // FormID) changes target when its `formid` does.
    if map.contains_key("formid") {
        out.extend(a.formid());
        out.extend(b.formid());
    }
    for (key, child) in map {
        walk(child, a.field(key), b.field(key), out);
    }
}

fn walk_array(envelope: &Map<String, Value>, a: Side<'_>, b: Side<'_>, out: &mut HashSet<FormId>) {
    for change in envelope
        .get("changed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let index = |key: &str| {
            change
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|i| usize::try_from(i).ok())
        };
        let (Some(from), Some(to), Some(changes)) = (
            index("index_from"),
            index("index_to"),
            change.get("changes"),
        ) else {
            continue;
        };
        // A keyed pair names its element by key fields, references included.
        for name in change
            .get("key")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
        {
            out.extend(key_body(a.element(from)).path(name).formid());
            out.extend(key_body(b.element(to)).path(name).formid());
        }
        walk(changes, a.element(from), b.element(to), out);
    }
    for (key, side, other) in [("removed", a, b), ("added", b, a)] {
        let Some(elements) = envelope.get(key).and_then(Value::as_array) else {
            continue;
        };
        let mut positions = element_positions(side, other);
        for element in elements {
            let found = positions
                .get_mut(&element.to_string())
                .and_then(VecDeque::pop_front);
            if let Some(index) = found {
                side.element(index).all_formids(out);
            }
        }
    }
}

/// Where a keyed diff reads an element's key fields: inside its single-member
/// wrapper object (`{"Navmesh Info": {..}}`), as `array_diff` keys it.
fn key_body(element: Side<'_>) -> Side<'_> {
    match element.json.and_then(Value::as_object) {
        Some(map) if map.len() == 1 => match map.iter().next() {
            Some((wrapper, Value::Object(_))) => element.field(wrapper),
            _ => element,
        },
        _ => element,
    }
}

/// The indices of `side`'s array elements, grouped by their rendering, so
/// an added or removed element (a copy of one) finds where it came from.
/// Elements that render alike can differ in type (a string and a FormID
/// both render `"0x…"`), so within a rendering the positions whose typed
/// content `other` has fewer of — the ones the diff can have added or
/// removed — come first.
fn element_positions(side: Side<'_>, other: Side<'_>) -> HashMap<String, VecDeque<usize>> {
    let elements = |s: Side<'_>| {
        let count = s.json.and_then(Value::as_array).map_or(0, Vec::len);
        (0..count)
            .map(|i| {
                let e = s.element(i);
                let rendering = e.json.map(Value::to_string).unwrap_or_default();
                let typed = e.node.map(|n| format!("{n:?}")).unwrap_or_default();
                (rendering, typed)
            })
            .collect::<Vec<_>>()
    };
    let mut other_counts: HashMap<(String, String), usize> = HashMap::new();
    for key in elements(other) {
        *other_counts.entry(key).or_default() += 1;
    }
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut unmatched: HashMap<String, VecDeque<usize>> = HashMap::new();
    let mut matched: HashMap<String, VecDeque<usize>> = HashMap::new();
    for (i, key) in elements(side).into_iter().enumerate() {
        let n = seen.entry(key.clone()).or_default();
        let bucket = if *n >= other_counts.get(&key).copied().unwrap_or(0) {
            &mut unmatched
        } else {
            &mut matched
        };
        *n += 1;
        bucket.entry(key.0).or_default().push_back(i);
    }
    for (rendering, rest) in matched {
        unmatched.entry(rendering).or_default().extend(rest);
    }
    unmatched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::json_diff;
    use serde_json::json;

    fn formid(raw: u32) -> Node {
        Node::FormId {
            id: FormId(raw),
            curve: false,
        }
    }

    /// A record with a keyword reference and one texture hash, as a typed
    /// tree and the JSON it renders at `--resolve none`.
    fn record(keyword: u32, hash: &str) -> (Value, Node) {
        let node = Node::obj([
            ("Keywords", Node::Array(vec![formid(keyword)])),
            (
                "Textures",
                Node::Array(vec![Node::obj([("File Hash", Node::str(hash))])]),
            ),
        ]);
        let json = json!({
            "Keywords": [FormId(keyword).display()],
            "Textures": [{"File Hash": hash}],
        });
        (json, node)
    }

    #[test]
    fn a_hash_equal_to_an_unchanged_reference_is_not_a_changed_ref() {
        let (ja, na) = record(0x1234_5678, "0x12345679");
        let (jb, nb) = record(0x1234_5678, "0x12345678");
        let diff = json_diff(&ja, &jb);
        assert!(diff.get("Textures").is_some(), "{diff}");
        assert!(changed_refs(&diff, Side::new(&ja, &na), Side::new(&jb, &nb)).is_empty());
    }

    #[test]
    fn a_changed_reference_is_collected_on_both_sides() {
        let (ja, na) = record(0x10, "0x0");
        let (jb, nb) = record(0x20, "0x0");
        let diff = json_diff(&ja, &jb);
        assert_eq!(
            changed_refs(&diff, Side::new(&ja, &na), Side::new(&jb, &nb)),
            HashSet::from([FormId(0x10), FormId(0x20)])
        );
    }

    #[test]
    fn a_keyed_elements_key_reference_is_collected() {
        let cell = |world: u32, x: i64| {
            (
                json!({"World": FormId(world).display(), "X": x}),
                Node::obj([("World", formid(world)), ("X", Node::int(x))]),
            )
        };
        let wrap = |elems: Vec<(Value, Node)>| {
            let (json, nodes): (Vec<Value>, Vec<Node>) = elems
                .into_iter()
                .map(|(j, n)| (json!({"Cell": j}), Node::obj([("Cell", n)])))
                .unzip();
            (
                json!({"Cells": json}),
                Node::obj([("Cells", Node::Array(nodes))]),
            )
        };
        let (ja, na) = wrap(vec![cell(0x10, 1), cell(0x11, 1)]);
        let (jb, nb) = wrap(vec![cell(0x10, 2), cell(0x11, 1)]);
        let diff = json_diff(&ja, &jb);
        assert_eq!(
            diff["Cells"]["_array_diff"]["strategy"],
            json!("keyed"),
            "{diff}"
        );
        assert_eq!(
            changed_refs(&diff, Side::new(&ja, &na), Side::new(&jb, &nb)),
            HashSet::from([FormId(0x10)])
        );
    }

    /// A string and a FormID that render alike: the diff's removed
    /// `"0x12345678"` is the one of the two that the other side lacks.
    #[test]
    fn a_removed_element_is_told_from_a_kept_one_that_renders_alike() {
        let text = || Node::str("0x12345678");
        let both = (
            json!({"Items": ["0x12345678", "0x12345678"]}),
            Node::obj([("Items", Node::Array(vec![text(), formid(0x1234_5678)]))]),
        );
        for (kept, want) in [
            (text(), HashSet::from([FormId(0x1234_5678)])),
            (formid(0x1234_5678), HashSet::new()),
        ] {
            let one = (
                json!({"Items": ["0x12345678"]}),
                Node::obj([("Items", Node::Array(vec![kept]))]),
            );
            let diff = json_diff(&both.0, &one.0);
            assert!(diff.get("Items").is_some(), "{diff}");
            assert_eq!(
                changed_refs(
                    &diff,
                    Side::new(&both.0, &both.1),
                    Side::new(&one.0, &one.1)
                ),
                want
            );
        }
    }

    #[test]
    fn a_changed_leaf_collects_only_its_own_reference() {
        let node = |target: u32, n: i64| {
            Node::obj([
                ("Target", formid(target)),
                ("Other", formid(0x99)),
                ("Count", Node::int(n)),
            ])
        };
        let json = |target: u32, n: i64| json!({"Target": FormId(target).display(), "Other": FormId(0x99).display(), "Count": n});
        let (ja, na, jb, nb) = (json(0x10, 1), node(0x10, 1), json(0x20, 2), node(0x20, 2));
        let diff = json_diff(&ja, &jb);
        assert_eq!(
            changed_refs(&diff, Side::new(&ja, &na), Side::new(&jb, &nb)),
            HashSet::from([FormId(0x10), FormId(0x20)])
        );
    }
}
