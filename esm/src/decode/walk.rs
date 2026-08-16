use crate::reader::OwnedSubrecord;
use crate::schema::{ArrayCount, FieldDef, LStringTable, MemberDef, UnionDecider};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};

use super::model_info::decode_model_info;
use super::rules::{PostDecodeTarget, apply_post_decode_rules};
use super::scalars::{
    choose_union_variant, field_int_value, field_value_key, int_size, member_from_size_ok,
    member_version_ok, read_le_uint, scalar_bytes, scalar_float, scalar_formid, scalar_int,
    scalar_rgba, scalar_string, scalar_vec3, sibling_target_sig,
};
use super::scope::*;
use super::vmad::{
    decode_vmad, decode_vmad_info, decode_vmad_pack, decode_vmad_perk, decode_vmad_qust,
    decode_vmad_scen,
};
use super::{DecodeContext, hex, lstring_table_to_kind, markers};

pub(crate) fn decode_member(
    ctx: &DecodeContext<'_>,
    member: &MemberDef,
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, VecDeque<&OwnedSubrecord>>,
    payload: Option<&[u8]>,
) {
    if !member_version_ok(ctx.form_version, member) {
        return;
    }

    match member {
        MemberDef::Struct {
            sig, name, fields, ..
        } => decode_struct_member(ctx, sig, name, fields, out, by_sig, payload),
        MemberDef::Integer {
            sig,
            name,
            width,
            signed,
            format,
            stop_before,
            ..
        } => {
            if let Some(data) = payload {
                if let Some(v) = scalar_int(data, *width, *signed, format.as_ref()) {
                    out.insert(name.clone(), v);
                }
            } else if let Some(sig) = sig {
                // If stop_before is set and a boundary sig precedes this
                // integer in document order, defer — leave the subrecord in
                // the pool for the correctly-positioned schema member.
                if !stop_before.is_empty() && stop_before_check(by_sig, sig, stop_before) {
                    // deferred
                } else if let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
                    && let Some(v) = scalar_int(&sr.data, *width, *signed, format.as_ref())
                {
                    out.insert(name.clone(), v);
                }
            }
        }
        MemberDef::Float { sig, name, .. } => {
            if let Some(data) = payload {
                if let Some(v) = scalar_float(data) {
                    out.insert(name.clone(), v);
                }
            } else if let Some(sig) = sig
                && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
                && let Some(v) = scalar_float(&sr.data)
            {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::String {
            sig, name, sized, ..
        } => {
            if let Some(sig) = sig
                && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
            {
                out.insert(name.clone(), scalar_string(&sr.data, sized));
            }
        }
        MemberDef::LString { sig, name, table } => {
            decode_lstring_member(ctx, sig, name, table, out, by_sig)
        }
        MemberDef::FormId {
            sig,
            name,
            valid_refs,
            ..
        } => {
            if let Some(data) = payload {
                if let Some(v) = scalar_formid(ctx, valid_refs, data) {
                    out.insert(name.clone(), v);
                }
            } else if let Some(sig) = sig
                && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
                && let Some(v) = scalar_formid(ctx, valid_refs, &sr.data)
            {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::Bytes { sig, name, len, .. } => {
            if let Some(data) = payload {
                let n = len.unwrap_or(data.len());
                out.insert(name.clone(), scalar_bytes(&data[..data.len().min(n)]));
            } else if let Some(sig) = sig
                && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
            {
                let n = len.unwrap_or(sr.data.len());
                out.insert(name.clone(), scalar_bytes(&sr.data[..sr.data.len().min(n)]));
            }
        }
        MemberDef::ByteRgba { sig, name, .. } => {
            if let Some(sig) = sig
                && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
                && let Some(v) = scalar_rgba(&sr.data)
            {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::Vec3 { sig, name } => {
            // Two calling shapes, mirroring the Struct arm above: (1) called with
            // an explicit `payload` slice already in hand — e.g. as a bare array
            // element via decode_field_value (NVNM's Vertices is the first real
            // exercise of this path); (2) called with no payload but its own
            // `sig`, so it must pull its own subrecord's bytes via `by_sig`.
            // Before this fix only (2) was handled, so a sig-less Vec3 array
            // element silently decoded to `{}` (nothing inserted into `out`).
            if let Some(data) = payload {
                if let Some(v) = scalar_vec3(data) {
                    out.insert(name.clone(), v);
                }
            } else if let Some(sig) = sig
                && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
                && let Some(v) = scalar_vec3(&sr.data)
            {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::RStruct { name, members } => {
            let (scope_min, scope_max) = rstruct_present_signature_scope(by_sig, members);
            let scoped_ctx = if scope_min.is_some() || scope_max.is_some() {
                &ctx.with_scope(scope_min, scope_max)
            } else {
                ctx
            };
            let mut group = Map::new();
            for m in members {
                decode_member(scoped_ctx, m, &mut group, by_sig, None);
            }
            if !group.is_empty() {
                out.insert(name.clone(), Value::Object(group));
            }
        }
        MemberDef::RArray {
            name,
            element,
            count,
            stop_before,
        } => decode_rarray_member(ctx, name, element, count, stop_before, out, by_sig),
        MemberDef::Array {
            sig,
            name,
            element,
            count,
        } => decode_array_member(ctx, sig, name, element, count, out, by_sig, payload),
        MemberDef::Union {
            sig,
            name,
            decider,
            variants,
        } => decode_union_member(ctx, sig, name, decider, variants, out, by_sig, payload),
        MemberDef::Empty { sig, name, .. } => {
            if let Some(sig) = sig {
                // Unscoped (`take_first`, not `take_first_in_scope`): QUST
                // alias bodies close with an `Empty{sig:"ALED"}` ("Alias
                // End") member, and `rstruct_present_signature_scope` defines
                // an alias's own scope_max as "up to but NOT including the
                // next ALED" (see its doc comment) — i.e. exclusive of the
                // alias's own closing ALED subrecord. Scoping this arm made
                // an alias unable to consume its own terminator (doc_index ==
                // scope_max is out of range), regressing
                // qust_gq_horde_alias_fill_decodes_correctly /
                // qust_gq_workshop_reclaim_decodes_correctly with a leftover
                // `_unmapped 'ALED'`. No concrete real-ESM case currently
                // needs Empty scoped (GMRW's ITME terminator is bounded by
                // the RArray's own inclusive `term_idx + 1` upper bound in
                // the RArray arm above, not by this take), so it stays
                // unscoped rather than special-casing ALED here.
                //
                // Only emit the marker when the empty subrecord is actually present.
                if take_first(by_sig, sig).is_some() {
                    out.insert(name.clone(), json!(null));
                }
            }
        }
        MemberDef::Unused { bytes, sig, .. } => {
            if let Some(data) = payload {
                // Payload-context skip: bytes already consumed as part of an
                // enclosing struct/subrecord, nothing left to look up in `by_sig`.
                let _ = data.get(..*bytes);
            } else if let Some(sig) = sig {
                // Subrecord-level `wbUnused(SIG, 0)`: consume and discard the
                // whole subrecord so it doesn't linger as `_unmapped`.
                let _ = take_first_in_scope(by_sig, sig, ctx);
            }
        }
        MemberDef::Unknown { sig, name } => {
            if let Some(sig) = sig
                && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
            {
                out.insert(
                    name.clone(),
                    json!({
                        "hex": hex::encode(&sr.data),
                        "_raw": true
                    }),
                );
            }
        }
        MemberDef::RawFallback { sig, name, reason } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
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
        MemberDef::Vmad { sig, name } => decode_vmad_member(ctx, sig, name, out, by_sig),
        MemberDef::Ctda { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
                    out.insert(name.clone(), crate::ctda::decode_ctda(&sr.data, ctx));
                }
            } else if let Some(data) = payload {
                out.insert(name.clone(), crate::ctda::decode_ctda(data, ctx));
            }
        }
        MemberDef::ModelInfo { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
                    out.insert(name.clone(), decode_model_info(&sr.data));
                }
            } else if let Some(data) = payload {
                out.insert(name.clone(), decode_model_info(data));
            }
        }
    }
}

pub(super) fn decode_struct_member(
    ctx: &DecodeContext<'_>,
    sig: &Option<String>,
    name: &str,
    fields: &[FieldDef],
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, VecDeque<&OwnedSubrecord>>,
    payload: Option<&[u8]>,
) {
    if let Some(payload) = payload {
        decode_struct_fields(ctx, name, fields, payload, out);
    } else if let Some(sig) = sig
        && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
    {
        let child_ctx = if fields.iter().any(contains_field_value_union) {
            Some(ctx.with_outer_struct(out.clone()))
        } else {
            None
        };
        let decode_ctx = child_ctx.as_ref().unwrap_or(ctx);
        decode_struct_fields(decode_ctx, name, fields, &sr.data, out);
    }
}

pub(super) fn decode_lstring_member(
    ctx: &DecodeContext<'_>,
    sig: &Option<String>,
    name: &str,
    table: &LStringTable,
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, VecDeque<&OwnedSubrecord>>,
) {
    if let Some(sig) = sig
        && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
    {
        // "No string present" must decode to the same JSON in both
        // modes (`Value::Null`). The two representations are not
        // interchangeable on the wire — localized files store a
        // 4-byte ID, non-localized files store inline text — so a
        // mode-dependent encoding of "empty" makes every nameless
        // record look changed when a localized snapshot is diffed
        // against a non-localized one.
        let value = if ctx.is_localized {
            // Localized ESM: field is a 4-byte ID into string tables.
            if sr.data.len() < 4 {
                Value::Null
            } else {
                let id = u32::from_le_bytes(sr.data[0..4].try_into().unwrap());
                if id == 0 {
                    // 0 is the engine's "no string" sentinel, not a
                    // missing table entry — mirrors resolve_formid's
                    // null-FormID special case.
                    Value::Null
                } else {
                    let kind = lstring_table_to_kind(table, ctx.record_signature, sig);
                    match ctx.localization.and_then(|loc| loc.lookup(kind, id)) {
                        Some(text) => json!(text),
                        None => json!({
                            "lstring_id": format!("0x{:08X}", id),
                            (markers::UNRESOLVED): true
                        }),
                    }
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
            if text.is_empty() {
                Value::Null
            } else {
                json!(text)
            }
        };
        out.insert(name.to_owned(), value);
    }
}

pub(super) fn decode_rarray_member(
    ctx: &DecodeContext<'_>,
    name: &str,
    element: &MemberDef,
    count: &Option<ArrayCount>,
    stop_before: &[String],
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, VecDeque<&OwnedSubrecord>>,
) {
    let mut items = Vec::new();
    let target_count = rarray_count(count.as_ref(), out, ctx);
    let anchor = anchor_sig(element);
    // Elements whose RStruct ends in a sig-bearing `Empty` terminator
    // (e.g. GMRW Reward's trailing `ITME` "Reward End Marker") can
    // fall back to partitioning by that terminator's doc_index
    // instead of by the element's *leading* anchor, for the
    // iterations where the anchor-based scope below comes up empty.
    // This matters when the leading anchor is optional and can be
    // absent on every element in a given record (GMRW Reward's
    // `CTRG` is one such case): an anchor-based scope degrades to
    // "unscoped" the moment the anchor sig is missing, letting every
    // member's own `by_sig` pop bleed across element boundaries
    // (column-wise mixing).
    //
    // This is a per-*iteration* fallback, not a whole-array
    // strategy switch: a trailing sig-bearing `Empty` is
    // not always a true one-per-element terminator — e.g. MESG Menu
    // Button's trailing `MBNR` ("No Response") has the same shape
    // but is itself an optional per-element flag, present on only
    // some buttons in a given record. Using it as an array-wide
    // terminator would misattribute its (sparse) doc_index as the
    // bound for buttons that never carried it. Reaching for it only
    // when the *anchor* (normally reliable — `ITXT`/"Button Text" is
    // mandatory) fails to produce a scope keeps well-behaved arrays
    // like Menu Buttons on their existing anchor-based path
    // untouched, while rescuing GMRW's Reward array, whose anchor
    // (`CTRG`) is unconditionally absent so anchor-based scoping
    // fails on every single iteration.
    let terminator = element_terminator_sig(element);
    let mut next_element_start: usize = 0;
    while target_count.is_none_or(|n| items.len() < n) {
        // If stop_before is set, halt when a boundary sig precedes
        // the element's anchor in document order.
        if !stop_before.is_empty()
            && let Some(anchor) = anchor
            && stop_before_check(by_sig, anchor, stop_before)
        {
            break;
        }
        let before: usize = by_sig.values().map(|v| v.len()).sum();

        // Bound this element to [its own anchor's doc_index, the next
        // anchor's doc_index) before decoding it. `by_sig` is one
        // global FIFO queue per signature across the whole record, so
        // without this an element's *optional* trailing sig-bearing
        // members (e.g. ALCH/SPEL Effect's CVT0/MAGA/DURG/MAGG/CODV)
        // can be stolen from a later element that happens to share
        // the same signature — the earlier element decodes with the
        // later element's subrecord instead of leaving it absent.
        // Mandatory members (present on every element, e.g.
        // EFID/EFIT) are unaffected: FIFO order already aligns them
        // correctly, and `take_first_in_scope` is a no-op restriction
        // when the popped subrecord is genuinely this element's own.
        let element_scope = anchor.and_then(|sig| {
            by_sig.get(sig).and_then(|queue| {
                let mut iter = queue.iter();
                iter.next()
                    .map(|first| (first.doc_index, iter.next().map(|second| second.doc_index)))
            })
        });

        let scoped_ctx;
        let element_ctx: &DecodeContext<'_>;
        match element_scope {
            Some((current_idx, next_idx)) => {
                scoped_ctx = ctx.with_scope(Some(current_idx), next_idx);
                // Track a floor for a *future* iteration's terminator
                // fallback, in case a later element in this same
                // array lacks the anchor (mixed presence).
                if let Some(next_idx) = next_idx {
                    next_element_start = next_idx;
                }
                element_ctx = &scoped_ctx;
            }
            None => {
                // Anchor-based scoping produced nothing (no anchor,
                // or the anchor sig's queue is exhausted/absent for
                // this element) — fall back to terminator-based
                // scoping: [next_element_start, terminator_doc_index
                // + 1). The `+ 1` includes the terminator subrecord
                // itself so the element's own trailing Empty member
                // can still consume it.
                let term_idx = terminator.and_then(|term_sig| {
                    by_sig
                        .get(term_sig)
                        .and_then(|q| q.front())
                        .map(|sr| sr.doc_index)
                });
                match term_idx {
                    Some(term_idx) => {
                        scoped_ctx = ctx.with_scope(Some(next_element_start), Some(term_idx + 1));
                        // Advance past this terminator for the next
                        // iteration. Since this element's own decode
                        // below consumes the terminator (its trailing
                        // Empty member pops the front of the
                        // terminator's queue), the queue's front
                        // necessarily moves forward each iteration —
                        // this loop cannot spin forever on the same
                        // terminator.
                        next_element_start = term_idx + 1;
                        element_ctx = &scoped_ctx;
                    }
                    // No anchor and no terminator left either —
                    // decode fully unscoped, same as before this fix.
                    None => {
                        element_ctx = ctx;
                    }
                }
            }
        }

        let mut item = Map::new();
        decode_member(element_ctx, element, &mut item, by_sig, None);
        let after: usize = by_sig.values().map(|v| v.len()).sum();
        if before == after {
            break; // no subrecords consumed — done
        }
        items.push(Value::Object(item));
    }
    if !items.is_empty() {
        out.insert(name.to_owned(), Value::Array(items));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn decode_array_member(
    ctx: &DecodeContext<'_>,
    sig: &Option<String>,
    name: &str,
    element: &FieldDef,
    count: &Option<ArrayCount>,
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, VecDeque<&OwnedSubrecord>>,
    payload: Option<&[u8]>,
) {
    if let Some(sig) = sig {
        let taken = take_all(by_sig, sig);
        // A single subrecord may pack multiple fixed-size elements (e.g. KWDA
        // packs every keyword FormID into one subrecord, counted by KSIZ; APPR
        // packs attach-parent-slot FormIDs similarly).  Split each subrecord by
        // the element's static byte size when it is known and the subrecord is
        // strictly larger; otherwise fall back to one element per subrecord so
        // variable-size element arrays are unaffected.
        let elem_size = field_byte_size(ctx, element);
        let mut items: Vec<Value> = Vec::new();
        for sr in taken {
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
                    // count-prefixed array): the subrecord may pack one or
                    // more instances back-to-back with no static per-element
                    // size. Loop using the real consumed-byte count per
                    // instance (mirrors advance_union) until the subrecord's
                    // data is exhausted, instead of decoding only the first
                    // instance and silently dropping the rest.
                    if let MemberDef::Struct {
                        name: elem_name,
                        fields,
                        ..
                    } = element
                    {
                        let mut pos = 0;
                        while pos < sr.data.len() {
                            let mut elem_out = Map::new();
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
                            if let Some(v) = elem_out.remove(elem_name) {
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
            out.insert(name.to_owned(), Value::Array(items));
        }
    } else if let (Some(data), Some(ArrayCount::Fixed(n))) = (payload, count) {
        // No sig: a nested array element (e.g. the inner dimension of an
        // array-of-arrays, such as CELL's 32x32 Max Height Data grid)
        // reached via decode_field_value with its own byte slice as
        // `payload`. Only the Fixed-count shape is handled here (the only
        // one that currently occurs in this position) — mirrors
        // decode_struct_fields's packed Array arm but starting at position
        // 0 of the given slice, since decode_field_value already hands us
        // exactly this one array instance's bytes.
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
                out.insert(name.to_owned(), Value::Array(items));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn decode_union_member(
    ctx: &DecodeContext<'_>,
    sig: &Option<String>,
    name: &str,
    decider: &UnionDecider,
    variants: &[MemberDef],
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, VecDeque<&OwnedSubrecord>>,
    payload: Option<&[u8]>,
) {
    // If the union has a sig, consume the subrecord and use its bytes as payload.
    let taken = sig
        .as_deref()
        .and_then(|s| take_first_in_scope(by_sig, s, ctx));
    let taken_data: Option<&[u8]> = taken.as_ref().map(|sr| sr.data.as_slice());
    let effective_payload = taken_data.or(payload);

    let chosen = match decider {
        UnionDecider::FieldValue {
            field,
            map,
            default_variant,
            bits,
        } => {
            // Bitmask check first (for flag-field deciders like wbBOOKTeachesDecider).
            let by_bits = if !bits.is_empty() {
                let raw = field_int_value(out, field).or_else(|| {
                    ctx.outer_struct
                        .as_ref()
                        .and_then(|o| field_int_value(o, field))
                });
                raw.and_then(|v| {
                    bits.iter().find_map(|[mask, var_idx]| {
                        if v & mask != 0 {
                            Some(*var_idx as usize)
                        } else {
                            None
                        }
                    })
                })
            } else {
                None
            };
            by_bits
                .or_else(|| {
                    field_value_key(out, field)
                        .or_else(|| {
                            ctx.outer_struct
                                .as_ref()
                                .and_then(|o| field_value_key(o, field))
                        })
                        .and_then(|k| map.get(&k).copied())
                })
                .or(*default_variant)
        }
        UnionDecider::ByteAtOffset {
            byte_offset,
            map,
            default_variant,
            width_bytes,
        } => effective_payload
            .and_then(|p| read_le_uint(p, *byte_offset, *width_bytes))
            .and_then(|b| map.get(&b.to_string()).copied())
            .or(*default_variant),
        UnionDecider::PayloadSize {
            payload_size,
            default_variant,
        } => effective_payload
            .and_then(|p| payload_size.get(&p.len().to_string()).copied())
            .or(*default_variant),
        UnionDecider::PresentSignature { present_signature } => {
            // wbRUnion: select the variant whose anchor subrecord appears
            // earliest in the document (lowest doc_index).  Each variant
            // may have multiple anchor sigs (nested-union branches).
            // When `scope_*_doc_index` is set (QUST alias bodies), only
            // anchors inside that range are considered so later aliases
            // cannot steal fill-type subrecords.
            let in_scope = |idx: usize| doc_index_in_present_signature_scope(ctx, idx);
            present_signature
                .iter()
                .enumerate()
                .filter_map(|(i, anchors)| {
                    anchors
                        .iter()
                        .filter_map(|anchor| {
                            by_sig.get(anchor.as_str()).and_then(|subs| {
                                subs.iter()
                                    .map(|sr| sr.doc_index)
                                    .find(|&idx| in_scope(idx))
                            })
                        })
                        .min()
                        .map(|doc_idx| (i, doc_idx))
                })
                .min_by_key(|&(_, doc_idx)| doc_idx)
                .map(|(i, _)| i)
        }
        UnionDecider::FormIdTargetType {
            form_id_target_type,
            map,
            default_variant,
        } => out
            .get(form_id_target_type)
            .or_else(|| {
                ctx.outer_struct
                    .as_ref()
                    .and_then(|o| o.get(form_id_target_type))
            })
            .and_then(|v| sibling_target_sig(v, ctx))
            .and_then(|sig| map.get(&sig).copied())
            .or(*default_variant),
        _ => choose_union_variant(
            ctx.form_version,
            ctx.record_edid_char,
            decider,
            variants.len(),
        ),
    };
    if let Some(idx) = chosen
        && let Some(variant) = variants.get(idx)
    {
        // Decode into a temporary map first: some variants are
        // anonymous (Pascal `wbInteger('', ...)` reusing the
        // union's own name conceptually), so their decoded value
        // would otherwise land under the empty-string key instead
        // of the union's own (correctly-deduped) name.
        let mut tmp = Map::new();
        decode_member(ctx, variant, &mut tmp, by_sig, effective_payload);
        for (k, v) in tmp {
            let key = if k.is_empty() { name.to_owned() } else { k };
            insert_unique(out, key, v);
        }
        return;
    }
    if let UnionDecider::PresentSignature { present_signature } = decider {
        let in_scope = |idx: usize| doc_index_in_present_signature_scope(ctx, idx);
        let any_anchor_in_scope = present_signature.iter().flatten().any(|anchor| {
            by_sig
                .get(anchor.as_str())
                .is_some_and(|subs| subs.iter().any(|sr| in_scope(sr.doc_index)))
        });
        if !any_anchor_in_scope {
            return;
        }
    }
    out.insert(
        name.to_owned(),
        json!({
            "_raw": true,
            "reason": "union decider unresolved"
        }),
    );
}

pub(super) fn decode_vmad_member(
    ctx: &DecodeContext<'_>,
    sig: &Option<String>,
    name: &str,
    out: &mut Map<String, Value>,
    by_sig: &mut HashMap<String, VecDeque<&OwnedSubrecord>>,
) {
    if let Some(sig) = sig
        && let Some(sr) = take_first_in_scope(by_sig, sig, ctx)
    {
        let decoded = match ctx.record_signature {
            Some("QUST") => decode_vmad_qust(ctx, &sr.data),
            Some("INFO") => decode_vmad_info(ctx, &sr.data),
            Some("PACK") => decode_vmad_pack(ctx, &sr.data),
            Some("PERK") => decode_vmad_perk(ctx, &sr.data),
            Some("SCEN") => decode_vmad_scen(ctx, &sr.data),
            // TERM wires wbVMADFragmentedPERK in xEdit's FO76 definitions
            // ("same fragments format as in PERK") — reuse that decoder so
            // the fragment tail's script-entry properties (e.g. a prize
            // terminal's `Form_*` item grants) are decoded and harvested
            // into the xref index instead of being silently dropped by the
            // generic `decode_vmad`, which stops after the base scripts.
            Some("TERM") => decode_vmad_perk(ctx, &sr.data),
            _ => decode_vmad(ctx, &sr.data),
        };
        out.insert(name.to_owned(), decoded);
    }
}

/// Insert `value` into `map` under `key`. If `key` is already present, try
/// `"key 2"`, `"key 3"`, … to avoid silently clobbering an earlier value.
///
/// This handles schema patterns where the same `wbXxx` definition is reused
/// for two different struct slots (e.g. MGEF's two `wbActorValue` fields).
fn insert_unique(map: &mut Map<String, Value>, key: String, value: Value) {
    if !map.contains_key(&key) {
        map.insert(key, value);
        return;
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{key} {n}");
        if !map.contains_key(&candidate) {
            map.insert(candidate, value);
            return;
        }
        n += 1;
    }
}

/// Returns true when `member` or any nested field uses a `FieldValue` union decider.
fn contains_field_value_union(member: &MemberDef) -> bool {
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
    out: &mut Map<String, Value>,
) -> usize {
    let mut pos = 0usize;
    let mut struct_out = Map::new();
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
                    if let Some(v) = scalar_formid(ctx, valid_refs, &data[pos..]) {
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
                    struct_out.insert(
                        name.clone(),
                        json!({
                            "hex": hex::encode(&data[pos..]),
                            "_raw": true,
                            "reason": reason
                        }),
                    );
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
                let chosen = match decider {
                    UnionDecider::ByteAtOffset {
                        byte_offset,
                        map,
                        default_variant,
                        width_bytes,
                    } => read_le_uint(data, pos + byte_offset, *width_bytes)
                        .and_then(|b| map.get(&b.to_string()).copied())
                        .or(*default_variant),
                    UnionDecider::FieldValue {
                        field,
                        map,
                        default_variant,
                        bits,
                    } => {
                        // Bitmask check first.
                        let by_bits = if !bits.is_empty() {
                            let raw = field_int_value(&struct_out, field).or_else(|| {
                                ctx.outer_struct
                                    .as_ref()
                                    .and_then(|o| field_int_value(o, field))
                            });
                            raw.and_then(|v| {
                                bits.iter().find_map(|[mask, var_idx]| {
                                    if v & mask != 0 {
                                        Some(*var_idx as usize)
                                    } else {
                                        None
                                    }
                                })
                            })
                        } else {
                            None
                        };
                        by_bits
                            .or_else(|| {
                                field_value_key(&struct_out, field)
                                    .or_else(|| {
                                        ctx.outer_struct
                                            .as_ref()
                                            .and_then(|o| field_value_key(o, field))
                                    })
                                    .and_then(|k| map.get(&k).copied())
                            })
                            .or(*default_variant)
                    }
                    UnionDecider::FormIdTargetType {
                        form_id_target_type,
                        map,
                        default_variant,
                    } => struct_out
                        .get(form_id_target_type)
                        .or_else(|| {
                            ctx.outer_struct
                                .as_ref()
                                .and_then(|o| o.get(form_id_target_type))
                        })
                        .and_then(|v| sibling_target_sig(v, ctx))
                        .and_then(|sig| map.get(&sig).copied())
                        .or(*default_variant),
                    _ => choose_union_variant(
                        ctx.form_version,
                        ctx.record_edid_char,
                        decider,
                        variants.len(),
                    ),
                };
                if let Some(idx) = chosen {
                    if let Some(variant) = variants.get(idx) {
                        let mut dummy = HashMap::new();
                        // Decode into a temporary map so we can insert_unique
                        // each key, avoiding silent clobbers when two union
                        // slots share the same variant name (e.g. MGEF's two
                        // `wbActorValue` fields both named "Actor Value").
                        let mut tmp = Map::new();
                        decode_member(ctx, variant, &mut tmp, &mut dummy, Some(&data[pos..]));
                        for (k, v) in tmp {
                            insert_unique(&mut struct_out, k, v);
                        }
                        // advance pos heuristically for known variants
                        pos = advance_union(ctx, variant, &data[pos..], pos);
                    }
                } else {
                    struct_out.insert(
                        name.clone(),
                        json!({"hex": hex::encode(&data[pos..]), "_raw": true}),
                    );
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
                        struct_out.get(path).and_then(|v| v.as_u64()).unwrap_or(0) as usize
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
                        struct_out.insert(name.clone(), Value::Array(items));
                    }
                }
            }
            MemberDef::Unknown { name, .. } => {
                if pos < data.len() {
                    insert_unique(
                        &mut struct_out,
                        name.clone(),
                        json!({"hex": hex::encode(&data[pos..]), "_raw": true}),
                    );
                }
                break;
            }
            _ => {}
        }
    }
    apply_post_decode_rules(PostDecodeTarget::Struct(&mut struct_out));
    if !struct_out.is_empty() {
        out.insert(struct_name.to_string(), Value::Object(struct_out));
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
            let mut tmp = Map::new();
            let consumed = decode_struct_fields(ctx, name, fields, data, &mut tmp);
            pos + consumed
        }
        _ => {
            let p = field_byte_size(ctx, variant).unwrap_or(0);
            pos + p.min(data.len())
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{FormIdRefResolver, FormIdStub, ResolveDepth};
    use crate::formid::FormId;
    use crate::schema::{IntegerWidth, Schema};

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
            scope_min_doc_index: None,
            scope_max_doc_index: None,
        }
    }

    fn empty_schema() -> Schema {
        serde_json::from_str(r#"{"records":{}}"#).unwrap()
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
            stop_before: vec![],
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
            stop_before: vec![],
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
    /// `tests/decode_records.rs` — the 4-byte path is intentionally covered by
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
        let mut out = Map::new();
        decode_struct_fields(&ctx, "Test", &fields, &data, &mut out);
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
        let mut out = Map::new();
        decode_struct_fields(&ctx, "Test", &fields, &data, &mut out);
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
                        }),
                        count: Some(ArrayCount::CountPath("Count".into())),
                        stop_before: Vec::new(),
                    },
                    sig_int_field("MPPK", "Tail", IntegerWidth::U16),
                ],
            }),
            count: None,
            stop_before: Vec::new(),
        };

        let subrecords = [
            subrecord("MPPC", 1u32.to_le_bytes().to_vec(), 0),
            subrecord("MPPI", 10u32.to_le_bytes().to_vec(), 1),
            subrecord("MPPK", 100u16.to_le_bytes().to_vec(), 2),
            subrecord("MPPC", 1u32.to_le_bytes().to_vec(), 3),
            subrecord("MPPI", 20u32.to_le_bytes().to_vec(), 4),
            subrecord("MPPK", 200u16.to_le_bytes().to_vec(), 5),
        ];
        let mut by_sig: HashMap<String, VecDeque<&OwnedSubrecord>> = HashMap::new();
        for sr in &subrecords {
            by_sig
                .entry(sr.signature.as_str().to_string())
                .or_default()
                .push_back(sr);
        }

        let mut out = Map::new();
        decode_member(&ctx, &morph_groups, &mut out, &mut by_sig, None);
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
                        stop_before: vec![],
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

        let mut out = Map::new();
        decode_struct_fields(&ctx, "Extra Data", &fields, &payload, &mut out);
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
        let mut out2 = Map::new();
        decode_struct_fields(&ctx_no_resolver, "Extra Data", &fields, &payload, &mut out2);
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
                stop_before: vec![],
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
                stop_before: vec![],
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
                stop_before: vec![],
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
        let mut out = Map::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
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
        let mut out = Map::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
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
        let mut out = Map::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
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
        let mut out = Map::new();
        decode_struct_fields(&ctx, "Effect Item Data", &efit_fields(), &data, &mut out);
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
        let mut by_sig: HashMap<String, VecDeque<&OwnedSubrecord>> = HashMap::new();
        for sr in &subrecords {
            by_sig
                .entry(sr.signature.as_str().to_string())
                .or_default()
                .push_back(sr);
        }

        let member = MemberDef::Vmad {
            sig: Some("VMAD".into()),
            name: "Virtual Machine Adapter".into(),
        };
        let mut out = Map::new();
        decode_member(&ctx, &member, &mut out, &mut by_sig, None);

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

    /// Decode an `lstring` member against a one-subrecord `by_sig` map.
    fn decode_lstring(ctx: &DecodeContext<'_>, sr: &OwnedSubrecord) -> Map<String, Value> {
        let member = MemberDef::LString {
            sig: Some("DESC".into()),
            name: "Description".into(),
            table: LStringTable::Dlstrings,
        };
        let mut by_sig: HashMap<String, VecDeque<&OwnedSubrecord>> = HashMap::new();
        by_sig
            .entry(sr.signature.as_str().to_string())
            .or_default()
            .push_back(sr);
        let mut out = Map::new();
        decode_member(ctx, &member, &mut out, &mut by_sig, None);
        out
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

    /// Fix E regression: a sig-bearing `Unused` member (Pascal
    /// `wbUnused(INDX, 0)` — an entire subrecord whose payload is
    /// intentionally ignored) must consume its subrecord from `by_sig` and
    /// emit nothing, instead of leaving it queued to show up as `_unmapped`.
    /// Before this fix `MemberDef::Unused`'s decode arm only ever looked at
    /// `payload` (the struct-payload-context byte-skip path) and never
    /// touched `by_sig` at all, so a schema entry giving it a `sig` would
    /// have been silently inert.
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
        let mut by_sig: HashMap<String, VecDeque<&OwnedSubrecord>> = HashMap::new();
        for sr in &subrecords {
            by_sig
                .entry(sr.signature.as_str().to_string())
                .or_default()
                .push_back(sr);
        }

        let mut out = Map::new();
        decode_member(&ctx, &member, &mut out, &mut by_sig, None);

        assert!(
            out.is_empty(),
            "sig-bearing Unused must emit no output key, got {out:?}"
        );
        assert!(
            by_sig.get("INDX").is_none_or(|q| q.is_empty()),
            "sig-bearing Unused must consume its subrecord from by_sig, \
             leaving nothing behind to show up as _unmapped"
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
        let mut out = Map::new();
        decode_struct_fields(&ctx, "Test", &fields, &data, &mut out);
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
