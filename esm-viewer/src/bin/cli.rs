use clap::{Parser, Subcommand};
use fo76_esm_parser::{parse_form_id_input, Database};
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
        #[arg(long)]
        strings: Option<PathBuf>,
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
        #[arg(long)]
        strings: Option<PathBuf>,
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
            &lang,
            startup_ba2,
            resolve,
        ),
        Commands::List {
            file,
            r#type,
            limit,
            strings,
            lang,
        } => cmd_list(&file, &r#type, limit, strings, &lang),
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

fn apply_strings_override(db: &mut Database, strings: Option<PathBuf>, lang: &str) {
    if let Some(ba2_path) = strings {
        match fo76_esm_parser::strings::Localization::from_ba2(&ba2_path, lang, "seventysix") {
            Ok(loc) => db.set_localization(loc),
            Err(e) => eprintln!(
                "Warning: failed to load localization from {}: {}",
                ba2_path.display(),
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
    lang: &str,
    startup_ba2: Option<PathBuf>,
    resolve: String,
) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
    apply_strings_override(&mut db, strings, lang);
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
    lang: &str,
) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
    apply_strings_override(&mut db, strings, lang);
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
    use std::collections::BTreeMap;

    let db_a = Database::open(file_a)?;
    let db_b = Database::open(file_b)?;
    let mut result = diff_databases(&db_a, &db_b)?;

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
