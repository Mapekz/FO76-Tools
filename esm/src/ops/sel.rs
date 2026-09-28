//! [`RecordSel`], how a request names a record, and [`resolve_sel`], the one
//! place a selector becomes a FormID.

use crate::{Database, FormId, FormIdBase};
use anyhow::bail;
use serde::{Deserialize, Serialize};

/// Record selector: FormID, EditorID, or an ambiguous bare token that could be
/// either (see [`RecordSel::Auto`]).
///
/// Adjacently tagged so primitive-newtype variants (FormId wraps u32, Edid wraps String)
/// survive JSON round-trips. Internally-tagged enums cannot serialize non-map payloads.
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RecordSel {
    FormId(FormId),
    Edid(String),
    /// A bare token with no explicit `0x` prefix that nonetheless *looks*
    /// like a FormID per [`crate::looks_like_formid`] (e.g. `"18000"`,
    /// `"cafe"`) — resolution tries the FormID interpretation first, then
    /// falls back to an EditorID lookup (see [`resolve_sel`]). This exists
    /// because `looks_like_formid` is a syntactic heuristic that also
    /// matches plenty of real, numeric-looking EditorIDs; an explicit `0x`
    /// prefix (or an explicit `--formid`/`--edid` flag) is unambiguous and
    /// stays `FormId`/`Edid` directly, never `Auto`.
    Auto(String),
    /// A PERK "Entry Point" name or numeric id (see [`crate::EntryPointSpec::parse`]),
    /// resolving to every PERK that carries it rather than a single record.
    /// Only meaningful for [`Op::ReferencedBy`] (via [`resolve_ref_seeds`]) —
    /// `resolve_sel`, which every other `Op` uses, rejects it. Never produced
    /// by [`RecordSel::from_input`]/[`RecordSel::from_parts`]; constructed
    /// only by the CLI's explicit `--entry-point`/`--ep` flag.
    EntryPoint(String),
    /// An OMOD Property name or scoped numeric id (see
    /// [`crate::OmodPropertySpec::parse`]), resolving to every OMOD that declares it
    /// rather than a single record. Only meaningful for
    /// [`Op::ReferencedBy`] (via [`resolve_ref_seeds`]) — `resolve_sel`,
    /// which every other `Op` uses, rejects it. Never produced by
    /// [`RecordSel::from_input`]/[`RecordSel::from_parts`]; constructed only
    /// by the CLI's explicit `--omod-property`/`--prop` flag.
    OmodProperty(String),
}

/// The decimal reading of a bare (no `0x`/`0X` prefix), all-ASCII-digit
/// token, e.g. for `--decimal`-mode selector construction. `None` for a
/// `0x`-prefixed token, an empty string, or anything containing a
/// non-digit character (a letter-bearing hex token, an EditorID) — those
/// are never read as decimal regardless of mode.
fn bare_decimal_formid(s: &str) -> Option<FormId> {
    let t = s.trim();
    let has_hex_prefix = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .is_some();
    if has_hex_prefix || t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<u32>().ok().map(FormId::new)
}

impl RecordSel {
    /// Build a selector from a single user-supplied token, auto-detecting whether
    /// it denotes a FormID (numeric/hex) or an EditorID via [`crate::looks_like_formid`].
    /// Equivalent to [`RecordSel::from_input_with`] under [`FormIdBase::Hex`].
    ///
    /// A bare (no `0x`/`0X` prefix) formid-looking token is ambiguous — it
    /// could be a real FormID or a numeric-looking EditorID — so it becomes
    /// [`RecordSel::Auto`] rather than eagerly committing to `FormId`; an
    /// explicit `0x`-prefixed token is unambiguous and stays `FormId`.
    pub fn from_input(s: &str) -> anyhow::Result<RecordSel> {
        Self::from_input_with(s, FormIdBase::Hex)
    }

    /// [`RecordSel::from_input`], but under [`FormIdBase::Dec`] a bare
    /// all-digit token (no `0x` prefix, no letters) commits directly to its
    /// decimal reading as a concrete `FormId` rather than becoming
    /// [`RecordSel::Auto`], and the hex reading is never attempted at all.
    /// This is the only way to reach a decimal-reading FormID: without this
    /// flag, [`resolve_sel`]'s `Auto` arm always reads a bare digit token as
    /// hex, with no implicit decimal fallback on a miss (see
    /// `docs/adr/0010-formid-input-base.md`). A `0x`-prefixed token, or one
    /// containing a letter, is unaffected by `base` and resolves exactly as
    /// under `Hex`.
    pub fn from_input_with(s: &str, base: FormIdBase) -> anyhow::Result<RecordSel> {
        let trimmed = s.trim();
        let has_hex_prefix = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .is_some();
        if !crate::looks_like_formid(s) {
            return Ok(RecordSel::Edid(s.to_string()));
        }
        if has_hex_prefix {
            return Ok(RecordSel::FormId(crate::parse_form_id_input(s)?));
        }
        if base == FormIdBase::Dec
            && let Some(fid) = bare_decimal_formid(s)
        {
            return Ok(RecordSel::FormId(fid));
        }
        Ok(RecordSel::Auto(s.to_string()))
    }

    /// Build a selector from explicit `--formid`/`--edid` inputs, falling back to
    /// auto-detecting a single ambiguous token (a positional CLI arg) via
    /// [`RecordSel::from_input`].
    /// Equivalent to [`RecordSel::from_parts_with`] under [`FormIdBase::Hex`].
    pub fn from_parts(
        formid: Option<&str>,
        edid: Option<&str>,
        target: Option<&str>,
    ) -> anyhow::Result<RecordSel> {
        Self::from_parts_with(formid, edid, target, FormIdBase::Hex)
    }

    /// [`RecordSel::from_parts`], base-aware — see [`RecordSel::from_input_with`]
    /// for what `base` changes. Applies to both the explicit `--formid` value
    /// and the bare positional `target` token.
    pub fn from_parts_with(
        formid: Option<&str>,
        edid: Option<&str>,
        target: Option<&str>,
        base: FormIdBase,
    ) -> anyhow::Result<RecordSel> {
        if let Some(fid) = formid {
            if base == FormIdBase::Dec
                && let Some(dec) = bare_decimal_formid(fid)
            {
                Ok(RecordSel::FormId(dec))
            } else {
                Ok(RecordSel::FormId(crate::parse_form_id_input(fid)?))
            }
        } else if let Some(e) = edid {
            Ok(RecordSel::Edid(e.to_string()))
        } else if let Some(t) = target {
            RecordSel::from_input_with(t, base)
        } else {
            bail!("specify a FormID/EditorID, or --formid/--edid")
        }
    }

    /// Render the selector for display/correlation purposes: a FormID hex
    /// string (`0x0000463F`) or the literal EditorID text. Used to tag each
    /// entry of a [`Op::RecordBulk`] result so callers can match a result back
    /// to the selector they requested, even on failure.
    pub fn display(&self) -> String {
        match self {
            RecordSel::FormId(fid) => fid.display(),
            RecordSel::Edid(edid) => edid.clone(),
            RecordSel::Auto(token) => token.clone(),
            RecordSel::EntryPoint(token) => token.clone(),
            RecordSel::OmodProperty(token) => token.clone(),
        }
    }
}

/// Resolve a [`RecordSel`] to a concrete [`FormId`], looking up the EditorID
/// index when needed. The one canonical selector-resolution used by every
/// serving surface (CLI, `esm batch`, N-API) — do not reimplement this locally.
pub fn resolve_sel(db: &Database, sel: &RecordSel) -> anyhow::Result<FormId> {
    match sel {
        RecordSel::FormId(fid) => Ok(*fid),
        RecordSel::Edid(edid) => {
            db.ensure_edid_index()?;
            // Real ESM records take precedence — only consult the
            // engine-hardcoded table (`crate::hardcoded`) once the real
            // index has already missed, per its own fallback-only contract.
            db.index
                .get_by_edid(edid)
                .or_else(|| crate::hardcoded::lookup_by_editor_id(edid))
                .ok_or_else(|| anyhow::anyhow!("EditorID '{}' not found", edid))
        }
        RecordSel::Auto(token) => {
            // Try the FormID interpretation first. Only fall back to an
            // EditorID lookup when that fails, so a real FormID never gets
            // silently redirected to an unrelated same-named EditorID.
            //
            // A bare all-digit token is always read as hex here (see
            // `parse_formid`) — never decimal. Decimal is available only via
            // an explicit `FormIdBase::Dec` at selector-construction time
            // (`RecordSel::from_input_with`/`from_parts_with`, the CLI's
            // `--decimal` flag), which commits directly to a concrete
            // `RecordSel::FormId` and never reaches this `Auto` arm at all.
            // Deliberately no implicit decimal fallback on a hex miss: see
            // `docs/adr/0010-formid-input-base.md`.
            let formid_attempt = crate::parse_form_id_input(token).ok();
            if let Some(fid) = formid_attempt
                && db.get_formid_meta(fid).is_ok()
            {
                return Ok(fid);
            }
            db.ensure_edid_index()?;
            if let Some(fid) = db.index.get_by_edid(token) {
                return Ok(fid);
            }
            // Both real-ESM attempts (FormID and EditorID) have now missed —
            // only then fall back to the hardcoded table, same
            // real-record-wins ordering as the `Edid` branch above.
            if let Some(fid) = crate::hardcoded::lookup_by_editor_id(token) {
                return Ok(fid);
            }
            match formid_attempt {
                Some(fid) => bail!(
                    "'{token}' did not resolve as FormID {} (not found) or as EditorID '{token}' (not found)",
                    fid.display()
                ),
                None => bail!("EditorID '{token}' not found"),
            }
        }
        RecordSel::EntryPoint(token) => bail!(
            "entry-point selector '{token}' is only valid for refs \
             (Op::ReferencedBy) — it doesn't resolve to a single record"
        ),
        RecordSel::OmodProperty(token) => bail!(
            "OMOD-property selector '{token}' is only valid for refs \
             (Op::ReferencedBy) — it doesn't resolve to a single record"
        ),
    }
}
