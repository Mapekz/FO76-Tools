use crate::formid::FormId;
use crate::reader::OwnedSubrecord;
use crate::schema::{
    ArrayCount, EnumFormat, FieldDef, IntegerWidth, LStringTable, MemberDef, Schema, UnionDecider,
    ValueFormat,
};
use crate::strings::{Localization, StringKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// Controls how deeply FormID references are followed during decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolveDepth {
    /// Emit raw hex string — no resolution (default).
    #[default]
    None,
    /// Resolve to a stub: `{"formid": "...", "editor_id": "...", "record_type": "..."}`.
    Stub,
    /// Recursively decode the referenced record (depth-limited to 2 hops).
    Full,
}

pub trait FormIdRefResolver: Send + Sync {
    /// Look up a FormID stub. Returns None if not found.
    fn stub(&self, id: FormId) -> Option<FormIdStub>;
    /// Fully decode a record by FormID. Returns None if not found or on error.
    fn decode_full(&self, id: FormId) -> Option<Value>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormIdStub {
    pub formid: String,
    pub editor_id: Option<String>,
    pub record_type: String,
}

pub struct DecodeContext<'a> {
    pub schema: &'a Schema,
    pub form_version: u16,
    /// Whether the ESM file has the Localized flag set in its TES4 header.
    ///
    /// When `false`, FULL/DESC and other `lstring` fields contain inline
    /// NUL-terminated strings (optionally prefixed with `<ID=XXXXXXXX>`).
    /// When `true`, they contain 4-byte IDs into the string tables.
    pub is_localized: bool,
    /// Optional localization tables used to resolve LString IDs to text.
    pub localization: Option<&'a Localization>,
    /// Optional curve index for inlining CURV record data on FormID fields.
    pub curves: Option<&'a crate::curves::CurveIndex>,
    /// How to expand FormID references.
    pub resolve_depth: ResolveDepth,
    /// Resolver implementation (None when resolve_depth == None).
    pub resolver: Option<&'a dyn FormIdRefResolver>,
}

/// Resolve a FormID field to its JSON representation.
///
/// If the field's `valid_refs` includes `"CURV"` and a curve index is loaded,
/// the curve's path and point data are inlined into the output object.
/// When `ctx.resolve_depth` is `Stub` or `Full` and a resolver is present,
/// the referenced record is expanded inline. Otherwise, a bare hex string is returned.
fn resolve_formid(ctx: &DecodeContext<'_>, valid_refs: &[String], id: FormId) -> Value {
    // Existing curve branch — unchanged
    if valid_refs.iter().any(|r| r == "CURV") {
        if let Some(curves) = ctx.curves {
            if let Some(curve) = curves.get(id) {
                return json!({
                    "formid": id.display(),
                    "curve_path": curve.path,
                    "curve": curve.points.iter().map(|p| json!({"x": p.x, "y": p.y})).collect::<Vec<_>>()
                });
            }
        }
    }

    // Reference-following branch
    if ctx.resolve_depth != ResolveDepth::None {
        if let Some(resolver) = ctx.resolver {
            if id.0 == 0 {
                return json!(null);
            }
            match ctx.resolve_depth {
                ResolveDepth::Stub => {
                    if let Some(stub) = resolver.stub(id) {
                        return serde_json::to_value(&stub).unwrap_or_else(|_| json!(id.display()));
                    }
                }
                ResolveDepth::Full => {
                    if let Some(full) = resolver.decode_full(id) {
                        return full;
                    }
                }
                ResolveDepth::None => {}
            }
        }
    }

    // Null FormID
    if id.0 == 0 {
        return json!(null);
    }

    json!(id.display())
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
            if let Some(data) = payload {
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
            if let Some(data) = payload {
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
        MemberDef::LString { sig, name, table } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if ctx.is_localized {
                        // Localized ESM: field is a 4-byte ID into string tables.
                        if sr.data.len() >= 4 {
                            let id = u32::from_le_bytes(sr.data[0..4].try_into().unwrap());
                            let kind = lstring_table_to_kind(table);
                            if let Some(text) =
                                ctx.localization.and_then(|loc| loc.lookup(kind, id))
                            {
                                out.insert(name.clone(), json!(text));
                            } else {
                                out.insert(
                                    name.clone(),
                                    json!({
                                        "lstring_id": format!("0x{:08X}", id),
                                        "_unresolved": true
                                    }),
                                );
                            }
                        }
                    } else {
                        // Non-localized ESM: field is an inline NUL-terminated string,
                        // optionally prefixed with `<ID=XXXXXXXX>` (a reference marker).
                        let raw = &sr.data;
                        let nul_end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                        let s = String::from_utf8_lossy(&raw[..nul_end]);
                        // Strip the optional `<ID=XXXXXXXX>` prefix.
                        let text = if s.starts_with("<ID=") {
                            if let Some(close) = s.find('>') {
                                s[close + 1..].trim_start().to_string()
                            } else {
                                s.into_owned()
                            }
                        } else {
                            s.into_owned()
                        };
                        out.insert(name.clone(), json!(text));
                    }
                }
            }
        }
        MemberDef::FormId {
            sig,
            name,
            valid_refs,
        } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if sr.data.len() >= 4 {
                        let id = FormId::new(u32::from_le_bytes(sr.data[0..4].try_into().unwrap()));
                        out.insert(name.clone(), resolve_formid(ctx, valid_refs, id));
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
                if before == after {
                    break; // no subrecords consumed — done
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
            let chosen = match decider {
                UnionDecider::FieldValue {
                    field,
                    map,
                    default_variant,
                } => field_value_key(out, field)
                    .and_then(|k| map.get(&k).copied())
                    .or(*default_variant),
                UnionDecider::ByteAtOffset {
                    byte_offset,
                    map,
                    default_variant,
                } => payload
                    .and_then(|p| p.get(*byte_offset).copied())
                    .and_then(|b| map.get(&b).copied())
                    .or(*default_variant),
                _ => choose_union_variant(ctx.form_version, decider, variants.len()),
            };
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
        MemberDef::Vmad { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    out.insert(name.clone(), decode_vmad(&sr.data));
                }
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
            MemberDef::FormId {
                name, valid_refs, ..
            } => {
                if pos + 4 <= data.len() {
                    let id =
                        FormId::new(u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()));
                    struct_out.insert(name.clone(), resolve_formid(ctx, valid_refs, id));
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
                let chosen = match decider {
                    UnionDecider::ByteAtOffset {
                        byte_offset,
                        map,
                        default_variant,
                    } => data
                        .get(pos + byte_offset)
                        .copied()
                        .and_then(|b| map.get(&b).copied())
                        .or(*default_variant),
                    UnionDecider::FieldValue {
                        field,
                        map,
                        default_variant,
                    } => field_value_key(&struct_out, field)
                        .and_then(|k| map.get(&k).copied())
                        .or(*default_variant),
                    _ => choose_union_variant(ctx.form_version, decider, variants.len()),
                };
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
        // ByteAtOffset and FieldValue are handled by the callers before reaching here
        UnionDecider::ByteAtOffset { .. } | UnionDecider::FieldValue { .. } => None,
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

/// Resolve a `FieldValue` lookup key from an already-decoded output map.
///
/// Supports dot-separated paths (e.g. `"Effect Header.Effect Type"`) to reach
/// into nested objects. For enum-formatted integers, the object has a `"value"`
/// key whose integer is used as the map key.
fn field_value_key(out: &Map<String, Value>, field: &str) -> Option<String> {
    let val = if let Some((parent, child)) = field.split_once('.') {
        out.get(parent)?.get(child)?
    } else {
        out.get(field)?
    };
    let key = match val {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Object(o) => o
            .get("value")
            .and_then(Value::as_i64)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        _ => val.to_string(),
    };
    Some(key)
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

fn take_all<'a>(
    by_sig: &mut HashMap<String, Vec<&'a OwnedSubrecord>>,
    sig: &str,
) -> Vec<&'a OwnedSubrecord> {
    by_sig.remove(sig).unwrap_or_default()
}

/// Decode a VMAD (Papyrus scripts) subrecord into a structured JSON value.
///
/// VMAD stores Papyrus script attachments with properties in a compact binary format.
/// Never panics on truncated or malformed input — returns a raw hex fallback instead.
pub fn decode_vmad(data: &[u8]) -> Value {
    let mut pos = 0usize;

    macro_rules! need {
        ($n:expr) => {
            if pos + $n > data.len() {
                return json!({
                    "_raw": true,
                    "reason": "VMAD truncated",
                    "hex": hex::encode(&data[pos..])
                });
            }
        };
    }

    macro_rules! read_u16 {
        () => {{
            need!(2);
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }

    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            need!(len);
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }

    let version = read_u16!();
    let obj_format = read_u16!();
    let script_count = read_u16!();

    let mut scripts = Vec::new();
    for _ in 0..script_count {
        let name = read_wstring!();
        need!(1);
        let status = data[pos];
        pos += 1;
        let prop_count = read_u16!();
        let mut props = Vec::new();
        for _ in 0..prop_count {
            let prop_name = read_wstring!();
            need!(2);
            let prop_type = data[pos];
            pos += 1;
            let _prop_status = data[pos];
            pos += 1;
            let value = decode_vmad_property(data, &mut pos, prop_type, obj_format);
            props.push(json!({"name": prop_name, "type": prop_type, "value": value}));
        }
        scripts.push(json!({"name": name, "status": status, "properties": props}));
    }

    json!({"version": version, "scripts": scripts})
}

fn decode_vmad_property(data: &[u8], pos: &mut usize, prop_type: u8, obj_format: u16) -> Value {
    let read_object = |data: &[u8], pos: &mut usize, obj_format: u16| -> Value {
        if obj_format == 2 {
            if *pos + 4 > data.len() {
                return json!(null);
            }
            let form_id =
                u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
            *pos += 4;
            json!(format!("{:#010X}", form_id))
        } else {
            if *pos + 6 > data.len() {
                return json!(null);
            }
            *pos += 2; // unused alias field
            let form_id =
                u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
            *pos += 4;
            json!(format!("{:#010X}", form_id))
        }
    };

    match prop_type {
        1 => read_object(data, pos, obj_format),
        2 => {
            // String
            if *pos + 2 > data.len() {
                return json!(null);
            }
            let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
            *pos += 2;
            if *pos + len > data.len() {
                return json!(null);
            }
            let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
            *pos += len;
            json!(s)
        }
        3 => {
            // Int
            if *pos + 4 > data.len() {
                return json!(null);
            }
            let v =
                i32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
            *pos += 4;
            json!(v)
        }
        4 => {
            // Float
            if *pos + 4 > data.len() {
                return json!(null);
            }
            let v =
                f32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
            *pos += 4;
            json!(v)
        }
        5 => {
            // Bool
            if *pos >= data.len() {
                return json!(null);
            }
            let v = data[*pos] != 0;
            *pos += 1;
            json!(v)
        }
        _ => {
            // Array types and unknown — emit type tag as raw fallback
            json!({"_raw": true, "type": prop_type})
        }
    }
}

/// Map a schema [`LStringTable`] selector to the runtime [`StringKind`].
fn lstring_table_to_kind(table: &LStringTable) -> StringKind {
    match table {
        LStringTable::Strings => StringKind::Strings,
        LStringTable::Dlstrings => StringKind::DlStrings,
        LStringTable::Ilstrings => StringKind::IlStrings,
    }
}

// Minimal hex encoding without extra dependency
mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
