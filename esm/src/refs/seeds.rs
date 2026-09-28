//! Carrier seeds for a reverse-reference walk: PERK entry points and OMOD
//! properties, selected by id or name pattern.

use crate::formid::FormId;
use crate::wildcard::wildcard_match;
use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A PERK "Entry Point" selector for [`Database::perks_by_entry_point`] —
/// either the enum's numeric id (some ids carry no name at all, e.g. one
/// that only `mod_custom_V63-BERTHA_Perk` uses) or a name pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryPointSpec {
    Id(u16),
    /// Case-insensitive exact match, unless `name` contains `*`, in which
    /// case it's matched via [`crate::wildcard::wildcard_match`] (same
    /// matcher `Database::search` uses).
    Name(String),
}

/// Kind of virtual-seed selector that produced a [`CarrierTag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub enum CarrierKind {
    EntryPoint,
    OmodProperty,
}

/// One tag a virtual-seed carrier matched under a selector (e.g. a PERK
/// entry point under an [`EntryPointSpec`]).
///
/// Carried on [`ops::RefRow::tags`] so every reverse-ref row in a
/// carrier-seeded walk (such as `--entry-point`/`--ep`) can name which
/// hook(s) it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct CarrierTag {
    pub kind: CarrierKind,
    pub id: u16,
    pub name: Option<String>,
    /// Enum-space qualifier for a kind that has one (OMOD property scope:
    /// "weap"/"armo"/"npc"). `None` for `CarrierKind::EntryPoint`, which has
    /// no scope concept.
    pub scope: Option<String>,
}

/// Carrier records from a virtual selector (e.g. [`Database::perks_by_entry_point`]),
/// each tagged with the match(es) that selected it.
pub type Carriers = Vec<(FormId, Vec<CarrierTag>)>;

impl EntryPointSpec {
    /// Parse a CLI token: an all-ASCII-digit token is a numeric id,
    /// everything else is a name pattern — except a `0x`/`0X`-prefixed
    /// token, which is rejected outright rather than silently becoming a
    /// (never-matching) name pattern: it's unambiguously someone passing a
    /// FormID to `--entry-point` by mistake, and entry points are only ever
    /// selected by name or by their small decimal id, never hex.
    pub fn parse(s: &str) -> anyhow::Result<EntryPointSpec> {
        let trimmed = s.trim();
        if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            bail!(
                "'{trimmed}' looks like a FormID, not a PERK entry-point name or \
                 numeric id; use the positional target or --formid for a FormID lookup"
            );
        }
        if !trimmed.is_empty()
            && trimmed.bytes().all(|b| b.is_ascii_digit())
            && let Ok(id) = trimmed.parse::<u16>()
        {
            return Ok(EntryPointSpec::Id(id));
        }
        Ok(EntryPointSpec::Name(trimmed.to_string()))
    }

    pub(crate) fn display(&self) -> String {
        match self {
            EntryPointSpec::Id(id) => id.to_string(),
            EntryPointSpec::Name(n) => format!("'{n}'"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropScope {
    Weap,
    Armo,
    Npc,
}

impl PropScope {
    /// The `Data.Form Type` enum name this scope's OMODs decode to.
    fn form_type_name(self) -> &'static str {
        match self {
            PropScope::Weap => "Weapon",
            PropScope::Armo => "Armor",
            PropScope::Npc => "Non-player character",
        }
    }

    pub(crate) fn tag_str(self) -> &'static str {
        match self {
            PropScope::Weap => "weap",
            PropScope::Armo => "armo",
            PropScope::Npc => "npc",
        }
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        if prefix.eq_ignore_ascii_case("weap") || prefix.eq_ignore_ascii_case("weapon") {
            Some(PropScope::Weap)
        } else if prefix.eq_ignore_ascii_case("armo") || prefix.eq_ignore_ascii_case("armor") {
            Some(PropScope::Armo)
        } else if prefix.eq_ignore_ascii_case("npc") || prefix.eq_ignore_ascii_case("npc_") {
            Some(PropScope::Npc)
        } else {
            None
        }
    }

    pub(crate) fn from_form_type_name(name: &str) -> Option<Self> {
        [PropScope::Weap, PropScope::Armo, PropScope::Npc]
            .into_iter()
            .find(|scope| name == scope.form_type_name())
    }
}

/// An OMOD Property selector for [`Database::omods_by_property`] — optionally
/// scoped to the weapon, armor, or NPC property enum space, then selected by
/// numeric id or name pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmodPropertySpec {
    pub(crate) scope: Option<PropScope>,
    pub(crate) sel: OmodPropertySel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OmodPropertySel {
    Id(u16),
    Name(String),
}

impl OmodPropertySpec {
    /// Parse a CLI token. Numeric ids require a form-type scope because
    /// each OMOD property enum space assigns different meanings to the same
    /// number. Names may be scoped or may fan out across all three spaces.
    pub fn parse(s: &str) -> anyhow::Result<OmodPropertySpec> {
        let trimmed = s.trim();
        if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            bail!(
                "'{trimmed}' looks like a FormID, not an OMOD property name or \
                 numeric id; use the positional target or --formid for a FormID lookup"
            );
        }

        let (scope, rest) = match trimmed.split_once(':') {
            Some((prefix, rest)) => match PropScope::from_prefix(prefix) {
                Some(scope) => (Some(scope), rest),
                None => (None, trimmed),
            },
            None => (None, trimmed),
        };
        let sel = if !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
            && let Ok(id) = rest.parse::<u16>()
        {
            if scope.is_none() {
                bail!("property ids are per-form-type; use weap:<id>, armo:<id>, or npc:<id>");
            }
            OmodPropertySel::Id(id)
        } else {
            OmodPropertySel::Name(rest.to_string())
        };

        Ok(OmodPropertySpec { scope, sel })
    }

    pub(crate) fn display(&self) -> String {
        match (self.scope, &self.sel) {
            (Some(scope), OmodPropertySel::Id(id)) => format!("{}:{id}", scope.tag_str()),
            (Some(scope), OmodPropertySel::Name(name)) => {
                format!("{}:'{name}'", scope.tag_str())
            }
            (None, OmodPropertySel::Id(id)) => id.to_string(),
            (None, OmodPropertySel::Name(name)) => format!("'{name}'"),
        }
    }
}

/// `true` if `name` satisfies `pattern` per [`EntryPointSpec::Name`]'s
/// matching rule: exact case-insensitive unless `pattern` contains `*`.
/// Not [`crate::wildcard::wildcard_match`] alone — that matcher treats a
/// `*`-free pattern as a *substring* search, which would make `--ep 'Mod
/// Weapon Attack Damage'` also hit unrelated entry points like `Mod Weapon
/// DMG Bonus Mult`-adjacent names sharing a prefix.
pub(crate) fn entry_point_name_matches(pattern: &str, name: &str) -> bool {
    if pattern.contains('*') {
        wildcard_match(pattern, name)
    } else {
        pattern.eq_ignore_ascii_case(name)
    }
}

/// `true` if `name` satisfies `pattern` per [`OmodPropertySpec`]'s matching
/// rule: exact case- and whitespace-insensitive unless `pattern` contains
/// `*`, in which case the same glob rule applies to whitespace-stripped forms.
/// Not [`crate::wildcard::wildcard_match`] alone — that matcher treats a
/// `*`-free pattern as a substring search, which is wrong here too.
pub(crate) fn omod_property_name_matches(pattern: &str, name: &str) -> bool {
    let pattern: String = pattern.chars().filter(|c| !c.is_whitespace()).collect();
    let name: String = name.chars().filter(|c| !c.is_whitespace()).collect();
    if pattern.contains('*') {
        wildcard_match(&pattern, &name)
    } else {
        pattern.eq_ignore_ascii_case(&name)
    }
}

/// Extract `(numeric id, name)` from a decoded enum value. Handles both the
/// resolved `{value, name}` shape and the bare-int fallback `format_int` in
/// `decode/mod.rs` emits when an id falls outside its enum's name table.
pub(crate) fn enum_id_name(v: &Value) -> Option<(u16, Option<&str>)> {
    match v {
        Value::Object(o) => {
            let id = o.get("value")?.as_u64()?;
            Some((
                u16::try_from(id).ok()?,
                o.get("name").and_then(Value::as_str),
            ))
        }
        Value::Number(n) => Some((u16::try_from(n.as_u64()?).ok()?, None)),
        _ => None,
    }
}
