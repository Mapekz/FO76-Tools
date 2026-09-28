//! Magic and special effects: MGEF, ENCH, SPEL, EXPL, PROJ.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{assert_fully_decoded, assert_record_type, bare_ctx, decode_fixture};
use esm::decode::decode_record;
use esm::format::Signature;
use esm::reader::OwnedSubrecord;
use esm::schema::Schema;

/// MGEF DATA decodes to the expected structure with all fields correctly aligned.
///
/// Uses the real embedded schema with a synthetic DATA payload so no game file
/// is required. Catches structural regressions as the decoder evolves:
///
///   - Fields after the `Spellmaking` nested struct (offset 48, 8 bytes) must
///     be present and correctly positioned. If the nested-struct pos-advance
///     regresses, every field from Taper Curve onward shifts 8 bytes early and
///     "Explosion" would read from the Actor Value slot.
///
///   - Both `wbActorValue` union slots (offsets 72 and 92) must appear under
///     distinct keys. If the duplicate-name de-dup regresses, the second slot
///     silently overwrites the first.
#[test]
fn mgef_data_decodes_correct_structure() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx(&schema);

    // 96-byte DATA payload (form_version 208: Flags 2 present, Actor Value is
    // an AVIF FormID). Actor Value (offset 72) and Explosion (offset 80) carry
    // distinct non-null values so the test can tell them apart.
    let mut payload = vec![0u8; 96];
    payload[72..76].copy_from_slice(&1u32.to_le_bytes()); // Actor Value  = FormID(1)
    payload[80..84].copy_from_slice(&2u32.to_le_bytes()); // Explosion    = FormID(2)

    let subrecords = vec![OwnedSubrecord {
        signature: Signature::from_slice(b"DATA"),
        data: payload,
        doc_index: 0,
    }];

    let result = decode_record(&ctx, "MGEF", &subrecords);
    assert_record_type(&result, "Magic Effect");
    // Only DATA is provided; the full record is covered by the CLEAN_TYPES sweep.
    // This test guards the DATA structural invariants independently of game data.
    assert_fully_decoded(&result);

    let data = result
        .get("Magic Effect Data")
        .and_then(|v| v.get("Data"))
        .and_then(|v| v.as_object())
        .expect("Magic Effect Data.Data must decode");

    // All fields that follow the Spellmaking nested struct must be present.
    for field in [
        "Taper Curve",
        "Taper Duration",
        "Second AV Weight",
        "Archetype",
        "Actor Value",
        "Projectile",
        "Explosion",
        "Casting Type",
        "Delivery",
        "Actor Value 2",
    ] {
        assert!(
            data.contains_key(field),
            "'{field}' must be present after Spellmaking"
        );
    }

    // Archetype at offset 68 must decode as Value Modifier (0), not shifted
    // to whatever bytes were at offset 60 before the alignment fix.
    assert_eq!(
        data.get("Archetype")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str()),
        Some("Value Modifier"),
    );

    // Actor Value (offset 72) and Explosion (offset 80) must occupy different
    // byte positions.
    assert_ne!(
        data.get("Actor Value"),
        data.get("Explosion"),
        "Actor Value and Explosion must be distinct fields"
    );

    // The second wbActorValue slot must survive as "Actor Value 2" without
    // clobbering the primary "Actor Value".
    assert_ne!(
        data.get("Actor Value"),
        data.get("Actor Value 2"),
        "both Actor Value slots must have distinct output keys"
    );
}

/// PROJ 0x000021E1 — `ProjectileAudioGrenade` — decodes to Projectile fully.
///
/// 13-subrecord record with a DEST/DSTD/DSTF destructible block and a large
/// DNAM payload.  Exercises the destructible-object sub-struct path and the
/// projectile data (Type enum, Speed, Gravity, Range).
#[test]
fn proj_audio_grenade_decodes_correctly() {
    let result = decode_fixture(
        "PROJ",
        175,
        &[
            ("EDID", "50726f6a656374696c65417564696f4772656e61646500"),
            ("OBND", "fffff9fffeff020005000200"),
            (
                "FULL",
                "3c49443d30303033443935373e4372796f67656e6963204772656e61646500",
            ),
            (
                "MODL",
                "576561706f6e735c4772656e6164655c4372796f4772656e61646550726f6a656374696c652e6e696600",
            ),
            (
                "MODT",
                "040000000400000000000000010000000100000012b6e1c46464730060d0c0b4718bee3e6464730060d0c0b43ed7ef2a6464730060d0c0b4203e3aca6464730060d0c0b4ca5ecd996267736d026bace5",
            ),
            ("DEST", "050000000100000000000000"),
            (
                "DSTD",
                "00000006000000007fa5170000000000000000000000000000000000",
            ),
            ("DSTF", ""),
            ("DATA", ""),
            (
                "DNAM",
                "060002000000003f0000af4400606a46000000000000000000000000000020400000000000000000cdcc4c3d0000003f00000000000000000000000000000000000000000000a0400000c8420000803e00000000668708000000000000",
            ),
            (
                "NAM1",
                "456666656374735c4d757a4d616368696e6547756e30312e6e696600",
            ),
            (
                "NAM2",
                "0400000004000000030000000200000000000000855b0aef6464730038973cead18c32ea6464730038973cea51edc5736464730038973ceae66605156464730038973cea000000000100000011000000",
            ),
            ("VNAM", "01000000"),
        ],
    );

    assert_record_type(&result, "Projectile");
    assert_fully_decoded(&result);

    let data = result.get("Data").expect("Data struct must decode");
    assert_eq!(
        data.pointer("/Type/name").and_then(|v| v.as_str()),
        Some("Lobber"),
        "Data.Type"
    );
    let speed = data.get("Speed").and_then(|v| v.as_f64()).expect("Speed");
    assert!(
        (speed - 1400.0).abs() < 0.1,
        "Speed should be 1400, got {speed}"
    );
    let gravity = data
        .get("Gravity")
        .and_then(|v| v.as_f64())
        .expect("Gravity");
    assert!(
        (gravity - 0.5).abs() < 1e-4,
        "Gravity should be 0.5, got {gravity}"
    );
}

/// ENCH 0x00002B4F — `zzzcrEnchFireflyIchorSplatterFX` — decodes to
/// Enchantment fully.
///
/// 8-subrecord record (EDID OBND ENIT EFID EFIT MAGF CODV MIID); form_version
/// 208.  Exercises the ENIT struct and magic-effect entry (EFID/EFIT) without
/// any raw fallbacks.  This test locks the type as clean.
#[test]
fn ench_firefly_ichor_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x00002B4F --raw` (form_version 208).
    let result = decode_fixture(
        "ENCH",
        208,
        &[
            (
                "EDID",
                "7a7a7a6372456e636846697265666c794963686f7253706c6174746572465800",
            ),
            ("OBND", "000000000000000000000000"),
            (
                "ENIT",
                "000000000100000000000000000000000000000006000000000000000000000000000000",
            ),
            ("EFID", "502b0000"),
            ("EFIT", "000000000000e0400000000005000000"),
            ("MAGF", "00000000"),
            ("CODV", "00000000"),
            ("MIID", "00000000"),
        ],
    );

    assert_record_type(&result, "Enchantment");
    assert_fully_decoded(&result);

    // Effects[0].Effect.Base Effect = 0x00002B50 (EFID value).
    assert_eq!(
        result
            .pointer("/Effects/0/Effect/Base Effect")
            .and_then(|v| v.as_str()),
        Some("0x00002B50"),
        "first effect Base Effect FormID"
    );
}

/// ENCH 0x00900A5A — `EnchPerkConcentratedFire` — decodes to Enchantment fully,
/// including the newer-than-reference Keywords block (KSIZ + KWDA).
///
/// 12-subrecord record (EDID OBND FULL KSIZ KWDA ENIT EFID EFIT MAGA MAGF CODV
/// MIID); form_version 209.  KSIZ/KWDA are absent from the Pascal reference but
/// present in the live ESM — covered by an `append` override.  This test
/// locks that path clean.
#[test]
fn ench_perk_concentrated_fire_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x00900A5A --raw` (form_version 209).
    let result = decode_fixture(
        "ENCH",
        209,
        &[
            ("EDID", "456e63685065726b436f6e63656e7472617465644669726500"),
            ("OBND", "000000000000000000000000"),
            (
                "FULL",
                "3c49443d33393239443530453e436f6e63656e747261746564204669726500",
            ),
            ("KSIZ", "01000000"),
            ("KWDA", "d18b8600"),
            (
                "ENIT",
                "000000000100000000000000000000000000000006000000000000000000000000000000",
            ),
            ("EFID", "5c0a9000"),
            ("EFIT", "01000000000000000000000000000000"),
            ("MAGA", "590a9000"),
            ("MAGF", "00000000"),
            ("CODV", "00000000"),
            ("MIID", "01000000"),
        ],
    );

    assert_record_type(&result, "Enchantment");
    assert_fully_decoded(&result);

    // Keywords[0] = 0x00868BD1 (KWDA value d18b8600 little-endian).
    assert_eq!(
        result
            .pointer("/Keywords/Keywords/0")
            .and_then(|v| v.as_str()),
        Some("0x00868BD1"),
        "first keyword FormID"
    );
}

/// SPEL 0x00004381 — `AbNemesisRank00` — decodes to Spell fully.
///
/// Simple ability spell: EDID, OBND, ETYP, DESC, SPIT + one effect (EFID/EFIT).
/// Exercises the SPIT Data struct (Type = Ability) and the Effects array.
/// form_version 131.
///
/// Verbatim subrecords from `esm get <esm> 0x00004381 --raw`.
#[test]
fn spel_ab_nemesis_rank00_decodes_correctly() {
    let result = decode_fixture(
        "SPEL",
        131,
        &[
            ("EDID", "41624e656d6573697352616e6b303000"),
            ("OBND", "000000000000000000000000"),
            ("ETYP", "443f0100"),
            ("DESC", "00"),
            (
                "SPIT",
                "000000000000000004000000000000000000000000000000000000000000000000000000",
            ),
            ("EFID", "82430000"),
            ("EFIT", "0000f0410000000000000000"),
        ],
    );

    assert_record_type(&result, "Spell");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("AbNemesisRank00"),
        "Editor ID"
    );
    // SPIT Type field: 4 = Ability.
    assert_eq!(
        result.pointer("/Data/Type/name").and_then(|v| v.as_str()),
        Some("Ability"),
        "Spell Type"
    );
    // One EFID/EFIT pair → one entry in Effects.
    let effects = result
        .get("Effects")
        .and_then(|v| v.as_array())
        .expect("Effects must be an array");
    assert_eq!(effects.len(), 1, "Effects count");
    assert_eq!(
        effects[0]
            .pointer("/Effect/Base Effect")
            .and_then(|v| v.as_str()),
        Some("0x00004382"),
        "Effects[0] Base Effect FormID"
    );
}

/// EXPL 0x000001F5 — `ExplosionDefaultWater` — decodes to Explosion fully.
///
/// Six subrecords: EDID, OBND, DESC, MODL, MODT, DATA (92 bytes).  Exercises
/// the DATA struct including the Sound 1 FormID and Sound Level enum.
/// form_version 202.
///
/// Verbatim subrecords from `esm get <esm> 0x000001F5 --raw`.
#[test]
fn expl_explosion_default_water_decodes_correctly() {
    let result = decode_fixture(
        "EXPL",
        202,
        &[
            ("EDID", "4578706c6f73696f6e44656661756c74576174657200"),
            ("OBND", "01fff7fe58ffff00f400bf00"),
            ("DESC", "00"),
            (
                "MODL",
                "456666656374735c556e64657257617465724578706c6f73696f6e2e6e696600",
            ),
            (
                "MODT",
                "04000000050000000000000005000000000000003450abb06464730038973cea\
             1bd7c6c464647300582c55330d670eb76464730038973ceaf10eee3e6464730038973cea\
             3ea5ff086464730038973cea",
            ),
            (
                "DATA",
                "000000002c420500000000000000000000000000000000000000000000000000\
             000000000000000000000000000000000000000000000000010000000000000000\
             000000000000000000000000000000000000000000000000000000",
            ),
        ],
    );

    assert_record_type(&result, "Explosion");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("ExplosionDefaultWater"),
        "Editor ID"
    );
    // DATA offset 4: Sound 1 = FormID 0x0005422C.
    assert_eq!(
        result.pointer("/Data/Sound 1").and_then(|v| v.as_str()),
        Some("0x0005422C"),
        "DATA Sound 1"
    );
    // Sound Level enum: 1 = Normal.
    assert_eq!(
        result
            .pointer("/Data/Sound Level/name")
            .and_then(|v| v.as_str()),
        Some("Normal"),
        "Sound Level"
    );
}
