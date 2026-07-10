mod common;

use esm::curves::{ba2_internal_path, eval, CurveIndex, CurvePoint};
use esm::index::Index;
use esm::reader::EsmFile;
use esm::FormId;

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
