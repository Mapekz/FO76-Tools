use super::*;

pub(super) enum PostDecodeTarget<'a> {
    Struct(&'a mut Map<String, Value>),
    Record(&'a mut Map<String, Value>),
}

/// Central registration point for FO76-specific post-decode rules.
pub(super) fn apply_post_decode_rules(target: PostDecodeTarget<'_>) {
    match target {
        PostDecodeTarget::Struct(out) => apply_crafting_quantity(out),
        PostDecodeTarget::Record(out) => apply_weapon_bash_curve(out),
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
fn apply_crafting_quantity(struct_out: &mut Map<String, Value>) {
    if !struct_out.contains_key("Curve Table") {
        return;
    }
    // Recognise both count-key spellings; stop if neither is present.
    let count = field_int_value(struct_out, "Count")
        .or_else(|| field_int_value(struct_out, "Scrap Component Count"));
    let Some(count) = count else { return };

    let (quantity, source): (Value, &str) = match struct_out.get("Curve Table") {
        // Curve inlined by `resolve_formid`: {"formid", "curve_path", "curve":[{x,y}…]}.
        Some(Value::Object(o)) => match o.get("curve").and_then(|c| c.as_array()) {
            Some(pts) if !pts.is_empty() => {
                let points: Vec<crate::curves::CurvePoint> = pts
                    .iter()
                    .filter_map(|p| {
                        Some(crate::curves::CurvePoint {
                            x: p.get("x").and_then(Value::as_f64)? as f32,
                            y: p.get("y").and_then(Value::as_f64)? as f32,
                        })
                    })
                    .collect();
                match crate::curves::eval(&points, count as f32) {
                    Some(y) => (json_f32(y), "curve"),
                    None => (serde_json::json!(count), "count"),
                }
            }
            _ => (serde_json::json!(count), "count"),
        },
        // Bare hex string: curve referenced but curves not loaded (no Startup BA2).
        Some(Value::String(_)) => (serde_json::json!(count), "count_unresolved_curve"),
        // null slot or any other shape → literal count is the effective quantity.
        _ => (serde_json::json!(count), "count"),
    };
    struct_out.insert("Quantity".to_string(), quantity);
    struct_out.insert("Quantity Source".to_string(), serde_json::json!(source));
}

/// FormID for `WeaponTypeAutomaticMelee` (KYWD `0x006D5081`), referenced by the
/// "Stable Tools" perk's `HasKeyword` condition — the game-authoritative gate for
/// power-tool bash damage scaling (Auto Axe, Chainsaw, Drill, Ripper, Buzz Blade).
const AUTOMATIC_MELEE_KEYWORD: &str = "0x006D5081";

fn automatic_melee_keyword_present(out: &Map<String, Value>) -> bool {
    let Some(keywords) = out
        .get("Keywords")
        .and_then(|v| v.get("Keywords"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    keywords.iter().any(|kw| match kw {
        Value::String(s) => s == AUTOMATIC_MELEE_KEYWORD,
        Value::Object(o) => o
            .get("formid")
            .and_then(Value::as_str)
            .is_some_and(|s| s == AUTOMATIC_MELEE_KEYWORD),
        _ => false,
    })
}

fn weapon_bash_eligible(out: &Map<String, Value>, data: &Map<String, Value>) -> bool {
    match data
        .get("Weapon Type")
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
pub(crate) fn apply_weapon_bash_curve(out: &mut Map<String, Value>) {
    let Some(data) = out.get("Data").and_then(Value::as_object) else {
        return;
    };
    let secondary = data
        .get("Secondary Damage")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if secondary == 0.0 {
        return;
    }
    let Some(damage_curve) = out.get("Damage Curve") else {
        return;
    };

    match damage_curve {
        Value::Object(o) => match o.get("curve").and_then(|c| c.as_array()) {
            Some(pts) if !pts.is_empty() => {
                let points: Vec<crate::curves::CurvePoint> = pts
                    .iter()
                    .filter_map(|p| {
                        Some(crate::curves::CurvePoint {
                            x: p.get("x").and_then(Value::as_f64)? as f32,
                            y: p.get("y").and_then(Value::as_f64)? as f32,
                        })
                    })
                    .collect();
                let reference = crate::curves::eval(&points, 1.0);
                if reference.is_none_or(|r| r <= 0.0) {
                    out.insert(
                        "Bash Damage".to_string(),
                        json!({"source": "curve_zero_reference"}),
                    );
                    return;
                }
                let reference = reference.unwrap();
                if !weapon_bash_eligible(out, data) {
                    out.insert("Bash Damage".to_string(), json!({"source": "ineligible"}));
                    return;
                }
                let curve: Vec<Value> = points
                    .iter()
                    .map(|p| {
                        json!({
                            "level": json_f32(p.x),
                            "damage": json_f32(secondary as f32 * p.y / reference),
                        })
                    })
                    .collect();
                out.insert(
                    "Bash Damage".to_string(),
                    json!({"source": "curve", "curve": curve}),
                );
            }
            _ => {}
        },
        Value::String(_) => {
            out.insert(
                "Bash Damage".to_string(),
                json!({"source": "unresolved_curve"}),
            );
        }
        _ => {}
    }
}
