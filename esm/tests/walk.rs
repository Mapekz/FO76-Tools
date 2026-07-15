//! Integration tests for `esm::walk` — the native port of
//! `dps-76/scripts/esm-walk.ts`. Mirrors `tests/chase.rs`'s `FakeFetcher`
//! pattern: `bulk_get` looks selectors up in a canned `records` map, `refs`
//! returns a canned `RefList` keyed by `(target, type_filter)`.

use esm::chase::ChaseFetcher;
use esm::ipc::RecordSel;
use esm::reader::RecordHeaderInfo;
use esm::walk::{build_refs_digest, render_text, walk, WalkOptions, WalkResult};
use esm::{BulkRecordEntry, FormId, RefList, RefRow, ResolveDepth};
use serde_json::json;
use std::collections::HashMap;

struct FakeFetcher {
    records: HashMap<String, BulkRecordEntry>,
    refs_by_type: HashMap<(String, String), RefList>,
}

impl FakeFetcher {
    fn new() -> Self {
        Self {
            records: HashMap::new(),
            refs_by_type: HashMap::new(),
        }
    }

    fn insert(&mut self, formid: &str, sig: &str, edid: &str, fields: serde_json::Value) {
        self.records.insert(
            formid.to_string(),
            BulkRecordEntry {
                sel: formid.to_string(),
                header: Some(RecordHeaderInfo {
                    signature: sig.to_string(),
                    form_id: formid.parse().unwrap(),
                    flags: 0,
                    form_version: 0,
                    data_size: 0,
                    offset: 0,
                }),
                editor_id: Some(edid.to_string()),
                fields: Some(fields),
                error: None,
            },
        );
    }
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

fn sel(fid: &str) -> RecordSel {
    RecordSel::FormId(fid.parse().unwrap())
}

fn node_digest<'a>(result: &'a WalkResult, formid: &str) -> &'a [String] {
    &result
        .nodes
        .iter()
        .find(|n| n.formid == formid)
        .unwrap_or_else(|| panic!("node {formid} not visited; nodes = {:?}", result.nodes))
        .digest
}

// ─── PERK digest ────────────────────────────────────────────────────────────

const PERK_FID: &str = "0x00600010";
const ABILITY_SPEL_FID: &str = "0x00600011";
const PERK_NO_EFFECTS_FID: &str = "0x00600012";
const ENTRY_AV_FID: &str = "0x00600013";
const PERK_COND_GLOB_FID: &str = "0x00600014";

fn perk_fixture() -> FakeFetcher {
    let mut f = FakeFetcher::new();
    f.insert(
        PERK_FID,
        "PERK",
        "TestPerkRoot",
        json!({
            "_record_type": "Perk",
            "Description": "Grants bonus damage.",
            "Data": {"Num Ranks": 3, "Playable": {"value": 1, "name": "True"}},
            "Effects": [
                {
                    "Effect": {
                        "Effect Header": {"Effect Type": {"value": 0, "name": "Ability"}},
                        "Ability": {"formid": ABILITY_SPEL_FID, "editor_id": "TestAbilitySpel", "record_type": "SPEL"},
                    }
                },
                {
                    "Effect": {
                        "Effect Header": {"Effect Type": {"value": 1, "name": "Entry Point"}},
                        "Entry Point": {
                            "Entry Point": {"value": 1, "name": "ModIncomingDamage"},
                            "Function": {"value": 1, "name": "AddValue"},
                        },
                        "Float": 0.1,
                        "Function Parameter 3 (Actor Value)": {
                            "formid": ENTRY_AV_FID, "editor_id": "DamageResist", "record_type": "AVIF"
                        },
                        "Perk Conditions": [
                            {
                                "Perk Condition": {
                                    "Run On (Tab Index)": 0,
                                    "Conditions": [
                                        {
                                            "Condition": {
                                                "Condition Data": {
                                                    "Function": "GetValue",
                                                    "Operator": "Greater Than Or Equal To",
                                                    "Comparison Value": {
                                                        "formid": PERK_COND_GLOB_FID,
                                                        "editor_id": "LGND_Threshold",
                                                        "record_type": "GLOB",
                                                    },
                                                    "Parameter 1": null,
                                                    "Run On": "Subject",
                                                    "AND/OR": "AND",
                                                }
                                            }
                                        }
                                    ],
                                }
                            }
                        ],
                    }
                },
            ],
        }),
    );
    f.insert(
        ABILITY_SPEL_FID,
        "SPEL",
        "TestAbilitySpel",
        json!({"_record_type": "Spell", "Editor ID": "TestAbilitySpel"}),
    );
    f.insert(
        PERK_NO_EFFECTS_FID,
        "PERK",
        "TestPerkNoEffects",
        json!({"_record_type": "Perk", "Description": "Engine-side only."}),
    );
    f.insert(
        PERK_COND_GLOB_FID,
        "GLOB",
        "LGND_Threshold",
        json!({"_record_type": "Global", "Value": 40.0}),
    );
    f
}

#[test]
fn perk_digest_enqueues_ability_spel_and_renders_entry_point() {
    let mut f = perk_fixture();
    let result = walk(&mut f, sel(PERK_FID), &WalkOptions { depth: 1 }).unwrap();

    // The Ability effect's SPEL target was fetched and visited one hop out.
    assert!(
        result.nodes.iter().any(|n| n.formid == ABILITY_SPEL_FID),
        "Ability SPEL should have been enqueued and visited; nodes = {:?}",
        result.nodes
    );
    let ability_node = result
        .nodes
        .iter()
        .find(|n| n.formid == ABILITY_SPEL_FID)
        .unwrap();
    assert_eq!(ability_node.via.as_deref(), Some("Ability"));

    let perk_lines = node_digest(&result, PERK_FID);
    let text = perk_lines.join("\n");
    assert!(text.contains("description \"Grants bonus damage.\""));
    assert!(text.contains("ranks 3"));
    assert!(text.contains("effect[0] Ability → SPEL"));
    assert!(text.contains("TestAbilitySpel"));
    assert!(text.contains("effect[1] Entry Point \"ModIncomingDamage\""));
    assert!(text.contains("fn AddValue"));
    assert!(text.contains("value 0.1"));
    assert!(text.contains("AV"));
    assert!(text.contains("DamageResist"));
    // Perk Conditions' GLOB comparison value resolved inline.
    assert!(
        text.contains("LGND_Threshold=40"),
        "expected resolved GLOB annotation in: {text}"
    );
}

#[test]
fn perk_digest_no_effects_variant() {
    let mut f = perk_fixture();
    let result = walk(&mut f, sel(PERK_NO_EFFECTS_FID), &WalkOptions { depth: 1 }).unwrap();
    let lines = node_digest(&result, PERK_NO_EFFECTS_FID);
    assert!(lines
        .iter()
        .any(|l| l.contains("NO effects — bonus is engine/script-side (description only)")));
}

// ─── magic-item digest: GLOB flat-wins both ways ───────────────────────────

const SPEL_MAGIC_FID: &str = "0x00600020";
const GLOB_MAG_FID: &str = "0x00600021";

fn magic_item_fixture() -> FakeFetcher {
    let mut f = FakeFetcher::new();
    f.insert(
        SPEL_MAGIC_FID,
        "SPEL",
        "TestMagicSpel",
        json!({
            "_record_type": "Spell",
            "Effects": [
                {
                    "Effect": {
                        "Base Effect": {"formid": "0x00600099", "editor_id": "SomeMgef", "record_type": "MGEF"},
                        "Effect Item Data": {"Magnitude": 0, "Duration": 0},
                        "Magnitude": {"formid": GLOB_MAG_FID, "editor_id": "LGND_Survival_Scale", "record_type": "GLOB"},
                    }
                },
                {
                    "Effect": {
                        "Base Effect": {"formid": "0x00600099", "editor_id": "SomeMgef", "record_type": "MGEF"},
                        "Effect Item Data": {"Magnitude": 25, "Duration": 0},
                        "Magnitude": {"formid": GLOB_MAG_FID, "editor_id": "LGND_Survival_Scale", "record_type": "GLOB"},
                    }
                },
            ],
        }),
    );
    f.insert(
        GLOB_MAG_FID,
        "GLOB",
        "LGND_Survival_Scale",
        json!({"_record_type": "Global", "Value": 12.5}),
    );
    f
}

#[test]
fn magic_item_glob_magnitude_flat_wins_rule_both_ways() {
    let mut f = magic_item_fixture();
    let result = walk(&mut f, sel(SPEL_MAGIC_FID), &WalkOptions { depth: 1 }).unwrap();
    let text = node_digest(&result, SPEL_MAGIC_FID).join("\n");

    assert!(
        text.contains("magnitude GLOB LGND_Survival_Scale=12.5  ← real value (flat is 0)"),
        "expected flat-is-0 branch in: {text}"
    );
    assert!(
        text.contains(
            "sibling Magnitude GLOB LGND_Survival_Scale=12.5  ← IGNORE (flat wins; survival scale const)"
        ),
        "expected flat-wins branch in: {text}"
    );
}

// ─── KYWD reverse-chase ─────────────────────────────────────────────────────

const KYWD_FID: &str = "0x00600030";
const SPEL_CONSUMER_FID: &str = "0x00600031";

#[test]
fn kywd_digest_lists_spel_consumers_and_skips_empty_perk_group() {
    let mut f = FakeFetcher::new();
    f.insert(
        KYWD_FID,
        "KYWD",
        "if_tmp_TestTag",
        json!({"_record_type": "Keyword"}),
    );
    f.refs_by_type.insert(
        (KYWD_FID.to_string(), "SPEL".to_string()),
        RefList {
            target: KYWD_FID.to_string(),
            rows: vec![RefRow {
                form_id: SPEL_CONSUMER_FID.to_string(),
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
    // No fixture entry for (KYWD_FID, "PERK") -> FakeFetcher defaults to empty.

    let result = walk(&mut f, sel(KYWD_FID), &WalkOptions { depth: 1 }).unwrap();
    let text = node_digest(&result, KYWD_FID).join("\n");
    assert!(text.contains("SPEL consumers (gate on this):"));
    assert!(text.contains(SPEL_CONSUMER_FID));
    assert!(text.contains("TestGatedSpell"));
    assert!(text.contains("via Effects[0].Conditions.Conditions[0].Parameter 1"));
    assert!(
        !text.contains("PERK consumers"),
        "empty PERK consumer group should be skipped: {text}"
    );
}

// ─── depth capping + visited dedup ──────────────────────────────────────────

const CHAIN_PERK_FID: &str = "0x00600040";
const CHAIN_SPEL_FID: &str = "0x00600041";

fn chain_fixture() -> FakeFetcher {
    let mut f = FakeFetcher::new();
    // Two Ability effects pointing at the SAME SPEL — visited-set dedup means
    // only one node should ever be produced for it.
    f.insert(
        CHAIN_PERK_FID,
        "PERK",
        "TestChainPerk",
        json!({
            "_record_type": "Perk",
            "Effects": [
                {
                    "Effect": {
                        "Effect Header": {"Effect Type": {"value": 0, "name": "Ability"}},
                        "Ability": {"formid": CHAIN_SPEL_FID, "editor_id": "TestChainSpel", "record_type": "SPEL"},
                    }
                },
                {
                    "Effect": {
                        "Effect Header": {"Effect Type": {"value": 0, "name": "Ability"}},
                        "Ability": {"formid": CHAIN_SPEL_FID, "editor_id": "TestChainSpel", "record_type": "SPEL"},
                    }
                },
            ],
        }),
    );
    f.insert(
        CHAIN_SPEL_FID,
        "SPEL",
        "TestChainSpel",
        json!({"_record_type": "Spell", "Editor ID": "TestChainSpel"}),
    );
    f
}

#[test]
fn depth_zero_never_enqueues_children() {
    let mut f = chain_fixture();
    let result = walk(&mut f, sel(CHAIN_PERK_FID), &WalkOptions { depth: 0 }).unwrap();
    assert_eq!(result.nodes.len(), 1, "nodes = {:?}", result.nodes);
    assert_eq!(result.nodes[0].formid, CHAIN_PERK_FID);
}

#[test]
fn repeated_reference_is_visited_only_once() {
    let mut f = chain_fixture();
    let result = walk(&mut f, sel(CHAIN_PERK_FID), &WalkOptions { depth: 1 }).unwrap();
    let spel_nodes: Vec<_> = result
        .nodes
        .iter()
        .filter(|n| n.formid == CHAIN_SPEL_FID)
        .collect();
    assert_eq!(
        spel_nodes.len(),
        1,
        "the same SPEL referenced twice should only be visited once; nodes = {:?}",
        result.nodes
    );
    assert_eq!(result.nodes.len(), 2);
}

// ─── refs grouping ──────────────────────────────────────────────────────────

#[test]
fn build_refs_digest_groups_sorts_tags_and_flags_nonplayable() {
    let rows = vec![
        RefRow {
            form_id: "0x1".to_string(),
            record_type: Some("COBJ".to_string()),
            editor_id: Some("co_Weapon_Test".to_string()),
            name: None,
            offset: 0,
            depth: 1,
            path: Vec::new(),
            field_paths: None,
        },
        RefRow {
            form_id: "0x2".to_string(),
            record_type: Some("COBJ".to_string()),
            editor_id: Some("co_Weapon_Test_NONPLAYABLE".to_string()),
            name: None,
            offset: 0,
            depth: 1,
            path: Vec::new(),
            field_paths: None,
        },
        RefRow {
            form_id: "0x3".to_string(),
            record_type: Some("COBJ".to_string()),
            editor_id: Some("co_Weapon_Test2".to_string()),
            name: None,
            offset: 0,
            depth: 1,
            path: Vec::new(),
            field_paths: None,
        },
        RefRow {
            form_id: "0x4".to_string(),
            record_type: Some("LVLI".to_string()),
            editor_id: Some("LL_Test".to_string()),
            name: None,
            offset: 0,
            depth: 1,
            path: Vec::new(),
            field_paths: None,
        },
        RefRow {
            form_id: "0x5".to_string(),
            record_type: Some("NPC_".to_string()),
            editor_id: Some("SomeNpc".to_string()),
            name: None,
            offset: 0,
            depth: 1,
            path: Vec::new(),
            field_paths: None,
        },
    ];
    let digest = build_refs_digest(&rows);

    // Sorted by count desc: COBJ (3) before LVLI (1) / NPC_ (1).
    assert_eq!(digest.groups[0].record_type, "COBJ");
    assert_eq!(digest.groups[0].count, 3);
    assert_eq!(
        digest.groups[0].tag.as_deref(),
        Some("  [player-facing signal]")
    );
    assert!(digest.groups[0]
        .sample
        .iter()
        .any(|s| s == "co_Weapon_Test_NONPLAYABLE ⚠NONPLAYABLE"));

    let lvli = digest
        .groups
        .iter()
        .find(|g| g.record_type == "LVLI")
        .unwrap();
    assert_eq!(
        lvli.tag.as_deref(),
        Some("  [only player-facing LVLI chains count]")
    );

    let npc = digest
        .groups
        .iter()
        .find(|g| g.record_type == "NPC_")
        .unwrap();
    assert_eq!(npc.tag, None);
}

#[test]
fn build_refs_digest_empty_renders_no_reverse_references_message() {
    let digest = build_refs_digest(&[]);
    assert!(digest.groups.is_empty());

    let result = WalkResult {
        not_found: None,
        nodes: Vec::new(),
        refs: Some(digest),
    };
    let text = render_text(&result);
    assert!(text.contains("NO reverse references"));
    assert!(!text.contains("Reminder:"));
}

#[test]
fn render_text_refs_summary_ends_with_reminder_when_nonempty() {
    let rows = vec![RefRow {
        form_id: "0x1".to_string(),
        record_type: Some("QUST".to_string()),
        editor_id: Some("MQ000".to_string()),
        name: None,
        offset: 0,
        depth: 1,
        path: Vec::new(),
        field_paths: None,
    }];
    let result = WalkResult {
        not_found: None,
        nodes: Vec::new(),
        refs: Some(build_refs_digest(&rows)),
    };
    let text = render_text(&result);
    assert!(text.contains("QUST ×1: MQ000"));
    assert!(text.contains("[player-facing signal]"));
    assert!(text
        .contains("Reminder: the record graph cannot distinguish shipped from UNRELEASED content"));
}

// ─── not-found search fallback ──────────────────────────────────────────────

#[test]
fn walk_reports_not_found_with_empty_matches_for_unresolved_root() {
    let mut f = FakeFetcher::new();
    let result = walk(&mut f, sel("0x0069999A"), &WalkOptions::default()).unwrap();
    let nf = result
        .not_found
        .as_ref()
        .expect("expected not_found to be set");
    assert_eq!(nf.target, "0x0069999A");
    assert!(nf.matches.is_empty());
    assert!(result.nodes.is_empty());

    let text = render_text(&result);
    assert!(text.contains("not found by get."));
    assert!(text.contains("No search matches either."));
}

#[test]
fn render_text_shows_search_matches_when_present() {
    use esm::RecordRow;
    let result = WalkResult {
        not_found: Some(esm::walk::NotFound {
            target: "Psyco".to_string(),
            matches: vec![RecordRow {
                form_id: "0x00123456".to_string(),
                record_type: Some("ALCH".to_string()),
                editor_id: Some("Psycho".to_string()),
                name: Some("Psycho".to_string()),
                offset: 0,
            }],
        }),
        nodes: Vec::new(),
        refs: None,
    };
    let text = render_text(&result);
    assert!(text.contains("\"Psyco\" not found by get."));
    assert!(text.contains("Search matches:"));
    assert!(text.contains("0x00123456 ALCH Psycho Psycho"));
}

// ─── OMOD ENCH-follow ───────────────────────────────────────────────────────

const OMOD_FID: &str = "0x00600050";
const ENCH_PROP_FID: &str = "0x00600051";

#[test]
fn omod_follows_ench_property_and_enqueues_it() {
    let mut f = FakeFetcher::new();
    f.insert(
        OMOD_FID,
        "OMOD",
        "mod_Legendary_Weapon1_Test",
        json!({
            "_record_type": "Object Modification",
            "Data": {
                "Properties": [
                    {
                        "Property": {"value": 19, "name": "Enchantments"},
                        "Value 1": {"formid": ENCH_PROP_FID, "editor_id": "TestGrantedEnch", "record_type": "ENCH"},
                        "Value 2": 0,
                    }
                ]
            },
        }),
    );
    f.insert(
        ENCH_PROP_FID,
        "ENCH",
        "TestGrantedEnch",
        json!({"_record_type": "Enchantment", "Editor ID": "TestGrantedEnch"}),
    );

    let result = walk(&mut f, sel(OMOD_FID), &WalkOptions { depth: 1 }).unwrap();
    let text = node_digest(&result, OMOD_FID).join("\n");
    assert!(text.contains("enchantment →"));
    assert!(text.contains(ENCH_PROP_FID));
    assert!(text.contains("TestGrantedEnch"));

    let ench_node = result
        .nodes
        .iter()
        .find(|n| n.formid == ENCH_PROP_FID)
        .expect("ENCH property should have been enqueued and visited");
    assert_eq!(ench_node.via.as_deref(), Some("OMOD property"));
}
