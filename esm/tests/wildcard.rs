use esm::wildcard::wildcard_match;

// ── substring (no wildcard) ──────────────────────────────────────────────

#[test]
fn substring_present() {
    assert!(wildcard_match("aligned", "AlignedBarrel"));
    assert!(wildcard_match("aligned", "Barrel_Aligned"));
    assert!(wildcard_match("aligned", "barrel_aligned_vats"));
}

#[test]
fn substring_absent() {
    assert!(!wildcard_match("aligned", "SomethingElse"));
}

#[test]
fn substring_case_insensitive() {
    assert!(wildcard_match("ALIGNED", "aligned barrel"));
    assert!(wildcard_match("Aligned", "ALIGNED_BARREL"));
}

// ── prefix wildcard ──────────────────────────────────────────────────────

#[test]
fn prefix_match() {
    assert!(wildcard_match("HTO_*", "HTO_AlignedFrame"));
    assert!(wildcard_match("HTO_*", "HTO_")); // zero suffix
}

#[test]
fn prefix_no_match() {
    assert!(!wildcard_match("HTO_*", "NotHTO_Frame"));
}

#[test]
fn prefix_case_insensitive() {
    assert!(wildcard_match("hto_*", "HTO_AlignedFrame"));
    assert!(wildcard_match("HTO_*", "hto_frame"));
}

// ── suffix wildcard ──────────────────────────────────────────────────────

#[test]
fn suffix_match() {
    assert!(wildcard_match("*Rifle", "PlasmaRifle"));
    assert!(wildcard_match("*Rifle", "Rifle")); // zero prefix
}

#[test]
fn suffix_no_match() {
    assert!(!wildcard_match("*Rifle", "RifleScope"));
}

// ── both anchors ─────────────────────────────────────────────────────────

#[test]
fn both_anchors_match() {
    assert!(wildcard_match("Plasma*Rifle", "PlasmaAutoRifle"));
    assert!(wildcard_match("Plasma*Rifle", "PlasmaRifle")); // zero middle
}

#[test]
fn both_anchors_no_match() {
    assert!(!wildcard_match("Plasma*Rifle", "ElectronRifle"));
    assert!(!wildcard_match("Plasma*Rifle", "PlasmaScope"));
}

// ── multiple stars ───────────────────────────────────────────────────────

#[test]
fn multiple_stars() {
    assert!(wildcard_match("*auto*rifle*", "PlasmaAutoRifleScope"));
    assert!(wildcard_match("*auto*rifle*", "AutoRifle"));
    assert!(!wildcard_match("*auto*rifle*", "AutoScope"));
}

#[test]
fn consecutive_stars() {
    // double star collapses to single
    assert!(wildcard_match("HTO_**", "HTO_Barrel"));
    assert!(wildcard_match("**rifle**", "BigRifle"));
}

// ── match-all ────────────────────────────────────────────────────────────

#[test]
fn star_alone_matches_all() {
    assert!(wildcard_match("*", "AnythingAtAll"));
    assert!(wildcard_match("*", ""));
}

#[test]
fn empty_pattern_matches_all() {
    assert!(wildcard_match("", "AnythingAtAll"));
    assert!(wildcard_match("", ""));
}
