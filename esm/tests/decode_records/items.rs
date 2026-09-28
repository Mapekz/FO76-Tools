//! Inventory items and entitlements: ALCH, ARMO, ARMA, BOOK, MISC, MSCS, NOTE, COEN.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{assert_fully_decoded, assert_record_type, decode_fixture};
use serde_json::{Value, json};

/// ALCH 0x000045C9 — `TrackingDart` — decodes to Ingestible fully.
///
/// 20-subrecord record spanning OBND, KSIZ/KWDA keyword block, MODL/MODT
/// model, ENIT effect data, and an EFID/EFIT magic effect entry.  Exercises
/// the keyword-array path (KSIZ count + KWDA payload) and the ENIT struct.
#[test]
fn alch_tracking_dart_decodes_correctly() {
    let result = decode_fixture(
        "ALCH",
        182,
        &[
            ("EDID", "547261636b696e674461727400"),
            ("OBND", "fffffaff0000010007000200"),
            ("PTRN", "16702400"),
            (
                "FULL",
                "3c49443d30303033444139463e547261636b696e67204461727400",
            ),
            ("KSIZ", "02000000"),
            ("KWDA", "6f5018008df95000"),
            ("MODL", "50726f70735c537972696e6765416d6d6f2e6e696600"),
            (
                "MODT",
                "0400000004000000000000000100000001000000595f8af364647300b7d70ce13a62850964647300b7d70ce1753e841d64647300b7d70ce16bd751fd64647300b7d70ce1329b0c2d6267736daeef2e19",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("YNAM", "479c2400"),
            (
                "DESC",
                "3c49443d30303033444141303e54617267657420656d697473206120747261636b696e67207369676e616c2e00",
            ),
            ("DATA", "0000803e"),
            (
                "ENIT",
                "28000000190000000000000000000000f2ba02000000000000000000",
            ),
            ("DNAM", "00"),
            ("EFID", "ca450000"),
            ("EFIT", "010000000000000000000000100e00000000000000000000"),
            ("DURG", "93bf2d00"),
            ("MIID", "01000000"),
        ],
    );

    assert_record_type(&result, "Ingestible");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Name").and_then(|v| v.as_str()),
        Some("Tracking Dart"),
        "Name"
    );
    assert_eq!(
        result.get("Description").and_then(|v| v.as_str()),
        Some("Target emits a tracking signal."),
        "Description"
    );

    // Weight is a standalone DATA float
    let weight = result
        .get("Weight")
        .and_then(|v| v.as_f64())
        .expect("Weight");
    assert!(
        (weight - 0.25).abs() < 1e-4,
        "Weight should be 0.25, got {weight}"
    );

    // Keyword block
    let kws = result
        .pointer("/Keywords/Keywords")
        .and_then(|v| v.as_array())
        .expect("Keywords.Keywords array");
    assert_eq!(kws.len(), 2, "expected 2 keywords");
}

/// Synthetic ALCH with two effects: each effect's optional trailing members
/// (`CVT0`/`MAGA`/`DURG`/`MAGG`/`EIES`/`CODG`/`CODV`) bind to that effect only,
/// so an effect with no `DURG`/`MAGG` of its own never takes a later effect's.
/// This is the live-ESM shape of ALCH `Psycho` (0x0003377D) and `Buffout`
/// (0x00033778), whose ghoul-only effect carries the `GLOB`.
#[test]
fn alch_two_effects_do_not_cross_contaminate_optional_trailers() {
    // EFIT payload at form_version 209 (>=183, so neither below_version-gated
    // `_unknown` field applies): Effect ID(u32) + Magnitude(f32) + Area(u32) +
    // Duration(u32) = 16 zeroed bytes. Values are irrelevant here — only
    // EFID/DURG/MAGG FormIDs are asserted.
    let result = decode_fixture(
        "ALCH",
        209,
        &[
            ("EDID", "54776f4566666563745465737400"),
            // Effect 0: EFID + EFIT only — no DURG/MAGG of its own.
            ("EFID", "3c2b1a00"), // Base Effect = 0x001A2B3C
            ("EFIT", "00000000000000000000000000000000"),
            // Effect 1: EFID + EFIT + DURG + MAGG.
            ("EFID", "7c6b5a00"), // Base Effect = 0x005A6B7C
            ("EFIT", "00000000000000000000000000000000"),
            ("DURG", "ccbbaa00"), // Duration GLOB = 0x00AABBCC
            ("MAGG", "ffeedd00"), // Magnitude GLOB = 0x00DDEEFF
        ],
    );

    assert_record_type(&result, "Ingestible");
    assert_fully_decoded(&result);

    let effects = result
        .get("Effects")
        .and_then(|v| v.as_array())
        .expect("Effects array must decode");
    assert_eq!(effects.len(), 2, "expected exactly 2 effects");

    // Effect 0 must have neither Duration nor Magnitude — it has no DURG/MAGG
    // of its own, and must not steal Effect 1's.
    assert_eq!(
        effects[0]
            .pointer("/Effect/Base Effect")
            .and_then(|v| v.as_str()),
        Some("0x001A2B3C"),
        "Effect 0 Base Effect"
    );
    assert!(
        effects[0].pointer("/Effect/Duration").is_none(),
        "Effect 0 must not have a Duration (it has no DURG of its own): {:?}",
        effects[0].pointer("/Effect/Duration")
    );
    assert!(
        effects[0].pointer("/Effect/Magnitude").is_none(),
        "Effect 0 must not have a Magnitude (it has no MAGG of its own): {:?}",
        effects[0].pointer("/Effect/Magnitude")
    );

    // Effect 1 must resolve Duration/Magnitude to its own DURG/MAGG FormIDs.
    assert_eq!(
        effects[1]
            .pointer("/Effect/Base Effect")
            .and_then(|v| v.as_str()),
        Some("0x005A6B7C"),
        "Effect 1 Base Effect"
    );
    assert_eq!(
        effects[1]
            .pointer("/Effect/Duration")
            .and_then(|v| v.as_str()),
        Some("0x00AABBCC"),
        "Effect 1 Duration must resolve to its own DURG FormID"
    );
    assert_eq!(
        effects[1]
            .pointer("/Effect/Magnitude")
            .and_then(|v| v.as_str()),
        Some("0x00DDEEFF"),
        "Effect 1 Magnitude must resolve to its own MAGG FormID"
    );
}

/// ARMO 0x00000D64 — `SkinNaked` — decodes to Armor fully.
///
/// 18-subrecord record with repeated INDX/MODL pairs (armor addon list) and
/// BOD2 biped-body template.  Exercises the indexed-model array and confirms
/// no extra subrecords are left unmapped.
#[test]
fn armo_skin_naked_decodes_correctly() {
    let result = decode_fixture(
        "ARMO",
        209,
        &[
            ("EDID", "536b696e4e616b656400"),
            ("OBND", "000000000000000000000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("BOD2", "38000000"),
            ("RNAM", "46370100"),
            ("DESC", "00"),
            ("INDX", "0000"),
            ("MODL", "6c0d0000"),
            ("INDX", "0000"),
            ("MODL", "670d0000"),
            ("DATA", "000000000000000000000000"),
            ("FNAM", "000000000000000000000000"),
            ("VCRY", "0f000000"),
        ],
    );

    assert_record_type(&result, "Armor");
    assert_fully_decoded(&result);

    let models = result
        .get("Models")
        .and_then(|v| v.as_array())
        .expect("Models array");
    assert_eq!(models.len(), 2, "expected 2 armor addon entries");
    assert_eq!(
        models[0]
            .pointer("/Model/Armor Addon")
            .and_then(|v| v.as_str()),
        Some("0x00000D6C"),
        "Models[0].Armor Addon"
    );
    assert_eq!(
        result.get("Value Currency").and_then(|v| v.as_str()),
        Some("0x0000000F"),
        "VCRY decodes as a Value Currency FormID, not raw bytes"
    );
}

/// ARMO 0x005DD339 — `Armor_BOSInfantry_Torso` — Brotherhood Recon Chest Piece
/// decodes to Armor fully.
///
/// 43-subrecord record; form_version 208.  Exercises model info (MOD2/MO2T),
/// dual Enlighten blocks, BOD2 biped template, KSIZ/KWDA keywords, damage
/// resistances (DAMA), appearance (APPR), 2-entry OBTE/OBTF/FULL/OBTS object
/// template chain, and CVT1–CVT3 curve refs.
#[test]
fn armo_bos_recon_chest_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x005DD339 --raw` (form_version 208).
    let result = decode_fixture(
        "ARMO",
        208,
        &[
            ("EDID", "41726d6f725f424f53496e66616e7472795f546f72736f00"),
            ("OBND", "f0fff1ff000010000c003100"),
            ("PTRN", "f1ea5e00"),
            (
                "FULL",
                "3c49443d34313030314242343e42726f74686572686f6f64205265636f6e20436865737420506965636500",
            ),
            (
                "MOD2",
                "41726d6f722f424f535f496e66616e7472792f424f535f496e66616e7472795f41726d6f725f546f72736f5f474f2e6e696600",
            ),
            ("MO2T", "0400000000000000000000000000000000000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("BOD2", "00080000"),
            ("RNAM", "46370100"),
            ("KSIZ", "0e000000"),
            (
                "KWDA",
                "cafa07003bd35d0045d35d007a9f4900f42e4500e94a0f0039c44300ecc006003ad35d00169a5200f6f55f00d6146200825418007ace7a00",
            ),
            ("DESC", "00"),
            ("INRD", "c14b1800"),
            ("EILV", "2800000032000000"),
            ("IBSD", "35d24e00"),
            ("INDX", "0000"),
            ("MODL", "34d35d00"),
            ("DATA", "3c000000d7a3204064000000"),
            ("FNAM", "0f0000000000000000000000"),
            (
                "DAMA",
                "810a060000000000d56b8400850a060000000000d06b8400870a060000000000e16b8400840a060000000000ce6b8400820a060000000000d46b8400830a060000000000d06b8400",
            ),
            (
                "APPR",
                "c6360500212802005a2e1800c8321e00a8894e00a9894e00aa894e00ab894e001e7043009bb91c00d814620064a24700",
            ),
            ("OBTE", "02000000"),
            ("OBTF", ""),
            ("FULL", "3c49443d38313030314438373e44656661756c7400"),
            (
                "OBTS",
                "030000000000000000000000ffff01000000ced65d000000019ce5180000000172f43c00000001",
            ),
            (
                "OBTS",
                "0400000000000000000000000000000181418a0000008a418a0000000148e54e00000001856d4f0000000160da8300000001",
            ),
            ("STOP", ""),
            ("CVT1", "97ab1f00"),
            ("CVT2", "c9bc1800"),
            ("CVT3", "565a3400"),
            ("ABPO", "bbe45400"),
            ("VCRY", "0f000000"),
        ],
    );

    assert_record_type(&result, "Armor");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("Armor_BOSInfantry_Torso"),
        "Editor ID"
    );
}

/// BOOK 0x00000871 — `recipe_mod_AssaultRifle_Receiver_FastTrigger-CritDMG` —
/// decodes to Book fully.
///
/// 19-subrecord record (EDID OBND PTRN XALG FULL MODL MODT ENLT ENLS AUUV
/// DESC YNAM KSIZ KWDA DATA DNAM CNAM INAM BTOF); form_version 185.  The XALG
/// subrecord is mapped in the BOOK schema (unlike GMRW where it is drift) —
/// this test confirms that XALG decodes cleanly here.
#[test]
fn book_assault_rifle_recipe_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x00000871 --raw` (form_version 185).
    let result = decode_fixture(
        "BOOK",
        185,
        &[
            (
                "EDID",
                "7265636970655f6d6f645f41737361756c745269666c655f52656365697665725f46617374547269676765722d43726974444d4700",
            ),
            ("OBND", "f9fff2ff00000d000e000100"),
            ("PTRN", "ad2e1e00"),
            ("XALG", "8000000000000000"),
            (
                "FULL",
                "3c49443d30303034313635303e506c616e3a2041737361756c74205269666c652046696572636520526563656976657200",
            ),
            (
                "MODL",
                "50726f70735c496e7374523033426c75655072696e742e6e696600",
            ),
            (
                "MODT",
                "040000000400000000000000010000000100000067993c1664647300b7d70ce104a433ec64647300b7d70ce14bf832f864647300b7d70ce15511e71864647300b7d70ce12f8f53536267736daeef2e19",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "0189da2f000048420000f04100009c429a99193fcdcccc3d00a8becb",
            ),
            ("DESC", "00"),
            ("YNAM", "0f4c1d00"),
            ("KSIZ", "02000000"),
            ("KWDA", "67153e0027433d00"),
            ("DATA", "fa0000000000803e"),
            ("DNAM", "20000000000000000000000000"),
            ("CNAM", "00"),
            ("INAM", "32491100"),
            ("BTOF", "00000000"),
        ],
    );

    assert_record_type(&result, "Book");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("recipe_mod_AssaultRifle_Receiver_FastTrigger-CritDMG"),
        "Editor ID"
    );

    // MODT decodes as a structured wbModelInfo blob (4 textures, 0 addon nodes, 1
    // material), not raw hex.
    let model_info = result
        .pointer("/Model/Model Information")
        .expect("Model Information");
    assert_eq!(model_info.pointer("/Counters/Textures"), Some(&json!(4)));
    assert_eq!(model_info.pointer("/Counters/Addon Nodes"), Some(&json!(0)));
    assert_eq!(model_info.pointer("/Counters/Materials"), Some(&json!(1)));
    assert_eq!(
        model_info
            .pointer("/Textures/0/Extension")
            .and_then(|v| v.as_str()),
        Some("dds"),
    );
    assert_eq!(
        model_info
            .pointer("/Materials/0/Extension")
            .and_then(|v| v.as_str()),
        Some("bgsm"),
    );
}

/// NOTE 0x00002CBA — `<no edid>` — basic decode regression.
#[test]
fn note_fs_jacob_part01_decodes_correctly() {
    let result = decode_fixture(
        "NOTE",
        192,
        &[
            ("EDID", "46535f4a61636f6250617274303100"),
            ("OBND", "fdfffeff0000030002000000"),
            ("PTRN", "a5950900"),
            ("SNTP", "74705300"),
            (
                "FULL",
                "3c49443d30303033443938383e4a61636f62277320486f6c6f7461706500",
            ),
            ("MODL", "50726f70735c486f6c6f746170655f50726f702e6e696600"),
            (
                "MODT",
                "0400000005000000010000000100000001000000aae4fd56646473007b24d06cc9d9f2ac646473007b24d06c223d2fc1646473007b24d06c8685f3b8646473007b24d06c986c2658646473007b24d06c4a040000bd0d0a246267736daeef2e19",
            ),
            ("YNAM", "a8bf0b00"),
            ("ZNAM", "a9bf0b00"),
            ("KSIZ", "01000000"),
            ("KWDA", "27433d00"),
            ("VCRY", "0f000000"),
            ("DNAM", "01"),
            ("DATA", "0000000000000000"),
            ("SNAM", "5b902a00"),
        ],
    );

    assert_record_type(&result, "Note");
    assert_fully_decoded(&result);
}

/// MISC 0x0000000A — `<no edid>` — basic decode regression.
#[test]
fn misc_bobby_pin_decodes_correctly() {
    let result = decode_fixture(
        "MISC",
        187,
        &[
            ("EDID", "426f62627950696e00"),
            ("OBND", "fbfffeff0000030004000000"),
            ("PTRN", "8b882400"),
            ("FULL", "3c49443d30303031444236423e426f6262792050696e00"),
            ("MODL", "50726f70735c426f62627950696e2e6e696600"),
            (
                "MODT",
                "04000000040000000000000001000000010000004054c76664647300cfe14e232369c89c64647300cfe14e236c35c98864647300cfe14e2372dc1c6864647300cfe14e23056489426267736d528aab77",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("YNAM", "649d2400"),
            ("KSIZ", "02000000"),
            ("KWDA", "47940e006ac41c00"),
            ("DATA", "050000006f12833a"),
            ("AQIC", "000000000000baba"),
        ],
    );

    assert_record_type(&result, "Misc. Item");
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("BobbyPin"),
    );
}

/// ARMA `0x00156CB9` (`AAClothesPreWarHouseDress`) shape: the "Biped Model"
/// rstruct's `Male` group has no model of its own (`MOD2`/`MO2T`/`MO2C`/`MO2S`
/// absent), so its `ENLT`/`ENLS`/`AUUV` open it, and the `XFLG` after
/// `Female`'s `MOD3` belongs to `Female`. Nothing is left `_unmapped`.
#[test]
fn arma_biped_model_male_missing_model_does_not_strand_scoped_members() {
    let result = decode_fixture(
        "ARMA",
        209,
        &[
            ("EDID", "4d696e53636f706500"), // "MinScope\0"
            // Male biped group: no MOD2/MO2T/MO2C/MO2S — no model at all, only
            // the Enlighten trio. Its own ENLS = 1.0.
            ("ENLT", "aabbccdd"),
            ("ENLS", "0000803f"), // 1.0
            ("AUUV", &"00".repeat(28)),
            // Female biped group: has its own model (MOD3) *and* the only XFLG
            // in the record, both appearing before its own Enlighten trio. Its
            // own ENLS = 2.0 (deliberately different from Male's, to catch
            // cross-group contamination).
            ("MOD3", "46656d616c652e6e696600"), // "Female.nif\0"
            ("MO3T", "0100000000000000"),       // model_info: 1 counter, 0 textures
            ("XFLG", "02"),
            ("ENLT", "11223344"),
            ("ENLS", "00000040"), // 2.0
            ("AUUV", &"00".repeat(28)),
            ("MO3F", "00"),
        ],
    );
    assert_record_type(&result, "Armor Addon");
    assert_fully_decoded(&result);

    let biped = result
        .get("Biped Model")
        .and_then(|v| v.as_object())
        .expect("Biped Model must decode");

    let male_enls = biped
        .get("Male")
        .and_then(|v| v.get("Enlighten Simplified Shape Scale"))
        .and_then(|v| v.as_f64());
    assert_eq!(male_enls, Some(1.0), "Male's own ENLS must not be stranded");

    let female = biped.get("Female").expect("Female group must decode");
    assert_eq!(
        female.get("Model Filename").and_then(|v| v.as_str()),
        Some("Female.nif"),
        "Female's own MOD3 must not be stranded"
    );
    assert_eq!(
        female
            .get("Enlighten Simplified Shape Scale")
            .and_then(|v| v.as_f64()),
        Some(2.0),
        "Female's own ENLS must be its own value, not Male's"
    );
}

/// MSCS 0x001122E8 — `TEST_miscItems` (Misc Item Spawner). form_version 157.
/// The biggest of the 3 `TEST_*` MSCS records in the reference ESM — 13 `SPWN` entries.
/// TES5Edit already defines MSCS fully (`../TES5Edit/Core/wbDefinitionsFO76.pas:16411`);
/// this is a plain safelist addition (`tools/extractor/extract.py`'s `SAFELIST`), no
/// hand-authored schema — this test just guards it stays clean.
#[test]
fn mscs_test_misc_items_decodes_correctly() {
    // Verbatim subrecords from `esm get 0x001122e8 --raw`.
    let result = decode_fixture(
        "MSCS",
        157,
        &[
            ("EDID", "544553545f6d6973634974656d7300"),
            ("OBND", "f8fffdff0000080003001b00"),
            ("DATA", ""),
            ("SPWN", "ab300100"),
            ("SPWN", "ac300100"),
            ("SPWN", "a8300100"),
            ("SPWN", "a1300100"),
            ("SPWN", "a0300100"),
            ("SPWN", "a9300100"),
            ("SPWN", "aa300100"),
            ("SPWN", "ada00800"),
            ("SPWN", "969e1c00"),
            ("SPWN", "929e1c00"),
            ("SPWN", "317f1c00"),
            ("SPWN", "730e0600"),
            ("SPWN", "279b0500"),
        ],
    );
    assert_record_type(&result, "Misc Item Spawner");
    assert_fully_decoded(&result);

    let spawns = result
        .get("Spawns")
        .and_then(|v| v.as_array())
        .expect("Spawns must decode");
    assert_eq!(spawns.len(), 13, "expected exactly 13 SPWN entries");
}

/// COEN `ETGR` is a 0-byte marker; its presence is the whole value.
#[test]
fn coen_etgr_marker_decodes() {
    // ATX_COEN_Character_LevelBoost_LevelUp (0x008DC1E8).
    let result = decode_fixture(
        "COEN",
        210,
        &[
            (
                "EDID",
                "4154585f434f454e5f4368617261637465725f4c6576656c426f6f73745f4c6576656c55\
             7000",
            ),
            (
                "FULL",
                "3c49443d38313031323843413e43686172616374657220426f6f7374204c6576656c2055\
             7000",
            ),
            (
                "DESC",
                "3c49443d38313031323843423e4c6576656c20757020796f757220636861726163746572\
             210d0a0d0a2d20424f4f5354204953204c4f535420494620434841524143544552204953\
             2044454c455445442e2d00",
            ),
            ("KSIZ", "02000000"),
            ("KWDA", "7bc67b0040dd6000"),
            ("FNAM", "01000000"),
            ("NNAM", "3c49443d38313031323843433e4c6576656c20557000"),
            ("ETGR", ""),
            (
                "ETIP",
                "54657874757265732f4154582f53746f726566726f6e742f4c6576656c426f6f73742f00",
            ),
            (
                "ETDI",
                "4154585f4368617261637465725f4c6576656c426f6f73745f4c6576656c55702e646473\
             00",
            ),
            (
                "ECIL",
                "4154585f4368617261637465725f4c6576656c426f6f73745f4c6576656c55705f43312e\
             64647300",
            ),
        ],
    );

    assert_record_type(&result, "Consumable Entitlement");
    assert_fully_decoded(&result);
    assert!(
        result.get("ETGR - Unknown").is_some_and(Value::is_null),
        "a present ETGR marker renders as null"
    );
}
