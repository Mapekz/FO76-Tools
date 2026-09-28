//! Payload decoding: the bytes of one subrecord (or a slice of them) into
//! nodes. Which subrecord a member reads is `bind.rs`'s business.

use crate::reader::OwnedSubrecord;
use crate::schema::{ArrayCount, CountPath, FieldDef, MemberDef, UnionDecider};

use super::DecodeContext;
use super::model_info::decode_model_info;
use super::node::{Fields, Node, insert_unique};
use super::rules::{PostDecodeTarget, apply_post_decode_rules};
use super::scalars::{
    choose_union_variant, count_path_value, field_int_value, field_value_key, int_size,
    member_from_size_ok, member_version_ok, read_le_uint, scalar_bytes, scalar_float,
    scalar_formid, scalar_int, scalar_rgba, scalar_string, scalar_vec3, sibling_target_sig,
};

/// Decode `member` from `data`, a payload slice already in hand: a union
/// variant or an array element inside a subrecord.
pub(crate) fn decode_member(
    ctx: &DecodeContext<'_>,
    member: &MemberDef,
    out: &mut Fields,
    data: &[u8],
) {
    if !member_version_ok(ctx.form_version, member) {
        return;
    }
    match member {
        MemberDef::Struct { name, fields, .. } => {
            decode_struct_fields(ctx, name, fields, data, out);
        }
        MemberDef::Integer {
            name,
            width,
            signed,
            format,
            ..
        } => {
            if let Some(v) = scalar_int(data, *width, *signed, format.as_ref()) {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::Float { name, .. } => {
            if let Some(v) = scalar_float(data) {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::FormId {
            name, valid_refs, ..
        } => {
            if let Some(v) = scalar_formid(valid_refs, data) {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::Bytes { name, len, .. } => {
            let n = len.unwrap_or(data.len());
            out.insert(name.clone(), scalar_bytes(&data[..data.len().min(n)]));
        }
        MemberDef::Vec3 { name, .. } => {
            // NVNM's Vertices is a bare Vec3 array element.
            if let Some(v) = scalar_vec3(data) {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::Array {
            sig: None,
            name,
            element,
            count: Some(ArrayCount::Fixed(n)),
        } => {
            // A nested array element (e.g. the inner dimension of an
            // array-of-arrays, such as CELL's 32x32 Max Height Data grid)
            // reached via decode_field_value with its own byte slice. Only
            // the Fixed-count shape occurs in this position — mirrors
            // decode_struct_fields's packed Array arm but starting at
            // position 0 of the given slice.
            if let Some(elem_size) = field_byte_size(ctx, element) {
                let mut items = Vec::with_capacity((*n).min(4096));
                let mut pos = 0;
                for _ in 0..*n {
                    if pos + elem_size > data.len() {
                        break;
                    }
                    items.push(decode_field_value(
                        ctx,
                        element,
                        &data[pos..pos + elem_size],
                    ));
                    pos += elem_size;
                }
                if !items.is_empty() {
                    out.insert(name.to_owned(), Node::Array(items));
                }
            }
        }
        MemberDef::Union {
            name,
            decider,
            variants,
            ..
        } => decode_union(ctx, name, decider, variants, out, data),
        MemberDef::RawFallback {
            sig: None,
            name,
            reason,
        } => {
            out.insert(name.clone(), Node::raw_reason(None, reason));
        }
        MemberDef::Ctda { name, .. } => {
            out.insert(name.clone(), crate::ctda::ctda_node(data, ctx));
        }
        MemberDef::ModelInfo { name, .. } => {
            out.insert(name.clone(), decode_model_info(data));
        }
        // Subrecord-level kinds: they only ever decode a whole subrecord of
        // their own (see bind.rs).
        MemberDef::String { .. }
        | MemberDef::LString { .. }
        | MemberDef::ByteRgba { .. }
        | MemberDef::Empty { .. }
        | MemberDef::Unused { .. }
        | MemberDef::Unknown { .. }
        | MemberDef::RawFallback { .. }
        | MemberDef::Vmad { .. }
        | MemberDef::Array { .. }
        | MemberDef::RStruct { .. }
        | MemberDef::RArray { .. } => {}
    }
}

/// Pick a union's variant. `fields` are the already-decoded siblings (read by
/// field-value and FormID-target-type deciders, falling back to
/// `ctx.outer_struct`); `payload` is the bytes the union decodes from (read by
/// byte-offset and payload-size deciders).
pub(super) fn choose_variant(
    ctx: &DecodeContext<'_>,
    decider: &UnionDecider,
    n_variants: usize,
    fields: &Fields,
    payload: &[u8],
) -> Option<usize> {
    let outer = ctx.outer_struct.as_ref();
    match decider {
        UnionDecider::FieldValue {
            field,
            map,
            default_variant,
            bits,
        } => {
            // Bitmask check first (for flag-field deciders like wbBOOKTeachesDecider).
            let by_bits = if bits.is_empty() {
                None
            } else {
                field_int_value(fields, field, ctx)
                    .or_else(|| outer.and_then(|o| field_int_value(o, field, ctx)))
                    .and_then(|v| {
                        bits.iter()
                            .find_map(|[mask, idx]| (v & mask != 0).then_some(*idx as usize))
                    })
            };
            by_bits
                .or_else(|| {
                    field_value_key(fields, field, ctx)
                        .or_else(|| outer.and_then(|o| field_value_key(o, field, ctx)))
                        .and_then(|k| map.get(&k).copied())
                })
                .or(*default_variant)
        }
        UnionDecider::ByteAtOffset {
            byte_offset,
            map,
            default_variant,
            width_bytes,
        } => read_le_uint(payload, *byte_offset, *width_bytes)
            .and_then(|b| map.get(&b.to_string()).copied())
            .or(*default_variant),
        UnionDecider::PayloadSize {
            payload_size,
            default_variant,
        } => payload_size
            .get(&payload.len().to_string())
            .copied()
            .or(*default_variant),
        UnionDecider::FormIdTargetType {
            form_id_target_type,
            map,
            default_variant,
        } => fields
            .get(form_id_target_type)
            .or_else(|| outer.and_then(|o| o.get(form_id_target_type)))
            .and_then(|v| sibling_target_sig(v, ctx))
            .and_then(|sig| map.get(&sig).copied())
            .or(*default_variant),
        _ => choose_union_variant(ctx.form_version, ctx.record_edid_char, decider, n_variants),
    }
}

/// Decode a union whose bytes are `payload` (its own subrecord, or a payload
/// variant), inserting the chosen variant's value under the union's name.
pub(super) fn decode_union(
    ctx: &DecodeContext<'_>,
    name: &str,
    decider: &UnionDecider,
    variants: &[MemberDef],
    out: &mut Fields,
    payload: &[u8],
) {
    let chosen = choose_variant(ctx, decider, variants.len(), out, payload);
    let Some(variant) = chosen.and_then(|idx| variants.get(idx)) else {
        out.insert(
            name.to_owned(),
            Node::raw_reason(None, "union decider unresolved"),
        );
        return;
    };
    // Decode into a temporary map first: some variants are anonymous (Pascal
    // `wbInteger('', ...)` reusing the union's own name conceptually), so
    // their decoded value would otherwise land under the empty-string key
    // instead of the union's own (correctly-deduped) name.
    let mut tmp = Fields::new();
    decode_member(ctx, variant, &mut tmp, payload);
    for (k, v) in tmp {
        let key = if k.is_empty() { name.to_owned() } else { k };
        insert_unique(out, key, v);
    }
}

/// Decode a packed-array member from its subrecords (`run`: one, or several
/// consecutive copies of the same signature).
pub(super) fn decode_array_payloads(
    ctx: &DecodeContext<'_>,
    name: &str,
    element: &FieldDef,
    count: &Option<ArrayCount>,
    run: &[&OwnedSubrecord],
    out: &mut Fields,
) {
    // A single subrecord may pack multiple fixed-size elements (e.g. KWDA
    // packs every keyword FormID into one subrecord, counted by KSIZ; APPR
    // packs attach-parent-slot FormIDs similarly).  Split each subrecord by
    // the element's static byte size when it is known and the subrecord is
    // strictly larger; otherwise fall back to one element per subrecord so
    // variable-size element arrays are unaffected.
    let elem_size = field_byte_size(ctx, element);
    let mut items: Vec<Node> = Vec::new();
    for sr in run {
        match elem_size {
            Some(sz) if sz > 0 && sr.data.len() > sz => {
                let mut pos = 0;
                while pos + sz <= sr.data.len() {
                    items.push(decode_field_value(ctx, element, &sr.data[pos..pos + sz]));
                    pos += sz;
                }
            }
            None if matches!(element, MemberDef::Struct { .. }) => {
                // Variable-size struct element (e.g. contains a nested
                // count-prefixed array): the subrecord may pack one or more
                // instances back-to-back with no static per-element size.
                // Loop using the real consumed-byte count per instance
                // (mirrors advance_union) until the subrecord's data is
                // exhausted, instead of decoding only the first instance and
                // silently dropping the rest.
                if let MemberDef::Struct {
                    name: elem_name,
                    fields,
                    ..
                } = element
                {
                    let mut pos = 0;
                    while pos < sr.data.len() {
                        let mut elem_out = Fields::new();
                        let consumed = decode_struct_fields(
                            ctx,
                            elem_name,
                            fields,
                            &sr.data[pos..],
                            &mut elem_out,
                        );
                        if consumed == 0 {
                            break;
                        }
                        if let Some(v) = elem_out.swap_remove(elem_name) {
                            items.push(v);
                        }
                        pos += consumed;
                    }
                }
            }
            _ => items.push(decode_field_value(ctx, element, &sr.data)),
        }
    }
    if let Some(ArrayCount::Fixed(n)) = count {
        items.truncate(*n);
    }
    if !items.is_empty() {
        out.insert(name.to_owned(), Node::Array(items));
    }
}

/// Whether `member` is an array whose count lives in the enclosing scope
/// (`up >= 1`), so its struct must be decoded with that scope in
/// `ctx.outer_struct`.
pub(super) fn counts_from_enclosing_scope(member: &MemberDef) -> bool {
    matches!(
        member,
        MemberDef::Array {
            count: Some(ArrayCount::CountPath(CountPath { up: 1.., .. })),
            ..
        }
    )
}

/// Whether `member` or any nested field uses a `FieldValue` union decider.
pub(super) fn contains_field_value_union(member: &MemberDef) -> bool {
    match member {
        MemberDef::Union {
            decider: UnionDecider::FieldValue { .. },
            ..
        } => true,
        MemberDef::Struct { fields, .. } => fields.iter().any(contains_field_value_union),
        MemberDef::Union { variants, .. } => variants.iter().any(contains_field_value_union),
        MemberDef::Array { element, .. } => contains_field_value_union(element),
        _ => false,
    }
}

/// Decode the fields of a struct payload into `out` under the key `struct_name`.
/// Returns the number of bytes consumed from `data`.
pub(crate) fn decode_struct_fields(
    ctx: &DecodeContext<'_>,
    struct_name: &str,
    fields: &[FieldDef],
    data: &[u8],
    out: &mut Fields,
) -> usize {
    let mut pos = 0usize;
    let mut struct_out = Fields::new();
    for field in fields {
        if !member_version_ok(ctx.form_version, field) {
            continue;
        }
        if !member_from_size_ok(data.len(), field) {
            continue;
        }
        match field {
            MemberDef::Unused { bytes, .. } => {
                pos = pos.saturating_add(*bytes).min(data.len());
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
                    if let Some(v) = scalar_int(&data[pos..], *width, *signed, format.as_ref()) {
                        struct_out.insert(name.clone(), v);
                    }
                    pos += size;
                }
            }
            MemberDef::Float { name, .. } => {
                if pos + 4 <= data.len() {
                    if let Some(v) = scalar_float(&data[pos..]) {
                        struct_out.insert(name.clone(), v);
                    }
                    pos += 4;
                }
            }
            MemberDef::FormId {
                name, valid_refs, ..
            } => {
                if pos + 4 <= data.len() {
                    if let Some(v) = scalar_formid(valid_refs, &data[pos..]) {
                        struct_out.insert(name.clone(), v);
                    }
                    pos += 4;
                }
            }
            MemberDef::String { name, sized, .. } => {
                match sized {
                    Some(n) if *n > 0 => {
                        let end = (pos + *n as usize).min(data.len());
                        struct_out.insert(name.clone(), scalar_string(&data[pos..end], sized));
                        pos = end;
                    }
                    _ => {
                        // None or sized=0 both mean null-terminated.
                        let end = data[pos..]
                            .iter()
                            .position(|&b| b == 0)
                            .map(|i| pos + i)
                            .unwrap_or(data.len());
                        struct_out.insert(name.clone(), scalar_string(&data[pos..], sized));
                        pos = if end < data.len() { end + 1 } else { end };
                    }
                }
            }
            MemberDef::Bytes { name, len, .. } => {
                let n = len.unwrap_or(data.len().saturating_sub(pos));
                let end = (pos + n).min(data.len());
                struct_out.insert(name.clone(), scalar_bytes(&data[pos..end]));
                pos = end;
            }
            MemberDef::ByteRgba { name, .. } => {
                if pos + 4 <= data.len() {
                    if let Some(v) = scalar_rgba(&data[pos..]) {
                        struct_out.insert(name.clone(), v);
                    }
                    pos += 4;
                }
            }
            MemberDef::Vec3 { name, .. } => {
                if pos + 12 <= data.len() {
                    if let Some(v) = scalar_vec3(&data[pos..]) {
                        struct_out.insert(name.clone(), v);
                    }
                    pos += 12;
                }
            }
            MemberDef::RawFallback { name, reason, .. } => {
                if pos < data.len() {
                    struct_out.insert(name.clone(), Node::raw_reason(Some(&data[pos..]), reason));
                }
                pos = data.len();
                break;
            }
            MemberDef::Struct { name, fields, .. } => {
                let sub_data = data.get(pos..).unwrap_or(&[]);
                let consumed = decode_struct_fields(ctx, name, fields, sub_data, &mut struct_out);
                pos = (pos + consumed).min(data.len());
            }
            MemberDef::Union {
                name,
                decider,
                variants,
                ..
            } => {
                let chosen =
                    choose_variant(ctx, decider, variants.len(), &struct_out, &data[pos..]);
                if let Some(idx) = chosen {
                    if let Some(variant) = variants.get(idx) {
                        // Decode into a temporary map so we can insert_unique
                        // each key, avoiding silent clobbers when two union
                        // slots share the same variant name (e.g. MGEF's two
                        // `wbActorValue` fields both named "Actor Value").
                        let mut tmp = Fields::new();
                        decode_member(ctx, variant, &mut tmp, &data[pos..]);
                        for (k, v) in tmp {
                            insert_unique(&mut struct_out, k, v);
                        }
                        // advance pos heuristically for known variants
                        pos = advance_union(ctx, variant, &data[pos..], pos);
                    }
                } else {
                    struct_out.insert(name.clone(), Node::raw(&data[pos..]));
                    pos = data.len();
                    break;
                }
            }
            MemberDef::Array {
                name,
                element,
                count,
                ..
            } => {
                let n: usize = match count {
                    Some(ArrayCount::CountPrefix(width)) => {
                        // The prefix byte width comes from the xEdit wbArray count arg:
                        //   -1 → 4 bytes (u32), -2 → 2 bytes (u16), -4 → 1 byte (u8).
                        // Read `width` bytes as a little-endian unsigned integer.
                        let w = *width;
                        if w > 0 && pos + w <= data.len() {
                            let mut n: usize = 0;
                            for i in 0..w {
                                n |= (data[pos + i] as usize) << (8 * i);
                            }
                            pos += w;
                            n
                        } else {
                            0
                        }
                    }
                    Some(ArrayCount::CountPath(path)) => {
                        count_path_value(&struct_out, ctx, path).unwrap_or(0) as usize
                    }
                    Some(ArrayCount::Fixed(n)) => *n,
                    _ => 0,
                };
                if n > 0
                    && let Some(elem_size) = field_byte_size(ctx, element)
                {
                    let mut items = Vec::with_capacity(n.min(4096));
                    // Snapshot current fields so element structs can resolve
                    // FieldValue deciders that reference parent-scope fields
                    // (e.g. "Form Type" for OMOD property enum selection).
                    let child_ctx = ctx.with_outer_struct(struct_out.clone());
                    for _ in 0..n {
                        if pos + elem_size > data.len() {
                            break;
                        }
                        let v =
                            decode_field_value(&child_ctx, element, &data[pos..pos + elem_size]);
                        items.push(v);
                        pos += elem_size;
                    }
                    if !items.is_empty() {
                        struct_out.insert(name.clone(), Node::Array(items));
                    }
                }
            }
            MemberDef::Unknown { name, .. } => {
                if pos < data.len() {
                    insert_unique(&mut struct_out, name.clone(), Node::raw(&data[pos..]));
                }
                break;
            }
            _ => {}
        }
    }
    apply_post_decode_rules(PostDecodeTarget::Struct(&mut struct_out), ctx);
    if !struct_out.is_empty() {
        out.insert(struct_name.to_string(), Node::Struct(struct_out));
    }
    pos
}

/// Returns the fixed byte size of a field when it can be determined statically.
/// Returns None for variable-length fields (NUL-terminated strings, fill-to-end bytes, etc.).
fn field_byte_size(ctx: &DecodeContext<'_>, field: &FieldDef) -> Option<usize> {
    if !member_version_ok(ctx.form_version, field) {
        return Some(0);
    }
    match field {
        MemberDef::Integer { width, .. } => Some(int_size(*width)),
        MemberDef::Float { .. } => Some(4),
        MemberDef::FormId { .. } => Some(4),
        MemberDef::ByteRgba { .. } => Some(4),
        MemberDef::Vec3 { .. } => Some(12),
        MemberDef::Unused { bytes, .. } => Some(*bytes),
        MemberDef::Empty { .. } => Some(0),
        MemberDef::Bytes { len: Some(n), .. } => Some(*n),
        MemberDef::Struct { fields, .. } => {
            let mut total = 0usize;
            for f in fields {
                total = total.checked_add(field_byte_size(ctx, f)?)?;
            }
            Some(total)
        }
        MemberDef::Array { element, count, .. } => {
            if let Some(ArrayCount::Fixed(n)) = count {
                field_byte_size(ctx, element)?.checked_mul(*n)
            } else {
                None
            }
        }
        MemberDef::Union {
            decider, variants, ..
        } => match decider {
            UnionDecider::ByteAtOffset { .. } | UnionDecider::FieldValue { .. } => {
                // Can't statically pick variant; check if all variants share the same size.
                let sizes: Vec<Option<usize>> =
                    variants.iter().map(|v| field_byte_size(ctx, v)).collect();
                let first = (*sizes.first()?)?;
                if sizes.iter().all(|s| *s == Some(first)) {
                    Some(first)
                } else {
                    None
                }
            }
            _ => {
                let idx = choose_union_variant(
                    ctx.form_version,
                    ctx.record_edid_char,
                    decider,
                    variants.len(),
                )?;
                variants.get(idx).and_then(|v| field_byte_size(ctx, v))
            }
        },
        _ => None,
    }
}

fn advance_union(ctx: &DecodeContext<'_>, variant: &MemberDef, data: &[u8], pos: usize) -> usize {
    match variant {
        MemberDef::Struct { name, fields, .. } => {
            let mut tmp = Fields::new();
            let consumed = decode_struct_fields(ctx, name, fields, data, &mut tmp);
            pos + consumed
        }
        _ => {
            let p = field_byte_size(ctx, variant).unwrap_or(0);
            pos + p.min(data.len())
        }
    }
}

fn decode_field_value(ctx: &DecodeContext<'_>, field: &FieldDef, data: &[u8]) -> Node {
    let mut m = Fields::new();
    decode_member(ctx, field, &mut m, data);
    if m.len() == 1 {
        m.into_values().next().unwrap()
    } else {
        Node::Struct(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{FormIdRefResolver, FormIdStub, ResolveDepth};
    use serde_json::{Map, Value, json};

    /// Bind `members` against `subrecords` the way a record's members bind,
    /// returning the rendered fields and the signatures left unbound.
    fn bind(
        ctx: &DecodeContext<'_>,
        members: Vec<MemberDef>,
        subrecords: &[OwnedSubrecord],
    ) -> (Map<String, Value>, Vec<String>) {
        let def = crate::schema::RecordDef {
            name: "Test".into(),
            members,
            unordered: false,
        };
        let mut cur = super::super::bind::Cursor::new(subrecords);
        let mut out = Fields::new();
        super::super::bind::bind_record(ctx, &def, &mut cur, &mut out);
        let unbound = cur
            .into_unbound()
            .iter()
            .map(|sr| sr.signature.as_str().to_owned())
            .collect();
        (rendered(ctx, out), unbound)
    }

    /// Render decoded fields to boundary JSON for assertions.
    fn rendered(ctx: &DecodeContext<'_>, fields: Fields) -> Map<String, Value> {
        match Node::Struct(fields).into_json(ctx) {
            Value::Object(map) => map,
            _ => unreachable!("a struct renders to an object"),
        }
    }
    use crate::formid::FormId;
    use crate::schema::{IntegerWidth, LStringTable, Schema};

    fn bare_ctx(schema: &Schema) -> DecodeContext<'_> {
        DecodeContext {
            schema,
            form_version: 208,
            is_localized: false,
            localization: None,
            curves: None,
            resolve_depth: crate::ResolveDepth::None,
            resolver: None,
            outer_struct: None,
            record_signature: None,
            record_edid_char: None,
        }
    }

    fn empty_schema() -> Schema {
        crate::schema::Schema::from_json(r#"{"records":{}}"#).unwrap()
    }

    fn int_field(name: &str, width: IntegerWidth) -> MemberDef {
        MemberDef::Integer {
            sig: None,
            name: name.to_string(),
            width,
            signed: false,
            format: None,
            from_version: None,
            below_version: None,
            from_size: None,
        }
    }

    fn prefix_array(name: &str, width: usize, elem: MemberDef) -> MemberDef {
        MemberDef::Array {
            sig: None,
            name: name.to_string(),
            element: Box::new(elem),
            count: Some(ArrayCount::CountPrefix(width)),
        }
    }

    fn sig_int_field(sig: &str, name: &str, width: IntegerWidth) -> MemberDef {
        MemberDef::Integer {
            sig: Some(sig.to_string()),
            name: name.to_string(),
            width,
            signed: false,
            format: None,
            from_version: None,
            below_version: None,
            from_size: None,
        }
    }

    fn subrecord(sig: &str, data: Vec<u8>, doc_index: usize) -> OwnedSubrecord {
        OwnedSubrecord {
            signature: crate::format::Signature::from_slice(sig.as_bytes()),
            data,
            doc_index,
        }
    }

    /// `CountPrefix(4)`: pins the 4-byte-prefix `Attach Parent Slots` / `Items`
    /// decode path.  The decoder must consume all 4 bytes and leave the trailing
    /// sentinel value intact.
    ///
    /// This is the hermetic, byte-exact mirror of the public-API integration
    /// test `omod_legendary_weapon_data_decodes_correctly` in
    /// `tests/decode_records/weapons.rs` — the 4-byte path is intentionally covered by
    /// both.  This unit test calls `decode_struct_fields` directly and pins the
    /// return value (bytes consumed), which is invisible at the `decode_record`
    /// boundary.  The `count_prefix_u8` test below is the *only* guard for the
    /// 1-byte / OBTS `Keywords` path.
    ///
    /// Buffer layout:
    ///   [00 00 00 00]  — u32 LE count prefix = 0  (no items)
    ///   [2A]           — sentinel u8 = 42
    #[test]
    fn count_prefix_u32_consumes_four_bytes() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let fields = vec![
            prefix_array("Items", 4, int_field("item", IntegerWidth::U32)),
            int_field("Sentinel", IntegerWidth::U8),
        ];
        let data: Vec<u8> = vec![0x00, 0x00, 0x00, 0x00, 0x2A];
        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Test", &fields, &data, &mut out);
        let out = rendered(&ctx, out);
        // decode_struct_fields nests all fields under the struct name key.
        let inner = out
            .get("Test")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        // Items absent (count=0, nothing inserted).
        assert!(
            inner.get("Items").is_none(),
            "empty Items array should be absent"
        );
        // Sentinel must land at offset 4, not 1.
        assert_eq!(
            inner.get("Sentinel").and_then(|v| v.as_u64()),
            Some(42),
            "Sentinel should be 42 (4-byte prefix consumed correctly)"
        );
    }

    /// `CountPrefix(1)`: lock the OBTS `Keywords` path to a 1-byte prefix;
    /// must not regress.
    ///
    /// Buffer layout:
    ///   [01]           — u8 count prefix = 1
    ///   [07 00 00 00]  — one u32 item = 7
    ///   [FF]           — sentinel u8 = 255
    #[test]
    fn count_prefix_u8_consumes_one_byte() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let fields = vec![
            prefix_array("Keywords", 1, int_field("kwd", IntegerWidth::U32)),
            int_field("Sentinel", IntegerWidth::U8),
        ];
        let data: Vec<u8> = vec![0x01, 0x07, 0x00, 0x00, 0x00, 0xFF];
        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Test", &fields, &data, &mut out);
        let out = rendered(&ctx, out);
        let inner = out
            .get("Test")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            inner
                .get("Keywords")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1),
            "should decode 1 keyword"
        );
        assert_eq!(
            inner.get("Sentinel").and_then(|v| v.as_u64()),
            Some(255),
            "Sentinel should be 255 (1-byte prefix consumed correctly)"
        );
    }

    #[test]
    fn array_count_path_reads_the_enclosing_record_scope() {
        // FSTS shape: XCNT holds the counts, and each DATA array is sized by
        // an XCNT field one scope up (xEdit `..\\XCNT\\Walking Count`).
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let counts = MemberDef::Struct {
            sig: Some("XCNT".into()),
            name: "Counts".into(),
            fields: vec![
                int_field("Walking", IntegerWidth::U32),
                int_field("Running", IntegerWidth::U32),
            ],
            from_version: None,
            below_version: None,
        };
        let counted = |name: &str, field: &str| MemberDef::Array {
            sig: None,
            name: name.into(),
            element: Box::new(int_field("Step", IntegerWidth::U32)),
            count: Some(ArrayCount::CountPath(CountPath {
                up: 1,
                path: vec!["Counts".into(), field.into()],
            })),
        };
        let data = MemberDef::Struct {
            sig: Some("DATA".into()),
            name: "Footsteps".into(),
            fields: vec![
                counted("Walking Steps", "Walking"),
                counted("Running Steps", "Running"),
            ],
            from_version: None,
            below_version: None,
        };
        let xcnt: Vec<u8> = [2u32, 1].iter().flat_map(|n| n.to_le_bytes()).collect();
        let payload: Vec<u8> = [7u32, 8, 9].iter().flat_map(|n| n.to_le_bytes()).collect();
        let subrecords = [subrecord("XCNT", xcnt, 0), subrecord("DATA", payload, 1)];

        let (out, _) = bind(&ctx, vec![counts, data], &subrecords);
        assert_eq!(out["Footsteps"]["Walking Steps"], json!([7, 8]));
        assert_eq!(out["Footsteps"]["Running Steps"], json!([9]));
    }

    #[test]
    fn rarray_count_path_bounds_repeated_subrecord_groups() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let morph_groups = MemberDef::RArray {
            name: "Morph Groups".into(),
            element: Box::new(MemberDef::RStruct {
                name: "Morph Group".into(),
                members: vec![
                    sig_int_field("MPPC", "Count", IntegerWidth::U32),
                    MemberDef::RArray {
                        name: "Morph Presets".into(),
                        element: Box::new(MemberDef::RStruct {
                            name: "Morph Preset".into(),
                            members: vec![sig_int_field("MPPI", "Index", IntegerWidth::U32)],
                            unordered: false,
                            any_member: false,
                            skip_sigs: Vec::new(),
                        }),
                        count: Some(ArrayCount::CountPath(CountPath {
                            up: 0,
                            path: vec!["Count".into()],
                        })),
                    },
                    sig_int_field("MPPK", "Tail", IntegerWidth::U16),
                ],
                unordered: false,
                any_member: false,
                skip_sigs: Vec::new(),
            }),
            count: None,
        };

        let subrecords = [
            subrecord("MPPC", 1u32.to_le_bytes().to_vec(), 0),
            subrecord("MPPI", 10u32.to_le_bytes().to_vec(), 1),
            subrecord("MPPK", 100u16.to_le_bytes().to_vec(), 2),
            subrecord("MPPC", 1u32.to_le_bytes().to_vec(), 3),
            subrecord("MPPI", 20u32.to_le_bytes().to_vec(), 4),
            subrecord("MPPK", 200u16.to_le_bytes().to_vec(), 5),
        ];

        let (out, _) = bind(&ctx, vec![morph_groups], &subrecords);
        let groups = out
            .get("Morph Groups")
            .and_then(|v| v.as_array())
            .expect("morph groups");

        assert_eq!(groups.len(), 2);
        for (idx, expected_index) in [10u64, 20u64].into_iter().enumerate() {
            let presets = groups[idx]
                .pointer("/Morph Group/Morph Presets")
                .and_then(|v| v.as_array())
                .expect("presets");
            assert_eq!(presets.len(), 1, "group {idx} should consume one preset");
            assert_eq!(
                presets[0]
                    .pointer("/Morph Preset/Index")
                    .and_then(|v| v.as_u64()),
                Some(expected_index)
            );
        }
    }

    fn vmad_wstring(s: &str) -> Vec<u8> {
        let mut out = (s.len() as u16).to_le_bytes().to_vec();
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn vmad_header(obj_format: u16, script_count: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&2u16.to_le_bytes()); // version
        out.extend_from_slice(&obj_format.to_le_bytes());
        out.extend_from_slice(&script_count.to_le_bytes());
        out
    }

    struct StubResolver {
        stubs: std::collections::HashMap<FormId, FormIdStub>,
    }

    impl FormIdRefResolver for StubResolver {
        fn stub(&self, id: FormId) -> Option<FormIdStub> {
            self.stubs.get(&id).cloned()
        }

        fn decode_full(&self, _id: FormId) -> Option<Value> {
            None
        }
    }

    /// COED owner-decider: NPC_ owner → Global Variable variant; no resolver → Unused.
    #[test]
    fn coed_owner_decider_selects_variant_by_target_signature() {
        use crate::schema::UnionDecider;
        use std::collections::HashMap;

        let owner_id = FormId::new(0x0000_1234);
        let glob_id = FormId::new(0x0000_00AB);
        let resolver = StubResolver {
            stubs: HashMap::from([(
                owner_id,
                FormIdStub {
                    formid: owner_id.display(),
                    editor_id: Some("TestNPC".into()),
                    record_type: "NPC_".into(),
                },
            )]),
        };

        let fields = vec![
            MemberDef::FormId {
                sig: None,
                name: "Owner".into(),
                valid_refs: vec!["NPC_".into(), "FACT".into(), "NULL".into()],
                from_version: None,
                below_version: None,
                from_size: None,
            },
            MemberDef::Union {
                sig: None,
                name: "union".into(),
                decider: UnionDecider::FormIdTargetType {
                    form_id_target_type: "Owner".into(),
                    map: HashMap::from([("NPC_".into(), 1), ("FACT".into(), 2)]),
                    default_variant: Some(0),
                },
                variants: vec![
                    MemberDef::Unused {
                        bytes: 4,
                        sig: None,
                        from_version: None,
                        below_version: None,
                    },
                    MemberDef::FormId {
                        sig: None,
                        name: "Global Variable".into(),
                        valid_refs: vec!["GLOB".into(), "NULL".into()],
                        from_version: None,
                        below_version: None,
                        from_size: None,
                    },
                    MemberDef::Integer {
                        sig: None,
                        name: "Required Rank".into(),
                        width: crate::schema::IntegerWidth::S32,
                        signed: true,
                        format: None,
                        from_version: None,
                        below_version: None,
                        from_size: None,
                    },
                ],
            },
        ];

        let mut payload = vec![0u8; 8];
        payload[0..4].copy_from_slice(&owner_id.raw().to_le_bytes());
        payload[4..8].copy_from_slice(&glob_id.raw().to_le_bytes());

        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.resolve_depth = ResolveDepth::Stub;
        ctx.resolver = Some(&resolver);

        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Extra Data", &fields, &payload, &mut out);
        let out = rendered(&ctx, out);
        let inner = out
            .get("Extra Data")
            .and_then(|v| v.as_object())
            .expect("struct");
        assert_eq!(
            inner.get("Global Variable").and_then(|v| v.as_str()),
            Some(glob_id.display().as_str())
        );

        // Without resolver, default variant 0 (Unused) — no Global Variable key.
        let ctx_no_resolver = bare_ctx(&schema);
        let mut out2 = Fields::new();
        decode_struct_fields(&ctx_no_resolver, "Extra Data", &fields, &payload, &mut out2);
        let out2 = rendered(&ctx_no_resolver, out2);
        let inner2 = out2
            .get("Extra Data")
            .and_then(|v| v.as_object())
            .expect("struct");
        assert!(inner2.get("Global Variable").is_none());
    }

    // -----------------------------------------------------------------------
    // EFIT — version-aware Effect Item struct (schema-native since B3)
    // -----------------------------------------------------------------------

    fn efit_fields() -> Vec<MemberDef> {
        vec![
            MemberDef::Integer {
                sig: None,
                name: "Effect ID".into(),
                width: IntegerWidth::U32,
                signed: false,
                format: None,
                from_version: Some(166),
                below_version: None,
                from_size: None,
            },
            MemberDef::Float {
                sig: None,
                name: "Magnitude".into(),
                from_version: None,
                below_version: None,
                from_size: None,
            },
            MemberDef::Integer {
                sig: None,
                name: "Area".into(),
                width: IntegerWidth::U32,
                signed: false,
                format: None,
                from_version: None,
                below_version: None,
                from_size: None,
            },
            MemberDef::Integer {
                sig: None,
                name: "Duration".into(),
                width: IntegerWidth::U32,
                signed: false,
                format: None,
                from_version: None,
                below_version: None,
                from_size: None,
            },
            MemberDef::Bytes {
                sig: None,
                name: "_unknown".into(),
                len: Some(12),
                from_version: Some(154),
                below_version: Some(166),
                from_size: None,
            },
            MemberDef::Bytes {
                sig: None,
                name: "_unknown".into(),
                len: Some(8),
                from_version: Some(166),
                below_version: Some(183),
                from_size: None,
            },
        ]
    }

    /// FV 197 (> 182): real Endangerol bytes — Effect ID + Magnitude + Area + Duration.
    #[test]
    fn efit_fv197_endangerol_bytes() {
        let data: [u8; 16] = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3e, 0x00, 0x00, 0x00, 0x00, 0x78, 0x00,
            0x00, 0x00,
        ];
        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.form_version = 197;
        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
        let out = rendered(&ctx, out);
        let obj = out
            .get("Effect Item Data")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(obj.get("Effect ID").and_then(|v| v.as_u64()), Some(0));
        let mag = obj.get("Magnitude").and_then(|v| v.as_f64()).unwrap();
        assert!((mag - 0.25).abs() < 1e-6);
        assert_eq!(obj.get("Area").and_then(|v| v.as_u64()), Some(0));
        assert_eq!(obj.get("Duration").and_then(|v| v.as_u64()), Some(120));
        assert!(obj.get("_unknown").is_none());
    }

    /// FV 170 (166-182): Effect ID present, 8-byte trailing unknown.
    #[test]
    fn efit_fv170_effect_id_and_trailing_unknown() {
        let mut data = [0u8; 24];
        data[0..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..8].copy_from_slice(&2.5f32.to_le_bytes());
        data[8..12].copy_from_slice(&3u32.to_le_bytes());
        data[12..16].copy_from_slice(&4u32.to_le_bytes());
        data[16..24].fill(0xAB);

        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.form_version = 170;
        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
        let out = rendered(&ctx, out);
        let obj = out
            .get("Effect Item Data")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(obj.get("Effect ID").and_then(|v| v.as_u64()), Some(1));
        let unk = obj.get("_unknown").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            unk.get("hex").and_then(|v| v.as_str()),
            Some("abababababababab")
        );
    }

    /// FV 160 (154-165): no Effect ID, 12-byte trailing unknown.
    #[test]
    fn efit_fv160_no_effect_id_trailing_unknown() {
        let mut data = [0u8; 24];
        data[0..4].copy_from_slice(&1.5f32.to_le_bytes());
        data[4..8].copy_from_slice(&5u32.to_le_bytes());
        data[8..12].copy_from_slice(&10u32.to_le_bytes());
        data[12..24].fill(0xCC);

        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.form_version = 160;
        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
        let out = rendered(&ctx, out);
        let obj = out
            .get("Effect Item Data")
            .and_then(|v| v.as_object())
            .unwrap();
        assert!(obj.get("Effect ID").is_none());
        let unk = obj.get("_unknown").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            unk.get("hex").and_then(|v| v.as_str()),
            Some("cccccccccccccccccccccccc")
        );
    }

    /// FV 150 (< 154): classic 12-byte layout — no Effect ID, no trailing unknown.
    #[test]
    fn efit_fv150_classic_layout() {
        let mut data = [0u8; 12];
        data[0..4].copy_from_slice(&3.0f32.to_le_bytes());
        data[4..8].copy_from_slice(&0u32.to_le_bytes());
        data[8..12].copy_from_slice(&30u32.to_le_bytes());

        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.form_version = 150;
        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
        let out = rendered(&ctx, out);
        let obj = out
            .get("Effect Item Data")
            .and_then(|v| v.as_object())
            .unwrap();
        assert!(obj.get("Effect ID").is_none());
        assert!(obj.get("_unknown").is_none());
    }

    /// Regression test: TERM's VMAD is `wbVMADFragmentedPERK` in xEdit's FO76
    /// definitions ("same fragments format as in PERK"), but the
    /// record-signature dispatch in `MemberDef::Vmad`'s decode arm had no
    /// `"TERM"` case, so it fell through to the generic `decode_vmad`, which
    /// stops after the base scripts array and never parses the fragment
    /// tail. A prize terminal (e.g. `Arcade_PrizeTerminal_Tier02`) stores its
    /// item-grant properties (`Form_NWOTShirt`, ...) as Object-type
    /// properties of that tail's script entry — they were silently dropped
    /// from the decoded record and therefore from the xref index (`refs`).
    /// Pin that TERM now dispatches through `decode_vmad_perk` and the
    /// tail's Object-property FormID surfaces intact.
    #[test]
    fn vmad_term_dispatches_to_perk_fragment_decoder_and_decodes_tail_formid() {
        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.record_signature = Some("TERM");

        // Header: version=2, obj_format=2, script_count=0 — the prize
        // property lives in the fragment tail's script_entry, not the base
        // scripts array.
        let mut data = vmad_header(2, 0);
        data.push(4); // extra_bind_data_version (s8)
        // script_entry: name + status + prop_count=1 + one Object property
        data.extend(vmad_wstring("Arcade_PrizeTerminal_Tier02"));
        data.push(0); // status
        data.extend_from_slice(&1u16.to_le_bytes()); // prop_count
        data.extend(vmad_wstring("Form_NWOTShirt"));
        data.push(1); // type = object
        data.push(1); // status
        // Object format 2: Unused(u16) + Alias(s16) + FormID(u32) =
        // 0x006677E5 (little-endian, matching the real ESM bytes).
        data.extend_from_slice(&[0x00, 0x00, 0xff, 0xff, 0xe5, 0x77, 0x66, 0x00]);
        data.extend_from_slice(&0u16.to_le_bytes()); // frag_count = 0

        let subrecords = [subrecord("VMAD", data, 0)];

        let member = MemberDef::Vmad {
            sig: Some("VMAD".into()),
            name: "Virtual Machine Adapter".into(),
        };
        let (out, _) = bind(&ctx, vec![member], &subrecords);

        let decoded = out
            .get("Virtual Machine Adapter")
            .expect("VMAD member must be decoded");
        assert!(
            decoded.get("_raw").is_none(),
            "must not truncate: {decoded}"
        );
        let value = decoded
            .pointer("/script_fragments/script_entry/properties/0/value")
            .and_then(|v| v.as_str());
        assert_eq!(
            value,
            Some("0x006677E5"),
            "TERM's fragment-tail Object property must decode via the PERK \
             dispatch, not vanish through the generic decode_vmad fallback"
        );
    }

    /// Bind an `lstring` member to one subrecord.
    fn decode_lstring(ctx: &DecodeContext<'_>, sr: &OwnedSubrecord) -> Map<String, Value> {
        let member = MemberDef::LString {
            sig: Some("DESC".into()),
            name: "Description".into(),
            table: LStringTable::Dlstrings,
        };
        bind(ctx, vec![member], std::slice::from_ref(sr)).0
    }

    /// "No string present" must decode to `Value::Null` in BOTH localization
    /// modes, and the key must always be emitted when the subrecord exists.
    ///
    /// The two modes encode the field differently on disk (4-byte table ID vs
    /// inline NUL-terminated text), so a mode-dependent encoding of "empty"
    /// makes every nameless record look changed when a localized snapshot is
    /// diffed against a non-localized one. The 20260710 -> 20260717 pair is
    /// exactly that case (`flags` 0x01 vs 0x81) and produced 50,720 bogus
    /// `"" -> null` rows plus 11,860 `null -> null` rows from the omitted key.
    #[test]
    fn empty_lstring_decodes_to_null_in_both_localization_modes() {
        let schema = empty_schema();

        // Non-localized: inline empty string (bare NUL terminator).
        let non_loc = bare_ctx(&schema);
        let out = decode_lstring(&non_loc, &subrecord("DESC", vec![0x00], 0));
        assert_eq!(
            out.get("Description"),
            Some(&Value::Null),
            "empty inline lstring must decode to null, not \"\""
        );

        // Localized: the id==0 "no string" sentinel.
        let mut loc = bare_ctx(&schema);
        loc.is_localized = true;
        let out = decode_lstring(&loc, &subrecord("DESC", vec![0, 0, 0, 0], 0));
        assert_eq!(out.get("Description"), Some(&Value::Null));

        // Localized: truncated payload (<4 bytes) must still emit the key,
        // rather than omitting it and pairing with the other side as a change.
        let out = decode_lstring(&loc, &subrecord("DESC", vec![0x01], 0));
        assert_eq!(
            out.get("Description"),
            Some(&Value::Null),
            "short localized lstring payload must emit an explicit null key"
        );
    }

    /// The empty-is-null normalization must not swallow real text.
    #[test]
    fn non_empty_inline_lstring_still_decodes_to_text() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);

        let mut data = b"Vote Counter".to_vec();
        data.push(0);
        let out = decode_lstring(&ctx, &subrecord("DESC", data, 0));
        assert_eq!(out.get("Description"), Some(&json!("Vote Counter")));

        // `<ID=...>`-prefixed form: prefix stripped, remainder preserved.
        let mut data = b"<ID=0001A2B3>Vote Counter".to_vec();
        data.push(0);
        let out = decode_lstring(&ctx, &subrecord("DESC", data, 0));
        assert_eq!(out.get("Description"), Some(&json!("Vote Counter")));

        // A prefix with nothing after it is still "no string".
        let mut data = b"<ID=0001A2B3>".to_vec();
        data.push(0);
        let out = decode_lstring(&ctx, &subrecord("DESC", data, 0));
        assert_eq!(out.get("Description"), Some(&Value::Null));
    }

    /// Inline text must decode to what the snapshot's UTF-8 string table
    /// holds for the same ID, or every such string looks changed when the
    /// Localized flag flips: whitespace after the prefix is kept, and the
    /// bytes are Windows-1252 (FO76's `¬` legendary star glyph, `•` bullets).
    #[test]
    fn inline_lstring_decodes_like_the_string_table() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let cases: [(&[u8], &str); 3] = [
            (b"<ID=000359F8> days", " days"),
            (b"<ID=610217FF>LEGENDARY MOD \xac\xac", "LEGENDARY MOD ¬¬"),
            (b"\x95\x93Quote\x94 \x81", "•“Quote” \u{81}"),
        ];
        for (raw, want) in cases {
            let mut data = raw.to_vec();
            data.push(0);
            let out = decode_lstring(&ctx, &subrecord("DESC", data, 0));
            assert_eq!(out.get("Description"), Some(&json!(want)), "{raw:?}");
        }
    }

    /// A sig-bearing `Unused` member (Pascal `wbUnused(INDX, 0)` — an entire
    /// subrecord whose payload is intentionally ignored) binds its subrecord
    /// and emits nothing, so the subrecord never shows up as `_unmapped`.
    #[test]
    fn unused_with_sig_consumes_subrecord_and_emits_nothing() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let member = MemberDef::Unused {
            bytes: 0,
            sig: Some("INDX".into()),
            from_version: None,
            below_version: None,
        };
        let subrecords = [subrecord("INDX", vec![0x01, 0x02, 0x03, 0x04], 0)];

        let (out, unbound) = bind(&ctx, vec![member], &subrecords);

        assert!(
            out.is_empty(),
            "sig-bearing Unused must emit no output key, got {out:?}"
        );
        assert!(
            unbound.is_empty(),
            "sig-bearing Unused must bind its subrecord, leaving nothing \
             behind to show up as _unmapped"
        );
    }

    /// Payload-context `Unused` (no `sig`, the pre-existing/common case: byte
    /// padding skipped *within* an already-consumed struct payload) must keep
    /// working unchanged — this is a guard against Fix E's `sig` addition
    /// regressing the far more common path.
    #[test]
    fn unused_without_sig_still_skips_payload_bytes_only() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let fields = vec![
            MemberDef::Unused {
                bytes: 3,
                sig: None,
                from_version: None,
                below_version: None,
            },
            int_field("Sentinel", IntegerWidth::U8),
        ];
        let data: Vec<u8> = vec![0xAA, 0xBB, 0xCC, 0x2A];
        let mut out = Fields::new();
        decode_struct_fields(&ctx, "Test", &fields, &data, &mut out);
        let out = rendered(&ctx, out);
        let inner = out
            .get("Test")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            inner.get("Sentinel").and_then(|v| v.as_u64()),
            Some(42),
            "Sentinel should be 42 (3 padding bytes skipped correctly)"
        );
    }
}
