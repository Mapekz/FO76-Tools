//! Shared printing/JSON helpers used across the subcommand handler modules
//! (`query`, `refs`, `diff`, `daemon`, `inspect`): JSON/table rendering plus
//! the localization-override plumbing (`apply_strings_override`,
//! `esm_string_prefix`) and the daemon-mode override guard
//! (`bail_if_daemon_mode_overrides`) that several handlers need identically.

use esm::{Database, RecordRow};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) fn print_json(value: &Value, pretty: bool) {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else {
        println!("{}", serde_json::to_string(value).unwrap());
    }
}

pub(crate) fn print_record_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    // Compute column widths: max of header char-count and any cell char-count.
    let ncols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < ncols {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    // Print header.
    let header_parts: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            if i + 1 < ncols {
                format!("{:<width$}", h, width = widths[i])
            } else {
                h.to_string()
            }
        })
        .collect();
    println!("{}", header_parts.join("  "));
    // Print rows.
    for row in rows {
        let parts: Vec<String> = (0..ncols)
            .map(|i| {
                let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
                if i + 1 < ncols {
                    format!("{:<width$}", cell, width = widths[i])
                } else {
                    cell.to_string()
                }
            })
            .collect();
        println!("{}", parts.join("  "));
    }
}

/// Render a `&[RecordRow]` as an aligned table (FORMID / TYPE / EDID / NAME columns).
/// When `json` is true, emit the rows as JSON instead. `limit` is used only for
/// the "capped" stderr note.
pub(crate) fn print_record_rows(rows: &[RecordRow], limit: usize, json: bool, pretty: bool) {
    let capped = limit > 0 && rows.len() == limit;
    if json {
        print_json(&serde_json::to_value(rows).unwrap(), pretty);
    } else {
        let table_rows: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                vec![
                    r.form_id.clone(),
                    r.record_type.as_deref().unwrap_or("").to_string(),
                    r.editor_id.as_deref().unwrap_or("").to_string(),
                    r.name.as_deref().unwrap_or("").to_string(),
                ]
            })
            .collect();
        print_record_table(&["FORMID", "TYPE", "EDID", "NAME"], &table_rows);
    }
    if capped {
        eprintln!(
            "note: output capped at {} results; use --limit 0 to show all",
            limit
        );
    }
}

pub(crate) fn print_search_results(results: &[RecordRow], limit: usize, json: bool, pretty: bool) {
    print_record_rows(results, limit, json, pretty);
}

pub(crate) fn esm_string_prefix(esm_path: &Path) -> String {
    esm_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "game".to_string())
}

pub(crate) fn apply_strings_override(
    db: &mut Database,
    esm_path: &Path,
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) {
    if let Some(ba2_path) = localization_ba2 {
        let prefix = esm_string_prefix(esm_path);
        match esm::strings::Localization::from_ba2(&ba2_path, lang, &prefix) {
            Ok(loc) => db.set_localization(loc),
            Err(e) => eprintln!(
                "Warning: failed to load localization from {}: {}",
                ba2_path.display(),
                e
            ),
        }
    } else if let Some(dir) = strings_dir {
        let prefix = esm_string_prefix(esm_path);
        match esm::strings::Localization::from_loose_files(&dir, lang, &prefix) {
            Ok(loc) => db.set_localization(loc),
            Err(e) => eprintln!(
                "Warning: failed to load string tables from {}: {}",
                dir.display(),
                e
            ),
        }
    }
}

/// Bails when source-override flags were passed while daemon mode is active.
///
/// Source-override flags (`--localization-ba2`/`--strings-dir`/`--startup-ba2`/
/// `--curves-dir`, including `diff`'s per-side `_a`/`_b` variants) are
/// CLI-only — see `docs/adr/0008-source-overrides-cli-only.md`. The
/// daemon's `Registry` caches exactly one warm `Database` per canonical ESM
/// path, shared across every client; a per-request source override can't be
/// warmed into that shared cache, so there is no `Op` this could ever dispatch
/// to. Each call site computes `has_overrides` itself — a single flag-presence
/// check for `list`/`get`/`search`, an 8-way `_a`/`_b` coalesce for `diff` — and
/// passes the flag names to mention in the error.
pub(crate) fn bail_if_daemon_mode_overrides(
    has_overrides: bool,
    daemon_mode: bool,
    flags: &str,
) -> anyhow::Result<()> {
    if has_overrides && daemon_mode {
        anyhow::bail!(
            "{flags} are not supported in daemon mode; use --local to open the ESM directly"
        );
    }
    Ok(())
}
