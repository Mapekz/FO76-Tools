//! Extract BA2 archive entries to a directory tree on disk.
//!
//! Path-traversal sanitization: every backslash-separated component in the
//! archive-internal name is validated before joining under `out_dir`.
//! Components that are `..`, absolute, or look like Windows drive specifiers
//! (e.g. `C:`) are rejected so a hostile archive cannot escape the output
//! directory.

use crate::compress::Codec;
use crate::reader::{Ba2Archive, Ba2Entry};
use anyhow::{bail, Context, Result};
use globset::GlobSet;
use std::path::{Component, Path, PathBuf};

/// Validate a single backslash-separated archive path and return the
/// corresponding OS path relative to `out_dir`.
///
/// Returns an error if any path component would escape `out_dir`.
fn safe_output_path(out_dir: &Path, archive_name: &str) -> Result<PathBuf> {
    // Reject absolute paths before splitting so a leading `/` isn't silently
    // treated as an empty component and skipped.
    if archive_name.starts_with('/') || archive_name.starts_with('\\') {
        bail!(
            "archive entry '{}' is an absolute path — refusing to extract",
            archive_name
        );
    }
    let normalized = archive_name.replace('\\', "/");
    let mut result = out_dir.to_path_buf();
    for raw in normalized.split('/') {
        let component = Path::new(raw);
        match component.components().next() {
            Some(Component::Normal(c)) => result.push(c),
            Some(Component::ParentDir) => {
                bail!(
                    "archive entry '{}' contains '..' component — refusing to extract",
                    archive_name
                );
            }
            Some(Component::RootDir) | Some(Component::Prefix(_)) => {
                bail!(
                    "archive entry '{}' contains absolute path component — refusing to extract",
                    archive_name
                );
            }
            None | Some(Component::CurDir) => {
                // Skip empty segments and `.`.
                continue;
            }
        }
    }
    // Final sanity check: canonical path must stay within out_dir.
    // (We can't canonicalize a path that doesn't exist yet, so we check the
    // prefix on the constructed path string instead.)
    let out_str = out_dir.to_string_lossy();
    let result_str = result.to_string_lossy();
    if !result_str.starts_with(out_str.as_ref()) {
        bail!(
            "archive entry '{}' resolves outside output directory — refusing to extract",
            archive_name
        );
    }
    Ok(result)
}

/// Options for extraction.
pub struct ExtractOptions {
    /// Codec override for decompressing blobs.  `Auto` (default) sniffs each blob.
    pub codec: Codec,
    /// If set, only entries whose lowercased names match this glob set are extracted.
    pub filter: Option<GlobSet>,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        ExtractOptions {
            codec: Codec::Auto,
            filter: None,
        }
    }
}

/// Extract all matching entries from `archive` into `out_dir`.
///
/// Returns the number of files written.
pub fn extract_all(archive: &Ba2Archive, out_dir: &Path, opts: &ExtractOptions) -> Result<usize> {
    let mut count = 0;
    for entry in archive.list() {
        if !should_extract(entry, opts) {
            continue;
        }
        extract_entry(archive, entry, out_dir, opts.codec)?;
        count += 1;
    }
    Ok(count)
}

/// Extract a single named entry from `archive` into `out_dir`.
///
/// `name` is matched case-insensitively.  Returns the path written.
pub fn extract_one(
    archive: &Ba2Archive,
    name: &str,
    out_dir: &Path,
    codec: Codec,
) -> Result<PathBuf> {
    // Normalise the same way Ba2Archive::read() does: lowercase + `/` → `\`.
    let name_lower = name.to_lowercase().replace('/', "\\");
    let entry = archive
        .list()
        .iter()
        .find(|e| e.name == name_lower)
        .ok_or_else(|| anyhow::anyhow!("'{}' not found in archive", name))?;
    extract_entry(archive, entry, out_dir, codec)
}

fn should_extract(entry: &Ba2Entry, opts: &ExtractOptions) -> bool {
    if let Some(gs) = &opts.filter {
        // Archive entry names use backslash separators; globset on Unix expects
        // forward slashes, so normalise before matching.
        let fwd = entry.name.replace('\\', "/");
        gs.is_match(&fwd)
    } else {
        true
    }
}

fn extract_entry(
    archive: &Ba2Archive,
    entry: &Ba2Entry,
    out_dir: &Path,
    codec: Codec,
) -> Result<PathBuf> {
    let dest = safe_output_path(out_dir, &entry.name)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory '{}'", parent.display()))?;
    }
    let data = archive
        .read(&entry.name, codec)
        .with_context(|| format!("failed to read entry '{}'", entry.name))?;
    std::fs::write(&dest, &data)
        .with_context(|| format!("failed to write '{}'", dest.display()))?;
    Ok(dest)
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// Only `safe_output_path` tests live here — that function is module-private so
// integration tests in `tests/extract.rs` cannot reach it.  The public-API
// tests (extract_all, extract_one) live in `tests/extract.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_path_normal() {
        let base = Path::new("/tmp/out");
        let p = safe_output_path(base, "interface\\translate_de.txt").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/out/interface/translate_de.txt"));
    }

    #[test]
    fn safe_path_forward_slash() {
        let base = Path::new("/tmp/out");
        let p = safe_output_path(base, "strings/foo.strings").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/out/strings/foo.strings"));
    }

    #[test]
    fn safe_path_rejects_dotdot() {
        let base = Path::new("/tmp/out");
        assert!(safe_output_path(base, "..\\..\\etc\\passwd").is_err());
    }

    #[test]
    fn safe_path_rejects_absolute_forward_slash() {
        let base = Path::new("/tmp/out");
        assert!(safe_output_path(base, "/etc/passwd").is_err());
    }

    #[test]
    fn safe_path_rejects_absolute_backslash() {
        let base = Path::new("/tmp/out");
        // A leading backslash in an archive path must be rejected.
        assert!(safe_output_path(base, "\\etc\\passwd").is_err());
    }

    /// Empty segments (`dir//file`) and CurDir components (`./file`) are
    /// silently skipped, producing a clean path under `out_dir`.
    #[test]
    fn safe_path_skips_empty_and_dot_segments() {
        let base = Path::new("/tmp/out");
        // Empty segment between two slashes.
        let p = safe_output_path(base, "dir//file.txt").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/out/dir/file.txt"));
        // CurDir segment (`.`) is skipped.
        let p2 = safe_output_path(base, "./dir/file.txt").unwrap();
        assert_eq!(p2, PathBuf::from("/tmp/out/dir/file.txt"));
    }
}
