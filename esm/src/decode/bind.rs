//! Subrecord binding: which schema member each of a record's subrecords
//! belongs to.
//!
//! A record's subrecords are walked once, in file order, the way xEdit binds
//! them (`TwbMainRecord`, `TwbSubRecordStruct` and `TwbSubRecordArray` in
//! `wbImplementation.pas`):
//!
//! - A container (the record, or an `rstruct`) keeps a cursor over its member
//!   list. Each subrecord goes to the next member at or after the cursor that
//!   can handle its signature; members passed over are absent. An
//!   `unordered` container instead looks the member up by signature.
//! - An `rstruct` handles the signatures of its first member only, unless it
//!   is `unordered` or `any_member`, and it ends at the first subrecord none of
//!   its members handles, the first one its cursor can't place, or once a
//!   member listed in its `terminators` binds.
//! - An `rarray` takes elements while its element handles the next
//!   subrecord, so each element ends where its struct ends.
//! - A signature-less union takes the first variant that handles the
//!   subrecord (a field-value decider picks the variant instead, when it can).
//!
//! One deliberate difference: xEdit leaves every subrecord after an
//! out-of-order one unbound at the record level. Here an out-of-order
//! subrecord binds to its member and the cursor stays put.

use crate::reader::OwnedSubrecord;
use crate::schema::{LStringTable, MemberDef, RecordDef, UnionDecider};

use super::model_info::decode_model_info;
use super::node::{Fields, Node, insert_unique};
use super::scalars::{
    choose_union_variant, field_int_value, field_value_key, member_version_ok, scalar_bytes,
    scalar_float, scalar_formid, scalar_int, scalar_rgba, scalar_string, scalar_vec3,
};
use super::vmad::{
    decode_vmad_info, decode_vmad_pack, decode_vmad_perk, decode_vmad_qust, decode_vmad_scen,
    vmad_node,
};
use super::walk::{
    contains_field_value_union, counts_from_enclosing_scope, decode_array_payloads,
    decode_struct_fields, decode_union,
};
use super::{DecodeContext, lstring_table_to_kind};

/// A record's subrecords in file order, consumed front to back. Subrecords
/// no member takes are kept, in order, for `_unmapped`.
pub(super) struct Cursor<'a> {
    subs: &'a [OwnedSubrecord],
    pos: usize,
    unbound: Vec<&'a OwnedSubrecord>,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(subs: &'a [OwnedSubrecord]) -> Self {
        Cursor {
            subs,
            pos: 0,
            unbound: Vec::new(),
        }
    }

    fn peek(&self) -> Option<&'a OwnedSubrecord> {
        self.subs.get(self.pos)
    }

    fn peek_sig(&self) -> Option<&'a str> {
        self.peek().map(|sr| sr.signature.as_str())
    }

    fn take(&mut self) -> Option<&'a OwnedSubrecord> {
        let sr = self.subs.get(self.pos)?;
        self.pos += 1;
        Some(sr)
    }

    /// Pass over the current subrecord without binding it.
    fn skip(&mut self) {
        self.skip_one();
    }

    /// [`Self::skip`], reporting whether there was a subrecord to skip.
    pub(super) fn skip_one(&mut self) -> bool {
        match self.take() {
            Some(sr) => {
                self.unbound.push(sr);
                true
            }
            None => false,
        }
    }

    /// The subrecords no member took, in file order.
    pub(super) fn into_unbound(self) -> Vec<&'a OwnedSubrecord> {
        self.unbound
    }
}

/// Whether `member` can bind a subrecord with signature `sig` (xEdit's
/// `CanHandle`). Members outside the record's form-version range bind
/// nothing.
fn can_handle(ctx: &DecodeContext<'_>, member: &MemberDef, sig: &str) -> bool {
    if !member_version_ok(ctx.form_version, member) {
        return false;
    }
    match member {
        MemberDef::RStruct {
            members,
            unordered,
            any_member,
            ..
        } => {
            if *unordered || *any_member {
                members.iter().any(|m| can_handle(ctx, m, sig))
            } else {
                members.first().is_some_and(|m| can_handle(ctx, m, sig))
            }
        }
        MemberDef::RArray { element, .. } => can_handle(ctx, element, sig),
        MemberDef::Union {
            sig: None,
            variants,
            ..
        } => variants.iter().any(|v| can_handle(ctx, v, sig)),
        other => other.sig() == Some(sig),
    }
}

/// Bind a record's subrecords to `def`'s members, appending the decoded
/// fields to `out` in member order.
pub(super) fn bind_record(
    ctx: &DecodeContext<'_>,
    def: &RecordDef,
    cur: &mut Cursor<'_>,
    out: &mut Fields,
) {
    let members = &def.members;
    let mut fields = Fields::new();
    let mut origin: Vec<usize> = Vec::new();
    let mut bound = vec![false; members.len()];
    let mut def_pos = 0usize;
    while let Some(sig) = cur.peek_sig() {
        // The members at or after the cursor first, then (out of order, or
        // an unordered record) the rest.
        let start = if def.unordered { 0 } else { def_pos };
        let candidates = (start..members.len()).chain(0..start);
        let mut bound_here = false;
        for j in candidates {
            let member = &members[j];
            if !can_handle(ctx, member, sig) || (bound[j] && !continues_across_runs(member)) {
                continue;
            }
            let before = cur.pos;
            bind_member(ctx, member, cur, &mut fields);
            origin.resize(fields.len(), j);
            if cur.pos == before {
                continue;
            }
            bound[j] = true;
            if !def.unordered && j >= def_pos {
                def_pos = j + 1;
            }
            bound_here = true;
            break;
        }
        if !bound_here {
            cur.skip();
        }
    }
    add_raw_fallback_markers(members, &mut fields, &mut origin);
    out.extend(in_member_order(members, fields, &origin));
}

/// Whether a member bound once can bind again later in the record (a later,
/// non-contiguous run continues it; see [`bind_member`]).
fn continues_across_runs(member: &MemberDef) -> bool {
    matches!(
        member,
        MemberDef::RArray { .. } | MemberDef::Array { sig: Some(_), .. }
    )
}

/// Bind an `rstruct`'s members from the cursor into `out`. The struct ends
/// after binding a member whose signature is one of its `terminators`.
fn bind_struct(
    ctx: &DecodeContext<'_>,
    members: &[MemberDef],
    unordered: bool,
    terminators: &[String],
    cur: &mut Cursor<'_>,
    out: &mut Fields,
) {
    let mut origin: Vec<usize> = vec![usize::MAX; out.len()];
    let mut found = vec![false; members.len()];
    let mut def_pos = 0usize;
    while let Some(sig) = cur.peek_sig() {
        if def_pos >= members.len() || !members.iter().any(|m| can_handle(ctx, m, sig)) {
            break;
        }
        if unordered {
            def_pos = members
                .iter()
                .position(|m| can_handle(ctx, m, sig))
                .expect("some member handles sig");
        }
        let member = &members[def_pos];
        if !can_handle(ctx, member, sig) {
            def_pos += 1;
            continue;
        }
        if found[def_pos] {
            break;
        }
        let before = cur.pos;
        bind_member(ctx, member, cur, out);
        origin.resize(out.len(), def_pos);
        if cur.pos == before {
            if unordered {
                break;
            }
            def_pos += 1;
            continue;
        }
        found[def_pos] = true;
        if member
            .sig()
            .is_some_and(|sig| terminators.iter().any(|t| t == sig))
        {
            break;
        }
        def_pos = if unordered { 0 } else { def_pos + 1 };
    }
    add_raw_fallback_markers(members, out, &mut origin);
    let fields = std::mem::take(out);
    *out = in_member_order(members, fields, &origin);
}

/// A signature-less `raw_fallback` member marks schema the extractor could
/// not model; it binds no subrecord, so its marker is added to every
/// container it belongs to.
fn add_raw_fallback_markers(members: &[MemberDef], out: &mut Fields, origin: &mut Vec<usize>) {
    for (j, member) in members.iter().enumerate() {
        if let MemberDef::RawFallback {
            sig: None,
            name,
            reason,
        } = member
            && !out.contains_key(name)
        {
            out.insert(name.clone(), Node::raw_reason(None, reason));
            origin.push(j);
        }
    }
}

/// `fields` reordered by the member each entry came from (`origin`, parallel
/// to `fields`; entries the container had before binding carry
/// `usize::MAX`). A field under its member's own name sorts at the first
/// member of that name, so it keeps its place whichever of its repeated
/// definitions bound it; a second copy (`"<name> 2"`) keeps its own place.
fn in_member_order(members: &[MemberDef], fields: Fields, origin: &[usize]) -> Fields {
    let position = |j: usize, key: &str| match members.get(j) {
        Some(m) if m.name() == key => members
            .iter()
            .position(|other| other.name() == key)
            .unwrap_or(j),
        _ => j,
    };
    let mut entries: Vec<(usize, (String, Node))> = origin
        .iter()
        .zip(fields)
        .map(|(&j, (k, v))| (position(j, &k), (k, v)))
        .collect();
    entries.sort_by_key(|(j, _)| *j);
    entries.into_iter().map(|(_, kv)| kv).collect()
}

/// Bind one member starting at the cursor, whose current subrecord the
/// member can handle. When an earlier member of the same name (a repeated
/// definition, see the extractor's `_dedup_field_names`) already produced a
/// value, arrays continue it and anything else is added as `"<name> 2"`.
fn bind_member(
    ctx: &DecodeContext<'_>,
    member: &MemberDef,
    cur: &mut Cursor<'_>,
    out: &mut Fields,
) {
    if matches!(member, MemberDef::Union { sig: None, .. }) {
        return bind_one(ctx, member, cur, out);
    }
    let name = member.name();
    let Some(index) = out.get_index_of(name) else {
        return bind_one(ctx, member, cur, out);
    };
    let (key, prior) = out
        .shift_remove_index(index)
        .expect("index from get_index_of");
    bind_one(ctx, member, cur, out);
    match (prior, out.shift_remove(name)) {
        (Node::Array(mut items), Some(Node::Array(more))) => {
            items.extend(more);
            out.shift_insert(index, key, Node::Array(items));
        }
        (prior, new) => {
            out.shift_insert(index, key, prior);
            if let Some(new) = new {
                insert_unique(out, name.to_owned(), new);
            }
        }
    }
}

fn bind_one(ctx: &DecodeContext<'_>, member: &MemberDef, cur: &mut Cursor<'_>, out: &mut Fields) {
    match member {
        MemberDef::RStruct {
            name,
            members,
            unordered,
            terminators,
            ..
        } => {
            let mut group = Fields::new();
            bind_struct(ctx, members, *unordered, terminators, cur, &mut group);
            if !group.is_empty() {
                out.insert(name.clone(), Node::Struct(group));
            }
        }
        MemberDef::RArray { name, element, .. } => bind_rarray(ctx, name, element, cur, out),
        MemberDef::Union {
            sig: None,
            name,
            decider,
            variants,
        } => bind_runion(ctx, name, decider, variants, cur, out),
        MemberDef::Array {
            sig: Some(sig),
            name,
            element,
            count,
        } => {
            // A packed array subrecord; consecutive copies of it continue
            // the same array.
            let mut run = Vec::new();
            while cur.peek_sig() == Some(sig.as_str()) {
                run.extend(cur.take());
            }
            decode_array_payloads(ctx, name, element, count, &run, out);
        }
        _ => {
            if let Some(sr) = cur.take() {
                decode_subrecord(ctx, member, sr, out);
            }
        }
    }
}

/// Bind consecutive elements of an `rarray`. (A later, non-contiguous run
/// continues the same array; see [`bind_member`].)
fn bind_rarray(
    ctx: &DecodeContext<'_>,
    name: &str,
    element: &MemberDef,
    cur: &mut Cursor<'_>,
    out: &mut Fields,
) {
    let mut items = Vec::new();
    while let Some(sig) = cur.peek_sig() {
        if !can_handle(ctx, element, sig) {
            break;
        }
        let before = cur.pos;
        let mut item = Fields::new();
        bind_member(ctx, element, cur, &mut item);
        if cur.pos == before {
            break;
        }
        items.push(Node::Struct(item));
    }
    if !items.is_empty() {
        out.insert(name.to_owned(), Node::Array(items));
    }
}

/// Bind a signature-less union: a field-value decider picks the variant from
/// the fields bound so far; otherwise the first variant that handles the
/// current subrecord wins.
fn bind_runion(
    ctx: &DecodeContext<'_>,
    name: &str,
    decider: &UnionDecider,
    variants: &[MemberDef],
    cur: &mut Cursor<'_>,
    out: &mut Fields,
) {
    let Some(sig) = cur.peek_sig() else {
        return;
    };
    let decided = match decider {
        UnionDecider::FieldValue {
            field,
            map,
            default_variant,
            bits,
        } => {
            let lookup = |fields: &Fields| {
                let by_bits = field_int_value(fields, field, ctx).and_then(|v| {
                    bits.iter()
                        .find_map(|[mask, idx]| (v & mask != 0).then_some(*idx as usize))
                });
                by_bits.or_else(|| {
                    field_value_key(fields, field, ctx).and_then(|k| map.get(&k).copied())
                })
            };
            lookup(out)
                .or_else(|| ctx.outer_struct.as_ref().and_then(lookup))
                .or(*default_variant)
                .map(Some)
        }
        UnionDecider::BySignature { .. } => None,
        other => Some(choose_union_variant(
            ctx.form_version,
            ctx.record_edid_char,
            other,
            variants.len(),
        )),
    };
    let chosen = match decided {
        // The decided variant can't take this subrecord: the union declines
        // it (xEdit's union CanHandle only accepts the decided variant).
        Some(Some(i)) if !variants.get(i).is_some_and(|v| can_handle(ctx, v, sig)) => return,
        Some(idx) => idx,
        None => variants.iter().position(|v| can_handle(ctx, v, sig)),
    };
    let Some(variant) = chosen.and_then(|i| variants.get(i)) else {
        out.insert(
            name.to_owned(),
            Node::raw_reason(None, "union decider unresolved"),
        );
        return;
    };
    // Some variants are anonymous (Pascal `wbInteger('', ...)` reusing the
    // union's own name), so their value lands under the union's name.
    let mut tmp = Fields::new();
    bind_member(ctx, variant, cur, &mut tmp);
    for (k, v) in tmp {
        let key = if k.is_empty() { name.to_owned() } else { k };
        insert_unique(out, key, v);
    }
}

/// Decode `member` from the one subrecord bound to it.
fn decode_subrecord(
    ctx: &DecodeContext<'_>,
    member: &MemberDef,
    sr: &OwnedSubrecord,
    out: &mut Fields,
) {
    let data = sr.data.as_slice();
    match member {
        MemberDef::Struct { name, fields, .. } => {
            let child_ctx = fields
                .iter()
                .any(|f| contains_field_value_union(f) || counts_from_enclosing_scope(f))
                .then(|| ctx.with_outer_struct(out.clone()));
            decode_struct_fields(child_ctx.as_ref().unwrap_or(ctx), name, fields, data, out);
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
        MemberDef::String { name, sized, .. } => {
            out.insert(name.clone(), scalar_string(data, sized));
        }
        MemberDef::LString { name, table, .. } => {
            out.insert(name.clone(), lstring(ctx, table, sr));
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
        MemberDef::ByteRgba { name, .. } => {
            if let Some(v) = scalar_rgba(data) {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::Vec3 { name, .. } => {
            if let Some(v) = scalar_vec3(data) {
                out.insert(name.clone(), v);
            }
        }
        MemberDef::Empty { name, .. } => {
            out.insert(name.clone(), Node::Null);
        }
        MemberDef::Unused { .. } => {}
        MemberDef::Unknown { name, .. } => {
            out.insert(name.clone(), Node::raw(data));
        }
        MemberDef::RawFallback { name, reason, .. } => {
            out.insert(name.clone(), Node::raw_reason(Some(data), reason));
        }
        MemberDef::Vmad { name, .. } => {
            let decoded = match ctx.record_signature {
                Some("QUST") => decode_vmad_qust(ctx, data),
                Some("INFO") => decode_vmad_info(ctx, data),
                Some("PACK") => decode_vmad_pack(ctx, data),
                Some("PERK") => decode_vmad_perk(ctx, data),
                Some("SCEN") => decode_vmad_scen(ctx, data),
                // TERM wires wbVMADFragmentedPERK in xEdit's FO76 definitions
                // ("same fragments format as in PERK") — reuse that decoder so
                // the fragment tail's script-entry properties (e.g. a prize
                // terminal's `Form_*` item grants) are decoded and harvested
                // into the xref index instead of being silently dropped by the
                // generic `decode_vmad`, which stops after the base scripts.
                Some("TERM") => decode_vmad_perk(ctx, data),
                _ => vmad_node(ctx, data),
            };
            out.insert(name.clone(), decoded);
        }
        MemberDef::Ctda { name, .. } => {
            out.insert(name.clone(), crate::ctda::ctda_node(data, ctx));
        }
        MemberDef::ModelInfo { name, .. } => {
            out.insert(name.clone(), decode_model_info(data));
        }
        MemberDef::Union {
            name,
            decider,
            variants,
            ..
        } => decode_union(ctx, name, decider, variants, out, data),
        // Bound by `bind_member` (a signature-bearing array takes a run of
        // subrecords) or never bound (no signature of their own).
        MemberDef::Array { .. } | MemberDef::RStruct { .. } | MemberDef::RArray { .. } => {}
    }
}

/// An `lstring` subrecord's value.
///
/// "No string present" decodes to the same value in both modes (`Null`). The
/// two representations are not interchangeable on the wire — localized files
/// store a 4-byte ID, non-localized files store inline text — so a
/// mode-dependent encoding of "empty" would make every nameless record look
/// changed when a localized snapshot is diffed against a non-localized one.
fn lstring(ctx: &DecodeContext<'_>, table: &LStringTable, sr: &OwnedSubrecord) -> Node {
    if !ctx.is_localized {
        // Non-localized ESM: field is inline Windows-1252 text.
        return crate::reader::decode_inline_lstring(&sr.data).map_or(Node::Null, Node::Str);
    }
    // Localized ESM: field is a 4-byte ID into string tables.
    let Some(bytes) = sr.data.get(0..4) else {
        return Node::Null;
    };
    let id = u32::from_le_bytes(bytes.try_into().unwrap());
    if id == 0 {
        // 0 is the engine's "no string" sentinel, not a missing table
        // entry — mirrors render_formid's null-FormID special case.
        return Node::Null;
    }
    let kind = lstring_table_to_kind(table, ctx.record_signature, sr.signature.as_str());
    Node::LString { id, kind }
}
