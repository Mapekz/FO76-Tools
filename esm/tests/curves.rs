mod common;

use esm::curves::{ba2_internal_path, eval, CurveIndex, CurvePoint};
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
