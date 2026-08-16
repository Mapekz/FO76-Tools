use crate::curves::Curve;
use crate::formid::FormId;
use crate::reader::OwnedSubrecord;
use crate::schema::{ArrayCount, LStringTable, MemberDef, Schema};
use crate::strings::{Localization, StringKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};

mod model_info;
mod rules;
mod scalars;
mod scope;
mod vmad;
mod walk;

#[cfg(test)]
pub(crate) use rules::apply_weapon_bash_curve;
use rules::{PostDecodeTarget, apply_post_decode_rules};
// `ArrayCount`/`MemberDef` above and `field_int_value` here aren't used
// directly in this file; they exist so that `scope.rs`'s and `rules.rs`'s own
// `use super::*;` (both are private submodules that historically drew these
// names from decode/mod.rs's own namespace) keep resolving after the
// scalar/leaf toolbox and core interpreter moved out to `scalars.rs`/`walk.rs`.
use scalars::field_int_value;
pub(crate) use scalars::json_f32;
#[cfg(test)]
pub(crate) use scalars::member_version_bounds;
pub(crate) use scalars::member_version_ok;
pub use vmad::decode_vmad;
#[cfg(test)]
use vmad::{
    decode_vmad_info, decode_vmad_pack, decode_vmad_perk, decode_vmad_qust, decode_vmad_scen,
};
use walk::decode_member;

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
    if valid_refs.iter().any(|r| r == "CURV")
        && let Some(curves) = ctx.curves
        && let Some(curve) = curves.get(id)
    {
        return json!({
            "formid": id.display(),
            "editor_id": curve.edid,
            "curve_path": curve.path,
            "curve": curve_points_value(curve)
        });
    }

    // Reference-following branch
    if ctx.resolve_depth != ResolveDepth::None
        && let Some(resolver) = ctx.resolver
    {
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
        apply_post_decode_rules(PostDecodeTarget::Record(&mut out));
    }

    Value::Object(out)
}

fn lstring_table_to_kind(
    table: &LStringTable,
    record_sig: Option<&str>,
    subrecord_sig: &str,
) -> StringKind {
    match table {
        LStringTable::Dlstrings => return StringKind::DlStrings,
        LStringTable::Ilstrings => return StringKind::IlStrings,
        LStringTable::Strings => {}
    }
    match (record_sig, subrecord_sig) {
        (Some(rec), "DESC") if rec != "LSCR" => StringKind::DlStrings, // DESC always dlstrings except LSCR
        (Some("QUST"), "CNAM") => StringKind::DlStrings,               // quest log entry
        (Some("BOOK"), "CNAM") => StringKind::DlStrings,               // book description
        (Some("INFO"), sub) if sub != "RNAM" => StringKind::IlStrings, // dialog; RNAM stays lsString
        _ => StringKind::Strings,
    }
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
    use crate::schema::Schema;
    use serde_json::Map;

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
    /// (`0x0050A5D8`): its VMAD script property `MutationSpell` (object
    /// format 2) carries the verbatim union bytes
    /// `00 00 ff ff 14 1f 4e 00` — Unused(u16)=0, Alias(s16)=-1,
    /// FormID(u32)=0x004E1F14 (the SPEL `Mutation_AdrenalReaction`).
    ///
    /// The offset (`if obj_format == 2 { 0 } else { 4 }`) must read the
    /// *last* 4 bytes: reading the first 4 (`00 00 ff ff`) instead produces
    /// the garbage FormID `0xFFFF0000`, which doesn't exist in any ESM,
    /// silently dropping the real mutation-SPEL reference from both the
    /// decoded record and the xref index.
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
        assert!(
            out.get("Bash Damage")
                .and_then(|v| v.get("curve"))
                .is_none()
        );
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
