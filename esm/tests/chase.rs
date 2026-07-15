//! Integration tests for `esm::chase` — the native port of
//! `tools/chase/chase.py`, generalized to accept OMOD/PERK/SPEL/ALCH/ENCH
//! selectors. Mirrors `tools/tests/test_chase.py`'s fixture (same synthetic
//! FormIDs/fields) so the two can be eyeballed side by side during the parity
//! check, even though this crate can't literally share Python's `FakeGateway`.
//!
//! `FakeFetcher` below stands in for a `Backend`-driven fetcher: `bulk_get`
//! looks selectors up in a canned `records` map, `refs` returns a canned
//! `RefList` keyed by `(target, type_filter)` — the exact two calls
//! `chase()`'s reverse-chase makes (one per `CONSUMER_TYPES` entry).

use esm::chase::{chase, render_text, ChaseFetcher, ChaseOptions, EffectHopKind, HopKind};
use esm::ipc::RecordSel;
use esm::reader::RecordHeaderInfo;
use esm::{BulkRecordEntry, FormId, RefList, RefRow, ResolveDepth};
use serde_json::json;
use std::collections::HashMap;

const OMOD_FID: &str = "0x00500000";
const PERK_FID: &str = "0x00500020";
const KYWD_FID: &str = "0x00500010";
const SPEL_FID: &str = "0x00500030";
const WEAP_FID: &str = "0x00500099"; // non-chaseable selector, for the rejection test
const ENCH_FID: &str = "0x00500040";
const MGEF_FID: &str = "0x00500050";
const GRANTED_PERK_FID: &str = "0x00500051";
const PERK_ROOT_FID: &str = "0x00500060";
const ABILITY_SPEL_FID: &str = "0x00500061";
const SPEL_ROOT_FID: &str = "0x00500070";
const ALCH_ROOT_FID: &str = "0x00500080";

fn header(sig: &str, formid: &str) -> RecordHeaderInfo {
    RecordHeaderInfo {
        signature: sig.to_string(),
        form_id: formid.parse().unwrap(),
        flags: 0,
        form_version: 0,
        data_size: 0,
        offset: 0,
    }
}

fn ok_entry(
    sel: &str,
    header: RecordHeaderInfo,
    editor_id: &str,
    fields: serde_json::Value,
) -> BulkRecordEntry {
    BulkRecordEntry {
        sel: sel.to_string(),
        header: Some(header),
        editor_id: Some(editor_id.to_string()),
        fields: Some(fields),
        error: None,
    }
}

/// In-memory stand-in for a `Backend`-driven fetcher, mirroring the Python
/// prototype's `FakeGateway`. `refs_by_type` is keyed by `(target formid,
/// record-type filter)` since that's exactly what `chase()`'s reverse-chase
/// calls with (one `refs()` call per `CONSUMER_TYPES` entry) — no need to
/// reimplement `referenced_by_enriched`'s BFS/filter walk here, that's
/// already covered by `tests/ipc.rs`.
struct FakeFetcher {
    records: HashMap<String, BulkRecordEntry>,
    refs_by_type: HashMap<(String, String), RefList>,
}

impl ChaseFetcher for FakeFetcher {
    fn bulk_get(
        &mut self,
        sels: &[RecordSel],
        _depth: ResolveDepth,
    ) -> anyhow::Result<Vec<BulkRecordEntry>> {
        Ok(sels
            .iter()
            .map(|sel| {
                let display = sel.display();
                self.records
                    .get(&display)
                    .cloned()
                    .unwrap_or_else(|| BulkRecordEntry {
                        sel: display.clone(),
                        header: None,
                        editor_id: None,
                        fields: None,
                        error: Some(format!("not found: {display}")),
                    })
            })
            .collect())
    }

    fn refs(
        &mut self,
        target: FormId,
        _depth: usize,
        _limit: usize,
        type_filter: &str,
        _paths: bool,
    ) -> anyhow::Result<RefList> {
        let key = (target.display(), type_filter.to_string());
        Ok(self
            .refs_by_type
            .get(&key)
            .cloned()
            .unwrap_or_else(|| RefList {
                target: target.display(),
                rows: Vec::new(),
                total: 0,
                capped: false,
            }))
    }
}

/// Build the fixture described in `tools/tests/test_chase.py`'s `_fixture()`,
/// extended with:
/// - a 4th OMOD `Data.Properties[]` row (a direct ENCH attachment) that
///   exercises the MGEF pass-through fix on the existing OMOD forward-fetch
///   path (ENCH -> Base Effect -> MGEF -> "Perk to Apply", the exact
///   "Severing's confirmed chase" scenario from the mechanics KB);
/// - standalone PERK/SPEL/ALCH root fixtures for the new `effect_chase` walk.
fn fixture() -> FakeFetcher {
    let omod_fields = json!({
        "_record_type": "Object Modification",
        "Editor ID": "mod_Custom_Test",
        "Name": "Test Unique Mod",
        "Description": "Grants a unique effect.",
        "Data": {
            "Properties": [
                {
                    "Property": {"value": 1, "name": "SomeStat"},
                    "Function Type": {"value": 2, "name": "Multiply"},
                    "Value 1": 1.5,
                    "Value 2": 0,
                    "Curve Table": null,
                },
                {
                    "Property": {"value": 116, "name": "Perks"},
                    "Function Type": {"value": 2, "name": "ADD"},
                    "Value 1": {
                        "formid": PERK_FID,
                        "editor_id": "TestGrantedPerk",
                        "record_type": "PERK",
                    },
                    "Value 2": 0,
                    "Curve Table": null,
                },
                {
                    "Property": {"value": 31, "name": "Keywords"},
                    "Function Type": {"value": 2, "name": "ADD"},
                    "Value 1": {
                        "formid": KYWD_FID,
                        "editor_id": "if_tmp_TestTag",
                        "record_type": "KYWD",
                    },
                    "Value 2": 0,
                    "Curve Table": null,
                },
                {
                    "Property": {"value": 19, "name": "Enchantments"},
                    "Function Type": {"value": 2, "name": "ADD"},
                    "Value 1": {
                        "formid": ENCH_FID,
                        "editor_id": "TestGrantedEnch",
                        "record_type": "ENCH",
                    },
                    "Value 2": 0,
                    "Curve Table": null,
                },
            ]
        },
    });

    let perk_fields = json!({
        "_record_type": "Perk",
        "Description": "Grants bonus damage.",
        "Effects": [
            {
                "Effect": {
                    "Base Effect": {"formid": "0x00500021", "editor_id": "TestPerkEffect"},
                    "Effect Item Data": {"Magnitude": 10},
                }
            }
        ],
    });

    let spel_fields = json!({
        "_record_type": "Spell",
        "Effects": [
            {
                "Effect": {
                    "Base Effect": {"formid": "0x00500031", "editor_id": "TestSpellEffect"},
                    "Conditions": {
                        "Conditions": [
                            {
                                "Function": "WornHasKeyword",
                                "Operator": "EqualTo",
                                "Comparison Value": 1.0,
                                "Parameter 1": {"formid": KYWD_FID, "editor_id": "if_tmp_TestTag"},
                            }
                        ]
                    },
                    "Effect Item Data": {"Magnitude": 25},
                }
            }
        ],
    });

    let weap_fields = json!({"_record_type": "Weapon", "Editor ID": "NotAnOmod"});

    let ench_fields = json!({
        "_record_type": "Enchantment",
        "Editor ID": "TestGrantedEnch",
        "Description": "Adds a bonus effect.",
        "Effects": [
            {
                "Effect": {
                    "Base Effect": {
                        "formid": MGEF_FID,
                        "editor_id": "TestMgefWithPerk",
                        "record_type": "MGEF",
                    },
                    "Effect Item Data": {"Magnitude": 5},
                }
            }
        ],
    });

    let mgef_fields = json!({
        "_record_type": "Magic Effect",
        "Editor ID": "TestMgefWithPerk",
        "Magic Effect Data": {
            "Data": {
                "Perk to Apply": {
                    "formid": GRANTED_PERK_FID,
                    "editor_id": "GrantedPerkViaMgef",
                    "record_type": "PERK",
                },
                "Equip Ability": null,
            }
        },
    });

    let perk_root_fields = json!({
        "_record_type": "Perk",
        "Editor ID": "TestPerkRoot",
        "Name": "Test Perk Root",
        "Description": "A perk with an ability grant and a bare effect.",
        "Effects": [
            {
                "Effect": {
                    "Ability": {
                        "formid": ABILITY_SPEL_FID,
                        "editor_id": "TestAbilitySpel",
                        "record_type": "SPEL",
                    },
                }
            },
            {
                "Effect": {
                    "Entry Point": {
                        "Entry Point": {"value": 1, "name": "ModIncomingDamage"},
                        "Function": {"value": 1, "name": "AddValue"},
                    },
                    "Float": 0.1,
                }
            }
        ],
    });

    let ability_spel_fields = json!({
        "_record_type": "Spell",
        "Editor ID": "TestAbilitySpel",
        "Description": "Grants a temporary buff.",
    });

    let spel_root_fields = json!({
        "_record_type": "Spell",
        "Editor ID": "TestSpelRoot",
        "Effects": [
            {
                "Effect": {
                    "Base Effect": {
                        "formid": MGEF_FID,
                        "editor_id": "TestMgefWithPerk",
                        "record_type": "MGEF",
                    },
                    "Effect Item Data": {"Magnitude": 25},
                }
            }
        ],
    });

    let alch_root_fields = json!({
        "_record_type": "Ingestible",
        "Editor ID": "TestAlchRoot",
        "Effects": [
            {
                "Effect": {
                    "Base Effect": {
                        "formid": MGEF_FID,
                        "editor_id": "TestMgefWithPerk",
                        "record_type": "MGEF",
                    },
                    "Effect Item Data": {"Magnitude": 25},
                }
            }
        ],
    });

    let mut records = HashMap::new();
    records.insert(
        OMOD_FID.to_string(),
        ok_entry(
            OMOD_FID,
            header("OMOD", OMOD_FID),
            "mod_Custom_Test",
            omod_fields,
        ),
    );
    records.insert(
        PERK_FID.to_string(),
        ok_entry(
            PERK_FID,
            header("PERK", PERK_FID),
            "TestGrantedPerk",
            perk_fields,
        ),
    );
    records.insert(
        SPEL_FID.to_string(),
        ok_entry(
            SPEL_FID,
            header("SPEL", SPEL_FID),
            "TestGatedSpell",
            spel_fields,
        ),
    );
    records.insert(
        WEAP_FID.to_string(),
        ok_entry(WEAP_FID, header("WEAP", WEAP_FID), "NotAnOmod", weap_fields),
    );
    records.insert(
        ENCH_FID.to_string(),
        ok_entry(
            ENCH_FID,
            header("ENCH", ENCH_FID),
            "TestGrantedEnch",
            ench_fields,
        ),
    );
    records.insert(
        MGEF_FID.to_string(),
        ok_entry(
            MGEF_FID,
            header("MGEF", MGEF_FID),
            "TestMgefWithPerk",
            mgef_fields,
        ),
    );
    records.insert(
        PERK_ROOT_FID.to_string(),
        ok_entry(
            PERK_ROOT_FID,
            header("PERK", PERK_ROOT_FID),
            "TestPerkRoot",
            perk_root_fields,
        ),
    );
    records.insert(
        ABILITY_SPEL_FID.to_string(),
        ok_entry(
            ABILITY_SPEL_FID,
            header("SPEL", ABILITY_SPEL_FID),
            "TestAbilitySpel",
            ability_spel_fields,
        ),
    );
    records.insert(
        SPEL_ROOT_FID.to_string(),
        ok_entry(
            SPEL_ROOT_FID,
            header("SPEL", SPEL_ROOT_FID),
            "TestSpelRoot",
            spel_root_fields,
        ),
    );
    records.insert(
        ALCH_ROOT_FID.to_string(),
        ok_entry(
            ALCH_ROOT_FID,
            header("ALCH", ALCH_ROOT_FID),
            "TestAlchRoot",
            alch_root_fields,
        ),
    );

    let mut refs_by_type = HashMap::new();
    refs_by_type.insert(
        (KYWD_FID.to_string(), "SPEL".to_string()),
        RefList {
            target: KYWD_FID.to_string(),
            rows: vec![RefRow {
                form_id: SPEL_FID.to_string(),
                record_type: Some("SPEL".to_string()),
                editor_id: Some("TestGatedSpell".to_string()),
                name: None,
                offset: 0,
                depth: 1,
                path: Vec::new(),
                field_paths: Some(vec![
                    "Effects[0].Conditions.Conditions[0].Parameter 1".to_string()
                ]),
            }],
            total: 1,
            capped: false,
        },
    );
    // No fixture entry for (KYWD_FID, "PERK") -> defaults to an empty RefList.

    FakeFetcher {
        records,
        refs_by_type,
    }
}

fn sel(fid: &str) -> RecordSel {
    RecordSel::FormId(fid.parse().unwrap())
}

#[test]
fn omod_stub_fields_are_populated() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    assert_eq!(tree.root.formid.as_deref(), Some(OMOD_FID));
    assert_eq!(tree.root.record_type.as_deref(), Some("OMOD"));
    assert_eq!(tree.root.editor_id.as_deref(), Some("mod_Custom_Test"));
    assert_eq!(tree.root.name, Some(json!("Test Unique Mod")));
    assert_eq!(
        tree.root.description,
        Some(json!("Grants a unique effect."))
    );
}

#[test]
fn four_hops_classified_by_pattern() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    assert_eq!(tree.hops.len(), 4);
    assert_eq!(tree.hops[0].kind, HopKind::DirectProperty);
    assert_eq!(tree.hops[1].kind, HopKind::PerkGrant);
    assert_eq!(tree.hops[2].kind, HopKind::KeywordHook);
    assert_eq!(tree.hops[3].kind, HopKind::DirectProperty);
    assert!(tree.effect_hops.is_empty());
}

#[test]
fn direct_property_scalar_has_no_further_chase() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    let hop = &tree.hops[0];
    assert_eq!(hop.value1, json!(1.5));
    assert!(hop.target.is_none());
    assert!(hop.evidence.is_empty());
}

#[test]
fn perk_grant_forward_evidence_via_bulk_get() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    let hop = &tree.hops[1];
    let target = hop.target.as_ref().unwrap();
    assert_eq!(
        target.get("formid").and_then(|v| v.as_str()),
        Some(PERK_FID)
    );
    // Only the original forward evidence — this PERK's own Base Effect has no
    // record_type, so the MGEF pass-through naturally doesn't fire for it.
    assert_eq!(hop.evidence.len(), 1);
    let ev = &hop.evidence[0];
    assert!(ev.via.is_none());
    assert_eq!(
        ev.detail.get("description").and_then(|v| v.as_str()),
        Some("Grants bonus damage.")
    );
    assert_eq!(
        ev.detail
            .get("effects")
            .and_then(|v| v.as_array())
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn keyword_hook_reverse_evidence_slices_gated_effect() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    let hop = &tree.hops[2];
    let target = hop.target.as_ref().unwrap();
    assert_eq!(
        target.get("formid").and_then(|v| v.as_str()),
        Some(KYWD_FID)
    );
    assert_eq!(hop.evidence.len(), 1);
    let ev = &hop.evidence[0];
    assert_eq!(
        ev.source.get("formid").and_then(|v| v.as_str()),
        Some(SPEL_FID)
    );
    assert_eq!(
        ev.via.as_deref(),
        Some("Effects[0].Conditions.Conditions[0].Parameter 1")
    );
    // The sliced evidence is the whole gated Effects[0] entry, not the full
    // SPEL record — see `esm::chase`'s `slice_effect`.
    let effect = ev.detail.get("effect").unwrap();
    assert_eq!(
        effect
            .pointer("/Effect/Base Effect/editor_id")
            .and_then(|v| v.as_str()),
        Some("TestSpellEffect")
    );
}

#[test]
fn keyword_hook_with_no_matching_consumer_is_a_dead_end() {
    let mut f = fixture();
    f.refs_by_type
        .remove(&(KYWD_FID.to_string(), "SPEL".to_string()));
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    assert!(tree.hops[2].evidence.is_empty());
}

#[test]
fn omod_forward_fetch_follows_base_effect_to_mgef_perk_to_apply() {
    // The literal fix for the mechanics KB's "Severing's confirmed chase" gap:
    // OMOD -> (direct ENCH property) -> ENCH's Base Effect -> MGEF -> "Perk to
    // Apply" now surfaces automatically instead of needing a manual chase.
    let mut f = fixture();
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    let hop = &tree.hops[3];
    assert_eq!(hop.kind, HopKind::DirectProperty);
    assert_eq!(hop.evidence.len(), 2);
    assert_eq!(
        hop.evidence[0]
            .detail
            .get("description")
            .and_then(|v| v.as_str()),
        Some("Adds a bonus effect.")
    );
    let pass_through = &hop.evidence[1];
    assert_eq!(pass_through.via.as_deref(), Some("Base Effect"));
    assert_eq!(
        pass_through.source.get("formid").and_then(|v| v.as_str()),
        Some(MGEF_FID)
    );
    assert_eq!(
        pass_through.detail["perk_to_apply"]["formid"].as_str(),
        Some(GRANTED_PERK_FID)
    );
    assert!(pass_through.detail.get("equip_ability").is_none());
}

#[test]
fn perk_root_forward_fetches_ability_and_has_no_target_entry() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(PERK_ROOT_FID), &ChaseOptions::default()).unwrap();
    assert!(tree.hops.is_empty());
    assert_eq!(tree.effect_hops.len(), 2);

    let ability_hop = &tree.effect_hops[0];
    assert_eq!(ability_hop.kind, EffectHopKind::ForwardTarget);
    assert_eq!(
        ability_hop
            .target
            .as_ref()
            .unwrap()
            .get("formid")
            .and_then(|v| v.as_str()),
        Some(ABILITY_SPEL_FID)
    );
    assert_eq!(ability_hop.evidence.len(), 1);
    assert_eq!(
        ability_hop.evidence[0]
            .detail
            .get("description")
            .and_then(|v| v.as_str()),
        Some("Grants a temporary buff.")
    );

    let bare_hop = &tree.effect_hops[1];
    assert_eq!(bare_hop.kind, EffectHopKind::NoTarget);
    assert!(bare_hop.target.is_none());
    assert!(bare_hop.evidence.is_empty());
}

#[test]
fn spel_root_base_effect_follows_mgef_perk_to_apply() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(SPEL_ROOT_FID), &ChaseOptions::default()).unwrap();
    assert!(tree.hops.is_empty());
    assert_eq!(tree.effect_hops.len(), 1);
    let hop = &tree.effect_hops[0];
    assert_eq!(hop.kind, EffectHopKind::BaseEffect);
    assert_eq!(hop.evidence.len(), 1);
    assert_eq!(
        hop.evidence[0].detail["perk_to_apply"]["formid"].as_str(),
        Some(GRANTED_PERK_FID)
    );
}

#[test]
fn alch_root_dispatches_via_ingestible_record_type_and_follows_mgef() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(ALCH_ROOT_FID), &ChaseOptions::default()).unwrap();
    assert!(tree.hops.is_empty());
    assert_eq!(tree.effect_hops.len(), 1);
    assert_eq!(
        tree.effect_hops[0].evidence[0].detail["perk_to_apply"]["formid"].as_str(),
        Some(GRANTED_PERK_FID)
    );
}

#[test]
fn ench_root_dispatches_and_follows_mgef_pass_through() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(ENCH_FID), &ChaseOptions::default()).unwrap();
    assert!(tree.hops.is_empty());
    assert_eq!(tree.effect_hops.len(), 1);
    assert_eq!(tree.effect_hops[0].kind, EffectHopKind::BaseEffect);
    assert_eq!(
        tree.effect_hops[0].evidence[0].detail["perk_to_apply"]["formid"].as_str(),
        Some(GRANTED_PERK_FID)
    );
    assert_eq!(tree.root.record_type.as_deref(), Some("ENCH"));
}

#[test]
fn unsupported_selector_type_is_rejected() {
    let mut f = fixture();
    let err = chase(&mut f, sel(WEAP_FID), &ChaseOptions::default()).unwrap_err();
    assert!(
        err.to_string()
            .contains("chase supports OMOD, PERK, SPEL, ALCH, and ENCH"),
        "{err}"
    );
}

#[test]
fn unresolvable_selector_surfaces_as_an_error_not_a_panic() {
    let mut f = fixture();
    let err = chase(&mut f, sel("0xFFFFFFFF"), &ChaseOptions::default()).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

#[test]
fn omod_with_no_properties_has_empty_hops() {
    let mut f = fixture();
    let entry = f.records.get_mut(OMOD_FID).unwrap();
    entry
        .fields
        .as_mut()
        .unwrap()
        .pointer_mut("/Data")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("Properties".to_string(), json!([]));
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    assert!(tree.hops.is_empty());
    assert!(tree.effect_hops.is_empty());
}

#[test]
fn render_text_mentions_omod_and_hop_kinds() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    let text = render_text(&tree);
    assert!(text.starts_with("OMOD "));
    assert!(text.contains("mod_Custom_Test"));
    assert!(text.contains("perk_grant"));
    assert!(text.contains("keyword_hook"));
    assert!(text.contains("TestSpellEffect"));
}

#[test]
fn render_text_no_properties_message() {
    let mut f = fixture();
    let entry = f.records.get_mut(OMOD_FID).unwrap();
    entry
        .fields
        .as_mut()
        .unwrap()
        .pointer_mut("/Data")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("Properties".to_string(), json!([]));
    let tree = chase(&mut f, sel(OMOD_FID), &ChaseOptions::default()).unwrap();
    let text = render_text(&tree);
    assert!(text.contains("nothing to chase"));
}

#[test]
fn render_text_effect_root_shows_record_type_header_and_pass_through() {
    let mut f = fixture();
    let tree = chase(&mut f, sel(SPEL_ROOT_FID), &ChaseOptions::default()).unwrap();
    let text = render_text(&tree);
    assert!(text.starts_with("SPEL "));
    assert!(text.contains("base_effect"));
    assert!(text.contains("Perk to Apply:"));
}
