//! `diff` subcommand handler and its text-mode rendering (the JSON path is
//! just `output::print_json` on the raw `DiffResult`).

use anyhow::Context as _;
use esm::ipc::Op;
use esm::{BodyDetail, Database, DiffResult};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::Backend;
use crate::output::{bail_if_daemon_mode_overrides, esm_string_prefix, print_json};

/// Resolve localization for one ESM side, or bail loudly if no string tables
/// can be found.  Precedence:
///   1. Explicit BA2 via `--localization-ba2` → `Localization::from_ba2`.
///   2. Loose files: search `--strings-dir`, then `<esm-dir>/strings`,
///      then `<esm-dir>` for `<stem>_<lang>.strings`.
///   3. Any `*localization*.ba2` in `<esm-dir>`.
///   4. Bail with an actionable error message — output without strings is noise.
fn resolve_localization_or_bail(
    esm_path: &Path,
    strings_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) -> anyhow::Result<esm::strings::Localization> {
    use esm::strings::Localization;

    let esm_dir = esm_path.parent().unwrap_or(Path::new("."));
    let stem = esm_string_prefix(esm_path);

    // 1. Explicit BA2.
    if let Some(ba2) = strings_ba2 {
        return Localization::from_ba2(&ba2, lang, &stem)
            .with_context(|| format!("loading localization from {}", ba2.display()));
    }

    // 2. Loose files — search ordered dirs until we find <stem>_<lang>.strings.
    let search_dirs: Vec<PathBuf> = if let Some(dir) = strings_dir {
        vec![dir]
    } else {
        vec![esm_dir.join("strings"), esm_dir.to_path_buf()]
    };

    for dir in &search_dirs {
        let probe = dir.join(format!("{}_{}.strings", stem, lang));
        if probe.exists() {
            return Localization::from_loose_files(dir, lang, &stem).with_context(|| {
                format!(
                    "loading loose strings for '{}' from {}",
                    stem,
                    dir.display()
                )
            });
        }
    }

    // 3. Any *localization*.ba2 in the esm directory.
    if let Some(ba2) = esm::discover::find_ba2_containing(esm_dir, "localization") {
        return Localization::from_ba2(&ba2, lang, &stem)
            .with_context(|| format!("loading localization BA2 from {}", ba2.display()));
    }

    // 4. Nothing found — fail loudly.
    let dirs_tried: Vec<String> = search_dirs
        .iter()
        .map(|d| d.display().to_string())
        .collect();
    anyhow::bail!(
        "No string tables found for '{stem}' (lang={lang}).\n\
         Looked for loose files in: {dirs}\n\
         Also scanned '{esm_dir}' for a Localization BA2 — none found.\n\
         \n\
         Refusing to diff without string tables — output would contain unresolved LString IDs.\n\
         \n\
         Fix options:\n  \
           --strings-dir <DIR>        path to a directory with {stem}_{lang}.strings/.dlstrings/.ilstrings\n  \
           --localization-ba2 <BA2>   path to a Localization BA2 archive",
        stem = stem,
        lang = lang,
        dirs = dirs_tried.join(", "),
        esm_dir = esm_dir.display(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_diff(
    backend: &mut Backend,
    file_a: &Path,
    file_b: &Path,
    record_type: Option<&str>,
    as_json: bool,
    pretty: bool,
    localization_ba2: Option<PathBuf>,
    localization_ba2_a: Option<PathBuf>,
    localization_ba2_b: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    strings_dir_a: Option<PathBuf>,
    strings_dir_b: Option<PathBuf>,
    lang: &str,
    startup_ba2: Option<PathBuf>,
    startup_ba2_a: Option<PathBuf>,
    startup_ba2_b: Option<PathBuf>,
    curves_dir: Option<PathBuf>,
    curves_dir_a: Option<PathBuf>,
    curves_dir_b: Option<PathBuf>,
    bodies: BodyDetail,
    keep_noise: bool,
    exclude_type: Vec<String>,
    daemon_mode: bool,
) -> anyhow::Result<()> {
    let options = esm::query::diff_options(bodies, !keep_noise, &exclude_type);

    // Coalesce per-side over shared for each source kind.
    let lba2_a = localization_ba2_a.or_else(|| localization_ba2.clone());
    let lba2_b = localization_ba2_b.or_else(|| localization_ba2.clone());
    let sd_a = strings_dir_a.or_else(|| strings_dir.clone());
    let sd_b = strings_dir_b.or_else(|| strings_dir.clone());
    let sb_a = startup_ba2_a.or_else(|| startup_ba2.clone());
    let sb_b = startup_ba2_b.or_else(|| startup_ba2.clone());
    let cd_a = curves_dir_a.or_else(|| curves_dir.clone());
    let cd_b = curves_dir_b.or_else(|| curves_dir.clone());

    let force_local = lba2_a.is_some()
        || lba2_b.is_some()
        || sd_a.is_some()
        || sd_b.is_some()
        || sb_a.is_some()
        || sb_b.is_some()
        || cd_a.is_some()
        || cd_b.is_some();

    if force_local {
        bail_if_daemon_mode_overrides(
            force_local,
            daemon_mode,
            "--localization-ba2*/--strings-dir*/--startup-ba2*/--curves-dir*",
        )?;

        // Resolve folder → ESM so that esm_string_prefix/resolve_localization_or_bail
        // receive the actual .esm path (not a folder).
        let esm_a = esm::discover::resolve_sources(file_a, "en")?.esm;
        let esm_b = esm::discover::resolve_sources(file_b, "en")?.esm;

        let mut db_a = Database::open(&esm_a)?;
        let mut db_b = Database::open(&esm_b)?;

        // Load localization per side — each side is independently optional.
        //
        // A side whose TES4 header lacks the Localized flag stores FULL/DESC
        // inline and never consults a string table, so requiring one there
        // would fail a diff that needs none. The two sides can genuinely
        // differ: a PTS build may ship localized while the release build it is
        // diffed against does not.
        if lba2_a.is_some() || sd_a.is_some() {
            if db_a.is_localized {
                let loc_a = resolve_localization_or_bail(&esm_a, lba2_a, sd_a, lang)?;
                db_a.set_localization(loc_a);
            } else {
                eprintln!(
                    "note: {} is not localized (TES4 Localized flag unset); \
                     ignoring the string tables supplied for it",
                    esm_a.display()
                );
            }
        }
        if lba2_b.is_some() || sd_b.is_some() {
            if db_b.is_localized {
                let loc_b = resolve_localization_or_bail(&esm_b, lba2_b, sd_b, lang)?;
                db_b.set_localization(loc_b);
            } else {
                eprintln!(
                    "note: {} is not localized (TES4 Localized flag unset); \
                     ignoring the string tables supplied for it",
                    esm_b.display()
                );
            }
        }

        // Load curves per side.
        if let Some(ba2) = sb_a {
            db_a.load_curves(&ba2)?;
        } else if let Some(dir) = cd_a {
            db_a.load_curves_from_dir(&dir)?;
        }
        if let Some(ba2) = sb_b {
            db_b.load_curves(&ba2)?;
        } else if let Some(dir) = cd_b {
            db_b.load_curves_from_dir(&dir)?;
        }

        let record_type_owned = record_type.map(str::to_string);
        let v = esm::ipc::diff_locked(&db_a, &db_b, &options, &record_type_owned)?;
        let mut result: DiffResult = serde_json::from_value(v)?;

        return print_diff(file_a, file_b, &mut result, record_type, as_json, pretty);
    }

    // No local flags — use the backend path (daemon or local).
    let v = backend.run(
        file_a,
        Op::Diff {
            b: file_b.to_path_buf(),
            record_type: record_type.map(|s| s.to_string()),
            options,
        },
    )?;
    let mut result: DiffResult = serde_json::from_value(v)?;
    print_diff(file_a, file_b, &mut result, record_type, as_json, pretty)
}

fn print_diff(
    file_a: &Path,
    file_b: &Path,
    result: &mut DiffResult,
    record_type: Option<&str>,
    as_json: bool,
    pretty: bool,
) -> anyhow::Result<()> {
    if as_json {
        print_json(&serde_json::to_value(result)?, pretty);
        return Ok(());
    }

    println!("A: {}", file_a.display());
    println!("B: {}", file_b.display());
    println!();
    println!("Summary:");
    println!("  Added:   {}", result.added.len());
    println!("  Removed: {}", result.removed.len());
    println!("  Changed: {}", result.changed.len());

    if record_type.is_none() {
        let mut added_by_type: BTreeMap<&str, usize> = BTreeMap::new();
        let mut removed_by_type: BTreeMap<&str, usize> = BTreeMap::new();
        let mut changed_by_type: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &result.added {
            *added_by_type.entry(&s.record_type).or_default() += 1;
        }
        for s in &result.removed {
            *removed_by_type.entry(&s.record_type).or_default() += 1;
        }
        for d in &result.changed {
            *changed_by_type.entry(&d.stub.record_type).or_default() += 1;
        }

        let all_types: std::collections::BTreeSet<&str> = added_by_type
            .keys()
            .chain(removed_by_type.keys())
            .chain(changed_by_type.keys())
            .copied()
            .collect();
        if !all_types.is_empty() {
            println!();
            println!("By record type:");
            for t in all_types {
                println!(
                    "  {}: +{} -{} ~{}",
                    t,
                    added_by_type.get(t).copied().unwrap_or(0),
                    removed_by_type.get(t).copied().unwrap_or(0),
                    changed_by_type.get(t).copied().unwrap_or(0),
                );
            }
        }
    }

    if !result.added.is_empty() {
        println!();
        println!("Added ({}):", result.added.len());
        for s in &result.added {
            let edid = s.editor_id.as_deref().unwrap_or("<no edid>");
            if let Some(name) = &s.name {
                println!("  [{}] {} \"{}\"", s.form_id, edid, name);
            } else {
                println!("  [{}] {}", s.form_id, edid);
            }
        }
    }
    if !result.removed.is_empty() {
        println!();
        println!("Removed ({}):", result.removed.len());
        for s in &result.removed {
            let edid = s.editor_id.as_deref().unwrap_or("<no edid>");
            if let Some(name) = &s.name {
                println!("  [{}] {} \"{}\"", s.form_id, edid, name);
            } else {
                println!("  [{}] {}", s.form_id, edid);
            }
        }
    }
    if !result.changed.is_empty() {
        println!();
        println!("Changed ({}):", result.changed.len());
        for d in &result.changed {
            let edid = d.stub.editor_id.as_deref().unwrap_or("<no edid>");
            if let Some(prev) = &d.prev_editor_id {
                // EDID rename this patch (e.g. deprecation prefix added)
                println!("  [{}] {} (was: {})", d.stub.form_id, edid, prev);
            } else if let Some(name) = &d.stub.name {
                println!("  [{}] {} \"{}\"", d.stub.form_id, edid, name);
            } else {
                println!("  [{}] {}", d.stub.form_id, edid);
            }
            print_field_changes(&d.field_changes, "    ");
        }
    }
    Ok(())
}

fn print_field_changes(changes: &Value, indent: &str) {
    if let Some(obj) = changes.as_object() {
        for (key, val) in obj {
            if let Some(inner) = val.as_object() {
                if let Some(array_diff) = inner.get("_array_diff").and_then(Value::as_object) {
                    print_array_diff(key, array_diff, indent);
                } else if inner.contains_key("from") && inner.contains_key("to") {
                    println!(
                        "{}  {}: {} \u{2192} {}",
                        indent,
                        key,
                        format_val(&inner["from"]),
                        format_val(&inner["to"])
                    );
                } else {
                    println!("{}  {}:", indent, key);
                    print_field_changes(val, &format!("{}  ", indent));
                }
            }
        }
    }
}

/// Typed form of an `_array_diff` envelope's `"strategy"` field (see
/// `diff.rs`'s `array_diff`/`unkeyed_array_diff`/`keyed_array_diff` etc. for
/// the four cases this crate's diff pipeline actually produces — `keyed`,
/// `positional`, `set`, `unkeyed`, per `esm/CLAUDE.md`'s `diff.rs` entry).
/// `diff.rs` itself never keeps a Rust-side enum for this — every strategy
/// is written straight to an untyped `serde_json::Value` string at the point
/// it's decided, so this is CLI-local: a named, testable home for the
/// strategy dispatch, kept out of `print_array_diff` itself. `Other` covers
/// a future/unrecognized strategy string rather than panicking or dropping
/// it silently.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArrayDiffStrategy {
    Keyed { key_fields: Vec<String> },
    Positional,
    Set,
    Unkeyed,
    Other(String),
}

impl ArrayDiffStrategy {
    fn parse(array_diff: &serde_json::Map<String, Value>) -> Self {
        match array_diff.get("strategy").and_then(Value::as_str) {
            Some("keyed") => {
                let key_fields = array_diff
                    .get("key_fields")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                ArrayDiffStrategy::Keyed { key_fields }
            }
            Some("positional") => ArrayDiffStrategy::Positional,
            Some("set") => ArrayDiffStrategy::Set,
            Some("unkeyed") => ArrayDiffStrategy::Unkeyed,
            Some(other) => ArrayDiffStrategy::Other(other.to_string()),
            None => ArrayDiffStrategy::Other("?".to_string()),
        }
    }

    /// Human-readable description used in the summary line's parenthetical,
    /// e.g. "keyed by Reference, Minimum Level" / "positional" / "unkeyed".
    fn describe(&self) -> String {
        match self {
            ArrayDiffStrategy::Keyed { key_fields } if !key_fields.is_empty() => {
                format!("keyed by {}", key_fields.join(", "))
            }
            ArrayDiffStrategy::Keyed { .. } => "keyed".to_string(),
            ArrayDiffStrategy::Positional => "positional".to_string(),
            ArrayDiffStrategy::Set => "set".to_string(),
            ArrayDiffStrategy::Unkeyed => "unkeyed".to_string(),
            ArrayDiffStrategy::Other(s) => s.clone(),
        }
    }
}

/// The decided contents of an `_array_diff` envelope's one-line summary —
/// pulled out of `print_array_diff` so the bucket-counting/strategy-dispatch
/// logic is unit-testable without capturing stdout.
#[derive(Debug, PartialEq, Eq)]
struct ArrayDiffSummary {
    /// "+3 −1 ~2" (only non-zero buckets, space-joined) or "no changes".
    counts: String,
    strategy_desc: String,
    count_from: Option<u64>,
    count_to: Option<u64>,
}

fn summarize_array_diff(array_diff: &serde_json::Map<String, Value>) -> ArrayDiffSummary {
    let added_count = array_diff
        .get("added")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let removed_count = array_diff
        .get("removed")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let changed_count = array_diff
        .get("changed")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    let mut buckets = Vec::new();
    if added_count > 0 {
        buckets.push(format!("+{added_count}"));
    }
    if removed_count > 0 {
        buckets.push(format!("\u{2212}{removed_count}"));
    }
    if changed_count > 0 {
        buckets.push(format!("~{changed_count}"));
    }
    let counts = if buckets.is_empty() {
        "no changes".to_string()
    } else {
        buckets.join(" ")
    };

    ArrayDiffSummary {
        counts,
        strategy_desc: ArrayDiffStrategy::parse(array_diff).describe(),
        count_from: array_diff.get("count_from").and_then(Value::as_u64),
        count_to: array_diff.get("count_to").and_then(Value::as_u64),
    }
}

/// Render one `{"_array_diff": {...}}` envelope (see `json_diff`/`array_diff`
/// in `diff.rs`) as a one-line summary plus compact per-element detail lines,
/// e.g.:
///
/// ```text
///     Entries: +3 −1 ~2 entries (12 → 13, keyed by Reference, Minimum Level)
///       + {"Leveled List Entry":{"Reference":"0x0001A2B3", ...}}
///       - {"Leveled List Entry":{"Reference":"0x0001A2B4", ...}}
///       ~ Reference=0x0001A2B5, Minimum Level=10
///         Count: 1 → 2
/// ```
fn print_array_diff(field: &str, array_diff: &serde_json::Map<String, Value>, indent: &str) {
    let summary = summarize_array_diff(array_diff);

    match (summary.count_from, summary.count_to) {
        (Some(from), Some(to)) => println!(
            "{indent}  {field}: {} entries ({from} \u{2192} {to}, {})",
            summary.counts, summary.strategy_desc
        ),
        _ => println!(
            "{indent}  {field}: {} entries ({})",
            summary.counts, summary.strategy_desc
        ),
    }

    let elem_indent = format!("{indent}    ");
    if let Some(added) = array_diff.get("added").and_then(Value::as_array) {
        for elem in added {
            println!("{elem_indent}+ {}", compact_value(elem));
        }
    }
    if let Some(removed) = array_diff.get("removed").and_then(Value::as_array) {
        for elem in removed {
            println!("{elem_indent}- {}", compact_value(elem));
        }
    }
    if let Some(changed) = array_diff.get("changed").and_then(Value::as_array) {
        for entry in changed {
            let key = entry.get("key").cloned().unwrap_or(Value::Null);
            println!("{elem_indent}~ {}", format_key(&key));
            if let Some(changes) = entry.get("changes") {
                print_field_changes(changes, &format!("{elem_indent}  "));
            }
        }
    }
}

/// Render a keyed/positional array-diff entry's `"key"` object as compact
/// `field=value, field=value` text (e.g. `Reference=0x0001A2B3, Minimum
/// Level=10`, or `index=3` for a positional pairing).
fn format_key(key: &Value) -> String {
    match key.as_object() {
        Some(map) if !map.is_empty() => map
            .iter()
            .map(|(k, v)| format!("{}={}", k, compact_value(v)))
            .collect::<Vec<_>>()
            .join(", "),
        _ => compact_value(key),
    }
}

/// Compact single-line rendering of a JSON value for the `_array_diff`
/// added/removed detail lines, truncated to ~100 characters so one oversized
/// element (e.g. a fully-decoded leveled-list entry) doesn't spill the
/// terminal. Reuses [`format_val`] for scalars; falls back to compact JSON
/// (not pretty-printed) for objects/arrays.
fn compact_value(v: &Value) -> String {
    let s = format_val(v);
    const MAX_CHARS: usize = 100;
    if s.chars().count() > MAX_CHARS {
        let truncated: String = s.chars().take(MAX_CHARS).collect();
        format!("{truncated}\u{2026}")
    } else {
        s
    }
}

fn format_val(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ArrayDiffStrategy / summarize_array_diff: the four `_array_diff`
    // strategies `diff.rs` produces ───────────────────────────────────────

    fn array_diff_obj(json: Value) -> serde_json::Map<String, Value> {
        json.as_object().unwrap().clone()
    }

    #[test]
    fn array_diff_strategy_keyed_with_key_fields() {
        let ad = array_diff_obj(serde_json::json!({
            "strategy": "keyed",
            "key_fields": ["Reference", "Minimum Level"],
        }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(
            strategy,
            ArrayDiffStrategy::Keyed {
                key_fields: vec!["Reference".to_string(), "Minimum Level".to_string()]
            }
        );
        assert_eq!(strategy.describe(), "keyed by Reference, Minimum Level");
    }

    #[test]
    fn array_diff_strategy_keyed_without_key_fields_describes_bare() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "keyed" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Keyed { key_fields: vec![] });
        assert_eq!(strategy.describe(), "keyed");
    }

    #[test]
    fn array_diff_strategy_positional() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "positional" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Positional);
        assert_eq!(strategy.describe(), "positional");
    }

    #[test]
    fn array_diff_strategy_set() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "set" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Set);
        assert_eq!(strategy.describe(), "set");
    }

    #[test]
    fn array_diff_strategy_unkeyed() {
        let ad = array_diff_obj(serde_json::json!({
            "strategy": "unkeyed",
            "count_from": 2,
            "count_to": 1,
        }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Unkeyed);
        assert_eq!(strategy.describe(), "unkeyed");
    }

    #[test]
    fn array_diff_strategy_unrecognized_falls_back_to_raw_string() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "future_strategy" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(
            strategy,
            ArrayDiffStrategy::Other("future_strategy".to_string())
        );
        assert_eq!(strategy.describe(), "future_strategy");
    }

    #[test]
    fn summarize_array_diff_reports_buckets_and_counts() {
        let ad = array_diff_obj(serde_json::json!({
            "strategy": "keyed",
            "key_fields": ["Reference"],
            "count_from": 12,
            "count_to": 13,
            "added": [{"x": 1}, {"x": 2}, {"x": 3}],
            "removed": [{"x": 4}],
            "changed": [
                {"key": {"Reference": "0x1"}, "changes": {}},
                {"key": {}, "changes": {}},
            ],
        }));
        let summary = summarize_array_diff(&ad);
        assert_eq!(summary.counts, "+3 \u{2212}1 ~2");
        assert_eq!(summary.strategy_desc, "keyed by Reference");
        assert_eq!(summary.count_from, Some(12));
        assert_eq!(summary.count_to, Some(13));
    }

    #[test]
    fn summarize_array_diff_no_changes_when_all_buckets_empty() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "set" }));
        let summary = summarize_array_diff(&ad);
        assert_eq!(summary.counts, "no changes");
        assert_eq!(summary.strategy_desc, "set");
        assert_eq!(summary.count_from, None);
        assert_eq!(summary.count_to, None);
    }
}
