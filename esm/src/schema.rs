use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// The decoder's record definitions, keyed by signature.
///
/// Each definition stays raw JSON until first use: a query touches a handful
/// of record types, and parsing all ~180 definitions up front was most of
/// what opening a database cost. The embedded schema arrives already split
/// per record type (`build.rs`) and is parsed and validated in full by
/// `tests::embedded_schema_parses_and_validates`; a schema loaded from a
/// file is parsed and validated eagerly.
#[derive(Debug)]
pub struct Schema {
    records: HashMap<Cow<'static, str>, LazyRecord>,
}

#[derive(Debug)]
struct LazyRecord {
    raw: Cow<'static, str>,
    parsed: OnceLock<RecordDef>,
}

impl LazyRecord {
    fn new(raw: Cow<'static, str>) -> Self {
        LazyRecord {
            raw,
            parsed: OnceLock::new(),
        }
    }
}

impl LazyRecord {
    fn parse(&self, sig: &str) -> anyhow::Result<&RecordDef> {
        if let Some(def) = self.parsed.get() {
            return Ok(def);
        }
        let def: RecordDef = serde_json::from_str(&self.raw)
            .map_err(|e| anyhow::anyhow!("schema record {sig}: {e}"))?;
        def.validate(sig)?;
        Ok(self.parsed.get_or_init(|| def))
    }
}

#[derive(Deserialize)]
struct RawSchema {
    records: HashMap<String, Box<RawValue>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordDef {
    pub name: String,
    #[serde(default)]
    pub members: Vec<MemberDef>,
    /// xEdit `aAllowUnordered`: subrecords bind to members by signature in
    /// any order, instead of following member order (see `decode/bind.rs`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unordered: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind")]
pub enum MemberDef {
    #[serde(rename = "struct")]
    Struct {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        fields: Vec<FieldDef>,
        #[serde(default)]
        from_version: Option<u16>,
        #[serde(default)]
        below_version: Option<u16>,
    },
    #[serde(rename = "rstruct")]
    RStruct {
        name: String,
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
        element: Box<MemberDef>,
        #[serde(default)]
        count: Option<ArrayCount>,
    },
    #[serde(rename = "array")]
    Array {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        element: Box<FieldDef>,
        #[serde(default)]
        count: Option<ArrayCount>,
    },
    #[serde(rename = "union")]
    Union {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        decider: UnionDecider,
        variants: Vec<MemberDef>,
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
        /// `member_from_size_ok` in decode.rs.
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

// `SCHEMA_DIGEST`: FNV-1a over the embedded `fo76.json`, `fo76.ctda.json` and
// `hardcoded_fo76.json`, computed by `build.rs`.
include!(concat!(env!("OUT_DIR"), "/schema_digest.rs"));

// `EMBEDDED_RECORDS`: `fo76.json`'s record definitions as raw JSON, one
// `(signature, definition)` pair per record type, split by `build.rs`.
include!(concat!(env!("OUT_DIR"), "/schema_records.rs"));

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

/// Default `width_bytes` value (1) for `ByteAtOffset`.
fn default_width_bytes() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
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
        #[serde(default)]
        default_variant: Option<usize>,
        map: HashMap<String, usize>,
        #[serde(default = "default_width_bytes")]
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
        #[serde(default)]
        default_variant: Option<usize>,
    },
    /// Select a variant by looking up an already-decoded sibling field's value.
    /// `field` supports dot-separated paths (e.g. `"Struct.Field"`).
    /// `bits` is checked first: ordered `[mask, variant_index]` pairs; first match wins.
    /// `map` is checked next: string key of the integer/enum value → variant index.
    FieldValue {
        field: String,
        #[serde(default)]
        default_variant: Option<usize>,
        #[serde(default)]
        map: HashMap<String, usize>,
        /// Ordered bitmask checks: `[[mask, variant_index], ...]`.
        /// First entry where `(int_value & mask) != 0` wins; checked before `map`.
        #[serde(default)]
        bits: Vec<[u64; 2]>,
    },
    /// Select variant by resolving a sibling FormID field to its target record signature.
    FormIdTargetType {
        form_id_target_type: String,
        map: HashMap<String, usize>,
        #[serde(default)]
        default_variant: Option<usize>,
    },
    /// Select variant by the first character of the record's EditorID (EDID subrecord).
    /// `edid_prefix` maps single-char strings to variant indices.
    EdidPrefix {
        edid_prefix: HashMap<String, usize>,
        #[serde(default)]
        edid_default: Option<usize>,
    },
    /// A `wbRUnion` without a decider: the first variant that can bind the
    /// current subrecord (see `decode/bind.rs`). Always `{"by_signature": true}`.
    BySignature { by_signature: bool },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FormVersionRange {
    pub min: u16,
    pub max: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
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
        f: &mut impl FnMut(&MemberDef) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
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

impl Schema {
    pub fn load_embedded() -> anyhow::Result<Self> {
        Ok(Schema {
            records: EMBEDDED_RECORDS
                .iter()
                .map(|&(sig, raw)| (Cow::Borrowed(sig), LazyRecord::new(Cow::Borrowed(raw))))
                .collect(),
        })
    }

    pub fn load_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    /// Parse and validate every record definition in `text`.
    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        let raw: RawSchema = serde_json::from_str(text)?;
        let schema = Schema {
            records: raw
                .records
                .into_iter()
                .map(|(sig, raw)| {
                    let raw = Cow::Owned(raw.get().to_owned());
                    (Cow::Owned(sig), LazyRecord::new(raw))
                })
                .collect(),
        };
        for (sig, record) in &schema.records {
            record.parse(sig)?;
        }
        Ok(schema)
    }

    /// The definition for record type `sig`, parsed on first use.
    ///
    /// # Panics
    /// If the embedded schema holds a definition the decoder rejects — which
    /// `tests::embedded_schema_parses_and_validates` makes a test failure,
    /// not a runtime one.
    pub fn record(&self, sig: &str) -> Option<&RecordDef> {
        let record = self.records.get(sig)?;
        Some(record.parse(sig).unwrap_or_else(|e| panic!("{e:#}")))
    }

    /// Every record type's signature and definition, parsing each on first use.
    pub fn records(&self) -> impl Iterator<Item = (&str, &RecordDef)> {
        self.records
            .keys()
            .filter_map(|sig| Some((sig.as_ref(), self.record(sig)?)))
    }
}

impl RecordDef {
    /// Reject content the decoder cannot interpret, so a bad extractor or
    /// override edit fails loudly instead of decoding silently wrong.
    fn validate(&self, sig: &str) -> anyhow::Result<()> {
        {
            for member in &self.members {
                member.try_visit(&mut |m| match m {
                    MemberDef::RArray {
                        name,
                        count: Some(ArrayCount::CountPath(c)),
                        ..
                    }
                    | MemberDef::Array {
                        name,
                        count: Some(ArrayCount::CountPath(c)),
                        ..
                    } if c.up > 1 || c.path.is_empty() => anyhow::bail!(
                        "{sig} {name:?}: count path {c:?} must climb at most one scope \
                         and name a field"
                    ),
                    _ => Ok(()),
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordDef, Schema};

    fn one_array(count_path: &str) -> String {
        format!(
            r#"{{"records":{{"TEST":{{"name":"Test","members":[{{"kind":"array","name":"Things",
            "element":{{"kind":"integer","name":"Thing","width":"u8"}},
            "count":{{"count_path":{count_path}}}}}]}}}}}}"#
        )
    }

    /// Every record definition, through both the build-time split the
    /// binary embeds and a parse of the file itself.
    #[test]
    fn embedded_schema_parses_and_validates() {
        let text = include_str!("../schema/fo76.json");
        let raw: serde_json::Value = serde_json::from_str(text).unwrap();
        let count = raw["records"].as_object().unwrap().len();
        assert_eq!(Schema::from_json(text).unwrap().records().count(), count);
        let embedded = Schema::load_embedded().unwrap();
        assert_eq!(embedded.records().count(), count);
        for (sig, def) in raw["records"].as_object().unwrap() {
            let parsed = serde_json::to_value(embedded.record(sig).unwrap()).unwrap();
            let reparsed: RecordDef = serde_json::from_value(def.clone()).unwrap();
            assert_eq!(parsed, serde_json::to_value(reparsed).unwrap(), "{sig}");
        }
    }

    #[test]
    fn accepts_a_count_path_one_scope_up() {
        Schema::from_json(&one_array(r#"{"up":1,"path":["Counts","Things"]}"#)).unwrap();
    }

    #[test]
    fn rejects_a_count_path_the_decoder_cannot_resolve() {
        for bad in [
            r#"{"up":2,"path":["Count"]}"#,
            r#"{"up":0,"path":[]}"#,
            r#""..\\XCNT\\Count""#,
        ] {
            assert!(Schema::from_json(&one_array(bad)).is_err(), "{bad}");
        }
    }
}
