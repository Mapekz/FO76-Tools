//! PERK records, excluding the stat-scaling goldens in [`super::perks_stat`].
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{assert_fully_decoded, assert_record_type, decode_fixture};

/// PERK 0x00004168 — `TestTamePerk` — decodes to Perk fully (includes VMAD).
///
/// 17-subrecord record; form_version 175.  Carries a VMAD header (version 6,
/// object_format 2, 0 scripts) as well as PRKE/PRKC/CTDA/EPF2/EPF3/PRKF perk
/// entry blocks.  VMAD must decode without a "VMAD truncated" raw_fallback;
/// this test locks the clean path.
///
/// Also locks the `EPF2`/`EPF3` union-by-`Function Type` decode: this effect
/// has `Function Type` = 4 ("Activate Choice"), so `EPF2` must decode as the
/// inline `<ID=...>`-prefixed lstring "Button Label" (not raw hex), and `EPF3`
/// as the u16 "Script Flags" flags field (not a dropped/short FormID).
#[test]
fn perk_test_tame_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x00004168 --raw` (form_version 175).
    let result = decode_fixture(
        "PERK",
        175,
        &[
            ("EDID", "5465737454616d655065726b00"),
            (
                "VMAD",
                "060002000000042a00467261676d656e74733a5065726b733a50524b465f5465737454616d655065726b5f3030303034313638000200090053515f4d617374657201010000ffffb8250500160053515f416e696d616c54616d696e674b6579776f726401010000ffff66410000010000000000012a00467261676d656e74733a5065726b733a50524b465f5465737454616d655065726b5f30303030343136381100467261676d656e745f456e7472795f30300300",
            ),
            ("FULL", "3c49443d30303033423545353e54616d6520416e696d616c00"),
            (
                "DESC",
                "3c49443d30303033423545363e436f6d6d756e652077697468206265617374732100",
            ),
            ("DATA", "0000030100"),
            ("SNAM", "27761a00"),
            ("PRKE", "020000"),
            ("DATA", "0e090200"),
            ("PRKC", "01"),
            (
                "CTDA",
                "000000000000000047002f4367410000000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f30022f4366410000000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "00000000000000002e002f4300000000000000000000000000000000ffffffff",
            ),
            ("EPFT", "04"),
            ("EPFB", "0000"),
            ("EPF2", "3c49443d30303033423545373e54414d4500"),
            ("EPF3", "0200"),
            ("PRKF", ""),
        ],
    );

    assert_record_type(&result, "Perk");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("TestTamePerk"),
        "Editor ID"
    );

    let effect = &result["Effects"][0]["Effect"];
    assert_eq!(
        effect.get("Button Label").and_then(|v| v.as_str()),
        Some("TAME"),
        "EPF2 should decode as Button Label lstring when Function Type=4"
    );
    assert_eq!(
        effect["Script Flags"].get("value").and_then(|v| v.as_str()),
        Some("0x2"),
        "EPF3 should decode as Script Flags when Function Type=4"
    );
    let set_flags: Vec<&str> = effect["Script Flags"]["flags"]
        .as_array()
        .expect("Script Flags.flags must be an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        set_flags,
        vec!["Replace Default"],
        "EPF3 Script Flags bit 1 set"
    );
}

/// PERK 0x0085B9A0 — `HTO_Legendary_Armor_RagingPerk` — decodes `EPF2` to an
/// integer instead of raw hex.
///
/// This effect has `Function Type` = 5 ("Spell Item"), so `EPF2` must decode
/// as the u32 "Unknown Spell Value" field rather than `{"hex": "03000000"}`.
#[test]
fn perk_raging_armor_epf2_decodes_to_int() {
    // Verbatim subrecords from `esm get <esm> HTO_Legendary_Armor_RagingPerk --raw`.
    let result = decode_fixture(
        "PERK",
        208,
        &[
            (
                "EDID",
                "48544f5f4c6567656e646172795f41726d6f725f526167696e675065726b00",
            ),
            (
                "FULL",
                "3c49443d37313031343730423e526167696e672041726d6f72205065726b00",
            ),
            ("DESC", "00"),
            ("DATA", "010001"),
            ("PRKE", "0201"),
            ("DATA", "c10a0400"),
            ("EPFT", "05"),
            ("EPFB", "0000"),
            ("EPFD", "a3b98500"),
            ("EPF2", "03000000"),
            ("PRKF", ""),
        ],
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("HTO_Legendary_Armor_RagingPerk"),
        "Editor ID"
    );

    let effect = &result["Effects"][0]["Effect"];
    assert_eq!(
        effect.get("Unknown Spell Value").and_then(|v| v.as_u64()),
        Some(3),
        "EPF2 should decode as Unknown Spell Value (u32) when Function Type=5, not raw hex"
    );
    assert!(
        effect.get("Function Parameter 2").is_none(),
        "EPF2 must not fall back to the raw bytes variant"
    );
}

/// Synthetic PERK with an `Ability` effect immediately followed by an `Entry
/// Point` effect: each effect's optional members bind to that effect, across
/// PERK's per-effect layout — a nested `Perk Conditions` rarray of its own,
/// plus a `Function Data` union whose chosen variant is itself sig-bearing
/// (`EPFD`). The ALCH shape is
/// `alch_two_effects_do_not_cross_contaminate_optional_trailers`.
#[test]
fn perk_ability_then_entry_point_effects_do_not_cross_contaminate_optional_trailers() {
    let result = decode_fixture(
        "PERK",
        209,
        &[
            ("EDID", "5065726b4162696c6974795468656e456e7472795465737400"), // "PerkAbilityThenEntryTest\0"
            // Top-level Perk "Data" struct (Trait,Level,NumRanks,Playable,Hidden,Unknown).
            ("DATA", "0001010001"),
            // Effect 0: Ability (Effect Type=1), rank 0. Effect Data = Ability
            // FormID 0x00112233 (4 bytes, little-endian). No PRKC/EPFT/EPFD/EPFB
            // of its own — only the mandatory PRKE/DATA/PRKF.
            ("PRKE", "0100"),
            ("DATA", "33221100"),
            ("PRKF", ""),
            // Effect 1: Entry Point (Effect Type=2), rank 0. Effect Data = Entry
            // Point struct (Entry Point=0, Function=1, Perk Condition Tab
            // Count=1, unused=0). Carries its own Perk Conditions (PRKC+CTDA)
            // and a Function Type=1 ("Float") Function Data (EPFT/EPFB/EPFD).
            ("PRKE", "0200"),
            ("DATA", "00010100"),
            ("PRKC", "01"),
            (
                "CTDA",
                "000000000000803f30020000b5f36700000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0000"),
            ("EPFD", "00002040"), // f32 2.5
            ("PRKF", ""),
        ],
    );
    assert_record_type(&result, "Perk");
    assert_fully_decoded(&result);

    let effects = result
        .get("Effects")
        .and_then(|v| v.as_array())
        .expect("Effects array must decode");
    assert_eq!(effects.len(), 2, "expected exactly 2 effects");

    let ability = &effects[0]["Effect"];
    assert!(
        ability.get("Float").is_none(),
        "Ability effect must not steal the Entry Point effect's Float: {:?}",
        ability.get("Float")
    );
    assert!(
        ability.get("Perk Conditions").is_none(),
        "Ability effect must not steal the Entry Point effect's Perk Conditions: {:?}",
        ability.get("Perk Conditions")
    );

    let entry = &effects[1]["Effect"];
    let float_val = entry
        .get("Float")
        .and_then(|v| v.as_f64())
        .expect("Entry Point effect must have its own Float");
    assert!(
        (float_val - 2.5).abs() < 1e-6,
        "expected Float ~= 2.5, got {float_val}"
    );
    let conditions = entry
        .get("Perk Conditions")
        .and_then(|v| v.as_array())
        .expect("Entry Point effect must have its own Perk Conditions");
    assert_eq!(conditions.len(), 1, "expected exactly 1 Perk Condition");
}

/// PERK 0x003DE597 — `Suppressor_TargetDebuff` — an `EPFT`=8 ("Actor Value
/// and Value") entry-point effect with an 8-byte `EPFD` payload (FormID +
/// float) must decode as a struct. Decoding it as a single bare `float` would
/// reinterpret the leading FormID bytes as garbage (e.g.
/// `6.211654813182083e-39` for FormID `0x0043A391`) and silently drop the
/// trailing float value.
///
/// Ground truth (TES5Edit `wbEPFDAVDataDecider`,
/// `Core/wbDefinitionsFO76.pas:9899-9905`): `EPFD` DataSize >= 8 selects a
/// `{Actor Value, Float}` struct; DataSize < 8 selects a bare `Float` (see
/// `perk_ticket_to_revenge_actor_value_and_value_short_epfd_decodes_float`
/// below for that companion case).
#[test]
fn perk_suppressor_target_debuff_actor_value_and_value_decodes_formid() {
    // Verbatim subrecords from `esm get <esm> Suppressor_TargetDebuff --raw`.
    let result = decode_fixture(
        "PERK",
        181,
        &[
            ("EDID", "53757070726573736f725f54617267657444656275666600"),
            (
                "FULL",
                "3c49443d30303033423338413e53757070726573736f7220285461726765742900",
            ),
            (
                "DESC",
                "3c49443d30303033423338423e5265647563652074617267657427732064616d616765206f7574707574206279202876616c7565206f66205065726b53757070726573736f7243757272656e74446562756666206163746f722076616c75652920666f722058207365636f6e647320616674657220796f752061747461636b2e00",
            ),
            ("DATA", "0001010001"),
            ("SNAM", "33761a00"),
            ("PRKE", "0200"),
            ("DATA", "230e0300"),
            ("EPFT", "08"),
            ("EPFB", "0000"),
            ("EPFD", "91a343000ad723bc"),
            ("PRKF", ""),
        ],
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("Suppressor_TargetDebuff"),
        "Editor ID"
    );

    let effect = &result["Effects"][0]["Effect"];
    let av_float = &effect["Actor Value, Float"];
    assert_eq!(
        av_float["Actor Value"].as_str(),
        Some("0x0043A391"),
        "EPFD's leading 4 bytes must decode as the Actor Value FormID, not a float"
    );
    let float_val = av_float["Float"]
        .as_f64()
        .expect("Actor Value, Float.Float must decode as a number");
    assert!(
        (float_val - -0.01).abs() < 1e-6,
        "EPFD's trailing 4 bytes must decode as the float -0.01, got {float_val}"
    );
    assert!(
        effect.get("Float").is_none(),
        "an 8-byte EPFD under Function Type=8 must not fall back to the bare Float variant"
    );
}

/// PERK 0x00913B5F — `custom_TickettoRevenge_Perk` — companion case to the
/// test above: an `EPFT`=8 effect whose `EPFD` payload is only 4
/// bytes (the actor value instead lives in the sibling `EPF3` FormID field)
/// must keep decoding as a bare `Float`, not the `{Actor Value, Float}`
/// struct.
#[test]
fn perk_ticket_to_revenge_actor_value_and_value_short_epfd_decodes_float() {
    // Verbatim subrecords from `esm get <esm> custom_TickettoRevenge_Perk --raw`.
    let result = decode_fixture(
        "PERK",
        209,
        &[
            (
                "EDID",
                "637573746f6d5f5469636b6574746f526576656e67655f5065726b00",
            ),
            (
                "FULL",
                "3c49443d36313032414534343e5469636b657420746f20526576656e676500",
            ),
            (
                "DESC",
                "3c49443d36313032414534353e2b33252041726d6f722050656e6574726174696f6e20706572204f6e736c617567687420737461636b00",
            ),
            ("DATA", "010001"),
            ("FNAM", "436f6d6d616e646f00"),
            ("PRKE", "0201"),
            ("DATA", "250e0300"),
            ("EPFT", "08"),
            ("EPFB", "0000"),
            ("EPFD", "8fc2f5bc"),
            ("EPF3", "95030000"),
            ("PRKF", ""),
        ],
    );
    assert_fully_decoded(&result);

    let effect = &result["Effects"][0]["Effect"];
    let float_val = effect
        .get("Float")
        .and_then(|v| v.as_f64())
        .expect("4-byte EPFD under Function Type=8 must still decode as a bare Float");
    assert!(
        (float_val - -0.03).abs() < 1e-6,
        "expected Float ~= -0.03, got {float_val}"
    );
    assert_eq!(
        effect["Function Parameter 3 (Actor Value)"].as_str(),
        Some("0x00000395"),
        "EPF3 must still decode as the Actor Value FormID for the short-EPFD form"
    );
    assert!(
        effect.get("Actor Value, Float").is_none(),
        "a 4-byte EPFD must not be mis-routed into the struct variant"
    );
}

/// PERK 0x004E1F1C — `PlayerTeamPerk` (fv 201, 47 subs) — fully decoded.
#[test]
fn perk_player_team_perk_decodes_correctly() {
    let result = decode_fixture(
        "PERK",
        201,
        &[
            ("EDID", "506c617965725465616d5065726b00"),
            (
                "FULL",
                "3c49443d30303033423242363e506c61796572205465616d205065726b00",
            ),
            (
                "DESC",
                "3c49443d30303033423242373e537570706f727420666f7220706c61796572207465616d2d6261736564207065726b732c206d75746174696f6e732c206574632e0d0a0d0a4e4f54453a2053706f746c69676874207765656b20627566667320696d706c656d656e746564206865726520666f72206e6f772e0d0a4e4f54453a2048616c66206f662053706f746c69676874207765656b203820697320616c736f20696d706c656d656e74656420696e2074686520616c636f686f6c20616464696374696f6e20455020696e2074686520416464696374696f6e4d616e61676572207065726b202873616d65206f6e6520666f722050726f66657373696f6e616c204472696e6b6572292e00",
            ),
            ("DATA", "000101"),
            ("PRKE", "025c"),
            ("DATA", "24030300"),
            ("PRKC", "00"),
            (
                "CTDA",
                "00000000000000400e00000002ed7a00000000000d0000000000000001000000",
            ),
            ("EPFT", "01"),
            ("EPFB", "0e00"),
            ("EPFD", "7b142e3f"),
            ("PRKF", ""),
            ("PRKE", "025a"),
            ("DATA", "24030300"),
            ("PRKC", "00"),
            (
                "CTDA",
                "000000000000803f0e00000002ed7a00000000000d0000000000000001000000",
            ),
            (
                "CTDA",
                "80000000000000400e00000002ed7a00000000000d0000000000000000000000",
            ),
            ("EPFT", "01"),
            ("EPFB", "0300"),
            ("EPFD", "0000403f"),
            ("PRKF", ""),
            ("PRKE", "0100"),
            ("DATA", "e9c24700"),
            ("PRKF", ""),
            ("PRKE", "0200"),
            ("DATA", "57030300"),
            ("PRKC", "00"),
            (
                "CTDA",
                "00000000000000404a000000eac24700000000000000000000000000ffffffff",
            ),
            ("PRKC", "02"),
            (
                "CTDA",
                "000000000000803f3002000027724e00000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0000"),
            ("EPFD", "00000040"),
            ("PRKF", ""),
            ("PRKE", "0200"),
            ("DATA", "1e030400"),
            ("PRKC", "00"),
            (
                "CTDA",
                "00000000000000414a000000eac24700000000000000000000000000ffffffff",
            ),
            ("PRKC", "01"),
            (
                "CTDA",
                "000000000000803fb502000016c41000000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "0000000000000000f5010000ab754e00000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "0000000000000000f5010000c8924300000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "0000000000000000f50100007eae3b00000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0500"),
            ("EPFD", "00002040"),
            ("PRKF", ""),
        ],
    );
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("PlayerTeamPerk"),
        "Editor ID"
    );
}

/// Synthetic PERK — 3-byte top-level `DATA` (fv 209) — `from_size`-gated Trait/
/// Level/`Num Ranks` must be absent; Playable/Hidden/Unknown decode from the
/// short layout.
#[test]
fn perk_data_short_form_decodes_playable_hidden_unknown_not_trait_level_num_ranks() {
    let result = decode_fixture(
        "PERK",
        209,
        &[
            ("EDID", "5065726b4461746153686f7274466f726d5465737400"),
            ("DATA", "0100ab"),
        ],
    );
    assert_record_type(&result, "Perk");
    assert_fully_decoded(&result);

    let data = result.get("Data").expect("Data struct must decode");

    assert!(
        data.get("Trait").is_none(),
        "Trait must be absent for 3-byte PERK DATA"
    );
    assert!(
        data.get("Level").is_none(),
        "Level must be absent for 3-byte PERK DATA"
    );
    assert!(
        data.get("Num Ranks").is_none(),
        "'Num Ranks' must be absent for 3-byte PERK DATA"
    );

    assert_eq!(
        data.pointer("/Playable/name").and_then(|v| v.as_str()),
        Some("True"),
        "Playable"
    );
    assert_eq!(
        data.pointer("/Hidden/name").and_then(|v| v.as_str()),
        Some("False"),
        "Hidden"
    );
    assert_eq!(
        data.pointer("/Unknown/hex").and_then(|v| v.as_str()),
        Some("ab"),
        "Unknown"
    );
}

/// PERK 0x00056DD7 — `PlayerPerk` (fv 209, 115 subs) — fully decoded.
#[test]
fn perk_player_perk_decodes_correctly() {
    let result = decode_fixture(
        "PERK",
        209,
        &[
            ("EDID", "506c617965725065726b00"),
            ("FULL", "3c49443d36313031343142333e506c61796572205065726b00"),
            ("DESC", "00"),
            ("DATA", "000101"),
            ("PRKE", "020e"),
            ("DATA", "08030200"),
            ("PRKC", "01"),
            (
                "CTDA",
                "000000000000803f30020000b5f36700000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0c00"),
            ("EPFD", "0000a03f"),
            ("PRKF", ""),
            ("PRKE", "020d"),
            ("DATA", "52030200"),
            ("PRKC", "00"),
            (
                "CTDA",
                "040000009c3648004a000000eac24700000000000000000000000000ffffffff",
            ),
            ("PRKC", "01"),
            (
                "CTDA",
                "010000000000803f450000002f0f0a00000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "010000000000803f4500000006ba1300000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0000"),
            ("EPFD", "0000c03f"),
            ("PRKF", ""),
            ("PRKE", "020c"),
            ("DATA", "9f100100"),
            ("EPFT", "09"),
            ("EPFB", "0e00"),
            ("EPFD", "12a04e00"),
            ("PRKF", ""),
            ("PRKE", "020b"),
            ("DATA", "7b020100"),
            ("PRKC", "00"),
            (
                "CTDA",
                "a0000000000000000e0000006c030000000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0d00"),
            ("EPFD", "9a9919be"),
            ("PRKF", ""),
            ("PRKE", "020a"),
            ("DATA", "08030200"),
            ("PRKC", "01"),
            (
                "CTDA",
                "000000000000803f47000000c4ff1900000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0100"),
            ("EPFD", "0000403f"),
            ("PRKF", ""),
            ("PRKE", "0209"),
            ("DATA", "3c030200"),
            ("PRKC", "01"),
            (
                "CTDA",
                "000000000000803f47000000c4ff1900000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0200"),
            ("EPFD", "0000a03f"),
            ("PRKF", ""),
            ("PRKE", "0208"),
            ("DATA", "12050300"),
            ("EPFT", "08"),
            ("EPFB", "0500"),
            ("EPFD", "0ad7233c"),
            ("EPF3", "52106e00"),
            ("PRKF", ""),
            ("PRKE", "0207"),
            ("DATA", "300e0200"),
            ("EPFT", "08"),
            ("EPFB", "0400"),
            ("EPFD", "000080bf"),
            ("EPF3", "16331800"),
            ("PRKF", ""),
            ("PRKE", "0206"),
            ("DATA", "3a0d0100"),
            ("EPFT", "08"),
            ("EPFB", "0900"),
            ("EPFD", "0000803f"),
            ("EPF3", "3da40b00"),
            ("PRKF", ""),
            ("PRKE", "0205"),
            ("DATA", "2f0e0200"),
            ("PRKC", "00"),
            (
                "CTDA",
                "8000000000004842c702000000000000000000000000000000000000ffffffff",
            ),
            ("EPFT", "08"),
            ("EPFB", "0600"),
            ("EPFD", "000080bf"),
            ("EPF3", "12331800"),
            ("PRKF", ""),
            ("PRKE", "0204"),
            ("DATA", "27050100"),
            ("EPFT", "08"),
            ("EPFB", "0a00"),
            ("EPFD", "0000803f"),
            ("EPF3", "0e331800"),
            ("PRKF", ""),
            ("PRKE", "0203"),
            ("DATA", "16030400"),
            ("PRKC", "00"),
            (
                "CTDA",
                "000000000000803f4a00000067581400000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0800"),
            ("EPFD", "6666863f"),
            ("PRKF", ""),
            ("PRKE", "0100"),
            ("DATA", "c4767a00"),
            ("PRKF", ""),
            ("PRKE", "0100"),
            ("DATA", "1a1b6e00"),
            ("PRKF", ""),
            ("PRKE", "0200"),
            ("DATA", "b80a0300"),
            ("PRKC", "00"),
            (
                "CTDA",
                "000000000000803faa0200008d7f5200000000000000000000000000ffffffff",
            ),
            ("PRKC", "02"),
            (
                "CTDA",
                "000000000000000030020000f32b0b00000000000000000000000000ffffffff",
            ),
            ("EPFT", "05"),
            ("EPFB", "0300"),
            ("EPFD", "a9c87900"),
            ("EPF2", "00000000"),
            ("PRKF", ""),
        ],
    );
    assert_fully_decoded(&result);
    // Post-from_size 3-byte PERK DATA layout: Playable lives in Data, not Trait/Level/Num Ranks.
    assert!(
        result.get("Data").and_then(|d| d.get("Playable")).is_some(),
        "Data.Playable must be present"
    );
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("PlayerPerk"),
        "Editor ID"
    );
}

/// PERK 0x007A76BA — `GHL_PlayerPerk` (fv 208, 40 subs) — fully decoded.
#[test]
fn perk_ghl_player_perk_decodes_correctly() {
    let result = decode_fixture(
        "PERK",
        208,
        &[
            ("EDID", "47484c5f506c617965725065726b00"),
            (
                "FULL",
                "3c49443d34313030463046423e506c617965722047686f756c205065726b00",
            ),
            ("DESC", "00"),
            ("DATA", "000101"),
            ("PRKE", "0106"),
            ("DATA", "45e77a00"),
            ("PRKF", ""),
            ("PRKE", "0105"),
            ("DATA", "46e77a00"),
            ("PRKF", ""),
            ("PRKE", "0204"),
            ("DATA", "ca010200"),
            ("PRKC", "00"),
            (
                "CTDA",
                "00000000000000005603000087850b00000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "00000000000000005603000056400200000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "0000000000000000bb020000b7ff7f00000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "00000000000000002327000000000000000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0200"),
            ("EPFD", "3333b33e"),
            ("PRKF", ""),
            ("PRKE", "0202"),
            ("DATA", "6a010100"),
            ("EPFT", "01"),
            ("EPFB", "0300"),
            ("EPFD", "00000000"),
            ("PRKF", ""),
            ("PRKE", "0100"),
            ("DATA", "d5e48e00"),
            ("PRKF", ""),
            ("PRKE", "0200"),
            ("DATA", "cb030200"),
            ("PRKC", "00"),
            (
                "CTDA",
                "00000000000000005603000087850b00000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "00000000000000005603000056400200000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "0000000000000000bb020000b7ff7f00000000000000000000000000ffffffff",
            ),
            ("EPFT", "01"),
            ("EPFB", "0e00"),
            ("EPFD", "3333b33e"),
            ("PRKF", ""),
        ],
    );
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("GHL_PlayerPerk"),
        "Editor ID"
    );
}
