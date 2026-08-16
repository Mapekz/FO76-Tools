//! `refs` subcommand handler (both the reverse-reference walk and the
//! `--to` bidirectional path search), plus its table-rendering logic.

use esm::ipc::{Op, RecordSel};
use esm::{CarrierKind, Database, RefList};
use std::path::{Path, PathBuf};

use crate::Backend;
use crate::output::{apply_strings_override, print_json, print_record_table};
use crate::query::record_sel;

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_refs(
    backend: &mut Backend,
    file: &Path,
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
    entry_point: Option<String>,
    omod_property: Option<String>,
    limit: usize,
    depth: usize,
    record_type: Option<String>,
    paths: bool,
    sort: esm::ipc::RefSort,
    json: bool,
    pretty: bool,
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    daemon_mode: bool,
) -> anyhow::Result<()> {
    if depth == 0 {
        // Must warn *before* dispatching — an unbounded walk runs
        // synchronously and can take minutes with no other feedback; a note
        // attached to the finished RefList (as `capped`/`depth_capped` are)
        // would only print after the wait is already over.
        eprintln!(
            "warning: --depth 0 is unbounded and can take minutes on hub-heavy graphs \
             (--type/--limit don't reduce the cost); Ctrl-C to abort."
        );
    }
    // Carrier selectors bypass `record_sel`'s FormID/EditorID parsing
    // entirely — clap's `conflicts_with_all` guarantees mutual exclusion.
    let sel = match (entry_point, omod_property) {
        (Some(token), None) => RecordSel::EntryPoint(token),
        (None, Some(token)) => RecordSel::OmodProperty(token),
        (None, None) => record_sel(formid, edid, target)?,
        (Some(_), Some(_)) => {
            unreachable!("clap conflicts_with_all guarantees mutual exclusion")
        }
    };
    if localization_ba2.is_some() || strings_dir.is_some() {
        if daemon_mode {
            anyhow::bail!(
                "--localization-ba2/--strings-dir are not supported in daemon mode; \
                 use --local to open the ESM directly"
            );
        }
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        let mut db = Database::open(&esm_path)?;
        apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
        let op = Op::ReferencedBy {
            sel,
            limit,
            depth,
            type_filter: record_type,
            paths,
            sort,
        };
        let v = esm::ipc::dispatch_op(&mut db, &op)?;
        let ref_list: RefList = serde_json::from_value(v)?;
        print_refs(&ref_list, sort, json, pretty);
        return Ok(());
    }
    let v = backend.run(
        file,
        Op::ReferencedBy {
            sel,
            limit,
            depth,
            type_filter: record_type,
            paths,
            sort,
        },
    )?;
    let ref_list: RefList = serde_json::from_value(v)?;
    print_refs(&ref_list, sort, json, pretty);
    Ok(())
}

/// Which columns/lines [`print_refs`] should render for a given [`RefList`],
/// computed once from the data about to be printed — pulled out of
/// `print_refs` itself so this decision logic is unit-testable without
/// capturing stdout (see this module's test module: coverage for
/// carrier-only rows, depth>1 rows, and a plain flat list).
#[derive(Debug, PartialEq, Eq)]
struct RefColumns {
    /// Print the `{target}` legend line above the table — true whenever any
    /// row carries a tag (a virtual-seed carrier walk), so a type filter
    /// that suppresses every carrier row still shows it (BFS rows inherit
    /// `tags`).
    show_target_line: bool,
    /// Show the D(epth) column: true when depth is informative on at least
    /// one row — a carrier row (depth 0), or any hop beyond a direct
    /// reference (depth 1 alone would just repeat "1" on every row).
    show_depth: bool,
    show_via: bool,
    show_paths: bool,
    /// Distinct (kind, scope, id) tags matched across all rows. The tag
    /// column is shown iff this is > 1 — a single id is already named by
    /// the legend line, so a constant column would add noise.
    distinct_tag_count: usize,
    /// "PROP" when any matched tag is `CarrierKind::OmodProperty`, else
    /// "EP" — used both as the tag column's header (when shown) and, in the
    /// capped-output note, to pick the "properties"/"entry points" noun. In
    /// practice a `refs` invocation resolves through exactly one selector,
    /// so only one `CarrierKind` ever appears.
    tag_header: &'static str,
}

impl RefColumns {
    fn show_tag_column(&self) -> bool {
        self.distinct_tag_count > 1
    }
}

fn ref_columns(ref_list: &RefList) -> RefColumns {
    let has_carriers = ref_list.rows.iter().any(|r| r.depth == 0);
    let show_depth = has_carriers || ref_list.rows.iter().any(|r| r.depth > 1);
    let show_target_line = ref_list.rows.iter().any(|r| !r.tags.is_empty());
    let distinct_tag_count: std::collections::BTreeSet<_> = ref_list
        .rows
        .iter()
        .flat_map(|r| r.tags.iter().map(|t| (t.kind, t.scope.as_deref(), t.id)))
        .collect();
    let tag_kinds: std::collections::BTreeSet<CarrierKind> = ref_list
        .rows
        .iter()
        .flat_map(|r| r.tags.iter().map(|t| t.kind))
        .collect();
    let tag_header = if tag_kinds.contains(&CarrierKind::OmodProperty) {
        "PROP"
    } else {
        "EP"
    };
    let show_via = ref_list.rows.iter().any(|r| !r.path.is_empty());
    let show_paths = ref_list.rows.iter().any(|r| r.field_paths.is_some());
    RefColumns {
        show_target_line,
        show_depth,
        show_via,
        show_paths,
        distinct_tag_count: distinct_tag_count.len(),
        tag_header,
    }
}

fn print_refs(ref_list: &RefList, sort: esm::ipc::RefSort, json: bool, pretty: bool) {
    let columns = ref_columns(ref_list);
    if json {
        print_json(&serde_json::to_value(&ref_list.rows).unwrap(), pretty);
    } else {
        if ref_list.rows.is_empty() {
            eprintln!("note: no records reference {}", ref_list.target);
        } else {
            if columns.show_target_line {
                eprintln!("{}", ref_list.target);
            }
            // VIA is shown only when at least one row has a multi-hop path,
            // and PATHS only when --paths was requested (field_paths is
            // Some(...) on every row in that case, even if the inner Vec is empty).
            let table_rows: Vec<Vec<String>> = ref_list
                .rows
                .iter()
                .map(|row| {
                    let mut cells = vec![
                        row.form_id.clone(),
                        row.record_type.as_deref().unwrap_or("").to_string(),
                        row.editor_id.as_deref().unwrap_or("").to_string(),
                        row.name.as_deref().unwrap_or("").to_string(),
                    ];
                    if columns.show_depth {
                        // depth 0 marks a carrier — `RefRow::depth`'s doc
                        // says 1 = direct reference, so 0 is free as a "this
                        // is the walk's starting point, not something it
                        // found" sentinel.
                        cells.push(row.depth.to_string());
                    }
                    if columns.show_tag_column() {
                        let cells_tags: Vec<String> = row
                            .tags
                            .iter()
                            .map(|t| match t.kind {
                                CarrierKind::EntryPoint => t.id.to_string(),
                                CarrierKind::OmodProperty => match &t.scope {
                                    Some(s) => format!("{s}:{}", t.id),
                                    None => t.id.to_string(),
                                },
                            })
                            .collect();
                        cells.push(cells_tags.join(","));
                    }
                    if columns.show_via {
                        let via = if !row.path.is_empty() {
                            let chain: Vec<_> =
                                row.path.iter().map(|n| n.form_id.as_str()).collect();
                            chain.join(" → ")
                        } else {
                            String::new()
                        };
                        cells.push(via);
                    }
                    if columns.show_paths {
                        let paths = row
                            .field_paths
                            .as_deref()
                            .map(|p| p.join("; "))
                            .unwrap_or_default();
                        cells.push(paths);
                    }
                    cells
                })
                .collect();
            let mut headers = vec!["FORMID", "TYPE", "EDID", "NAME"];
            if columns.show_depth {
                headers.push("D");
            }
            if columns.show_tag_column() {
                headers.push(columns.tag_header);
            }
            if columns.show_via {
                headers.push("VIA");
            }
            if columns.show_paths {
                headers.push("PATHS");
            }
            print_record_table(&headers, &table_rows);
        }
    }
    // depth=1 is this tool's documented "direct referencers only" mode, not
    // a truncated state — nearly every interconnected record has *some*
    // referencer-of-a-referencer, so warning about it on the default depth
    // would fire on almost every plain lookup and drown out the cases where
    // this note is actually actionable (an intentionally elevated --depth
    // that still didn't reach the full graph).
    if ref_list.depth_capped && ref_list.requested_depth > 1 {
        // Independent of --limit: the BFS itself stopped with an unexpanded
        // frontier, so this result is a genuine subset of the full
        // reverse-reference graph — not just a display truncation.
        let Some(d) = ref_list.effective_depth else {
            unreachable!("an unbounded walk (effective_depth=None) never leaves a frontier");
        };
        let escape = if d < esm::ipc::DEFAULT_MAX_DEPTH {
            format!(
                "raise --depth (up to {}) or pass --depth 0 for an unbounded walk",
                esm::ipc::DEFAULT_MAX_DEPTH
            )
        } else {
            "pass --depth 0 for an unbounded walk".to_string()
        };
        eprintln!(
            "note: walk stopped at max depth {d} with {} frontier node(s) unexpanded; \
             results are incomplete beyond depth {d} — {escape}",
            ref_list.frontier_remaining,
        );
    }
    if ref_list.capped {
        let mut note = format!(
            "note: output capped at {} of {} results, ordered by {}",
            ref_list.rows.len(),
            ref_list.total,
            match sort {
                esm::ipc::RefSort::Formid => "formid",
                esm::ipc::RefSort::Depth => "depth",
            }
        );
        if let (Some(carrier_total), Some(tag_total)) = (ref_list.carrier_total, ref_list.tag_total)
        {
            let carriers_shown = ref_list.rows.iter().filter(|r| r.depth == 0).count();
            let tags_shown = columns.distinct_tag_count;
            let tag_noun = if columns.tag_header == "PROP" {
                "properties"
            } else {
                "entry points"
            };
            note.push_str(&format!(
                " ({carriers_shown} of {carrier_total} carriers, \
                 {tags_shown} of {tag_total} {tag_noun} shown)"
            ));
        }
        if !ref_list.per_depth_totals.is_empty() {
            let totals: Vec<String> = ref_list
                .per_depth_totals
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(d, n)| format!("{d}:{n}"))
                .collect();
            note.push_str(&format!(
                "\n      per-depth totals: {}\n      rows shown cover depth 1-{} only",
                totals.join(" "),
                ref_list.shown_max_depth
            ));
        }
        note.push_str("; use --limit 0 to show all, or --sort depth for a breadth-first prefix");
        eprintln!("{note}");
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_ref_path(
    backend: &mut Backend,
    file: &Path,
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
    to: String,
    max_hops: usize,
    paths: bool,
    json: bool,
    pretty: bool,
) -> anyhow::Result<()> {
    let from = record_sel(formid, edid, target)?;
    let to = RecordSel::from_input(&to)?;
    let v = backend.run(
        file,
        Op::RefPath {
            from,
            to,
            max_hops,
            paths,
        },
    )?;
    let result: esm::refs::RefPathResult = serde_json::from_value(v)?;
    print_ref_path(&result, json, pretty);
    Ok(())
}

fn print_ref_path(result: &esm::refs::RefPathResult, json: bool, pretty: bool) {
    if json {
        print_json(&serde_json::to_value(result).unwrap(), pretty);
        return;
    }
    let Some(chain) = &result.chain else {
        if result.budget_exhausted {
            eprintln!(
                "no path found between {} and {}, but the search budget was exhausted first \
                 — this is inconclusive, not a confirmed absence of any connection; try a \
                 smaller --max-hops or a --type-narrowed manual refs walk from one side",
                result.from, result.to
            );
        } else {
            eprintln!(
                "no path found between {} and {} within the hop budget (raise --max-hops, \
                 default {}). This search is directional — it looks for a chain of \
                 referencers connecting FROM's transitive referencer chain to TO (the same \
                 direction `esm refs FROM` walks); if you're not sure which side is the \
                 \"referenced\" one, also try swapping them: refs {} --to {}",
                result.from,
                result.to,
                esm::refs::DEFAULT_MAX_PATH_HOPS,
                result.to,
                result.from
            );
        }
        return;
    };
    let hops = result.hops.unwrap_or(chain.len().saturating_sub(1));
    println!("path found, {hops} hop(s) (reverse-reference direction):");
    println!();
    for (i, hop) in chain.iter().enumerate() {
        let label = format!(
            "{}  {}  {}",
            hop.form_id,
            hop.record_type.as_deref().unwrap_or(""),
            hop.editor_id.as_deref().unwrap_or(""),
        );
        let name = hop.name.as_deref().unwrap_or("");
        if i == 0 {
            println!("  {label}  {name}");
        } else {
            println!("   <-{i} {label}  {name}");
        }
        if let Some(fp) = &hop.field_paths
            && !fp.is_empty()
        {
            println!("        via: {}", fp.join("; "));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use esm::RefRow;

    // ── ref_columns: print_refs's column-visibility decisions ───────────

    fn ref_row(depth: usize) -> RefRow {
        RefRow {
            depth,
            ..Default::default()
        }
    }

    #[test]
    fn ref_columns_carrier_only_rows_show_depth_but_no_other_columns() {
        let list = RefList {
            rows: vec![ref_row(0), ref_row(0)],
            ..Default::default()
        };
        let columns = ref_columns(&list);
        assert!(columns.show_depth, "a carrier (depth 0) row must show D");
        assert!(!columns.show_target_line, "no tags on any row");
        assert!(!columns.show_via, "no row has a path");
        assert!(!columns.show_paths, "no row has field_paths");
        assert!(!columns.show_tag_column());
    }

    #[test]
    fn ref_columns_depth_beyond_one_shows_depth_and_via() {
        let deep_row = RefRow {
            depth: 2,
            path: vec![esm::ipc::RefPathNode {
                form_id: "0x00001234".to_string(),
                record_type: Some("WEAP".to_string()),
                editor_id: None,
            }],
            ..Default::default()
        };
        let list = RefList {
            rows: vec![ref_row(1), deep_row],
            ..Default::default()
        };
        let columns = ref_columns(&list);
        assert!(columns.show_depth, "a depth > 1 row must show D");
        assert!(
            columns.show_via,
            "a row with a non-empty path must show VIA"
        );
        assert!(!columns.show_target_line, "no tags on any row");
        assert!(!columns.show_paths, "no row has field_paths");
    }

    #[test]
    fn ref_columns_plain_flat_list_shows_no_optional_columns() {
        // The common case: every row a direct (depth 1) reference, no tags,
        // no multi-hop path, `--paths` not requested.
        let list = RefList {
            rows: vec![ref_row(1), ref_row(1), ref_row(1)],
            ..Default::default()
        };
        let columns = ref_columns(&list);
        assert!(
            !columns.show_depth,
            "depth 1 alone would just repeat \"1\" on every row"
        );
        assert!(!columns.show_target_line);
        assert!(!columns.show_via);
        assert!(!columns.show_paths);
        assert!(!columns.show_tag_column());
    }

    #[test]
    fn ref_columns_multiple_omod_property_tags_show_prop_column() {
        let tag = |id: u16| esm::CarrierTag {
            kind: CarrierKind::OmodProperty,
            id,
            name: None,
            scope: Some("weap".to_string()),
        };
        let list = RefList {
            rows: vec![
                RefRow {
                    depth: 0,
                    tags: vec![tag(1)],
                    ..Default::default()
                },
                RefRow {
                    depth: 1,
                    tags: vec![tag(2)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let columns = ref_columns(&list);
        assert!(
            columns.show_target_line,
            "a tagged row must show the legend line"
        );
        assert_eq!(columns.tag_header, "PROP");
        assert_eq!(columns.distinct_tag_count, 2);
        assert!(columns.show_tag_column());
    }

    #[test]
    fn ref_columns_single_tag_id_does_not_earn_a_column() {
        // A single distinct tag id is already named by the legend line, so
        // a constant-valued column would add noise.
        let tag = esm::CarrierTag {
            kind: CarrierKind::EntryPoint,
            id: 7,
            name: None,
            scope: None,
        };
        let list = RefList {
            rows: vec![RefRow {
                depth: 0,
                tags: vec![tag],
                ..Default::default()
            }],
            ..Default::default()
        };
        let columns = ref_columns(&list);
        assert!(columns.show_target_line);
        assert_eq!(columns.distinct_tag_count, 1);
        assert!(!columns.show_tag_column());
        assert_eq!(columns.tag_header, "EP");
    }
}
