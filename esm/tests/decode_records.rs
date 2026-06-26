mod common;

use common::{
    assert_fully_decoded, assert_only_drift_markers, bare_ctx, bare_ctx_fv, sr, subrecords_from,
};
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
    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Magic Effect"),
    );

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
    // byte positions — before the fix both would resolve from offset 72.
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

/// OMOD 0x0085B998 — `HTO_mod_Legendary_Weapon4_Tarnished` — decodes to the
/// expected structure and values.
///
/// This test pins the full DATA decode path that was broken by the
/// count-prefix width bug (CountPrefix read 1 byte; the correct width for
/// xEdit `-1` arrays is 4 bytes).  The OMOD DATA subrecord contains two
/// 4-byte-prefix inline arrays — `Attach Parent Slots` and `Items` — both
/// empty here.  Before the fix each was under-read by 3 bytes (6 bytes
/// total), misaligning `Includes` and `Properties` entirely.
///
/// The binary payload used here is the verbatim hex from `esm get
/// SeventySix_20260619.esm --formid 0x0085B998 --raw`.  The record uses
/// form_version 208 and has no compressed subrecords.
///
/// What is asserted (covering every field that the bug corrupted):
///   - `Include Count` / `Property Count` header fields
///   - `Form Type` resolves to the "Weapon" enum name
///   - `Includes[0]`: Mod FormID, Optional flag, Don't Use All flag
///   - `Properties[0–3]`: Value Type, Function Type, Property name (enum),
///     Value 1 (FormID string), Value 2 (integer), Curve Table (null)
#[test]
fn omod_legendary_weapon_data_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx(&schema);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x0085B998 --raw` (form_version 208, flags 0x10 = Legendary Mod).
    let subrecords = vec![
        sr(
            "EDID",
            "48544f5f6d6f645f4c6567656e646172795f576561706f6e345f5461726e697368656400",
            0,
        ),
        sr("DURL", "3000", 1),
        sr("FULL", "3c49443d37313031343730303e5461726e697368656400", 2),
        sr("DESC", "00", 3),
        sr("ENLT", "ffffffff", 4),
        sr("ENLS", "0000803f", 5),
        sr(
            "AUUV",
            "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            6,
        ),
        sr("INDX", "00", 7),
        // 131-byte DATA subrecord (verbatim from the ESM):
        //   +0   Include Count (u32 LE)              = 1
        //   +4   Property Count (u32 LE)             = 4
        //   +8   Unknown Bool 1 (u8)                 = 0 (False)
        //   +9   Unknown Bool 2 (u8)                 = 0 (False)
        //   +10  Form Type (u32 LE, sig bytes "WEAP") = 1346454871 → Weapon
        //   +14  Max Rank (u8, from_version 90)      = 0
        //   +15  Level Tier Offset (u8, fv 107)      = 0
        //   +16  Attach Point (FormID u32 LE)        = 0x004E89AA
        //   +20  Attach Parent Slots count (u32 LE)  = 0  ← 4-byte prefix (-1)
        //   +24  Items count (u32 LE)                = 0  ← 4-byte prefix (-1)
        //   +28  Includes[0] (7 bytes): Mod=0x004519F7, MinLevel=0, Opt=0, DontUseAll=1
        //   +35  Properties[0..3]: 4 × 24 bytes (see assertions below)
        sr(
            "DATA",
            concat!(
                "01000000", // Include Count = 1
                "04000000", // Property Count = 4
                "00",       // Unknown Bool 1 = False
                "00",       // Unknown Bool 2 = False
                "57454150", // Form Type = WEAP (Weapon)
                "00",       // Max Rank = 0
                "00",       // Level Tier Scaled Offset = 0
                "aa894e00", // Attach Point = 0x004E89AA
                "00000000", // Attach Parent Slots count (u32) = 0
                "00000000", // Items count (u32) = 0
                // Includes[0]: Mod, MinLevel, Optional, Don't Use All
                "f7194500", "00", "00", "01",
                // Property[0]: VT=4(FormID,Int) Func=2(ADD) Prop=65(Enchantments)
                //              Value1=0x0085B97F  Value2=1  Step=CurveTable(null)
                "04", "000000", "02", "000000", "4100", "0000", "7fb98500", "01000000", "00000000",
                // Property[1]: VT=4(FormID,Int) Func=2(ADD) Prop=31(Keywords)
                //              Value1=0x0085B984  Value2=2  Step=CurveTable(null)
                "04", "000000", "02", "000000", "1f00", "0000", "84b98500", "02000000", "00000000",
                // Property[2]: VT=4(FormID,Int) Func=2(ADD) Prop=31(Keywords)
                //              Value1=0x005380CA  Value2=2  Step=CurveTable(null)
                "04", "000000", "02", "000000", "1f00", "0000", "ca805300", "02000000", "00000000",
                // Property[3]: VT=4(FormID,Int) Func=2(ADD) Prop=31(Keywords)
                //              Value1=0x001B3FAC  Value2=2  Step=CurveTable(null)
                "04", "000000", "02", "000000", "1f00", "0000", "ac3f1b00", "02000000", "00000000",
            ),
            8,
        ),
        sr("MNAM", "6e7e7800", 9),
        sr("NAM1", "64", 10),
    ];

    let result = decode_record(&ctx, "OMOD", &subrecords);

    // Record type resolved, no subrecords left over.
    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Object Modification"),
    );
    assert!(
        result.get("_unmapped").is_none(),
        "all subrecords should be consumed by the schema"
    );

    // Navigate via &Value so .pointer() is available for nested paths.
    let data = result.get("Data").expect("Data struct must decode");

    // Header count fields.
    assert_eq!(data.get("Include Count").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(data.get("Property Count").and_then(|v| v.as_u64()), Some(4));

    // Form Type resolves to the "Weapon" enum name.
    assert_eq!(
        data.pointer("/Form Type/name").and_then(|v| v.as_str()),
        Some("Weapon"),
    );

    // Include[0]: Mod FormID and both bool flags.
    let includes = data
        .get("Includes")
        .and_then(|v| v.as_array())
        .expect("Includes must be an array");
    assert_eq!(includes.len(), 1);
    assert_eq!(
        includes[0].get("Mod").and_then(|v| v.as_str()),
        Some("0x004519F7"),
        "Include[0].Mod"
    );
    assert_eq!(
        includes[0]
            .pointer("/Optional/name")
            .and_then(|v| v.as_str()),
        Some("False"),
        "Include[0].Optional"
    );
    assert_eq!(
        includes[0]
            .get("Don't Use All")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str()),
        Some("True"),
        "Include[0].Don't Use All"
    );

    // All four Properties decoded correctly.
    let props = data
        .get("Properties")
        .and_then(|v| v.as_array())
        .expect("Properties must be an array");
    assert_eq!(props.len(), 4, "expected exactly 4 Properties");

    // Property[0]: adds an Enchantment FormID (Value Type 4 = FormID,Int).
    let p0 = &props[0];
    assert_eq!(
        p0.pointer("/Value Type/name").and_then(|v| v.as_str()),
        Some("FormID,Int"),
        "P[0] Value Type"
    );
    assert_eq!(
        p0.pointer("/Function Type/name").and_then(|v| v.as_str()),
        Some("ADD"),
        "P[0] Function Type"
    );
    assert_eq!(
        p0.pointer("/Property/name").and_then(|v| v.as_str()),
        Some("Enchantments"),
        "P[0] Property name"
    );
    assert_eq!(
        p0.get("Value 1").and_then(|v| v.as_str()),
        Some("0x0085B97F"),
        "P[0] Value 1 (FormID)"
    );
    assert_eq!(
        p0.get("Value 2").and_then(|v| v.as_u64()),
        Some(1),
        "P[0] Value 2 (int)"
    );
    assert!(
        p0.get("Curve Table").map(|v| v.is_null()).unwrap_or(false),
        "P[0] Curve Table must be null (form_version 208 uses Step field, not CURV)"
    );

    // Properties[1–3]: each adds a Keyword FormID with multiplicity 2.
    let kwd_props = [
        ("0x0085B984", 2u64),
        ("0x005380CA", 2u64),
        ("0x001B3FAC", 2u64),
    ];
    for (i, (fid, v2)) in kwd_props.iter().enumerate() {
        let p = &props[i + 1];
        assert_eq!(
            p.pointer("/Value Type/name").and_then(|v| v.as_str()),
            Some("FormID,Int"),
            "P[{}] Value Type",
            i + 1
        );
        assert_eq!(
            p.pointer("/Property/name").and_then(|v| v.as_str()),
            Some("Keywords"),
            "P[{}] Property name",
            i + 1
        );
        assert_eq!(
            p.get("Value 1").and_then(|v| v.as_str()),
            Some(*fid),
            "P[{}] Value 1",
            i + 1
        );
        assert_eq!(
            p.get("Value 2").and_then(|v| v.as_u64()),
            Some(*v2),
            "P[{}] Value 2",
            i + 1
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Curated CI decode tests — verbatim subrecord bytes from
//   esm get SeventySix_20260619.esm --formid <ID> --raw
//
// Each test:
//   1. Asserts the correct _record_type name.
//   2. Asserts no _unknown_record / raw_fallback / _unmapped markers
//      (full decode) via assert_fully_decoded().
//   3. Spot-checks one or two key field values to pin the decode path.
//
// The form_version used for each context matches the value in the
// record's header.form_version as reported by --raw.  Using the wrong
// form_version would silently mis-decode version-gated fields.
// ════════════════════════════════════════════════════════════════════════════

/// GLOB 0x00000035 — `GameYear` — decodes to Global with the correct float value.
///
/// Simple 2-subrecord record (EDID + FLTV); exercises the Global float path and
/// confirms no extra subrecords leak through.  form_version 157 (an older FO76
/// build; GLOB has no version-gated fields so the number doesn't matter much,
/// but we honour it for consistency).
#[test]
fn glob_game_year_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 157);

    // EDID = "GameYear\0"   FLTV = 287.0f32 LE (0x438f8000)
    let subs = subrecords_from(&[("EDID", "47616d655965617200"), ("FLTV", "00808f43")]);

    let result = decode_record(&ctx, "GLOB", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Global"),
    );
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("GameYear"),
    );
    // 0x438f8000 = 287.0f32
    let value = result
        .get("Value")
        .and_then(|v| v.as_f64())
        .expect("Value must be present");
    assert!(
        (value - 287.0).abs() < 0.01,
        "Value should be ~287.0, got {value}"
    );
}

/// KYWD 0x000000C1 — `SplineLink` — decodes to Keyword with RGBA color and type enum.
///
/// 3-subrecord record (EDID + CNAM + TNAM).  Exercises the keyword color struct
/// (4 bytes → {r,g,b,a}) and the keyword-type enum (0 → "None").
#[test]
fn kywd_spline_link_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 57);

    // EDID = "SplineLink\0"   CNAM = RGBA(255,255,0,0)   TNAM = enum 0 (None)
    let subs = subrecords_from(&[
        ("EDID", "53706c696e654c696e6b00"),
        ("CNAM", "ffff0000"),
        ("TNAM", "00000000"),
    ]);

    let result = decode_record(&ctx, "KYWD", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Keyword"),
    );
    assert_fully_decoded(&result);

    let color = result.get("Color").expect("Color must be present");
    assert_eq!(color.get("r").and_then(|v| v.as_u64()), Some(255), "r");
    assert_eq!(color.get("g").and_then(|v| v.as_u64()), Some(255), "g");
    assert_eq!(color.get("b").and_then(|v| v.as_u64()), Some(0), "b");
    assert_eq!(color.get("a").and_then(|v| v.as_u64()), Some(0), "a");

    assert_eq!(
        result.pointer("/Type/name").and_then(|v| v.as_str()),
        Some("None"),
        "keyword type must be 'None'"
    );
}

/// FLST 0x00000163 — `HelpManualPC` — decodes to FormID List with 100 LNAM entries.
///
/// 101-subrecord record (EDID + 100 × LNAM).  Exercises the repeated-LNAM
/// array path; confirms all entries decode without leaking any as _unmapped.
#[test]
fn flst_help_manual_pc_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    // EDID = "HelpManualPC\0"  +  100 LNAM FormID entries (verbatim LE u32 bytes)
    let subs = subrecords_from(&[
        ("EDID", "48656c704d616e75616c504300"),
        ("LNAM", "872d5a00"),
        ("LNAM", "24231e00"),
        ("LNAM", "77d47a00"),
        ("LNAM", "1c175100"),
        ("LNAM", "66c78400"),
        ("LNAM", "67c78400"),
        ("LNAM", "63c78400"),
        ("LNAM", "64c78400"),
        ("LNAM", "65c78400"),
        ("LNAM", "68c78400"),
        ("LNAM", "75737c00"),
        ("LNAM", "26c01e00"),
        ("LNAM", "1e175100"),
        ("LNAM", "19601e00"),
        ("LNAM", "e3731e00"),
        ("LNAM", "7ab81e00"),
        ("LNAM", "fb9c2b00"),
        ("LNAM", "cfa47b00"),
        ("LNAM", "e6731e00"),
        ("LNAM", "d4a47b00"),
        ("LNAM", "d1a47b00"),
        ("LNAM", "d3a47b00"),
        ("LNAM", "d0a47b00"),
        ("LNAM", "d5a47b00"),
        ("LNAM", "d2a47b00"),
        ("LNAM", "1fc01e00"),
        ("LNAM", "9dd55000"),
        ("LNAM", "b3277b00"),
        ("LNAM", "852d5a00"),
        ("LNAM", "fc9c2b00"),
        ("LNAM", "fe9c2b00"),
        ("LNAM", "b2277b00"),
        ("LNAM", "ff9c2b00"),
        ("LNAM", "24c01e00"),
        ("LNAM", "23c01e00"),
        ("LNAM", "af8a8100"),
        ("LNAM", "654c8d00"),
        ("LNAM", "21c01e00"),
        ("LNAM", "842d5a00"),
        ("LNAM", "b0677d00"),
        ("LNAM", "f1777b00"),
        ("LNAM", "a6767a00"),
        ("LNAM", "f2777b00"),
        ("LNAM", "7b010000"),
        ("LNAM", "55363d00"),
        ("LNAM", "c9d98800"),
        ("LNAM", "822d5a00"),
        ("LNAM", "eaba1e00"),
        ("LNAM", "b1a77a00"),
        ("LNAM", "34465c00"),
        ("LNAM", "78010000"),
        ("LNAM", "daea5e00"),
        ("LNAM", "f2ab1e00"),
        ("LNAM", "693b3d00"),
        ("LNAM", "04e07e00"),
        ("LNAM", "24e38200"),
        ("LNAM", "ffb54400"),
        ("LNAM", "efab1e00"),
        ("LNAM", "41ac7900"),
        ("LNAM", "6b083a00"),
        ("LNAM", "7c010000"),
        ("LNAM", "73ee7a00"),
        ("LNAM", "2cc01e00"),
        ("LNAM", "42d47a00"),
        ("LNAM", "ec0e1f00"),
        ("LNAM", "edab1e00"),
        ("LNAM", "76083a00"),
        ("LNAM", "40b55c00"),
        ("LNAM", "6d083a00"),
        ("LNAM", "ebab1e00"),
        ("LNAM", "25231e00"),
        ("LNAM", "ecab1e00"),
        ("LNAM", "b1277b00"),
        ("LNAM", "862d5a00"),
        ("LNAM", "92166a00"),
        ("LNAM", "0e776c00"),
        ("LNAM", "3ae97600"),
        ("LNAM", "e8c16700"),
        ("LNAM", "6d065f00"),
        ("LNAM", "e7ab1e00"),
        ("LNAM", "cb2f7f00"),
        ("LNAM", "1e971e00"),
        ("LNAM", "75083a00"),
        ("LNAM", "1d971e00"),
        ("LNAM", "0a1f6c00"),
        ("LNAM", "1c971e00"),
        ("LNAM", "fecc6600"),
        ("LNAM", "10c44f00"),
        ("LNAM", "80010000"),
        ("LNAM", "70083a00"),
        ("LNAM", "7d010000"),
        ("LNAM", "72083a00"),
        ("LNAM", "6e083a00"),
        ("LNAM", "ec667d00"),
        ("LNAM", "75928400"),
        ("LNAM", "7b2e8500"),
        ("LNAM", "de3a8600"),
        ("LNAM", "ea9a8600"),
        ("LNAM", "eb9a8600"),
        ("LNAM", "59528b00"),
    ]);

    let result = decode_record(&ctx, "FLST", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("FormID List"),
    );
    assert_fully_decoded(&result);

    let formids = result
        .get("FormIDs")
        .and_then(|v| v.as_array())
        .expect("FormIDs must be an array");
    assert_eq!(formids.len(), 100, "expected exactly 100 LNAM entries");
    assert_eq!(
        formids[0].get("FormID").and_then(|v| v.as_str()),
        Some("0x005A2D87"),
        "first FormID"
    );
    assert_eq!(
        formids[99].get("FormID").and_then(|v| v.as_str()),
        Some("0x008B5259"),
        "last FormID"
    );
}

/// AMMO 0x00001BA4 — `crAmmoScorchbeastSonicAttack` — decodes to Ammunition fully.
///
/// 9-subrecord record (EDID OBND FULL ENLT ENLS DESC DATA DNAM ONAM).
/// Exercises the DNAM inline struct (Projectile FormID, Flags bitfield, Damage
/// float, Health uint) without any raw fallbacks.
#[test]
fn ammo_scorchbeast_sonic_attack_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 175);

    let subs = subrecords_from(&[
        ("EDID", "6372416d6d6f53636f7263686265617374536f6e696341747461636b00"),
        ("OBND", "000000000000000000000000"),
        ("FULL", "3c49443d30303033453543343e53636f726368626561737420536f6e69632041747461636b20416d6d6f00"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("DESC", "00"),
        ("DATA", "0000000000000000"),
        ("DNAM", "94001300020000000000000000000000"),
        ("ONAM", "3c49443d30303033453543353e53636f726368626561737420536f6e69632041747461636b20416d6d6f00"),
    ]);

    let result = decode_record(&ctx, "AMMO", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Ammunition"),
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Name").and_then(|v| v.as_str()),
        Some("Scorchbeast Sonic Attack Ammo"),
        "Name"
    );

    // DNAM — inline struct
    let dnam = result.get("DNAM").expect("DNAM must decode");
    assert_eq!(
        dnam.get("Projectile").and_then(|v| v.as_str()),
        Some("0x00130094"),
        "DNAM.Projectile"
    );
    assert_eq!(
        dnam.pointer("/Flags/flags/0").and_then(|v| v.as_str()),
        Some("Non-Playable"),
        "DNAM.Flags must include Non-Playable"
    );
    assert_eq!(
        dnam.get("Damage").and_then(|v| v.as_f64()),
        Some(0.0),
        "DNAM.Damage"
    );
}

/// ALCH 0x000045C9 — `TrackingDart` — decodes to Ingestible fully.
///
/// 20-subrecord record spanning OBND, KSIZ/KWDA keyword block, MODL/MODT
/// model, ENIT effect data, and an EFID/EFIT magic effect entry.  Exercises
/// the keyword-array path (KSIZ count + KWDA payload) and the ENIT struct.
#[test]
fn alch_tracking_dart_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 182);

    let subs = subrecords_from(&[
        ("EDID", "547261636b696e674461727400"),
        ("OBND", "fffffaff0000010007000200"),
        ("PTRN", "16702400"),
        ("FULL", "3c49443d30303033444139463e547261636b696e67204461727400"),
        ("KSIZ", "02000000"),
        ("KWDA", "6f5018008df95000"),
        ("MODL", "50726f70735c537972696e6765416d6d6f2e6e696600"),
        ("MODT", "0400000004000000000000000100000001000000595f8af364647300b7d70ce13a62850964647300b7d70ce1753e841d64647300b7d70ce16bd751fd64647300b7d70ce1329b0c2d6267736daeef2e19"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("YNAM", "479c2400"),
        ("DESC", "3c49443d30303033444141303e54617267657420656d697473206120747261636b696e67207369676e616c2e00"),
        ("DATA", "0000803e"),
        ("ENIT", "28000000190000000000000000000000f2ba02000000000000000000"),
        ("DNAM", "00"),
        ("EFID", "ca450000"),
        ("EFIT", "010000000000000000000000100e00000000000000000000"),
        ("DURG", "93bf2d00"),
        ("MIID", "01000000"),
    ]);

    let result = decode_record(&ctx, "ALCH", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Ingestible"),
    );
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

/// PROJ 0x000021E1 — `ProjectileAudioGrenade` — decodes to Projectile fully.
///
/// 13-subrecord record with a DEST/DSTD/DSTF destructible block and a large
/// DNAM payload.  Exercises the destructible-object sub-struct path and the
/// projectile data (Type enum, Speed, Gravity, Range).
#[test]
fn proj_audio_grenade_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 175);

    let subs = subrecords_from(&[
        ("EDID", "50726f6a656374696c65417564696f4772656e61646500"),
        ("OBND", "fffff9fffeff020005000200"),
        ("FULL", "3c49443d30303033443935373e4372796f67656e6963204772656e61646500"),
        ("MODL", "576561706f6e735c4772656e6164655c4372796f4772656e61646550726f6a656374696c652e6e696600"),
        ("MODT", "040000000400000000000000010000000100000012b6e1c46464730060d0c0b4718bee3e6464730060d0c0b43ed7ef2a6464730060d0c0b4203e3aca6464730060d0c0b4ca5ecd996267736d026bace5"),
        ("DEST", "050000000100000000000000"),
        ("DSTD", "00000006000000007fa5170000000000000000000000000000000000"),
        ("DSTF", ""),
        ("DATA", ""),
        ("DNAM", "060002000000003f0000af4400606a46000000000000000000000000000020400000000000000000cdcc4c3d0000003f00000000000000000000000000000000000000000000a0400000c8420000803e00000000668708000000000000"),
        ("NAM1", "456666656374735c4d757a4d616368696e6547756e30312e6e696600"),
        ("NAM2", "0400000004000000030000000200000000000000855b0aef6464730038973cead18c32ea6464730038973cea51edc5736464730038973ceae66605156464730038973cea000000000100000011000000"),
        ("VNAM", "01000000"),
    ]);

    let result = decode_record(&ctx, "PROJ", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Projectile"),
    );
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

/// ARMO 0x00000D64 — `SkinNaked` — decodes to Armor fully.
///
/// 18-subrecord record with repeated INDX/MODL pairs (armor addon list) and
/// BOD2 biped-body template.  Exercises the indexed-model array and confirms
/// no extra subrecords are left unmapped.
#[test]
fn armo_skin_naked_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    let subs = subrecords_from(&[
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
    ]);

    let result = decode_record(&ctx, "ARMO", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Armor"),
    );
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
}

/// AVIF 0x000002C2 — `Strength` — decodes to Actor Value Information fully.
///
/// 8-subrecord record (EDID DURL FULL DESC ANAM NAM0 NAM5 NAM6).  Exercises
/// the actor-value schema (name, description, abbreviation, float bounds) and
/// confirms the DURL string and both float range fields decode cleanly.
#[test]
fn avif_strength_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 172);

    let subs = subrecords_from(&[
        ("EDID", "537472656e67746800"),
        ("DURL", "312e3030303000"),
        ("FULL", "3c49443d30303031443733433e537472656e67746800"),
        ("DESC", "3c49443d30303031443733423e537472656e6774682069732061206d656173757265206f6620796f75722072617720706879736963616c20706f7765722e204974206166666563747320686f77206d75636820796f752063616e2063617272792c20616e64207468652064616d616765206f6620616c6c206d656c65652061747461636b732e00"),
        ("ANAM", "3c49443d30303032333938353e53545200"),
        ("NAM0", "00000000"),
        ("NAM5", "0000803f"),
        ("NAM6", "ffff7f7f"),
    ]);

    let result = decode_record(&ctx, "AVIF", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Actor Value Information"),
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Name").and_then(|v| v.as_str()),
        Some("Strength"),
        "Name"
    );
    assert_eq!(
        result.get("Abbreviation").and_then(|v| v.as_str()),
        Some("STR"),
        "Abbreviation"
    );

    let min = result
        .get("Minimum Value")
        .and_then(|v| v.as_f64())
        .expect("Minimum Value");
    assert!(
        (min - 1.0).abs() < 1e-4,
        "Minimum Value should be 1.0, got {min}"
    );

    let def = result
        .get("Default Value")
        .and_then(|v| v.as_f64())
        .expect("Default Value");
    assert!(
        (def - 0.0).abs() < 1e-6,
        "Default Value should be 0.0, got {def}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Fix A / Fix B / Fix D — per-type regression tests added alongside the
// VMAD/COED/RACE-morph decode fixes (HEAD commit).
//
// Five clean-type tests (assert_fully_decoded) lock the no-marker status.
// Three drift-type tests (assert_only_drift_markers) lock the drift boundary
// so that future regressions introducing NEW unexpected markers fail.
// ════════════════════════════════════════════════════════════════════════════

/// ENCH 0x00002B4F — `zzzcrEnchFireflyIchorSplatterFX` — decodes to
/// Enchantment fully.
///
/// 8-subrecord record (EDID OBND ENIT EFID EFIT MAGF CODV MIID); form_version
/// 208.  Exercises the ENIT struct and magic-effect entry (EFID/EFIT) without
/// any raw fallbacks.  Before the COED/VMAD fixes this type was in the dirty
/// list; this test locks it as clean.
#[test]
fn ench_firefly_ichor_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 208);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x00002B4F --raw` (form_version 208).
    let subs = subrecords_from(&[
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
    ]);

    let result = decode_record(&ctx, "ENCH", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Enchantment"),
    );
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

/// BOOK 0x00000871 — `recipe_mod_AssaultRifle_Receiver_FastTrigger-CritDMG` —
/// decodes to Book fully.
///
/// 19-subrecord record (EDID OBND PTRN XALG FULL MODL MODT ENLT ENLS AUUV
/// DESC YNAM KSIZ KWDA DATA DNAM CNAM INAM BTOF); form_version 185.  The XALG
/// subrecord is mapped in the BOOK schema (unlike GMRW where it is drift) —
/// this test confirms that XALG decodes cleanly here.
#[test]
fn book_assault_rifle_recipe_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 185);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x00000871 --raw` (form_version 185).
    let subs = subrecords_from(&[
        ("EDID", "7265636970655f6d6f645f41737361756c745269666c655f52656365697665725f46617374547269676765722d43726974444d4700"),
        ("OBND", "f9fff2ff00000d000e000100"),
        ("PTRN", "ad2e1e00"),
        ("XALG", "8000000000000000"),
        ("FULL", "3c49443d30303034313635303e506c616e3a2041737361756c74205269666c652046696572636520526563656976657200"),
        ("MODL", "50726f70735c496e7374523033426c75655072696e742e6e696600"),
        ("MODT", "040000000400000000000000010000000100000067993c1664647300b7d70ce104a433ec64647300b7d70ce14bf832f864647300b7d70ce15511e71864647300b7d70ce12f8f53536267736daeef2e19"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "0189da2f000048420000f04100009c429a99193fcdcccc3d00a8becb"),
        ("DESC", "00"),
        ("YNAM", "0f4c1d00"),
        ("KSIZ", "02000000"),
        ("KWDA", "67153e0027433d00"),
        ("DATA", "fa0000000000803e"),
        ("DNAM", "20000000000000000000000000"),
        ("CNAM", "00"),
        ("INAM", "32491100"),
        ("BTOF", "00000000"),
    ]);

    let result = decode_record(&ctx, "BOOK", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Book"),
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("recipe_mod_AssaultRifle_Receiver_FastTrigger-CritDMG"),
        "Editor ID"
    );
}

/// WEAP 0x000001F6 — `GasTrapDummy` — decodes to Weapon fully.
///
/// 8-subrecord record (EDID OBND FULL ENLT ENLS DESC DNAM CRDT); form_version
/// 176.  Simple weapon with a DNAM struct; exercises the weapon-data decode
/// path without any raw fallbacks.
#[test]
fn weap_gas_trap_dummy_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 176);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x000001F6 --raw` (form_version 176).
    let subs = subrecords_from(&[
        ("EDID", "4761735472617044756d6d7900"),
        ("OBND", "000000000000000000000000"),
        ("FULL", "3c49443d30303032333945383e476173547261702044756d6d7900"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("DESC", "00"),
        ("DNAM", "000000000000803f0000803f0000803f0000803f0000fa430000fa440000000000000000000000000000003f000000000000000000000000400100000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000000000009a99993e00000000a041000000000000000002000000ffff7f7f00000000"),
        ("CRDT", "000000400000803f00000000"),
    ]);

    let result = decode_record(&ctx, "WEAP", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Weapon"),
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("GasTrapDummy"),
        "Editor ID"
    );
}

/// PERK 0x00004168 — `TestTamePerk` — decodes to Perk fully (includes VMAD).
///
/// 17-subrecord record; form_version 175.  Carries a VMAD header (version 6,
/// object_format 2, 0 scripts) as well as PRKE/PRKC/CTDA/EPF2/EPF3/PRKF perk
/// entry blocks.  Before Fix A, records carrying VMAD hit a raw_fallback
/// "VMAD truncated"; this test locks the clean path.
#[test]
fn perk_test_tame_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 175);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x00004168 --raw` (form_version 175).
    let subs = subrecords_from(&[
        ("EDID", "5465737454616d655065726b00"),
        ("VMAD", "060002000000042a00467261676d656e74733a5065726b733a50524b465f5465737454616d655065726b5f3030303034313638000200090053515f4d617374657201010000ffffb8250500160053515f416e696d616c54616d696e674b6579776f726401010000ffff66410000010000000000012a00467261676d656e74733a5065726b733a50524b465f5465737454616d655065726b5f30303030343136381100467261676d656e745f456e7472795f30300300"),
        ("FULL", "3c49443d30303033423545353e54616d6520416e696d616c00"),
        ("DESC", "3c49443d30303033423545363e436f6d6d756e652077697468206265617374732100"),
        ("DATA", "0000030100"),
        ("SNAM", "27761a00"),
        ("PRKE", "020000"),
        ("DATA", "0e090200"),
        ("PRKC", "01"),
        ("CTDA", "000000000000000047002f4367410000000000000000000000000000ffffffff"),
        ("CTDA", "000000000000803f30022f4366410000000000000000000000000000ffffffff"),
        ("CTDA", "00000000000000002e002f4300000000000000000000000000000000ffffffff"),
        ("EPFT", "04"),
        ("EPFB", "0000"),
        ("EPF2", "3c49443d30303033423545373e54414d4500"),
        ("EPF3", "0200"),
        ("PRKF", ""),
    ]);

    let result = decode_record(&ctx, "PERK", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Perk"),
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("TestTamePerk"),
        "Editor ID"
    );
}

/// RACE 0x0001D31E (`PowerArmorRace`) morph subset — Fix B ported RACE morph
/// and face-morph builders; this test locks those schema paths.
///
/// Instead of embedding the full ~72 KB record, only EDID + one morph-group
/// element (MPGN/MPPC/MPPK/MPGS) + one face-morph element (FMRI/FMRN) are
/// provided.  Before Fix B these six sigs went to `_unmapped`; after the fix
/// they decode via the ported RArray schemas.  form_version 209.
#[test]
fn race_power_armor_morph_subset_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    // Selective real bytes from `esm get SeventySix_20260619.esm
    // --formid 0x0001D31E --raw` (form_version 209).
    // Only the morph/face-morph sigs are included; the rest of the record's
    // ~500 other subrecords are omitted (they decode as null/absent — no markers).
    let subs = subrecords_from(&[
        ("EDID", "506f77657241726d6f725261636500"), // "PowerArmorRace"
        // Morph Groups Male — first element: name="Nose", 0 presets, MPPK=-1, MPGS=[0]
        ("MPGN", "4e6f736500"),
        ("MPPC", "00000000"),
        ("MPPK", "ffff"),
        ("MPGS", "0100000000000000"),
        // Face Morphs Male — first element: index=0, name="EyeBrow Main"
        ("FMRI", "00000000"),
        (
            "FMRN",
            "3c49443d34313031313632393e45796562726f77204d61696e00",
        ),
    ]);

    let result = decode_record(&ctx, "RACE", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Race"),
    );
    // No _unmapped markers: MPGN/MPPC/MPPK/MPGS/FMRI/FMRN are all in the
    // schema after Fix B.  Any regression that removes these sigs from the
    // schema would surface as _unmapped here.
    assert_fully_decoded(&result);
}

// ── Drift-type regression tests ──────────────────────────────────────────────
//
// These three types remain intentionally partial: they carry subrecords that
// are absent from or version-gated out of the TES5Edit Pascal reference.
// The tests lock the drift boundary: exactly the documented sigs appear as
// _unmapped, and no NEW unexpected markers (raw_fallback, additional _unmapped)
// should emerge from future decoder changes.

/// GMRW 0x008B2016 — `WorldPets_Reward_PetLevelling_Generic_CUR_Goldbullions01`
/// — XALG is the documented drift subrecord.
///
/// form_version 209.  XALG is absent from the TES5Edit GMRW definition (which
/// covers only EDID/FTAGs/ANAM/RWDS/Rewards); it is newer than the reference
/// and intentionally left _unmapped.
#[test]
fn gmrw_xalg_drift_locked() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x008B2016 --raw` (form_version 209).
    let subs = subrecords_from(&[
        ("EDID", "576f726c64506574735f5265776172645f5065744c6576656c6c696e675f47656e657269635f4355525f476f6c6462756c6c696f6e73303100"),
        ("XALG", "0010000000000000"),
        ("RWDS", "01000000"),
        ("ESRE", "00000000"),
        ("QRCO", "c8c15500"),
        ("NAM8", "ad1f8b00"),
        ("QRLR", "01000000"),
        ("ITME", ""),
    ]);

    let result = decode_record(&ctx, "GMRW", &subs);

    // XALG must be the sole _unmapped sig — no other unexpected markers.
    assert_only_drift_markers(&result, &["XALG"]);
}

/// LVLI 0x0000129C — `LL_Flora_Corn` — LVLD is the documented drift subrecord.
///
/// form_version 197 (≥174).  `wbBelowVersion(174, LVLD …)` means LVLD is only
/// in the schema for form_version < 174; live data is ≥174, so LVLD is
/// _unmapped.  All other subrecords (OBND, ONAM, LVMV, LVCV, LVLF, LLCT, LVLO,
/// CTDA, LVOV, LVIV, LVLV, MODL, MODT, ENLT, ENLS, AUUV) are correctly
/// decoded.
#[test]
fn lvli_lvld_drift_locked() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 197);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x0000129C --raw` (form_version 197).
    let subs = subrecords_from(&[
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
    ]);

    let result = decode_record(&ctx, "LVLI", &subs);

    // LVLD must be the sole _unmapped sig — no other unexpected markers.
    assert_only_drift_markers(&result, &["LVLD"]);
}

/// NPC_ 0x0084FB8F — `ATX_CAMPPets_Actor_RadHog_Standard` — AWPB and CTDA are
/// documented drift subrecords.
///
/// form_version 209.  AWPB is absent from the entire TES5Edit FO76 reference
/// (newer than the reference); CTDA in NPC_ context is absent from the NPC_
/// definition (appears only as a sub-condition in NPC_-adjacent records in the
/// Pascal source).  Both remain _unmapped intentionally.  No VMAD raw_fallback
/// appears, confirming Fix A applies cleanly to this record too.
#[test]
fn npc_awpb_ctda_drift_locked() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x0084FB8F --raw` (form_version 209).
    let subs = subrecords_from(&[
        ("EDID", "4154585f43414d50506574735f4163746f725f526164486f675f5374616e6461726400"),
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
        ("TPTA", "0000000090fb840090fb840090fb840090fb840090fb840090fb84000000000090fb840090fb840090fb840090fb84000000000090fb8400"),
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
        ("KWDA", "94707900ff7982008cfb84008dfb8400b6c54f0011962400fba9440018825300126252002ad50a00164b6300"),
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
        ("CTDA", "000000000000803f5b0300008afb8400000000000000000000000000ffffffff"),
        ("CTDA", "000000000000803f5b0300008afb8400000000000000000000000000ffffffff"),
        ("CTDA", "000000000000803f5b0300008afb8400000000000000000000000000ffffffff"),
        ("CTDA", "000000000000803f5b0300008afb8400000000000000000000000000ffffffff"),
        ("CTDA", "000000000000803f5b0300008afb8400000000000000000000000000ffffffff"),
        ("CTDA", "000000000000803f5b0300008afb8400000000000000000000000000ffffffff"),
    ]);

    let result = decode_record(&ctx, "NPC_", &subs);

    // AWPB and CTDA are the only _unmapped sigs — no raw_fallback or other
    // unexpected markers, confirming Fix A cleared VMAD truncation on NPC_ too.
    assert_only_drift_markers(&result, &["AWPB", "CTDA"]);
}
