use esm::decode::{decode_record, DecodeContext, ResolveDepth};
use esm::format::Signature;
use esm::reader::OwnedSubrecord;
use esm::schema::Schema;

fn bare_ctx(schema: &Schema) -> DecodeContext<'_> {
    DecodeContext {
        schema,
        form_version: 208,
        is_localized: false,
        localization: None,
        curves: None,
        resolve_depth: ResolveDepth::None,
        resolver: None,
        outer_struct: None,
        record_edid_char: None,
    }
}

fn sr(sig: &str, hex: &str, idx: usize) -> OwnedSubrecord {
    OwnedSubrecord {
        signature: Signature::from_slice(sig.as_bytes()),
        data: (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect(),
        doc_index: idx,
    }
}

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
        sr("EDID", "48544f5f6d6f645f4c6567656e646172795f576561706f6e345f5461726e697368656400", 0),
        sr("DURL", "3000", 1),
        sr("FULL", "3c49443d37313031343730303e5461726e697368656400", 2),
        sr("DESC", "00", 3),
        sr("ENLT", "ffffffff", 4),
        sr("ENLS", "0000803f", 5),
        sr("AUUV", "01000000000048420000f04100009c429a99193fcdcccc3d00000000", 6),
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
        sr("DATA", concat!(
            "01000000",                         // Include Count = 1
            "04000000",                         // Property Count = 4
            "00",                               // Unknown Bool 1 = False
            "00",                               // Unknown Bool 2 = False
            "57454150",                         // Form Type = WEAP (Weapon)
            "00",                               // Max Rank = 0
            "00",                               // Level Tier Scaled Offset = 0
            "aa894e00",                         // Attach Point = 0x004E89AA
            "00000000",                         // Attach Parent Slots count (u32) = 0
            "00000000",                         // Items count (u32) = 0
            // Includes[0]: Mod, MinLevel, Optional, Don't Use All
            "f7194500", "00", "00", "01",
            // Property[0]: VT=4(FormID,Int) Func=2(ADD) Prop=65(Enchantments)
            //              Value1=0x0085B97F  Value2=1  Step=CurveTable(null)
            "04", "000000", "02", "000000", "4100", "0000",
            "7fb98500", "01000000", "00000000",
            // Property[1]: VT=4(FormID,Int) Func=2(ADD) Prop=31(Keywords)
            //              Value1=0x0085B984  Value2=2  Step=CurveTable(null)
            "04", "000000", "02", "000000", "1f00", "0000",
            "84b98500", "02000000", "00000000",
            // Property[2]: VT=4(FormID,Int) Func=2(ADD) Prop=31(Keywords)
            //              Value1=0x005380CA  Value2=2  Step=CurveTable(null)
            "04", "000000", "02", "000000", "1f00", "0000",
            "ca805300", "02000000", "00000000",
            // Property[3]: VT=4(FormID,Int) Func=2(ADD) Prop=31(Keywords)
            //              Value1=0x001B3FAC  Value2=2  Step=CurveTable(null)
            "04", "000000", "02", "000000", "1f00", "0000",
            "ac3f1b00", "02000000", "00000000",
        ), 8),
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
        includes[0].pointer("/Optional/name").and_then(|v| v.as_str()),
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
