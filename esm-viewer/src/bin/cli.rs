use clap::{Parser, Subcommand, ValueEnum};
use fo76_esm_parser::{parse_form_id_input, Database, SearchField};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "fo76", about = "Fallout 76 ESM parser")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print TES4 header info
    Info { file: PathBuf },
    /// Fetch a record by FormID or EditorID
    Get {
        file: PathBuf,
        #[arg(long, conflicts_with = "edid")]
        formid: Option<String>,
        #[arg(long, conflicts_with = "formid")]
        edid: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        #[arg(long)]
        raw: bool,
        /// Path to a localization BA2 (overrides auto-detected sibling BA2)
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        /// Directory containing loose .strings/.dlstrings/.ilstrings files
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        /// Language code to use when loading string tables (default: "en")
        #[arg(long, default_value = "en")]
        lang: String,
        /// Path to a Startup BA2 archive for inlining curve table data
        #[arg(long)]
        startup_ba2: Option<PathBuf>,
        /// Resolve FormID references: none (default), stub, full
        #[arg(long, default_value = "none")]
        resolve: String,
    },
    /// List records of a given type
    List {
        file: PathBuf,
        #[arg(long)]
        r#type: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Path to a localization BA2 (overrides auto-detected sibling BA2)
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        /// Directory containing loose .strings/.dlstrings/.ilstrings files
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        /// Language code to use when loading string tables (default: "en")
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// Diff two ESM versions by FormID alignment
    Diff {
        file_a: PathBuf,
        file_b: PathBuf,
        /// Filter output to a specific record type (e.g. GLOB)
        #[arg(long = "type")]
        record_type: Option<String>,
        /// Output full diff as JSON
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
    },
    /// Browse the hierarchical GRUP structure of an ESM file
    Tree {
        file: PathBuf,
        /// Record type to drill into (e.g. WEAP). Omit to list top-level groups.
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        pretty: bool,
    },
    /// Audit schema coverage: count raw_fallback / unmapped / unresolved markers per record type.
    ///
    /// Decodes every record in the ESM (or a sampled subset) and tallies the internal
    /// coverage markers emitted when fields have no schema or use raw_fallback deciders.
    /// Output is sorted by total markers descending (worst offenders first).
    ///
    /// Exit code is non-zero when --gate is specified and any raw_fallback markers are found.
    Coverage {
        file: PathBuf,
        /// Audit only this record type (4-char signature, e.g. PACK).
        #[arg(long = "type")]
        record_type: Option<String>,
        /// Max records to decode per type. 0 = all records (default).
        #[arg(long, default_value_t = 0)]
        sample: usize,
        /// Emit results as JSON.
        #[arg(long)]
        json: bool,
        /// Exit non-zero if any raw_fallback markers remain.
        #[arg(long)]
        gate: bool,
    },
    /// Find records that reference a given record (reverse FormID lookup).
    ///
    /// Builds (and caches) a reverse-reference index on first use, then lists every
    /// record whose decoded fields point at the target FormID. Results are sorted by
    /// FormID ascending and capped at --limit (default 100; 0 = unlimited).
    ///
    /// Note: the xref index only covers references between records that both exist
    /// in this file, so references to FormIDs from master files will not appear.
    /// The first run decodes every record (may take tens of seconds); subsequent
    /// runs use the cached index and are instant.
    Refs {
        file: PathBuf,
        #[arg(long, conflicts_with = "edid")]
        formid: Option<String>,
        #[arg(long, conflicts_with = "formid")]
        edid: Option<String>,
        /// Maximum number of results (default 100; 0 = unlimited).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        /// Path to a localization BA2 (overrides auto-detected sibling BA2).
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        /// Directory containing loose .strings/.dlstrings/.ilstrings files.
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        /// Language code to use when loading string tables (default: "en").
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// Search records by EditorID and/or display name using a wildcard pattern.
    ///
    /// Plain text is treated as a case-insensitive substring match.
    /// Use `*` as a multi-character wildcard: `HTO_*` (prefix), `*Rifle` (suffix),
    /// `Plasma*Rifle` (both anchors), `*` (match all).
    ///
    /// Results are sorted by FormID and capped at --limit (default 100; 0 = unlimited).
    Search {
        file: PathBuf,
        /// Pattern to match (supports `*` wildcard and plain substring search).
        pattern: String,
        /// Restrict search to one or more record types (comma-separated or repeated).
        /// Example: `--type WEAP,OMOD` or `--type WEAP --type OMOD`.
        #[arg(long = "type", value_delimiter = ',')]
        types: Vec<String>,
        /// Which field(s) to match against.
        #[arg(long = "in", value_enum, default_value = "both")]
        search_in: SearchInArg,
        /// Maximum number of results (default 100; 0 = unlimited).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Output results as JSON.
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        /// Path to a localization BA2 (overrides auto-detected sibling BA2).
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        /// Directory containing loose .strings/.dlstrings/.ilstrings files.
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        /// Language code to use when loading string tables (default: "en").
        #[arg(long, default_value = "en")]
        lang: String,
    },
}

/// Controls which record fields are compared during a `search`.
#[derive(Clone, Copy, ValueEnum)]
enum SearchInArg {
    /// Match only the EditorID.
    Edid,
    /// Match only the display name (FULL) and description (DESC).
    Name,
    /// Match EditorID or display name / description (default).
    Both,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Info { file } => cmd_info(&file),
        Commands::Get {
            file,
            formid,
            edid,
            json,
            pretty,
            raw,
            strings,
            strings_dir,
            lang,
            startup_ba2,
            resolve,
        } => cmd_get(
            &file,
            formid,
            edid,
            json,
            pretty,
            raw,
            strings,
            strings_dir,
            &lang,
            startup_ba2,
            resolve,
        ),
        Commands::List {
            file,
            r#type,
            limit,
            strings,
            strings_dir,
            lang,
        } => cmd_list(&file, &r#type, limit, strings, strings_dir, &lang),
        Commands::Diff {
            file_a,
            file_b,
            record_type,
            json,
            pretty,
        } => cmd_diff(&file_a, &file_b, record_type.as_deref(), json, pretty),
        Commands::Tree {
            file,
            record_type,
            offset,
            limit,
            pretty,
        } => cmd_tree(&file, record_type.as_deref(), offset, limit, pretty),
        Commands::Coverage {
            file,
            record_type,
            sample,
            json,
            gate,
        } => cmd_coverage(&file, record_type.as_deref(), sample, json, gate),
        Commands::Refs {
            file,
            formid,
            edid,
            limit,
            json,
            pretty,
            strings,
            strings_dir,
            lang,
        } => cmd_refs(
            &file,
            formid,
            edid,
            limit,
            json,
            pretty,
            strings,
            strings_dir,
            &lang,
        ),
        Commands::Search {
            file,
            pattern,
            types,
            search_in,
            limit,
            json,
            pretty,
            strings,
            strings_dir,
            lang,
        } => cmd_search(
            &file,
            &pattern,
            types,
            search_in,
            limit,
            json,
            pretty,
            strings,
            strings_dir,
            &lang,
        ),
    }
}

fn cmd_info(file: &PathBuf) -> anyhow::Result<()> {
    let db = Database::open(file)?;
    let info = db.file_info()?;
    println!("File: {}", file.display());
    println!("Version: {}", info.version);
    println!("Record count: {}", info.record_count);
    println!("Next Object ID: 0x{:08X}", info.next_object_id);
    println!("Flags: 0x{:08X}", info.flags);
    println!("ESM: {}", info.is_esm);
    println!("Localized: {}", info.is_localized);
    if let Some(a) = &info.author {
        println!("Author: {}", a);
    }
    if let Some(d) = &info.description {
        println!("Description: {}", d);
    }
    if !info.masters.is_empty() {
        println!("Masters:");
        for m in &info.masters {
            println!("  - {}", m);
        }
    }
    Ok(())
}

/// Derive the string table prefix from an ESM file path.
///
/// E.g. `SeventySix_20260612.esm` → `"SeventySix_20260612"`.
fn esm_string_prefix(esm_path: &std::path::Path) -> String {
    esm_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "SeventySix".to_string())
}

fn apply_strings_override(
    db: &mut Database,
    esm_path: &std::path::Path,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) {
    if let Some(ba2_path) = strings {
        match fo76_esm_parser::strings::Localization::from_ba2(&ba2_path, lang, "seventysix") {
            Ok(loc) => db.set_localization(loc),
            Err(e) => eprintln!(
                "Warning: failed to load localization from {}: {}",
                ba2_path.display(),
                e
            ),
        }
    } else if let Some(dir) = strings_dir {
        let prefix = esm_string_prefix(esm_path);
        match fo76_esm_parser::strings::Localization::from_loose_files(&dir, lang, &prefix) {
            Ok(loc) => db.set_localization(loc),
            Err(e) => eprintln!(
                "Warning: failed to load string tables from {}: {}",
                dir.display(),
                e
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_get(
    file: &PathBuf,
    formid: Option<String>,
    edid: Option<String>,
    json: bool,
    pretty: bool,
    raw: bool,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    startup_ba2: Option<PathBuf>,
    resolve: String,
) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
    apply_strings_override(&mut db, file, strings, strings_dir, lang);
    if let Some(ba2_path) = startup_ba2 {
        db.load_curves(&ba2_path)?;
    }

    if raw {
        let form_id = resolve_form_id(&mut db, formid, edid)?;
        let rec = db.record_raw(form_id)?;
        let out = serde_json::json!({
            "header": rec.header,
            "subrecords": rec.subrecords.iter().map(|sr| serde_json::json!({
                "signature": sr.signature.to_string(),
                "size": sr.data.len(),
                "hex": sr.data.iter().map(|b| format!("{:02x}", b)).collect::<String>()
            })).collect::<Vec<_>>()
        });
        print_json(&out, pretty || !json);
        return Ok(());
    }

    let depth = match resolve.as_str() {
        "stub" => fo76_esm_parser::ResolveDepth::Stub,
        "full" => fo76_esm_parser::ResolveDepth::Full,
        _ => fo76_esm_parser::ResolveDepth::None,
    };

    let result = if depth != fo76_esm_parser::ResolveDepth::None {
        if let Some(fid) = formid {
            db.record_by_formid_resolved(parse_form_id_input(&fid)?, depth)?
        } else if let Some(e) = edid {
            db.record_by_edid_resolved(&e, depth)?
        } else {
            anyhow::bail!("specify --formid or --edid");
        }
    } else if let Some(fid) = formid {
        db.record_by_formid(parse_form_id_input(&fid)?)?
    } else if let Some(e) = edid {
        db.record_by_edid(&e)?
    } else {
        anyhow::bail!("specify --formid or --edid");
    };

    let out = serde_json::json!({
        "header": result.header,
        "editor_id": result.editor_id,
        "fields": result.fields
    });
    print_json(&out, pretty || !json);
    Ok(())
}

fn cmd_list(
    file: &PathBuf,
    sig: &str,
    limit: usize,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
    apply_strings_override(&mut db, file, strings, strings_dir, lang);
    db.index.ensure_edid_index(&db.esm)?;
    let entries = db.list_by_type(sig, limit)?;
    for e in entries {
        print!(
            "{}  {}",
            e.form_id,
            e.editor_id.as_deref().unwrap_or("<no edid>")
        );
        if let Some(full) = e.full_lstring_id {
            print!("  FULL={}", full);
        }
        println!();
    }
    Ok(())
}

/// One referencer row, enriched with the record type for display.
#[derive(serde::Serialize)]
struct RefRow {
    form_id: String,
    record_type: Option<String>,
    editor_id: Option<String>,
    name: Option<String>,
    offset: u64,
}

#[allow(clippy::too_many_arguments)]
fn cmd_refs(
    file: &PathBuf,
    formid: Option<String>,
    edid: Option<String>,
    limit: usize,
    json: bool,
    pretty: bool,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
    apply_strings_override(&mut db, file, strings, strings_dir, lang);

    let target = resolve_form_id(&mut db, formid, edid)?;
    // referenced_by returns an owned Vec — the mutable borrow on db ends here.
    let mut rows = db.referenced_by(target)?;

    // Deterministic output: sort by FormID ascending.
    rows.sort_by_key(|r| {
        parse_form_id_input(&r.form_id)
            .map(|f| f.0)
            .unwrap_or(u32::MAX)
    });

    // Enrich each row with the referencer's record type via a cheap index lookup.
    let enriched: Vec<RefRow> = rows
        .into_iter()
        .map(|r| {
            let record_type = parse_form_id_input(&r.form_id)
                .ok()
                .and_then(|fid| db.index.get_by_formid(fid))
                .map(|m| m.signature.clone());
            RefRow {
                form_id: r.form_id,
                record_type,
                editor_id: r.editor_id,
                name: r.name,
                offset: r.offset,
            }
        })
        .collect();

    let total = enriched.len();
    let limited: Vec<RefRow> = if limit > 0 {
        enriched.into_iter().take(limit).collect()
    } else {
        enriched
    };
    let capped = limit > 0 && total > limit;

    if json {
        print_json(&serde_json::to_value(&limited)?, pretty);
    } else {
        for row in &limited {
            print!(
                "{}  {}  {}",
                row.form_id,
                row.record_type.as_deref().unwrap_or("????"),
                row.editor_id.as_deref().unwrap_or("<no edid>")
            );
            if let Some(ref name) = row.name {
                print!("  {}", name);
            }
            println!();
        }
        if limited.is_empty() {
            eprintln!("note: no records reference {}", target);
        }
    }

    if capped {
        eprintln!(
            "note: output capped at {} of {} results; use --limit 0 to show all",
            limit, total
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_search(
    file: &PathBuf,
    pattern: &str,
    types: Vec<String>,
    search_in: SearchInArg,
    limit: usize,
    json: bool,
    pretty: bool,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) -> anyhow::Result<()> {
    if pattern.is_empty() {
        anyhow::bail!("search pattern must not be empty (use \"*\" to match all records)");
    }

    // Validate and uppercase type filters.
    let types: Vec<String> = types
        .into_iter()
        .map(|t| {
            let up = t.to_uppercase();
            if up.len() != 4 {
                anyhow::bail!("record type '{}' must be a 4-character signature", t);
            }
            Ok(up)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let field = match search_in {
        SearchInArg::Edid => SearchField::Edid,
        SearchInArg::Name => SearchField::Name,
        SearchInArg::Both => SearchField::Both,
    };

    let mut db = Database::open(file)?;
    apply_strings_override(&mut db, file, strings, strings_dir, lang);

    let results = db.search(pattern, &types, field, limit)?;

    let capped = limit > 0 && results.len() == limit;

    if json {
        print_json(&serde_json::to_value(&results)?, pretty);
    } else {
        for row in &results {
            print!(
                "{}  {}",
                row.form_id,
                row.editor_id.as_deref().unwrap_or("<no edid>")
            );
            if let Some(ref name) = row.name {
                print!("  {}", name);
            }
            println!();
        }
    }

    if capped {
        eprintln!(
            "note: output capped at {} results; use --limit 0 to show all",
            limit
        );
    }

    Ok(())
}

fn resolve_form_id(
    db: &mut Database,
    formid: Option<String>,
    edid: Option<String>,
) -> anyhow::Result<fo76_esm_parser::FormId> {
    if let Some(fid) = formid {
        parse_form_id_input(&fid)
    } else if let Some(e) = edid {
        db.index.ensure_edid_index(&db.esm)?;
        db.index
            .get_by_edid(&e)
            .ok_or_else(|| anyhow::anyhow!("EditorID '{}' not found", e))
    } else {
        anyhow::bail!("specify --formid or --edid")
    }
}

fn cmd_diff(
    file_a: &PathBuf,
    file_b: &PathBuf,
    record_type: Option<&str>,
    as_json: bool,
    pretty: bool,
) -> anyhow::Result<()> {
    use fo76_esm_parser::diff::diff_databases;
    use std::time::Instant;

    let t0 = Instant::now();
    let db_a = Database::open(file_a)?;
    let t1 = Instant::now();
    eprintln!(
        "timing: opened {:?} in {:.2}s",
        file_a.file_name().unwrap_or_default(),
        t1.duration_since(t0).as_secs_f64()
    );

    let db_b = Database::open(file_b)?;
    let t2 = Instant::now();
    eprintln!(
        "timing: opened {:?} in {:.2}s",
        file_b.file_name().unwrap_or_default(),
        t2.duration_since(t1).as_secs_f64()
    );

    let mut result = diff_databases(&db_a, &db_b)?;
    let t3 = Instant::now();
    eprintln!(
        "timing: diff computed in {:.2}s ({} added, {} removed, {} changed)",
        t3.duration_since(t2).as_secs_f64(),
        result.added.len(),
        result.removed.len(),
        result.changed.len()
    );
    eprintln!(
        "timing: total elapsed {:.2}s",
        t3.duration_since(t0).as_secs_f64()
    );

    // Apply --type filter
    if let Some(sig) = record_type {
        let sig = sig.to_uppercase();
        result.added.retain(|s| s.record_type == sig);
        result.removed.retain(|s| s.record_type == sig);
        result.changed.retain(|d| d.stub.record_type == sig);
    }

    if as_json {
        let out = serde_json::to_value(&result)?;
        print_json(&out, pretty);
        return Ok(());
    }

    // Human-readable output
    println!("A: {}", file_a.display());
    println!("B: {}", file_b.display());
    println!();
    println!("Summary:");
    println!("  Added:   {}", result.added.len());
    println!("  Removed: {}", result.removed.len());
    println!("  Changed: {}", result.changed.len());

    // Group by record type for summary
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
            println!(
                "  [{}] {}",
                s.form_id,
                s.editor_id.as_deref().unwrap_or("<no edid>")
            );
        }
    }
    if !result.removed.is_empty() {
        println!();
        println!("Removed ({}):", result.removed.len());
        for s in &result.removed {
            println!(
                "  [{}] {}",
                s.form_id,
                s.editor_id.as_deref().unwrap_or("<no edid>")
            );
        }
    }
    if !result.changed.is_empty() {
        println!();
        println!("Changed ({}):", result.changed.len());
        for d in &result.changed {
            println!(
                "  [{}] {}",
                d.stub.form_id,
                d.stub.editor_id.as_deref().unwrap_or("<no edid>")
            );
            print_field_changes(&d.field_changes, "    ");
        }
    }

    Ok(())
}

fn print_field_changes(changes: &serde_json::Value, indent: &str) {
    if let Some(obj) = changes.as_object() {
        for (key, val) in obj {
            if let Some(inner) = val.as_object() {
                if inner.contains_key("from") && inner.contains_key("to") {
                    println!(
                        "{}  {}: {} \u{2192} {}",
                        indent,
                        key,
                        format_val(&inner["from"]),
                        format_val(&inner["to"])
                    );
                } else {
                    // nested diff
                    println!("{}  {}:", indent, key);
                    print_field_changes(val, &format!("{}  ", indent));
                }
            }
        }
    }
}

fn format_val(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn cmd_tree(
    file: &PathBuf,
    record_type: Option<&str>,
    offset: usize,
    limit: usize,
    pretty: bool,
) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
    if let Some(sig) = record_type {
        let children = db.list_type_children(sig, offset, limit)?;
        print_json(&serde_json::to_value(&children)?, pretty);
    } else {
        let groups = db.list_groups();
        print_json(&serde_json::to_value(&groups)?, pretty);
    }
    Ok(())
}

fn print_json(value: &serde_json::Value, pretty: bool) {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else {
        println!("{}", serde_json::to_string(value).unwrap());
    }
}

// ─── Coverage audit ──────────────────────────────────────────────────────────

/// Counts of schema-coverage markers found while walking a decoded record.
#[derive(Debug, Default, Clone, serde::Serialize)]
struct Markers {
    /// Records with no schema definition at all (`_unknown_record: true`).
    unknown_record: u64,
    /// Objects emitted by a `raw_fallback` schema member (`_raw:true` + `reason` key).
    raw_fallback: u64,
    /// Total subrecord payloads left in `_unmapped` (consumed nothing, schema was incomplete).
    unmapped: u64,
    /// Unresolved LString IDs (`_unresolved: true`).
    unresolved: u64,
    /// Total records sampled.
    records: u64,
}

impl Markers {
    fn total(&self) -> u64 {
        self.unknown_record + self.raw_fallback + self.unmapped + self.unresolved
    }

    fn add(&mut self, other: &Markers) {
        self.unknown_record += other.unknown_record;
        self.raw_fallback += other.raw_fallback;
        self.unmapped += other.unmapped;
        self.unresolved += other.unresolved;
        self.records += other.records;
    }
}

/// Recursively walk a decoded JSON value and accumulate coverage markers.
fn count_markers(v: &Value, m: &mut Markers) {
    match v {
        Value::Object(obj) => {
            // Top-level unknown record
            if obj.get("_unknown_record") == Some(&Value::Bool(true)) {
                m.unknown_record += 1;
            }
            // raw_fallback: has "_raw": true AND "reason": "..." (but NOT from _unmapped)
            if obj.get("_raw") == Some(&Value::Bool(true)) && obj.contains_key("reason") {
                m.raw_fallback += 1;
            }
            // _unresolved LString
            if obj.get("_unresolved") == Some(&Value::Bool(true)) {
                m.unresolved += 1;
            }
            // _unmapped: count the total raw subrecord entries within
            if let Some(Value::Object(unmapped)) = obj.get("_unmapped") {
                for subs in unmapped.values() {
                    if let Value::Array(arr) = subs {
                        m.unmapped += arr.len() as u64;
                    }
                }
                // Don't recurse into _unmapped (it's raw hex, nothing to count there)
            }
            // Recurse into all other fields
            for (key, child) in obj {
                if key == "_unmapped" {
                    continue; // already handled
                }
                count_markers(child, m);
            }
        }
        Value::Array(arr) => {
            for child in arr {
                count_markers(child, m);
            }
        }
        _ => {}
    }
}

fn cmd_coverage(
    file: &PathBuf,
    record_type: Option<&str>,
    sample: usize,
    as_json: bool,
    gate: bool,
) -> anyhow::Result<()> {
    let db = Database::open(file)?;

    // Collect all distinct record types from the index (sorted)
    let mut all_sigs: Vec<String> = db
        .index
        .form_index
        .values()
        .map(|m| m.signature.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    all_sigs.sort();

    // Apply --type filter
    if let Some(rt) = record_type {
        let rt_upper = rt.to_uppercase();
        all_sigs.retain(|s| *s == rt_upper);
        if all_sigs.is_empty() {
            anyhow::bail!("no records of type '{}' found", rt);
        }
    }

    // Per-type marker tallies
    let mut by_type: BTreeMap<String, Markers> = BTreeMap::new();

    for sig in &all_sigs {
        let metas: Vec<fo76_esm_parser::reader::RecordMeta> = db
            .index
            .records_by_type(sig)
            .into_iter()
            .map(|(_, m)| m.clone())
            .take(if sample == 0 { usize::MAX } else { sample })
            .collect();

        let mut type_markers = Markers::default();
        for meta in &metas {
            match db.record_at_meta(meta) {
                Ok(result) => {
                    type_markers.records += 1;
                    let mut rec_markers = Markers::default();
                    count_markers(&result.fields, &mut rec_markers);
                    type_markers.add(&rec_markers);
                }
                Err(e) => {
                    eprintln!("Warning: failed to decode {} record: {}", sig, e);
                }
            }
        }
        by_type.insert(sig.clone(), type_markers);
    }

    // Sort by total markers descending (worst first)
    let mut rows: Vec<(&String, &Markers)> = by_type.iter().collect();
    rows.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(b.0)));

    // Totals
    let totals = rows.iter().fold(Markers::default(), |mut acc, (_, m)| {
        acc.add(m);
        acc
    });

    if as_json {
        let out = serde_json::json!({
            "by_type": by_type,
            "totals": totals,
        });
        print_json(&out, true);
    } else {
        // Human-readable table
        println!(
            "{:<6}  {:>10}  {:>12}  {:>8}  {:>10}  {:>8}",
            "SIG", "records", "raw_fallback", "unmapped", "unresolved", "unknown"
        );
        println!("{}", "-".repeat(64));
        for (sig, m) in &rows {
            if m.total() > 0 || record_type.is_some() {
                println!(
                    "{:<6}  {:>10}  {:>12}  {:>8}  {:>10}  {:>8}",
                    sig, m.records, m.raw_fallback, m.unmapped, m.unresolved, m.unknown_record
                );
            }
        }
        println!("{}", "-".repeat(64));
        println!(
            "{:<6}  {:>10}  {:>12}  {:>8}  {:>10}  {:>8}",
            "TOTAL",
            totals.records,
            totals.raw_fallback,
            totals.unmapped,
            totals.unresolved,
            totals.unknown_record
        );
        if totals.total() == 0 {
            println!("\n✓ Zero coverage markers — all records fully decoded.");
        }
    }

    if gate && totals.raw_fallback > 0 {
        anyhow::bail!(
            "gate check failed: {} raw_fallback marker(s) found across {} record types",
            totals.raw_fallback,
            rows.iter().filter(|(_, m)| m.raw_fallback > 0).count()
        );
    }

    Ok(())
}
