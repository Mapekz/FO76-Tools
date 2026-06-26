use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Schema {
    pub records: HashMap<String, RecordDef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordDef {
    pub name: String,
    #[serde(default)]
    pub members: Vec<MemberDef>,
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
    },
    #[serde(rename = "rarray")]
    RArray {
        name: String,
        element: Box<MemberDef>,
        #[serde(default)]
        count: Option<ArrayCount>,
        /// Halt iteration before consuming the next element when any listed
        /// signature has a lower `doc_index` than the element's first sig-bearing
        /// member. Used for PERK condition groups that are interleaved with
        /// other subrecords (EPFT/PRKC) and cannot be bounded by count alone.
        #[serde(default)]
        stop_before: Vec<String>,
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
    },
    #[serde(rename = "string")]
    String {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        sized: Option<u32>,
        #[serde(default)]
        keep_case: bool,
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
    },
    #[serde(rename = "bytes")]
    Bytes {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        #[serde(default)]
        len: Option<usize>,
    },
    #[serde(rename = "byte_rgba")]
    ByteRgba {
        #[serde(default)]
        sig: Option<String>,
        name: String,
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
    },
    #[serde(rename = "unused")]
    Unused {
        bytes: usize,
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
    #[serde(rename = "raw_fallback")]
    RawFallback {
        #[serde(default)]
        sig: Option<String>,
        name: String,
        reason: String,
    },
    #[serde(rename = "vmad")]
    Vmad {
        #[serde(default)]
        sig: Option<String>,
        name: String,
    },
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
    FillToEnd,
    Fixed(usize),
    CountPath(String),
    /// The array is prefixed by a little-endian unsigned integer that gives the element count.
    /// The prefix byte width is encoded in xEdit's negative `wbArray` count argument:
    /// `-1` → 4 bytes (u32), `-2` → 2 bytes (u16), `-4` → 1 byte (u8).
    /// See `TwbArrayDef::GetPrefixLength` in `TES5Edit/Core/wbInterface.pas`.
    CountPrefix(usize),
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
    FormVersion {
        form_version: FormVersionRange,
    },
    /// Multi-threshold form-version decider: `wbFormVersionDecider([N1, N2, ...])`.
    /// Returns the index of the first threshold where `form_version < threshold`.
    /// If `form_version >= all thresholds`, returns `thresholds.len()` (last variant).
    /// N thresholds produce N+1 variants (indices 0..=N).
    FormVersionThresholds {
        form_version_thresholds: Vec<u16>,
    },
    FromVersion {
        from_version: u16,
    },
    BelowVersion {
        below_version: u16,
    },
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
    /// Select variant by which anchor subrecord is present in the stream.
    /// `present_signature[i]` is the set of subrecord signatures that select
    /// variant `i` (any match counts).  The variant whose earliest-matching
    /// anchor has the lowest `doc_index` wins.
    /// Used for `wbRUnion` (record-level polymorphic unions).
    PresentSignature {
        present_signature: Vec<Vec<String>>,
    },
    Raw,
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

impl Schema {
    pub fn load_embedded() -> anyhow::Result<Self> {
        Self::from_json(include_str!("../schema/fo76.json"))
    }

    pub fn load_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(text)?)
    }

    pub fn record(&self, sig: &str) -> Option<&RecordDef> {
        self.records.get(sig)
    }
}
