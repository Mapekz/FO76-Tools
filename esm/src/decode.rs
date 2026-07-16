use crate::curves::Curve;
use crate::formid::{parse_formid, FormId};
use crate::reader::OwnedSubrecord;
use crate::schema::{
    ArrayCount, EnumFormat, FieldDef, IntegerWidth, LStringTable, MemberDef, Schema, UnionDecider,
    ValueFormat,
};
use crate::strings::{Localization, StringKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};

/// Single source of truth for the schema decode-coverage marker keys (see
/// "Decode output key conventions" in esm/CLAUDE.md). Exported to TypeScript
/// (`esm-viewer/src/shared/generated/markers.generated.ts`) so the renderer's
/// coverage-badge logic (`alignedTree.ts`'s `coverageBadges`) never hardcodes
/// these strings independently of the decoder that produces them.
pub mod markers {
    /// Emitted at the top level of a record with no schema mapping at all.
    pub const UNKNOWN_RECORD: &str = "_unknown_record";
    /// Emitted on a value that fell back to a raw hex dump (malformed/unmapped bytes).
    pub const RAW: &str = "_raw";
    /// Emitted alongside leftover subrecords the schema didn't consume.
    pub const UNMAPPED: &str = "_unmapped";
    /// Emitted on an LString field whose ID had no match in the loaded string tables.
    pub const UNRESOLVED: &str = "_unresolved";
}

/// Controls how deeply FormID references are followed during decode.
///
/// `ts_rs::TS` is derived only under `#[cfg(test)]` (`ts-rs` is a dev-dependency,
/// not a regular one — see `esm/CLAUDE.md` "N-API Binding and Electron App").
/// The export test itself lives behind `#[ts(export)]`, which `ts-rs` already
/// gates on `#[cfg(test)]` internally; the outer `cfg_attr` is what keeps the
/// `TS` impl (and the `ts_rs` extern crate reference) out of non-test builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, ts(export))]
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct FormIdStub {
    pub formid: String,
    pub editor_id: Option<String>,
    pub record_type: String,
}

#[derive(Clone)]
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
    /// Already-decoded fields of the enclosing struct, set when decoding array
    /// elements so that `FieldValue` deciders in element structs can reach parent
    /// fields (e.g. "Form Type" for OMOD property enum selection).
    pub outer_struct: Option<Map<String, Value>>,
    /// Signature of the record type currently being decoded (e.g. `"QUST"`, `"NPC_"`).
    /// Set at the top of `decode_record` so record-type-aware sub-decoders can
    /// branch on it (e.g. `decode_vmad_qust` vs `decode_vmad`).
    pub record_signature: Option<&'a str>,
    /// First character of the current record's EditorID subrecord.
    /// Pre-scanned in `decode_record` for use by `EdidPrefix` union deciders.
    pub record_edid_char: Option<char>,
    /// When set, `PresentSignature` union deciders only consider anchor subrecords
    /// at or after this document index (inclusive).
    pub scope_min_doc_index: Option<usize>,
    /// When set, `PresentSignature` union deciders only consider anchor subrecords
    /// strictly before this document index (typically the enclosing `ALED`).
    pub scope_max_doc_index: Option<usize>,
}

impl<'a> DecodeContext<'a> {
    /// Build a fresh top-level context for decoding a record: the five
    /// recursion-threading fields (`outer_struct`, `record_signature`,
    /// `record_edid_char`, `scope_min_doc_index`, `scope_max_doc_index`) start
    /// unset. `decode_record` populates `record_signature`/`record_edid_char`
    /// itself once it has scanned the record's subrecords.
    pub fn for_record(
        schema: &'a Schema,
        form_version: u16,
        is_localized: bool,
        localization: Option<&'a Localization>,
        curves: Option<&'a crate::curves::CurveIndex>,
        resolve_depth: ResolveDepth,
        resolver: Option<&'a dyn FormIdRefResolver>,
    ) -> DecodeContext<'a> {
        DecodeContext {
            schema,
            form_version,
            is_localized,
            localization,
            curves,
            resolve_depth,
            resolver,
            outer_struct: None,
            record_signature: None,
            record_edid_char: None,
            scope_min_doc_index: None,
            scope_max_doc_index: None,
        }
    }

    /// Return a new context identical to `self` but with `outer_struct` set.
    fn with_outer_struct(&self, outer: Map<String, Value>) -> DecodeContext<'a> {
        DecodeContext {
            outer_struct: Some(outer),
            ..self.clone()
        }
    }

    /// Narrow the current scope to `min`/`max`, intersecting with (rather
    /// than replacing) any scope already in effect. This matters because a
    /// scope set up by an enclosing `MemberDef::RArray` element (see its
    /// per-element anchor-bounded scope) must survive a nested rstruct's own
    /// scope computation — e.g. `rstruct_present_signature_scope`'s QUST
    /// alias ALED bounding — instead of being silently widened back to
    /// unbounded when that inner call has no opinion about one side of the
    /// range (`None`).
    fn with_scope(&self, min: Option<usize>, max: Option<usize>) -> DecodeContext<'a> {
        let scope_min_doc_index = match (self.scope_min_doc_index, min) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let scope_max_doc_index = match (self.scope_max_doc_index, max) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        DecodeContext {
            scope_min_doc_index,
            scope_max_doc_index,
            ..self.clone()
        }
    }
}

/// Render a curve's points as a JSON array of `{"x", "y"}` objects.
///
/// Shared by [`resolve_formid`]'s inline curve branch and the CURV-record's own
/// `"Curve"` field injection (`Database::record_at_meta_with_depth`) so both
/// render identically.
pub(crate) fn curve_points_value(curve: &Curve) -> Value {
    Value::Array(
        curve
            .points
            .iter()
            .map(|p| json!({"x": json_f32(p.x), "y": json_f32(p.y)}))
            .collect(),
    )
}

/// Resolve a FormID field to its JSON representation.
///
/// If the field's `valid_refs` includes `"CURV"` and a curve index is loaded,
/// the curve's EditorID, path, and point data are inlined into the output
/// object. When `ctx.resolve_depth` is `Stub` or `Full` and a resolver is
/// present, the referenced record is expanded inline. Otherwise, a bare hex
/// string is returned.
pub(crate) fn resolve_formid(ctx: &DecodeContext<'_>, valid_refs: &[String], id: FormId) -> Value {
    if valid_refs.iter().any(|r| r == "CURV") {
        if let Some(curves) = ctx.curves {
            if let Some(curve) = curves.get(id) {
                return json!({
                    "formid": id.display(),
                    "editor_id": curve.edid,
                    "curve_path": curve.path,
                    "curve": curve_points_value(curve)
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
    // Pre-scan the EDID subrecord for EdidPrefix union deciders (e.g. GMST value type).
    let edid_char = subrecords
        .iter()
        .find(|sr| sr.signature.as_str() == "EDID")
        .and_then(|sr| std::str::from_utf8(&sr.data).ok())
        .and_then(|s| s.trim_end_matches('\0').chars().next());

    // Shadow ctx with an updated context that carries the EDID first char.
    let ctx_with_meta;
    let ctx: &DecodeContext<'_> =
        if edid_char != ctx.record_edid_char || ctx.record_signature != Some(signature) {
            ctx_with_meta = DecodeContext {
                record_signature: Some(signature),
                record_edid_char: edid_char,
                schema: ctx.schema,
                form_version: ctx.form_version,
                is_localized: ctx.is_localized,
                localization: ctx.localization,
                curves: ctx.curves,
                resolve_depth: ctx.resolve_depth,
                resolver: ctx.resolver,
                outer_struct: None,
                scope_min_doc_index: ctx.scope_min_doc_index,
                scope_max_doc_index: ctx.scope_max_doc_index,
            };
            &ctx_with_meta
        } else {
            ctx
        };

    let mut out = Map::new();
    let record_def = ctx.schema.record(signature);

    let mut by_sig: HashMap<String, VecDeque<&OwnedSubrecord>> = HashMap::new();
    for sr in subrecords {
        by_sig
            .entry(sr.signature.as_str().to_string())
            .or_default()
            .push_back(sr);
    }

    if let Some(def) = record_def {
        out.insert("_record_type".into(), json!(def.name));
        for member in &def.members {
            decode_member(ctx, member, &mut out, &mut by_sig, None);
        }
    } else {
        out.insert("_record_type".into(), json!(signature));
        out.insert(markers::UNKNOWN_RECORD.into(), json!(true));
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
        out.insert(markers::UNMAPPED.into(), Value::Object(raw_remaining));
    }

    if signature == "WEAP" {
        apply_weapon_bash_curve(&mut out);
    }

    Value::Object(out)
}

fn decode_member(
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
        } => {
            if let Some(payload) = payload {
                decode_struct_fields(ctx, name, fields, payload, out);
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    let child_ctx = if fields.iter().any(contains_field_value_union) {
                        Some(ctx.with_outer_struct(out.clone()))
                    } else {
                        None
                    };
                    let decode_ctx = child_ctx.as_ref().unwrap_or(ctx);
                    decode_struct_fields(decode_ctx, name, fields, &sr.data, out);
                }
            }
        }
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
                if let Some(v) = read_int(data, *width, *signed) {
                    out.insert(name.clone(), format_int(v, format.as_ref()));
                }
            } else if let Some(sig) = sig {
                // If stop_before is set and a boundary sig precedes this
                // integer in document order, defer — leave the subrecord in
                // the pool for the correctly-positioned schema member.
                if !stop_before.is_empty() && stop_before_check(by_sig, sig, stop_before) {
                    // deferred
                } else if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
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
                    out.insert(name.clone(), json_f32(f));
                }
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
                    if sr.data.len() >= 4 {
                        let f = f32::from_le_bytes(sr.data[0..4].try_into().unwrap());
                        out.insert(name.clone(), json_f32(f));
                    }
                }
            }
        }
        MemberDef::String {
            sig, name, sized, ..
        } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
                    let s = match sized {
                        Some(n) if *n > 0 => {
                            String::from_utf8_lossy(&sr.data[..sr.data.len().min(*n as usize)])
                                .trim_end_matches('\0')
                                .to_string()
                        }
                        _ => read_zstring(&sr.data),
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
                                        (markers::UNRESOLVED): true
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
            ..
        } => {
            if let Some(data) = payload {
                if data.len() >= 4 {
                    let id = FormId::new(u32::from_le_bytes(data[0..4].try_into().unwrap()));
                    out.insert(name.clone(), resolve_formid(ctx, valid_refs, id));
                }
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
                    if sr.data.len() >= 4 {
                        let id = FormId::new(u32::from_le_bytes(sr.data[0..4].try_into().unwrap()));
                        out.insert(name.clone(), resolve_formid(ctx, valid_refs, id));
                    }
                }
            }
        }
        MemberDef::Bytes { sig, name, len, .. } => {
            if let Some(data) = payload {
                let n = len.unwrap_or(data.len());
                out.insert(
                    name.clone(),
                    json!({"hex": hex::encode(&data[..data.len().min(n)])}),
                );
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first_in_scope(by_sig, sig, ctx) {
                    let n = len.unwrap_or(sr.data.len());
                    out.insert(
                        name.clone(),
                        json!({"hex": hex::encode(&sr.data[..sr.data.len().min(n)])}),
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
            // Two calling shapes, mirroring the Struct arm above: (1) called with
            // an explicit `payload` slice already in hand — e.g. as a bare array
            // element via decode_field_value (NVNM's Vertices is the first real
            // exercise of this path); (2) called with no payload but its own
            // `sig`, so it must pull its own subrecord's bytes via `by_sig`.
            // Before this fix only (2) was handled, so a sig-less Vec3 array
            // element silently decoded to `{}` (nothing inserted into `out`).
            if let Some(data) = payload {
                if data.len() >= 12 {
                    out.insert(
                        name.clone(),
                        json!({
                            "x": json_f32(f32::from_le_bytes(data[0..4].try_into().unwrap())),
                            "y": json_f32(f32::from_le_bytes(data[4..8].try_into().unwrap())),
                            "z": json_f32(f32::from_le_bytes(data[8..12].try_into().unwrap())),
                        }),
                    );
                }
            } else if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    if sr.data.len() >= 12 {
                        out.insert(
                            name.clone(),
                            json!({
                                "x": json_f32(f32::from_le_bytes(sr.data[0..4].try_into().unwrap())),
                                "y": json_f32(f32::from_le_bytes(sr.data[4..8].try_into().unwrap())),
                                "z": json_f32(f32::from_le_bytes(sr.data[8..12].try_into().unwrap())),
                            }),
                        );
                    }
                }
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
        } => {
            let mut items = Vec::new();
            let target_count = rarray_count(count.as_ref(), out, ctx);
            let anchor = anchor_sig(element);
            while target_count.is_none_or(|n| items.len() < n) {
                // If stop_before is set, halt when a boundary sig precedes
                // the element's anchor in document order.
                if !stop_before.is_empty() {
                    if let Some(anchor) = anchor {
                        if stop_before_check(by_sig, anchor, stop_before) {
                            break;
                        }
                    }
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
                        iter.next().map(|first| {
                            (first.doc_index, iter.next().map(|second| second.doc_index))
                        })
                    })
                });
                let scoped_ctx;
                let element_ctx: &DecodeContext<'_> = match element_scope {
                    Some((current_idx, next_idx)) => {
                        scoped_ctx = ctx.with_scope(Some(current_idx), next_idx);
                        &scoped_ctx
                    }
                    None => ctx,
                };

                let mut item = Map::new();
                decode_member(element_ctx, element, &mut item, by_sig, None);
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
                                items.push(decode_field_value(
                                    ctx,
                                    element,
                                    &sr.data[pos..pos + sz],
                                ));
                                pos += sz;
                            }
                        }
                        None if matches!(element.as_ref(), MemberDef::Struct { .. }) => {
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
                            } = element.as_ref()
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
                    out.insert(name.clone(), Value::Array(items));
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
                        out.insert(name.clone(), Value::Array(items));
                    }
                }
            }
        }
        MemberDef::Union {
            sig,
            name,
            decider,
            variants,
        } => {
            // If the union has a sig, consume the subrecord and use its bytes as payload.
            let taken = sig.as_deref().and_then(|s| take_first(by_sig, s));
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
            if let Some(idx) = chosen {
                if let Some(variant) = variants.get(idx) {
                    // Decode into a temporary map first: some variants are
                    // anonymous (Pascal `wbInteger('', ...)` reusing the
                    // union's own name conceptually), so their decoded value
                    // would otherwise land under the empty-string key instead
                    // of the union's own (correctly-deduped) name.
                    let mut tmp = Map::new();
                    decode_member(ctx, variant, &mut tmp, by_sig, effective_payload);
                    for (k, v) in tmp {
                        let key = if k.is_empty() { name.clone() } else { k };
                        insert_unique(out, key, v);
                    }
                    return;
                }
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
                name.clone(),
                json!({
                    "_raw": true,
                    "reason": "union decider unresolved"
                }),
            );
        }
        MemberDef::Empty { sig, name, .. } => {
            if let Some(sig) = sig {
                // Only emit the marker when the empty subrecord is actually present.
                if take_first(by_sig, sig).is_some() {
                    out.insert(name.clone(), json!(null));
                }
            }
        }
        MemberDef::Unused { bytes, .. } => {
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
                    let decoded = match ctx.record_signature {
                        Some("QUST") => decode_vmad_qust(ctx, &sr.data),
                        Some("INFO") => decode_vmad_info(ctx, &sr.data),
                        Some("PACK") => decode_vmad_pack(ctx, &sr.data),
                        Some("PERK") => decode_vmad_perk(ctx, &sr.data),
                        Some("SCEN") => decode_vmad_scen(ctx, &sr.data),
                        _ => decode_vmad(ctx, &sr.data),
                    };
                    out.insert(name.clone(), decoded);
                }
            }
        }
        MemberDef::Ctda { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    out.insert(name.clone(), crate::ctda::decode_ctda(&sr.data, ctx));
                }
            } else if let Some(data) = payload {
                out.insert(name.clone(), crate::ctda::decode_ctda(data, ctx));
            }
        }
        MemberDef::ModelInfo { sig, name } => {
            if let Some(sig) = sig {
                if let Some(sr) = take_first(by_sig, sig) {
                    out.insert(name.clone(), decode_model_info(&sr.data));
                }
            } else if let Some(data) = payload {
                out.insert(name.clone(), decode_model_info(data));
            }
        }
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
fn decode_struct_fields(
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
                    if let Some(v) = read_int(&data[pos..], *width, *signed) {
                        struct_out.insert(name.clone(), format_int(v, format.as_ref()));
                    }
                    pos += size;
                }
            }
            MemberDef::Float { name, .. } => {
                if pos + 4 <= data.len() {
                    let f = f32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
                    struct_out.insert(name.clone(), json_f32(f));
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
                match sized {
                    Some(n) if *n > 0 => {
                        let end = (pos + *n as usize).min(data.len());
                        let s = String::from_utf8_lossy(&data[pos..end])
                            .trim_end_matches('\0')
                            .to_string();
                        struct_out.insert(name.clone(), json!(s));
                        pos = end;
                    }
                    _ => {
                        // None or sized=0 both mean null-terminated.
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
            }
            MemberDef::Bytes { name, len, .. } => {
                let n = len.unwrap_or(data.len().saturating_sub(pos));
                let end = (pos + n).min(data.len());
                struct_out.insert(name.clone(), json!({"hex": hex::encode(&data[pos..end])}));
                pos = end;
            }
            MemberDef::ByteRgba { name, .. } => {
                if pos + 4 <= data.len() {
                    struct_out.insert(
                        name.clone(),
                        json!({
                            "r": data[pos], "g": data[pos + 1], "b": data[pos + 2], "a": data[pos + 3]
                        }),
                    );
                    pos += 4;
                }
            }
            MemberDef::Vec3 { name, .. } => {
                if pos + 12 <= data.len() {
                    struct_out.insert(
                        name.clone(),
                        json!({
                            "x": json_f32(f32::from_le_bytes(data[pos..pos + 4].try_into().unwrap())),
                            "y": json_f32(f32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap())),
                            "z": json_f32(f32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap())),
                        }),
                    );
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
                if n > 0 {
                    if let Some(elem_size) = field_byte_size(ctx, element) {
                        let mut items = Vec::with_capacity(n.min(4096));
                        // Snapshot current fields so element structs can resolve
                        // FieldValue deciders that reference parent-scope fields
                        // (e.g. "Form Type" for OMOD property enum selection).
                        let child_ctx = ctx.with_outer_struct(struct_out.clone());
                        for _ in 0..n {
                            if pos + elem_size > data.len() {
                                break;
                            }
                            let v = decode_field_value(
                                &child_ctx,
                                element,
                                &data[pos..pos + elem_size],
                            );
                            items.push(v);
                            pos += elem_size;
                        }
                        if !items.is_empty() {
                            struct_out.insert(name.clone(), Value::Array(items));
                        }
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
    apply_crafting_quantity(&mut struct_out);
    if !struct_out.is_empty() {
        out.insert(struct_name.to_string(), Value::Object(struct_out));
    }
    pos
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
fn apply_weapon_bash_curve(out: &mut Map<String, Value>) {
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
        MemberDef::Unused {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        MemberDef::Empty {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        MemberDef::Bytes {
            from_version,
            below_version,
            ..
        } => (*from_version, *below_version),
        MemberDef::FormId {
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

fn choose_union_variant(
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

/// Resolve the target record signature for a decoded sibling FormID field.
fn sibling_target_sig(value: &Value, ctx: &DecodeContext<'_>) -> Option<String> {
    if let Value::Object(o) = value {
        if let Some(rt) = o.get("record_type").and_then(|v| v.as_str()) {
            return Some(rt.to_string());
        }
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

/// Read `width` bytes starting at `offset` in `data` as a little-endian unsigned integer.
/// Returns None if there isn't enough data.
fn read_le_uint(data: &[u8], offset: usize, width: usize) -> Option<u64> {
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
fn field_int_value(out: &Map<String, Value>, field: &str) -> Option<u64> {
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
fn parse_uint_str(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u64>().ok()
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
    json!(f
        .to_string()
        .parse::<f64>()
        .expect("f32::to_string output must parse as f64"))
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
            json!({"value": format!("0x{:X}", v as u64), "flags": set})
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
/// key whose integer is used as the map key. JSON `null` maps to the key `"null"`
/// (used by union deciders such as `wbNAVIParentDecider`).
fn field_value_key(out: &Map<String, Value>, field: &str) -> Option<String> {
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

fn take_first<'a>(
    by_sig: &mut HashMap<String, VecDeque<&'a OwnedSubrecord>>,
    sig: &str,
) -> Option<&'a OwnedSubrecord> {
    by_sig.get_mut(sig).and_then(|v| v.pop_front())
}

/// Like `take_first`, but only pops the front subrecord when its `doc_index`
/// falls inside `ctx`'s current scope.
///
/// `by_sig` is one global FIFO queue per signature across the *entire*
/// record, with no per-element partitioning. For a mandatory, always-present
/// member (e.g. an `Effect`'s `EFID`/`EFIT`) that FIFO order happens to align
/// correctly element-by-element. But an *optional* trailing sig-bearing
/// member (e.g. an ALCH/SPEL `Effect`'s `CVT0`/`MAGA`/`DURG`/`MAGG`/`CODV`)
/// is not guaranteed to be present on every element, so a bare `pop_front`
/// can steal a later element's subrecord when an earlier element lacks its
/// own. Scoping the pop to `[scope_min_doc_index, scope_max_doc_index)` (set
/// up per-element by `MemberDef::RArray`) keeps each element's optional
/// members bound to that element's own document span. Returns `None`
/// (without popping) when the front subrecord is out of scope, leaving it
/// queued for the correctly-scoped element to consume later.
fn take_first_in_scope<'a>(
    by_sig: &mut HashMap<String, VecDeque<&'a OwnedSubrecord>>,
    sig: &str,
    ctx: &DecodeContext<'_>,
) -> Option<&'a OwnedSubrecord> {
    let front_idx = by_sig
        .get(sig)
        .and_then(|v| v.front())
        .map(|sr| sr.doc_index)?;
    if doc_index_in_present_signature_scope(ctx, front_idx) {
        by_sig.get_mut(sig).and_then(|v| v.pop_front())
    } else {
        None
    }
}

fn doc_index_in_present_signature_scope(ctx: &DecodeContext<'_>, doc_index: usize) -> bool {
    if let Some(min) = ctx.scope_min_doc_index {
        if doc_index < min {
            return false;
        }
    }
    if let Some(max) = ctx.scope_max_doc_index {
        if doc_index >= max {
            return false;
        }
    }
    true
}

fn first_anchor_doc_index(
    by_sig: &HashMap<String, VecDeque<&OwnedSubrecord>>,
    member: &MemberDef,
) -> Option<usize> {
    match member {
        MemberDef::Struct { sig: Some(sig), .. }
        | MemberDef::Integer { sig: Some(sig), .. }
        | MemberDef::Float { sig: Some(sig), .. }
        | MemberDef::String { sig: Some(sig), .. }
        | MemberDef::LString { sig: Some(sig), .. }
        | MemberDef::FormId { sig: Some(sig), .. }
        | MemberDef::Bytes { sig: Some(sig), .. }
        | MemberDef::ByteRgba { sig: Some(sig), .. }
        | MemberDef::Vec3 { sig: Some(sig), .. }
        | MemberDef::Empty { sig: Some(sig), .. }
        | MemberDef::Unknown { sig: Some(sig), .. }
        | MemberDef::Union { sig: Some(sig), .. }
        | MemberDef::Vmad { sig: Some(sig), .. }
        | MemberDef::Ctda { sig: Some(sig), .. }
        | MemberDef::ModelInfo { sig: Some(sig), .. }
        | MemberDef::RawFallback { sig: Some(sig), .. } => by_sig
            .get(sig.as_str())
            .and_then(|v| v.front())
            .map(|sr| sr.doc_index),
        MemberDef::RStruct { members, .. } => members
            .iter()
            .find_map(|m| first_anchor_doc_index(by_sig, m)),
        _ => None,
    }
}

/// Bounds for `PresentSignature` inside repeated QUST alias bodies: from the
/// struct's opening anchor subrecord up to (but not including) the next `ALED`.
fn rstruct_present_signature_scope(
    by_sig: &HashMap<String, VecDeque<&OwnedSubrecord>>,
    members: &[MemberDef],
) -> (Option<usize>, Option<usize>) {
    let scope_min = members
        .iter()
        .find_map(|member| first_anchor_doc_index(by_sig, member));
    let scope_max = scope_min.and_then(|min| {
        by_sig.get("ALED").and_then(|subs| {
            subs.iter()
                .map(|sr| sr.doc_index)
                .filter(|&idx| idx > min)
                .min()
        })
    });
    (scope_min, scope_max)
}

/// Returns the first sig-bearing member's signature for `member`, if any.
/// Used by the `stop_before` RArray check to identify each element's anchor.
fn anchor_sig(member: &MemberDef) -> Option<&str> {
    match member {
        MemberDef::RStruct { members, .. } => members.iter().find_map(anchor_sig),
        MemberDef::Struct { sig, .. }
        | MemberDef::Integer { sig, .. }
        | MemberDef::Float { sig, .. }
        | MemberDef::String { sig, .. }
        | MemberDef::LString { sig, .. }
        | MemberDef::FormId { sig, .. }
        | MemberDef::Bytes { sig, .. }
        | MemberDef::ByteRgba { sig, .. }
        | MemberDef::Vec3 { sig, .. }
        | MemberDef::Empty { sig, .. }
        | MemberDef::Unknown { sig, .. }
        | MemberDef::Union { sig, .. }
        | MemberDef::Vmad { sig, .. }
        | MemberDef::Ctda { sig, .. }
        | MemberDef::ModelInfo { sig, .. }
        | MemberDef::RawFallback { sig, .. } => sig.as_deref(),
        _ => None,
    }
}

fn rarray_count(
    count: Option<&ArrayCount>,
    out: &Map<String, Value>,
    ctx: &DecodeContext<'_>,
) -> Option<usize> {
    match count {
        Some(ArrayCount::Fixed(n)) => Some(*n),
        Some(ArrayCount::CountPath(path)) => field_int_value(out, path)
            .or_else(|| {
                ctx.outer_struct
                    .as_ref()
                    .and_then(|o| field_int_value(o, path))
            })
            .map(|n| n as usize),
        // Prefix counts live inside a single payload-backed `Array`; repeated
        // subrecord arrays are bounded by sibling fields or document order.
        Some(ArrayCount::CountPrefix(_)) | Some(ArrayCount::FillToEnd) | None => None,
    }
}

/// Returns `true` when at least one `stop_before` sig has a lower `doc_index`
/// than the next occurrence of `anchor_sig` in `by_sig`. When this is true,
/// the calling RArray should halt iteration.
fn stop_before_check(
    by_sig: &HashMap<String, VecDeque<&OwnedSubrecord>>,
    anchor: &str,
    stop_before: &[String],
) -> bool {
    let anchor_idx = match by_sig.get(anchor).and_then(|v| v.front()) {
        Some(sr) => sr.doc_index,
        None => return true, // nothing left to consume
    };
    stop_before.iter().any(|sig| {
        by_sig
            .get(sig.as_str())
            .and_then(|v| v.front())
            .is_some_and(|sr| sr.doc_index < anchor_idx)
    })
}

fn take_all<'a>(
    by_sig: &mut HashMap<String, VecDeque<&'a OwnedSubrecord>>,
    sig: &str,
) -> Vec<&'a OwnedSubrecord> {
    by_sig
        .remove(sig)
        .map(|d| d.into_iter().collect())
        .unwrap_or_default()
}

/// Decode a VMAD (Papyrus scripts) subrecord into a structured JSON value.
///
/// VMAD stores Papyrus script attachments with properties in a compact binary format.
/// Never panics on truncated or malformed input — returns a raw hex fallback instead.
pub fn decode_vmad(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
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
            let value = decode_vmad_property(ctx, data, &mut pos, prop_type, obj_format);
            props.push(json!({"name": prop_name, "type": prop_type, "value": value}));
        }
        scripts.push(json!({"name": name, "status": status, "properties": props}));
    }

    json!({"version": version, "scripts": scripts})
}

/// Decode a `wbVMADFragmentedQUST` VMAD subrecord.
///
/// Extends the flat `decode_vmad` output with the `wbVMADFragmentedQUST`-specific
/// tail: a **Script Fragments** struct (extra bind data version, fragment count,
/// script name + optional script data, then N quest-stage fragments) followed by
/// an **Aliases** array (each alias carries a FormID/alias-ID, format version,
/// and its own script entries).
///
/// On any bounds-check failure the function returns the same `{"_raw": true,
/// "reason": "VMAD truncated", ...}` sentinel as `decode_vmad`, so callers can
/// treat both uniformly.
pub fn decode_vmad_qust(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
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
    macro_rules! read_u32 {
        () => {{
            need!(4);
            let v = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
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

    // ── Header + scripts (same layout as the flat decoder) ───────────────────
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
            let value = decode_vmad_property(ctx, data, &mut pos, prop_type, obj_format);
            props.push(json!({"name": prop_name, "type": prop_type, "value": value}));
        }
        scripts.push(json!({"name": name, "status": status, "properties": props}));
    }

    // ── Script Fragments (wbVMADFragmentedQUST tail) ─────────────────────────
    // Some QUST records carry only the plain VMAD header without a script-fragments tail.
    // Treat end-of-data here as a successful no-fragments result.
    if pos >= data.len() {
        return json!({"version": version, "scripts": scripts});
    }
    need!(1);
    let extra_bind_data_version = data[pos] as i8;
    pos += 1;
    let frag_count = read_u16!() as usize;
    let script_name = read_wstring!();
    // Script union: if script_name == "" then wbNull, else Script Data
    let script_data = if !script_name.is_empty() {
        need!(3); // flags u8 + prop_count u16
        let flags = data[pos];
        pos += 1;
        let pc = read_u16!() as usize;
        let mut props = Vec::new();
        for _ in 0..pc {
            let pn = read_wstring!();
            need!(2);
            let pt = data[pos];
            pos += 1;
            let _ps = data[pos];
            pos += 1;
            let val = decode_vmad_property(ctx, data, &mut pos, pt, obj_format);
            props.push(json!({"name": pn, "type": pt, "value": val}));
        }
        json!({"flags": flags, "properties": props})
    } else {
        json!(null)
    };
    let mut fragments = Vec::new();
    for _ in 0..frag_count {
        let quest_stage = read_u32!();
        let quest_stage_index = read_u32!();
        need!(1);
        pos += 1; // unknown byte
        let frag_script_name = read_wstring!();
        let fragment_name = read_wstring!();
        fragments.push(json!({
            "quest_stage": quest_stage,
            "quest_stage_index": quest_stage_index,
            "script_name": frag_script_name,
            "fragment_name": fragment_name,
        }));
    }
    let script_fragments = json!({
        "extra_bind_data_version": extra_bind_data_version,
        "script_name": script_name,
        "script_data": script_data,
        "fragments": fragments,
    });

    // ── Aliases (wbArrayS, u16-prefixed) ─────────────────────────────────────
    let alias_count = read_u16!() as usize;
    let mut aliases = Vec::new();
    for _ in 0..alias_count {
        // ScriptPropertyObject: obj_format 2 → u16 alias_id + u16 unused + u32 FormID
        //                       obj_format 1 → u32 FormID only
        let (alias_id, form_id) = if obj_format >= 2 {
            need!(8);
            let a = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            let _unused = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            let f = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            (a as u32, f)
        } else {
            let f = read_u32!();
            (0u32, f)
        };
        need!(4);
        let _version = i16::from_le_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        let alias_obj_format = u16::from_le_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        // Alias Scripts: u16-prefixed array of script entries
        let alias_script_count = read_u16!() as usize;
        let mut alias_scripts = Vec::new();
        for _ in 0..alias_script_count {
            let name = read_wstring!();
            need!(1);
            let status = data[pos];
            pos += 1;
            let pc = read_u16!() as usize;
            let mut props = Vec::new();
            for _ in 0..pc {
                let pn = read_wstring!();
                need!(2);
                let pt = data[pos];
                pos += 1;
                let _ps = data[pos];
                pos += 1;
                let val = decode_vmad_property(ctx, data, &mut pos, pt, alias_obj_format);
                props.push(json!({"name": pn, "type": pt, "value": val}));
            }
            alias_scripts.push(json!({"name": name, "status": status, "properties": props}));
        }
        aliases.push(json!({
            "alias_id": alias_id,
            "form_id": resolve_formid(ctx, &[], FormId::new(form_id)),
            "alias_scripts": alias_scripts,
        }));
    }

    json!({
        "version": version,
        "scripts": scripts,
        "script_fragments": script_fragments,
        "aliases": aliases,
    })
}

/// Parse the common VMAD header + scripts section, returning `(version, obj_format, scripts, pos)`
/// on success or a truncation `Value` on failure.
fn vmad_parse_header(
    ctx: &DecodeContext<'_>,
    data: &[u8],
) -> Result<(u16, u16, Vec<Value>, usize), Value> {
    let mut pos = 0usize;
    macro_rules! need {
        ($n:expr) => {
            if pos + $n > data.len() {
                return Err(json!({"_raw": true, "reason": "VMAD truncated", "hex": hex::encode(&data[pos..])}));
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
        let prop_count = read_u16!() as usize;
        let mut props = Vec::new();
        for _ in 0..prop_count {
            let pn = read_wstring!();
            need!(2);
            let pt = data[pos];
            pos += 1;
            let _ps = data[pos];
            pos += 1;
            let val = decode_vmad_property(ctx, data, &mut pos, pt, obj_format);
            props.push(json!({"name": pn, "type": pt, "value": val}));
        }
        scripts.push(json!({"name": name, "status": status, "properties": props}));
    }
    Ok((version, obj_format, scripts, pos))
}

/// Read a single script entry (name + status + props) from `data[*pos..]`.
/// Returns None on truncation; advances `*pos` on success.
fn vmad_read_script_entry(
    ctx: &DecodeContext<'_>,
    data: &[u8],
    pos: &mut usize,
    obj_format: u16,
) -> Option<Value> {
    fn read_wstr(data: &[u8], pos: &mut usize) -> Option<String> {
        if *pos + 2 > data.len() {
            return None;
        }
        let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
        *pos += 2;
        if *pos + len > data.len() {
            return None;
        }
        let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
        *pos += len;
        Some(s)
    }
    let name = read_wstr(data, pos)?;
    if *pos >= data.len() {
        return None;
    }
    let status = data[*pos];
    *pos += 1;
    if *pos + 2 > data.len() {
        return None;
    }
    let pc = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
    *pos += 2;
    let mut props = Vec::new();
    for _ in 0..pc {
        let pn = read_wstr(data, pos)?;
        if *pos + 2 > data.len() {
            return None;
        }
        let pt = data[*pos];
        *pos += 1;
        let _ps = data[*pos];
        *pos += 1;
        let val = decode_vmad_property(ctx, data, pos, pt, obj_format);
        props.push(json!({"name": pn, "type": pt, "value": val}));
    }
    Some(json!({"name": name, "status": status, "properties": props}))
}

/// Shared inner decoder for INFO/PACK/SCEN Script Fragments section.
/// `flag_mask` controls how many bits of the flags byte map to fragments:
/// 0x03 for INFO/SCEN (OnBegin|OnEnd), 0x07 for PACK (OnBegin|OnEnd|OnChange).
/// Returns (flags, script_entry_value, fragments_vec, pos) or a truncation error.
fn vmad_read_flags_fragments(
    ctx: &DecodeContext<'_>,
    data: &[u8],
    pos: &mut usize,
    obj_format: u16,
    flag_mask: u8,
) -> Result<(u8, Value, Vec<Value>), Value> {
    macro_rules! trunc {
        () => {
            return Err(json!({"_raw": true, "reason": "VMAD truncated", "hex": hex::encode(&data[*pos..])}))
        };
    }
    macro_rules! read_wstring {
        () => {{
            if *pos + 2 > data.len() {
                trunc!();
            }
            let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
            *pos += 2;
            if *pos + len > data.len() {
                trunc!();
            }
            let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
            *pos += len;
            s
        }};
    }
    if *pos >= data.len() {
        trunc!();
    }
    let extra_bind_data_version = data[*pos] as i8;
    *pos += 1;
    if *pos >= data.len() {
        trunc!();
    }
    let flags = data[*pos];
    *pos += 1;
    let frag_count = (flags & flag_mask).count_ones() as usize;
    let script_entry = vmad_read_script_entry(ctx, data, pos, obj_format).ok_or_else(
        || json!({"_raw": true, "reason": "VMAD truncated", "hex": hex::encode(&data[*pos..])}),
    )?;
    let mut fragments = Vec::new();
    for _ in 0..frag_count {
        if *pos >= data.len() {
            trunc!();
        }
        let _unknown = data[*pos];
        *pos += 1;
        let script_name = read_wstring!();
        let fragment_name = read_wstring!();
        fragments.push(json!({
            "extra_bind_data_version": extra_bind_data_version,
            "script_name": script_name,
            "fragment_name": fragment_name,
        }));
    }
    Ok((flags, script_entry, fragments))
}

/// Decode a fragmented VMAD for INFO records (wbVMADFragmentedINFO).
/// Script Fragments: extra_bind_data_version(s8) + flags(u8) + script_entry + fragments
/// Fragment count = popcount(flags & 0x03) — OnBegin (bit 0) and OnEnd (bit 1).
///
/// Some INFO records have scripts but no script-fragments tail (they use plain VMAD
/// layout even though INFO is classified as a fragmented type). When data ends right
/// after the header, return a successful result with no script_fragments key.
pub fn decode_vmad_info(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if pos >= data.len() {
        return json!({"version": version, "scripts": scripts});
    }
    match vmad_read_flags_fragments(ctx, data, &mut pos, obj_format, 0x03) {
        Err(e) => e,
        Ok((flags, script_entry, fragments)) => json!({
            "version": version,
            "scripts": scripts,
            "script_fragments": {
                "flags": flags,
                "script_entry": script_entry,
                "fragments": fragments,
            },
        }),
    }
}

/// Decode a fragmented VMAD for PACK records (wbVMADFragmentedPACK).
/// Script Fragments: extra_bind_data_version(s8) + flags(u8) + script_entry + fragments
/// Fragment count = popcount(flags & 0x07) — OnBegin, OnEnd, OnChange (bits 0-2).
///
/// Like INFO, some PACK records carry only the plain VMAD header without a
/// script-fragments tail. Return a no-fragments result when data ends after the header.
pub fn decode_vmad_pack(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if pos >= data.len() {
        return json!({"version": version, "scripts": scripts});
    }
    match vmad_read_flags_fragments(ctx, data, &mut pos, obj_format, 0x07) {
        Err(e) => e,
        Ok((flags, script_entry, fragments)) => json!({
            "version": version,
            "scripts": scripts,
            "script_fragments": {
                "flags": flags,
                "script_entry": script_entry,
                "fragments": fragments,
            },
        }),
    }
}

/// Decode a fragmented VMAD for PERK records (wbVMADFragmentedPERK).
/// Script Fragments: extra_bind_data_version(s8) + script_entry + u16-count fragments
/// Each fragment: fragment_index(u32) + unknown(1) + script_name(wstring) + fragment_name(wstring)
/// Followed by trailing unknown bytes (wbUnknown — consumed but not decoded).
///
/// Some PERK records carry only the plain VMAD header without a script-fragments tail.
/// Return a no-fragments result when data ends after the header.
pub fn decode_vmad_perk(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    macro_rules! trunc {
        () => {
            return json!({"_raw": true, "reason": "VMAD truncated", "hex": hex::encode(&data[pos..])})
        };
    }
    macro_rules! read_u16 {
        () => {{
            if pos + 2 > data.len() {
                trunc!();
            }
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }
    macro_rules! read_u32 {
        () => {{
            if pos + 4 > data.len() {
                trunc!();
            }
            let v = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        }};
    }
    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            if pos + len > data.len() {
                trunc!();
            }
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }
    if pos >= data.len() {
        return json!({"version": version, "scripts": scripts});
    }
    let extra_bind_data_version = data[pos] as i8;
    pos += 1;
    let script_entry = match vmad_read_script_entry(ctx, data, &mut pos, obj_format) {
        Some(v) => v,
        None => trunc!(),
    };
    let frag_count = read_u16!() as usize;
    let mut fragments = Vec::new();
    for _ in 0..frag_count {
        let fragment_index = read_u32!();
        if pos >= data.len() {
            trunc!();
        }
        let _unknown = data[pos];
        pos += 1;
        let script_name = read_wstring!();
        let fragment_name = read_wstring!();
        fragments.push(json!({
            "fragment_index": fragment_index,
            "script_name": script_name,
            "fragment_name": fragment_name,
        }));
    }
    json!({
        "version": version,
        "scripts": scripts,
        "script_fragments": {
            "extra_bind_data_version": extra_bind_data_version,
            "script_entry": script_entry,
            "fragments": fragments,
        },
    })
}

/// Decode a fragmented VMAD for SCEN records (wbVMADFragmentedSCEN).
/// Script Fragments: extra_bind_data_version(s8) + flags(u8) + script_entry + fragments + phase_fragments
/// Fragment count = popcount(flags & 0x03) — OnBegin (bit 1) and OnEnd (bit 2 in Pascal, but flags byte bits 0-1).
/// Phase fragments: u16-count-prefixed array; each = phase_flag(u8) + phase_index(u32) + unknown(1) + script_name + fragment_name.
pub fn decode_vmad_scen(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Some SCEN records carry only the plain VMAD header without a script-fragments tail.
    if pos >= data.len() {
        return json!({"version": version, "scripts": scripts});
    }
    let (flags, script_entry, fragments) =
        match vmad_read_flags_fragments(ctx, data, &mut pos, obj_format, 0x03) {
            Ok(v) => v,
            Err(e) => return e,
        };
    macro_rules! trunc {
        () => {
            return json!({"_raw": true, "reason": "VMAD truncated", "hex": hex::encode(&data[pos..])})
        };
    }
    macro_rules! read_u16 {
        () => {{
            if pos + 2 > data.len() {
                trunc!();
            }
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }
    macro_rules! read_u32 {
        () => {{
            if pos + 4 > data.len() {
                trunc!();
            }
            let v = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        }};
    }
    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            if pos + len > data.len() {
                trunc!();
            }
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }
    let phase_frag_count = read_u16!() as usize;
    let mut phase_fragments = Vec::new();
    for _ in 0..phase_frag_count {
        if pos >= data.len() {
            trunc!();
        }
        let phase_flag = data[pos];
        pos += 1;
        let phase_index = read_u32!();
        if pos >= data.len() {
            trunc!();
        }
        let _unknown = data[pos];
        pos += 1;
        let script_name = read_wstring!();
        let fragment_name = read_wstring!();
        phase_fragments.push(json!({
            "phase_flag": phase_flag,
            "phase_index": phase_index,
            "script_name": script_name,
            "fragment_name": fragment_name,
        }));
    }
    json!({
        "version": version,
        "scripts": scripts,
        "script_fragments": {
            "flags": flags,
            "script_entry": script_entry,
            "fragments": fragments,
            "phase_fragments": phase_fragments,
        },
    })
}

fn decode_vmad_property(
    ctx: &DecodeContext<'_>,
    data: &[u8],
    pos: &mut usize,
    prop_type: u8,
    obj_format: u16,
) -> Value {
    fn read_vmad_wstring(data: &[u8], pos: &mut usize) -> Option<String> {
        if *pos + 2 > data.len() {
            return None;
        }
        let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
        *pos += 2;
        if *pos + len > data.len() {
            return None;
        }
        let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
        *pos += len;
        Some(s)
    }

    // Nested `fn` items don't capture the enclosing function's variables, so
    // `ctx` must be threaded through explicitly rather than closed over.
    fn decode_vmad_struct(
        ctx: &DecodeContext<'_>,
        data: &[u8],
        pos: &mut usize,
        obj_format: u16,
    ) -> Value {
        if *pos + 4 > data.len() {
            return json!(null);
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut members = Vec::with_capacity(count.min(256));
        for _ in 0..count {
            let Some(name) = read_vmad_wstring(data, pos) else {
                break;
            };
            if *pos >= data.len() {
                break;
            }
            let member_type = data[*pos];
            *pos += 1;
            if *pos >= data.len() {
                break;
            }
            let _member_status = data[*pos];
            *pos += 1;
            let before = *pos;
            let value = decode_vmad_property(ctx, data, pos, member_type, obj_format);
            // Type 0 (None) is zero-width — pos not advancing is correct, not a stall.
            if member_type != 0 && *pos == before {
                break;
            }
            members.push(json!({"name": name, "type": member_type, "value": value}));
        }
        json!(members)
    }

    fn read_scalar(
        ctx: &DecodeContext<'_>,
        data: &[u8],
        pos: &mut usize,
        base_type: u8,
        obj_format: u16,
    ) -> Value {
        match base_type {
            1 => {
                // Scripted object: always 8 bytes (FormID + Alias + Unused).
                if *pos + 8 > data.len() {
                    return json!(null);
                }
                // xEdit ground truth (wbDefinitionsFO76.pas `wbScriptPropertyObject`
                // + `wbGetScriptObjFormat`): objFormat == 1 selects "Object v1"
                // (FormID, Alias, Unused — FormID first); anything else (incl. the
                // common objFormat == 2) selects "Object v2" (Unused, Alias, FormID
                // — FormID last).
                let form_off = if obj_format == 1 { 0 } else { 4 };
                let form_id = u32::from_le_bytes([
                    data[*pos + form_off],
                    data[*pos + form_off + 1],
                    data[*pos + form_off + 2],
                    data[*pos + form_off + 3],
                ]);
                *pos += 8;
                resolve_formid(ctx, &[], FormId::new(form_id))
            }
            2 => {
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
                if *pos + 4 > data.len() {
                    return json!(null);
                }
                let v = i32::from_le_bytes([
                    data[*pos],
                    data[*pos + 1],
                    data[*pos + 2],
                    data[*pos + 3],
                ]);
                *pos += 4;
                json!(v)
            }
            4 => {
                if *pos + 4 > data.len() {
                    return json!(null);
                }
                let v = f32::from_le_bytes([
                    data[*pos],
                    data[*pos + 1],
                    data[*pos + 2],
                    data[*pos + 3],
                ]);
                *pos += 4;
                json_f32(v)
            }
            5 => {
                if *pos >= data.len() {
                    return json!(null);
                }
                let v = data[*pos] != 0;
                *pos += 1;
                json!(v)
            }
            // Type 6 = Variable: 1-byte type discriminator, then value of that type.
            6 => {
                if *pos >= data.len() {
                    return json!(null);
                }
                let sub_type = data[*pos];
                *pos += 1;
                read_scalar(ctx, data, pos, sub_type, obj_format)
            }
            // Type 7 = Struct: u32 member count, then N × (name + type + status + value).
            7 => decode_vmad_struct(ctx, data, pos, obj_format),
            _ => json!({"_raw": true, "type": base_type}),
        }
    }

    // Type 0 = None: zero bytes, null value.
    if prop_type == 0 {
        return json!(null);
    }

    if prop_type == 7 {
        return decode_vmad_struct(ctx, data, pos, obj_format);
    }

    if (11..=15).contains(&prop_type) {
        let base_type = prop_type - 10;
        if *pos + 4 > data.len() {
            return json!(null);
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let before = *pos;
            items.push(read_scalar(ctx, data, pos, base_type, obj_format));
            if *pos == before {
                break;
            }
        }
        return json!(items);
    }

    if prop_type == 17 {
        if *pos + 4 > data.len() {
            return json!(null);
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let before = *pos;
            items.push(decode_vmad_struct(ctx, data, pos, obj_format));
            if *pos == before {
                break;
            }
        }
        return json!(items);
    }

    if prop_type == 16 {
        if *pos + 4 > data.len() {
            return json!(null);
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            if *pos >= data.len() {
                break;
            }
            let elem_type = data[*pos];
            *pos += 1;
            let before = *pos;
            items.push(decode_vmad_property(ctx, data, pos, elem_type, obj_format));
            if *pos == before {
                break;
            }
        }
        return json!({"_variable_array": true, "items": items});
    }

    read_scalar(ctx, data, pos, prop_type, obj_format)
}

/// Map a schema [`LStringTable`] selector to the runtime [`StringKind`].
fn lstring_table_to_kind(table: &LStringTable) -> StringKind {
    match table {
        LStringTable::Strings => StringKind::Strings,
        LStringTable::Dlstrings => StringKind::DlStrings,
        LStringTable::Ilstrings => StringKind::IlStrings,
    }
}

/// Decodes the structured `wbModelInfo` (FO4/FO76 non-TES5) layout shared by every "Model
/// Information" subrecord (`MODT`, `DMDT`, `MO2T`..`MO5T`, `NAM2`, `NAM5`):
///
/// ```text
/// wbStruct('', [
///   wbArray('Counters', wbInteger(itU32), -1, ['Textures','Addon Nodes','SRGB','Materials']),
///   wbArray('Textures', wbStruct[File Hash: u32, Extension: char[4], Folder Hash: u32])
///     .SetCountPath('Counters\[0]'),
///   wbArray('Addon Nodes', wbInteger(itU32)).SetCountPath('Counters\[1]'),
///   wbArray('Materials', wbStruct[File Hash: u32, Extension: char[4], Folder Hash: u32])
///     .SetCountPath('Counters\[3]'),
/// ])
/// ```
/// (`wbDefinitionsCommon.pas` `wbModelInfo`, non-TES5 branch — `wbArray('Counters', ...)`
/// is itself count-prefixed, so the leading u32 is *how many* counters follow, not a counter
/// itself.) `Counters[2]` ("SRGB") gates no array of its own — it's a plain count value with
/// no associated bytes.
///
/// Because the layout isn't expressible in the static schema model (it's a self-describing
/// blob, not a signature-keyed struct), the extractor stubs every "Model Information" member
/// as a byte array, and this function is dispatched from the `MemberDef::Bytes` arm by field
/// name. Falls back to `{"hex": ..., "_raw": true}` whenever the declared counters don't
/// exactly account for the subrecord's length — covering the TES5-style 2-counter layout,
/// corrupt data, and any record this heuristic doesn't actually fit — rather than panicking,
/// consistent with the decoder-must-never-panic invariant.
pub(crate) fn decode_model_info(data: &[u8]) -> Value {
    fn raw(data: &[u8]) -> Value {
        json!({ "hex": hex::encode(data), "_raw": true })
    }
    fn read_u32(data: &[u8], off: usize) -> Option<u32> {
        data.get(off..off + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    // A file entry is { File Hash: u32, Extension: char[4], Folder Hash: u32 } — 12 bytes.
    fn read_file_entry(data: &[u8], off: usize) -> Value {
        let file_hash = read_u32(data, off).unwrap_or(0);
        let ext = data.get(off + 4..off + 8).unwrap_or(&[]);
        let folder_hash = read_u32(data, off + 8).unwrap_or(0);
        json!({
            "File Hash": format!("0x{:08X}", file_hash),
            "Extension": String::from_utf8_lossy(ext).trim_end_matches('\0').to_string(),
            "Folder Hash": format!("0x{:08X}", folder_hash),
        })
    }

    let Some(num_counters) = read_u32(data, 0) else {
        return raw(data);
    };
    let num_counters = num_counters as usize;
    // Real layouts use 2 (TES5) or 4 (FO4/FO76) counters — bound generously but reject
    // anything that can't plausibly be this format.
    if !(1..=8).contains(&num_counters) {
        return raw(data);
    }

    let mut counters = Vec::with_capacity(num_counters);
    for i in 0..num_counters {
        match read_u32(data, 4 + i * 4) {
            Some(c) => counters.push(c as usize),
            None => return raw(data),
        }
    }

    let num_textures = counters.first().copied().unwrap_or(0);
    let num_addon_nodes = counters.get(1).copied().unwrap_or(0);
    let num_materials = counters.get(3).copied().unwrap_or(0);

    // Validate the total byte length up front, via checked arithmetic, before doing any
    // per-entry allocation — so a corrupt/huge count can't trigger an OOM abort, and once
    // this passes every read below is guaranteed in-bounds.
    let header_len = 4 + num_counters * 4;
    let total_len = (|| {
        header_len
            .checked_add(num_textures.checked_mul(12)?)?
            .checked_add(num_addon_nodes.checked_mul(4)?)?
            .checked_add(num_materials.checked_mul(12)?)
    })();
    if total_len != Some(data.len()) {
        return raw(data);
    }

    let counter_names = ["Textures", "Addon Nodes", "SRGB", "Materials"];
    let mut counters_obj = Map::new();
    for (i, &c) in counters.iter().enumerate() {
        let name = counter_names.get(i).copied().unwrap_or("Unknown");
        counters_obj.insert(name.to_string(), json!(c));
    }

    let mut off = header_len;
    let textures: Vec<Value> = (0..num_textures)
        .map(|_| {
            let entry = read_file_entry(data, off);
            off += 12;
            entry
        })
        .collect();
    let addon_nodes: Vec<Value> = (0..num_addon_nodes)
        .map(|_| {
            let v = json!(read_u32(data, off).unwrap_or(0));
            off += 4;
            v
        })
        .collect();
    let materials: Vec<Value> = (0..num_materials)
        .map(|_| {
            let entry = read_file_entry(data, off);
            off += 12;
            entry
        })
        .collect();

    json!({
        "Counters": Value::Object(counters_obj),
        "Textures": textures,
        "Addon Nodes": addon_nodes,
        "Materials": materials,
    })
}

// Minimal hex encoding without extra dependency
pub(crate) mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ArrayCount, IntegerWidth, MemberDef, Schema};
    use serde_json::Map;
    use std::collections::HashMap;

    /// Build a minimal `DecodeContext` around a borrowed `Schema`.
    ///
    /// Private-side twin of `tests/common::bare_ctx` — if `DecodeContext` gains
    /// or loses a field, update both copies.
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

    /// Regression test: `resolve_formid`'s CURV branch inlines `formid`,
    /// `curve_path`, and `curve`, but was missing `editor_id` even though
    /// `Curve` already carries the EditorID parsed off the CURV record at
    /// index-build time — every FormID field referencing a curve table (e.g.
    /// ALCH `Health`, ENCH `Curve Table`) silently dropped the curve's own
    /// EditorID. Pin that it's now surfaced, and that a curve with no EDID
    /// subrecord serializes as `null` rather than an empty string.
    #[test]
    fn resolve_formid_curv_branch_includes_editor_id() {
        let curve = crate::curves::Curve {
            edid: Some("CT_Legendary_Weapon_Adrenal".to_string()),
            path: r"LegendaryMods\Weapon_DamagePerKill.json".to_string(),
            points: vec![crate::curves::CurvePoint { x: 0.0, y: 0.0 }],
        };
        let curves = crate::curves::CurveIndex::from_entries(vec![(0x1, curve)]);
        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.curves = Some(&curves);

        let result = resolve_formid(&ctx, &["CURV".to_string()], FormId::new(0x1));
        assert_eq!(result["editor_id"], json!("CT_Legendary_Weapon_Adrenal"));
        assert_eq!(result["formid"], json!(FormId::new(0x1).display()));

        let curve_no_edid = crate::curves::Curve {
            edid: None,
            path: "Foo.json".to_string(),
            points: vec![],
        };
        let curves_no_edid = crate::curves::CurveIndex::from_entries(vec![(0x2, curve_no_edid)]);
        let mut ctx2 = bare_ctx(&schema);
        ctx2.curves = Some(&curves_no_edid);
        let result2 = resolve_formid(&ctx2, &["CURV".to_string()], FormId::new(0x2));
        assert_eq!(result2["editor_id"], Value::Null);
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

    /// Object format 2 (the common case): xEdit's "Object v2" layout is
    /// Unused(u16) + Alias(s16) + FormID(u32) — FormID at offset 4 within the
    /// 8-byte union. See `wbScriptPropertyObject` / `wbGetScriptObjFormat` in
    /// TES5Edit's `wbDefinitionsFO76.pas` / `wbDefinitionsCommon.pas`.
    #[test]
    fn vmad_object_format2_reads_eight_bytes() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let mut data = vmad_header(2, 1);
        data.extend(vmad_wstring("TestScript"));
        data.push(0); // status
        data.extend_from_slice(&2u16.to_le_bytes()); // prop_count
        data.extend(vmad_wstring("MyRef"));
        data.push(1); // type = object
        data.push(0); // status

        // Unused u16, Alias i16, FormID @4
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&3i16.to_le_bytes());
        data.extend_from_slice(&0x00000042u32.to_le_bytes());
        // Second property: int32 — must not be misaligned
        data.extend(vmad_wstring("Count"));
        data.push(3); // type = int
        data.push(0); // status
        data.extend_from_slice(&7i32.to_le_bytes());

        let decoded = decode_vmad(&ctx, &data);
        assert!(
            decoded.get("_raw").is_none(),
            "must not truncate: {decoded}"
        );
        let props = decoded
            .pointer("/scripts/0/properties")
            .and_then(|v| v.as_array())
            .expect("properties");
        assert_eq!(
            props[0].pointer("/value").and_then(|v| v.as_str()),
            Some("0x00000042")
        );
        assert_eq!(props[1].pointer("/value").and_then(|v| v.as_i64()), Some(7));
    }

    /// Object format 1: xEdit's "Object v1" layout is FormID(u32) + Alias(s16)
    /// + Unused(u16) — FormID at offset 0 within the 8-byte union.
    #[test]
    fn vmad_object_format1_reads_eight_bytes() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let mut data = vmad_header(1, 1);
        data.extend(vmad_wstring("TestScript"));
        data.push(0);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend(vmad_wstring("MyRef"));
        data.push(1);
        data.push(0);
        // FormID @0, Alias i16, Unused u16
        data.extend_from_slice(&0x00000099u32.to_le_bytes());
        data.extend_from_slice(&1i16.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());

        let decoded = decode_vmad(&ctx, &data);
        assert!(
            decoded.get("_raw").is_none(),
            "must not truncate: {decoded}"
        );
        let value = decoded
            .pointer("/scripts/0/properties/0/value")
            .and_then(|v| v.as_str());
        assert_eq!(value, Some("0x00000099"));
    }

    /// Array property type 11 = count + N objects (object format 2: FormID last).
    #[test]
    fn vmad_object_array_decodes_without_truncation() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let mut data = vmad_header(2, 1);
        data.extend(vmad_wstring("TestScript"));
        data.push(0);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend(vmad_wstring("Refs"));
        data.push(11); // type = object array
        data.push(0);
        data.extend_from_slice(&2u32.to_le_bytes()); // count
        for fid in [0x11u32, 0x22u32] {
            // Unused u16, Alias i16, FormID @4 (object format 2)
            data.extend_from_slice(&0u16.to_le_bytes());
            data.extend_from_slice(&0i16.to_le_bytes());
            data.extend_from_slice(&fid.to_le_bytes());
        }

        let decoded = decode_vmad(&ctx, &data);
        assert!(
            decoded.get("_raw").is_none(),
            "must not truncate: {decoded}"
        );
        let arr = decoded
            .pointer("/scripts/0/properties/0/value")
            .and_then(|v| v.as_array())
            .expect("object array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("0x00000011"));
        assert_eq!(arr[1].as_str(), Some("0x00000022"));
    }

    /// Regression test for the real `Serum_AdrenalReactionApplier` MGEF
    /// (`0x0050A5D8`) bug: its VMAD script property `MutationSpell` (object
    /// format 2) carries the verbatim union bytes
    /// `00 00 ff ff 14 1f 4e 00` — Unused(u16)=0, Alias(s16)=-1,
    /// FormID(u32)=0x004E1F14 (the SPEL `Mutation_AdrenalReaction`).
    ///
    /// The inverted offset (`if obj_format == 2 { 0 } else { 4 }`) used to read
    /// the *first* 4 bytes instead — `00 00 ff ff` — producing the garbage
    /// FormID `0xFFFF0000`, which doesn't exist in any ESM, so the real
    /// mutation-SPEL reference silently vanished from both the decoded record
    /// and the xref index.
    #[test]
    fn vmad_object_property_decodes_real_serum_adrenal_reaction_bug_bytes() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let mut data = vmad_header(2, 1);
        data.extend(vmad_wstring("AddMutationOnEffectScript"));
        data.push(0); // status
        data.extend_from_slice(&1u16.to_le_bytes()); // prop_count
        data.extend(vmad_wstring("MutationSpell"));
        data.push(1); // type = object
        data.push(1); // status
        data.extend_from_slice(&[0x00, 0x00, 0xff, 0xff, 0x14, 0x1f, 0x4e, 0x00]);

        let decoded = decode_vmad(&ctx, &data);
        assert!(
            decoded.get("_raw").is_none(),
            "must not truncate: {decoded}"
        );
        let value = decoded
            .pointer("/scripts/0/properties/0/value")
            .and_then(|v| v.as_str());
        assert_eq!(
            value,
            Some("0x004E1F14"),
            "must read the FormID from the last 4 bytes of the union (object \
             format 2), not the Unused+Alias bytes (which decode as the \
             nonexistent 0xFFFF0000)"
        );
    }

    /// Same bug-reproducing bytes as above, but with a resolving ctx: the
    /// object-property FormID must come out as a `{formid, editor_id,
    /// record_type}` stub (matching a normal `MemberDef::FormId` field), not a
    /// bare hex string — that's what makes it a clickable, named reference in
    /// the ESM Viewer.
    #[test]
    fn vmad_object_property_resolves_to_stub_with_resolver() {
        let schema = empty_schema();
        let target_id = FormId::new(0x004E_1F14);
        let resolver = StubResolver {
            stubs: std::collections::HashMap::from([(
                target_id,
                FormIdStub {
                    formid: target_id.display(),
                    editor_id: Some("Mutation_AdrenalReaction".into()),
                    record_type: "SPEL".into(),
                },
            )]),
        };
        let mut ctx = bare_ctx(&schema);
        ctx.resolve_depth = ResolveDepth::Stub;
        ctx.resolver = Some(&resolver);

        let mut data = vmad_header(2, 1);
        data.extend(vmad_wstring("AddMutationOnEffectScript"));
        data.push(0);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend(vmad_wstring("MutationSpell"));
        data.push(1);
        data.push(1);
        data.extend_from_slice(&[0x00, 0x00, 0xff, 0xff, 0x14, 0x1f, 0x4e, 0x00]);

        let decoded = decode_vmad(&ctx, &data);
        let value = decoded
            .pointer("/scripts/0/properties/0/value")
            .expect("value present");
        assert_eq!(
            value.get("editor_id").and_then(|v| v.as_str()),
            Some("Mutation_AdrenalReaction")
        );
        assert_eq!(
            value.get("record_type").and_then(|v| v.as_str()),
            Some("SPEL")
        );
        assert_eq!(
            value.get("formid").and_then(|v| v.as_str()),
            Some("0x004E1F14")
        );
    }

    /// Struct property type 6 = member-count + (wstring name + u8 type + value)*.
    #[test]
    fn vmad_struct_property_decodes_without_truncation() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let mut data = vmad_header(2, 1);
        data.extend(vmad_wstring("TestScript"));
        data.push(0);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend(vmad_wstring("Config"));
        data.push(7); // type = struct
        data.push(0);
        data.extend_from_slice(&2u32.to_le_bytes()); // member count
        data.extend(vmad_wstring("Count"));
        data.push(3); // type = int
        data.push(0); // status
        data.extend_from_slice(&42i32.to_le_bytes());
        data.extend(vmad_wstring("Label"));
        data.push(2); // string
        data.push(0); // status
        data.extend(vmad_wstring("hello"));

        let decoded = decode_vmad(&ctx, &data);
        assert!(
            decoded.get("_raw").is_none(),
            "must not truncate: {decoded}"
        );
        let members = decoded
            .pointer("/scripts/0/properties/0/value")
            .and_then(|v| v.as_array())
            .expect("struct members");
        assert_eq!(members.len(), 2);
        assert_eq!(
            members[0].pointer("/value").and_then(|v| v.as_i64()),
            Some(42)
        );
        assert_eq!(
            members[1].pointer("/value").and_then(|v| v.as_str()),
            Some("hello")
        );
    }

    /// Array-of-struct property type 17 = count + N struct payloads.
    #[test]
    fn vmad_struct_array_decodes_without_truncation() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let mut data = vmad_header(2, 1);
        data.extend(vmad_wstring("TestScript"));
        data.push(0);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend(vmad_wstring("Rows"));
        data.push(17); // type = array of struct
        data.push(0);
        data.extend_from_slice(&2u32.to_le_bytes()); // count
        for (name, val) in [("A", 1i32), ("B", 2i32)] {
            let _ = name;
            data.extend_from_slice(&1u32.to_le_bytes()); // one member per struct
            data.extend(vmad_wstring("X"));
            data.push(3);
            data.push(0);
            data.extend_from_slice(&val.to_le_bytes());
        }

        let decoded = decode_vmad(&ctx, &data);
        assert!(
            decoded.get("_raw").is_none(),
            "must not truncate: {decoded}"
        );
        let arr = decoded
            .pointer("/scripts/0/properties/0/value")
            .and_then(|v| v.as_array())
            .expect("struct array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].pointer("/0/value").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(arr[1].pointer("/0/value").and_then(|v| v.as_i64()), Some(2));
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
                        from_version: None,
                        below_version: None,
                    },
                    MemberDef::FormId {
                        sig: None,
                        name: "Global Variable".into(),
                        valid_refs: vec!["GLOB".into(), "NULL".into()],
                        from_version: None,
                        below_version: None,
                    },
                    MemberDef::Integer {
                        sig: None,
                        name: "Required Rank".into(),
                        width: crate::schema::IntegerWidth::S32,
                        signed: true,
                        format: None,
                        from_version: None,
                        below_version: None,
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
                stop_before: vec![],
            },
            MemberDef::Float {
                sig: None,
                name: "Magnitude".into(),
                from_version: None,
                below_version: None,
            },
            MemberDef::Integer {
                sig: None,
                name: "Area".into(),
                width: IntegerWidth::U32,
                signed: false,
                format: None,
                from_version: None,
                below_version: None,
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
                stop_before: vec![],
            },
            MemberDef::Bytes {
                sig: None,
                name: "_unknown".into(),
                len: Some(12),
                from_version: Some(154),
                below_version: Some(166),
            },
            MemberDef::Bytes {
                sig: None,
                name: "_unknown".into(),
                len: Some(8),
                from_version: Some(166),
                below_version: Some(183),
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

    // ── VMAD no-fragments tail tests ─────────────────────────────────────────

    /// Build a minimal VMAD header (version + obj_format + script_count=0).
    /// This is the payload for records that have VMAD attached-scripts but no
    /// script-fragments tail (plain VMAD layout in a "fragmented" record type).
    fn vmad_plain_header() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&5u16.to_le_bytes()); // version
        d.extend_from_slice(&1u16.to_le_bytes()); // obj_format
        d.extend_from_slice(&0u16.to_le_bytes()); // script_count = 0
        d
    }

    #[test]
    fn decode_vmad_info_no_fragments_tail_returns_success() {
        // An INFO VMAD that ends after the scripts header (no fragments tail)
        // must NOT be treated as truncated — it's a valid plain-VMAD layout.
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = decode_vmad_info(&ctx, &data);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
        assert!(
            obj.get("script_fragments").is_none(),
            "script_fragments must be absent when tail is missing"
        );
    }

    #[test]
    fn decode_vmad_pack_no_fragments_tail_returns_success() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = decode_vmad_pack(&ctx, &data);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    #[test]
    fn decode_vmad_perk_no_fragments_tail_returns_success() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = decode_vmad_perk(&ctx, &data);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    #[test]
    fn decode_vmad_scen_no_fragments_tail_returns_success() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = decode_vmad_scen(&ctx, &data);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    #[test]
    fn decode_vmad_qust_no_fragments_tail_returns_success() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = decode_vmad_qust(&ctx, &data);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    fn sample_bash_damage_curve() -> Value {
        json!({
            "formid": "0xDEADBEEF",
            "editor_id": "CT_Test",
            "curve_path": "test.json",
            "curve": [
                {"x": 1.0, "y": 10.0},
                {"x": 50.0, "y": 50.0}
            ]
        })
    }

    fn weap_bash_fixture(
        weapon_type: &str,
        secondary: f64,
        damage_curve: Value,
        keywords: Option<Value>,
    ) -> Map<String, Value> {
        let mut out = Map::new();
        let mut data = Map::new();
        data.insert(
            "Weapon Type".to_string(),
            json!({"value": 0, "name": weapon_type}),
        );
        if secondary != 0.0 {
            data.insert("Secondary Damage".to_string(), json!(secondary));
        }
        out.insert("Data".to_string(), Value::Object(data));
        out.insert("Damage Curve".to_string(), damage_curve);
        if let Some(kw) = keywords {
            out.insert("Keywords".to_string(), json!({"Keywords": kw}));
        }
        out
    }

    fn bash_damage_source(out: &Map<String, Value>) -> Option<&str> {
        out.get("Bash Damage")
            .and_then(|v| v.get("source"))
            .and_then(Value::as_str)
    }

    #[test]
    fn weapon_bash_curve_gun_computes_table() {
        let mut out = weap_bash_fixture("Gun", 5.0, sample_bash_damage_curve(), None);
        apply_weapon_bash_curve(&mut out);
        assert_eq!(bash_damage_source(&out), Some("curve"));
        let curve = out
            .get("Bash Damage")
            .and_then(|v| v.get("curve"))
            .and_then(Value::as_array)
            .expect("curve table");
        assert_eq!(curve.len(), 2);
        assert_eq!(curve[0].get("level").and_then(Value::as_f64), Some(1.0));
        assert_eq!(curve[0].get("damage").and_then(Value::as_f64), Some(5.0));
        assert_eq!(curve[1].get("level").and_then(Value::as_f64), Some(50.0));
        assert_eq!(curve[1].get("damage").and_then(Value::as_f64), Some(25.0));
    }

    #[test]
    fn weapon_bash_curve_automatic_melee_keyword_computes_table() {
        let mut out = weap_bash_fixture(
            "HandToHandMelee",
            8.0,
            sample_bash_damage_curve(),
            Some(json!(["0x006D5081"])),
        );
        apply_weapon_bash_curve(&mut out);
        assert_eq!(bash_damage_source(&out), Some("curve"));
        let damage = out
            .get("Bash Damage")
            .and_then(|v| v.get("curve"))
            .and_then(|c| c.get(1))
            .and_then(|p| p.get("damage"))
            .and_then(Value::as_f64);
        assert_eq!(damage, Some(40.0));
    }

    #[test]
    fn weapon_bash_curve_melee_without_keyword_is_ineligible() {
        let mut out = weap_bash_fixture("TwoHandAxe", 5.0, sample_bash_damage_curve(), None);
        apply_weapon_bash_curve(&mut out);
        assert_eq!(bash_damage_source(&out), Some("ineligible"));
    }

    #[test]
    fn weapon_bash_curve_grenade_is_ineligible() {
        let mut out = weap_bash_fixture("Grenade", 3.0, sample_bash_damage_curve(), None);
        apply_weapon_bash_curve(&mut out);
        assert_eq!(bash_damage_source(&out), Some("ineligible"));
    }

    #[test]
    fn weapon_bash_curve_zero_secondary_stays_silent() {
        let mut absent = weap_bash_fixture("Gun", 0.0, sample_bash_damage_curve(), None);
        absent
            .get_mut("Data")
            .and_then(Value::as_object_mut)
            .expect("Data")
            .remove("Secondary Damage");
        apply_weapon_bash_curve(&mut absent);
        assert!(!absent.contains_key("Bash Damage"));

        let mut zero = weap_bash_fixture("Gun", 0.0, sample_bash_damage_curve(), None);
        apply_weapon_bash_curve(&mut zero);
        assert!(!zero.contains_key("Bash Damage"));
    }

    #[test]
    fn weapon_bash_curve_zero_reference_emits_marker_not_null_damage() {
        let curve = json!({
            "formid": "0x1",
            "curve": [
                {"x": 1.0, "y": 0.0},
                {"x": 50.0, "y": 20.0}
            ]
        });
        let mut out = weap_bash_fixture("Gun", 5.0, curve, None);
        apply_weapon_bash_curve(&mut out);
        assert_eq!(bash_damage_source(&out), Some("curve_zero_reference"));
        assert!(out
            .get("Bash Damage")
            .and_then(|v| v.get("curve"))
            .is_none());
    }

    #[test]
    fn weapon_bash_curve_unresolved_curve_marker() {
        let mut out = weap_bash_fixture("Gun", 5.0, json!("0x0080F217"), None);
        apply_weapon_bash_curve(&mut out);
        assert_eq!(bash_damage_source(&out), Some("unresolved_curve"));
    }

    #[test]
    fn weapon_bash_curve_not_truncated_at_player_cap() {
        let curve = json!({
            "formid": "0x1",
            "curve": [
                {"x": 1.0, "y": 10.0},
                {"x": 50.0, "y": 50.0},
                {"x": 540.0, "y": 540.0}
            ]
        });
        let mut out = weap_bash_fixture("Gun", 2.0, curve, None);
        apply_weapon_bash_curve(&mut out);
        let curve = out
            .get("Bash Damage")
            .and_then(|v| v.get("curve"))
            .and_then(Value::as_array)
            .expect("curve table");
        assert_eq!(curve.len(), 3);
        assert_eq!(curve[2].get("level").and_then(Value::as_f64), Some(540.0));
        assert_eq!(curve[2].get("damage").and_then(Value::as_f64), Some(108.0));
        for point in curve {
            assert!(
                point.get("damage").map(Value::is_null) != Some(true),
                "damage must never be null"
            );
        }
    }
}
