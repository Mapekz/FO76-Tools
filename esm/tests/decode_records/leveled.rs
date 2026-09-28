//! Leveled lists: LVLI, LVLN, LVLP, LVPC.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{assert_fully_decoded, assert_record_type, decode_fixture};

/// LVLI 0x0000129C — `LL_Flora_Corn`.
///
/// form_version 197 (≥174).  `wbBelowVersion(174, LVLD …)` means the existing
/// LVLD schema member is inactive at fv≥174; an `empty` member with
/// `from_version:174` in `fo76.overrides.json` consumes the empty subrecord.
#[test]
fn lvli_flora_corn_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x0000129C --raw` (form_version 197).
    let result = decode_fixture(
        "LVLI",
        197,
        &[
            ("EDID", "4c4c5f466c6f72615f436f726e00"),
            ("OBND", "fefffefff8ff020002000800"),
            ("LVLD", ""),
            ("ONAM", "00"),
            ("LVMV", "00000000"),
            ("LVCV", "00000000"),
            ("LVLF", "0400"),
            ("LLCT", "04"),
            ("LVLO", "f8300300"),
            (
                "CTDA",
                "40000000000000004e0300009d12000044b009000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "a4000000a8c443004d00000000000000000000000000000000000000ffffffff",
            ),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "f8300300"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "f8300300"),
            (
                "CTDA",
                "000000000000803f5603000042d82e00000000000100000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f1403000000000000000000000000000000000000ffffffff",
            ),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "f8300300"),
            (
                "CTDA",
                "a4000000c4ba6b004d00000000000000000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f5603000042d82e00000000000100000000000000ffffffff",
            ),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            (
                "MODL",
                "4c616e6473636170655c506c616e74735c496e6772656469656e74735c436f726e2e6e696600",
            ),
            ("MODT", "0400000000000000000000000000000000000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
        ],
    );

    assert_fully_decoded(&result);
}

/// LVLN 0x00004073 — `<no edid>` — basic decode regression.
#[test]
fn lvln_test_essl_char_short_decodes_correctly() {
    let result = decode_fixture(
        "LVLN",
        175,
        &[
            ("EDID", "546573744553534c4368617253686f727400"),
            ("OBND", "000000000000000000000000"),
            ("LVLD", ""),
            ("LVMV", "00000000"),
            ("LVCV", "00000000"),
            ("LVLF", "00"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
        ],
    );

    assert_record_type(&result, "Leveled NPC");
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("TestESSLCharShort"),
    );
}

/// LVPC 0x006DE9E3 — `NPE_Loadout_CommandoSelectionList` — basic decode regression.
#[test]
fn lvpc_loadout_commando_selection_list_decodes_correctly() {
    let result = decode_fixture(
        "LVPC",
        204,
        &[
            (
                "EDID",
                "4e50455f4c6f61646f75745f436f6d6d616e646f53656c656374696f6e4c69737400",
            ),
            ("LVLD", ""),
            ("ONAM", "00"),
            ("LVMV", "00000000"),
            ("LVCV", "00000000"),
            ("LVLF", "0c00"),
            ("LLCT", "12"),
            ("LVLO", "cebc1000"),
            ("LVUD", "01"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "e5070900"),
            ("LVUD", "01"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "6c203500"),
            ("LVUD", "01"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "f6ae3100"),
            ("LVUD", "02"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "93e53d00"),
            ("LVUD", "01"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "53ad0800"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "3beb0300"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "893f3200"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "843e0900"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "9e0b3300"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "5a0d3100"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "e0a72500"),
            ("LVUD", "01"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "47d20800"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "6bd03900"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "903f3200"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "056a3500"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "9bab3800"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVLO", "46a52b00"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            ("LVCL", ""),
            ("LVUO", "0000"),
        ],
    );

    assert_record_type(&result, "Leveled Perk Card");
    assert_fully_decoded(&result);
}

/// LVLP 0x003A1255 — `<no edid>` — basic decode regression.
#[test]
fn lvlp_frag_mine_owned_decodes_correctly() {
    let result = decode_fixture(
        "LVLP",
        178,
        &[
            ("EDID", "4c50414b5f467261674d696e655f4f776e65644d61696e00"),
            ("OBND", "f9fff9ff0000070007000400"),
            ("LVLD", ""),
            ("LVMV", "00000000"),
            ("LVCV", "00000000"),
            ("LVLF", "00"),
            ("LLCT", "01"),
            ("LVLO", "54123a00"),
            ("LVOV", "00000000"),
            ("LVIV", "0000803f"),
            ("LVLV", "0000803f"),
            (
                "MODL",
                "576561706f6e735c4d696e655c467261674d696e6550726f6a656374696c652e6e696600",
            ),
            (
                "MODT",
                "040000000400000000000000010000000100000081a1769c64647300fefd0268e29c796664647300fefd0268adc0787264647300fefd0268b329ad9264647300fefd0268131325cd6267736d6cf7e8e6",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
        ],
    );

    assert_record_type(&result, "Leveled Pack In");
    assert_fully_decoded(&result);
}
