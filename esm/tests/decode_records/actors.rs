//! NPC_ records.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{assert_fully_decoded, assert_record_type, bare_ctx, decode_fixture};
use esm::schema::Schema;

/// NPC_ 0x00425171 — `W05_Settler_Ward` — decodes to Non-Player Character fully.
///
/// form_version 209.  Exercises VMAD (12 properties on one script), ACBS flags,
/// TPTA template array, PRPS actor properties, COCT/CNTO inventory, KSIZ/KWDA
/// keywords, TETI/TEND tint entries, FMRI/FMRS face morph pairs, and CVT2 curve.
#[test]
fn npc_w05_settler_ward_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x00425171 --raw` (form_version 209).
    let result = decode_fixture(
        "NPC_",
        209,
        &[
            ("EDID", "5730355f536574746c65725f5761726400"),
            (
                "VMAD",
                "0600020001001b005730355f4163746f724e756b655265616374696f6e536372697074000b0019005730355f4e50434e756b655265616374696f6e52616469757301010000ffff0c8253001f005730355f4e756b65536574746c656d656e745f426f6d62496e636f6d696e6701010000ffff0682530018005730355f4e50434e756b65457175697044656c61794d617801010000ffff1182530021005730355f4e50434e756b655f48756d616e48617a6d6174537569744f757466697401010000ffff138253001a005730355f4e50434e756b65556e657175697044656c61794d617801010000ffff168253001300454e30375f4d515f4e756b655f4d617374657201010000ffff670f2d001c005730355f4e50434e756b65466c65655461726765744b6579776f726401010000ffff0e82530014005730355f4e50434e756b65466c656556616c756501010000ffff0d82530018005730355f4e50434e756b65457175697044656c61794d696e01010000ffff108253001100454e30375f4d515f466c6565426c61737401010000ffff690f2d001a005730355f4e50434e756b65556e657175697044656c61794d696e01010000ffff15825300",
            ),
            ("OBND", "d7ffe7ff0000290048005600"),
            ("PHST", "00000000"),
            ("ACBS", "3a0000a200004200270000002300020000000200"),
            ("AJNG", "38405c00"),
            ("AJXG", "33405c00"),
            ("SNAM", "00bc3f0000"),
            ("SNAM", "08c03f0000"),
            ("SNAM", "c8e0030000"),
            ("VTCK", "80514200"),
            ("TPLT", "de1f5a00"),
            (
                "TPTA",
                "00000000de1f5a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            ),
            ("RNAM", "46370100"),
            ("ATKR", "46370100"),
            ("ECOR", "3f220400"),
            (
                "PRPS",
                "da0200000000c84200000000d50200000000484200000000c80200000000000000000000c70200000000a04000000000c60200000000000000000000c50200000000000000000000c40200000000000000000000c30200000000804000000000c20200000000004100000000d402000000000000fae97600",
            ),
            ("COCT", "03000000"),
            ("CNTO", "c8d8580001000000"),
            ("CNTO", "c9d8580001000000"),
            ("CNTO", "c5d8580001000000"),
            ("AIDT", "000032000002000000000000000000000000000000000000"),
            ("PKID", "59375c00"),
            ("KSIZ", "04000000"),
            ("KWDA", "b6c54f00fba944001882530012625200"),
            ("CNAM", "70f90500"),
            ("FULL", "3c49443d44393030324344343e5761726400"),
            ("SHRT", "3c49443d44393030324344353e5761726400"),
            ("DATA", ""),
            ("DNAM", "1602320000000100"),
            ("ZNAM", "3b180300"),
            ("NAM5", "ff00"),
            ("NAM6", "0000803f"),
            ("NAM4", "0000803f"),
            ("NAM8", "01000000"),
            ("DOFT", "70514200"),
            ("DPLT", "332b0200"),
            ("PNAM", "35f70100"),
            ("PNAM", "31160500"),
            ("PNAM", "b81e0500"),
            ("PNAM", "a61e0500"),
            ("PNAM", "662a1d00"),
            ("PNAM", "652a1d00"),
            ("PNAM", "8f101400"),
            ("PNAM", "a7440800"),
            ("PNAM", "bdcc1100"),
            ("PNAM", "af230400"),
            ("PNAM", "bbee0100"),
            ("PNAM", "9e7f1d00"),
            ("HCLF", "2c040a00"),
            ("MWGT", "d6560c3fb6f3dd3e2b18953c"),
            ("FTST", "598c1200"),
            ("QNAM", "ebe9693fdcda5a3fdad8583f0000803f"),
            (
                "MSDK",
                "3c36ef36d635ef367834ef36d435ef361436ef363336ef360b36ef367c35ef361336ef36d535ef36d335ef362d36ef36",
            ),
            (
                "MSDV",
                "295c8f3e04e81dbd4e4b3abf5c8f023f7b14ae3e7b146e3f14aec73ecdcc4c3e0ad7a33eae47e13d1f852b3fec51383f",
            ),
            ("TETI", "0100b504"),
            ("TEND", "1900000000b204"),
            ("TETI", "01008404"),
            ("TEND", "64e9dad8008604"),
            ("TETI", "02009e04"),
            ("TEND", "4e"),
            ("TETI", "02004805"),
            ("TEND", "64"),
            ("TETI", "0200e506"),
            ("TEND", "12"),
            ("TETI", "02005a05"),
            ("TEND", "1d"),
            ("TETI", "02001d06"),
            ("TEND", "4a"),
            ("MRSV", "85eb51bfd7a3f03eb81e853ecdcc4cbe00000000"),
            ("FMRI", "00000000"),
            (
                "FMRS",
                "ce0704be5c8f02bfb7cfc53edfa8113e0000000000000000000000000000000000000000",
            ),
            ("FMRI", "c534ef36"),
            (
                "FMRS",
                "e2b30ebf0ad723bdda0290bc0000000000000000295c8fbd000000000000000000000000",
            ),
            ("FMRI", "ba34ef36"),
            (
                "FMRS",
                "493d493d3449853e0724b23e000000000000000000000000ef84323f0000000000000000",
            ),
            ("FMRI", "c734ef36"),
            (
                "FMRS",
                "00000000adef3bbe954575be0000000000000000000000009d9125bf0000000000000000",
            ),
            ("FMRI", "09000000"),
            (
                "FMRS",
                "0ad7233dd2915cbea3c7f43e000000000000000000000000f781d6bc0000000000000000",
            ),
            ("FMRI", "02000000"),
            (
                "FMRS",
                "cdccccbd9a99193f10d184be000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "3e000000"),
            (
                "FMRS",
                "7234e03e6a76f73d52d5e83e000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "bc34ef36"),
            (
                "FMRS",
                "00000000a79558be69bb11bf000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "c134ef36"),
            (
                "FMRS",
                "0ad7a3bcc591823e309f9cbece5bf2be0000000000000000d9fc13bf0000000000000000",
            ),
            ("FMRI", "04000000"),
            (
                "FMRS",
                "00000000000000000ad723bf000000000000000000000000c8a606bf0000000000000000",
            ),
            ("FMRI", "06000000"),
            (
                "FMRS",
                "2601a7be5264eabee17a943e000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "c334ef36"),
            (
                "FMRS",
                "0ad723bdae47e13dac4924beff12fbbe0000000000000000632ed2be0000000000000000",
            ),
            ("FMRI", "be34ef36"),
            (
                "FMRS",
                "000000000000000085c2fabe000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "0a000000"),
            (
                "FMRS",
                "3333b3be48e1fa3e0ad7a3bc0000000000000000000000003b9dcb3d0000000000000000",
            ),
            ("FMRI", "01000000"),
            (
                "FMRS",
                "24d8223e7b142ebfcefaa0be000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "c434ef36"),
            (
                "FMRS",
                "35ba7dbe3acc9dbe9f1e86be0000000000000000000000005c8f42bf0000000000000000",
            ),
            ("FMRI", "c634ef36"),
            (
                "FMRS",
                "00000000fcbf24be697b4fbe000000000000000000000000011422bf0000000000000000",
            ),
            ("FMRI", "bb34ef36"),
            (
                "FMRS",
                "1693913e713d4abff197a9be000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "03000000"),
            (
                "FMRS",
                "dd32053e8ff0a2bd531217bf000000000000000000000000067a2dbc0000000000000000",
            ),
            ("FMRI", "08000000"),
            (
                "FMRS",
                "bfba18bee66cfe3ec40fee3d0000000000000000000000006de2f73e0000000000000000",
            ),
            ("FMRI", "c034ef36"),
            (
                "FMRS",
                "000000006c9ef1be61ae9fbb000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "bd34ef36"),
            (
                "FMRS",
                "c38efdbef6285cbf14ae07bf295c0fbf0000000000000000295c8f3e0000000000000000",
            ),
            ("FMRI", "05000000"),
            (
                "FMRS",
                "68fb17bf9a9919be0b442a3f00000000145b99bc000000009a9919be0000000000000000",
            ),
            ("FMRI", "3f000000"),
            (
                "FMRS",
                "000000008326043f8d4badbe000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "07000000"),
            (
                "FMRS",
                "0036bbbd04a1bbbe9370cfbe000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "bf34ef36"),
            (
                "FMRS",
                "f26e733eab29d0be36f0413eec5138be000000000ad7233d1bf8133e0000000000000000",
            ),
            ("FMRI", "c234ef36"),
            (
                "FMRS",
                "00000000000000bf1039153e0000000000000000000000001f5b1fbf0000000000000000",
            ),
            ("FMRI", "19000000"),
            (
                "FMRS",
                "00000000d39b0c3fbdd995be000000000000000000000000dda1b63e0000000000000000",
            ),
            ("FMRI", "1a000000"),
            (
                "FMRS",
                "cdccccbd1f85ebbe8458b5be000000000000000000000000000000000000000000000000",
            ),
            ("FMRI", "1b000000"),
            (
                "FMRS",
                "f092953ef2229ebe56c14d3d000000000000000000000000000000000000000000000000",
            ),
            ("FMIN", "9a99993f"),
            ("CVT2", "65ef3400"),
        ],
    );

    assert_record_type(&result, "Non-Player Character");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("W05_Settler_Ward"),
        "Editor ID"
    );
    assert_eq!(
        result.get("Short Name").and_then(|v| v.as_str()),
        Some("Ward"),
        "Short Name"
    );
}

/// NPC_ 0x0084FB8F — `ATX_CAMPPets_Actor_RadHog_Standard`.
///
/// form_version 209.  AWPB (reverse-engineered FormID) and CTDA (standard
/// 32-byte condition) are mapped in `fo76.overrides.json`.
#[test]
fn npc_radhog_camp_pet_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x0084FB8F --raw` (form_version 209).
    let result = decode_fixture(
        "NPC_",
        209,
        &[
            (
                "EDID",
                "4154585f43414d50506574735f4163746f725f526164486f675f5374616e6461726400",
            ),
            ("OBND", "eaffbbff0000160061005600"),
            ("PHST", "00000000"),
            ("ACBS", "7a8000a8000001000000000023007e2f00000000"),
            ("AJNG", "c6c28d00"),
            ("AJXG", "c7c28d00"),
            ("SNAM", "c8e0030000"),
            ("SNAM", "306c100000"),
            ("SNAM", "f337030000"),
            ("VTCK", "147a8200"),
            ("TPLT", "90fb8400"),
            (
                "TPTA",
                "0000000090fb840090fb840090fb840090fb840090fb840090fb84000000000090fb840090fb840090fb840090fb84000000000090fb8400",
            ),
            ("RNAM", "91fb8400"),
            ("SPCT", "01000000"),
            ("SPLO", "54078400"),
            ("WNAM", "88fb8400"),
            ("ATKR", "91fb8400"),
            ("ECOR", "3f220400"),
            ("PRKZ", "04000000"),
            ("PRKR", "75270a00"),
            ("PRKR", "cbc28d00"),
            ("PRKR", "cac28d00"),
            ("PRKR", "13218f00"),
            ("PRPS", "da0200000000c84200000000"),
            ("INRD", "02715b00"),
            ("AIDT", "000332030001000000000000000000000000000000000000"),
            ("PKID", "09058b00"),
            ("PKID", "f7be8a00"),
            ("PKID", "4f957900"),
            ("PKID", "14be7a00"),
            ("PKID", "4e957900"),
            ("PKID", "4d957900"),
            ("PKID", "27cd3800"),
            ("KSIZ", "0b000000"),
            (
                "KWDA",
                "94707900ff7982008cfb84008dfb8400b6c54f0011962400fba9440018825300126252002ad50a00164b6300",
            ),
            ("APPR", "64a24700"),
            ("CNAM", "64a58d00"),
            ("FULL", "3c49443d36313032383230303e526164686f6700"),
            ("DATA", ""),
            ("DNAM", "c201960000000100"),
            ("ZNAM", "f64d8f00"),
            ("NAM5", "ff00"),
            ("NAM6", "6666663f"),
            ("NAM4", "6666663f"),
            ("NAM8", "01000000"),
            ("CSCR", "8f9a8600"),
            ("DPLT", "332b0200"),
            ("HCLF", "2e040a00"),
            ("MWGT", "0000003f0000003f00000000"),
            ("QNAM", "8180003f8180003f8180003f0000803f"),
            ("AWPB", "d3a68b00"),
            ("AWPC", ""),
            ("CITC", "06000000"),
            (
                "CTDA",
                "000000000000803f5b0300008afb8400000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f5b0300008afb8400000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f5b0300008afb8400000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f5b0300008afb8400000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f5b0300008afb8400000000000000000000000000ffffffff",
            ),
            (
                "CTDA",
                "000000000000803f5b0300008afb8400000000000000000000000000ffffffff",
            ),
        ],
    );

    assert_fully_decoded(&result);
}
// Part 2 — basic decode tests for partial records.

/// NPC_ 0x0061E065 — `zzzSCORE_S5_COMP_SuperMutant_Maul` — VMAD with type-0 (None) and
/// type-7 (Struct) script properties.  This test asserts the 1433-byte VMAD decodes
/// fully into 5 scripts.
#[test]
fn npc_comp_super_mutant_maul_vmad_decodes_correctly() {
    use esm::decode::decode_vmad;
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx(&schema);
    let data = crate::common::hex_bytes(concat!(
        "0600020005001400434f4d505f49646c6554696d65725363726970740004001700434f4d505f4964",
        "6c654368617474657254696d654d617801010000ffff861c5a001700434f4d505f49646c65436861",
        "7474657254696d654d696e01010000ffff851c5a00120049646c654368617474657254696d654d61",
        "7801010000ffffcd020000120049646c654368617474657254696d654d696e01010000ffffcc0200",
        "001d00436f6d70616e696f6e5251537461676548616e646c65725363726970740002000f00434f4d",
        "505f5175657374436f756e7401010000ffffb43f5600060052514461746100010f00436f6d70616e",
        "696f6e53637269707400160017005475746f7269616c5f416c6c6965735f496e74726f3031010100",
        "00ffff0a385a00090043616d70517565737401010000ffff63e061001900434f4d505f41565f496e",
        "74726f5475746f7269616c4f6e636501010000ffff09385a000e00434f4d505f52515f4d61737465",
        "7201010000fffff1615500140053796e6365644163746f7256616c75654461746107010200000006",
        "00416c6c79415601010000ffffb43f56000800506c61796572415601010000ffff51e06100130043",
        "4f4d505f43616d705175657374537461727401010000ffff6aeb54000f00434f4d505f5175657374",
        "436f756e7401010000ffffb43f560017005475746f7269616c5f416c6c6965735f496e74726f3032",
        "01010000ffff0b385a001b00506c6179657252616469616e74517565737444617461496e64657801",
        "010000ffffe3245a00100052616469616e74517565737444617461000117005475746f7269616c5f",
        "416c6c6965735f496e74726f303301010000ffff0c385a001d0046657463684b6e6f776e4f626a65",
        "63744c6973745f496e6465785f415601010000ffff974c5a002400434f4d505f52515f4c6f634b65",
        "79776f7264735f4d555354484156455f44656661756c7401010000ffffc9765500170043616d7041",
        "63746f72546f576f726b73686f704c696e6b01010000ffff691b41001200506c6179657251756573",
        "74436f756e74415601010000ffff51e06100180043616d705175657374436f6d706c6574696f6e53",
        "746167650301ffffffff0c00476c6f62616c546f67676c6501010000ffff53e06100150043616d70",
        "4163746f72546f506c617965724c696e6b01010000ffff681b41001500434f4d505f53686f756c64",
        "426544697361626c656401010000ffff46e159001300434f4d505f52515f52616e646f6d53746172",
        "7401010000ffffa5f154000f00434f4d505f5175657374537461676501010000ffffb02b56001e00",
        "434f4d505f4461696c7951756573745f4e657874416c6c6f77656444617901010000ffffddcc5500",
        "1600436f6d70616e696f6e56697369746f725363726970740007000c00434f4d505f56697369746f",
        "7201010000ffff53fd55001700434f4d505f56697369746f724b6579776f72644c69737401010000",
        "ffff57fd55001100434f4d505f56697369746f72537461727401010000ffff59fd55001400434f4d",
        "505f56697369746f725f43757272656e7401010000ffff5cfd55000a0043616d7052616469757301",
        "010000ffff6c1b4100180043616d70566973746f72546f576f726b73686f704c696e6b01010000ff",
        "ffed3c57001200537061776e656456697369746f7244617461070107000000090045787069727944",
        "61790401000000400b00457870697279446179415601010000ffffec3c57000b00537061776e4368",
        "616e63650301140000000e00506c6179657241565f56616c75650401000000401d00506c61796572",
        "41565f477265617465725468616e4f72457175616c546f0501010800506c61796572415601010000",
        "ffffd45c58000e0056697369746f72546f537061776e01010000ffffeb3c5700240044656661756c",
        "744163746f7249676e6f7265467269656e646c7948697473536372697074000000"
    ));
    let result = decode_vmad(&ctx, &data);
    assert!(
        result.get("_raw").is_none(),
        "decode_vmad produced raw fallback (type-0/type-7 regression): {result}",
    );
    let scripts = result["scripts"].as_array().expect("scripts array");
    assert_eq!(scripts.len(), 5, "expected 5 scripts");
    // Script[1] has a type-0 (None) prop: RQData
    let script1_props = scripts[1]["properties"].as_array().expect("props");
    assert_eq!(script1_props.len(), 2);
    assert!(
        script1_props[1]["value"].is_null(),
        "RQData (type-0) must be null"
    );
    // Script[2] has a type-7 (Struct) prop: SyncedActorValueData with 2 members
    let script2_props = scripts[2]["properties"].as_array().expect("props");
    let struct_prop = script2_props
        .iter()
        .find(|p| p["name"] == "SyncedActorValueData")
        .expect("SyncedActorValueData prop");
    assert_eq!(struct_prop["type"], 7);
    let members = struct_prop["value"].as_array().expect("struct members");
    assert_eq!(members.len(), 2);
}
