//! Generic game-data discovery: resolve an ESM path or data folder to the ESM
//! file plus any adjacent string tables and curve tables.
//!
//! When given a **directory**, the directory is scanned for exactly one `.esm`
//! file; zero or multiple ESMs produce a clear error.  When given a **file**,
//! it is used directly and sibling discovery proceeds in its parent directory.
//!
//! Discovery order:
//! - **Strings**: prefer a loose `strings/` subdirectory or the folder itself
//!   containing `<stem>_<locale>.strings`; else any `.ba2` in the folder whose
//!   name contains `"localization"`.
//! - **Curves**: prefer `misc/curvetables/json` or `curvetables/json` in the
//!   folder; else any `.ba2` in the folder whose name contains `"startup"`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Source for string tables found next to an ESM.
pub enum StringsSrc {
    /// Directory containing `<stem>_<locale>.{strings,dlstrings,ilstrings}`.
    Loose(PathBuf),
    /// GNRL BA2 archive containing the string tables.
    Ba2(PathBuf),
}

/// Source for curve table JSON files found next to an ESM.
pub enum CurvesSrc {
    /// Base directory whose `curvetables/json/` subdirectory holds the JSON.
    /// Pass this to [`CurveIndex::build_from_dir`](crate::curves::CurveIndex::build_from_dir).
    LooseBase(PathBuf),
    /// Startup BA2 archive.
    /// Pass this to [`CurveIndex::build`](crate::curves::CurveIndex::build).
    Ba2(PathBuf),
}

/// Resolved data sources for one ESM.
pub struct ResolvedSources {
    /// Resolved path to the `.esm` file.
    pub esm: PathBuf,
    /// Resolved string table source, if any was found.
    pub strings: Option<StringsSrc>,
    /// Resolved curve table source, if any was found.
    pub curves: Option<CurvesSrc>,
    /// Exact-case ESM file stem; used as prefix for loose string filenames
    /// (e.g. stem `"Game"` → `"Game_en.strings"`).
    pub loose_prefix: String,
    /// Locale used for the string table search.
    pub locale: String,
}

/// Resolve an input path (`.esm` file or folder) to its ESM and sibling sources.
///
/// **Idempotency guarantee:** a file input is never scanned for siblings of its
/// parent directory — only a directory input triggers a scan for the single ESM
/// within it.  This is required so that test fixtures sharing `std::env::temp_dir()`
/// are never misidentified as ambiguous.
pub fn resolve_sources(input: &Path, locale: &str) -> Result<ResolvedSources> {
    let (esm, folder) = if input.is_dir() {
        let esm = find_single_esm(input)
            .with_context(|| format!("scanning folder {}", input.display()))?;
        (esm, input.to_path_buf())
    } else {
        let folder = input.parent().unwrap_or(Path::new(".")).to_path_buf();
        (input.to_path_buf(), folder)
    };

    let loose_prefix = esm
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "game".to_string());

    let strings = find_strings(&folder, &loose_prefix, locale);
    let curves = find_curves(&folder);

    Ok(ResolvedSources {
        esm,
        strings,
        curves,
        loose_prefix,
        locale: locale.to_string(),
    })
}

fn find_single_esm(dir: &Path) -> Result<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("esm"))
                .unwrap_or(false)
        })
        .collect();
    found.sort();

    match found.len() {
        1 => Ok(found.into_iter().next().unwrap()),
        0 => bail!("no .esm file found in {}", dir.display()),
        _ => {
            let names: Vec<String> = found
                .iter()
                .filter_map(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .collect();
            bail!(
                "multiple .esm files in {}; pass the file path directly:\n  {}",
                dir.display(),
                names.join("\n  ")
            )
        }
    }
}

fn find_strings(folder: &Path, stem: &str, locale: &str) -> Option<StringsSrc> {
    // 1. Loose files in a strings/ subdirectory.
    let strings_dir = folder.join("strings");
    if strings_dir
        .join(format!("{}_{}.strings", stem, locale))
        .exists()
    {
        return Some(StringsSrc::Loose(strings_dir));
    }
    // 2. Loose files directly in the folder.
    if folder.join(format!("{}_{}.strings", stem, locale)).exists() {
        return Some(StringsSrc::Loose(folder.to_path_buf()));
    }
    // 3. A BA2 containing "localization" in its name.
    find_ba2_containing(folder, "localization").map(StringsSrc::Ba2)
}

fn find_curves(folder: &Path) -> Option<CurvesSrc> {
    // 1. misc/curvetables/json (typical game-data extraction layout).
    let misc_dir = folder.join("misc");
    if misc_dir.join("curvetables/json").is_dir() {
        return Some(CurvesSrc::LooseBase(misc_dir));
    }
    // 2. curvetables/json directly in the folder.
    if folder.join("curvetables/json").is_dir() {
        return Some(CurvesSrc::LooseBase(folder.to_path_buf()));
    }
    // 3. A BA2 containing "startup" in its name.
    find_ba2_containing(folder, "startup").map(CurvesSrc::Ba2)
}

/// Scan `dir` for the first `.ba2` (sorted) whose lowercase filename contains `keyword`.
pub fn find_ba2_containing(dir: &Path, keyword: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_lowercase();
            name.ends_with(".ba2") && name.contains(keyword)
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}
