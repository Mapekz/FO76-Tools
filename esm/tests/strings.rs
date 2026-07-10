//! Regression test for `Localization::from_ba2` against a real-archive-shaped
//! fixture (backslash-separated internal paths).

mod common;

use esm::strings::{Localization, StringKind};

/// Build a minimal `.strings` table: `count`(u32) + `data_size`(u32) +
/// `count` × (id: u32, offset: u32) + NUL-terminated UTF-8 data block.
fn make_strings_table(entries: &[(u32, &str)]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut index = Vec::new();
    for (id, text) in entries {
        let offset = data.len() as u32;
        data.extend_from_slice(text.as_bytes());
        data.push(0);
        index.push((*id, offset));
    }

    let mut buf = Vec::new();
    buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    for (id, offset) in index {
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&offset.to_le_bytes());
    }
    buf.extend_from_slice(&data);
    buf
}

/// An empty (but structurally valid) string table: `count = 0, data_size = 0`.
fn make_empty_table() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf
}

#[test]
fn from_ba2_resolves_backslash_separated_archive() {
    let strings = make_strings_table(&[(0xDEADBEEF, "Test String")]);
    let dlstrings = make_empty_table();
    let ilstrings = make_empty_table();

    let buf = common::make_ba2(&[
        (r"Strings\Foo_en.strings", &strings),
        (r"Strings\Foo_en.dlstrings", &dlstrings),
        (r"Strings\Foo_en.ilstrings", &ilstrings),
    ]);
    let path = common::write_ba2(&buf, "localization_from_ba2");

    let result = Localization::from_ba2(&path, "en", "Foo");
    std::fs::remove_file(&path).ok();

    let loc =
        result.expect("Localization::from_ba2 must succeed against a real-archive-shaped fixture");
    assert_eq!(
        loc.lookup(StringKind::Strings, 0xDEADBEEF),
        Some("Test String")
    );
    assert!(loc.dlstrings.is_empty());
    assert!(loc.ilstrings.is_empty());
}

/// Regression test: a single Localization BA2 can bundle more than one
/// product's string tables (observed in the real, retail archive — a shared
/// `nw_<locale>.strings` family sits alongside the game's own
/// `seventysix_<locale>.strings`). `from_ba2` must resolve strictly by the
/// caller-supplied prefix, not by scanning the archive for "the first
/// `strings/*_<locale>.strings` entry" — that would silently return the
/// wrong product's strings.
#[test]
fn from_ba2_picks_requested_prefix_not_first_match() {
    let other_product = make_strings_table(&[(0x1, "Wrong Product's String")]);
    let target = make_strings_table(&[(0xDEADBEEF, "Correct String")]);
    let empty = make_empty_table();

    let buf = common::make_ba2(&[
        // Deliberately listed before the target entry, and alphabetically
        // first, so a "take the first match" scan would pick this one.
        (r"Strings\Aaa_en.strings", &other_product),
        (r"Strings\Aaa_en.dlstrings", &empty),
        (r"Strings\Aaa_en.ilstrings", &empty),
        (r"Strings\Foo_en.strings", &target),
        (r"Strings\Foo_en.dlstrings", &empty),
        (r"Strings\Foo_en.ilstrings", &empty),
    ]);
    let path = common::write_ba2(&buf, "localization_multi_prefix");

    let result = Localization::from_ba2(&path, "en", "Foo");
    std::fs::remove_file(&path).ok();

    let loc = result.expect("from_ba2 must succeed when the requested prefix is present");
    assert_eq!(
        loc.lookup(StringKind::Strings, 0xDEADBEEF),
        Some("Correct String")
    );
    assert_eq!(loc.lookup(StringKind::Strings, 0x1), None);
}
