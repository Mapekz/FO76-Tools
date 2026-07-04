use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const HARDCODED_MAX: u32 = 0x800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FormId(pub u32);

impl FormId {
    pub fn new(raw: u32) -> Self {
        FormId(raw)
    }

    pub fn raw(self) -> u32 {
        self.0
    }

    pub fn master_index(self) -> u8 {
        (self.0 >> 24) as u8
    }

    pub fn object_id(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }

    pub fn is_hardcoded(self) -> bool {
        self.0 < HARDCODED_MAX
    }

    pub fn display(self) -> String {
        format!("0x{:08X}", self.0)
    }
}

impl fmt::Display for FormId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

impl FromStr for FormId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_formid(s)
    }
}

pub fn parse_formid(s: &str) -> anyhow::Result<FormId> {
    let s = s.trim();
    let raw = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)?
    } else if s.chars().all(|c| c.is_ascii_hexdigit())
        && s.len() <= 8
        && s.chars().any(|c| c.is_ascii_alphabetic())
    {
        u32::from_str_radix(s, 16)?
    } else {
        s.parse::<u32>()?
    };
    Ok(FormId(raw))
}

/// Serde helper for struct fields that must stay a genuine `FormId` for
/// internal Rust use (e.g. HashMap keys, `.raw()`/`.master_index()` calls)
/// but need to cross a JSON API boundary as a pre-formatted hex string
/// (`"0x0000463F"`) rather than `FormId`'s default bare-number derive.
///
/// `FormId`'s own `#[derive(Serialize, Deserialize)]` intentionally stays a
/// raw `u32` newtype — it's used as a `bincode`-cached `HashMap` key
/// (`Index::form_index` etc.), and switching that derive to a string would
/// bloat and break the on-disk `.esm.idx` cache. Apply this module instead,
/// per-field, via `#[serde(with = "crate::formid::hex_string")]`, wherever a
/// struct's `FormId` field is meant for JSON output/input specifically (see
/// `RecordHeaderInfo::form_id`).
pub mod hex_string {
    use super::FormId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &FormId, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&id.display())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<FormId, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse::<FormId>().map_err(serde::de::Error::custom)
    }
}
