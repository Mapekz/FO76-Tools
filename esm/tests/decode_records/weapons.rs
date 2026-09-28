//! Weapons and their modifications: WEAP, AMMO, OMOD, COBJ.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{
    assert_fully_decoded, assert_record_type, bare_ctx, bare_ctx_fv, decode_fixture, sr,
    subrecords_from,
};
use esm::decode::decode_record;
use esm::schema::Schema;
use serde_json::{Value, json};

/// OMOD 0x0085B998 — `HTO_mod_Legendary_Weapon4_Tarnished` — decodes to the
/// expected structure and values.
///
/// This test pins the full DATA decode path.  xEdit `-1` arrays use a 4-byte
/// count prefix.  The OMOD DATA subrecord contains two 4-byte-prefix inline
/// arrays — `Attach Parent Slots` and `Items` — both empty here.
///
/// The binary payload used here is the verbatim hex from `esm get
/// a reference ESM --formid 0x0085B998 --raw`.  The record uses
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

    // Verbatim subrecords from `esm get <esm>
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

    // Record type resolved, no subrecords left over, no raw_fallbacks.
    assert_record_type(&result, "Object Modification");
    // assert_fully_decoded catches _unmapped, _raw+reason, and _unknown_record —
    // stronger than the previous bare _unmapped.is_none() check.
    assert_fully_decoded(&result);

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

/// OMOD DATA `Properties` resolve `Function Type` and `Property` correctly
/// across Value-Type branches and Form-Type targets, and fall back to a bare
/// integer (never panicking, never losing the value) for an out-of-range
/// Property index.
///
/// Ground truth: `TES5Edit/Core/wbDefinitionsFO76.pas`:
///   - `wbOMODDataFunctionTypeDecider` (~line 3229) maps Value Type →
///     Function Type branch: `FormID,Int` (4) → branch 3 (`SET`/`REM`/`ADD`);
///     `Float` (1) → branch 0 (`SET`/`MUL+ADD`/`ADD`).
///   - `GetObjectModPropertyEnum` (~line 3160): for an OMOD record, the
///     DATA struct's `Form Type` field (stored as the target record's 4-char
///     signature) selects `wbArmorPropertyEnum` (19 entries, ~line 7197) /
///     `wbWeaponPropertyEnum` (115 entries, ~line 7228) /
///     `wbActorPropertyEnum` (6 entries, ~line 7219).
///
/// This test uses `Form Type = ARMO` with three Properties:
///   - `Properties[0]`: Value Type `Float`, Function Type 1 → `"MUL+ADD"`;
///     Property 5 → `"Value"` (the 6th, 0-indexed, entry of
///     `wbArmorPropertyEnum`).
///   - `Properties[1]`: Value Type `FormID,Int`, Function Type 1 → `"REM"`
///     (the FormID branch is `SET`/`REM`/`ADD`).
///   - `Properties[2]`: Property index 9999 — far beyond ARMO's 19-entry
///     table — must decode as a bare JSON number, not an enum object, while
///     the record as a whole still decodes fully (no `_raw`/`_unmapped`
///     markers).
#[test]
fn omod_data_properties_resolve_armo_form_type_and_out_of_range_property() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx(&schema);

    let data_hex = concat!(
        "00000000", // Include Count = 0
        "03000000", // Property Count = 3
        "00",       // Unknown Bool 1 = False
        "00",       // Unknown Bool 2 = False
        "41524d4f", // Form Type = ARMO (Armor)
        "00",       // Max Rank = 0
        "00",       // Level Tier Scaled Offset = 0
        "00000000", // Attach Point = null
        "00000000", // Attach Parent Slots count (u32) = 0
        "00000000", // Items count (u32) = 0
        // (Includes: Include Count = 0, so no Includes bytes follow)
        // Property[0]: VT=1(Float) Func=1(MUL+ADD) Prop=5(Value)
        "01", "000000", "01", "000000", "0500", "0000", "0000c03f", "00000000", "00000000",
        // Property[1]: VT=4(FormID,Int) Func=1(REM) Prop=0(Enchantments)
        "04", "000000", "01", "000000", "0000", "0000", "34120000", "07000000", "00000000",
        // Property[2]: VT=0(Int) Func=0(SET) Prop=9999 (out of range for ARMO's 19 entries)
        "00", "000000", "00", "000000", "0f27", "0000", "2a000000", "00000000", "00000000",
    );

    let subrecords = vec![sr("DATA", data_hex, 0)];
    let result = decode_record(&ctx, "OMOD", &subrecords);

    assert_record_type(&result, "Object Modification");
    assert_fully_decoded(&result);

    let data = result.get("Data").expect("Data struct must decode");
    assert_eq!(
        data.pointer("/Form Type/name").and_then(|v| v.as_str()),
        Some("Armor"),
        "Form Type"
    );

    let props = data
        .get("Properties")
        .and_then(|v| v.as_array())
        .expect("Properties must be an array");
    assert_eq!(props.len(), 3, "expected exactly 3 Properties");

    // Properties[0]: Float branch Function Type → "MUL+ADD"; ARMO Property 5 → "Value".
    assert_eq!(
        props[0]
            .pointer("/Function Type/name")
            .and_then(|v| v.as_str()),
        Some("MUL+ADD"),
        "P[0] Function Type (Float branch)"
    );
    assert_eq!(
        props[0].pointer("/Property/name").and_then(|v| v.as_str()),
        Some("Value"),
        "P[0] Property (ARMO index 5)"
    );

    // Properties[1]: FormID branch Function Type → "REM".
    assert_eq!(
        props[1]
            .pointer("/Function Type/name")
            .and_then(|v| v.as_str()),
        Some("REM"),
        "P[1] Function Type (FormID branch)"
    );

    // Properties[2]: out-of-range Property index preserved as a bare integer
    // (no enum object / "name"); `assert_fully_decoded` above already confirms
    // this did not trip a raw-fallback or _unmapped marker.
    let prop2 = props[2].get("Property").expect("Property must be present");
    assert!(
        prop2.is_number(),
        "out-of-range Property index must decode as a bare integer, got {prop2:?}"
    );
    assert_eq!(
        prop2.as_u64(),
        Some(9999),
        "P[2] Property raw value preserved"
    );
}

/// WEAP's own `Object Template` → `Object Mod Template Item` (OBTS) embeds
/// the same `wbObjectModProperties` array as OMOD's DATA struct, but WEAP
/// (unlike OMOD) has no `Form Type` field anywhere — per
/// `GetObjectModPropertyEnum` in the Pascal, when the containing record's own
/// signature isn't OMOD, the record's signature is used directly to pick
/// `wbWeaponPropertyEnum`/`wbArmorPropertyEnum`/`wbActorPropertyEnum`.
///
/// The schema bakes this in as a per-record-type `default_variant` on the
/// `Property` union (`tools/extractor/extract.py`'s
/// `_fixup_obts_property_default`): a WEAP-embedded OBTS's Property union
/// defaults to the WEAP table when the `Form Type` lookup key isn't found in
/// scope. This test locks that behaviour down directly against a WEAP record
/// that has no `Form Type` field at all.
#[test]
fn weap_obts_property_resolves_via_record_signature_not_form_type() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx(&schema);

    let obts_hex = concat!(
        "00000000", // Include Count = 0
        "01000000", // Property Count = 1
        "00",       // Level Min = 0
        "00",       // unused
        "00",       // Level Max = 0
        "00",       // unused
        "ffff",     // Parent Combination Index = -1
        "00",       // Default = False
        "00",       // Keywords count_prefix(1) = 0
        "00",       // Min Level For Ranks = 0
        "00",       // Alt Levels Per Tier = 0
        // (Includes: Include Count = 0, so no Includes bytes follow)
        // Property[0]: VT=1(Float) Func=0(SET) Prop=33(AimModelMinConeDegrees)
        "01", "000000", "00", "000000", "2100", "0000", "0000803f", "00000000", "00000000",
    );

    let subrecords = vec![
        sr("OBTE", "01000000", 0),
        sr("OBTF", "", 1),
        sr("FULL", "44656661756c7400", 2), // "Default\0"
        sr("OBTS", obts_hex, 3),
    ];

    let result = decode_record(&ctx, "WEAP", &subrecords);
    assert_record_type(&result, "Weapon");
    assert_fully_decoded(&result);

    // WEAP has no top-level "Form Type" field anywhere in its schema — the
    // Property union below must still resolve purely from the record's own
    // signature ("WEAP"), never from a "Form Type" sibling.
    assert!(
        result.get("Form Type").is_none(),
        "WEAP records have no Form Type field"
    );

    let item = result
        .pointer("/Object Template/Combinations/0/Combination/Object Mod Template Item")
        .expect("Object Mod Template Item must decode");
    let props = item
        .get("Properties")
        .and_then(|v| v.as_array())
        .expect("Properties must be an array");
    assert_eq!(props.len(), 1);
    assert_eq!(
        props[0].pointer("/Property/name").and_then(|v| v.as_str()),
        Some("AimModelMinConeDegrees"),
        "WEAP OBTS Property must resolve via record signature default, not a Form Type field"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Curated CI decode tests — verbatim subrecord bytes from
//   esm get a reference ESM --formid <ID> --raw
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

/// AMMO 0x00001BA4 — `crAmmoScorchbeastSonicAttack` — decodes to Ammunition fully.
///
/// 9-subrecord record (EDID OBND FULL ENLT ENLS DESC DATA DNAM ONAM).
/// Exercises the DNAM inline struct (Projectile FormID, Flags bitfield, Damage
/// float, Health uint) without any raw fallbacks.
#[test]
fn ammo_scorchbeast_sonic_attack_decodes_correctly() {
    let result = decode_fixture(
        "AMMO",
        175,
        &[
            (
                "EDID",
                "6372416d6d6f53636f7263686265617374536f6e696341747461636b00",
            ),
            ("OBND", "000000000000000000000000"),
            (
                "FULL",
                "3c49443d30303033453543343e53636f726368626561737420536f6e69632041747461636b20416d6d6f00",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            ("DESC", "00"),
            ("DATA", "0000000000000000"),
            ("DNAM", "94001300020000000000000000000000"),
            (
                "ONAM",
                "3c49443d30303033453543353e53636f726368626561737420536f6e69632041747461636b20416d6d6f00",
            ),
        ],
    );

    assert_record_type(&result, "Ammunition");
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

/// WEAP 0x000001F6 — `GasTrapDummy` — decodes to Weapon fully.
///
/// 8-subrecord record (EDID OBND FULL ENLT ENLS DESC DNAM CRDT); form_version
/// 176.  Simple weapon with a DNAM struct; exercises the weapon-data decode
/// path without any raw fallbacks.
#[test]
fn weap_gas_trap_dummy_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x000001F6 --raw` (form_version 176).
    let result = decode_fixture(
        "WEAP",
        176,
        &[
            ("EDID", "4761735472617044756d6d7900"),
            ("OBND", "000000000000000000000000"),
            (
                "FULL",
                "3c49443d30303032333945383e476173547261702044756d6d7900",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            ("DESC", "00"),
            (
                "DNAM",
                "000000000000803f0000803f0000803f0000803f0000fa430000fa440000000000000000000000000000003f000000000000000000000000400100000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000000000009a99993e00000000a041000000000000000002000000ffff7f7f00000000",
            ),
            ("CRDT", "000000400000803f00000000"),
        ],
    );

    assert_record_type(&result, "Weapon");
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
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x000FF964 --raw` (form_version 209).
    let result = decode_fixture(
        "WEAP",
        209,
        &[
            ("EDID", "5375706572536c6564676500"),
            ("OBND", "f6fff5fffbff2b0039000400"),
            ("PTRN", "510d0200"),
            (
                "FULL",
                "3c49443d34313031313439303e537570657220536c6564676500",
            ),
            (
                "MODL",
                "576561706f6e735c526f636b657448616d6d65725c526f636b657448616d6d65723173742e6e696600",
            ),
            (
                "MODT",
                "040000000c0000000000000006000000020000002fb9ccae64647300a0b863054c84c35464647300a0b8630503d8c24064647300a0b863052a0e974d64647300d126dddb493398b764647300d126dddb066f99a364647300d126dddbe804cd346464730038973cea29f70f0064647300582c553393cb280d6464730038973cea18864c4364647300d126dddbaff0dd356464730038973cea1d3117a064647300a0b86305edaca0006267736d2792e1e0d6597aee6267736db98041fd",
            ),
            ("XFLG", "10"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("MODD", "10"),
            ("ETYP", "423f0100"),
            ("BIDS", "8c390d00"),
            ("BAMT", "c1740700"),
            ("KSIZ", "0c000000"),
            (
                "KWDA",
                "a2b40e00a5a004009bc81000b0900600ea4a0f0024ab330013ab330012ab3300820e3d00b1175100a6d160006e7e7800",
            ),
            ("DESC", "00"),
            ("INRD", "38970b00"),
            ("EILV", "1e0000002800000032000000"),
            ("IBSD", "33d24e00"),
            (
                "APPR",
                "4c520500c8321e00a8894e00a9894e00aa894e00ab894e0064a247001e001a00",
            ),
            ("OBTE", "0a000000"),
            ("OBTF", ""),
            ("FULL", "3c49443d34313031313439313e44656661756c7400"),
            (
                "OBTS",
                "020000000000000000000000ffff01000000b1900600000001bf175100000001",
            ),
            ("OBTF", ""),
            ("FULL", "3c49443d34313031313439323e5374616e6461726400"),
            (
                "OBTS",
                "020000000000000000000000ffff0001a27f02000000b1900600000001bf175100000001",
            ),
            ("OBTF", ""),
            (
                "FULL",
                "3c49443d34313031313439333e5374616e64617264204570696300",
            ),
            (
                "OBTS",
                "020000000000000000000000010000012df4050000000b032300000001bf175100000001",
            ),
            ("OBTF", ""),
            ("FULL", "3c49443d34313031313439343e53696d706c6500"),
            (
                "OBTS",
                "020000000000000000000000ffff0001000323000000b1900600000001bf175100000001",
            ),
            ("OBTF", ""),
            (
                "OBTS",
                "050000000000000000000000ffff0001e4f167000000563d11000000017e184700000001736b60000000018d6c600000000152415200000001",
            ),
            (
                "OBTS",
                "020000000000000000000000ffff00015c287c000000ae900600000001b8815200000001",
            ),
            ("OBTF", ""),
            (
                "OBTS",
                "060000000000000000000000ffff0001655c88000000822e87000000017d574f00000001da7b1a00000001fc9952000000016a5c8800000001b1900600000001",
            ),
            (
                "OBTS",
                "060000000000000000000000ffff0001fa208f000000b19006000000017b574f00000001eb047900000001ec04790000000108d064000000015a276600000001",
            ),
            (
                "OBTS",
                "060000000000000000000000ffff00014a2a8f000000b1900600000001bf175100000001ea047900000001eb047900000001fd9952000000016ac25e00000001",
            ),
            (
                "OBTS",
                "060000000000000000000000ffff00014d2a8f000000b1900600000001ea047900000001da7b1a00000001ec0479000000011f316800000001254c6700000001",
            ),
            ("STOP", ""),
            (
                "MOD4",
                "576561706f6e735c526f636b657448616d6d65725c526f636b657448616d6d65723173745f312e6e696600",
            ),
            ("MO4T", "0400000000000000000000000000000000000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            (
                "DNAM",
                "000000000000803f0000803f00000000cdcc4c3f0000803f0000000000002041000000000000000000000000000080bf000000000000000000000000000100000000010005000070410000a041460000002800000000002f5c2400000000000000000000000000eb5c4000ae982400ac2615000000000000bb66e63f000000005042000000000000000002000000ffff7f7f640000000000000000000000cdcccc3dcdcccc3d00000000",
            ),
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
        ],
    );

    assert_record_type(&result, "Weapon");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("SuperSledge"),
        "Editor ID"
    );

    // MODT decodes as a structured wbModelInfo blob (12 textures, 0 addon nodes, 2
    // materials), not raw hex.
    let model_info = result
        .pointer("/Model/Model Information")
        .expect("Model Information");
    assert_eq!(model_info.pointer("/Counters/Textures"), Some(&json!(12)));
    assert_eq!(model_info.pointer("/Counters/Addon Nodes"), Some(&json!(0)));
    assert_eq!(model_info.pointer("/Counters/Materials"), Some(&json!(2)));
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

    // MO4T (1st Person Model) is an empty wbModelInfo blob — 0 textures, 0 addon nodes,
    // 0 materials — decodes structurally rather than as raw hex too.
    let mo4t = result
        .pointer("/1st Person Model/Model Information")
        .expect("1st Person Model Information");
    assert_eq!(mo4t.pointer("/Counters/Textures"), Some(&json!(0)));
    assert_eq!(
        mo4t.pointer("/Textures").and_then(|v| v.as_array()),
        Some(&Vec::new())
    );
}

/// Regression guard: `apply_weapon_bash_curve` is a record-level post-pass that
/// must see top-level `Damage Curve` and `Data.Secondary Damage` together.
/// SuperSledge is melee without `WeaponTypeAutomaticMelee`, so with curves
/// loaded it must emit `source: "ineligible"` rather than staying silent.
#[test]
fn weap_bash_damage_post_pass_sees_record_level_fields() {
    use esm::curves::CurveIndex;

    let schema = Schema::load_embedded().expect("embedded schema must load");
    let curves = CurveIndex::from_curves([(
        esm::FormId::new(8_450_583),
        esm::curves::Curve {
            edid: Some("CT_Test_Bash".into()),
            path: "Test.json".into(),
            points: vec![
                esm::curves::CurvePoint { x: 1.0, y: 10.0 },
                esm::curves::CurvePoint { x: 50.0, y: 50.0 },
            ],
        },
    )])
    .expect("in-memory curve index");

    let mut ctx = bare_ctx_fv(&schema, 209);
    ctx.curves = Some(&curves);

    // Minimal SuperSledge slice: EDID + DNAM (secondary bash damage) + CVT0.
    let subs = subrecords_from(&[
        ("EDID", "5375706572536c6564676500"),
        (
            "DNAM",
            "000000000000803f0000803f00000000cdcc4c3f0000803f0000000000002041000000000000000000000000000080bf000000000000000000000000000100000000010005000070410000a041460000002800000000002f5c2400000000000000000000000000eb5c4000ae982400ac2615000000000000bb66e63f000000005042000000000000000002000000ffff7f7f640000000000000000000000cdcccc3dcdcccc3d00000000",
        ),
        ("CVT0", "17f28000"),
    ]);

    let result = decode_record(&ctx, "WEAP", &subs);

    assert!(
        result
            .pointer("/Data/Secondary Damage")
            .and_then(Value::as_f64)
            .is_some_and(|v| v > 0.0),
        "fixture must carry non-zero Secondary Damage"
    );
    assert!(
        result.pointer("/Damage Curve/curve").is_some(),
        "Damage Curve must be inlined when curves are loaded"
    );
    assert_eq!(
        result
            .pointer("/Bash Damage/source")
            .and_then(|v| v.as_str()),
        Some("ineligible"),
        "melee without WeaponTypeAutomaticMelee must be explicitly ineligible"
    );
}

/// WEAP 0x00113083 — `crDLC04AnimatronicAlienBlaster` — decodes to Weapon
/// fully, including the newer-than-reference EAMT subrecord.
///
/// 39-subrecord record; form_version 205.  EAMT (Enchantment Amount, u16) is
/// absent from the Pascal WEAP definition but present in the live ESM — covered
/// by an `append` override.  This test locks that path clean.
#[test]
fn weap_animatronic_alien_blaster_decodes_correctly() {
    // Verbatim subrecords from `esm get <esm>
    // --formid 0x00113083 --raw` (form_version 205).
    let result = decode_fixture(
        "WEAP",
        205,
        &[
            (
                "EDID",
                "6372444c433034416e696d6174726f6e6963416c69656e426c617374657200",
            ),
            ("OBND", "fcfffcfff8ff040018000800"),
            ("PTRN", "dd1d0700"),
            (
                "FULL",
                "3c49443d30303030304634363e416e696d6174726f6e696320416c69656e20426c617374657200",
            ),
            (
                "MODL",
                "444c4330345c576561706f6e735c46616b65416c69656e426c61737465725c46616b65416c69656e426c61737465722e6e696600",
            ),
            (
                "MODT",
                "040000000d000000010000000300000003000000dae093a564647300607b9d87b9dd9c5f64647300607b9d87f6819d4b64647300607b9d87105544aa646473007b49fa3773684b50646473007b49fa37988c963d646473007b49fa373c344a44646473007b49fa371008ee15646473004feae24d7335e1ef646473004feae24d3c69e0fb646473004feae24d2280351b646473004feae24d22dd9fa4646473007b49fa37e86848ab64647300607b9d87c600000036cd804b6267736db95ede76bd56cc1d6267736dae9326475cf1e05c6267736d96cfa1bc",
            ),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            ("EITM", "3a961700"),
            ("EAMT", "1027"),
            ("ETYP", "423f0100"),
            ("BIDS", "ff830100"),
            ("YNAM", "23490300"),
            ("ZNAM", "c3f50100"),
            ("KSIZ", "0c000000"),
            (
                "KWDA",
                "8b96160048f90100ea4a0f0096f90f00ac3f1b00b3ad030085c41000e0881800797b1a006ac41c00453e1100c8a73300",
            ),
            ("DESC", "00"),
            ("INRD", "cf772300"),
            ("APPR", "9d240200992402009f240200c8321e00d7d40500"),
            ("OBTE", "01000000"),
            ("OBTS", "000000000000000000000000ffff01000000"),
            ("STOP", ""),
            (
                "MOD4",
                "444c4330345c576561706f6e735c46616b65416c69656e426c61737465725c46616b65416c69656e426c61737465725f312e6e696600",
            ),
            ("MO4T", "0400000000000000000000000000000000000000"),
            ("ENLT", "ffffffff"),
            ("ENLS", "0000803f"),
            (
                "AUUV",
                "01000000000048420000f04100009c429a99193fcdcccc3d00000000",
            ),
            (
                "DNAM",
                "97180c000000803f0000803f000000000000803f0000803f000080430000804300000000cdcccc3d000000000000003f000000000000000000000000000112002a000100090000c0400000004000000000000001000000000000000000000000000000cdda01000000000000000000ac2615000000000000abaa2a3f00000000a041000000000000000001000000ffff7f7f000000000000000000000000cdcccc3dcdcccc3d00000000",
            ),
            (
                "RGW3",
                "84301100acc527376666663fcdcc4c3fcdcc4c3ea2773740a8aaea3f9a99193e9a99193e0000803e0000803f000000000300000001",
            ),
            ("CRDT", "000000400000803f00000000"),
            ("INAM", "56181700"),
            ("LNAM", "98180c00"),
            ("WAMD", "e1881800"),
            ("DAMA", "810a060000000000c9e97600"),
            ("VCRY", "0f000000"),
            ("WTDT", "00000000"),
            ("WSAM", "00000040"),
        ],
    );

    assert_record_type(&result, "Weapon");
    assert_fully_decoded(&result);

    // EAMT = 0x2710 = 10000 (little-endian u16).
    assert_eq!(
        result.get("Enchantment Amount").and_then(|v| v.as_u64()),
        Some(10000),
        "Enchantment Amount"
    );
}

/// COBJ 0x00004170 — `co_Weapon_Ranged_AlienBlaster` — decodes to Constructible Object fully.
///
/// 13 subrecords: EDID, FVPA (7 components × 12 B = 84 B), REPR (4 repair entries × 12 B),
/// REPM, LRNM, DESC, CNAM, BNAM, GNAM, FNAM, DNAM, CIFK, RECF.  Exercises the
/// FVPA and REPR struct arrays plus the LRNM learn-method enum.  form_version 195.
///
/// Verbatim subrecords from `esm get <esm> 0x00004170 --raw`.
#[test]
fn cobj_weapon_ranged_alien_blaster_decodes_correctly() {
    let result = decode_fixture(
        "COBJ",
        195,
        &[
            (
                "EDID",
                "636f5f576561706f6e5f52616e6765645f416c69656e426c617374657200",
            ),
            (
                "FVPA",
                "a5fa010002000000ef531f0091fa01000a000000f3531f009bfa010004000000fc531f00\
             a0fa0100030000002c541f00a4fa0100040000002e541f00b7fa01000c000000b1541f00\
             94d2030009000000bf541f00",
            ),
            (
                "REPR",
                "a5fa010001000000ef531f00b7fa010002000000b1541f00\
             9bfa010001000000fc531f009ffa01000100000029541f00",
            ),
            ("REPM", "05"),
            ("LRNM", "04"),
            ("DESC", "00"),
            ("CNAM", "95f90f00"),
            ("BNAM", "49601f00"),
            ("GNAM", "0c704300"),
            ("FNAM", "241f2400"),
            ("DNAM", "0000000001000079"),
            ("CIFK", "04041600"),
            ("RECF", "0000000000000000"),
        ],
    );

    assert_record_type(&result, "Constructible Object");
    assert_fully_decoded(&result);

    assert_eq!(
        result.get("Editor ID").and_then(|v| v.as_str()),
        Some("co_Weapon_Ranged_AlienBlaster"),
        "Editor ID"
    );
    // FVPA: 7 component entries (7 × 12 B = 84 B).
    let components = result
        .get("Components")
        .and_then(|v| v.as_array())
        .expect("Components must be an array");
    assert_eq!(components.len(), 7, "Components count");
    assert_eq!(
        components[0].get("Component").and_then(|v| v.as_str()),
        Some("0x0001FAA5"),
        "Components[0] FormID"
    );
    // REPR: 4 repair entries (4 × 12 B = 48 B).
    let repair = result
        .get("Repair")
        .and_then(|v| v.as_array())
        .expect("Repair must be an array");
    assert_eq!(repair.len(), 4, "Repair count");
    // LRNM: 4 = Learned From Plan.
    assert_eq!(
        result
            .pointer("/Learn Method/name")
            .and_then(|v| v.as_str()),
        Some("Learned From Plan"),
        "Learn Method"
    );
    // CNAM: Created Object FormID.
    assert_eq!(
        result.get("Created Object").and_then(|v| v.as_str()),
        Some("0x000FF995"),
        "Created Object"
    );
}

/// the recipe's rare-craft leveled list.
#[test]
fn cobj_rare_craft_list_decodes() {
    // HIDE_co_Lead_Hard (0x008F531D).
    let result = decode_fixture(
        "COBJ",
        210,
        &[
            ("EDID", "484944455f636f5f4c6561645f4861726400"),
            ("YNAM", "86fd0500"),
            ("ZNAM", "87fd0500"),
            ("FVPA", "79fc8e0003000000000000000f000000e803000000000000"),
            ("REPM", "00"),
            ("LRNM", "03"),
            ("DESC", "00"),
            (
                "CTDA",
                "000000000000803f4a000000aa8f9400000000000000000000000000ffffffff",
            ),
            ("CNAM", "78fc8e00"),
            ("ENAM", "b28f9400"),
            ("BNAM", "f22f0100"),
            ("FNAM", "1f538f00"),
            ("DNAM", "0000000001000000"),
            ("RECF", "0000000000000000"),
        ],
    );

    assert_record_type(&result, "Constructible Object");
    assert_fully_decoded(&result);
    assert_eq!(
        result
            .get("ENAM (Rare Craft List?)")
            .and_then(Value::as_str),
        Some("0x00948FB2"),
        "ENAM -> HIDE_LL_CriticalCrafting_HardLead"
    );
    assert_eq!(
        result.get("Workbench Keyword").and_then(Value::as_str),
        Some("0x00012FF2"),
        "BNAM after ENAM still decodes"
    );
}
