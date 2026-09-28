//! Shared printing/JSON helpers used across the subcommand handler modules
//! (`query`, `refs`, `diff`, `inspect`): JSON/table rendering plus
//! the localization-override plumbing (`apply_strings_override`,
//! `esm_string_prefix`) that several handlers need identically.

use esm::{Database, FormIdBase, RecordRow};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Rewrite an already-rendered FormID string (`"0x0000463F"`, from
/// `FormId::display()`) into `base`. Anything that doesn't parse as a
/// FormID — an EditorID, a carrier-walk label, an empty string — is
/// returned unchanged rather than erroring, since several callers pass a
/// string that is *usually* but not always a FormID (e.g. `RefList::target`
/// on a carrier-seeded walk is a text label, not a FormID).
pub(crate) fn render_form_id(s: &str, base: FormIdBase) -> String {
    match esm::parse_form_id_input(s) {
        Ok(fid) => fid.display_base(base),
        Err(_) => s.to_string(),
    }
}

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
/// the "capped" stderr note. `base` controls how the identity FormID (the
/// FORMID column / JSON `form_id` field) is rendered — see `--decimal`.
pub(crate) fn print_record_rows(
    rows: &[RecordRow],
    limit: usize,
    json: bool,
    pretty: bool,
    base: FormIdBase,
) {
    let capped = limit > 0 && rows.len() == limit;
    if json {
        let rows: Vec<RecordRow> = rows
            .iter()
            .cloned()
            .map(|mut r| {
                r.form_id = render_form_id(&r.form_id, base);
                r
            })
            .collect();
        print_json(&serde_json::to_value(&rows).unwrap(), pretty);
    } else {
        let table_rows: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                vec![
                    render_form_id(&r.form_id, base),
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

pub(crate) fn print_search_results(
    results: &[RecordRow],
    limit: usize,
    json: bool,
    pretty: bool,
    base: FormIdBase,
) {
    print_record_rows(results, limit, json, pretty, base);
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
