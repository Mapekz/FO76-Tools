//! The schema's record definitions: the serde types `fo76.json` parses
//! into, and the checks that reject a definition the decoder can't
//! interpret. Self-contained (serde and std only) so `build.rs` compiles it
//! too and fails the build on an invalid embedded definition.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordDef {
    pub name: String,
    #[serde(default, deserialize_with = "commented::members")]
    pub members: Vec<MemberDef>,
    /// xEdit `aAllowUnordered`: subrecords bind to members by signature in
    /// any order, instead of following member order (see `decode/bind.rs`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unordered: bool,
    /// A note for the schema's reader; the decoder ignores it.
    #[serde(default, rename = "_comment", skip_serializing)]
    pub comment: Option<String>,
}

/// Definitions may carry a `_comment` for their reader, which the decoder
/// ignores; any other key a definition's kind doesn't know is an error. The
/// derived deserializer can't express "ignore this one key" on an internally
/// tagged enum, so nested members are read through these.
mod commented {
    use super::MemberDef;
    use serde::de::{Deserialize, Deserializer, Error};

    fn member<E: Error>(mut value: serde_json::Value) -> Result<MemberDef, E> {
        if let serde_json::Value::Object(map) = &mut value {
            map.remove("_comment");
        }
        serde_json::from_value(value).map_err(E::custom)
    }

    pub(super) fn members<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<MemberDef>, D::Error> {
        Vec::<serde_json::Value>::deserialize(d)?
            .into_iter()
            .map(member)
            .collect()
    }

    pub(super) fn boxed<'de, D: Deserializer<'de>>(d: D) -> Result<Box<MemberDef>, D::Error> {
        member(serde_json::Value::deserialize(d)?).map(Box::new)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum MemberDef {
    #[serde(rename = "struct")]
    Struct {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(deserialize_with = "commented::members")]
        fields: Vec<FieldDef>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
    },
    #[serde(rename = "rstruct")]
    RStruct {
        name: String,
        #[serde(deserialize_with = "commented::members")]
        members: Vec<MemberDef>,
        /// xEdit `aAllowUnordered`: members bind by signature in any order.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unordered: bool,
        /// xEdit `dfAllowAnyMember`: any member's subrecord (not only the
        /// first member's) can open the struct.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        any_member: bool,
        /// Signatures whose member closes the struct once bound (xEdit
        /// `dfTerminator`; an unordered struct's last member).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        terminators: Vec<String>,
    },
    #[serde(rename = "rarray")]
    RArray {
        name: String,
        #[serde(deserialize_with = "commented::boxed")]
        element: Box<MemberDef>,
        #[serde(default)]
        count: Option<ArrayCount>,
    },
    #[serde(rename = "array")]
    Array {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(deserialize_with = "commented::boxed")]
        element: Box<FieldDef>,
        #[serde(default)]
        count: Option<ArrayCount>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
    },
    #[serde(rename = "union")]
    Union {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        decider: UnionDecider,
        #[serde(deserialize_with = "commented::members")]
        variants: Vec<MemberDef>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
    },
    #[serde(rename = "integer")]
    Integer {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        width: IntegerWidth,
        #[serde(default)]
        signed: bool,
        #[serde(default)]
        format: Option<ValueFormat>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
        /// `wbFromSize(N, ...)` gate (xEdit wbRecordSizeDecider): present
        /// only when the enclosing subrecord's DataSize >= N; see
        /// `member_from_size_ok` in `decode/scalars.rs`.
        #[serde(default)]
        from_size: Option<usize>,
    },
    #[serde(rename = "float")]
    Float {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
        #[serde(default)]
        from_size: Option<usize>,
    },
    #[serde(rename = "string")]
    String {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        sized: Option<u32>,
    },
    #[serde(rename = "lstring")]
    LString {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        /// Which string table file holds this field's strings.
        /// Defaults to `Strings` (`.strings`) when not specified in the schema.
        #[serde(default)]
        table: LStringTable,
    },
    #[serde(rename = "formid")]
    FormId {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        valid_refs: Vec<String>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
        #[serde(default)]
        from_size: Option<usize>,
    },
    #[serde(rename = "bytes")]
    Bytes {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        len: Option<usize>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
        #[serde(default)]
        from_size: Option<usize>,
    },
    #[serde(rename = "byte_rgba")]
    ByteRgba {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        from_size: Option<usize>,
    },
    #[serde(rename = "vec3")]
    Vec3 {
        #[serde(default)]
        sig: Option<String>,
        name: String,
    },
    #[serde(rename = "empty")]
    Empty {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
    },
    #[serde(rename = "unused")]
    Unused {
        bytes: usize,
        /// Present only for a sig-bearing, subrecord-level `wbUnused(SIG, 0)`
        /// (Pascal: an entire subrecord whose payload is intentionally
        /// ignored) — as opposed to the far more common payload-context
        /// `Unused` used to skip padding bytes *within* an already-consumed
        /// struct payload, which has no `sig` of its own. `None` (the
        /// default, so existing schema JSON keeps parsing without a schema
        /// regen) preserves the payload byte-skip behavior.
        #[serde(default)]
        sig: Option<String>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
    },
    #[serde(rename = "unknown")]
    Unknown {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
    },
    #[serde(rename = "vmad")]
    Vmad {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        /// The script-fragment tail after the scripts, when the record type
        /// has one (xEdit's `wbVMADFragmented*`).
        #[serde(default)]
        fragments: Option<VmadFragments>,
    },
    #[serde(rename = "ctda")]
    Ctda {
        #[serde(default)]
        sig: Option<String>,
        name: String,
    },
    #[serde(rename = "model_info")]
    ModelInfo {
        #[serde(default)]
        sig: Option<String>,
        name: String,
    },
}

/// A VMAD's script-fragment layout, named for the xEdit
/// `wbVMADFragmented*` definition that describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VmadFragments {
    Qust,
    Info,
    Pack,
    Perk,
    Scen,
}

impl MemberDef {
    /// Returns this member's directly declared subrecord signature, if any.
    pub fn sig(&self) -> Option<&str> {
        match self {
            MemberDef::Struct { sig, .. }
            | MemberDef::Array { sig, .. }
            | MemberDef::Union { sig, .. }
            | MemberDef::Integer { sig, .. }
            | MemberDef::Float { sig, .. }
            | MemberDef::String { sig, .. }
            | MemberDef::LString { sig, .. }
            | MemberDef::FormId { sig, .. }
            | MemberDef::Bytes { sig, .. }
            | MemberDef::ByteRgba { sig, .. }
            | MemberDef::Vec3 { sig, .. }
            | MemberDef::Empty { sig, .. }
            | MemberDef::Unused { sig, .. }
            | MemberDef::Unknown { sig, .. }
            | MemberDef::Vmad { sig, .. }
            | MemberDef::Ctda { sig, .. }
            | MemberDef::ModelInfo { sig, .. } => sig.as_deref(),
            MemberDef::RStruct { .. } | MemberDef::RArray { .. } => None,
        }
    }

    /// The member's name (its output key).
    pub fn name(&self) -> &str {
        match self {
            MemberDef::Struct { name, .. }
            | MemberDef::RStruct { name, .. }
            | MemberDef::RArray { name, .. }
            | MemberDef::Array { name, .. }
            | MemberDef::Union { name, .. }
            | MemberDef::Integer { name, .. }
            | MemberDef::Float { name, .. }
            | MemberDef::String { name, .. }
            | MemberDef::LString { name, .. }
            | MemberDef::FormId { name, .. }
            | MemberDef::Bytes { name, .. }
            | MemberDef::ByteRgba { name, .. }
            | MemberDef::Vec3 { name, .. }
            | MemberDef::Empty { name, .. }
            | MemberDef::Unknown { name, .. }
            | MemberDef::Vmad { name, .. }
            | MemberDef::Ctda { name, .. }
            | MemberDef::ModelInfo { name, .. } => name,
            MemberDef::Unused { .. } => "",
        }
    }

    /// Returns whether this member or any nested member declares `sig`.
    pub fn contains_sig(&self, sig: &str) -> bool {
        if self.sig() == Some(sig) {
            return true;
        }
        match self {
            MemberDef::Struct { fields, .. } => fields.iter().any(|field| field.contains_sig(sig)),
            MemberDef::RStruct { members, .. } => {
                members.iter().any(|member| member.contains_sig(sig))
            }
            MemberDef::RArray { element, .. } | MemberDef::Array { element, .. } => {
                element.contains_sig(sig)
            }
            MemberDef::Union { variants, .. } => {
                variants.iter().any(|variant| variant.contains_sig(sig))
            }
            _ => false,
        }
    }
}

pub type FieldDef = MemberDef;

/// Selects which of the three string-table files an LString lives in.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LStringTable {
    /// `.strings` file — plain NUL-terminated strings (e.g. EditorID-style names).
    #[default]
    Strings,
    /// `.dlstrings` file — length-prefixed strings used for descriptions.
    Dlstrings,
    /// `.ilstrings` file — length-prefixed strings used for inventory labels.
    Ilstrings,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegerWidth {
    U8,
    S8,
    U16,
    S16,
    U32,
    S32,
    U64,
    S64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrayCount {
    Fixed(usize),
    CountPath(CountPath),
    /// The array is prefixed by a little-endian unsigned integer that gives the element count.
    /// The prefix byte width is encoded in xEdit's negative `wbArray` count argument:
    /// `-1` → 4 bytes (u32), `-2` → 2 bytes (u16), `-4` → 1 byte (u8).
    /// See `TwbArrayDef::GetPrefixLength` in `TES5Edit/Core/wbInterface.pas`.
    CountPrefix(usize),
    /// The count is the enclosing subrecord's size divided by this (xEdit
    /// counter callbacks such as `wbRDOTCountCallback`).
    PayloadDiv(usize),
}

/// Where an array's element count lives: an integer field already decoded
/// earlier in the record. `up` is how many enclosing scopes to climb from the
/// struct holding the array (0 = a sibling field, 1 = the enclosing struct's or
/// record's fields); `path` is the chain of output names from that scope down to
/// the integer. The extractor produces this from xEdit's `SetCountPath`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CountPath {
    pub up: usize,
    pub path: Vec<String>,
}

/// How a union picks its variant. Each kind is named by its one
/// distinguishing key (`form_version`, `form_version_thresholds`,
/// `byte_offset`, `payload_size`, `field`, `form_id_target_type`,
/// `edid_prefix`, `by_signature`); a definition naming none or several, or
/// carrying a key its kind doesn't use, is rejected (see [`DeciderJson`]).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(try_from = "DeciderJson", into = "DeciderJson")]
pub enum UnionDecider {
    /// Binary form-version decider: variant 1 when `form_version` is in `[min, max]`,
    /// variant 0 otherwise. Matches Pascal `wbFormVersionDecider(N)`.
    FormVersion { form_version: FormVersionRange },
    /// Multi-threshold form-version decider: `wbFormVersionDecider([N1, N2, ...])`.
    /// Returns the index of the first threshold where `form_version < threshold`.
    /// If `form_version >= all thresholds`, returns `thresholds.len()` (last variant).
    /// N thresholds produce N+1 variants (indices 0..=N).
    FormVersionThresholds { form_version_thresholds: Vec<u16> },
    /// Select a variant by reading bytes at a fixed offset in the payload.
    /// `byte_offset` is relative to the union's position in the enclosing struct data.
    /// `width_bytes` controls how many bytes are read (1, 2, or 4, little-endian); default 1.
    /// `map` keys are the decimal string representation of the raw integer value.
    ByteAtOffset {
        byte_offset: usize,
        default_variant: Option<usize>,
        map: HashMap<String, usize>,
        width_bytes: usize,
    },
    /// Select a variant by the union's total available payload byte length.
    /// Matches Pascal deciders that dispatch purely on how many bytes are
    /// actually present (e.g. `wbDeciderCELLFlags`: 2 bytes → u16 flags,
    /// otherwise → u32 flags). `payload_size` keys are the decimal string of
    /// the exact byte length; `default_variant` is used when the length
    /// doesn't match any key.
    PayloadSize {
        payload_size: HashMap<String, usize>,
        default_variant: Option<usize>,
    },
    /// Select a variant by looking up an already-decoded sibling field's value.
    /// `field` supports dot-separated paths (e.g. `"Struct.Field"`).
    /// `bits` is checked first: ordered `[mask, variant_index]` pairs; first match wins.
    /// `map` is checked next: string key of the integer/enum value → variant index.
    FieldValue {
        field: String,
        default_variant: Option<usize>,
        map: HashMap<String, usize>,
        /// Ordered bitmask checks: `[[mask, variant_index], ...]`.
        /// First entry where `(int_value & mask) != 0` wins; checked before `map`.
        bits: Vec<[u64; 2]>,
    },
    /// Select variant by resolving a sibling FormID field to its target record signature.
    FormIdTargetType {
        form_id_target_type: String,
        map: HashMap<String, usize>,
        default_variant: Option<usize>,
    },
    /// Select variant by the first character of the record's EditorID (EDID subrecord).
    /// `edid_prefix` maps single-char strings to variant indices.
    EdidPrefix {
        edid_prefix: HashMap<String, usize>,
        edid_default: Option<usize>,
    },
    /// A `wbRUnion` without a decider: the first variant that can bind the
    /// current subrecord (see `decode/bind.rs`). Always `{"by_signature": true}`.
    BySignature { by_signature: bool },
}

/// A [`UnionDecider`] as written in the schema: every key any kind uses,
/// unknown keys rejected. Converting it checks that exactly one kind is
/// named and that only that kind's keys are set.
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeciderJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    form_version: Option<FormVersionRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    form_version_thresholds: Option<Vec<u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    byte_offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload_size: Option<HashMap<String, usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bits: Option<Vec<[u64; 2]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    form_id_target_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edid_prefix: Option<HashMap<String, usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edid_default: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by_signature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    map: Option<HashMap<String, usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_variant: Option<usize>,
}

impl DeciderJson {
    /// The keys set, by name.
    fn keys(&self) -> Vec<&'static str> {
        let set = [
            ("form_version", self.form_version.is_some()),
            (
                "form_version_thresholds",
                self.form_version_thresholds.is_some(),
            ),
            ("byte_offset", self.byte_offset.is_some()),
            ("width_bytes", self.width_bytes.is_some()),
            ("payload_size", self.payload_size.is_some()),
            ("field", self.field.is_some()),
            ("bits", self.bits.is_some()),
            ("form_id_target_type", self.form_id_target_type.is_some()),
            ("edid_prefix", self.edid_prefix.is_some()),
            ("edid_default", self.edid_default.is_some()),
            ("by_signature", self.by_signature.is_some()),
            ("map", self.map.is_some()),
            ("default_variant", self.default_variant.is_some()),
        ];
        set.into_iter()
            .filter_map(|(k, on)| on.then_some(k))
            .collect()
    }
}

impl TryFrom<DeciderJson> for UnionDecider {
    type Error = String;

    fn try_from(json: DeciderJson) -> Result<Self, String> {
        // Each kind: its distinguishing key, then the other keys it allows.
        const KINDS: [(&str, &[&str]); 8] = [
            ("form_version", &[]),
            ("form_version_thresholds", &[]),
            ("byte_offset", &["width_bytes", "map", "default_variant"]),
            ("payload_size", &["default_variant"]),
            ("field", &["bits", "map", "default_variant"]),
            ("form_id_target_type", &["map", "default_variant"]),
            ("edid_prefix", &["edid_default"]),
            ("by_signature", &[]),
        ];
        let keys = json.keys();
        let named: Vec<&str> = KINDS
            .iter()
            .map(|(kind, _)| *kind)
            .filter(|kind| keys.contains(kind))
            .collect();
        let [kind] = named[..] else {
            return Err(format!(
                "a union decider names exactly one kind, found {named:?} in {keys:?}"
            ));
        };
        let allowed = KINDS
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(&[][..], |(_, allowed)| allowed);
        if let Some(stray) = keys.iter().find(|k| **k != kind && !allowed.contains(k)) {
            return Err(format!("a {kind} union decider doesn't use {stray:?}"));
        }
        let DeciderJson {
            form_version,
            form_version_thresholds,
            byte_offset,
            width_bytes,
            payload_size,
            field,
            bits,
            form_id_target_type,
            edid_prefix,
            edid_default,
            by_signature,
            map,
            default_variant,
        } = json;
        Ok(match kind {
            "form_version" => UnionDecider::FormVersion {
                form_version: form_version.unwrap_or_default(),
            },
            "form_version_thresholds" => UnionDecider::FormVersionThresholds {
                form_version_thresholds: form_version_thresholds.unwrap_or_default(),
            },
            "byte_offset" => UnionDecider::ByteAtOffset {
                byte_offset: byte_offset.unwrap_or_default(),
                default_variant,
                map: map.ok_or("a byte_offset union decider needs a map")?,
                width_bytes: width_bytes.unwrap_or(1),
            },
            "payload_size" => UnionDecider::PayloadSize {
                payload_size: payload_size.unwrap_or_default(),
                default_variant,
            },
            "field" => UnionDecider::FieldValue {
                field: field.unwrap_or_default(),
                default_variant,
                map: map.unwrap_or_default(),
                bits: bits.unwrap_or_default(),
            },
            "form_id_target_type" => UnionDecider::FormIdTargetType {
                form_id_target_type: form_id_target_type.unwrap_or_default(),
                map: map.ok_or("a form_id_target_type union decider needs a map")?,
                default_variant,
            },
            "edid_prefix" => UnionDecider::EdidPrefix {
                edid_prefix: edid_prefix.unwrap_or_default(),
                edid_default,
            },
            _ => UnionDecider::BySignature {
                by_signature: by_signature.unwrap_or_default(),
            },
        })
    }
}

impl From<UnionDecider> for DeciderJson {
    fn from(decider: UnionDecider) -> Self {
        let empty = DeciderJson::default();
        match decider {
            UnionDecider::FormVersion { form_version } => DeciderJson {
                form_version: Some(form_version),
                ..empty
            },
            UnionDecider::FormVersionThresholds {
                form_version_thresholds,
            } => DeciderJson {
                form_version_thresholds: Some(form_version_thresholds),
                ..empty
            },
            UnionDecider::ByteAtOffset {
                byte_offset,
                default_variant,
                map,
                width_bytes,
            } => DeciderJson {
                byte_offset: Some(byte_offset),
                default_variant,
                map: Some(map),
                width_bytes: Some(width_bytes),
                ..empty
            },
            UnionDecider::PayloadSize {
                payload_size,
                default_variant,
            } => DeciderJson {
                payload_size: Some(payload_size),
                default_variant,
                ..empty
            },
            UnionDecider::FieldValue {
                field,
                default_variant,
                map,
                bits,
            } => DeciderJson {
                field: Some(field),
                default_variant,
                map: Some(map),
                bits: Some(bits),
                ..empty
            },
            UnionDecider::FormIdTargetType {
                form_id_target_type,
                map,
                default_variant,
            } => DeciderJson {
                form_id_target_type: Some(form_id_target_type),
                map: Some(map),
                default_variant,
                ..empty
            },
            UnionDecider::EdidPrefix {
                edid_prefix,
                edid_default,
            } => DeciderJson {
                edid_prefix: Some(edid_prefix),
                edid_default,
                ..empty
            },
            UnionDecider::BySignature { by_signature } => DeciderJson {
                by_signature: Some(by_signature),
                ..empty
            },
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FormVersionRange {
    pub min: u16,
    pub max: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ValueFormat {
    Enum {
        #[serde(rename = "enum")]
        values: EnumFormat,
    },
    Flags {
        flags: Vec<String>,
    },
    Str4,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum EnumFormat {
    Dense(Vec<String>),
    Sparse(HashMap<String, String>),
}

impl MemberDef {
    /// Call `f` on this member and every member nested inside it, stopping at
    /// the first error.
    fn try_visit(
        &self,
        f: &mut impl FnMut(&MemberDef) -> Result<(), String>,
    ) -> Result<(), String> {
        f(self)?;
        match self {
            MemberDef::Struct { fields, .. } => fields.iter().try_for_each(|m| m.try_visit(f)),
            MemberDef::RStruct { members, .. } => members.iter().try_for_each(|m| m.try_visit(f)),
            MemberDef::RArray { element, .. } | MemberDef::Array { element, .. } => {
                element.try_visit(f)
            }
            MemberDef::Union { variants, .. } => variants.iter().try_for_each(|m| m.try_visit(f)),
            _ => Ok(()),
        }
    }
}

impl RecordDef {
    /// Reject content the decoder cannot interpret, so a bad extractor or
    /// override edit fails loudly instead of decoding silently wrong: count
    /// paths it can't resolve, version gates that admit nothing, and union
    /// deciders that pick a variant the union lacks or read an impossible
    /// value.
    pub fn validate(&self, sig: &str) -> Result<(), String> {
        for member in &self.members {
            member.try_visit(&mut |m| {
                m.validate()
                    .map_err(|e| format!("{sig} {:?}: {e}", m.name()))
            })?;
        }
        Ok(())
    }
}

impl MemberDef {
    /// This member's own checks (see [`RecordDef::validate`]).
    fn validate(&self) -> Result<(), String> {
        let (from, below) = self.version_gate();
        if let (Some(from), Some(below)) = (from, below)
            && from >= below
        {
            return Err(format!(
                "version gate [{from}, {below}) admits no form version"
            ));
        }
        match self {
            MemberDef::RArray {
                count: Some(ArrayCount::CountPath(c)),
                ..
            }
            | MemberDef::Array {
                count: Some(ArrayCount::CountPath(c)),
                ..
            } if c.up > 1 || c.path.is_empty() => Err(format!(
                "count path {c:?} must climb at most one scope and name a field"
            )),
            MemberDef::Union {
                decider, variants, ..
            } => decider.validate(variants.len()),
            _ => Ok(()),
        }
    }

    /// `(from_version, below_version)`: the form versions this member is
    /// present in (from inclusive, below exclusive), for the kinds that
    /// carry a gate.
    pub fn version_gate(&self) -> (Option<u16>, Option<u16>) {
        match self {
            MemberDef::Array {
                from_version,
                below_version,
                ..
            }
            | MemberDef::Union {
                from_version,
                below_version,
                ..
            }
            | MemberDef::Unknown {
                from_version,
                below_version,
                ..
            }
            | MemberDef::Struct {
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
            | MemberDef::FormId {
                from_version,
                below_version,
                ..
            }
            | MemberDef::Bytes {
                from_version,
                below_version,
                ..
            }
            | MemberDef::Empty {
                from_version,
                below_version,
                ..
            }
            | MemberDef::Unused {
                from_version,
                below_version,
                ..
            } => (*from_version, *below_version),
            _ => (None, None),
        }
    }
}

impl UnionDecider {
    /// Reject a decider that can pick a variant outside `variants`, or reads
    /// a value it can't hold.
    fn validate(&self, variants: usize) -> Result<(), String> {
        let variant = |i: usize| {
            if i < variants {
                Ok(())
            } else {
                Err(format!("decider picks variant {i} of {variants}"))
            }
        };
        let integer_keys = |map: &HashMap<String, usize>| {
            map.iter().try_for_each(|(key, &i)| {
                key.parse::<u64>()
                    .map_err(|_| format!("decider key {key:?} is not an integer"))?;
                variant(i)
            })
        };
        match self {
            UnionDecider::FormVersion { .. } => variant(1),
            UnionDecider::FormVersionThresholds {
                form_version_thresholds: t,
            } => {
                if t.windows(2).any(|w| w[0] >= w[1]) {
                    return Err(format!("form version thresholds {t:?} don't ascend"));
                }
                variant(t.len())
            }
            UnionDecider::ByteAtOffset {
                map,
                default_variant,
                width_bytes,
                ..
            } => {
                if ![1, 2, 4].contains(width_bytes) {
                    return Err(format!("decider reads {width_bytes} bytes, not 1, 2 or 4"));
                }
                integer_keys(map)?;
                default_variant.map_or(Ok(()), variant)
            }
            UnionDecider::PayloadSize {
                payload_size,
                default_variant,
            } => {
                integer_keys(payload_size)?;
                default_variant.map_or(Ok(()), variant)
            }
            UnionDecider::FieldValue {
                field,
                map,
                bits,
                default_variant,
            } => {
                if field.is_empty() {
                    return Err("decider names no field".to_string());
                }
                map.values().try_for_each(|&i| variant(i))?;
                bits.iter().try_for_each(|&[_, i]| {
                    variant(usize::try_from(i).map_err(|_| format!("variant {i} out of range"))?)
                })?;
                default_variant.map_or(Ok(()), variant)
            }
            UnionDecider::FormIdTargetType {
                form_id_target_type,
                map,
                default_variant,
            } => {
                if form_id_target_type.is_empty() {
                    return Err("decider names no FormID field".to_string());
                }
                map.values().try_for_each(|&i| variant(i))?;
                default_variant.map_or(Ok(()), variant)
            }
            UnionDecider::EdidPrefix {
                edid_prefix,
                edid_default,
            } => {
                edid_prefix.iter().try_for_each(|(key, &i)| {
                    if key.chars().count() != 1 {
                        return Err(format!("EditorID prefix {key:?} is not one character"));
                    }
                    variant(i)
                })?;
                edid_default.map_or(Ok(()), variant)
            }
            UnionDecider::BySignature { by_signature } => {
                if *by_signature {
                    Ok(())
                } else {
                    Err("a by_signature decider is always true".to_string())
                }
            }
        }
    }
}
