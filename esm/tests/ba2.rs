//! Regression tests for `Ba2Archive` path-separator normalization.
//!
//! Real, Bethesda-shipped BA2 archives store internal paths with backslashes
//! (e.g. `strings\seventysix_en.strings`), but every consumer of
//! `Ba2Archive` (`strings.rs`, `curves.rs`) queries with forward slashes.
//! `Ba2Archive::open` must normalize separators so those queries match.

mod common;

use esm::ba2::Ba2Archive;

#[test]
fn backslash_names_are_normalized_to_forward_slash() {
    let buf = common::make_ba2(&[(r"Strings\SeventySix_en.strings", b"hello")]);
    let path = common::write_ba2(&buf, "backslash_normalize");

    let archive = Ba2Archive::open(&path).expect("open synthetic BA2");
    std::fs::remove_file(&path).ok();

    assert_eq!(
        archive.list()[0].name,
        "strings/seventysix_en.strings",
        "entry name must be forward-slash-normalized and lowercased"
    );
    assert_eq!(
        archive.read("strings/seventysix_en.strings").unwrap(),
        b"hello"
    );
}

#[test]
fn mixed_case_forward_slash_query_still_matches() {
    let buf = common::make_ba2(&[(r"Strings\SeventySix_en.strings", b"hello")]);
    let path = common::write_ba2(&buf, "mixed_case");

    let archive = Ba2Archive::open(&path).expect("open synthetic BA2");
    std::fs::remove_file(&path).ok();

    assert_eq!(
        archive.read("STRINGS/SeventySix_En.Strings").unwrap(),
        b"hello",
        "read() must match case-insensitively even with forward-slash query"
    );
}

#[test]
fn multiple_backslash_entries_do_not_collide() {
    let buf = common::make_ba2(&[
        (r"Strings\SeventySix_en.strings", b"one"),
        (
            r"Misc\CurveTables\JSON\Weapons\Weap_10mmSMGDMG.json",
            b"two",
        ),
    ]);
    let path = common::write_ba2(&buf, "no_collision");

    let archive = Ba2Archive::open(&path).expect("open synthetic BA2");
    std::fs::remove_file(&path).ok();

    assert_eq!(
        archive.read("strings/seventysix_en.strings").unwrap(),
        b"one"
    );
    assert_eq!(
        archive
            .read("misc/curvetables/json/weapons/weap_10mmsmgdmg.json")
            .unwrap(),
        b"two"
    );
}
