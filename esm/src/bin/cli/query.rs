//! `get` / `list` / `search` subcommand handlers.

use esm::ops::{Op, RecordSel};
use esm::{Database, FormIdBase, RecordRow, ResolveDepth, SearchField};
use std::path::{Path, PathBuf};

use crate::output::{
    apply_strings_override, print_json, print_record_rows, print_search_results, render_form_id,
};
use crate::{Backend, SearchInArg};

fn parse_resolve(s: &str) -> anyhow::Result<ResolveDepth> {
    esm::query::resolve_depth(Some(s), ResolveDepth::None)
}

/// Build a selector from explicit `--formid`/`--edid`/positional-target CLI
/// inputs, base-aware — see [`RecordSel::from_parts_with`]. The one parser
/// shared by every subcommand handler with separate `--formid`/`--edid`
/// flags (`get`, `refs`, `refs --to`'s `from` side).
pub(crate) fn record_sel_with(
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
    base: FormIdBase,
) -> anyhow::Result<RecordSel> {
    RecordSel::from_parts_with(formid.as_deref(), edid.as_deref(), target.as_deref(), base)
}

/// Rewrite `get`'s decoded-record identity FormID (`header.form_id`) into
/// `base`, in place. A no-op on an error entry (no `header`), and only ever
/// touches `header.form_id` — FormIDs inside decoded `fields` are reference
/// fields, not this record's own identity, and stay hex regardless of
/// `--decimal` (see `docs/adr/0010-formid-input-base.md`).
fn convert_header_form_id(entry: &mut serde_json::Value, base: FormIdBase) {
    if let Some(form_id) = entry.get_mut("header").and_then(|h| h.get_mut("form_id"))
        && let serde_json::Value::String(s) = form_id
    {
        *s = render_form_id(s, base);
    }
}

/// [`convert_header_form_id`] applied to either a single-record result
/// (identical to `convert_header_form_id`) or an `Op::RecordBulk` array
/// result (converts each entry).
fn convert_get_result(v: &mut serde_json::Value, base: FormIdBase) {
    match v {
        serde_json::Value::Array(entries) => {
            for entry in entries.iter_mut() {
                convert_header_form_id(entry, base);
            }
        }
        other => convert_header_form_id(other, base),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_get(
    backend: &mut Backend,
    file: &Path,
    formid: Option<String>,
    edid: Option<String>,
    targets: Vec<String>,
    json: bool,
    pretty: bool,
    raw: bool,
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    startup_ba2: Option<PathBuf>,
    resolve: String,
    base: FormIdBase,
) -> anyhow::Result<()> {
    let depth = parse_resolve(&resolve)?;
    let op = if targets.len() > 1 {
        // clap's `conflicts_with_all` on `targets` guarantees --formid/--edid
        // are never set here.
        if raw {
            anyhow::bail!("--raw does not support multiple selectors; run one target at a time");
        }
        let sels = targets
            .iter()
            .map(|t| RecordSel::from_input_with(t, base))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Op::RecordBulk(esm::ops::RecordBulkArgs { sels, depth })
    } else {
        let sel = record_sel_with(formid, edid, targets.into_iter().next(), base)?;
        if raw {
            Op::RecordRaw(esm::ops::RecordRawArgs { sel })
        } else {
            Op::Record(esm::ops::RecordArgs { sel, depth })
        }
    };

    let mut v = if localization_ba2.is_some() || strings_dir.is_some() || startup_ba2.is_some() {
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        crate::progress_ui::watched(&[&esm_path], || {
            let mut db = Database::open(&esm_path)?;
            apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
            if let Some(ba2_path) = startup_ba2 {
                db.load_curves(&ba2_path)?;
            }
            esm::ops::run(&db, &op)
        })?
    } else {
        backend.run(file, op)?
    };
    convert_get_result(&mut v, base);
    print_json(&v, pretty || !json);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_list(
    backend: &mut Backend,
    file: &Path,
    sig: &str,
    limit: usize,
    json: bool,
    pretty: bool,
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    base: FormIdBase,
) -> anyhow::Result<()> {
    let has_overrides = localization_ba2.is_some() || strings_dir.is_some();
    if has_overrides {
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        let rows = crate::progress_ui::watched(&[&esm_path], || {
            let mut db = Database::open(&esm_path)?;
            apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
            db.list_type_records(sig, 0, limit)
        })?;
        print_record_rows(&rows, limit, json, pretty, base);
        return Ok(());
    }
    let v = backend.run(
        file,
        Op::ListTypeRecords(esm::ops::ListTypeRecordsArgs {
            sig: sig.to_string(),
            offset: 0,
            limit,
        }),
    )?;
    let rows: Vec<RecordRow> = serde_json::from_value(v)?;
    print_record_rows(&rows, limit, json, pretty, base);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_search(
    backend: &mut Backend,
    file: &Path,
    pattern: &str,
    types: Vec<String>,
    search_in: SearchInArg,
    limit: usize,
    json: bool,
    pretty: bool,
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    base: FormIdBase,
) -> anyhow::Result<()> {
    let field = match search_in {
        SearchInArg::Edid => SearchField::Edid,
        SearchInArg::Name => SearchField::Name,
        SearchInArg::Both => SearchField::Both,
    };

    let has_overrides = localization_ba2.is_some() || strings_dir.is_some();
    if has_overrides {
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        let results = crate::progress_ui::watched(&[&esm_path], || {
            let mut db = Database::open(&esm_path)?;
            apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
            db.search(pattern, &types, field, limit)
        })?;
        print_search_results(&results, limit, json, pretty, base);
        return Ok(());
    }

    let v = backend.run(
        file,
        Op::Search(esm::ops::SearchArgs {
            pattern: pattern.to_string(),
            types,
            field,
            limit,
        }),
    )?;
    let results: Vec<RecordRow> = serde_json::from_value(v)?;
    print_search_results(&results, limit, json, pretty, base);
    Ok(())
}
