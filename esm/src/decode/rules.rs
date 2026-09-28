use super::node::{Fields, Node};
use super::*;

pub(super) enum PostDecodeTarget<'a> {
    Struct(&'a mut Fields),
    Record(&'a mut Fields),
}

/// Central registration point for FO76-specific post-decode rules.
pub(super) fn apply_post_decode_rules(target: PostDecodeTarget<'_>, ctx: &DecodeContext<'_>) {
    match target {
        PostDecodeTarget::Struct(out) => apply_crafting_quantity(out, ctx),
        PostDecodeTarget::Record(out) => apply_weapon_bash_curve(out, ctx),
    }
}

/// Post-decode pass for component/scrap-quantity structs.
///
/// Runs after a struct's fields have been decoded into `struct_out`. When the
/// map contains both a recognised count key *and* a `"Curve Table"` value, this
/// function inserts:
///
/// * `"Quantity"` — the effective quantity: `curve.eval(count)` when an inlined
///   curve is available, or the raw count otherwise.
/// * `"Quantity Source"` — one of `"curve"`, `"count"`, or
///   `"count_unresolved_curve"`.
///
/// This covers the three component-array structs used in FO76:
/// * COBJ `Components` / `Repair` / `Scrap Recieved`: `"Count"` + `"Curve Table"`
/// * CMPO `Junk Scrap Quantities`: `"Scrap Component Count"` + `"Curve Table"`
///
/// Shape-gated: no-op when either key is absent (prevents touching unrelated
/// structs that coincidentally share field names). Never panics.
fn apply_crafting_quantity(struct_out: &mut Fields, ctx: &DecodeContext<'_>) {
    let Some(curve_table) = struct_out.get("Curve Table") else {
        return;
    };
    // Recognise both count-key spellings; stop if neither is present.
    let count = field_int_value(struct_out, "Count", ctx)
        .or_else(|| field_int_value(struct_out, "Scrap Component Count", ctx));
    let Some(count) = count else { return };

    let count_node = Node::Int(count as i64);
    let (quantity, source): (Node, &str) = match curve_table.to_json(ctx) {
        // Curve inlined by `render_formid`: {"formid", "curve_path", "curve":[{x,y}…]}.
        v @ Value::Object(_) => match crate::curves::points_from_json(&v) {
            Some(points) if !points.is_empty() => {
                match crate::curves::eval(&points, count as f32) {
                    Some(y) => (Node::Float(y), "curve"),
                    None => (count_node, "count"),
                }
            }
            _ => (count_node, "count"),
        },
        // Bare hex string: curve referenced but curves not loaded (no Startup BA2).
        Value::String(_) => (count_node, "count_unresolved_curve"),
        // null slot or any other shape → literal count is the effective quantity.
        _ => (count_node, "count"),
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

fn weapon_bash_eligible(out: &Fields, data: &Fields, ctx: &DecodeContext<'_>) -> bool {
    match data
        .get("Weapon Type")
        .map(|v| v.to_json(ctx))
        .as_ref()
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
    {
        Some("Gun") => true,
        _ => automatic_melee_keyword_present(out),
    }
}

/// Record-level post-decode pass for WEAP bash damage curve tables.
///
/// Synthesises `"Bash Damage"` from top-level `"Damage Curve"` and
/// `Data.Secondary Damage`. Ranged weapons (`Weapon Type` = Gun) and records
/// carrying the `WeaponTypeAutomaticMelee` keyword are eligible; others emit an
/// explicit `"ineligible"` marker when a curve is present but the weapon does not
/// qualify.
pub(crate) fn apply_weapon_bash_curve(out: &mut Fields, ctx: &DecodeContext<'_>) {
    let Some(data) = out.get("Data").and_then(Node::as_struct) else {
        return;
    };
    let secondary = data
        .get("Secondary Damage")
        .and_then(|v| v.to_json(ctx).as_f64())
        .unwrap_or(0.0);
    if secondary == 0.0 {
        return;
    }
    let Some(damage_curve) = out.get("Damage Curve").map(|v| v.to_json(ctx)) else {
        return;
    };

    let source = |s: &str| Node::obj([("source", Node::str(s))]);
    let bash = match damage_curve {
        Value::Object(_) => match crate::curves::points_from_json(&damage_curve) {
            Some(points) if !points.is_empty() => {
                let reference = crate::curves::eval(&points, 1.0);
                match reference {
                    None => source("curve_zero_reference"),
                    Some(r) if r <= 0.0 => source("curve_zero_reference"),
                    Some(_) if !weapon_bash_eligible(out, data, ctx) => source("ineligible"),
                    Some(reference) => {
                        let curve = points
                            .iter()
                            .map(|p| {
                                Node::obj([
                                    ("level", Node::Float(p.x)),
                                    ("damage", Node::Float(secondary as f32 * p.y / reference)),
                                ])
                            })
                            .collect();
                        Node::obj([
                            ("source", Node::str("curve")),
                            ("curve", Node::Array(curve)),
                        ])
                    }
                }
            }
            _ => return,
        },
        Value::String(_) => source("unresolved_curve"),
        _ => return,
    };
    out.insert("Bash Damage".to_string(), bash);
}
