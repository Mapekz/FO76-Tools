use crate::formid::{FormId, parse_formid};
use crate::schema::{EnumFormat, IntegerWidth, MemberDef, UnionDecider, ValueFormat};
use serde_json::{Map, Value, json};

use super::{DecodeContext, hex, resolve_formid};

/// Emit a decoded f32 game value as a JSON number, free of f32→f64 widening
/// noise (e.g. `0.5f32` printing as `0.49999998`). `serde_json::Value` only
/// stores `f64`, and casting `f as f64` widens losslessly but then gets
/// formatted at *f64* round-trip precision (52-bit mantissa) instead of the
/// value's real *f32* precision (23-bit mantissa) — exposing bits that were
/// never meaningful. Routing through `f32::to_string()` (which already
/// implements shortest-round-trip formatting for f32) and re-parsing as f64
/// keeps exactly the digits the f32 actually carries, no more, no less. Non-
/// finite inputs are passed through unchanged so serde_json's existing
/// inf/NaN → null behavior is preserved.
pub(crate) fn json_f32(f: f32) -> Value {
    if !f.is_finite() {
        return json!(f);
    }
    json!(
        f.to_string()
            .parse::<f64>()
            .expect("f32::to_string output must parse as f64")
    )
}

pub(super) fn format_int(v: i64, format: Option<&ValueFormat>) -> Value {
    match format {
        Some(ValueFormat::Enum { values }) => match values {
            EnumFormat::Dense(names) => {
                if v >= 0 && (v as usize) < names.len() {
                    json!({"value": v, "name": names[v as usize]})
                } else {
                    json!(v)
                }
            }
            EnumFormat::Sparse(map) => {
                let key = format!("{}", v);
                if let Some(name) = map
                    .get(&key)
                    .or_else(|| map.get(&format!("0x{:X}", v as u32)))
                {
                    json!({"value": v, "name": name})
                } else {
                    json!(v)
                }
            }
        },
        Some(ValueFormat::Flags { flags }) => {
            let mut set = Vec::new();
            for (i, name) in flags.iter().enumerate() {
                if v & (1i64 << i) != 0 {
                    set.push(name.clone());
                }
            }
            json!({"value": format!("0x{:X}", v as u64), "flags": set})
        }
        _ => json!(v),
    }
}

pub(super) fn read_zstring(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

pub(super) fn scalar_int(
    bytes: &[u8],
    width: IntegerWidth,
    signed: bool,
    format: Option<&ValueFormat>,
) -> Option<Value> {
    read_int(bytes, width, signed).map(|v| format_int(v, format))
}

pub(super) fn scalar_float(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < 4 {
        return None;
    }
    Some(json_f32(f32::from_le_bytes(
        bytes[0..4].try_into().unwrap(),
    )))
}

pub(super) fn scalar_formid(
    ctx: &DecodeContext<'_>,
    valid_refs: &[String],
    bytes: &[u8],
) -> Option<Value> {
    if bytes.len() < 4 {
        return None;
    }
    let id = FormId::new(u32::from_le_bytes(bytes[0..4].try_into().unwrap()));
    Some(resolve_formid(ctx, valid_refs, id))
}

pub(super) fn scalar_bytes(bytes: &[u8]) -> Value {
    json!({"hex": hex::encode(bytes)})
}

pub(super) fn scalar_rgba(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < 4 {
        return None;
    }
    Some(json!({
        "r": bytes[0], "g": bytes[1], "b": bytes[2], "a": bytes[3]
    }))
}

pub(super) fn scalar_vec3(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < 12 {
        return None;
    }
    Some(json!({
        "x": json_f32(f32::from_le_bytes(bytes[0..4].try_into().unwrap())),
        "y": json_f32(f32::from_le_bytes(bytes[4..8].try_into().unwrap())),
        "z": json_f32(f32::from_le_bytes(bytes[8..12].try_into().unwrap())),
    }))
}

/// Fixed-size vs null-terminated string decode shared by `decode_member` (subrecord
/// pool) and `decode_struct_fields` (contiguous buffer cursor). Callers own bounds
/// checks and cursor advancement.
pub(super) fn scalar_string(bytes: &[u8], sized: &Option<u32>) -> Value {
    let s = match sized {
        Some(n) if *n > 0 => String::from_utf8_lossy(&bytes[..bytes.len().min(*n as usize)])
            .trim_end_matches('\0')
            .to_string(),
        _ => read_zstring(bytes),
    };
    json!(s)
}

pub(super) fn read_int(data: &[u8], width: IntegerWidth, signed: bool) -> Option<i64> {
    let size = int_size(width);
    if data.len() < size {
        return None;
    }
    let v = match width {
        IntegerWidth::U8 => data[0] as i64,
        IntegerWidth::S8 => data[0] as i8 as i64,
        IntegerWidth::U16 => u16::from_le_bytes(data[0..2].try_into().ok()?) as i64,
        IntegerWidth::S16 => i16::from_le_bytes(data[0..2].try_into().ok()?) as i64,
        IntegerWidth::U32 => u32::from_le_bytes(data[0..4].try_into().ok()?) as i64,
        IntegerWidth::S32 => i32::from_le_bytes(data[0..4].try_into().ok()?) as i64,
        IntegerWidth::U64 => u64::from_le_bytes(data[0..8].try_into().ok()?) as i64,
        IntegerWidth::S64 => i64::from_le_bytes(data[0..8].try_into().ok()?),
    };
    if !signed && v < 0 {
        return Some(v as u64 as i64);
    }
    Some(v)
}

pub(super) fn int_size(w: IntegerWidth) -> usize {
    match w {
        IntegerWidth::U8 | IntegerWidth::S8 => 1,
        IntegerWidth::U16 | IntegerWidth::S16 => 2,
        IntegerWidth::U32 | IntegerWidth::S32 => 4,
        IntegerWidth::U64 | IntegerWidth::S64 => 8,
    }
}

/// Read `width` bytes starting at `offset` in `data` as a little-endian unsigned integer.
/// Returns None if there isn't enough data.
pub(super) fn read_le_uint(data: &[u8], offset: usize, width: usize) -> Option<u64> {
    let end = offset.checked_add(width)?;
    let bytes = data.get(offset..end)?;
    let v = match width {
        1 => bytes[0] as u64,
        2 => u16::from_le_bytes(bytes.try_into().ok()?) as u64,
        4 => u32::from_le_bytes(bytes.try_into().ok()?) as u64,
        8 => u64::from_le_bytes(bytes.try_into().ok()?),
        _ => return None,
    };
    Some(v)
}

/// Resolve a field's raw integer value from an already-decoded output map.
///
/// Handles plain numbers, enum objects (`{"value": N, "name": "..."}`) and
/// flags objects (`{"value": "0x...", "flags": [...]}`).
pub(super) fn field_int_value(out: &Map<String, Value>, field: &str) -> Option<u64> {
    let val = if let Some((parent, child)) = field.split_once('.') {
        out.get(parent)
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(child))?
    } else {
        out.get(field)?
    };
    match val {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => parse_uint_str(s),
        Value::Object(o) => o.get("value").and_then(|v| match v {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => parse_uint_str(s),
            _ => None,
        }),
        _ => None,
    }
}

/// Parse a decimal or `0x`-prefixed hexadecimal string to u64.
pub(super) fn parse_uint_str(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

/// Resolve a `FieldValue` lookup key from an already-decoded output map.
///
/// Supports dot-separated paths (e.g. `"Effect Header.Effect Type"`) to reach
/// into nested objects. For enum-formatted integers, the object has a `"value"`
/// key whose integer is used as the map key. JSON `null` maps to the key `"null"`
/// (used by union deciders such as `wbNAVIParentDecider`).
pub(super) fn field_value_key(out: &Map<String, Value>, field: &str) -> Option<String> {
    let val = if let Some((parent, child)) = field.split_once('.') {
        out.get(parent)?.get(child)?
    } else {
        out.get(field)?
    };
    let key = match val {
        Value::Null => "null".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Object(o) => o
            .get("value")
            .and_then(Value::as_i64)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        _ => val.to_string(),
    };
    if key.is_empty() {
        return None;
    }
    Some(key)
}

/// Resolve the target record signature for a decoded sibling FormID field.
pub(super) fn sibling_target_sig(value: &Value, ctx: &DecodeContext<'_>) -> Option<String> {
    if let Value::Object(o) = value
        && let Some(rt) = o.get("record_type").and_then(|v| v.as_str())
    {
        return Some(rt.to_string());
    }
    let id = match value {
        Value::String(s) => parse_formid(s).ok(),
        Value::Object(o) => o
            .get("formid")
            .and_then(|v| v.as_str())
            .and_then(|s| parse_formid(s).ok()),
        _ => None,
    }?;
    ctx.resolver.and_then(|r| r.stub(id).map(|s| s.record_type))
}

pub(super) fn choose_union_variant(
    form_version: u16,
    record_edid: Option<char>,
    decider: &UnionDecider,
    n: usize,
) -> Option<usize> {
    match decider {
        UnionDecider::FormVersion {
            form_version: range,
        } => {
            // Pascal semantics (wbFormVersionDecider):
            //   form_version IN [min, max] → variant 1  (new/larger struct)
            //   form_version OUT of range  → variant 0  (old/smaller struct)
            // This is the OPPOSITE of what the name "FormVersion" might suggest.
            if form_version >= range.min && range.max.is_none_or(|m| form_version <= m) {
                Some(1.min(n.saturating_sub(1)))
            } else {
                Some(0)
            }
        }
        UnionDecider::FormVersionThresholds {
            form_version_thresholds,
        } => {
            // Return the index of the first threshold that is > form_version.
            // If all thresholds are ≤ form_version, return thresholds.len() (last variant).
            let idx = form_version_thresholds
                .iter()
                .position(|&t| form_version < t)
                .unwrap_or(form_version_thresholds.len());
            Some(idx.min(n.saturating_sub(1)))
        }
        UnionDecider::EdidPrefix {
            edid_prefix,
            edid_default,
        } => {
            let variant = record_edid
                .and_then(|c| edid_prefix.get(&c.to_string()).copied())
                .or(*edid_default);
            variant.map(|v| v.min(n.saturating_sub(1)))
        }
        UnionDecider::FromVersion { from_version } => {
            if form_version >= *from_version {
                Some(0)
            } else {
                None
            }
        }
        UnionDecider::BelowVersion { below_version } => {
            if form_version < *below_version {
                Some(0)
            } else {
                None
            }
        }
        // ByteAtOffset, FieldValue, PresentSignature, FormIdTargetType, and
        // PayloadSize are handled by the callers
        UnionDecider::ByteAtOffset { .. }
        | UnionDecider::FieldValue { .. }
        | UnionDecider::PresentSignature { .. }
        | UnionDecider::FormIdTargetType { .. }
        | UnionDecider::PayloadSize { .. } => None,
        UnionDecider::Raw => None,
    }
}

/// Form-version activation bounds `(from_version, below_version)` for the
/// member kinds that carry them. The single source both the decoder
/// ([`member_version_ok`]) and `diff`'s version-gated-transition stripping
/// read (issue #29): active iff `fv >= from` (when set) and `fv < below`
/// (when set, strict).
pub(crate) fn member_version_bounds(member: &MemberDef) -> (Option<u16>, Option<u16>) {
    match member {
        MemberDef::Struct {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Integer {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Float {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Unused {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Empty {
            from_version,
            below_version,
            ..
        }
        | MemberDef::Bytes {
            from_version,
            below_version,
            ..
        }
        | MemberDef::FormId {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        _ => (None, None),
    }
}

pub(crate) fn member_version_ok(form_version: u16, member: &MemberDef) -> bool {
    let (from_v, below_v) = member_version_bounds(member);
    from_v.is_none_or(|v| form_version >= v) && below_v.is_none_or(|v| form_version < v)
}

/// `wbFromSize(N, value)` gate. `data_len` must be the length of the `data`
/// slice for the *current* `decode_struct_fields` invocation — every known
/// FO76 `wbFromSize` site is a direct field of the struct that IS the
/// subrecord's own payload (never nested inside a further `Struct` member),
/// so `data_len` at that call depth already equals the subrecord's DataSize,
/// matching Pascal's `SubRecord.DataSize` check exactly. A `from_size` field
/// nested one more `Struct` level down would need the original subrecord
/// length threaded separately — not needed by any current schema site; if
/// one appears, this comment is the tripwire.
pub(super) fn member_from_size_ok(data_len: usize, member: &MemberDef) -> bool {
    let from_size = match member {
        MemberDef::Integer { from_size, .. }
        | MemberDef::Float { from_size, .. }
        | MemberDef::FormId { from_size, .. }
        | MemberDef::Bytes { from_size, .. }
        | MemberDef::ByteRgba { from_size, .. } => *from_size,
        _ => None,
    };
    from_size.is_none_or(|n| data_len >= n)
}
