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
    Info {
        file: PathBuf,
    },
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
    },
    /// List records of a given type
    List {
        file: PathBuf,
        #[arg(long)]
        r#type: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
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
        } => cmd_get(&file, formid, edid, json, pretty, raw),
        Commands::List { file, r#type, limit } => cmd_list(&file, &r#type, limit),
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

fn cmd_get(
    file: &PathBuf,
    formid: Option<String>,
    edid: Option<String>,
    json: bool,
    pretty: bool,
    raw: bool,
) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
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

    let result = if let Some(fid) = formid {
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

fn cmd_list(file: &PathBuf, sig: &str, limit: usize) -> anyhow::Result<()> {
    let mut db = Database::open(file)?;
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

fn print_json(value: &serde_json::Value, pretty: bool) {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else {
        println!("{}", serde_json::to_string(value).unwrap());
    }
}
