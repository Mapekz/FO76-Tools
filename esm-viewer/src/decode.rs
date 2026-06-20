use crate::formid::FormId;
use crate::reader::OwnedSubrecord;
use crate::schema::{
    ArrayCount, EnumFormat, FieldDef, IntegerWidth, MemberDef, Schema, UnionDecider, ValueFormat,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub struct DecodeContext<'a> {
    pub schema: &'a Schema,
    pub form_version: u16,
}

pub fn decode_record(
    ctx: &DecodeContext<'_>,
    signature: &str,
    subrecords: &[OwnedSubrecord],
) -> Value {
    let mut out = Map::new();
    let record_def = ctx.schema.record(signature);

    let mut by_sig: HashMap<String, Vec<&OwnedSubrecord>> = HashMap::new();
    for sr in subrecords {
        by_sig
            .entry(sr.signature.as_str().to_string())
            .or_default()
            .push(sr);
    }

    if let Some(def) = record_def {
        out.insert("_record_type".into(), json!(def.name));
        for member in &def.members {
            decode_member(ctx, member, &mut out, &mut by_sig, None);
        }
    } else {
        out.insert("_record_type".into(), json!(signature));
        out.insert("_unknown_record".into(), json!(true));
    }

    // Emit any subrecords not consumed
    let mut raw_remaining = Map::new();
    for (sig, subs) in &by_sig {
        if !subs.is_empty() {
            let entries: Vec<Value> = subs
                .iter()
                .map(|sr| {
                    json!({
                        "signature": sig,
                        "hex": hex::encode(&sr.data),
                        "_raw": true
                    })
                })
                .collect();
            raw_remaining.insert(sig.clone(), Value::Array(entries));
        }
    }
    if !raw_remaining.is_empty() {
        out.insert("_unmapped".into(), Value::Object(raw_remaining));
    }

    Value::Object(out)
}

fn decode_member(
    ctx: &DecodeContext<'_>,
    member: &MemberDef,
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, Vec<&OwnedSubrecord>>,
    payload: Option<&[u8]>,
) {
    if !member_version_ok(ctx.form_version, member) {
        return;
    }

    match member {
        MemberDef::Struct {
            sig, name, fields, ..
        } => {
            if let Some(payload) = payload {
                decode_struct_fields(ctx, name, fields, payload, out);
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    decode_struct_fields(ctx, name, fields, &sr.data, out);
                }
            }
        }
        MemberDef::Integer {
            sig,
            name,
            width,
            signed,
            format,
            ..
        } => {
            if let Some(data) = payload.or_else(|| {
                sig.as_ref()
                    .and_then(|s| peek_first(by_sig, s).map(|sr| sr.data.as_slice()))
            }) {
                if let Some(v) = read_int(data, *width, *signed) {
                    out.insert(name.clone(), format_int(v, format.as_ref()));
                }
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if let Some(v) = read_int(&sr.data, *width, *signed) {
                        out.insert(name.clone(), format_int(v, format.as_ref()));
                    }
                }
            }
        }
        MemberDef::Float { sig, name, .. } => {
            let data = payload.or_else(|| {
                sig.as_ref()
                    .and_then(|s| peek_first(by_sig, s).map(|sr| sr.data.as_slice()))
            });
            if let Some(data) = data {
                if data.len() >= 4 {
                    let f = f32::from_le_bytes(data[0..4].try_into().unwrap());
                    out.insert(name.clone(), json!(f));
                }
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if sr.data.len() >= 4 {
                        let f = f32::from_le_bytes(sr.data[0..4].try_into().unwrap());
                        out.insert(name.clone(), json!(f));
                    }
                }
            }
        }
        MemberDef::String {
            sig, name, sized, ..
        } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    let s = if let Some(n) = sized {
                        String::from_utf8_lossy(&sr.data[..sr.data.len().min(*n as usize)])
                            .trim_end_matches('\0')
                            .to_string()
                    } else {
                        read_zstring(&sr.data)
                    };
                    out.insert(name.clone(), json!(s));
                }
            }
        }
        MemberDef::LString { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if sr.data.len() >= 4 {
                        let id = u32::from_le_bytes(sr.data[0..4].try_into().unwrap());
                        out.insert(
                            name.clone(),
                            json!({
                                "lstring_id": format!("0x{:08X}", id),
                                "_unresolved": true
                            }),
                        );
                    }
                }
            }
        }
        MemberDef::FormId { sig, name, .. } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if sr.data.len() >= 4 {
                        let id = FormId::new(u32::from_le_bytes(sr.data[0..4].try_into().unwrap()));
                        out.insert(name.clone(), json!(id.display()));
                    }
                }
            }
        }
        MemberDef::Bytes { sig, name, len } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    let n = len.unwrap_or(sr.data.len());
                    out.insert(
                        name.clone(),
                        json!({
                            "hex": hex::encode(&sr.data[..sr.data.len().min(n)]),
                        }),
                    );
                }
            }
        }
        MemberDef::ByteRgba { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if sr.data.len() >= 4 {
                        out.insert(
                            name.clone(),
                            json!({
                                "r": sr.data[0], "g": sr.data[1], "b": sr.data[2], "a": sr.data[3]
                            }),
                        );
                    }
                }
            }
        }
        MemberDef::Vec3 { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if sr.data.len() >= 12 {
                        out.insert(
                            name.clone(),
                            json!({
                                "x": f32::from_le_bytes(sr.data[0..4].try_into().unwrap()),
                                "y": f32::from_le_bytes(sr.data[4..8].try_into().unwrap()),
                                "z": f32::from_le_bytes(sr.data[8..12].try_into().unwrap()),
                            }),
                        );
                    }
                }
            }
        }
        MemberDef::RStruct { name, members } => {
            let mut group = Map::new();
            for m in members {
                decode_member(ctx, m, &mut group, by_sig, None);
            }
            if !group.is_empty() {
                out.insert(name.clone(), Value::Object(group));
            }
        }
        MemberDef::RArray { name, element } => {
            let mut items = Vec::new();
            loop {
                let before: usize = by_sig.values().map(|v| v.len()).sum();
                let mut item = Map::new();
                decode_member(ctx, element, &mut item, by_sig, None);
                let after: usize = by_sig.values().map(|v| v.len()).sum();
                if item.is_empty() || before == after {
                    break;
                }
                items.push(Value::Object(item));
            }
            if !items.is_empty() {
                out.insert(name.clone(), Value::Array(items));
            }
        }
        MemberDef::Array {
            sig,
            name,
            element,
            count,
        } => {
            if let Some(sig) = sig {
                let taken = take_all(by_sig, sig);
                let items: Vec<Value> = match count {
                    Some(ArrayCount::Fixed(n)) => taken
                        .into_iter()
                        .take(*n)
                        .map(|sr| decode_field_value(ctx, element, &sr.data))
                        .collect(),
                    _ => taken
                        .into_iter()
                        .map(|sr| decode_field_value(ctx, element, &sr.data))
                        .collect(),
                };
                if !items.is_empty() {
                    out.insert(name.clone(), Value::Array(items));
                }
            }
        }
        MemberDef::Union {
            name,
            decider,
            variants,
        } => {
            let chosen = choose_union_variant(ctx.form_version, decider, variants.len());
            if let Some(idx) = chosen {
                if let Some(variant) = variants.get(idx) {
                    decode_member(ctx, variant, out, by_sig, payload);
                    return;
                }
            }
            out.insert(
                name.clone(),
                json!({
                    "_raw": true,
                    "reason": "union decider unresolved"
                }),
            );
        }
        MemberDef::Empty { sig, name } => {
            if let Some(sig) = sig {
                let _ = take_first(by_sig, sig);
                out.insert(name.clone(), json!(null));
            }
        }
        MemberDef::Unused { bytes } => {
            if let Some(data) = payload {
                let _ = data.get(..*bytes);
            }
        }
        MemberDef::Unknown { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    out.insert(
                        name.clone(),
                        json!({
                            "hex": hex::encode(&sr.data),
                            "_raw": true
                        }),
                    );
                }
            }
        }
        MemberDef::RawFallback { sig, name, reason } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    out.insert(
                        name.clone(),
                        json!({
                            "hex": hex::encode(&sr.data),
                            "_raw": true,
                            "reason": reason
                        }),
                    );
                }
            } else {
                out.insert(
                    name.clone(),
                    json!({
                        "_raw": true,
                        "reason": reason
                    }),
                );
            }
        }
    }
}

fn decode_struct_fields(
    ctx: &DecodeContext<'_>,
    struct_name: &str,
    fields: &[FieldDef],
    data: &[u8],
    out: &mut Map<String, Value>,
) {
    let mut pos = 0usize;
    let mut struct_out = Map::new();
    for field in fields {
        if !member_version_ok(ctx.form_version, field) {
            continue;
        }
        match field {
            MemberDef::Unused { bytes } => {
                pos = pos.saturating_add(*bytes);
            }
            MemberDef::Integer {
                name,
                width,
                signed,
                format,
                ..
            } => {
                let size = int_size(*width);
                if pos + size <= data.len() {
                    if let Some(v) = read_int(&data[pos..], *width, *signed) {
                        struct_out.insert(name.clone(), format_int(v, format.as_ref()));
                    }
                    pos += size;
                }
            }
            MemberDef::Float { name, .. } => {
                if pos + 4 <= data.len() {
                    let f = f32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
                    struct_out.insert(name.clone(), json!(f));
                    pos += 4;
                }
            }
            MemberDef::FormId { name, .. } => {
                if pos + 4 <= data.len() {
                    let id =
                        FormId::new(u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()));
                    struct_out.insert(name.clone(), json!(id.display()));
                    pos += 4;
                }
            }
            MemberDef::String { name, sized, .. } => {
                if let Some(n) = sized {
                    let end = (pos + *n as usize).min(data.len());
                    let s = String::from_utf8_lossy(&data[pos..end])
                        .trim_end_matches('\0')
                        .to_string();
                    struct_out.insert(name.clone(), json!(s));
                    pos = end;
                } else {
                    let end = data[pos..]
                        .iter()
                        .position(|&b| b == 0)
                        .map(|i| pos + i)
                        .unwrap_or(data.len());
                    let s = String::from_utf8_lossy(&data[pos..end]).to_string();
                    struct_out.insert(name.clone(), json!(s));
                    pos = if end < data.len() { end + 1 } else { end };
                }
            }
            MemberDef::Bytes { name, len, .. } => {
                let n = len.unwrap_or(data.len().saturating_sub(pos));
                let end = (pos + n).min(data.len());
                struct_out.insert(name.clone(), json!({"hex": hex::encode(&data[pos..end])}));
                pos = end;
            }
            MemberDef::Struct { name, fields, .. } => {
                decode_struct_fields(ctx, name, fields, &data[pos..], &mut struct_out);
            }
            MemberDef::Union {
                name,
                decider,
                variants,
            } => {
                let chosen = choose_union_variant(ctx.form_version, decider, variants.len());
                if let Some(idx) = chosen {
                    if let Some(variant) = variants.get(idx) {
                        let mut dummy = HashMap::new();
                        decode_member(
                            ctx,
                            variant,
                            &mut struct_out,
                            &mut dummy,
                            Some(&data[pos..]),
                        );
                        // advance pos heuristically for known variants
                        pos = advance_union(ctx, variant, &data[pos..], pos);
                    }
                } else {
                    struct_out.insert(
                        name.clone(),
                        json!({"hex": hex::encode(&data[pos..]), "_raw": true}),
                    );
                    break;
                }
            }
            MemberDef::Unknown { name, .. } => {
                struct_out.insert(
                    name.clone(),
                    json!({"hex": hex::encode(&data[pos..]), "_raw": true}),
                );
                break;
            }
            _ => {}
        }
    }
    if !struct_out.is_empty() {
        out.insert(struct_name.to_string(), Value::Object(struct_out));
    }
}

fn advance_union(ctx: &DecodeContext<'_>, variant: &MemberDef, data: &[u8], pos: usize) -> usize {
    let mut p = 0;
    match variant {
        MemberDef::Integer { width, .. } => p = int_size(*width),
        MemberDef::Float { .. } => p = 4,
        MemberDef::Unused { bytes } => p = *bytes,
        MemberDef::Struct { fields, .. } => {
            let mut dummy = Map::new();
            decode_struct_fields(ctx, "_", fields, data, &mut dummy);
            // estimate consumed bytes from field sizes
            for f in fields {
                if let MemberDef::Unused { bytes } = f {
                    p += bytes;
                } else if let MemberDef::Integer { width, .. } = f {
                    p += int_size(*width);
                } else if let MemberDef::Float { .. } = f {
                    p += 4;
                } else if let MemberDef::FormId { .. } = f {
                    p += 4;
                }
            }
        }
        _ => {}
    }
    pos + p.min(data.len())
}

fn decode_field_value(ctx: &DecodeContext<'_>, field: &FieldDef, data: &[u8]) -> Value {
    let mut m = Map::new();
    let mut by_sig = HashMap::new();
    decode_member(ctx, field, &mut m, &mut by_sig, Some(data));
    if m.len() == 1 {
        m.into_values().next().unwrap()
    } else {
        Value::Object(m)
    }
}

fn member_version_ok(form_version: u16, member: &MemberDef) -> bool {
    let (from_v, below_v) = match member {
        MemberDef::Struct {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        MemberDef::Integer {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        MemberDef::Float {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        _ => (None, None),
    };
    if let Some(v) = from_v {
        if form_version < v {
            return false;
        }
    }
    if let Some(v) = below_v {
        if form_version >= v {
            return false;
        }
    }
    true
}

fn choose_union_variant(form_version: u16, decider: &UnionDecider, n: usize) -> Option<usize> {
    match decider {
        UnionDecider::FormVersion {
            form_version: range,
        } => {
            if form_version >= range.min && range.max.is_none_or(|m| form_version <= m) {
                Some(0)
            } else {
                Some(1.min(n.saturating_sub(1)))
            }
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
        UnionDecider::Raw => None,
    }
}

fn int_size(w: IntegerWidth) -> usize {
    match w {
        IntegerWidth::U8 | IntegerWidth::S8 => 1,
        IntegerWidth::U16 | IntegerWidth::S16 => 2,
        IntegerWidth::U32 | IntegerWidth::S32 => 4,
        IntegerWidth::U64 | IntegerWidth::S64 => 8,
    }
}

fn read_int(data: &[u8], width: IntegerWidth, signed: bool) -> Option<i64> {
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

fn format_int(v: i64, format: Option<&ValueFormat>) -> Value {
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
            json!({"value": format!("0x{:X}", v as u32), "flags": set})
        }
        _ => json!(v),
    }
}

fn read_zstring(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

fn take_first<'a>(
    by_sig: &mut HashMap<String, Vec<&'a OwnedSubrecord>>,
    sig: &str,
) -> Option<&'a OwnedSubrecord> {
    by_sig.get_mut(sig).and_then(|v| {
        if v.is_empty() {
            None
        } else {
            Some(v.remove(0))
        }
    })
}

fn peek_first<'a>(
    by_sig: &HashMap<String, Vec<&'a OwnedSubrecord>>,
    sig: &str,
) -> Option<&'a OwnedSubrecord> {
    by_sig.get(sig).and_then(|v| v.first().copied())
}

fn take_all<'a>(
    by_sig: &mut HashMap<String, Vec<&'a OwnedSubrecord>>,
    sig: &str,
) -> Vec<&'a OwnedSubrecord> {
    by_sig.remove(sig).unwrap_or_default()
}

// Minimal hex encoding without extra dependency
mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
