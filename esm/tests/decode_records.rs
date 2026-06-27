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

/// ARMO 0x005DD339 — `Armor_BOSInfantry_Torso` — Brotherhood Recon Chest Piece
/// decodes to Armor fully.
///
/// 43-subrecord record; form_version 208.  Exercises model info (MOD2/MO2T),
/// dual Enlighten blocks, BOD2 biped template, KSIZ/KWDA keywords, damage
/// resistances (DAMA), appearance (APPR), 2-entry OBTE/OBTF/FULL/OBTS object
/// template chain, and CVT1–CVT3 curve refs.
#[test]
fn armo_bos_recon_chest_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 208);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x005DD339 --raw` (form_version 208).
    let subs = subrecords_from(&[
        ("EDID", "41726d6f725f424f53496e66616e7472795f546f72736f00"),
        ("OBND", "f0fff1ff000010000c003100"),
        ("PTRN", "f1ea5e00"),
        ("FULL", "3c49443d34313030314242343e42726f74686572686f6f64205265636f6e20436865737420506965636500"),
        ("MOD2", "41726d6f722f424f535f496e66616e7472792f424f535f496e66616e7472795f41726d6f725f546f72736f5f474f2e6e696600"),
        ("MO2T", "0400000000000000000000000000000000000000"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("BOD2", "00080000"),
        ("RNAM", "46370100"),
        ("KSIZ", "0e000000"),
        ("KWDA", "cafa07003bd35d0045d35d007a9f4900f42e4500e94a0f0039c44300ecc006003ad35d00169a5200f6f55f00d6146200825418007ace7a00"),
        ("DESC", "00"),
        ("INRD", "c14b1800"),
        ("EILV", "2800000032000000"),
        ("IBSD", "35d24e00"),
        ("INDX", "0000"),
        ("MODL", "34d35d00"),
        ("DATA", "3c000000d7a3204064000000"),
        ("FNAM", "0f0000000000000000000000"),
        ("DAMA", "810a060000000000d56b8400850a060000000000d06b8400870a060000000000e16b8400840a060000000000ce6b8400820a060000000000d46b8400830a060000000000d06b8400"),
        ("APPR", "c6360500212802005a2e1800c8321e00a8894e00a9894e00aa894e00ab894e001e7043009bb91c00d814620064a24700"),
        ("OBTE", "02000000"),
        ("OBTF", ""),
        ("FULL", "3c49443d38313030314438373e44656661756c7400"),
        ("OBTS", "030000000000000000000000ffff01000000ced65d000000019ce5180000000172f43c00000001"),
        ("OBTS", "0400000000000000000000000000000181418a0000008a418a0000000148e54e00000001856d4f0000000160da8300000001"),
        ("STOP", ""),
        ("CVT1", "97ab1f00"),
        ("CVT2", "c9bc1800"),
        ("CVT3", "565a3400"),
        ("ABPO", "bbe45400"),
        ("VCRY", "0f000000"),
    ]);

    let result = decode_record(&ctx, "ARMO", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Armor"),
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("Armor_BOSInfantry_Torso"),
        "Editor ID"
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

/// ENCH 0x00900A5A — `EnchPerkConcentratedFire` — decodes to Enchantment fully,
/// including the newer-than-reference Keywords block (KSIZ + KWDA).
///
/// 12-subrecord record (EDID OBND FULL KSIZ KWDA ENIT EFID EFIT MAGA MAGF CODV
/// MIID); form_version 209.  KSIZ/KWDA are absent from the Pascal reference but
/// present in the live ESM — covered via `record_additions` override.  This test
/// locks that path clean.
#[test]
fn ench_perk_concentrated_fire_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x00900A5A --raw` (form_version 209).
    let subs = subrecords_from(&[
        ("EDID", "456e63685065726b436f6e63656e7472617465644669726500"),
        ("OBND", "000000000000000000000000"),
        ("FULL", "3c49443d33393239443530453e436f6e63656e747261746564204669726500"),
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
    ]);

    let result = decode_record(&ctx, "ENCH", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Enchantment"),
    );
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

/// WEAP 0x000FF964 — `SuperSledge` — decodes to Weapon fully.
///
/// 59-subrecord record; form_version 209.  Exercises model info (MODL/MODT),
/// dual Enlighten blocks (ENLT/ENLS/AUUV), 10-entry object template chain
/// (OBTE/OBTF/FULL/OBTS×10), alternate-texture set (MOD4/MO4T), curve refs
/// (CVT0–CVT4), KSIZ/KWDA keywords, and the full DNAM weapon data struct.
#[test]
fn weap_super_sledge_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x000FF964 --raw` (form_version 209).
    let subs = subrecords_from(&[
        ("EDID", "5375706572536c6564676500"),
        ("OBND", "f6fff5fffbff2b0039000400"),
        ("PTRN", "510d0200"),
        ("FULL", "3c49443d34313031313439303e537570657220536c6564676500"),
        ("MODL", "576561706f6e735c526f636b657448616d6d65725c526f636b657448616d6d65723173742e6e696600"),
        ("MODT", "040000000c0000000000000006000000020000002fb9ccae64647300a0b863054c84c35464647300a0b8630503d8c24064647300a0b863052a0e974d64647300d126dddb493398b764647300d126dddb066f99a364647300d126dddbe804cd346464730038973cea29f70f0064647300582c553393cb280d6464730038973cea18864c4364647300d126dddbaff0dd356464730038973cea1d3117a064647300a0b86305edaca0006267736d2792e1e0d6597aee6267736db98041fd"),
        ("XFLG", "10"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("MODD", "10"),
        ("ETYP", "423f0100"),
        ("BIDS", "8c390d00"),
        ("BAMT", "c1740700"),
        ("KSIZ", "0c000000"),
        ("KWDA", "a2b40e00a5a004009bc81000b0900600ea4a0f0024ab330013ab330012ab3300820e3d00b1175100a6d160006e7e7800"),
        ("DESC", "00"),
        ("INRD", "38970b00"),
        ("EILV", "1e0000002800000032000000"),
        ("IBSD", "33d24e00"),
        ("APPR", "4c520500c8321e00a8894e00a9894e00aa894e00ab894e0064a247001e001a00"),
        ("OBTE", "0a000000"),
        ("OBTF", ""),
        ("FULL", "3c49443d34313031313439313e44656661756c7400"),
        ("OBTS", "020000000000000000000000ffff01000000b1900600000001bf175100000001"),
        ("OBTF", ""),
        ("FULL", "3c49443d34313031313439323e5374616e6461726400"),
        ("OBTS", "020000000000000000000000ffff0001a27f02000000b1900600000001bf175100000001"),
        ("OBTF", ""),
        ("FULL", "3c49443d34313031313439333e5374616e64617264204570696300"),
        ("OBTS", "020000000000000000000000010000012df4050000000b032300000001bf175100000001"),
        ("OBTF", ""),
        ("FULL", "3c49443d34313031313439343e53696d706c6500"),
        ("OBTS", "020000000000000000000000ffff0001000323000000b1900600000001bf175100000001"),
        ("OBTF", ""),
        ("OBTS", "050000000000000000000000ffff0001e4f167000000563d11000000017e184700000001736b60000000018d6c600000000152415200000001"),
        ("OBTS", "020000000000000000000000ffff00015c287c000000ae900600000001b8815200000001"),
        ("OBTF", ""),
        ("OBTS", "060000000000000000000000ffff0001655c88000000822e87000000017d574f00000001da7b1a00000001fc9952000000016a5c8800000001b1900600000001"),
        ("OBTS", "060000000000000000000000ffff0001fa208f000000b19006000000017b574f00000001eb047900000001ec04790000000108d064000000015a276600000001"),
        ("OBTS", "060000000000000000000000ffff00014a2a8f000000b1900600000001bf175100000001ea047900000001eb047900000001fd9952000000016ac25e00000001"),
        ("OBTS", "060000000000000000000000ffff00014d2a8f000000b1900600000001ea047900000001da7b1a00000001ec0479000000011f316800000001254c6700000001"),
        ("STOP", ""),
        ("MOD4", "576561706f6e735c526f636b657448616d6d65725c526f636b657448616d6d65723173745f312e6e696600"),
        ("MO4T", "0400000000000000000000000000000000000000"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("DNAM", "000000000000803f0000803f00000000cdcc4c3f0000803f0000000000002041000000000000000000000000000080bf000000000000000000000000000100000000010005000070410000a041460000002800000000002f5c2400000000000000000000000000eb5c4000ae982400ac2615000000000000bb66e63f000000005042000000000000000002000000ffff7f7f640000000000000000000000cdcccc3dcdcccc3d00000000"),
        ("CRDT", "000040400000803f00000000"),
        ("INAM", "f4132300"),
        ("CVT0", "17f28000"),
        ("CVT1", "b9ab1f00"),
        ("CVT2", "1b8f2e00"),
        ("CVT3", "f3610300"),
        ("CVT4", "4c5a3400"),
        ("MASE", "01000000"),
        ("WTDT", "00000000"),
        ("WSAM", "00002040"),
    ]);

    let result = decode_record(&ctx, "WEAP", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Weapon"),
    );
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("SuperSledge"),
        "Editor ID"
    );
}

/// WEAP 0x00113083 — `crDLC04AnimatronicAlienBlaster` — decodes to Weapon
/// fully, including the newer-than-reference EAMT subrecord.
///
/// 39-subrecord record; form_version 205.  EAMT (Enchantment Amount, u16) is
/// absent from the Pascal WEAP definition but present in the live ESM — covered
/// via `record_additions` override.  This test locks that path clean.
#[test]
fn weap_animatronic_alien_blaster_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 205);

    // Verbatim subrecords from `esm get SeventySix_20260619.esm
    // --formid 0x00113083 --raw` (form_version 205).
    let subs = subrecords_from(&[
        ("EDID", "6372444c433034416e696d6174726f6e6963416c69656e426c617374657200"),
        ("OBND", "fcfffcfff8ff040018000800"),
        ("PTRN", "dd1d0700"),
        ("FULL", "3c49443d30303030304634363e416e696d6174726f6e696320416c69656e20426c617374657200"),
        ("MODL", "444c4330345c576561706f6e735c46616b65416c69656e426c61737465725c46616b65416c69656e426c61737465722e6e696600"),
        ("MODT", "040000000d000000010000000300000003000000dae093a564647300607b9d87b9dd9c5f64647300607b9d87f6819d4b64647300607b9d87105544aa646473007b49fa3773684b50646473007b49fa37988c963d646473007b49fa373c344a44646473007b49fa371008ee15646473004feae24d7335e1ef646473004feae24d3c69e0fb646473004feae24d2280351b646473004feae24d22dd9fa4646473007b49fa37e86848ab64647300607b9d87c600000036cd804b6267736db95ede76bd56cc1d6267736dae9326475cf1e05c6267736d96cfa1bc"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("EITM", "3a961700"),
        ("EAMT", "1027"),
        ("ETYP", "423f0100"),
        ("BIDS", "ff830100"),
        ("YNAM", "23490300"),
        ("ZNAM", "c3f50100"),
        ("KSIZ", "0c000000"),
        ("KWDA", "8b96160048f90100ea4a0f0096f90f00ac3f1b00b3ad030085c41000e0881800797b1a006ac41c00453e1100c8a73300"),
        ("DESC", "00"),
        ("INRD", "cf772300"),
        ("APPR", "9d240200992402009f240200c8321e00d7d40500"),
        ("OBTE", "01000000"),
        ("OBTS", "000000000000000000000000ffff01000000"),
        ("STOP", ""),
        ("MOD4", "444c4330345c576561706f6e735c46616b65416c69656e426c61737465725c46616b65416c69656e426c61737465725f312e6e696600"),
        ("MO4T", "0400000000000000000000000000000000000000"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("DNAM", "97180c000000803f0000803f000000000000803f0000803f000080430000804300000000cdcccc3d000000000000003f000000000000000000000000000112002a000100090000c0400000004000000000000001000000000000000000000000000000cdda01000000000000000000ac2615000000000000abaa2a3f00000000a041000000000000000001000000ffff7f7f000000000000000000000000cdcccc3dcdcccc3d00000000"),
        ("RGW3", "84301100acc527376666663fcdcc4c3fcdcc4c3ea2773740a8aaea3f9a99193e9a99193e0000803e0000803f000000000300000001"),
        ("CRDT", "000000400000803f00000000"),
        ("INAM", "56181700"),
        ("LNAM", "98180c00"),
        ("WAMD", "e1881800"),
        ("DAMA", "810a060000000000c9e97600"),
        ("VCRY", "0f000000"),
        ("WTDT", "00000000"),
        ("WSAM", "00000040"),
    ]);

    let result = decode_record(&ctx, "WEAP", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Weapon"),
    );
    assert_fully_decoded(&result);

    // EAMT = 0x2710 = 10000 (little-endian u16).
    assert_eq!(
        result
            .get("Enchantment Amount")
            .and_then(|v| v.as_u64()),
        Some(10000),
        "Enchantment Amount"
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
// Part 2 — basic decode tests for partial-record fix plan (QUST deferred to part 3).

/// TERM 0x00001676 — `<no edid>` — basic decode regression.
#[test]
fn term_intel_room_exams_sub_terminal_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 201);

    let subs = subrecords_from(&[
        ("EDID", "454e30325f496e74656c526f6f6d4578616d735375625465726d696e616c303300"),
        ("VMAD",
            "0600020001001700454e30325f4578616d5175657374696f6e5363726970740002000d00546172676574416e7377657273110105000000020000000c0069416e7377657256616c75650301010000000700694d656e754944030101000000020000000c0069416e7377657256616c75650301030000000700694d656e754944030102000000020000000c0069416e7377657256616c75650301020000000700694d656e754944030103000000020000000700694d656e7549440301040000000c0069416e7377657256616c7565030100000000020000000c0069416e7377657256616c75650301010000000700694d656e7549440301050000000b0064656a614368616e6e656c02010400454e3032"),
        ("OBND", "ecffdeff0000130010006100"),
        ("NAM0", "3c49443d30303033433833353e5768697465737072696e675f4e6574202d2d20762e303800"),
        ("FULL", "3c49443d30303033433833363e5175657374696f6e6e61697265205465726d696e616c00"),
        ("MODL",
            "4675726e69747572655c5465726d696e616c735c5465726d696e616c436f6e736f6c654f6e2e6e696600"),
        ("MODT",
            "0400000019000000000000000a000000050000000f192fb864647300514b85776c24204264647300514b85772378215664647300514b8577886afdda64647300b5aa259deb57f22064647300b5aa259da40bf33464647300b5aa259df399858864647300b5aa259d90a48a7264647300b5aa259ddff88b6664647300b5aa259d74e31e5d64647300514b857717de11a764647300514b8577588210b364647300514b8577f0a1333264647300514b8577939c3cc864647300514b8577dcc03ddc64647300514b8577c229e83c64647300514b8577466bc55364647300514b8577839c30dc6464730038973ceac1115e8664647300b5aa259dbae226d464647300b5aa259db0a5a86e64647300b5aa259d3d91f4b664647300514b8577ff6512fd6464730038973cea7eef02fc64647300582c5533c80fd0fc6464730038973cea9a7f25c36267736de25d598b1f0637246267736de25d598b404118cb6267736d6070f9eddaa82f006267736d6070f9ed7de3679b6267736dde5ee364"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
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
        ("XMRK", "4d61726b6572735c4d61726b65724465736b5465726d696e616c30312e6e696600"),
        ("ZNAM", "00000000000088c2000000000000000000000000ff010000"),
        ("FFEF", "00000000"),
        ("BSIZ", "01000000"),
        ("BTXT",
            "3c49443d30303033433833373e5c5c5c5c20504552534f4e414c20494e464f524d4154494f4e202f2f2f2f0d0a0d0a5768696368206f662074686520666f6c6c6f77696e67207468696e6b657273272062656c6965662073797374656d73206d6f737420636c6f73656c79206d61746368657320796f7572206f776e3f0d0a5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f0d0a00"),
        ("TDAT", "0100"),
        ("ISIZ", "05000000"),
        ("ITXT", "3c49443d30303033433833383e53742e2054686f6d617320417175696e617300"),
        ("RNAM", "3c49443d30303033433833393e2e2e2e616e73776572207265636f726465642e2e2e00"),
        ("ANAM", "04"),
        ("ITID", "0100"),
        ("TNAM", "45160000"),
        ("ITXT", "3c49443d30303033433833413e4164616d20536d69746800"),
        ("RNAM", "3c49443d30303033433833423e2e2e2e20616e73776572207265636f72646564202e2e2e00"),
        ("ANAM", "04"),
        ("ITID", "0200"),
        ("TNAM", "45160000"),
        ("ITXT", "3c49443d30303033433833433e4a6f686e20537475617274204d696c6c00"),
        ("RNAM", "3c49443d30303033433833443e2e2e2e20616e73776572207265636f72646564202e2e2e00"),
        ("ANAM", "04"),
        ("ITID", "0300"),
        ("TNAM", "45160000"),
        ("ITXT", "3c49443d30303034353133343e4b61726c204d61727800"),
        ("RNAM", "3c49443d30303034353133353e2e2e2e20616e73776572207265636f72646564202e2e2e00"),
        ("ANAM", "04"),
        ("ITID", "0400"),
        ("TNAM", "45160000"),
        ("ITXT", "3c49443d30303034353133363e456c76697320507265736c657900"),
        ("RNAM", "3c49443d30303034353133373e2e2e2e20616e73776572207265636f72646564202e2e00"),
        ("ANAM", "04"),
        ("ITID", "0500"),
        ("TNAM", "45160000"),
    ]);

    let result = decode_record(&ctx, "TERM", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Terminal"),
    );
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("EN02_IntelRoomExamsSubTerminal03"),
    );
}

/// NOTE 0x00002CBA — `<no edid>` — basic decode regression.
#[test]
fn note_fs_jacob_part01_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 192);

    let subs = subrecords_from(&[
        ("EDID", "46535f4a61636f6250617274303100"),
        ("OBND", "fdfffeff0000030002000000"),
        ("PTRN", "a5950900"),
        ("SNTP", "74705300"),
        ("FULL", "3c49443d30303033443938383e4a61636f62277320486f6c6f7461706500"),
        ("MODL", "50726f70735c486f6c6f746170655f50726f702e6e696600"),
        ("MODT",
            "0400000005000000010000000100000001000000aae4fd56646473007b24d06cc9d9f2ac646473007b24d06c223d2fc1646473007b24d06c8685f3b8646473007b24d06c986c2658646473007b24d06c4a040000bd0d0a246267736daeef2e19"),
        ("YNAM", "a8bf0b00"),
        ("ZNAM", "a9bf0b00"),
        ("KSIZ", "01000000"),
        ("KWDA", "27433d00"),
        ("VCRY", "0f000000"),
        ("DNAM", "01"),
        ("DATA", "0000000000000000"),
        ("SNAM", "5b902a00"),
    ]);

    let result = decode_record(&ctx, "NOTE", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Note"),
    );
    assert_fully_decoded(&result);
}

/// FLOR 0x000017F8 — `<no edid>` — basic decode regression.
#[test]
fn flor_firecap_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 192);

    let subs = subrecords_from(&[
        ("EDID", "5573654c50495f466c6f726146697265436170303100"),
        ("OBND", "e9ffe8fffcff170018002c00"),
        ("OPDS",
            "0100000000000000000000000000803f0000803ec2b8b23dc2b8b23dc2b8323ec2b8323edb0fc93fdb0f4940"),
        ("PTRN", "521a4400"),
        ("OPDS",
            "0100000000000000000000000000803f0000803ec2b8b23dc2b8b23dc2b8323ec2b8323edb0fc93fdb0f4940"),
        ("DEFL", "2cd61e00"),
        ("FULL", "3c49443d30303033454236413e4669726563617000"),
        ("MODL", "6c616e6473636170652f706c616e74732f6669726563617030312e6e696600"),
        ("MODT",
            "0400000004000000000000000100000001000000eb474cff646473008a103af0887a4305646473008a103af0c7264211646473008a103af0d9cf97f1646473008a103af037df72446267736dd28d2cde"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01774a62000048420000f04100009c429a99193fcdcccc3d00d4a379"),
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
    ]);

    let result = decode_record(&ctx, "FLOR", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Flora"),
    );
    assert_fully_decoded(&result);
}

/// FURN 0x00002C9E — `<no edid>` — basic decode regression.
#[test]
fn furn_power_armor_raider_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    let subs = subrecords_from(&[
        ("EDID", "506f77657241726d6f724675726e69747572655261696465724d544e5a303400"),
        ("VMAD",
            "06000200010012004d544e5a30345f41726d6f725363726970740003000f004d544e5a30345f4d656c74646f776e01010000ffff942c00001d004d544e5a30345f4d656c74646f776e5f51756573745f4b6579776f726401010000ffffa22c00000b0041726d6f724c6f636b6564050101"),
        ("OBND", "d8ffe1ff000028000e008a00"),
        ("FULL", "3c49443d30303033453942313e506f7765722041726d6f7200"),
        ("MODL",
            "4675726e69747572655c506f77657241726d6f725c4368617261637465724173736574735c506f77657241726d6f724675726e69747572652e6e696600"),
        ("MODT",
            "040000000400000000000000010000000100000078ebd765646473000024eaaa1bd6d89f646473000024eaaa548ad98b646473000024eaaa4a630c6b646473000024eaaac4f8f4ef6267736d5beb74cf"),
        ("ENLM", "01000000"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
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
        ("XMRK",
            "4675726e69747572655c506f77657241726d6f725c4368617261637465724173736574735c506f77657241726d6f724675726e69747572652e6e696600"),
        ("ZNAM",
            "0000000000000000000000000000000000000000ff0100000000000042a097c20000000000000000a8bd0500ff010000"),
        ("APPR", "8c5f05008d5f05008e5f05008f5f0500905f0500fcda030020670500"),
        ("OBTE", "01000000"),
        ("OBTS",
            "050000000000000000000000ffff000000001f6705000000017d7b13000000017e7b13000000017f7b1300000001807b1300000001"),
        ("STOP", ""),
        ("FFEF", "00000000"),
        ("NVNM", ""),
    ]);

    let result = decode_record(&ctx, "FURN", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Furniture"),
    );
    assert_fully_decoded(&result);
}

/// MISC 0x0000000A — `<no edid>` — basic decode regression.
#[test]
fn misc_bobby_pin_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 187);

    let subs = subrecords_from(&[
        ("EDID", "426f62627950696e00"),
        ("OBND", "fbfffeff0000030004000000"),
        ("PTRN", "8b882400"),
        ("FULL", "3c49443d30303031444236423e426f6262792050696e00"),
        ("MODL", "50726f70735c426f62627950696e2e6e696600"),
        ("MODT",
            "04000000040000000000000001000000010000004054c76664647300cfe14e232369c89c64647300cfe14e236c35c98864647300cfe14e2372dc1c6864647300cfe14e23056489426267736d528aab77"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
        ("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000"),
        ("YNAM", "649d2400"),
        ("KSIZ", "02000000"),
        ("KWDA", "47940e006ac41c00"),
        ("DATA", "050000006f12833a"),
        ("AQIC", "000000000000baba"),
    ]);

    let result = decode_record(&ctx, "MISC", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Misc. Item"),
    );
    assert_fully_decoded(&result);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("BobbyPin"),
    );
}

/// INFO 0x0000219A — `<no edid>` — basic decode regression.
#[test]
fn info_dialog_response_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 195);

    let subs = subrecords_from(&[
        ("ENAM", "0000010000000000"),
        ("GNAM", "9e210000"),
        ("TRDA", "ffffffff0100000000000000ffffffffffffffff"),
        ("NAM1",
            "3c49443d30303033384139443e2a736967682a2049277665206e65766572207365656e2061206368696c6420736f2072656c756374616e742061626f757420706c61792074696d652e00"),
        ("NAM2", "00"),
        ("NAM3", "00"),
        ("NAM4", "00"),
        ("NAM9", "c0081683ec78d201"),
        ("CTDA", "002111e40000803f3602c94300000000000000000000000000000000ffffffff"),
        ("CTDA", "002111e40000803f3b00c94364000000000000000000000000000000ffffffff"),
        ("CTDA", "002111e4000000003b00c94358020000000000000000000000000000ffffffff"),
        ("NAM0", "00"),
        ("INAM", "01000000"),
        ("NAM8", "10664c00362cd401"),
    ]);

    let result = decode_record(&ctx, "INFO", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Dialog response"),
    );
    assert_fully_decoded(&result);
}

/// QMDL 0x006496AB — `<no edid>` — basic decode regression.
#[test]
fn qmdl_object_destruction_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 197);

    let subs = subrecords_from(&[
        ("EDID", "4f626a6563744465737472756374696f6e00"),
        ("QMDI", "0000"),
        ("QMDQ", "ff5d6400"),
        ("QMDP", "005e6400"),
        ("QMDT", "100e"),
        ("QMPO", "0000"),
        ("QMAD", "00000000"),
        ("QMSD", "00000000"),
    ]);

    let result = decode_record(&ctx, "QMDL", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Quest Module"),
    );
    assert_fully_decoded(&result);
}

/// LVLN 0x00004073 — `<no edid>` — basic decode regression.
#[test]
fn lvln_test_essl_char_short_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 175);

    let subs = subrecords_from(&[
        ("EDID", "546573744553534c4368617253686f727400"),
        ("OBND", "000000000000000000000000"),
        ("LVLD", ""),
        ("LVMV", "00000000"),
        ("LVCV", "00000000"),
        ("LVLF", "00"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
    ]);

    let result = decode_record(&ctx, "LVLN", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Leveled NPC"),
    );
    assert_only_drift_markers(&result, &["LVLD"]);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("TestESSLCharShort"),
    );
}

/// LVPC 0x006DE9E3 — `NPE_Loadout_CommandoSelectionList` — basic decode regression.
#[test]
fn lvpc_loadout_commando_selection_list_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 204);

    let subs = subrecords_from(&[
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
    ]);

    let result = decode_record(&ctx, "LVPC", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Leveled Perk Card"),
    );
    assert_only_drift_markers(&result, &["LVLD"]);
}

/// LVLP 0x003A1255 — `<no edid>` — basic decode regression.
#[test]
fn lvlp_frag_mine_owned_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 178);

    let subs = subrecords_from(&[
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
        ("MODL", "576561706f6e735c4d696e655c467261674d696e6550726f6a656374696c652e6e696600"),
        ("MODT",
            "040000000400000000000000010000000100000081a1769c64647300fefd0268e29c796664647300fefd0268adc0787264647300fefd0268b329ad9264647300fefd0268131325cd6267736d6cf7e8e6"),
        ("ENLT", "ffffffff"),
        ("ENLS", "0000803f"),
    ]);

    let result = decode_record(&ctx, "LVLP", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Leveled Pack In"),
    );
    assert_only_drift_markers(&result, &["LVLD"]);
}

/// RESO 0x008B53B3 — `ATX_Resource_TeaWizard_resource` — drift-locked NAM5.
#[test]
fn reso_tea_wizard_resource_decodes_correctly() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 209);

    let subs = subrecords_from(&[
        (
            "EDID",
            "4154585f5265736f757263655f54656157697a6172645f7265736f7572636500",
        ),
        ("NAM1", "47548b00"),
        ("NAM2", "fe538b00"),
        ("NAM4", "01548b00"),
        ("NAM5", "01"),
    ]);

    let result = decode_record(&ctx, "RESO", &subs);

    assert_eq!(
        result.get("_record_type").and_then(|v| v.as_str()),
        Some("Resource"),
    );
    assert_only_drift_markers(&result, &["NAM5"]);
    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("ATX_Resource_TeaWizard_resource"),
    );
}
