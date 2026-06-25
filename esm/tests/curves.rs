use esm::curves::{ba2_internal_path, eval, CurvePoint};

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
