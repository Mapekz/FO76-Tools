//! CTDA (Condition Data) subrecord decoder.
//!
//! Decodes the 32-byte CTDA struct: extracts the comparison operator from the
//! type byte, resolves the function index to a human-readable name, and decodes
//! each parameter field according to the function's declared parameter types.

use crate::decode::{hex, json_f32, resolve_formid, DecodeContext};
use crate::formid::FormId;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

// Comparison operators encoded in bits 5-7 of the CTDA type byte.
const OPERATORS: [&str; 6] = [
    "Equal To",
    "Not Equal To",
    "Greater Than",
    "Greater Than Or Equal To",
    "Less Than",
    "Less Than Or Equal To",
];

const RUN_ON: [&str; 17] = [
    "Subject",
    "Target",
    "Reference",
    "Combat Target",
    "Linked Reference",
    "Quest Alias",
    "Package Data",
    "Event Data",
    "Unknown 8",
    "Command Target",
    "Event Camera Ref",
    "My Killer",
    "Active Players",
    "Potential Players",
    "Player Teammates",
    "Target List",
    "Instance Owner",
];

// Parameter classification:
//   'N' = None (no param / skip)
//   'F' = Float (f32)
//   'I' = Integer (i32)
//   'S' = String (comes from CIS1/CIS2, not the 4-byte field)
//   'A' = ActorValue (u32; not yet named)
//   'R' = FormID reference
//   'T' = Form Type (u32 index into CONDITION_FORM_TYPES; used by GetIsObjectType, GetIsUsedItemType)
//
// Table is sorted by function index for binary search.

// wbConditionFormTypeEnum from wbDefinitionsFO76.pas
const CONDITION_FORM_TYPES: &[&str] = &[
    "Activator",
    "Armor",
    "Book",
    "Container",
    "Door",
    "Ingredient",
    "Light",
    "Misc Item",
    "Static Object",
    "Grass",
    "Tree",
    "Unknown 11",
    "Weapon",
    "Non-Player Character",
    "Leveled NPC",
    "Spell",
    "Enchantment",
    "Ingestible",
    "Leveled Item",
    "Key",
    "Ammunition",
    "Flora",
    "Furniture",
    "Sound Marker",
    "Land Texture",
    "Combat Style",
    "Load Screen",
    "Leveled Spell",
    "Animated Object",
    "Water",
    "Idle Marker",
    "Effect Shader",
    "Projectile",
    "Talking Activator",
    "Explosion",
    "Texture Set",
    "Debris",
    "Menu Icon",
    "List Form",
    "Perk",
    "Perk Card",
    "Body Part Data",
    "Addon Node",
    "Movable Static",
    "Camera Shot",
    "Impact Data",
    "Impact Data Set",
    "Quest",
    "Unknown 48",
    "Voice Type",
    "Class",
    "Race",
    "Eyes",
    "Head Part",
    "Faction",
    "Holotape",
    "Weather",
    "Climate",
    "Armor Addon",
    "Global",
    "Image Space",
    "Image Space Modifier",
    "Unknown 62",
    "Message",
    "Constructible Object",
    "Acoustic Space",
    "Unknown 66",
    "Script",
    "Effect Setting",
    "Music Type",
    "Static Collection",
    "Keyword",
    "Location",
    "Location Ref Type",
    "Footstep",
    "Footstep Set",
    "Material Type",
    "Action",
    "Music Track Form Helper",
    "Word Of Power",
    "Shout",
    "Relationship",
    "Equip Slot",
    "Association Type",
    "Outfit",
    "Art Object",
    "Material Object",
    "Unknown 87",
    "Lighting Template",
    "Shader Particle Geometry Data",
    "Reference Effect",
    "Unknown 91",
    "Movement Type",
    "Hazard",
    "Story Manager Event Node",
    "Sound Descriptor",
    "Dual Cast Data",
    "Sound Category",
    "Soul Gem",
    "Sound Output",
    "Collision Layer",
    "Scroll",
    "Color Form",
    "Reverb Parameters",
    "Pack-In",
    "Leveled Pack-In",
    "Reference Group",
    "Bendable Spline",
    "Unknown 108",
    "Unknown 109",
    "Aim Model",
    "Component",
    "Object Mod",
    "Material Swap",
    "Transform",
    "Zoom Data",
    "Instance Naming Rules",
    "Sound Keyword Mapping",
    "Terminal",
    "Audio Effect Chain",
    "Damage Type",
    "Actor Value",
    "Attraction Rule",
    "Sound Category Snapshot",
    "Sound Tag Set",
    "Lens Flare",
    "Unknown 126",
    "Snap Template Node",
    "Snap Template",
    "Ground Cover",
    "Emote",
    "Spell Threshold Data",
    "Resource",
    "Sound Echo",
    "Currency",
    "Unknown 135",
    "Perk Card Pack",
    "Leveled Perk Card",
    "Volumetric Lighting",
    "Curve Table Form",
    "Emote Category",
    "Workshop Permission",
    "Entitlement",
    "Power Armor Chasis",
    "Unknown 144",
    "Aim Assist Pose Data",
    "PhotoMode Feature",
    "Consumable Entitlement",
    "Crate Service Entitlement",
    "Challenge",
    "Avatar",
    "Region",
    "Condition Form",
    "Unknown 153",
    "Legendary Item",
    "Utility Item",
    "Model Swap",
    "Event Quest Widget",
    "Aim Assist Model",
    "Challenge Pass Reward Data",
    "Event Playlist",
    "Gameplay Reward",
    "Unknown 163",
    "Daily Content Group",
    "Idle Form",
];

#[derive(Debug, Deserialize)]
struct CtdaFunction {
    index: u32,
    name: String,
    p1: String,
    p2: String,
    p3: String,
}

#[derive(Debug, Deserialize)]
struct CtdaTable {
    functions: Vec<CtdaFunction>,
}

static CTDA_TABLE: OnceLock<CtdaTable> = OnceLock::new();

fn ctda_table() -> &'static CtdaTable {
    CTDA_TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("../schema/fo76.ctda.json"))
            .expect("fo76.ctda.json must be valid")
    })
}

fn lookup(idx: u32) -> Option<(String, char, char, char)> {
    let funcs = &ctda_table().functions;
    funcs.binary_search_by_key(&idx, |e| e.index).ok().map(|i| {
        let e = &funcs[i];
        (
            e.name.clone(),
            e.p1.chars().next().unwrap_or('N'),
            e.p2.chars().next().unwrap_or('N'),
            e.p3.chars().next().unwrap_or('N'),
        )
    })
}

static AVIF_REFS: OnceLock<Vec<String>> = OnceLock::new();

fn avif_refs() -> &'static [String] {
    AVIF_REFS.get_or_init(|| vec!["AVIF".into(), "NULL".into()])
}

fn decode_param(bytes: &[u8; 4], class: char, ctx: &DecodeContext<'_>) -> Value {
    match class {
        'N' | 'S' => json!(null),
        'F' => json_f32(f32::from_le_bytes(*bytes)),
        'I' => json!(i32::from_le_bytes(*bytes)),
        'A' => {
            if ctx.form_version >= 77 {
                let id = FormId(u32::from_le_bytes(*bytes));
                resolve_formid(ctx, avif_refs(), id)
            } else {
                json!(u32::from_le_bytes(*bytes))
            }
        }
        'R' => {
            let id = FormId(u32::from_le_bytes(*bytes));
            resolve_formid(ctx, &[], id)
        }
        'T' => {
            let idx = u32::from_le_bytes(*bytes) as usize;
            match CONDITION_FORM_TYPES.get(idx) {
                Some(name) => json!(name),
                None => json!(idx),
            }
        }
        _ => json!(u32::from_le_bytes(*bytes)),
    }
}

/// Decode a 32-byte CTDA data block into a structured JSON object.
pub fn decode_ctda(data: &[u8], ctx: &DecodeContext<'_>) -> Value {
    if data.len() < 32 {
        return json!({"hex": hex::encode(data), "_raw": true});
    }

    let type_byte = data[0];
    // Bits 5-7: comparison operator.
    let op_idx = ((type_byte & 0xE0) >> 5) as usize;
    // Bit 0: AND vs OR with the previous condition.
    let is_or = (type_byte & 0x01) != 0;
    // Bit 2: comparison value is a Global FormID rather than a float.
    let use_global = (type_byte & 0x04) != 0;

    let operator = OPERATORS.get(op_idx).copied().unwrap_or("Unknown");

    // Bytes 4-7: comparison value.
    let comp_bytes: [u8; 4] = data[4..8].try_into().unwrap();
    let comp_value: Value = if use_global {
        resolve_formid(ctx, &[], FormId(u32::from_le_bytes(comp_bytes)))
    } else {
        json_f32(f32::from_le_bytes(comp_bytes))
    };

    // Bytes 8-9: function index.
    let func_idx = u16::from_le_bytes(data[8..10].try_into().unwrap()) as u32;

    // Bytes 12-15, 16-19, 28-31: parameters 1-3.
    let p1: [u8; 4] = data[12..16].try_into().unwrap();
    let p2: [u8; 4] = data[16..20].try_into().unwrap();
    let p3: [u8; 4] = data[28..32].try_into().unwrap();

    // Bytes 20-23: Run On target.
    let run_on_idx = u32::from_le_bytes(data[20..24].try_into().unwrap()) as usize;
    let run_on = RUN_ON.get(run_on_idx).copied().unwrap_or("Unknown");

    // Bytes 24-27: Reference (FormID, used when Run On = Reference).
    let ref_id = FormId(u32::from_le_bytes(data[24..28].try_into().unwrap()));

    let mut out = Map::new();
    out.insert("Operator".into(), json!(operator));
    out.insert("AND/OR".into(), json!(if is_or { "OR" } else { "AND" }));
    out.insert("Comparison Value".into(), comp_value);

    if let Some((name, c1, c2, c3)) = lookup(func_idx) {
        out.insert("Function".into(), json!(name));
        if c1 != 'N' {
            out.insert("Parameter 1".into(), decode_param(&p1, c1, ctx));
        }
        if c2 != 'N' {
            out.insert("Parameter 2".into(), decode_param(&p2, c2, ctx));
        }
        if c3 != 'N' {
            out.insert("Parameter 3".into(), decode_param(&p3, c3, ctx));
        }
    } else {
        out.insert("Function".into(), json!(func_idx));
        // Unknown function: emit all three params as raw hex.
        out.insert("Parameter 1".into(), json!({"hex": hex::encode(&p1)}));
        out.insert("Parameter 2".into(), json!({"hex": hex::encode(&p2)}));
        out.insert("Parameter 3".into(), json!({"hex": hex::encode(&p3)}));
    }

    out.insert("Run On".into(), json!(run_on));
    if ref_id.0 != 0 {
        out.insert("Reference".into(), resolve_formid(ctx, &[], ref_id));
    }

    Value::Object(out)
}
