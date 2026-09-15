//! Placed-world and region records: FLOR, FURN, CONT, TERM, SCOL, REGN.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{assert_fully_decoded, assert_record_type, decode_fixture};
use serde_json::Value;

/// TERM 0x00001676 — `<no edid>` — basic decode regression.
#[test]
fn term_intel_room_exams_sub_terminal_decodes_correctly() {
    let result = decode_fixture(
        "TERM",
        201,
        &[
            (
                "EDID",
                "454e30325f496e74656c526f6f6d4578616d735375625465726d696e616c303300",
            ),
            (
                "VMAD",
                "0600020001001700454e30325f4578616d5175657374696f6e5363726970740002000d00546172676574416e7377657273110105000000020000000c0069416e7377657256616c75650301010000000700694d656e754944030101000000020000000c0069416e7377657256616c75650301030000000700694d656e754944030102000000020000000c0069416e7377657256616c75650301020000000700694d656e754944030103000000020000000700694d656e7549440301040000000c0069416e7377657256616c7565030100000000020000000c0069416e7377657256616c75650301010000000700694d656e7549440301050000000b0064656a614368616e6e656c02010400454e3032",
            ),
            ("OBND", "ecffdeff0000130010006100"),
            (
                "NAM0",
                "3c49443d30303033433833353e5768697465737072696e675f4e6574202d2d20762e303800",
            ),
            (
                "FULL",
                "3c49443d30303033433833363e5175657374696f6e6e61697265205465726d696e616c00",
            ),
            (
                "MODL",
                "4675726e69747572655c5465726d696e616c735c5465726d696e616c436f6e736f6c654f6e2e6e696600",
            ),
            (
                "MODT",
                "0400000019000000000000000a000000050000000f192fb864647300514b85776c24204264647300514b85772378215664647300514b8577886afdda64647300b5aa259deb57f22064647300b5aa259da40bf33464647300b5aa259df399858864647300b5aa259d90a48a7264647300b5aa259ddff88b6664647300b5aa259d74e31e5d64647300514b857717de11a764647300514b8577588210b364647300514b8577f0a1333264647300514b8577939c3cc864647300514b8577dcc03ddc64647300514b8577c229e83c64647300514b8577466bc55364647300514b8577839c30dc6464730038973ceac1115e8664647300b5aa259dbae226d464647300b5aa259db0a5a86e64647300b5aa259d3d91f4b664647300514b8577ff6512fd6464730038973cea7eef02fc64647300582c5533c80fd0fc6464730038973cea9a7f25c36267736de25d598b1f0637246267736de25d598b404118cb6267736d6070f9eddaa82f006267736d6070f9ed7de3679b6267736dde5ee364",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("KSIZ", "03000000"),
            ("KWDA", "e686020012cb0f00d7560a00"),
            ("PNAM", "cc4c3300"),
            ("AIID", "00"),
            ("FNAM", "0000"),
            ("PAHD", "00000000"),
            ("CTRN", "00000000000000000000000000"),
            ("COCT", "00000000"),
            ("MNAM", "01000040"),
            ("WBDT", "0000"),
            (
                "XMRK",
                "4d61726b6572735c4d61726b65724465736b5465726d696e616c30312e6e696600",
            ),
            ("ZNAM", "00000000000088c2000000000000000000000000ff010000"),
            ("FFEF", "00000000"),
            ("BSIZ", "01000000"),
            (
                "BTXT",
                "3c49443d30303033433833373e5c5c5c5c20504552534f4e414c20494e464f524d4154494f4e202f2f2f2f0d0a0d0a5768696368206f662074686520666f6c6c6f77696e67207468696e6b657273272062656c6965662073797374656d73206d6f737420636c6f73656c79206d61746368657320796f7572206f776e3f0d0a5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f0d0a00",
            ),
            ("TDAT", "0100"),
            ("ISIZ", "05000000"),
            (
                "ITXT",
                "3c49443d30303033433833383e53742e2054686f6d617320417175696e617300",
            ),
            (
                "RNAM",
                "3c49443d30303033433833393e2e2e2e616e73776572207265636f726465642e2e2e00",
            ),
            ("ANAM", "04"),
            ("ITID", "0100"),
            ("TNAM", "45160000"),
            ("ITXT", "3c49443d30303033433833413e4164616d20536d69746800"),
            (
                "RNAM",
                "3c49443d30303033433833423e2e2e2e20616e73776572207265636f72646564202e2e2e00",
            ),
            ("ANAM", "04"),
            ("ITID", "0200"),
            ("TNAM", "45160000"),
            (
                "ITXT",
                "3c49443d30303033433833433e4a6f686e20537475617274204d696c6c00",
            ),
            (
                "RNAM",
                "3c49443d30303033433833443e2e2e2e20616e73776572207265636f72646564202e2e2e00",
            ),
            ("ANAM", "04"),
            ("ITID", "0300"),
            ("TNAM", "45160000"),
            ("ITXT", "3c49443d30303034353133343e4b61726c204d61727800"),
            (
                "RNAM",
                "3c49443d30303034353133353e2e2e2e20616e73776572207265636f72646564202e2e2e00",
            ),
            ("ANAM", "04"),
            ("ITID", "0400"),
            ("TNAM", "45160000"),
            (
                "ITXT",
                "3c49443d30303034353133363e456c76697320507265736c657900",
            ),
            (
                "RNAM",
                "3c49443d30303034353133373e2e2e2e20616e73776572207265636f72646564202e2e00",
            ),
            ("ANAM", "04"),
            ("ITID", "0500"),
            ("TNAM", "45160000"),
        ],
    );

    assert_record_type(&result, "Terminal");
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("EN02_IntelRoomExamsSubTerminal03"),
    );
}

/// FLOR 0x000017F8 — `<no edid>` — basic decode regression.
#[test]
fn flor_firecap_decodes_correctly() {
    let result = decode_fixture(
        "FLOR",
        192,
        &[
            ("EDID", "5573654c50495f466c6f726146697265436170303100"),
            ("OBND", "e9ffe8fffcff170018002c00"),
            (
                "OPDS",
                "0100000000000000000000000000803f0000803ec2b8b23dc2b8b23dc2b8323ec2b8323edb0fc93fdb0f4940",
            ),
            ("PTRN", "521a4400"),
            (
                "OPDS",
                "0100000000000000000000000000803f0000803ec2b8b23dc2b8b23dc2b8323ec2b8323edb0fc93fdb0f4940",
            ),
            ("DEFL", "2cd61e00"),
            ("FULL", "3c49443d30303033454236413e4669726563617000"),
            (
                "MODL",
                "6c616e6473636170652f706c616e74732f6669726563617030312e6e696600",
            ),
            (
                "MODT",
                "0400000004000000000000000100000001000000eb474cff646473008a103af0887a4305646473008a103af0c7264211646473008a103af0d9cf97f1646473008a103af037df72446267736dd28d2cde",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01774a62000048420000f04100009c429a99193fcdcccc3d00d4a379",
            ),
            ("KSIZ", "03000000"),
            ("KWDA", "35fe2e0027724e000e684100"),
            ("PRPS", "834348000000803f00000000"),
            ("PNAM", "cc4c3300"),
            ("ATTX", "3c49443d30303033454236423e4861727665737400"),
            ("FNAM", "0000"),
            ("PFIG", "fffc0500"),
            ("SNAM", "053d2200"),
            ("CITC", "00000000"),
            ("FLFG", "00000000"),
            ("FMAH", "00000000"),
            ("FMIH", "00000000"),
        ],
    );

    assert_record_type(&result, "Flora");
    assert_fully_decoded(&result);
}

/// FURN 0x00002C9E — `<no edid>` — basic decode regression.
#[test]
fn furn_power_armor_raider_decodes_correctly() {
    let result = decode_fixture(
        "FURN",
        209,
        &[
            (
                "EDID",
                "506f77657241726d6f724675726e69747572655261696465724d544e5a303400",
            ),
            (
                "VMAD",
                "06000200010012004d544e5a30345f41726d6f725363726970740003000f004d544e5a30345f4d656c74646f776e01010000ffff942c00001d004d544e5a30345f4d656c74646f776e5f51756573745f4b6579776f726401010000ffffa22c00000b0041726d6f724c6f636b6564050101",
            ),
            ("OBND", "d8ffe1ff000028000e008a00"),
            ("FULL", "3c49443d30303033453942313e506f7765722041726d6f7200"),
            (
                "MODL",
                "4675726e69747572655c506f77657241726d6f725c4368617261637465724173736574735c506f77657241726d6f724675726e69747572652e6e696600",
            ),
            (
                "MODT",
                "040000000400000000000000010000000100000078ebd765646473000024eaaa1bd6d89f646473000024eaaa548ad98b646473000024eaaa4a630c6b646473000024eaaac4f8f4ef6267736d5beb74cf",
            ),
            ("ENLM", "01000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("KSIZ", "04000000"),
            ("KWDA", "0b430300f6a20b0002ff160048f81200"),
            ("DESC", "00"),
            ("PNAM", "00000000"),
            ("AIID", "00"),
            ("ATTX", "3c49443d30303033453942323e456e74657200"),
            ("FNAM", "0200"),
            ("PAHD", "00000000"),
            ("CTRN", "00000000000000000000000000"),
            ("COCT", "02000000"),
            ("CNTO", "43ae180001000000"),
            ("CNTO", "027b380001000000"),
            ("MNAM", "03000040"),
            ("WBDT", "0800"),
            (
                "XMRK",
                "4675726e69747572655c506f77657241726d6f725c4368617261637465724173736574735c506f77657241726d6f724675726e69747572652e6e696600",
            ),
            (
                "ZNAM",
                "0000000000000000000000000000000000000000ff0100000000000042a097c20000000000000000a8bd0500ff010000",
            ),
            (
                "APPR",
                "8c5f05008d5f05008e5f05008f5f0500905f0500fcda030020670500",
            ),
            ("OBTE", "01000000"),
            (
                "OBTS",
                "050000000000000000000000ffff000000001f6705000000017d7b13000000017e7b13000000017f7b1300000001807b1300000001",
            ),
            ("STOP", ""),
            ("FFEF", "00000000"),
            ("NVNM", ""),
        ],
    );

    assert_record_type(&result, "Furniture");
    assert_fully_decoded(&result);
}

/// CONT 0x0000FFEC — `DropCrate_Govt01` — decodes to Container fully.
///
/// 20-subrecord record; form_version 204.  Exercises the Enlighten block
/// (ENLM/ENLT/ENLS/AUUV), COCT/CNTO items array (2 entries), DATA flags +
/// weight, and the sound-ref triad (SNAM/QNAM/TNAM).  PHST and PTRN are also
/// present.  The CMIC subrecord decodes as a raw-hex Unknown field (`_raw: true`
/// but no `reason`) — this is intentional schema behaviour, not a fallback.
///
/// Verbatim subrecords from `esm get <esm>
/// --formid 0x0000FFEC --raw` (form_version 204).
#[test]
fn cont_drop_crate_govt01_decodes_correctly() {
    let result = decode_fixture(
        "CONT",
        204,
        &[
            ("EDID", "44726f7043726174655f476f7674303100"),
            ("OBND", "c1ffbcfff9ff330044004c00"),
            ("PTRN", "c0812300"),
            ("PHST", "08000000"),
            (
                "FULL",
                "3c49443d30303033463830343e476f7665726e6d656e74204169642044726f7000",
            ),
            (
                "MODL",
                "70726f70735c61697264726f70706564636f6e7461696e65725c61697264726f70706564636f6e7461696e65725f706879736963732e6e696600",
            ),
            (
                "MODT",
                "040000000c000000010000000600000002000000ef4b647d64647300be643c4c8c766b8764647300be643c4c748c06956464730038973cea85c0394b64647300e51f8366e6fd36b164647300e51f8366a9a137a564647300e51f8366b748e24564647300e51f83667da32aa66464730038973ceaf8d3913a6464730038973cea97a958b064647300582c5533a6a09d25646473007801bba00cd0f9ed6464730038973cea290100003a9e57cc6267736d199024220c9806106267736d466cc278",
            ),
            ("ENLM", "03000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("COCT", "02000000"),
            ("CNTO", "f099110001000000"),
            ("CNTO", "5de8590001000000"),
            ("DATA", "0000000000"),
            ("SNAM", "8b534500"),
            ("QNAM", "8c534500"),
            ("TNAM", "5d360200"),
            ("CMIC", "00000000"),
        ],
    );

    assert_record_type(&result, "Container");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("DropCrate_Govt01"),
        "Editor ID"
    );

    // COCT = 2: Items array must have exactly 2 entries.
    assert_eq!(
        result.get("Count").and_then(|v| v.as_u64()),
        Some(2),
        "Count"
    );
    let items = result
        .get("Items")
        .and_then(|v| v.as_array())
        .expect("Items must be an array");
    assert_eq!(items.len(), 2, "Items length");

    // First CNTO: Item FormID 0x001199F0, Count 1.
    assert_eq!(
        items[0].pointer("/Item/Item/Item").and_then(|v| v.as_str()),
        Some("0x001199F0"),
        "Items[0] FormID"
    );
    assert_eq!(
        items[0]
            .pointer("/Item/Item/Count")
            .and_then(|v| v.as_u64()),
        Some(1),
        "Items[0] Count"
    );

    // Sound refs decoded as FormIDs.
    assert_eq!(
        result.get("Sound - Open").and_then(|v| v.as_str()),
        Some("0x0045538B"),
        "Sound - Open"
    );
    assert_eq!(
        result.get("Sound - Close").and_then(|v| v.as_str()),
        Some("0x0045538C"),
        "Sound - Close"
    );
}

/// first part every part's placements and left the rest empty.
#[test]
fn scol_placements_stay_with_their_part() {
    // ClutterSC_RobCoPedTrim01 (0x00000B98): three parts whose DATA holds one,
    // two and four 28-byte placements.
    let result = decode_fixture(
        "SCOL",
        193,
        &[
            ("EDID", "436c757474657253435f526f62436f5065645472696d303100"),
            ("OBND", "e0fe60ff80ff2001a0000200"),
            (
                "MODL",
                "53434f4c5c536576656e74795369782e65736d5c434d30303030304239382e4e494600",
            ),
            (
                "MODT",
                "0400000011000000000000000500000004000000c2872c64646473009f9c1cd8a1ba239e\
             646473009f9c1cd8eee6228a646473009f9c1cd8c0c019e9646473009f9c1cd8a3fd1613\
             646473009f9c1cd8eca11707646473009f9c1cd8dd8f37b5646473009f9c1cd8beb2384f\
             646473009f9c1cd8f1ee395b646473009f9c1cd8c1c9db37646473009f9c1cd8a2f4d4cd\
             646473009f9c1cd8eda8d5d9646473009f9c1cd86c4dcdd5646473009f9c1cd8530a4725\
             64647300dcc67371ef07ecbb646473009f9c1cd8f248c2e7646473009f9c1cd8f00ff76a\
             646473009f9c1cd8dd8bd3446267736d2c8ac02451d6c1f86267736d2c8ac024d4c2af0f\
             6267736d2c8ac024b10acd9d6267736d2c8ac024",
            ),
            ("ENLM", "01000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("NAM1", "00000000"),
            ("LODP", "00000000"),
            ("ONAM", "1258030000000000"),
            (
                "DATA",
                "0000000000000000000000000000000000000000000000000000803f",
            ),
            ("ONAM", "880d080000000000"),
            (
                "DATA",
                "0000000000000000000000000000000000000080f00f49400000803f0000000000000000\
             000000000000000000000080010084360000803f",
            ),
            ("ONAM", "33410d0000000000"),
            (
                "DATA",
                "1e0000c30000303b000000000000000000000080e50f49400000803f1e0000c30000303b\
             000000000000000000000080f20fc93f0000803f08000043000080b90000000000000000\
             000000800400d0350000803f08000043000080b9000000000000000000000080e8cb9640\
             0000803f",
            ),
        ],
    );

    assert_record_type(&result, "Static Collection");
    assert_fully_decoded(&result);
    let parts = result
        .get("Parts")
        .and_then(Value::as_array)
        .expect("Parts must be an array");
    let placements: Vec<_> = parts
        .iter()
        .map(|p| {
            p.pointer("/Part/Placements")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        })
        .collect();
    assert_eq!(placements, [1, 2, 4]);
}

/// REGN `RDWC` is a cubemap .nif path inside the weather region data entry.
#[test]
fn regn_weather_entry_cubemap_scene_decodes() {
    // SundewGroveWeatherRegion (0x008B30A0).
    let result = decode_fixture(
        "REGN",
        209,
        &[
            ("EDID", "53756e64657747726f766557656174686572526567696f6e00"),
            ("RCLR", "0a0fc800"),
            ("WNAM", "15da2500"),
            ("RPLI", "00040000"),
            (
                "RPLD",
                "40cae84718d525c8498df747b09c25c8baf9f947968f2bc8a8f1e947001b2cc8",
            ),
            ("RPLI", "00040000"),
            (
                "RPLD",
                "63631a4884f68ec7f95c1b481c5c9cc7dce420487a089dc7eefa1f4897f38fc72f5b1d48\
             606c8ec7",
            ),
            ("RPLI", "00040000"),
            (
                "RPLD",
                "f61126483a44e4c7c9192b481015e5c73c302c48b824efc71c782948aeadf2c750212548\
             fed5eec7",
            ),
            ("RPLI", "00040000"),
            (
                "RPLD",
                "2d0c2f4893b625c8fc0b3248b0f623c800e13648040c24c8e4a03848a98b29c84c763748\
             d8e02bc80c0c314828362cc831cc2e48fda029c8",
            ),
            ("RDAT", "0300000000500000"),
            ("RDWT", ""),
            ("RDWR", "00"),
            (
                "RDWC",
                "536b792f437562656d61705363656e65732f53756e64657747726f76655f437562656d61\
             705363656e6530312e6e696600",
            ),
            ("RCBN", "01"),
        ],
    );

    assert_record_type(&result, "Region");
    assert_fully_decoded(&result);
    assert_eq!(
        result
            .pointer("/Region Data Entries/0/Region Data Entry/Cubemap Scene")
            .and_then(Value::as_str),
        Some("Sky/CubemapScenes/SundewGrove_CubemapScene01.nif")
    );
}
