//! Integration tests for `esm::walk`, over records and reverse references
//! held in a [`MemorySource`].

use esm::ops::RecordSel;
use esm::source::MemorySource;
use esm::walk::{
    Digest, RefsTag, WalkOptions, WalkResult, build_refs_digest, render_digest, render_text, walk,
};
use esm::{FormId, RefList, RefRow};
use serde_json::json;

/// Add a record with no header flags.
fn put(f: &mut MemorySource, formid: &str, sig: &str, edid: &str, fields: serde_json::Value) {
    f.insert(fid(formid), sig, edid, 0, fields);
}

fn fid(formid: &str) -> FormId {
    formid.parse().unwrap()
}

fn sel(formid: &str) -> RecordSel {
    RecordSel::FormId(fid(formid))
}

fn walk_at(f: &mut MemorySource, formid: &str, depth: usize) -> WalkResult {
    walk(
        f,
        sel(formid),
        &WalkOptions {
            depth: Some(depth),
            ..WalkOptions::default()
        },
    )
    .unwrap()
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

fn perk_fixture() -> MemorySource {
    let mut f = MemorySource::new();
    put(
        &mut f,
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
                                                    // Real `--resolve stub` output already
                                                    // inlines a GLOB reference's `Value` (see
                                                    // `src/decode/leaf_values.rs`); this fixture
                                                    // mirrors that shape directly.
                                                    "Comparison Value": {
                                                        "formid": PERK_COND_GLOB_FID,
                                                        "editor_id": "LGND_Threshold",
                                                        "record_type": "GLOB",
                                                        "Value": 40.0,
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
    put(
        &mut f,
        ABILITY_SPEL_FID,
        "SPEL",
        "TestAbilitySpel",
        json!({"_record_type": "Spell", "Editor ID": "TestAbilitySpel"}),
    );
    put(
        &mut f,
        PERK_NO_EFFECTS_FID,
        "PERK",
        "TestPerkNoEffects",
        json!({"_record_type": "Perk", "Description": "Engine-side only."}),
    );
    f
}

#[test]
fn perk_digest_enqueues_ability_spel_and_renders_entry_point() {
    let mut f = perk_fixture();
    let result = walk_at(&mut f, PERK_FID, 1);

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
    let result = walk_at(&mut f, PERK_NO_EFFECTS_FID, 1);
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

fn magic_item_fixture() -> MemorySource {
    let mut f = MemorySource::new();
    put(
        &mut f,
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
                        // Real `--resolve stub` output already inlines a GLOB
                        // reference's `Value` (see `src/decode/leaf_values.rs`);
                        // this fixture mirrors that shape directly rather than
                        // relying on a separate fetch of GLOB_MAG_FID.
                        "Magnitude": {"formid": GLOB_MAG_FID, "editor_id": "LGND_Survival_Scale", "record_type": "GLOB", "Value": 12.5},
                    }
                },
                {
                    "Effect": {
                        "Base Effect": {"formid": "0x00600099", "editor_id": "SomeMgef", "record_type": "MGEF"},
                        "Effect Item Data": {"Magnitude": 25, "Duration": 0},
                        "Magnitude": {"formid": GLOB_MAG_FID, "editor_id": "LGND_Survival_Scale", "record_type": "GLOB", "Value": 12.5},
                    }
                },
            ],
        }),
    );
    f
}

#[test]
fn magic_item_glob_magnitude_flat_wins_rule_both_ways() {
    let mut f = magic_item_fixture();
    let result = walk_at(&mut f, SPEL_MAGIC_FID, 1);
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

const LOOT_BAG_ALCH_FID: &str = "0x00600022";
const LOOT_BAG_MGEF_FID: &str = "0x00600023";
const LOOT_BAG_LVLI_FID: &str = "0x00600024";

/// A loot-bag consumable: its effect's MGEF names the LVLI it hands out only
/// through a Papyrus script property, never a record field.
fn loot_bag_fixture() -> MemorySource {
    let mut f = MemorySource::new();
    let lvli = json!({"formid": LOOT_BAG_LVLI_FID, "editor_id": "BagLoot", "record_type": "LVLI"});
    put(
        &mut f,
        LOOT_BAG_ALCH_FID,
        "ALCH",
        "LootBag",
        json!({
            "Effects": [{
                "Effect": {
                    "Base Effect": {"formid": LOOT_BAG_MGEF_FID, "editor_id": "LootBagEffect", "record_type": "MGEF"},
                    "Effect Item Data": {"Magnitude": 0, "Duration": 0},
                }
            }],
        }),
    );
    put(
        &mut f,
        LOOT_BAG_MGEF_FID,
        "MGEF",
        "LootBagEffect",
        json!({
            "Virtual Machine Adapter": {"version": 6, "scripts": [
                {"name": "Creatures:FestiveGiftAddItem", "status": 0, "properties": [
                    {"name": "FestiveLeveledList", "type": 1, "value": lvli},
                ]},
                {"name": "FXaddItemOnEffectScript", "status": 0, "properties": [
                    {"name": "fChanceToSpawn", "type": 4, "value": 70.0},
                    {"name": "leveledItemsToAdd", "type": 11, "value": [lvli]},
                    {"name": "NotALoot", "type": 1, "value": {"formid": "0x00600025", "editor_id": "Kw", "record_type": "KYWD"}},
                ]},
            ]},
            "Magic Effect Data": {"Data": {"Archetype": {"value": 1, "name": "Script"}}},
        }),
    );
    put(
        &mut f,
        LOOT_BAG_LVLI_FID,
        "LVLI",
        "BagLoot",
        json!({
            "Leveled List Entries": [lvli_entry(lvli_leaf("0x00600026", "ALCH", "BagCandy"))],
        }),
    );
    f
}

#[test]
fn magic_item_follows_base_effect_script_leveled_list() {
    let mut f = loot_bag_fixture();
    let result = walk_at(&mut f, LOOT_BAG_ALCH_FID, 1);
    let text = node_digest(&result, LOOT_BAG_ALCH_FID).join("\n");
    assert!(
        text.contains(&format!(
            "script Creatures:FestiveGiftAddItem.FestiveLeveledList → LVLI {LOOT_BAG_LVLI_FID} BagLoot"
        )),
        "expected the script loot line in:\n{text}"
    );
    assert!(
        text.contains("script FXaddItemOnEffectScript.leveledItemsToAdd → LVLI"),
        "expected the array-property loot line in:\n{text}"
    );
    assert!(
        !text.contains("NotALoot"),
        "non-LVLI properties stay out:\n{text}"
    );

    let lvli = result
        .nodes
        .iter()
        .find(|n| n.formid == LOOT_BAG_LVLI_FID)
        .expect("the script's LVLI is walked one hop from the item");
    assert_eq!(lvli.depth, 1);
    assert_eq!(
        lvli.via.as_deref(),
        Some("script Creatures:FestiveGiftAddItem.FestiveLeveledList")
    );
    assert!(
        node_digest(&result, LOOT_BAG_LVLI_FID)
            .join("\n")
            .contains("BagCandy")
    );
}

#[test]
fn mgef_root_follows_script_leveled_list() {
    let mut f = loot_bag_fixture();
    let result = walk_at(&mut f, LOOT_BAG_MGEF_FID, 1);
    assert!(result.nodes.iter().any(|n| n.formid == LOOT_BAG_LVLI_FID));
    let text = node_digest(&result, LOOT_BAG_MGEF_FID).join("\n");
    assert!(text.contains("script Creatures:FestiveGiftAddItem.FestiveLeveledList → LVLI"));
}

// ─── KYWD reverse-chase ─────────────────────────────────────────────────────

const KYWD_FID: &str = "0x00600030";
const SPEL_CONSUMER_FID: &str = "0x00600031";

#[test]
fn kywd_digest_lists_spel_consumers_and_skips_empty_perk_group() {
    let mut f = MemorySource::new();
    put(
        &mut f,
        KYWD_FID,
        "KYWD",
        "if_tmp_TestTag",
        json!({"_record_type": "Keyword"}),
    );
    f.insert_refs(
        fid(KYWD_FID),
        "SPEL",
        RefList {
            target: KYWD_FID.to_string(),
            rows: vec![RefRow {
                form_id: SPEL_CONSUMER_FID.to_string(),
                id: SPEL_CONSUMER_FID.parse().unwrap(),
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
    // No fixture entry for (KYWD_FID, "PERK") -> MemorySource defaults to empty.

    let result = walk_at(&mut f, KYWD_FID, 1);
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

fn chain_fixture() -> MemorySource {
    let mut f = MemorySource::new();
    // Two Ability effects pointing at the SAME SPEL — visited-set dedup means
    // only one node should ever be produced for it.
    put(
        &mut f,
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
    put(
        &mut f,
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
    let result = walk_at(&mut f, CHAIN_PERK_FID, 0);
    assert_eq!(result.nodes.len(), 1, "nodes = {:?}", result.nodes);
    assert_eq!(result.nodes[0].formid, CHAIN_PERK_FID);
}

#[test]
fn repeated_reference_is_visited_only_once() {
    let mut f = chain_fixture();
    let result = walk_at(&mut f, CHAIN_PERK_FID, 1);
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
    assert_eq!(digest.groups[0].tag, Some(RefsTag::PlayerFacing));
    assert!(
        digest.groups[0]
            .sample
            .iter()
            .any(|s| s.editor_id == "co_Weapon_Test_NONPLAYABLE" && s.nonplayable)
    );

    let lvli = digest
        .groups
        .iter()
        .find(|g| g.record_type == "LVLI")
        .unwrap();
    assert_eq!(lvli.tag, Some(RefsTag::LeveledList));

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
    let mut f = MemorySource::new();
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
    let mut f = MemorySource::new();
    put(
        &mut f,
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
    put(
        &mut f,
        ENCH_PROP_FID,
        "ENCH",
        "TestGrantedEnch",
        json!({"_record_type": "Enchantment", "Editor ID": "TestGrantedEnch"}),
    );

    let result = walk_at(&mut f, OMOD_FID, 1);
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
    let mut f = MemorySource::new();
    put(
        &mut f,
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
    put(
        &mut f,
        ENCH_PROP_FID,
        "ENCH",
        "TestGrantedEnch",
        json!({"_record_type": "Enchantment", "Editor ID": "TestGrantedEnch"}),
    );
    put(
        &mut f,
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
    f.insert_refs(
        fid(KYWD_HOOK_FID),
        "PERK",
        RefList {
            target: KYWD_HOOK_FID.to_string(),
            rows: vec![RefRow {
                form_id: GATING_PERK_FID.to_string(),
                id: GATING_PERK_FID.parse().unwrap(),
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
    let result = walk_at(&mut f, OMOD_MIXED_FID, 0);
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
    let mut f = MemorySource::new();
    put(
        &mut f,
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
    put(
        &mut f,
        ENCH_PROP_FID,
        "ENCH",
        "TestGrantedEnch",
        json!({"_record_type": "Enchantment", "Editor ID": "TestGrantedEnch"}),
    );

    let result = walk_at(&mut f, OMOD_ENCH_ONLY_FID, 0);
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
    let mut f = MemorySource::new();
    put(
        &mut f,
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
        put(
            &mut f,
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
            id: fid.parse().unwrap(),
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
    f.insert_refs(
        fid(KYWD_HUB_FID),
        "PERK",
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
            depth: Some(0),
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
const OMOD_COLLECTION_FID: &str = "0x0060005C";
const OMOD_ALT_FID: &str = "0x0060005D";

fn include_row(formid: &str, edid: &str, minimum_level: u64) -> serde_json::Value {
    json!({
        "Mod": {"formid": formid, "editor_id": edid, "record_type": "OMOD"},
        "Minimum Level": minimum_level,
        "Optional": {"value": 0, "name": "False"},
        "Don't Use All": {"value": 1, "name": "True"},
    })
}

fn weight_property(value: f64) -> serde_json::Value {
    json!({
        "Value Type": {"value": 1, "name": "Float"},
        "Function Type": {"value": 1, "name": "MUL+ADD"},
        "Property": {"value": 0, "name": "Weight"},
        "Value 1": value,
        "Value 2": 0.0,
    })
}

/// A plain OMOD's include is a mod template: its properties are the
/// includer's, rendered under the template's name, and the template isn't
/// walked as a node of its own.
#[test]
fn omod_template_include_renders_its_properties_under_the_template() {
    let mut f = MemorySource::new();
    put(
        &mut f,
        OMOD_SHELL_FID,
        "OMOD",
        "mod_Legendary_Weapon1_Shell",
        json!({
            "_record_type": "Object Modification",
            "Data": {"Includes": [include_row(OMOD_PARENT_FID, "_PARENT_mod_Weight", 0)]},
        }),
    );
    f.insert(
        fid(OMOD_PARENT_FID),
        "OMOD",
        "_PARENT_mod_Weight",
        0x100, // Mod Template
        json!({
            "_record_type": "Object Modification",
            "Data": {"Properties": [weight_property(0.25)]},
        }),
    );

    let result = walk_at(&mut f, OMOD_SHELL_FID, 1);
    let text = node_digest(&result, OMOD_SHELL_FID).join("\n");
    assert!(
        text.contains(&format!("from OMOD {OMOD_PARENT_FID} _PARENT_mod_Weight")),
        "expected the template's name, got:\n{text}"
    );
    assert!(text.contains("Weight") && text.contains("0.25"), "{text}");
    assert!(
        result.nodes.iter().all(|n| n.formid != OMOD_PARENT_FID),
        "a template is part of its includer, not a walked node"
    );
}

/// A template's ENCH property is the includer's own, so the ENCH is walked
/// (OMOD -> ENCH -> MGEF chains through `_PARENT_*` blocks land in one walk),
/// and a template with no properties still shows as included.
#[test]
fn omod_template_ench_is_walked_and_empty_templates_are_listed() {
    const ENCH_FID: &str = "0x0060005E";
    const EMPTY_FID: &str = "0x0060005F";
    let mut f = MemorySource::new();
    put(
        &mut f,
        OMOD_SHELL_FID,
        "OMOD",
        "mod_Test_Barbed",
        json!({
            "_record_type": "Object Modification",
            "Data": {"Includes": [
                include_row(OMOD_PARENT_FID, "_PARENT_mod_Barbed", 0),
                include_row(EMPTY_FID, "_PARENT_mod_Empty", 0),
            ]},
        }),
    );
    let ench =
        json!({"formid": ENCH_FID, "editor_id": "enchModArmorPenetration", "record_type": "ENCH"});
    f.insert(
        fid(OMOD_PARENT_FID),
        "OMOD",
        "_PARENT_mod_Barbed",
        0x100, // Mod Template
        json!({
            "_record_type": "Object Modification",
            "Data": {"Properties": [{
                "Value Type": {"value": 4, "name": "FormID,Int"},
                "Function Type": {"value": 2, "name": "ADD"},
                "Property": {"value": 1, "name": "Enchantments"},
                "Value 1": ench,
                "Value 2": 1,
            }]},
        }),
    );
    f.insert(
        fid(EMPTY_FID),
        "OMOD",
        "_PARENT_mod_Empty",
        0x100,
        json!({"_record_type": "Object Modification", "Data": {}}),
    );
    put(
        &mut f,
        ENCH_FID,
        "ENCH",
        "enchModArmorPenetration",
        json!({"_record_type": "Object Effect", "Effects": []}),
    );

    let result = walk_at(&mut f, OMOD_SHELL_FID, 1);
    assert!(
        result.nodes.iter().any(|n| n.formid == ENCH_FID),
        "the template's ENCH must be walked; nodes = {:?}",
        result.nodes.iter().map(|n| &n.formid).collect::<Vec<_>>()
    );
    let text = node_digest(&result, OMOD_SHELL_FID).join("\n");
    assert!(
        text.contains(&format!(
            "from OMOD {EMPTY_FID} _PARENT_mod_Empty  (no properties)"
        )),
        "{text}"
    );
}

/// A Mod Collection's includes are alternatives: listed with their minimum
/// level and walked as nodes, never merged into the collection.
#[test]
fn omod_collection_walks_its_alternatives() {
    let mut f = MemorySource::new();
    f.insert(
        fid(OMOD_COLLECTION_FID),
        "OMOD",
        "modcol_Test_Barrels",
        0x80, // Mod Collection
        json!({
            "_record_type": "Object Modification",
            "Data": {"Includes": [include_row(OMOD_ALT_FID, "mod_Test_Barrel_Long", 20)]},
        }),
    );
    put(
        &mut f,
        OMOD_ALT_FID,
        "OMOD",
        "mod_Test_Barrel_Long",
        json!({
            "_record_type": "Object Modification",
            "Data": {"Properties": [weight_property(0.5)]},
        }),
    );

    let result = walk_at(&mut f, OMOD_COLLECTION_FID, 1);
    let text = node_digest(&result, OMOD_COLLECTION_FID).join("\n");
    assert_eq!(
        text,
        format!("alternative → OMOD {OMOD_ALT_FID} mod_Test_Barrel_Long  (level 20+)")
    );
    let alt = node_digest(&result, OMOD_ALT_FID).join("\n");
    assert!(alt.contains("Weight") && alt.contains("0.5"), "{alt}");
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
fn lvli_fixture() -> MemorySource {
    let mut f = MemorySource::new();

    // Pool render — a descending GetRandomPercent >= N ladder plus an
    // unconditioned catch-all (the same shape as the real regression fixture
    // this feature was built to answer, `0x008308D7`).
    put(
        &mut f,
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

    // A sublist whose list-wide note covers two items, under a root that
    // also carries a direct leaf.
    put(
        &mut f,
        LVLI_SUBLIST_CHILD_FID,
        "LVLI",
        "TestSublistChild",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Max Count": 2,
            "Leveled List Entries": [
                lvli_entry(lvli_leaf("0x00700022", "WEAP", "NestedWeapon")),
                lvli_entry(lvli_leaf("0x00700023", "WEAP", "NestedPistol")),
            ],
        }),
    );
    put(
        &mut f,
        LVLI_SUBLIST_ROOT_FID,
        "LVLI",
        "TestSublistRoot",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x4", "flags": ["Use All"]},
            "Leveled List Entries": [
                lvli_entry(lvli_leaf(LVLI_SUBLIST_CHILD_FID, "LVLI", "TestSublistChild")),
                lvli_entry(lvli_leaf("0x00700024", "ALCH", "DirectStimpak")),
            ],
        }),
    );

    // Quantity Curve Table sibling — points climb 1 -> 5 over level 0 -> 100,
    // so `--level` should move the rendered expected-count row.
    put(
        &mut f,
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
    put(
        &mut f,
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
    put(
        &mut f,
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
    put(
        &mut f,
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
    let result = walk_at(&mut f, LVLI_POOL_ROOT_FID, 0);
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
fn lvli_depth_bounds_the_drop_tree_like_du() {
    let mut f = lvli_fixture();

    // Depth 1: the sublist is one subtotal row, and never its own BFS node.
    let result = walk_at(&mut f, LVLI_SUBLIST_ROOT_FID, 1);
    assert_eq!(result.nodes.len(), 1, "sublists stay inside the tree");
    let text = node_digest(&result, LVLI_SUBLIST_ROOT_FID).join("\n");
    assert!(
        text.contains("▸ LVLI 0x00700021 TestSublistChild  (pool, 2 items)"),
        "expected a collapsed subtotal row, got:\n{text}"
    );
    assert!(
        text.contains("DirectStimpak"),
        "direct leaf listed, got:\n{text}"
    );
    assert!(
        !text.contains("NestedWeapon"),
        "depth 1 hides nested items, got:\n{text}"
    );

    // Depth 2: the sublist expands in place, its items indented under it.
    let result = walk_at(&mut f, LVLI_SUBLIST_ROOT_FID, 2);
    assert_eq!(result.nodes.len(), 1);
    let lines = node_digest(&result, LVLI_SUBLIST_ROOT_FID);
    let parent = lines
        .iter()
        .position(|l| l.contains("▾ LVLI 0x00700021 TestSublistChild"))
        .unwrap_or_else(|| panic!("expected an expanded sublist row in {lines:#?}"));
    let child = &lines[parent + 1];
    assert!(child.contains("NestedWeapon") || child.contains("NestedPistol"));
    let indent = |l: &str| l.len() - l.trim_start().len();
    assert!(indent(child) > indent(&lines[parent]), "{lines:#?}");
}

#[test]
fn lvli_list_wide_note_prints_once_as_a_footnote() {
    let mut f = lvli_fixture();
    let result = walk_at(&mut f, LVLI_SUBLIST_CHILD_FID, 0);
    let text = node_digest(&result, LVLI_SUBLIST_CHILD_FID).join("\n");
    assert_eq!(
        text.matches("Max Count present on this list").count(),
        1,
        "got:\n{text}"
    );
    assert!(
        text.contains("list notes 1"),
        "header carries the tag, got:\n{text}"
    );
    assert!(
        text.contains("[1] Max Count present on this list"),
        "got:\n{text}"
    );
}

#[test]
fn lvli_tree_subtotal_matches_its_flat_rows() {
    let mut f = lvli_fixture();
    let result = walk_at(&mut f, LVLI_SUBLIST_ROOT_FID, 2);
    let Digest::Lvli(d) = &result.nodes[0].digest else {
        panic!("expected an LVLI digest");
    };
    let entries = d.table.tree.as_ref().unwrap().entries.as_ref().unwrap();
    let sub = entries
        .iter()
        .find(|b| b.formid == LVLI_SUBLIST_CHILD_FID)
        .unwrap();
    let nested: f64 = ["0x00700022", "0x00700023"]
        .iter()
        .map(|fid| {
            d.table
                .rows
                .iter()
                .find(|r| r.formid == *fid)
                .unwrap()
                .expected_count
        })
        .sum();
    assert!((sub.expected_count - nested).abs() < 1e-9);
    assert!((sub.p_at_least_one - 1.0).abs() < 1e-9);
    assert_eq!(d.table.rows.len(), 3, "flat rows still recurse fully");
}

#[test]
fn lvli_root_defaults_to_depth_one() {
    let mut f = lvli_fixture();
    let result = walk(&mut f, sel(LVLI_SUBLIST_ROOT_FID), &WalkOptions::default()).unwrap();
    let text = node_digest(&result, LVLI_SUBLIST_ROOT_FID).join("\n");
    assert!(text.contains("▸ LVLI 0x00700021"), "got:\n{text}");
    assert!(!text.contains("NestedWeapon"), "got:\n{text}");
}

#[test]
fn lvli_level_option_moves_a_curve_driven_quantity() {
    let mut f = lvli_fixture();
    let low = walk(
        &mut f,
        sel(LVLI_CURVE_ROOT_FID),
        &WalkOptions {
            depth: Some(0),
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
            depth: Some(0),
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
    let result = walk_at(&mut f, LVLI_FLAGS2_ROOT_FID, 0);
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
    let result = walk_at(&mut f, LVLI_LEGACY_ROOT_FID, 0);
    let text = node_digest(&result, LVLI_LEGACY_ROOT_FID).join("\n");
    assert!(
        text.contains("OldStyleItem"),
        "pre-174 Base Data entry should still resolve to its Item, got:\n{text}"
    );
}

#[test]
fn lvli_non_get_random_percent_gate_is_noted() {
    let mut f = lvli_fixture();
    let result = walk_at(&mut f, LVLI_GATED_ROOT_FID, 0);
    let text = node_digest(&result, LVLI_GATED_ROOT_FID).join("\n");
    assert!(
        text.contains("RecipeReward")
            && text.contains("[1] condition HasLearnedRecipe can't be computed"),
        "a non-GetRandomPercent gate must render as a caveat, not silently \
         assume-pass with no trace, got:\n{text}"
    );
}

// ─── level-keyed curve evaluation (`crate::walk::level_curves`) ──────────

const WEAP_CURVE_FID: &str = "0x00600050";
const NPC_PROPS_FID: &str = "0x00600060";
const ARMO_RESIST_FID: &str = "0x00600070";
const ENCH_GUARD_FID: &str = "0x00600080";
const LVLI_MINLEVEL_ROOT_FID: &str = "0x00600090";

/// A WEAP root's `Damage Curve` is evaluated using [`WalkOptions::default`]'s
/// own level constant — no `--level` flag passed — proving the feature fires
/// without the reader having to opt in.
#[test]
fn weap_damage_curve_evaluates_at_default_level_with_no_level_flag() {
    let mut f = MemorySource::new();
    put(
        &mut f,
        WEAP_CURVE_FID,
        "WEAP",
        "TestCurveWeapon",
        json!({
            "Damage Curve": {
                "formid": "0x00600051",
                "editor_id": "CT_TestDamage",
                "curve_path": "test/damage.json",
                "curve": [{"x": 1.0, "y": 10.0}, {"x": 100.0, "y": 100.0}],
            },
        }),
    );
    let result = walk(&mut f, sel(WEAP_CURVE_FID), &WalkOptions::default()).unwrap();
    let text = node_digest(&result, WEAP_CURVE_FID).join("\n");
    assert!(
        text.contains("curves @ level 50:"),
        "expected the default level (50) echoed in the curve block, got:\n{text}"
    );
    assert!(
        text.contains("damage: 54.5") && text.contains("[CT_TestDamage]"),
        "expected the Damage Curve row evaluated (not just present), got:\n{text}"
    );
}

/// NPC_ `Properties[]` rows are labeled by their sibling `Actor Value`'s
/// editor_id, one row per array element.
#[test]
fn npc_properties_curve_rows_labeled_by_actor_value() {
    let mut f = MemorySource::new();
    put(
        &mut f,
        NPC_PROPS_FID,
        "NPC_",
        "TestCreatureNpc",
        json!({
            "Properties": [
                {
                    "Actor Value": {"formid": "0x1", "editor_id": "Health", "record_type": "AVIF"},
                    "Value": 0.0,
                    "Curve Table": {"curve": [{"x": 1.0, "y": 100.0}, {"x": 50.0, "y": 500.0}]},
                },
                {
                    "Actor Value": {"formid": "0x2", "editor_id": "DamageResist", "record_type": "AVIF"},
                    "Value": 0.0,
                    "Curve Table": {"curve": [{"x": 1.0, "y": 10.0}, {"x": 50.0, "y": 40.0}]},
                },
            ],
        }),
    );
    let result = walk(
        &mut f,
        sel(NPC_PROPS_FID),
        &WalkOptions {
            depth: Some(0),
            level: 50.0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, NPC_PROPS_FID).join("\n");
    assert!(
        text.contains("Health: 500"),
        "expected the Health property row labeled by Actor Value and evaluated, got:\n{text}"
    );
    assert!(
        text.contains("DamageResist: 40"),
        "expected the DamageResist property row labeled by Actor Value and evaluated, got:\n{text}"
    );
    // The generic field tree must still be present too — NPC_ keeps the
    // generic field tree alongside its own digest arm.
    assert!(
        text.contains("\"Properties\""),
        "expected the generic field tree to still render Properties, got:\n{text}"
    );
}

/// ARMO `Resistances[]` rows are labeled by their sibling `Type` (a DMGT
/// stub), NOT `Actor Value` — the row-shape difference from NPC_/RACE
/// `Properties[]` that a copy-pasted `label_from` would silently get wrong.
#[test]
fn armo_resistances_labeled_by_type_not_actor_value() {
    let mut f = MemorySource::new();
    put(
        &mut f,
        ARMO_RESIST_FID,
        "ARMO",
        "TestArmorPiece",
        json!({
            "Resistances": [
                {
                    "Type": {"formid": "0x1", "editor_id": "dtEnergy", "record_type": "DMGT"},
                    "Amount": 0,
                    "Curve Table": {"curve": [{"x": 1.0, "y": 20.0}, {"x": 50.0, "y": 80.0}]},
                },
            ],
        }),
    );
    let result = walk(
        &mut f,
        sel(ARMO_RESIST_FID),
        &WalkOptions {
            depth: Some(0),
            level: 50.0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, ARMO_RESIST_FID).join("\n");
    assert!(
        text.contains("dtEnergy: 80"),
        "expected the resistance row labeled by Type (dtEnergy), got:\n{text}"
    );
}

/// ENCH `Effects[]` curve guard: an effect whose sibling `Actor Value` is
/// absent is evaluated; one with a named `Actor Value` (e.g. a legendary-mod
/// tier gate) is not — the axis note appears instead of a confidently wrong
/// number.
#[test]
fn ench_effect_curve_guard_evaluates_only_when_actor_value_absent() {
    let mut f = MemorySource::new();
    put(
        &mut f,
        ENCH_GUARD_FID,
        "ENCH",
        "TestGuardEnch",
        json!({
            "Effects": [
                {"Effect": {
                    "Base Effect": {"formid": "0x00600081", "editor_id": "SomeMgef", "record_type": "MGEF"},
                    "Effect Item Data": {"Magnitude": 0, "Duration": 0},
                    "Curve Table": {"editor_id": "CT_LevelDomained", "curve": [{"x": 1.0, "y": 10.0}, {"x": 50.0, "y": 100.0}]},
                }},
                {"Effect": {
                    "Base Effect": {"formid": "0x00600081", "editor_id": "SomeMgef", "record_type": "MGEF"},
                    "Effect Item Data": {"Magnitude": 0, "Duration": 0},
                    "Actor Value": {"formid": "0x00600082", "editor_id": "Perception", "record_type": "AVIF"},
                    "Curve Table": {"editor_id": "CT_CapsCurve", "curve": [{"x": 0.0, "y": 1.0}, {"x": 40000.0, "y": 5.0}]},
                }},
            ],
        }),
    );
    let result = walk(
        &mut f,
        sel(ENCH_GUARD_FID),
        &WalkOptions {
            depth: Some(0),
            level: 50.0,
            ..WalkOptions::default()
        },
    )
    .unwrap();
    let text = node_digest(&result, ENCH_GUARD_FID).join("\n");
    assert_eq!(
        text.matches("curve @ walk level:").count(),
        1,
        "expected exactly one evaluated curve line (effect[0] only), got:\n{text}"
    );
    assert!(
        text.contains("curve @ walk level: 100"),
        "expected effect[0]'s level-domained curve evaluated to 100 at level 50, got:\n{text}"
    );
    assert!(
        text.contains("curve INPUT axis: AV") && text.contains("Perception"),
        "expected effect[1]'s named-AV axis note instead of an evaluated number, got:\n{text}"
    );
}

/// COBJ's `Curve Table` is count-keyed (evaluated elsewhere by
/// the derived component `Quantity`) and must never appear in this
/// allowlist.
#[test]
fn cobj_curve_table_not_in_level_curves_allowlist() {
    let fields = json!({
        "Components": [{"Count": 3, "Curve Table": {"curve": [{"x": 1.0, "y": 1.0}, {"x": 5.0, "y": 5.0}]}}],
    });
    let rows = esm::walk::level_curves::eval_level_curves(
        "COBJ",
        &esm::Resolved::from_stub_json(&fields),
        50.0,
    );
    assert!(
        rows.is_empty(),
        "COBJ has no LEVEL_KEYED_CURVES rows — its Curve Table is count-keyed"
    );
}

/// LVLI's `Minimim Level Curve Table` (schema typo, preserved verbatim) is
/// tier-indexed, not level — it must stay absent from this allowlist, and
/// without its tier Global the drop table notes it rather than evaluating
/// it at the level.
#[test]
fn lvli_minimim_level_curve_table_without_a_global_is_noted_not_evaluated() {
    let fields_direct = json!({
        "Minimim Level Curve Table": {"curve": [{"x": 0.0, "y": 1.0}, {"x": 3.0, "y": 4.0}]},
    });
    assert!(
        esm::walk::level_curves::eval_level_curves(
            "LVLI",
            &esm::Resolved::from_stub_json(&fields_direct),
            50.0
        )
        .is_empty(),
        "LVLI has no LEVEL_KEYED_CURVES rows at all"
    );

    let mut f = MemorySource::new();
    put(
        &mut f,
        LVLI_MINLEVEL_ROOT_FID,
        "LVLI",
        "TestMinLevelCurveRoot",
        json!({
            "_record_type": "Leveled Item",
            "Flags": {"value": "0x0", "flags": []},
            "Leveled List Entries": [{"Leveled List Entry": {
                "Reference": lvli_leaf("0x00700099", "MISC", "SomeJunk"),
                "Chance None Value": 0.0,
                "Quantity": 1.0,
                "Minimim Level Curve Table": {
                    "formid": "0x00700098",
                    "editor_id": "TestMinLevelCurve",
                    "curve": [{"x": 0.0, "y": 1.0}, {"x": 3.0, "y": 4.0}],
                },
            }}],
        }),
    );
    let result = walk_at(&mut f, LVLI_MINLEVEL_ROOT_FID, 0);
    let text = node_digest(&result, LVLI_MINLEVEL_ROOT_FID).join("\n");
    assert!(
        text.contains("has no Global to read its tier from"),
        "expected the unresolved-input note, not an evaluated number, got:\n{text}"
    );
}

#[test]
fn lvli_with_no_eligible_entries_still_prints_its_list_footnotes() {
    let mut f = MemorySource::new();
    let mut entry = lvli_entry(lvli_leaf("0x00700091", "WEAP", "HighLevelGun"));
    entry["Leveled List Entry"]["Minimum Level"] = json!(100.0);
    put(
        &mut f,
        "0x00700090",
        "LVLI",
        "TooHighForLevel50",
        json!({"Flags": {"value": "0x0", "flags": []}, "Max Count": 1, "Leveled List Entries": [entry]}),
    );
    let result = walk_at(&mut f, "0x00700090", 0);
    let text = node_digest(&result, "0x00700090").join("\n");
    assert!(
        text.contains("(no eligible entries at this level)"),
        "got:\n{text}"
    );
    assert!(
        text.contains("list notes 1") && text.contains("[1] Max Count"),
        "got:\n{text}"
    );
}
