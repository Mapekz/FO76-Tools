//! Lookup table for xEdit's hardcoded-engine-form pseudo-plugin.
//!
//! A handful of low FormIDs (roughly `< 0x800`) are hardcoded into the FO76
//! game executable itself and never appear as a record in `SeventySix.esm` —
//! e.g. AVIF `KillStreak` at `0x00000399`, which shows up as an EPF3
//! actor-value reference on PERK effects. xEdit ships a pseudo-plugin
//! (`Core/Hardcoded/Fallout76.esp`) purely so it has something to resolve
//! these FormIDs against.
//!
//! `tools/extractor/hardcoded.py` parses that pseudo-plugin and emits
//! `schema/hardcoded_fo76.json`, embedded here at compile time (mirrors the
//! `Schema::load_embedded` pattern in `src/schema.rs` and the CTDA table in
//! `src/ctda.rs`). The table is small (~228 entries) and looked up rarely
//! (only on an index miss), so a sorted `Vec` + binary search is plenty.

use crate::formid::FormId;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
struct RawEntry {
    formid: String,
    #[serde(rename = "type")]
    record_type: String,
    editor_id: Option<String>,
    // `full` is present in the source JSON for a few entries (e.g. the S.P.E.C.I.A.L.
    // AVIFs) but isn't part of the public `HardcodedForm` shape — callers only need
    // record_type + editor_id to build a `FormIdStub`.
}

/// A hardcoded engine-defined form: its record type and EditorID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardcodedForm {
    pub record_type: String,
    pub editor_id: Option<String>,
}

struct HardcodedTable {
    /// Sorted by FormID ascending for binary search.
    entries: Vec<(u32, HardcodedForm)>,
    /// EditorID → FormID, the reverse of `entries`. Case-sensitive exact
    /// match, mirroring `Index::get_by_edid` (a plain `HashMap<String,
    /// _>::get(&str)` over raw EDID subrecord text) — a case-insensitive
    /// hardcoded side would let e.g. `esm get damagerecieved` resolve while
    /// `esm get abfortifydamagerecieved` still fails, which reads as a bug.
    /// `esm search` remains the case-insensitive surface.
    by_edid: HashMap<String, u32>,
}

static TABLE: OnceLock<HardcodedTable> = OnceLock::new();

fn table() -> &'static HardcodedTable {
    TABLE.get_or_init(|| {
        let raw: Vec<RawEntry> =
            serde_json::from_str(include_str!("../schema/hardcoded_fo76.json"))
                .expect("schema/hardcoded_fo76.json must be valid");
        let mut entries: Vec<(u32, HardcodedForm)> = raw
            .into_iter()
            .map(|e| {
                let hex = e
                    .formid
                    .strip_prefix("0x")
                    .or_else(|| e.formid.strip_prefix("0X"))
                    .unwrap_or(&e.formid);
                let id = u32::from_str_radix(hex, 16)
                    .expect("hardcoded_fo76.json formid must be 0x-prefixed hex");
                (
                    id,
                    HardcodedForm {
                        record_type: e.record_type,
                        editor_id: e.editor_id,
                    },
                )
            })
            .collect();
        entries.sort_by_key(|(id, _)| *id);
        // Built from the now-sorted `entries`, so a hypothetical duplicate
        // EditorID (none exist in the current table — see
        // `editor_ids_are_unique` below) resolves deterministically to the
        // lowest FormID rather than last-write-wins.
        let mut by_edid = HashMap::with_capacity(entries.len());
        for (id, form) in &entries {
            if let Some(edid) = &form.editor_id {
                by_edid.entry(edid.clone()).or_insert(*id);
            }
        }
        HardcodedTable { entries, by_edid }
    })
}

/// Look up a hardcoded engine form by FormID.
///
/// Returns `None` for the overwhelming majority of FormIDs — this table only
/// covers the ~228 forms xEdit's `Fallout76.esp` pseudo-plugin ships. Callers
/// should only consult this as a fallback after a real ESM index lookup misses.
pub fn lookup(id: FormId) -> Option<&'static HardcodedForm> {
    let t = table();
    t.entries
        .binary_search_by_key(&id.raw(), |(k, _)| *k)
        .ok()
        .map(|i| &t.entries[i].1)
}

/// Look up a hardcoded engine form's FormID by its EditorID (case-sensitive
/// exact match). The reverse of [`lookup`] — same fallback-only contract:
/// callers should only consult this after a real ESM EditorID lookup misses,
/// so a same-named real record always takes precedence.
pub fn lookup_by_editor_id(edid: &str) -> Option<FormId> {
    table().by_edid.get(edid).copied().map(FormId::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_streak_avif_resolves() {
        let form = lookup(FormId::new(0x0000_0399)).expect("0x399 should resolve");
        assert_eq!(form.record_type, "AVIF");
        assert_eq!(form.editor_id.as_deref(), Some("KillStreak"));
    }

    #[test]
    fn neighbouring_avifs_resolve() {
        let action_points_max = lookup(FormId::new(0x0000_0396)).expect("0x396 should resolve");
        assert_eq!(
            action_points_max.editor_id.as_deref(),
            Some("ActionPointsMax")
        );

        let fishing_bob = lookup(FormId::new(0x0000_039A)).expect("0x39A should resolve");
        assert_eq!(
            fishing_bob.editor_id.as_deref(),
            Some("Fishing_MaxBobModifier")
        );
    }

    #[test]
    fn missing_ids_return_none() {
        assert!(lookup(FormId::new(0x0092_4E31)).is_none());
        assert!(lookup(FormId::new(0xFFFF_FFFF)).is_none());
    }

    #[test]
    fn table_is_populated_with_expected_entry_count() {
        let t = table();
        assert!(
            t.entries.len() >= 220,
            "expected ~228 hardcoded entries, got {}",
            t.entries.len()
        );
    }

    #[test]
    fn lookup_by_editor_id_resolves_kill_streak() {
        let fid = lookup_by_editor_id("KillStreak").expect("KillStreak should resolve");
        assert_eq!(fid.raw(), 0x0000_0399);
    }

    #[test]
    fn lookup_by_editor_id_is_case_sensitive() {
        assert!(lookup_by_editor_id("killstreak").is_none());
        assert!(lookup_by_editor_id("KILLSTREAK").is_none());
    }

    #[test]
    fn lookup_by_editor_id_unknown_returns_none() {
        assert!(lookup_by_editor_id("NotARealHardcodedForm").is_none());
    }

    #[test]
    fn lookup_and_lookup_by_editor_id_agree() {
        let fid = lookup_by_editor_id("DamageRecieved").expect("DamageRecieved should resolve");
        let form = lookup(fid).expect("its FormID should look back up");
        assert_eq!(form.editor_id.as_deref(), Some("DamageRecieved"));
        assert_eq!(form.record_type, "AVIF");
    }

    /// Guards the assumption `by_edid`'s construction relies on: every entry
    /// has a distinct EditorID, so `.entry(..).or_insert(..)`'s
    /// lowest-FormID-wins tiebreak never actually triggers today. If
    /// `tools/extractor/hardcoded.py` regenerates the table with a collision,
    /// this catches it rather than letting `by_edid` silently drop an entry.
    #[test]
    fn editor_ids_are_unique() {
        let t = table();
        let named = t
            .entries
            .iter()
            .filter(|(_, form)| form.editor_id.is_some())
            .count();
        assert_eq!(
            t.by_edid.len(),
            named,
            "expected every hardcoded EditorID to be unique — by_edid dropped a collision"
        );
    }
}
