//! Progression and reward records: GMRW, PCRD, RESO.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{
    assert_fully_decoded, assert_record_type, bare_ctx, bare_ctx_fv, decode_fixture,
    subrecords_from,
};
use esm::decode::decode_record;
use esm::schema::Schema;

/// GMRW 0x008B2016 — `WorldPets_Reward_PetLevelling_Generic_CUR_Goldbullions01`.
///
/// form_version 209.  XALG is absent from the TES5Edit GMRW definition; it is
/// mapped in `fo76.overrides.json` as a raw `bytes` field (reverse-engineered).
#[test]
fn gmrw_world_pets_reward_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x008B2016 --raw` (form_version 209).
    let result = decode_fixture(
        "GMRW",
        209,
        &[
            (
                "EDID",
                "576f726c64506574735f5265776172645f5065744c6576656c6c696e675f47656e657269635f4355525f476f6c6462756c6c696f6e73303100",
            ),
            ("XALG", "0010000000000000"),
            ("RWDS", "01000000"),
            ("ESRE", "00000000"),
            ("QRCO", "c8c15500"),
            ("NAM8", "ad1f8b00"),
            ("QRLR", "01000000"),
            ("ITME", ""),
        ],
    );

    assert_fully_decoded(&result);
}

/// RESO 0x008B53B3 — `ATX_Resource_TeaWizard_resource`.
///
/// NAM5 (1 byte) is absent from the TES5Edit RESO definition; mapped in
/// `fo76.overrides.json` as a u8 integer (reverse-engineered).
#[test]
fn reso_tea_wizard_resource_decodes_correctly() {
    let result = decode_fixture(
        "RESO",
        209,
        &[
            (
                "EDID",
                "4154585f5265736f757263655f54656157697a6172645f7265736f7572636500",
            ),
            ("NAM1", "47548b00"),
            ("NAM2", "fe538b00"),
            ("NAM4", "01548b00"),
            ("NAM5", "01"),
        ],
    );

    assert_record_type(&result, "Resource");
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("ATX_Resource_TeaWizard_resource"),
    );
}
/// PCRD `DATA` decodes race eligibility, and the struct is named "Perk Card Data"
/// (not "Unknown" — xEdit's placeholder name, carried over verbatim by the
/// extractor before an override renamed it). Byte 6 is the
/// `Race Restriction` field: 0/1/2 = None/Human/Ghoul.
///
/// Regression test for the reported bug where legendary perk cards
/// (`GHL_LGN_ActionDiet_Card`, `LGN_WhatRads_Card`, `GHL_LGN_FeralRageCard`) —
/// and ordinary cards like `NaturalResistanceCard` / `GHL_RadioactiveStrengthCard` —
/// showed their race-restriction data under a key literally called "Unknown".
#[test]
fn pcrd_data_decodes_race_restriction() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx(&schema);

    // 7-byte DATA payload: Value(f32=0.0) | Min Level(u8=0x32=50) |
    // Special(u8=0x02=Endurance) | Race Restriction(u8, varies).
    for (hex, race) in [
        ("00000000320200", "None"),
        ("00000000320201", "Human"),
        ("00000000320202", "Ghoul"),
    ] {
        let subs = subrecords_from(&[("DATA", hex)]);
        let result = decode_record(&ctx, "PCRD", &subs);

        assert_record_type(&result, "Perk Card");
        assert_fully_decoded(&result);

        // The DATA struct carries its schema name, not a placeholder key.
        assert!(
            result.get("Unknown").is_none(),
            "PCRD DATA struct must not be keyed 'Unknown'"
        );
        let data = result
            .get("Perk Card Data")
            .expect("PCRD DATA must be keyed 'Perk Card Data'");

        assert_eq!(data["Min Level"].as_u64(), Some(50), "Min Level");
        assert_eq!(
            data["Special"]["name"].as_str(),
            Some("Endurance"),
            "Special"
        );
        assert_eq!(
            data["Race Restriction"]["name"].as_str(),
            Some(race),
            "race byte {hex} should decode to Race Restriction = {race}",
        );
    }
}

/// GMRW `0x006311D8` shape: a "Rewards List" rarray of "Reward" rstructs whose
/// optional leading `CTRG` is absent on every element and whose last member is
/// the `ITME` "Reward End Marker". Each reward's condition parameter strings
/// (`CIS2`) stay with that reward.
///
/// Two sparse rewards here: each carries only `NAM7` (to tell them apart) and
/// one condition (`CTDA` + `CIS2`), then its own `ITME`.
#[test]
fn gmrw_sparse_rewards_partitioned_by_shared_end_marker_terminator() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    // Verbatim CTDA bytes reused from other tests in this file (a real,
    // already-known-good 34-byte CTDA payload) — its exact condition
    // function/values are irrelevant here, only that it decodes without a
    // raw fallback.
    let ctda = "000000000000803f30020000b5f36700000000000000000000000000ffffffff";

    let subs = subrecords_from(&[
        ("EDID", "476d72774d696e5465737400"), // "GmrwMinTest\0"
        ("RWDS", "02000000"),                 // Rewards Count = 2
        // Reward 0: no CTRG. NAM7 distinguishes it from Reward 1.
        ("NAM7", "34120000"), // GLOB FormID = 0x00001234
        ("CITC", "01000000"), // Condition Count = 1
        ("CTDA", ctda),
        ("CIS2", "6361707352657761726452616e6b00"), // "capsRewardRank\0"
        ("ITME", ""),                               // Reward End Marker
        // Reward 1: no CTRG. NAM7 differs; CIS2 differs.
        ("NAM7", "78560000"), // GLOB FormID = 0x00005678
        ("CITC", "01000000"),
        ("CTDA", ctda),
        ("CIS2", "746f6b656e7352657761726452616e6b00"), // "tokensRewardRank\0"
        ("ITME", ""),
    ]);

    let result = decode_record(&ctx, "GMRW", &subs);
    assert_record_type(&result, "Gameplay Reward");
    assert_fully_decoded(&result);

    let rewards = result
        .get("Rewards List")
        .and_then(|v| v.as_array())
        .expect("Rewards List must decode");
    assert_eq!(
        rewards.len(),
        2,
        "expected exactly 2 rewards, no fragmentation"
    );

    for (idx, expected_param) in [(0, "capsRewardRank"), (1, "tokensRewardRank")] {
        let reward = &rewards[idx]["Reward"];
        let conditions = reward
            .pointer("/Conditions/Conditions")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("reward {idx} must have its own Conditions"));
        assert_eq!(
            conditions.len(),
            1,
            "reward {idx} must have exactly 1 condition"
        );
        assert_eq!(
            conditions[0]
                .pointer("/Condition/Parameter #2")
                .and_then(|v| v.as_str()),
            Some(expected_param),
            "reward {idx}'s CIS2 must stay with its own reward, not bleed into the other"
        );
    }
}
