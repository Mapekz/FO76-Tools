mod common;

use esm::curves::{CurveIndex, CurvePoint, ba2_internal_path, eval, points_from_json, sum_range};
use esm::index::Index;
use esm::reader::EsmFile;
use esm::{Database, FormId, ResolveDepth};

#[test]
fn ba2_path_mapping() {
    assert_eq!(
        ba2_internal_path(r"Weapons\Weap_10mmSMGDMG.json"),
        "misc/curvetables/json/weapons/weap_10mmsmgdmg.json"
    );
    assert_eq!(
        ba2_internal_path("Creatures/Weapon/Damage_Universal_Tier24.json"),
        "misc/curvetables/json/creatures/weapon/damage_universal_tier24.json"
    );
}

#[test]
fn eval_clamp_low() {
    let pts = vec![
        CurvePoint { x: 1.0, y: 10.0 },
        CurvePoint { x: 10.0, y: 100.0 },
    ];
    assert_eq!(eval(&pts, 0.0), Some(10.0)); // clamp to first
}

#[test]
fn eval_clamp_high() {
    let pts = vec![
        CurvePoint { x: 1.0, y: 10.0 },
        CurvePoint { x: 10.0, y: 100.0 },
    ];
    assert_eq!(eval(&pts, 20.0), Some(100.0)); // clamp to last
}

#[test]
fn eval_interpolation() {
    let pts = vec![
        CurvePoint { x: 0.0, y: 0.0 },
        CurvePoint { x: 10.0, y: 100.0 },
    ];
    assert_eq!(eval(&pts, 5.0), Some(50.0));
}

#[test]
fn eval_empty() {
    assert_eq!(eval(&[], 5.0), None);
}

#[test]
fn eval_duplicate_x() {
    let pts = vec![
        CurvePoint { x: 5.0, y: 42.0 },
        CurvePoint { x: 5.0, y: 99.0 }, // duplicate x
    ];
    // Should return first y value (a.y when b.x == a.x)
    assert!(eval(&pts, 5.0).is_some());
}

/// Regression test: real Startup BA2 archives store curve JSON under
/// backslash-separated internal paths (e.g. `misc\curvetables\json\...`).
/// `CurveIndex::build` must still find them via `Ba2Archive::read`'s
/// forward-slash-normalized lookup.
///
/// Uses `EsmFile::open` + `Index::build` directly (not `Database::open`) to
/// avoid `discover::resolve_sources`'s sibling-BA2 folder scan picking up
/// unrelated fixtures from the shared system temp dir.
#[test]
fn build_resolves_backslash_separated_startup_ba2() {
    let mut subrecords = Vec::new();
    common::append_subrecord(&mut subrecords, b"EDID", &common::cstr("TestCurve"));
    common::append_subrecord(
        &mut subrecords,
        b"CRVE",
        &common::cstr(r"Weapons\Weap_10mmSMGDMG.json"),
    );

    let mut records = Vec::new();
    common::append_record(&mut records, b"CURV", 0x001, &subrecords);

    let mut esm_buf = common::tes4_header();
    esm_buf.extend(common::wrap_grup(b"CURV", &records));

    let esm_path = common::unique_temp_path("curves_ba2");
    std::fs::write(&esm_path, &esm_buf).expect("write temp esm");

    let esm = EsmFile::open(&esm_path).expect("open synthetic esm");
    let index = Index::build(&esm).expect("build index");

    let ba2_buf = common::make_ba2(&[(
        r"Misc\CurveTables\JSON\Weapons\Weap_10mmSMGDMG.json",
        br#"[{"x":0.0,"y":0.0},{"x":10.0,"y":100.0}]"#,
    )]);
    let ba2_path = common::write_ba2(&ba2_buf, "startup_curves");

    let result = CurveIndex::build(&esm, &index, &ba2_path);

    std::fs::remove_file(&esm_path).ok();
    std::fs::remove_file(&ba2_path).ok();

    let curve_index = result.expect("CurveIndex::build must succeed");
    let curve = curve_index
        .get(FormId::new(0x001))
        .expect("curve for FormID 0x001 must be found in the backslash-pathed BA2");
    assert_eq!(curve.eval(5.0), Some(50.0));
}

/// Build a fresh, empty directory under the system temp dir, unique to this
/// test process. Used (instead of writing sibling files directly into the
/// shared `std::env::temp_dir()`, as [`common::write_and_open`] does) so
/// `Database::open`'s `misc/curvetables/json/` sibling-folder discovery only
/// ever sees fixtures this test itself created — never another test's files
/// sharing the system temp dir, and vice versa.
fn unique_temp_dir(stem: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fo76_esm_test_dir_{stem}_{}_{n}",
        std::process::id()
    ))
}

/// Regression test for the CURV `get` inline-curve feature: fetching a CURV
/// record directly (no `--resolve` flag) must include its parsed curve
/// points under a `"Curve"` key, not just the raw `JSON File Path[/2]`
/// string — the referencing-record inline (`resolve_formid`'s CURV branch)
/// already did this; this covers the CURV record itself.
#[test]
fn get_curv_record_inlines_curve_points() {
    let dir = unique_temp_dir("curv_inline");
    std::fs::create_dir_all(&dir).expect("create isolated test dir");

    let mut subrecords = Vec::new();
    common::append_subrecord(&mut subrecords, b"EDID", &common::cstr("CT_Test_Curve"));
    common::append_subrecord(
        &mut subrecords,
        b"JASF",
        &common::cstr(r"LegendaryMods\Weapon_DamagePerKill.json"),
    );

    let mut records = Vec::new();
    common::append_record(&mut records, b"CURV", 0x001, &subrecords);

    let mut esm_buf = common::tes4_header();
    esm_buf.extend(common::wrap_grup(b"CURV", &records));

    let esm_path = dir.join("Test.esm");
    std::fs::write(&esm_path, &esm_buf).expect("write test esm");

    let curve_json_dir = dir.join("misc/curvetables/json/legendarymods");
    std::fs::create_dir_all(&curve_json_dir).expect("create curve json dir");
    std::fs::write(
        curve_json_dir.join("weapon_damageperkill.json"),
        br#"{"curve":[{"x":0,"y":0},{"x":1,"y":10},{"x":10,"y":100}]}"#,
    )
    .expect("write curve json fixture");

    let db = Database::open(&dir).expect("open db (dir form, auto-discovers misc/curvetables)");
    let result = db
        .record_by_formid_resolved(FormId::new(0x001), ResolveDepth::None)
        .expect("decode CURV record");

    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(
        result.fields["JSON File Path 2"],
        serde_json::json!(r"LegendaryMods\Weapon_DamagePerKill.json"),
        "path field must still be present alongside the inlined points"
    );
    assert_eq!(
        result.fields["Curve"],
        serde_json::json!([
            {"x": 0.0, "y": 0.0},
            {"x": 1.0, "y": 10.0},
            {"x": 10.0, "y": 100.0},
        ])
    );
}

/// Fallback: when no curve source was discovered (no sibling
/// `misc/curvetables/json/`, no Startup BA2), a CURV `get` must fall back to
/// today's path-only behavior — no `"Curve"` key at all — rather than erroring
/// or emitting an empty array.
#[test]
fn get_curv_record_without_curves_loaded_omits_curve_field() {
    let dir = unique_temp_dir("curv_no_curves");
    std::fs::create_dir_all(&dir).expect("create isolated test dir");

    let mut subrecords = Vec::new();
    common::append_subrecord(&mut subrecords, b"EDID", &common::cstr("CT_No_Curves"));
    common::append_subrecord(
        &mut subrecords,
        b"JASF",
        &common::cstr(r"LegendaryMods\Weapon_DamagePerKill.json"),
    );

    let mut records = Vec::new();
    common::append_record(&mut records, b"CURV", 0x002, &subrecords);

    let mut esm_buf = common::tes4_header();
    esm_buf.extend(common::wrap_grup(b"CURV", &records));

    let esm_path = dir.join("Test.esm");
    std::fs::write(&esm_path, &esm_buf).expect("write test esm");
    // Deliberately no misc/curvetables/json/ sibling — curves stays unloaded.

    let db = Database::open(&dir).expect("open db");
    let result = db
        .record_by_formid_resolved(FormId::new(0x002), ResolveDepth::None)
        .expect("decode CURV record");

    std::fs::remove_dir_all(&dir).ok();

    assert!(
        result.fields.get("Curve").is_none(),
        "no curve source was loaded, so no Curve field should be injected"
    );
    assert_eq!(
        result.fields["JSON File Path 2"],
        serde_json::json!(r"LegendaryMods\Weapon_DamagePerKill.json")
    );
}

// ─── sum_range ──────────────────────────────────────────────────────────────

#[test]
fn sum_range_basic_case() {
    // x=0..10 step 1 -> 0,10,20,...,100 -> sum = 550.
    let pts = vec![
        CurvePoint { x: 0.0, y: 0.0 },
        CurvePoint { x: 10.0, y: 100.0 },
    ];
    let total = sum_range(&pts, 0.0, 10.0, 1.0).expect("sum_range must succeed");
    // Epsilon (not exact equality): `eval`'s `t = (x - a.x) / (b.x - a.x)`
    // interpolation fraction is computed in f32, so non-power-of-2 fractions
    // (e.g. i/10) carry a small rounding error even though the underlying
    // math is exact in real numbers.
    assert!((total - 550.0).abs() < 1e-3, "got {total}");
}

#[test]
fn sum_range_rejects_non_positive_step() {
    let pts = vec![
        CurvePoint { x: 0.0, y: 0.0 },
        CurvePoint { x: 10.0, y: 100.0 },
    ];
    assert!(sum_range(&pts, 0.0, 10.0, 0.0).is_err());
    assert!(sum_range(&pts, 0.0, 10.0, -1.0).is_err());
}

#[test]
fn sum_range_rejects_end_before_start() {
    let pts = vec![
        CurvePoint { x: 0.0, y: 0.0 },
        CurvePoint { x: 10.0, y: 100.0 },
    ];
    assert!(sum_range(&pts, 10.0, 0.0, 1.0).is_err());
}

/// `CT_WorldPets_XP_LevelingProgression` — real curve data, fetched live via
/// `esm get CT_WorldPets_XP_LevelingProgression --json` against
/// `$FO76_ESM_PATH` (Data/20260903/SeventySix.esm).
fn worldpets_xp_leveling_progression() -> Vec<CurvePoint> {
    [
        (1.0, 0.0),
        (2.0, 420.0),
        (5.0, 500.0),
        (10.0, 600.0),
        (15.0, 700.0),
        (20.0, 800.0),
        (25.0, 900.0),
        (30.0, 1000.0),
        (35.0, 1100.0),
        (40.0, 1200.0),
        (45.0, 1300.0),
        (50.0, 1400.0),
        (55.0, 1550.0),
        (60.0, 1700.0),
        (65.0, 1850.0),
        (70.0, 2000.0),
        (75.0, 2150.0),
        (80.0, 2300.0),
        (85.0, 2450.0),
        (90.0, 2600.0),
        (95.0, 2750.0),
        (100.0, 2900.0),
        (105.0, 3050.0),
        (110.0, 3200.0),
        (115.0, 3350.0),
        (120.0, 3500.0),
        (125.0, 3650.0),
        (130.0, 3800.0),
        (135.0, 3950.0),
        (140.0, 4100.0),
        (145.0, 4250.0),
        (150.0, 4400.0),
        (155.0, 4600.0),
        (160.0, 4800.0),
        (165.0, 5000.0),
        (170.0, 5200.0),
        (175.0, 5400.0),
        (180.0, 5600.0),
        (185.0, 5800.0),
        (190.0, 6000.0),
        (195.0, 6200.0),
        (200.0, 6400.0),
    ]
    .into_iter()
    .map(|(x, y)| CurvePoint { x, y })
    .collect()
}

#[test]
fn sum_range_worldpets_xp_golden_values() {
    let pts = worldpets_xp_leveling_progression();

    let sum_1_200 = sum_range(&pts, 1.0, 200.0, 1.0).expect("sum_range 1..200");
    assert!((sum_1_200 - 607540.0).abs() < 0.5, "got {sum_1_200}");

    let sum_1_100 = sum_range(&pts, 1.0, 100.0, 1.0).expect("sum_range 1..100");
    assert!((sum_1_100 - 153290.0).abs() < 0.5, "got {sum_1_100}");

    let sum_101_200 = sum_range(&pts, 101.0, 200.0, 1.0).expect("sum_range 101..200");
    assert!((sum_101_200 - 454250.0).abs() < 0.5, "got {sum_101_200}");
}

// ─── points_from_json ───────────────────────────────────────────────────────

#[test]
fn points_from_json_reads_curv_records_capital_curve_key() {
    let v = serde_json::json!({
        "_record_type": "Curve Table",
        "Curve": [{"x": 0.0, "y": 0.0}, {"x": 10.0, "y": 100.0}],
    });
    let points = points_from_json(&v).expect("must find points under \"Curve\"");
    assert_eq!(points.len(), 2);
    assert_eq!(points[1].x, 10.0);
    assert_eq!(points[1].y, 100.0);
}

#[test]
fn points_from_json_reads_reference_stub_lowercase_curve_key() {
    let v = serde_json::json!({
        "formid": "0x00123456",
        "editor_id": "CT_Test",
        "curve_path": "Weapons/Weap_Test.json",
        "curve": [{"x": 1.0, "y": 10.0}, {"x": 2.0, "y": 20.0}],
    });
    let points = points_from_json(&v).expect("must find points under \"curve\"");
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].x, 1.0);
    assert_eq!(points[0].y, 10.0);
}

#[test]
fn points_from_json_none_when_neither_key_present() {
    let v = serde_json::json!({"formid": "0x00123456", "editor_id": "SomeNonCurveRef"});
    assert!(points_from_json(&v).is_none());
}

#[test]
fn points_from_json_empty_array_is_some_empty_not_none() {
    // A present-but-empty "curve" key is a genuinely different signal than
    // an absent one (curves not loaded) — see the doc comment on
    // `points_from_json`.
    let v = serde_json::json!({"curve": []});
    let points = points_from_json(&v);
    assert!(points.is_some());
    assert!(points.unwrap().is_empty());
}
