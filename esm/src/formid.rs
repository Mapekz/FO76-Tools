use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FormId(pub u32);

impl FormId {
    pub fn new(raw: u32) -> Self {
        FormId(raw)
    }

    pub fn raw(self) -> u32 {
        self.0
    }

    /// Whether this is a "none" value rather than a reference: fields store
    /// no reference as 0, and some (INFO `Emotion`, for one) as 0xFFFFFFFF.
    pub fn is_null(self) -> bool {
        self.0 == 0 || self.0 == u32::MAX
    }

    pub fn display(self) -> String {
        format!("0x{:08X}", self.0)
    }

    /// Render in the given [`FormIdBase`]: hex is `display()`'s `0x########`,
    /// decimal is the bare `u32` value. Used only where a caller (currently
    /// just the CLI's `--decimal` flag) needs to switch rendering at
    /// runtime — every other call site keeps using `display()` directly, and
    /// `display()`'s exact output is part of every JSON contract.
    pub fn display_base(self, base: FormIdBase) -> String {
        match base {
            FormIdBase::Hex => self.display(),
            FormIdBase::Dec => self.0.to_string(),
        }
    }
}

/// Which base a bare (non-`0x`-prefixed) FormID token is read as, and which
/// base identity FormIDs are rendered in. `0x`-prefixed input is always hex
/// regardless of this setting. See `--decimal` in the CLI and
/// `docs/adr/0010-formid-input-base.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormIdBase {
    #[default]
    Hex,
    Dec,
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
    } else if s.chars().all(|c| c.is_ascii_hexdigit()) && s.len() <= 8 {
        // FormIDs are conventionally written in hex everywhere in this
        // domain (xEdit, the wiki, this CLI's own `display()`), so a bare
        // all-hex-digit token — including one that happens to be all decimal
        // digits, e.g. "00568635" — is read as hex here. Decimal is never an
        // implicit fallback: it's only available via an explicit
        // `FormIdBase::Dec` at selector-construction time (the CLI's
        // `--decimal` flag) — see `docs/adr/0010-formid-input-base.md`.
        u32::from_str_radix(s, 16)?
    } else {
        // Falls through for anything not a bare ≤8-digit hex run: an
        // explicit `--formid` value longer than 8 digits (e.g. a 9-digit
        // decimal), or non-hex input that should error out below.
        s.parse::<u32>()?
    };
    Ok(FormId(raw))
}

/// Serde helper for struct fields that must stay a genuine `FormId` for
/// internal Rust use (e.g. HashMap keys, `.raw()` calls)
/// but need to cross a JSON API boundary as a pre-formatted hex string
/// (`"0x0000463F"`) rather than `FormId`'s default bare-number derive.
///
/// `FormId`'s own `#[derive(Serialize, Deserialize)]` intentionally stays a
/// raw `u32` newtype. Its rkyv-cached counterparts (`forms`'s sorted
/// `Vec<(u32, RecordMeta)>`, `xref`'s `HashMap<u32, Vec<u32>>` — see
/// `index.rs`) go further and store the bare `u32` directly rather than this
/// type at all, since a plain integer needs no endian-wrapper ceremony to
/// archive; switching this serde derive to a string would still bloat any
/// JSON path that touches a `FormId` in bulk. Apply this module instead,
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

pub fn parse_form_id_input(s: &str) -> anyhow::Result<FormId> {
    parse_formid(s)
}

/// Heuristic: returns `true` if `s` looks like a FormID literal (a `0x`-prefixed
/// hex value, or a bare run of only hex digits up to 8 chars — which also covers
/// pure-decimal-looking input like `18000`, read as *hex* `0x18000` by
/// `parse_formid`, not decimal) rather than an EditorID.
///
/// Used to auto-route ambiguous CLI and addon input to the right lookup. Anything
/// with non-hex characters, or longer than 8 hex digits, is treated as an
/// EditorID. Short all-hex EditorIDs (e.g. `cafe`) are read as FormIDs; an
/// explicit `--edid` flag disambiguates those cases. There is no implicit
/// decimal fallback for a bare digit token — decimal is reachable only via
/// an explicit `FormIdBase::Dec` (the CLI's `--decimal` flag), see
/// `docs/adr/0010-formid-input-base.md`.
pub fn looks_like_formid(s: &str) -> bool {
    let s = s.trim();
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    !body.is_empty() && body.len() <= 8 && body.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_all_ones_are_null() {
        assert!(FormId::new(0).is_null());
        assert!(FormId::new(0xFFFF_FFFF).is_null());
        assert!(!FormId::new(0x14).is_null());
    }

    #[test]
    fn hex_prefix_is_always_hex() {
        assert_eq!(parse_formid("0x463F").unwrap().0, 0x463F);
        assert_eq!(parse_formid("0X463F").unwrap().0, 0x463F);
        assert_eq!(parse_formid("0x00568635").unwrap().0, 0x00568635);
    }

    #[test]
    fn bare_all_digit_token_is_hex_first() {
        // The reported bug: "00568635" must read as hex 0x00568635, not
        // decimal 568635 (= 0x0008AD3B).
        assert_eq!(parse_formid("00568635").unwrap().0, 0x00568635);
        assert_eq!(parse_formid("18000").unwrap().0, 0x18000);
    }

    #[test]
    fn bare_hex_with_letters_is_hex() {
        assert_eq!(parse_formid("463F").unwrap().0, 0x463F);
        assert_eq!(parse_formid("DEADBEEF").unwrap().0, 0xDEADBEEF);
    }

    #[test]
    fn nine_plus_digit_decimal_falls_through() {
        // Longer than 8 hex digits -> not a bare-hex candidate -> plain decimal.
        assert_eq!(parse_formid("123456789").unwrap().0, 123_456_789);
    }

    #[test]
    fn overflow_and_garbage_error() {
        assert!(parse_formid("4294967296").is_err()); // u32::MAX + 1, decimal
        assert!(parse_formid("0xFFFFFFFFF").is_err()); // too many hex digits
        assert!(parse_formid("not-a-formid").is_err());
        assert!(parse_formid("").is_err());
    }

    #[test]
    fn display_base_hex_matches_display() {
        let id = FormId::new(0x00568635);
        assert_eq!(id.display_base(FormIdBase::Hex), id.display());
        assert_eq!(id.display_base(FormIdBase::Hex), "0x00568635");
    }

    #[test]
    fn display_base_dec_is_bare_decimal() {
        let id = FormId::new(0x00568635);
        assert_eq!(id.display_base(FormIdBase::Dec), "5670453");
    }

    #[test]
    fn formid_base_default_is_hex() {
        assert_eq!(FormIdBase::default(), FormIdBase::Hex);
    }
}
