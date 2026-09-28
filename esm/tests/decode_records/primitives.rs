//! Scalar and list records: GLOB, KYWD, FLST, AVIF.
//!
//! See [`super`] for the fixture conventions shared by every module here.

use crate::common::{
    assert_fully_decoded, assert_record_type, bare_ctx_fv, decode_fixture, subrecords_from,
};
use esm::decode::decode_record;
use esm::schema::Schema;

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

    assert_record_type(&result, "Global");
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

/// GLOB `FLTV` — decoded f32 values are emitted free of f32->f64 widening noise.
///
/// Reuses the GLOB float path exercised above. serde_json's `f32 -> Value`
/// conversion widens to f64 before ryu formats it, which prints at f64
/// round-trip precision (52-bit mantissa) instead of the value's real f32
/// precision (23-bit mantissa) — surfacing bits that were never meaningful
/// (e.g. `0.10000000149011612` instead of `0.1`) unless the decoder corrects
/// for it. This asserts the correction runs and preserves real precision
/// rather than truncating it:
///   - clean values pass through unchanged (0.5, 0.0)
///   - f32->f64 widening noise on an exact-in-decimal value is erased (0.1)
///   - a value with genuine f32 precision beyond 5 decimal places keeps all
///     of it (1/3 as f32 -> 0.33333334, matching `f32::to_string()` exactly —
///     not truncated to 0.33333)
///   - a huge-magnitude value (f32::MAX, used elsewhere as a sentinel) also
///     comes out clean, since the correction isn't a decimal-place rounding
///     hack
#[test]
fn glob_float_value_has_no_f64_widening_noise() {
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let ctx = bare_ctx_fv(&schema, 157);

    let cases: &[(&str, f64)] = &[
        ("cdcccc3d", 0.1),            // 0.1f32, widens to 0.10000000149011612 raw
        ("abaaaa3e", 0.333_333_34),   // (1/3)f32, widens to 0.3333333432674408 raw
        ("0000003f", 0.5),            // 0.5f32, exact — must survive untouched
        ("00000000", 0.0),            // 0.0f32, exact
        ("ffff7f7f", 3.402_823_5e38), // f32::MAX, widens to a noisy 41-digit f64 raw
    ];

    for (fltv_hex, expected) in cases {
        let subs = subrecords_from(&[("EDID", "47616d655965617200"), ("FLTV", fltv_hex)]);
        let result = decode_record(&ctx, "GLOB", &subs);
        let value = result
            .get("Value")
            .and_then(|v| v.as_f64())
            .expect("Value must be present");
        assert_eq!(
            value, *expected,
            "FLTV {fltv_hex} should decode to exactly {expected}, got {value}"
        );
    }
}

/// KYWD 0x000000C1 — `SplineLink` — decodes to Keyword with RGBA color and type enum.
///
/// 3-subrecord record (EDID + CNAM + TNAM).  Exercises the keyword color struct
/// (4 bytes → {r,g,b,a}) and the keyword-type enum (0 → "None").
#[test]
fn kywd_spline_link_decodes_correctly() {
    // EDID = "SplineLink\0"   CNAM = RGBA(255,255,0,0)   TNAM = enum 0 (None)
    let result = decode_fixture(
        "KYWD",
        57,
        &[
            ("EDID", "53706c696e654c696e6b00"),
            ("CNAM", "ffff0000"),
            ("TNAM", "00000000"),
        ],
    );

    assert_record_type(&result, "Keyword");
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
    // EDID = "HelpManualPC\0"  +  100 LNAM FormID entries (verbatim LE u32 bytes)
    let result = decode_fixture(
        "FLST",
        209,
        &[
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
        ],
    );

    assert_record_type(&result, "FormID List");
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

/// AVIF 0x000002C2 — `Strength` — decodes to Actor Value Information fully.
///
/// 8-subrecord record (EDID DURL FULL DESC ANAM NAM0 NAM5 NAM6).  Exercises
/// the actor-value schema (name, description, abbreviation, float bounds) and
/// confirms the DURL string and both float range fields decode cleanly.
#[test]
fn avif_strength_decodes_correctly() {
    let result = decode_fixture(
        "AVIF",
        172,
        &[
            ("EDID", "537472656e67746800"),
            ("DURL", "312e3030303000"),
            ("FULL", "3c49443d30303031443733433e537472656e67746800"),
            (
                "DESC",
                "3c49443d30303031443733423e537472656e6774682069732061206d656173757265206f6620796f75722072617720706879736963616c20706f7765722e204974206166666563747320686f77206d75636820796f752063616e2063617272792c20616e64207468652064616d616765206f6620616c6c206d656c65652061747461636b732e00",
            ),
            ("ANAM", "3c49443d30303032333938353e53545200"),
            ("NAM0", "00000000"),
            ("NAM5", "0000803f"),
            ("NAM6", "ffff7f7f"),
        ],
    );

    assert_record_type(&result, "Actor Value Information");
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
// Per-type regression tests for the VMAD / COED / RACE-morph decode paths.
//
// All clean-type tests (assert_fully_decoded) lock the no-marker status.
// ════════════════════════════════════════════════════════════════════════════
