//! Integration tests for `esm::walk`. Mirrors `tests/chase.rs`'s `FakeFetcher`
//! pattern: `bulk_get` looks selectors up in a canned `records` map, `refs`
//! returns a canned `RefList` keyed by `(target, type_filter)`.

use esm::chase::ChaseFetcher;
use esm::ipc::RecordSel;
use esm::reader::RecordHeaderInfo;
use esm::walk::{WalkOptions, WalkResult, build_refs_digest, render_digest, render_text, walk};
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
        limit: usize,
        type_filter: &str,
        _paths: bool,
    ) -> anyhow::Result<RefList> {
        let key = (target.display(), type_filter.to_string());
        let mut list = self
            .refs_by_type
            .get(&key)
            .cloned()
            .unwrap_or_else(|| RefList {
                target: target.display(),
                rows: Vec::new(),
                total: 0,
                capped: false,
                ..Default::default()
            });
        // Simulate a real backend's `--ref-limit` truncation, so a test can
        // assert `WalkOptions::ref_limit`/`ChaseOptions::ref_limit` actually
        // bounds how many consumers get fetched (see
        // `omod_keyword_hook_consumer_fetch_bounded_by_ref_limit`).
        if limit > 0 && list.rows.len() > limit {
            list.rows.truncate(limit);
            list.capped = true;
        }
        Ok(list)
    }
}

fn sel(fid: &str) -> RecordSel {
    RecordSel::FormId(fid.parse().unwrap())
}

fn node_digest(result: &WalkResult, formid: &str) -> Vec<String> {
    let node = result
        .nodes
        .iter()
        .find(|n| n.formid == formid)
        .unwrap_or_else(|| panic!("node {formid} not visited; nodes = {:?}", result.nodes));
    render_digest(&node.digest)
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
    let result = walk(
        &mut f,
        sel(PERK_FID),
        &WalkOptions {
            depth: 1,
            ..WalkOptions::default()
        },
    )
    .unwrap();

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
    let result = walk(
        &mut f,
        sel(PERK_NO_EFFECTS_FID),
        &WalkOptions {
            depth: 1,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let lines = node_digest(&result, PERK_NO_EFFECTS_FID);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("NO effects — bonus is engine/script-side (description only)"))
    );
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
    let result = walk(
        &mut f,
        sel(SPEL_MAGIC_FID),
        &WalkOptions {
            depth: 1,
            ..WalkOptions::default()
        },
    )
    .unwrap();
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
                    "Effects[0].Conditions.Conditions[0].Parameter 1".to_string(),
                ]),
                ..Default::default()
            }],
            total: 1,
            capped: false,
            ..Default::default()
        },
    );
    // No fixture entry for (KYWD_FID, "PERK") -> FakeFetcher defaults to empty.

    let result = walk(
        &mut f,
        sel(KYWD_FID),
        &WalkOptions {
            depth: 1,
            ..WalkOptions::default()
        },
    )
    .unwrap();
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
    let result = walk(
        &mut f,
        sel(CHAIN_PERK_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    assert_eq!(result.nodes.len(), 1, "nodes = {:?}", result.nodes);
    assert_eq!(result.nodes[0].formid, CHAIN_PERK_FID);
}

#[test]
fn repeated_reference_is_visited_only_once() {
    let mut f = chain_fixture();
    let result = walk(
        &mut f,
        sel(CHAIN_PERK_FID),
        &WalkOptions {
            depth: 1,
            ..WalkOptions::default()
        },
    )
    .unwrap();
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
    assert!(
        digest.groups[0]
            .sample
            .iter()
            .any(|s| s == "co_Weapon_Test_NONPLAYABLE ⚠NONPLAYABLE")
    );

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
        ..Default::default()
    }];
    let result = WalkResult {
        not_found: None,
        nodes: Vec::new(),
        refs: Some(build_refs_digest(&rows)),
    };
    let text = render_text(&result);
    assert!(text.contains("QUST ×1: MQ000"));
    assert!(text.contains("[player-facing signal]"));
    assert!(
        text.contains(
            "Reminder: the record graph cannot distinguish shipped from UNRELEASED content"
        )
    );
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

// ─── OMOD direct ENCH attachment ────────────────────────────────────────────
//
// A directly-attached ENCH property renders through the classifier's normal
// `DirectProperty` path (`chase::FORWARD_FETCH_TYPES` includes ENCH) — same
// as a direct SPEL/PROJ attachment, not a separate ench-follow re-scan (see
// `omod_hops_enqueue`'s doc comment for why that pass was deleted).

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

    let result = walk(
        &mut f,
        sel(OMOD_FID),
        &WalkOptions {
            depth: 1,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, OMOD_FID).join("\n");
    assert!(text.contains("direct property → ENCH"));
    assert!(text.contains(ENCH_PROP_FID));
    assert!(text.contains("TestGrantedEnch"));

    let ench_node = result
        .nodes
        .iter()
        .find(|n| n.formid == ENCH_PROP_FID)
        .expect("ENCH property should have been enqueued and visited");
    assert_eq!(ench_node.via.as_deref(), Some("OMOD property"));
}

// ─── OMOD mechanism slice (chase classifier, inline) ───────────────────────

const OMOD_MIXED_FID: &str = "0x00600052";
const KYWD_HOOK_FID: &str = "0x00600053";
const OMOD_ENCH_ONLY_FID: &str = "0x00600054";
const GATING_PERK_FID: &str = "0x00600055";

/// An OMOD property row typed KYWD (a keyword-hook mechanism) should render a
/// `keyword hook →` line naming the KYWD, a `gates <consumer>` line naming
/// the reverse-chased SPEL/PERK that actually gates on it, and the exact
/// path-sliced `Effects[N]` row that consumer gates — never the consumer's
/// full digest — alongside the `direct property → ENCH` line the ENCH
/// property gets from the same classified-hops list (a directly-attached
/// ENCH is a plain `DirectProperty` forward attachment, same path as SPEL).
/// See `digest_node`'s `"OMOD"` arm.
#[test]
fn omod_mixed_property_renders_keyword_hook_slice() {
    let mut f = FakeFetcher::new();
    f.insert(
        OMOD_MIXED_FID,
        "OMOD",
        "mod_Legendary_Weapon1_Mixed",
        json!({
            "_record_type": "Object Modification",
            "Data": {
                "Properties": [
                    {
                        "Property": {"value": 19, "name": "Enchantments"},
                        "Value 1": {"formid": ENCH_PROP_FID, "editor_id": "TestGrantedEnch", "record_type": "ENCH"},
                        "Value 2": 0,
                    },
                    {
                        "Property": {"value": 31, "name": "Keywords"},
                        "Value 1": {"formid": KYWD_HOOK_FID, "editor_id": "TestKeywordHook", "record_type": "KYWD"},
                        "Value 2": 2,
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
    f.insert(
        GATING_PERK_FID,
        "PERK",
        "GatingPerkBACKUP",
        json!({
            "_record_type": "Perk",
            "Effects": [
                {
                    "Effect": {
                        "Entry Point": {
                            "Entry Point": {"value": 1, "name": "Set Damage on Consecutive Hits"},
                            "Function": {"value": 1, "name": "Set Value"},
                        },
                        "Float": 10,
                    }
                }
            ],
        }),
    );
    f.refs_by_type.insert(
        (KYWD_HOOK_FID.to_string(), "PERK".to_string()),
        RefList {
            target: KYWD_HOOK_FID.to_string(),
            rows: vec![RefRow {
                form_id: GATING_PERK_FID.to_string(),
                record_type: Some("PERK".to_string()),
                editor_id: Some("GatingPerkBACKUP".to_string()),
                name: None,
                offset: 0,
                depth: 1,
                path: Vec::new(),
                field_paths: Some(vec![
                    "Effects[0].Effect.Perk Conditions[0].Perk Condition.Conditions[0].Condition.Condition Data.Parameter 1"
                        .to_string(),
                ]),
                ..Default::default()
            }],
            total: 1,
            capped: false,
            ..Default::default()
        },
    );
    // No fixture entry for (KYWD_HOOK_FID, "SPEL") -> defaults to empty.

    // The mechanism slice runs regardless of `--depth` — depth 0 only caps
    // BFS enqueueing, not this inline classification.
    let result = walk(
        &mut f,
        sel(OMOD_MIXED_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, OMOD_MIXED_FID).join("\n");

    assert!(text.contains("direct property → ENCH"), "digest:\n{text}");
    assert!(
        text.contains(&format!(
            "keyword hook → KYWD {KYWD_HOOK_FID} TestKeywordHook"
        )),
        "digest:\n{text}"
    );
    assert!(
        text.contains(&format!("gates PERK {GATING_PERK_FID} GatingPerkBACKUP")),
        "digest:\n{text}"
    );
    assert!(
        text.contains("Effects[0] Set Damage on Consecutive Hits/Set Value  Float=10"),
        "expected the path-sliced gated effect row, got:\n{text}"
    );
}

/// An ENCH-only OMOD (every FormID property target is ENCH-typed) should
/// render its one `direct property → ENCH` line and no other mechanism-kind
/// lines — there is nothing else for the classifier to surface.
#[test]
fn omod_with_only_ench_properties_renders_no_other_mechanism_lines() {
    let mut f = FakeFetcher::new();
    f.insert(
        OMOD_ENCH_ONLY_FID,
        "OMOD",
        "mod_Legendary_Weapon1_EnchOnly",
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

    let result = walk(
        &mut f,
        sel(OMOD_ENCH_ONLY_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, OMOD_ENCH_ONLY_FID).join("\n");
    assert!(text.contains("direct property → ENCH"), "digest:\n{text}");
    for unexpected in ["keyword hook →", "perk grant →", "AV hook →", "gates "] {
        assert!(
            !text.contains(unexpected),
            "unexpected mechanism line {unexpected:?} in:\n{text}"
        );
    }
}

const OMOD_HUB_FID: &str = "0x00600058";
const KYWD_HUB_FID: &str = "0x00600059";

/// `ref_limit` (plumbed from `WalkOptions` through to
/// `esm::chase::ChaseOptions`) must bound how many reverse-chased consumers
/// the OMOD mechanism slice fetches/renders — mirrors the "hub AVIF/KYWD
/// blowup" gotcha in the esm-cli skill doc, where a widely-read
/// keyword/AVIF returns dozens of unrelated consumers.
#[test]
fn omod_keyword_hook_consumer_fetch_bounded_by_ref_limit() {
    let mut f = FakeFetcher::new();
    f.insert(
        OMOD_HUB_FID,
        "OMOD",
        "mod_Legendary_Hub_Test",
        json!({
            "_record_type": "Object Modification",
            "Data": {
                "Properties": [
                    {
                        "Property": {"value": 31, "name": "Keywords"},
                        "Value 1": {"formid": KYWD_HUB_FID, "editor_id": "HubKeyword", "record_type": "KYWD"},
                        "Value 2": 2,
                    }
                ]
            },
        }),
    );
    let mut rows = Vec::new();
    for i in 0..5 {
        let fid = format!("0x0060006{i}");
        f.insert(
            &fid,
            "PERK",
            &format!("HubConsumer{i}"),
            json!({
                "_record_type": "Perk",
                "Effects": [
                    {
                        "Effect": {
                            "Entry Point": {
                                "Entry Point": {"value": 1, "name": "SomeEntryPoint"},
                                "Function": {"value": 1, "name": "AddValue"},
                            },
                            "Float": i,
                        }
                    }
                ],
            }),
        );
        rows.push(RefRow {
            form_id: fid,
            record_type: Some("PERK".to_string()),
            editor_id: Some(format!("HubConsumer{i}")),
            name: None,
            offset: 0,
            depth: 1,
            path: Vec::new(),
            field_paths: Some(vec![
                "Effects[0].Effect.Perk Conditions[0].Perk Condition.Conditions[0].Condition.Condition Data.Parameter 1"
                    .to_string(),
            ]),
            ..Default::default()
        });
    }
    f.refs_by_type.insert(
        (KYWD_HUB_FID.to_string(), "PERK".to_string()),
        RefList {
            target: KYWD_HUB_FID.to_string(),
            rows,
            total: 5,
            capped: false,
            ..Default::default()
        },
    );

    let result = walk(
        &mut f,
        sel(OMOD_HUB_FID),
        &WalkOptions {
            depth: 0,
            ref_limit: 2,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, OMOD_HUB_FID).join("\n");
    let gates_count = text.matches("gates PERK").count();
    assert_eq!(
        gates_count, 2,
        "expected ref_limit=2 to bound the consumer fetch to 2 `gates` lines, got:\n{text}"
    );
}

const OMOD_SHELL_FID: &str = "0x0060005A";
const OMOD_PARENT_FID: &str = "0x0060005B";

/// A `_PARENT_*`-style empty-shell OMOD (no `Data.Properties[]` at all, its
/// real payload lives on the OMOD it `Data.Includes[]`) should render an
/// `include →` line pointing at the included OMOD — read straight off the
/// already-stub-resolved fields, no extra fetch.
#[test]
fn omod_includes_stub_renders_include_line() {
    let mut f = FakeFetcher::new();
    f.insert(
        OMOD_SHELL_FID,
        "OMOD",
        "_PARENT_Legendary_Weapon1_Shell",
        json!({
            "_record_type": "Object Modification",
            "Data": {
                "Includes": [
                    {
                        "Mod": {
                            "formid": OMOD_PARENT_FID,
                            "editor_id": "mod_Legendary_Weapon1_Parent",
                            "record_type": "OMOD",
                        },
                        "Minimum Level": 0,
                        "Optional": {"value": 0, "name": "False"},
                        "Don't Use All": {"value": 0, "name": "False"},
                    }
                ]
            },
        }),
    );

    let result = walk(
        &mut f,
        sel(OMOD_SHELL_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, OMOD_SHELL_FID).join("\n");
    assert!(
        text.contains(&format!(
            "include → OMOD {OMOD_PARENT_FID} mod_Legendary_Weapon1_Parent"
        )),
        "expected an include stub line, got:\n{text}"
    );
}

// ─── LVLI digest ────────────────────────────────────────────────────────────
//
// The selection/probability math itself is unit-tested exhaustively in
// `src/lvli.rs`'s own `#[cfg(test)]` module (pool/`Use All`/`Use First
// Match`, chance-none flat-vs-GLOB, curve evaluation, cycle guard, pool cap).
// These integration tests only cover the `walk()`/`digest_lvli` glue: that a
// walked LVLI root actually renders the table, that `WalkOptions::level`
// reaches `crate::lvli::drop_table`, and that a direct sublist entry gets
// enqueued as its own BFS node.

const LVLI_POOL_ROOT_FID: &str = "0x00700010";
const LVLI_SUBLIST_ROOT_FID: &str = "0x00700020";
const LVLI_SUBLIST_CHILD_FID: &str = "0x00700021";
const LVLI_CURVE_ROOT_FID: &str = "0x00700030";
const LVLI_FLAGS2_ROOT_FID: &str = "0x00700040";
const LVLI_LEGACY_ROOT_FID: &str = "0x00700050";
const LVLI_GATED_ROOT_FID: &str = "0x00700060";

fn lvli_leaf(formid: &str, rt: &str, edid: &str) -> serde_json::Value {
    json!({"formid": formid, "editor_id": edid, "record_type": rt})
}

fn lvli_entry_gated(target: serde_json::Value, operator: &str, cmp: f64) -> serde_json::Value {
    json!({"Leveled List Entry": {
        "Reference": target,
        "Chance None Value": 0.0,
        "Quantity": 1.0,
        "Minimum Level": 1.0,
        "Conditions": {"Conditions": [{"Condition": {"Condition Data": {
            "Function": "GetRandomPercent",
            "Operator": operator,
            "Comparison Value": cmp,
            "AND/OR": "AND",
            "Run On": "Subject",
        }}}]},
    }})
}

fn lvli_entry(target: serde_json::Value) -> serde_json::Value {
    json!({"Leveled List Entry": {
        "Reference": target,
        "Chance None Value": 0.0,
        "Quantity": 1.0,
        "Minimum Level": 1.0,
    }})
}

/// Bundles every LVLI digest scenario into one fetcher, mirroring
/// `perk_fixture`'s "one fixture per digest, many roots" shape.
fn lvli_fixture() -> FakeFetcher {
    let mut f = FakeFetcher::new();

    // Pool render — a descending GetRandomPercent >= N ladder plus an
    // unconditioned catch-all (the same shape as the real regression fixture
    // this feature was built to answer, `0x008308D7`).
    f.insert(
        LVLI_POOL_ROOT_FID,
        "LVLI",
        "TestPoolRoot",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Leveled List Entries": [
                lvli_entry_gated(
                    lvli_leaf("0x00700011", "ALCH", "RareSoup"),
                    "Greater Than Or Equal To",
                    92.0,
                ),
                lvli_entry(lvli_leaf("0x00700012", "ALCH", "CommonSoup")),
            ],
        }),
    );

    // Direct sublist entry → its own BFS node, on top of the aggregated
    // table already flattening through it.
    f.insert(
        LVLI_SUBLIST_CHILD_FID,
        "LVLI",
        "TestSublistChild",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Leveled List Entries": [lvli_entry(lvli_leaf("0x00700022", "WEAP", "NestedWeapon"))],
        }),
    );
    f.insert(
        LVLI_SUBLIST_ROOT_FID,
        "LVLI",
        "TestSublistRoot",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Leveled List Entries": [lvli_entry(lvli_leaf(
                LVLI_SUBLIST_CHILD_FID,
                "LVLI",
                "TestSublistChild",
            ))],
        }),
    );

    // Quantity Curve Table sibling — points climb 1 -> 5 over level 0 -> 100,
    // so `--level` should move the rendered expected-count row.
    f.insert(
        LVLI_CURVE_ROOT_FID,
        "LVLI",
        "TestCurveRoot",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Leveled List Entries": [{"Leveled List Entry": {
                "Reference": lvli_leaf("0x00700031", "MISC", "ScalingJunk"),
                "Chance None Value": 0.0,
                "Minimum Level": 0.0,
                "Quantity Curve Table": {
                    "formid": "0x00700032",
                    "editor_id": "TestQuantityCurve",
                    "curve_path": "test/quantity.json",
                    "curve": [{"x": 0.0, "y": 1.0}, {"x": 100.0, "y": 5.0}],
                },
            }}],
        }),
    );

    // XALG/LVLF "Flags" key collision — "Item Dispenser" is a real flag name
    // in both vocabularies, so only "Flags 2" (LVLF, present because XALG
    // took "Flags" first) may be trusted for the selection model.
    f.insert(
        LVLI_FLAGS2_ROOT_FID,
        "LVLI",
        "TestFlags2Root",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x10", "flags": ["Item Dispenser"]},
            "Flags 2": {"value": "0x4", "flags": ["Use All"]},
            "Leveled List Entries": [
                lvli_entry(lvli_leaf("0x00700041", "MISC", "AlwaysA")),
                lvli_entry(lvli_leaf("0x00700042", "MISC", "AlwaysB")),
            ],
        }),
    );

    // Legacy (form_version < 174) entry shape — `Base Data.{Level,Item,Count,
    // Chance None}` instead of the modern `Reference`/sibling fields.
    f.insert(
        LVLI_LEGACY_ROOT_FID,
        "LVLI",
        "TestLegacyRoot",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Leveled List Entries": [{"Leveled List Entry": {
                "Base Data": {
                    "Level": 5,
                    "Item": lvli_leaf("0x00700051", "MISC", "OldStyleItem"),
                    "Count": 2,
                    "Chance None": 20,
                },
            }}],
        }),
    );

    // A Condition gate that isn't `GetRandomPercent` — a real gate this
    // engine can't turn into a probability, so it must show up as a note
    // rather than silently reading as always-pass with no caveat.
    f.insert(
        LVLI_GATED_ROOT_FID,
        "LVLI",
        "TestGatedRoot",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Leveled List Entries": [{"Leveled List Entry": {
                "Reference": lvli_leaf("0x00700061", "BOOK", "RecipeReward"),
                "Chance None Value": 0.0,
                "Quantity": 1.0,
                "Minimum Level": 1.0,
                "Conditions": {"Conditions": [{"Condition": {"Condition Data": {
                    "Function": "HasLearnedRecipe",
                    "Operator": "Equal To",
                    "Comparison Value": 0.0,
                    "AND/OR": "AND",
                    "Run On": "Subject",
                }}}]},
            }}],
        }),
    );

    f
}

#[test]
fn lvli_pool_digest_renders_ranked_drop_table() {
    let mut f = lvli_fixture();
    let result = walk(
        &mut f,
        sel(LVLI_POOL_ROOT_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, LVLI_POOL_ROOT_FID).join("\n");
    assert!(
        text.contains("drop odds  model pool"),
        "expected a pool-model header, got:\n{text}"
    );
    // RareSoup's own gate passes 8% of the time, but pool selection splits
    // the "both pass" subset between the two entries too, so its actual odds
    // are 4% (0.92*1 + 0.08*0.5), not a naive 8% — CommonSoup should still
    // clearly outrank it.
    let rare_pos = text.find("RareSoup").expect("RareSoup row missing");
    let common_pos = text.find("CommonSoup").expect("CommonSoup row missing");
    assert!(
        common_pos < rare_pos,
        "expected CommonSoup ranked above RareSoup, got:\n{text}"
    );
    assert!(
        text.contains("4.00%"),
        "expected RareSoup's 4% pool odds, got:\n{text}"
    );
}

#[test]
fn lvli_direct_sublist_entry_is_enqueued_as_its_own_bfs_node() {
    let mut f = lvli_fixture();
    let result = walk(
        &mut f,
        sel(LVLI_SUBLIST_ROOT_FID),
        &WalkOptions {
            depth: 1,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    // The aggregated root table already flattens through to the leaf...
    let root_text = node_digest(&result, LVLI_SUBLIST_ROOT_FID).join("\n");
    assert!(
        root_text.contains("NestedWeapon"),
        "root's own table should already show the flattened leaf, got:\n{root_text}"
    );
    // ...but the intermediate sublist should still be independently visited.
    let child_text = node_digest(&result, LVLI_SUBLIST_CHILD_FID).join("\n");
    assert!(
        child_text.contains("NestedWeapon"),
        "sublist's own digest should render its own entry, got:\n{child_text}"
    );
}

#[test]
fn lvli_level_option_moves_a_curve_driven_quantity() {
    let mut f = lvli_fixture();
    let low = walk(
        &mut f,
        sel(LVLI_CURVE_ROOT_FID),
        &WalkOptions {
            depth: 0,
            level: 0.0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let mut f2 = lvli_fixture();
    let high = walk(
        &mut f2,
        sel(LVLI_CURVE_ROOT_FID),
        &WalkOptions {
            depth: 0,
            level: 100.0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let low_text = node_digest(&low, LVLI_CURVE_ROOT_FID).join("\n");
    let high_text = node_digest(&high, LVLI_CURVE_ROOT_FID).join("\n");
    assert!(
        low_text.contains("1.0000") && !low_text.contains("5.0000"),
        "level 0 should evaluate the curve to quantity 1, got:\n{low_text}"
    );
    assert!(
        high_text.contains("5.0000") && !high_text.contains("1.0000"),
        "level 100 should evaluate the curve to quantity 5, got:\n{high_text}"
    );
}

#[test]
fn lvli_flags_2_wins_selection_model_over_flags() {
    let mut f = lvli_fixture();
    let result = walk(
        &mut f,
        sel(LVLI_FLAGS2_ROOT_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, LVLI_FLAGS2_ROOT_FID).join("\n");
    assert!(
        text.contains("model Use All"),
        "XALG's own \"Item Dispenser\" flag under \"Flags\" must not be read \
         as LVLF's; \"Flags 2\" should win, got:\n{text}"
    );
}

#[test]
fn lvli_legacy_base_data_entry_renders() {
    let mut f = lvli_fixture();
    let result = walk(
        &mut f,
        sel(LVLI_LEGACY_ROOT_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, LVLI_LEGACY_ROOT_FID).join("\n");
    assert!(
        text.contains("OldStyleItem"),
        "pre-174 Base Data entry should still resolve to its Item, got:\n{text}"
    );
}

#[test]
fn lvli_non_get_random_percent_gate_is_noted() {
    let mut f = lvli_fixture();
    let result = walk(
        &mut f,
        sel(LVLI_GATED_ROOT_FID),
        &WalkOptions {
            depth: 0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, LVLI_GATED_ROOT_FID).join("\n");
    assert!(
        text.contains("RecipeReward") && text.contains("gated:HasLearnedRecipe"),
        "a non-GetRandomPercent gate must render as a caveat, not silently \
         assume-pass with no trace, got:\n{text}"
    );
}
