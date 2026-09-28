// Decoding runs over untrusted bytes and must never panic: a malformed
// record degrades to a raw or marked value instead. No `unwrap` outside tests.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use crate::formid::FormId;
use crate::reader::OwnedSubrecord;
use crate::schema::{LStringTable, Schema};
use crate::strings::{Localization, StringKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

mod bind;
mod derived;
pub(crate) mod leaf_values;
mod model_info;
pub mod node;
pub mod resolved;
mod scalars;
mod vmad;
mod walk;

#[cfg(test)]
pub(crate) use derived::apply_weapon_bash_curve;
use derived::{PostDecodeTarget, apply_post_decode_rules};
pub(crate) use derived::{curve_inline, curve_points_node};
use leaf_values::InlineSource;
pub(crate) use scalars::json_f32;
#[cfg(test)]
pub(crate) use scalars::member_version_bounds;
pub(crate) use scalars::member_version_ok;
pub use vmad::decode_vmad;

/// Single source of truth for the schema decode-coverage marker keys (see
/// "Decode output key conventions" in esm/AGENTS.md). Exported to TypeScript
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
    /// Emitted inside a struct whose subrecord has bytes left after its fields.
    pub const TRAILING: &str = "_trailing";
}

/// Controls how deeply FormID references are followed during decode.
///
/// `ts_rs::TS` is derived only under `#[cfg(test)]` (`ts-rs` is a dev-dependency,
/// not a regular one — see `esm/AGENTS.md` "N-API Binding and Electron App").
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
    /// For a reference to a "value-bearing leaf" record type (currently GLOB
    /// and CURV — see `leaf_values`), extra keys carrying that type's bounded
    /// payload are added flat alongside the three above (e.g. GLOB adds
    /// `"Value"`), so the shape above is a floor, not a ceiling.
    Stub,
    /// Recursively decode the referenced record (depth-limited to 2 hops).
    Full,
}

pub trait FormIdRefResolver: Send + Sync {
    /// Look up a FormID stub. Returns None if not found.
    fn stub(&self, id: FormId) -> Option<FormIdStub>;
    /// Fully decode a record by FormID. Returns None if not found or on error.
    fn decode_full(&self, id: FormId) -> Option<Value>;
    /// Value-bearing-leaf inline for `id`, whose target record has signature
    /// `record_type` (see `leaf_values`). Returns the COMPLETE replacement
    /// JSON for the reference (stub keys plus the leaf payload), or `None` to
    /// fall back to the plain stub. Default: never inline, so existing
    /// resolvers (including test fakes) opt out for free.
    fn leaf_inline(&self, id: FormId, record_type: &str) -> Option<Value> {
        let _ = (id, record_type);
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct FormIdStub {
    pub formid: String,
    pub editor_id: Option<String>,
    pub record_type: String,
}

/// The three stub keys as a JSON object map, shared by the plain-stub path
/// and by [`FormIdRefResolver::leaf_inline`] implementations that lift extra
/// keys onto it.
pub(crate) fn stub_map(stub: &FormIdStub) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("formid".to_string(), json!(stub.formid));
    map.insert("editor_id".to_string(), json!(stub.editor_id));
    map.insert("record_type".to_string(), json!(stub.record_type));
    map
}

/// Looks up a referenced record's signature, for unions whose variant depends
/// on what a sibling FormID points at. Independent of `--resolve`: the decoded
/// structure is the same at every depth.
pub trait RecordTypes: Sync {
    fn record_type(&self, id: FormId) -> Option<crate::format::Signature>;
}

impl RecordTypes for crate::index::Index {
    fn record_type(&self, id: FormId) -> Option<crate::format::Signature> {
        self.get_by_formid(id)
            .map(|meta| meta.signature)
            .or_else(|| {
                crate::hardcoded::lookup(id)
                    .map(|form| crate::format::Signature::from_slice(form.record_type.as_bytes()))
            })
    }
}

/// What decoding needs from the database, fixed for the database's lifetime.
#[derive(Clone, Copy)]
pub struct DecodeEnv<'a> {
    pub schema: &'a Schema,
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
    /// Signatures of referenced records, for FormID-target-type unions.
    pub types: Option<&'a dyn RecordTypes>,
    /// How to expand FormID references when rendering.
    pub resolve_depth: ResolveDepth,
    /// Expands FormID references at `--resolve stub`/`full`.
    pub resolver: Option<&'a dyn FormIdRefResolver>,
}

impl<'a> DecodeEnv<'a> {
    /// An environment with nothing but the schema: no string tables, curves,
    /// record lookups or reference resolution.
    pub fn new(schema: &'a Schema) -> Self {
        DecodeEnv {
            schema,
            is_localized: false,
            localization: None,
            curves: None,
            types: None,
            resolve_depth: ResolveDepth::None,
            resolver: None,
        }
    }

    /// A context for decoding one record of form version `form_version`.
    pub fn for_record(self, form_version: u16) -> DecodeContext<'a> {
        DecodeContext {
            env: self,
            form_version,
            outer_struct: None,
            record_signature: None,
            record_edid_char: None,
        }
    }
}

/// One record's decode: the database environment plus per-record and
/// recursion state. Dereferences to its [`DecodeEnv`].
#[derive(Clone)]
pub struct DecodeContext<'a> {
    pub env: DecodeEnv<'a>,
    pub form_version: u16,
    /// Already-decoded fields of the enclosing struct, set when decoding array
    /// elements so that `FieldValue` deciders in element structs can reach parent
    /// fields (e.g. "Form Type" for OMOD property enum selection).
    pub outer_struct: Option<node::Fields>,
    /// Signature of the record type currently being decoded (e.g. `"QUST"`, `"NPC_"`).
    /// Set at the top of `decode_record` so record-type-aware sub-decoders can
    /// branch on it (e.g. which string table an lstring reads).
    pub record_signature: Option<&'a str>,
    /// First character of the current record's EditorID subrecord.
    /// Pre-scanned in `decode_record` for use by `EdidPrefix` union deciders.
    pub record_edid_char: Option<char>,
}

impl<'a> std::ops::Deref for DecodeContext<'a> {
    type Target = DecodeEnv<'a>;

    fn deref(&self) -> &DecodeEnv<'a> {
        &self.env
    }
}

impl<'a> std::ops::DerefMut for DecodeContext<'a> {
    fn deref_mut(&mut self) -> &mut DecodeEnv<'a> {
        &mut self.env
    }
}

impl<'a> DecodeContext<'a> {
    /// A context with nothing but the schema, at form version `form_version`
    /// (see [`DecodeEnv::new`]).
    pub fn bare(schema: &'a Schema, form_version: u16) -> Self {
        DecodeEnv::new(schema).for_record(form_version)
    }

    /// Return a new context identical to `self` but with `outer_struct` set.
    pub(super) fn with_outer_struct(&self, outer: node::Fields) -> DecodeContext<'a> {
        DecodeContext {
            outer_struct: Some(outer),
            ..self.clone()
        }
    }
}

/// Whether a FormID field's `valid_refs` include a value-bearing leaf type
/// whose [`InlineSource`] is [`InlineSource::CurveIndex`] (currently only
/// `"CURV"`); such a reference renders its curve inline (see [`render_formid`]).
pub(crate) fn refs_curve(valid_refs: &[String]) -> bool {
    valid_refs
        .iter()
        .any(|r| matches!(leaf_values::lookup(r), Some(InlineSource::CurveIndex)))
}

/// [`render_formid`] for a field declaring `valid_refs`.
#[cfg(test)]
pub(crate) fn resolve_formid(ctx: &DecodeContext<'_>, valid_refs: &[String], id: FormId) -> Value {
    render_formid(ctx, refs_curve(valid_refs), id)
}

/// Render a FormID reference to its JSON representation.
///
/// For a `curve` reference (see [`refs_curve`]) with a curve index loaded, the
/// curve's EditorID, path, and point data are inlined into the output object —
/// this fires independently of `resolve_depth` because it needs no resolver
/// (see `leaf_values`'s module doc for why this can't merge with the
/// resolver-path branch below: no resolver exists at `ResolveDepth::None`, so
/// the declaring field's `valid_refs` is the only signature signal available
/// there). When `ctx.resolve_depth` is `Stub` or `Full` and a resolver is
/// present, the referenced record is expanded inline (a `Stub` also checks the
/// *target's* own signature for a value-bearing leaf, via
/// [`FormIdRefResolver::leaf_inline`]). Otherwise, a bare hex string is
/// returned.
pub(crate) fn render_formid(ctx: &DecodeContext<'_>, curve: bool, id: FormId) -> Value {
    if curve
        && let Some(curves) = ctx.curves
        && let Some(curve) = curves.get(id)
    {
        return curve_inline(id, curve);
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
                    if let Some(inlined) = resolver.leaf_inline(id, &stub.record_type) {
                        return inlined;
                    }
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

/// Decode a record's subrecords and render the result to JSON.
pub fn decode_record(
    ctx: &DecodeContext<'_>,
    signature: &str,
    subrecords: &[OwnedSubrecord],
) -> Value {
    decode_record_node(ctx, signature, subrecords).into_json(ctx)
}

/// Decode a record's subrecords into a typed [`node::Node`] tree; render it
/// with [`node::Node::into_json`] using the same `ctx`.
pub fn decode_record_node(
    ctx: &DecodeContext<'_>,
    signature: &str,
    subrecords: &[OwnedSubrecord],
) -> node::Node {
    let ctx = ctx.for_signature(signature, subrecords);
    record_node(&ctx, signature, subrecords)
}

impl<'a> DecodeContext<'a> {
    /// This context with the record-level fields (`record_signature`,
    /// `record_edid_char`) set for a record of `signature`, and no enclosing
    /// struct.
    fn for_signature(
        &self,
        signature: &'a str,
        subrecords: &[OwnedSubrecord],
    ) -> DecodeContext<'a> {
        // Pre-scan the EDID subrecord for EdidPrefix union deciders (e.g. GMST value type).
        let record_edid_char = subrecords
            .iter()
            .find(|sr| sr.signature.as_str() == "EDID")
            .and_then(|sr| std::str::from_utf8(&sr.data).ok())
            .and_then(|s| s.trim_end_matches('\0').chars().next());
        DecodeContext {
            record_signature: Some(signature),
            record_edid_char,
            outer_struct: None,
            ..self.clone()
        }
    }
}

/// Decode a record's subrecords into a [`node::Node`] tree. `ctx` must come
/// from [`DecodeContext::for_signature`].
fn record_node(
    ctx: &DecodeContext<'_>,
    signature: &str,
    subrecords: &[OwnedSubrecord],
) -> node::Node {
    use node::{Fields, Node};

    let mut out = Fields::new();
    let record_def = ctx.schema.record(signature);

    let mut cur = bind::Cursor::new(subrecords);
    if let Some(def) = record_def {
        out.insert("_record_type".into(), Node::str(&def.name));
        bind::bind_record(ctx, def, &mut cur, &mut out);
    } else {
        out.insert("_record_type".into(), Node::str(signature));
        out.insert(markers::UNKNOWN_RECORD.into(), Node::Bool(true));
        while cur.skip_one() {}
    }

    // Subrecords no member took, grouped by signature in document order.
    let mut unmapped: Fields = Fields::new();
    for sr in cur.into_unbound() {
        let entry = Node::obj([
            ("signature", Node::str(sr.signature.as_str())),
            ("hex", Node::Str(hex::encode(&sr.data))),
            (markers::RAW, Node::Bool(true)),
            ("reason", Node::str("no schema member takes this subrecord")),
        ]);
        match unmapped.get_mut(sr.signature.as_str()) {
            Some(Node::Array(entries)) => entries.push(entry),
            _ => {
                unmapped.insert(sr.signature.as_str().to_owned(), Node::Array(vec![entry]));
            }
        }
    }
    if !unmapped.is_empty() {
        out.insert(markers::UNMAPPED.into(), Node::Struct(unmapped));
    }

    if signature == "WEAP" {
        apply_post_decode_rules(PostDecodeTarget::Record(&mut out), ctx);
    }

    Node::Struct(out)
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
    use crate::schema::{Schema, VmadFragments};
    use serde_json::Map;

    fn bare_ctx(schema: &Schema) -> DecodeContext<'_> {
        DecodeContext::bare(schema, 208)
    }

    fn empty_schema() -> Schema {
        crate::schema::Schema::from_json(r#"{"records":{}}"#).unwrap()
    }

    /// `resolve_formid`'s CURV branch inlines `formid`, `curve_path`,
    /// `curve`, and `editor_id` (the EditorID `Curve` carries from the CURV
    /// record at index-build time), so every FormID field referencing a curve
    /// table (e.g. ALCH `Health`, ENCH `Curve Table`) keeps the curve's own
    /// EditorID. A curve with no EDID subrecord serializes `editor_id` as
    /// `null` rather than an empty string.
    #[test]
    fn resolve_formid_curv_branch_includes_editor_id() {
        let curve = crate::curves::Curve {
            edid: Some("CT_Legendary_Weapon_Adrenal".to_string()),
            path: r"LegendaryMods\Weapon_DamagePerKill.json".to_string(),
            points: vec![crate::curves::CurvePoint { x: 0.0, y: 0.0 }],
        };
        let curves = crate::curves::CurveIndex::from_curves([(FormId::new(0x1), curve)]).unwrap();
        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.env.curves = Some(&curves);

        let result = resolve_formid(&ctx, &["CURV".to_string()], FormId::new(0x1));
        assert_eq!(result["editor_id"], json!("CT_Legendary_Weapon_Adrenal"));
        assert_eq!(result["formid"], json!(FormId::new(0x1).display()));

        let curve_no_edid = crate::curves::Curve {
            edid: None,
            path: "Foo.json".to_string(),
            points: vec![],
        };
        let curves_no_edid =
            crate::curves::CurveIndex::from_curves([(FormId::new(0x2), curve_no_edid)]).unwrap();
        let mut ctx2 = bare_ctx(&schema);
        ctx2.env.curves = Some(&curves_no_edid);
        let result2 = resolve_formid(&ctx2, &["CURV".to_string()], FormId::new(0x2));
        assert_eq!(result2["editor_id"], Value::Null);
    }

    /// The CURV `valid_refs` branch needs no resolver and must keep firing at
    /// `ResolveDepth::None` — it's the only way a CURV reference's points
    /// (which live outside the ESM entirely) can ever surface. Its shape
    /// must stay exactly `{formid, editor_id, curve_path, curve}`: NO
    /// `record_type` key, unlike a resolver-path stub. `esm-viewer`'s
    /// `isFormIdStub` depends on that absence to render curve points inline
    /// instead of collapsing them to one clickable stub leaf.
    #[test]
    fn curv_valid_refs_branch_still_fires_at_depth_none() {
        let curve = crate::curves::Curve {
            edid: Some("CT_Test".to_string()),
            path: "Test.json".to_string(),
            points: vec![crate::curves::CurvePoint { x: 1.0, y: 2.0 }],
        };
        let curves = crate::curves::CurveIndex::from_curves([(FormId::new(0x3), curve)]).unwrap();
        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        assert_eq!(ctx.resolve_depth, ResolveDepth::None);
        assert!(ctx.resolver.is_none());
        ctx.env.curves = Some(&curves);

        let result = resolve_formid(&ctx, &["CURV".to_string()], FormId::new(0x3));
        let obj = result.as_object().expect("object");
        assert_eq!(
            obj.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            ["curve", "curve_path", "editor_id", "formid"]
                .into_iter()
                .collect()
        );
        assert_eq!(result["curve"], json!([{"x": 1.0, "y": 2.0}]));
    }

    /// A GLOB reference at `Stub` gains a flat `Value` key on top of the
    /// three stub keys, via `FormIdRefResolver::leaf_inline`.
    #[test]
    fn glob_ref_at_stub_carries_value() {
        let schema = empty_schema();
        let target_id = FormId::new(0x10);
        let resolver = StubResolver {
            stubs: std::collections::HashMap::from([(
                target_id,
                FormIdStub {
                    formid: target_id.display(),
                    editor_id: Some("Challenge_Global_0076".into()),
                    record_type: "GLOB".into(),
                },
            )]),
            inlines: std::collections::HashMap::from([(
                target_id,
                json!({
                    "formid": target_id.display(),
                    "editor_id": "Challenge_Global_0076",
                    "record_type": "GLOB",
                    "Value": 76.0,
                }),
            )]),
        };
        let mut ctx = bare_ctx(&schema);
        ctx.env.resolve_depth = ResolveDepth::Stub;
        ctx.env.resolver = Some(&resolver);

        let result = resolve_formid(&ctx, &[], target_id);
        assert_eq!(result["Value"], json!(76.0));
        assert_eq!(result["record_type"], json!("GLOB"));
        assert_eq!(result["formid"], json!(target_id.display()));
    }

    /// When the resolver's `leaf_inline` declines (returns `None` — e.g. the
    /// target has no leaf row, or its value field was absent), `Stub`
    /// degrades byte-identically to the plain three-key stub.
    #[test]
    fn leaf_inline_none_degrades_to_plain_stub() {
        let schema = empty_schema();
        let target_id = FormId::new(0x20);
        let resolver = StubResolver {
            stubs: std::collections::HashMap::from([(
                target_id,
                FormIdStub {
                    formid: target_id.display(),
                    editor_id: Some("SomeGlob".into()),
                    record_type: "GLOB".into(),
                },
            )]),
            ..Default::default()
        };
        let mut ctx = bare_ctx(&schema);
        ctx.env.resolve_depth = ResolveDepth::Stub;
        ctx.env.resolver = Some(&resolver);

        let result = resolve_formid(&ctx, &[], target_id);
        assert_eq!(
            result,
            json!({
                "formid": target_id.display(),
                "editor_id": "SomeGlob",
                "record_type": "GLOB",
            })
        );
    }

    /// A resolver's `leaf_inline` returning `None` for the requested id (here
    /// because a SPEL reference isn't a value-bearing leaf type at all —
    /// `DatabaseResolver`'s real `leaf_inline` would decline for the same
    /// reason, via `leaf_values::lookup` missing "SPEL") falls through to the
    /// plain stub, even when the resolver DOES have an unrelated inline
    /// entry populated — proving `resolve_formid` keys strictly off this
    /// reference's own id, never off "does this resolver do any inlining."
    #[test]
    fn non_table_record_type_stub_unchanged() {
        let schema = empty_schema();
        let target_id = FormId::new(0x30);
        let resolver = StubResolver {
            stubs: std::collections::HashMap::from([(
                target_id,
                FormIdStub {
                    formid: target_id.display(),
                    editor_id: Some("Mutation_AdrenalReaction".into()),
                    record_type: "SPEL".into(),
                },
            )]),
            // Deliberately populated for an unrelated FormID, to prove a SPEL
            // stub isn't accidentally matched by an empty/default inlines map.
            inlines: std::collections::HashMap::from([(
                FormId::new(0x31),
                json!({"formid": "0x00000031", "Value": 1.0}),
            )]),
        };
        let mut ctx = bare_ctx(&schema);
        ctx.env.resolve_depth = ResolveDepth::Stub;
        ctx.env.resolver = Some(&resolver);

        let result = resolve_formid(&ctx, &[], target_id);
        assert_eq!(
            result,
            json!({
                "formid": target_id.display(),
                "editor_id": "Mutation_AdrenalReaction",
                "record_type": "SPEL",
            })
        );
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
            ..Default::default()
        };
        let mut ctx = bare_ctx(&schema);
        ctx.env.resolve_depth = ResolveDepth::Stub;
        ctx.env.resolver = Some(&resolver);

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

    #[derive(Default)]
    struct StubResolver {
        stubs: std::collections::HashMap<FormId, FormIdStub>,
        /// Value-bearing-leaf inlines this fake resolver knows about, keyed
        /// by FormID. Empty by default, which — combined with
        /// `leaf_inline`'s crate-default body — makes every other
        /// `StubResolver` in this file opt out of inlining unchanged.
        inlines: std::collections::HashMap<FormId, Value>,
    }

    impl FormIdRefResolver for StubResolver {
        fn stub(&self, id: FormId) -> Option<FormIdStub> {
            self.stubs.get(&id).cloned()
        }

        fn decode_full(&self, _id: FormId) -> Option<Value> {
            None
        }

        fn leaf_inline(&self, id: FormId, _record_type: &str) -> Option<Value> {
            self.inlines.get(&id).cloned()
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
        let v = vmad::vmad_node(&ctx, &data, Some(VmadFragments::Info)).into_json(&ctx);
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
        let v = vmad::vmad_node(&ctx, &data, Some(VmadFragments::Pack)).into_json(&ctx);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    #[test]
    fn decode_vmad_perk_no_fragments_tail_returns_success() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = vmad::vmad_node(&ctx, &data, Some(VmadFragments::Perk)).into_json(&ctx);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    #[test]
    fn decode_vmad_scen_no_fragments_tail_returns_success() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = vmad::vmad_node(&ctx, &data, Some(VmadFragments::Scen)).into_json(&ctx);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    #[test]
    fn decode_vmad_qust_no_fragments_tail_returns_success() {
        let schema = empty_schema();
        let ctx = bare_ctx(&schema);
        let data = vmad_plain_header();
        let v = vmad::vmad_node(&ctx, &data, Some(VmadFragments::Qust)).into_json(&ctx);
        let obj = v.as_object().expect("must return an object");
        assert!(obj.get("_raw").is_none(), "must not be a raw fallback");
        assert!(obj.get("version").is_some(), "version must be present");
    }

    /// The Damage Curve every bash fixture references.
    const BASH_CURVE: FormId = FormId(0x1);

    const SAMPLE_BASH_CURVE: &[(f32, f32)] = &[(1.0, 10.0), (50.0, 50.0)];

    fn weap_bash_fixture(
        weapon_type: &str,
        secondary: f64,
        keywords: Option<Vec<FormId>>,
    ) -> node::Fields {
        use node::{Fields, Node};
        let mut out = Fields::new();
        let mut data = Fields::new();
        data.insert(
            "Weapon Type".to_string(),
            Node::Enum {
                value: 0,
                name: weapon_type.to_string(),
            },
        );
        if secondary != 0.0 {
            data.insert(
                "Secondary Damage".to_string(),
                Node::Float(secondary as f32),
            );
        }
        out.insert("Data".to_string(), Node::Struct(data));
        out.insert(
            "Damage Curve".to_string(),
            Node::FormId {
                id: BASH_CURVE,
                curve: true,
            },
        );
        if let Some(kw) = keywords {
            let kw = kw
                .into_iter()
                .map(|id| Node::FormId { id, curve: false })
                .collect();
            out.insert(
                "Keywords".to_string(),
                Node::obj([("Keywords", Node::Array(kw))]),
            );
        }
        out
    }

    /// Run the WEAP bash rule over `out`, with `BASH_CURVE` holding `points`
    /// (`None`: no curve index loaded), and render the result.
    fn bash(mut out: node::Fields, points: Option<&[(f32, f32)]>) -> Map<String, Value> {
        let curves = points.map(|points| {
            let curve = crate::curves::Curve {
                edid: None,
                path: "test.json".to_string(),
                points: points
                    .iter()
                    .map(|&(x, y)| crate::curves::CurvePoint { x, y })
                    .collect(),
            };
            crate::curves::CurveIndex::from_curves([(BASH_CURVE, curve)]).unwrap()
        });
        let schema = empty_schema();
        let mut ctx = bare_ctx(&schema);
        ctx.env.curves = curves.as_ref();
        apply_weapon_bash_curve(&mut out, &ctx);
        match node::Node::Struct(out).into_json(&ctx) {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    fn bash_damage_source(out: &Map<String, Value>) -> Option<&str> {
        out.get("Bash Damage")
            .and_then(|v| v.get("source"))
            .and_then(Value::as_str)
    }

    #[test]
    fn weapon_bash_curve_gun_computes_table() {
        let out = bash(weap_bash_fixture("Gun", 5.0, None), Some(SAMPLE_BASH_CURVE));
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
        let out = bash(
            weap_bash_fixture("HandToHandMelee", 8.0, Some(vec![FormId::new(0x006D5081)])),
            Some(SAMPLE_BASH_CURVE),
        );
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
        let out = bash(
            weap_bash_fixture("TwoHandAxe", 5.0, None),
            Some(SAMPLE_BASH_CURVE),
        );
        assert_eq!(bash_damage_source(&out), Some("ineligible"));
    }

    #[test]
    fn weapon_bash_curve_grenade_is_ineligible() {
        let out = bash(
            weap_bash_fixture("Grenade", 3.0, None),
            Some(SAMPLE_BASH_CURVE),
        );
        assert_eq!(bash_damage_source(&out), Some("ineligible"));
    }

    #[test]
    fn weapon_bash_curve_zero_secondary_stays_silent() {
        let absent = bash(weap_bash_fixture("Gun", 0.0, None), Some(SAMPLE_BASH_CURVE));
        assert!(!absent.contains_key("Bash Damage"));

        let mut zero = weap_bash_fixture("Gun", 0.0, None);
        if let Some(node::Node::Struct(data)) = zero.get_mut("Data") {
            data.insert("Secondary Damage".into(), node::Node::Float(0.0));
        }
        let zero = bash(zero, Some(SAMPLE_BASH_CURVE));
        assert!(!zero.contains_key("Bash Damage"));
    }

    #[test]
    fn weapon_bash_curve_zero_reference_emits_marker_not_null_damage() {
        let out = bash(
            weap_bash_fixture("Gun", 5.0, None),
            Some(&[(1.0, 0.0), (50.0, 20.0)]),
        );
        assert_eq!(bash_damage_source(&out), Some("curve_zero_reference"));
        assert!(
            out.get("Bash Damage")
                .and_then(|v| v.get("curve"))
                .is_none()
        );
    }

    #[test]
    fn weapon_bash_curve_unresolved_curve_marker() {
        let out = bash(weap_bash_fixture("Gun", 5.0, None), None);
        assert_eq!(bash_damage_source(&out), Some("unresolved_curve"));
    }

    #[test]
    fn weapon_bash_curve_not_truncated_at_player_cap() {
        let out = bash(
            weap_bash_fixture("Gun", 2.0, None),
            Some(&[(1.0, 10.0), (50.0, 50.0), (540.0, 540.0)]),
        );
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
