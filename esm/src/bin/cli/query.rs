//! `get` / `list` / `search` subcommand handlers.

use esm::ipc::{Op, RecordSel};
use esm::{Database, RecordRow, ResolveDepth, SearchField};
use std::path::{Path, PathBuf};

use crate::output::{
    apply_strings_override, bail_if_daemon_mode_overrides, print_json, print_record_rows,
    print_search_results,
};
use crate::{Backend, SearchInArg};

fn parse_resolve(s: &str) -> anyhow::Result<ResolveDepth> {
    esm::query::resolve_depth(Some(s), ResolveDepth::None)
}

pub(crate) fn record_sel(
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
) -> anyhow::Result<RecordSel> {
    RecordSel::from_parts(formid.as_deref(), edid.as_deref(), target.as_deref())
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
    daemon_mode: bool,
) -> anyhow::Result<()> {
    let has_overrides =
        localization_ba2.is_some() || strings_dir.is_some() || startup_ba2.is_some();

    // ── Bulk path (2+ positional targets) ─────────────────────────────────
    // clap's `conflicts_with_all` on `targets` guarantees --formid/--edid are
    // never set here. Single-target and zero-target calls fall through
    // untouched below, so that output stays byte-for-byte identical to the
    // pre-bulk CLI.
    if targets.len() > 1 {
        if raw {
            anyhow::bail!("--raw does not support multiple selectors; run one target at a time");
        }
        if has_overrides {
            anyhow::bail!(
                "--localization-ba2/--strings-dir/--startup-ba2 are not supported with \
                 multiple selectors; run one target at a time, or place the strings/curves \
                 next to the ESM so the warm daemon auto-loads them (see esm/CLAUDE.md)"
            );
        }
        let sels: Vec<RecordSel> = targets
            .iter()
            .map(|t| RecordSel::from_input(t))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let depth = parse_resolve(&resolve)?;
        let v = backend.run(file, Op::RecordBulk { sels, depth })?;
        print_json(&v, pretty || !json);
        return Ok(());
    }
    let target = targets.into_iter().next();

    bail_if_daemon_mode_overrides(
        has_overrides,
        daemon_mode,
        "--localization-ba2/--strings-dir/--startup-ba2",
    )?;
    if has_overrides {
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        let mut db = Database::open(&esm_path)?;
        apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
        if let Some(ba2_path) = startup_ba2 {
            db.load_curves(&ba2_path)?;
        }
        let sel = record_sel(formid, edid, target)?;
        let depth = parse_resolve(&resolve)?;
        let op = if raw {
            Op::RecordRaw { sel }
        } else {
            Op::Record { sel, depth }
        };
        let v = esm::ipc::dispatch_op(&mut db, &op)?;
        print_json(&v, pretty || !json);
        return Ok(());
    }

    let sel = record_sel(formid, edid, target)?;
    let depth = parse_resolve(&resolve)?;
    if raw {
        let v = backend.run(file, Op::RecordRaw { sel })?;
        print_json(&v, pretty || !json);
        return Ok(());
    }
    let v = backend.run(file, Op::Record { sel, depth })?;
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
    daemon_mode: bool,
) -> anyhow::Result<()> {
    let has_overrides = localization_ba2.is_some() || strings_dir.is_some();
    if has_overrides {
        bail_if_daemon_mode_overrides(
            has_overrides,
            daemon_mode,
            "--localization-ba2/--strings-dir",
        )?;
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        let mut db = Database::open(&esm_path)?;
        apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
        let rows = db.list_type_records(sig, 0, limit)?;
        print_record_rows(&rows, limit, json, pretty);
        return Ok(());
    }
    let v = backend.run(
        file,
        Op::ListTypeRecords {
            sig: sig.to_string(),
            offset: 0,
            limit,
        },
    )?;
    let rows: Vec<RecordRow> = serde_json::from_value(v)?;
    print_record_rows(&rows, limit, json, pretty);
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
    daemon_mode: bool,
) -> anyhow::Result<()> {
    let field = match search_in {
        SearchInArg::Edid => SearchField::Edid,
        SearchInArg::Name => SearchField::Name,
        SearchInArg::Both => SearchField::Both,
    };

    let has_overrides = localization_ba2.is_some() || strings_dir.is_some();
    if has_overrides {
        bail_if_daemon_mode_overrides(
            has_overrides,
            daemon_mode,
            "--localization-ba2/--strings-dir",
        )?;
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        let mut db = Database::open(&esm_path)?;
        apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
        let results = db.search(pattern, &types, field, limit)?;
        print_search_results(&results, limit, json, pretty);
        return Ok(());
    }

    let v = backend.run(
        file,
        Op::Search {
            pattern: pattern.to_string(),
            types,
            field,
            limit,
        },
    )?;
    let results: Vec<RecordRow> = serde_json::from_value(v)?;
    print_search_results(&results, limit, json, pretty);
    Ok(())
}
