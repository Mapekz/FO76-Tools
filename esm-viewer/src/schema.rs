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
    /// The array is prefixed by a 4-byte signed integer that gives the element count.
    CountPrefix,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UnionDecider {
    FormVersion {
        form_version: FormVersionRange,
    },
    FromVersion {
        from_version: u16,
    },
    BelowVersion {
        below_version: u16,
    },
    /// Select a variant by reading a single byte at a fixed offset in the payload.
    /// `byte_offset` is the discriminating field (unique to this variant).
    ByteAtOffset {
        byte_offset: usize,
        #[serde(default)]
        default_variant: Option<usize>,
        map: HashMap<u8, usize>,
    },
    /// Select a variant by looking up an already-decoded sibling field's integer value.
    /// `field` is the discriminating field (unique to this variant).
    FieldValue {
        field: String,
        #[serde(default)]
        default_variant: Option<usize>,
        map: HashMap<String, usize>,
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
