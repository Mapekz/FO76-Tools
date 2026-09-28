//! Derived values: fields the decoder computes from decoded ones instead of
//! reading them from the record's bytes. Every one is registered here:
//!
//! - `Quantity` / `Quantity Source` on component structs: the component count
//!   evaluated through its curve table ([`apply_crafting_quantity`]).
//! - `Bash Damage` on WEAP: the weapon's bash damage per level
//!   ([`apply_weapon_bash_curve`]).
//! - Curve points for a curve table: inlined on a reference to it
//!   ([`curve_inline`]) and on a CURV record's own tree
//!   ([`curve_points_node`]).
//!
//! Curve points come from the loaded curve index (`DecodeEnv::curves`),
//! never from rendered output.

use serde_json::{Value, json};

use super::DecodeContext;
use super::node::{Fields, Node};
use super::scalars::{field_int_value, json_f32};
use crate::curves::{ArchivedCurve, CurvePoint};
use crate::formid::FormId;

pub(super) enum PostDecodeTarget<'a> {
    Struct(&'a mut Fields),
    Record(&'a mut Fields),
}

/// Add the derived values that belong on a just-decoded struct or record.
pub(super) fn apply_post_decode_rules(target: PostDecodeTarget<'_>, ctx: &DecodeContext<'_>) {
    match target {
        PostDecodeTarget::Struct(out) => apply_crafting_quantity(out, ctx),
        PostDecodeTarget::Record(out) => apply_weapon_bash_curve(out, ctx),
    }
}

/// `{"formid", "editor_id", "curve_path", "curve"}`: a curve-table reference
/// with its points inlined.
pub(crate) fn curve_inline(id: FormId, curve: &ArchivedCurve) -> Value {
    json!({
        "formid": id.display(),
        "editor_id": curve.edid(),
        "curve_path": curve.path(),
        "curve": curve
            .points()
            .iter()
            .map(|p| json!({"x": json_f32(p.x), "y": json_f32(p.y)}))
            .collect::<Vec<_>>(),
    })
}

/// A CURV record's points as `[{"x", "y"}]`, the same shape
/// [`curve_inline`] renders.
pub(crate) fn curve_points_node(curve: &ArchivedCurve) -> Node {
    Node::Array(
        curve
            .points()
            .iter()
            .map(|p| Node::obj([("x", Node::Float(p.x)), ("y", Node::Float(p.y))]))
            .collect(),
    )
}

/// What a curve-table field refers to. A curve the loaded index doesn't
/// hold is unresolved at every resolve depth.
enum CurveRef {
    /// No reference (absent, null or not a FormID).
    None,
    /// A curve the loaded index doesn't hold (or no index is loaded).
    Unresolved,
    Points(Vec<CurvePoint>),
}

fn curve_ref(field: Option<&Node>, ctx: &DecodeContext<'_>) -> CurveRef {
    match field {
        Some(Node::FormId { id, .. }) if id.0 != 0 => {
            match ctx.curves.and_then(|curves| curves.get(*id)) {
                Some(curve) => CurveRef::Points(curve.points()),
                None => CurveRef::Unresolved,
            }
        }
        _ => CurveRef::None,
    }
}

/// Component-quantity structs: when a struct holds a count and a
/// `"Curve Table"`, insert:
///
/// * `"Quantity"` — the effective quantity: `curve.eval(count)` when the
///   curve is loaded, or the raw count otherwise.
/// * `"Quantity Source"` — one of `"curve"`, `"count"`, or
///   `"count_unresolved_curve"`.
///
/// This covers the three component-array structs used in FO76:
/// * COBJ `Components` / `Repair` / `Scrap Recieved`: `"Count"` + `"Curve Table"`
/// * CMPO `Junk Scrap Quantities`: `"Scrap Component Count"` + `"Curve Table"`
///
/// Shape-gated: no-op when either key is absent (prevents touching unrelated
/// structs that coincidentally share field names).
fn apply_crafting_quantity(struct_out: &mut Fields, ctx: &DecodeContext<'_>) {
    let Some(curve_table) = struct_out.get("Curve Table") else {
        return;
    };
    let count = field_int_value(struct_out, "Count", ctx)
        .or_else(|| field_int_value(struct_out, "Scrap Component Count", ctx));
    let Some(count) = count else { return };

    let count_node = Node::Int(count as i64);
    let (quantity, source) = match curve_ref(Some(curve_table), ctx) {
        CurveRef::Points(points) => match crate::curves::eval(&points, count as f32) {
            Some(y) => (Node::Float(y), "curve"),
            None => (count_node, "count"),
        },
        CurveRef::Unresolved => (count_node, "count_unresolved_curve"),
        CurveRef::None => (count_node, "count"),
    };
    struct_out.insert("Quantity".to_string(), quantity);
    struct_out.insert("Quantity Source".to_string(), Node::str(source));
}

/// FormID for `WeaponTypeAutomaticMelee` (KYWD `0x006D5081`), referenced by the
/// "Stable Tools" perk's `HasKeyword` condition — the game-authoritative gate for
/// power-tool bash damage scaling (Auto Axe, Chainsaw, Drill, Ripper, Buzz Blade).
const AUTOMATIC_MELEE_KEYWORD: FormId = FormId(0x006D5081);

fn automatic_melee_keyword_present(out: &Fields) -> bool {
    let Some(keywords) = out
        .get("Keywords")
        .and_then(|v| v.get("Keywords"))
        .and_then(Node::as_array)
    else {
        return false;
    };
    keywords
        .iter()
        .any(|kw| matches!(kw, Node::FormId { id, .. } if *id == AUTOMATIC_MELEE_KEYWORD))
}

fn weapon_bash_eligible(out: &Fields, data: &Fields) -> bool {
    match data.get("Weapon Type") {
        Some(Node::Enum { name, .. }) if name == "Gun" => true,
        _ => automatic_melee_keyword_present(out),
    }
}

/// WEAP bash damage: synthesises `"Bash Damage"` from top-level
/// `"Damage Curve"` and `Data.Secondary Damage`. Ranged weapons (`Weapon Type`
/// = Gun) and records carrying the `WeaponTypeAutomaticMelee` keyword are
/// eligible; others emit an explicit `"ineligible"` marker when a curve is
/// present but the weapon does not qualify.
pub(crate) fn apply_weapon_bash_curve(out: &mut Fields, ctx: &DecodeContext<'_>) {
    let Some(data) = out.get("Data").and_then(Node::as_struct) else {
        return;
    };
    let secondary = match data.get("Secondary Damage") {
        Some(Node::Float(f)) => *f,
        Some(Node::Int(i)) => *i as f32,
        _ => 0.0,
    };
    if secondary == 0.0 {
        return;
    }

    let source = |s: &str| Node::obj([("source", Node::str(s))]);
    let bash = match curve_ref(out.get("Damage Curve"), ctx) {
        CurveRef::Points(points) if !points.is_empty() => match crate::curves::eval(&points, 1.0) {
            None => source("curve_zero_reference"),
            Some(r) if r <= 0.0 => source("curve_zero_reference"),
            Some(_) if !weapon_bash_eligible(out, data) => source("ineligible"),
            Some(reference) => {
                let curve = points
                    .iter()
                    .map(|p| {
                        Node::obj([
                            ("level", Node::Float(p.x)),
                            ("damage", Node::Float(secondary * p.y / reference)),
                        ])
                    })
                    .collect();
                Node::obj([
                    ("source", Node::str("curve")),
                    ("curve", Node::Array(curve)),
                ])
            }
        },
        CurveRef::Unresolved => source("unresolved_curve"),
        CurveRef::Points(_) | CurveRef::None => return,
    };
    out.insert("Bash Damage".to_string(), bash);
}
