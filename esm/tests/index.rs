//! Integration tests for `index.rs`'s cross-cutting/public surface that
//! doesn't fit cleanly as a `#[cfg(test)]` unit test — currently just
//! `cache_inventory`, which needs a real `Database::open` (via a synthetic
//! ESM) to exercise the real `CACHE_VERSION`/layout fingerprints rather than
//! the module's own `#[cfg(test)]` `TEST_CACHE_VERSION` stand-in.

mod common;

use common::{make_minimal_esm, unique_temp_path};
use esm::progress::BuildStage;
use esm::{Database, FormId, SearchField, cache_inventory};
use std::io::Write;
use std::path::{Path, PathBuf};

fn write_esm(buf: &[u8], stem: &str) -> PathBuf {
    let tmp = unique_temp_path(stem);
    let mut f = std::fs::File::create(&tmp).expect("create temp esm");
    f.write_all(buf).expect("write temp esm");
    tmp
}

/// `write_and_open`-style temp files leave their `esm_cache/` sibling
/// directory behind (shared across other tests' ESMs in the same temp dir —
/// see `rkyvcache.rs`'s own test comments on this), so only remove this
/// test's own section files, keyed by its own ESM's file name, never the
/// shared directory itself.
fn cleanup(esm_path: &Path) {
    let _ = std::fs::remove_file(esm_path);
    if let Some(dir) = esm_path.parent() {
        let cache_dir = dir.join("esm_cache");
        if let Some(name) = esm_path.file_name() {
            for suffix in ["tree", "forms", "edid", "search", "xref"] {
                let mut section_name = name.to_os_string();
                section_name.push(".");
                section_name.push(suffix);
                let _ = std::fs::remove_file(cache_dir.join(section_name));
            }
        }
    }
}

#[test]
fn cache_inventory_before_any_open_is_fully_missing() {
    let tmp = write_esm(&make_minimal_esm(), "inventory_empty");

    let inv = cache_inventory(&tmp).expect("cache_inventory");
    assert!(inv.is_empty());
    assert!(!inv.is_complete());
    assert_eq!(inv.present.len(), 0);
    assert_eq!(inv.missing.len(), 5);

    cleanup(&tmp);
}

#[test]
fn cache_inventory_after_open_has_tree_and_forms_only() {
    let tmp = write_esm(&make_minimal_esm(), "inventory_open");
    let _db = Database::open(&tmp).expect("open db");

    let inv = cache_inventory(&tmp).expect("cache_inventory");
    assert!(inv.present.contains(&BuildStage::Forms));
    assert!(inv.present.contains(&BuildStage::Tree));
    assert!(!inv.present.contains(&BuildStage::Edid));
    assert!(!inv.present.contains(&BuildStage::Search));
    assert!(!inv.present.contains(&BuildStage::Xref));
    assert!(
        !inv.is_complete(),
        "tree+forms-only is the common 'partial' steady state, not complete"
    );

    cleanup(&tmp);
}

#[test]
fn cache_inventory_reflects_lazy_sections_after_use() {
    let tmp = write_esm(&make_minimal_esm(), "inventory_full");
    let mut db = Database::open(&tmp).expect("open db");

    // Each call triggers its matching `ensure_*_index`, regardless of
    // whether the lookup itself finds anything — `record_by_edid` on a
    // nonexistent EditorID still builds and persists the (empty) edid
    // section before failing the lookup.
    let _ = db.record_by_edid("nonexistent");
    let _ = db.search("*", &[], SearchField::Both, 10);
    let _ = db.referenced_by(FormId::new(1));

    let inv = cache_inventory(&tmp).expect("cache_inventory");
    assert!(
        inv.is_complete(),
        "expected all 5 sections present after edid+search+xref were built, got {inv:?}"
    );
    assert_eq!(inv.missing.len(), 0);

    cleanup(&tmp);
}
