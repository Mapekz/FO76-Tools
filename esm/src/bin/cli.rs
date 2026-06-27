use clap::{Parser, Subcommand, ValueEnum};
use esm::backend::{start_daemon_process, stop_daemon, LocalBackend, QueryBackend, RemoteBackend};
use esm::ipc::{Op, RecordSel};
use esm::{
    parse_form_id_input, CoverageReport, Database, DiffResult, Markers, RawRecordView, RecordRow,
    RefList, ResolveDepth, SearchField,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "esm", about = "Read and inspect Fallout 76 ESM files")]
struct Cli {
    #[arg(short = 'p', long)]
    print: bool,
    #[arg(long)]
    local: bool,
    #[arg(long)]
    addr: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    esm: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    Info {
        file: PathBuf,
    },
    Get {
        file: PathBuf,
        /// FormID or EditorID (auto-detected); overridden by --formid/--edid
        #[arg(conflicts_with_all = ["formid", "edid"])]
        target: Option<String>,
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
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        #[arg(long, default_value = "en")]
        lang: String,
        #[arg(long)]
        startup_ba2: Option<PathBuf>,
        #[arg(long, default_value = "none")]
        resolve: String,
    },
    List {
        file: PathBuf,
        #[arg(long)]
        r#type: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        #[arg(long, default_value = "en")]
        lang: String,
    },
    Diff {
        file_a: PathBuf,
        file_b: PathBuf,
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
    },
    Tree {
        file: PathBuf,
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        pretty: bool,
    },
    Coverage {
        file: PathBuf,
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long, default_value_t = 0)]
        sample: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        gate: bool,
    },
    Refs {
        file: PathBuf,
        /// FormID or EditorID (auto-detected); overridden by --formid/--edid
        #[arg(conflicts_with_all = ["formid", "edid"])]
        target: Option<String>,
        #[arg(long, conflicts_with = "edid")]
        formid: Option<String>,
        #[arg(long, conflicts_with = "formid")]
        edid: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        #[arg(long, default_value = "en")]
        lang: String,
    },
    Search {
        file: PathBuf,
        pattern: String,
        #[arg(long = "type", value_delimiter = ',')]
        types: Vec<String>,
        #[arg(long = "in", value_enum, default_value = "both")]
        search_in: SearchInArg,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        #[arg(long, conflicts_with = "strings_dir")]
        strings: Option<PathBuf>,
        #[arg(long, conflicts_with = "strings")]
        strings_dir: Option<PathBuf>,
        #[arg(long, default_value = "en")]
        lang: String,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    Start,
    Stop,
    Status,
}

#[derive(Parser)]
enum ReplCommand {
    Info,
    Get {
        /// FormID or EditorID (auto-detected); overridden by --formid/--edid
        #[arg(conflicts_with_all = ["formid", "edid"])]
        target: Option<String>,
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
        #[arg(long, default_value = "none")]
        resolve: String,
    },
    List {
        #[arg(long)]
        r#type: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Diff {
        file_b: PathBuf,
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
    },
    Tree {
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        pretty: bool,
    },
    Coverage {
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long, default_value_t = 0)]
        sample: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        gate: bool,
    },
    Refs {
        /// FormID or EditorID (auto-detected); overridden by --formid/--edid
        #[arg(conflicts_with_all = ["formid", "edid"])]
        target: Option<String>,
        #[arg(long, conflicts_with = "edid")]
        formid: Option<String>,
        #[arg(long, conflicts_with = "formid")]
        edid: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
    },
    Search {
        pattern: String,
        #[arg(long = "type", value_delimiter = ',')]
        types: Vec<String>,
        #[arg(long = "in", value_enum, default_value = "both")]
        search_in: SearchInArg,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SearchInArg {
    Edid,
    Name,
    Both,
}

enum Backend {
    Local(LocalBackend),
    Remote(RemoteBackend),
}

impl QueryBackend for Backend {
    fn run(&mut self, esm: &Path, op: Op) -> anyhow::Result<Value> {
        match self {
            Backend::Local(b) => b.run(esm, op),
            Backend::Remote(b) => b.run(esm, op),
        }
    }
}

fn make_backend(local: bool, addr: Option<&str>, port: Option<u16>) -> anyhow::Result<Backend> {
    if local {
        Ok(Backend::Local(LocalBackend::new()))
    } else {
        Ok(Backend::Remote(RemoteBackend::connect_with_override(
            addr, port,
        )?))
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(Commands::Daemon { action }) = cli.command {
        return match action {
            DaemonAction::Start => {
                let info = start_daemon_process()?;
                println!(
                    "daemon running on 127.0.0.1:{} (pid {})",
                    info.port, info.pid
                );
                Ok(())
            }
            DaemonAction::Stop => {
                stop_daemon()?;
                println!("daemon stopped");
                Ok(())
            }
            DaemonAction::Status => {
                let remote =
                    RemoteBackend::connect_existing_with_override(cli.addr.as_deref(), cli.port)?;
                let status = remote.status()?;
                println!("{}", serde_json::to_string_pretty(&status)?);
                Ok(())
            }
        };
    }

    // -p  → one-shot print; use an already-running daemon if available, else cold local load.
    // no -p → REPL mode; always daemon-backed (spawns one if not running).
    let mut backend = if cli.print && !cli.local {
        match RemoteBackend::connect_existing_with_override(cli.addr.as_deref(), cli.port) {
            Ok(r) => Backend::Remote(r),
            Err(_) => Backend::Local(LocalBackend::new()),
        }
    } else {
        make_backend(cli.local, cli.addr.as_deref(), cli.port)?
    };
    let daemon_mode = matches!(backend, Backend::Remote(_));

    if let Some(cmd) = cli.command {
        let esm_for_repl = match &cmd {
            Commands::Info { file } => file.clone(),
            Commands::Get { file, .. } => file.clone(),
            Commands::List { file, .. } => file.clone(),
            Commands::Diff { file_a, .. } => file_a.clone(),
            Commands::Tree { file, .. } => file.clone(),
            Commands::Coverage { file, .. } => file.clone(),
            Commands::Refs { file, .. } => file.clone(),
            Commands::Search { file, .. } => file.clone(),
            Commands::Daemon { .. } => unreachable!(),
        };
        match cmd {
            Commands::Info { file } => cmd_info(&mut backend, &file)?,
            Commands::Get {
                file,
                target,
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
                &mut backend,
                &file,
                formid,
                edid,
                target,
                json,
                pretty,
                raw,
                strings,
                strings_dir,
                &lang,
                startup_ba2,
                resolve,
                daemon_mode,
            )?,
            Commands::List {
                file,
                r#type,
                limit,
                strings,
                strings_dir,
                lang,
            } => cmd_list(
                &mut backend,
                &file,
                &r#type,
                limit,
                strings,
                strings_dir,
                &lang,
                daemon_mode,
            )?,
            Commands::Diff {
                file_a,
                file_b,
                record_type,
                json,
                pretty,
            } => cmd_diff(
                &mut backend,
                &file_a,
                &file_b,
                record_type.as_deref(),
                json,
                pretty,
            )?,
            Commands::Tree {
                file,
                record_type,
                offset,
                limit,
                pretty,
            } => cmd_tree(
                &mut backend,
                &file,
                record_type.as_deref(),
                offset,
                limit,
                pretty,
            )?,
            Commands::Coverage {
                file,
                record_type,
                sample,
                json,
                gate,
            } => cmd_coverage(
                &mut backend,
                &file,
                record_type.as_deref(),
                sample,
                json,
                gate,
            )?,
            Commands::Refs {
                file,
                target,
                formid,
                edid,
                limit,
                json,
                pretty,
                strings,
                strings_dir,
                lang,
            } => cmd_refs(
                &mut backend,
                &file,
                formid,
                edid,
                target,
                limit,
                json,
                pretty,
                strings,
                strings_dir,
                &lang,
                daemon_mode,
            )?,
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
                &mut backend,
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
                daemon_mode,
            )?,
            Commands::Daemon { .. } => unreachable!(),
        }
        if !cli.print {
            return run_repl(&esm_for_repl, &mut backend);
        }
        return Ok(());
    }

    // No subcommand: pure REPL
    let esm = cli
        .esm
        .ok_or_else(|| anyhow::anyhow!("ESM path required for REPL session"))?;
    run_repl(&esm, &mut backend)
}

fn run_repl(esm: &Path, backend: &mut Backend) -> anyhow::Result<()> {
    eprintln!(
        "esm REPL — session: {} (type 'help' for commands)",
        esm.display()
    );
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    loop {
        write!(stdout, "esm> ")?;
        stdout.flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "quit" || line == "exit" {
            break;
        }
        if line == "help" {
            eprintln!("Commands: info, get, list, search, refs, tree, diff, coverage, quit");
            continue;
        }
        let tokens: Vec<String> = shlex::split(line)
            .unwrap_or_else(|| line.split_whitespace().map(String::from).collect());
        let args: Vec<String> = std::iter::once("esm".to_string()).chain(tokens).collect();
        let cmd = match ReplCommand::try_parse_from(&args) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        if let Err(e) = dispatch_repl(esm, backend, cmd) {
            eprintln!("error: {:#}", e);
        }
    }
    Ok(())
}

fn dispatch_repl(esm: &Path, backend: &mut Backend, cmd: ReplCommand) -> anyhow::Result<()> {
    match cmd {
        ReplCommand::Info => cmd_info(backend, esm),
        ReplCommand::Get {
            target,
            formid,
            edid,
            json,
            pretty,
            raw,
            resolve,
        } => cmd_get(
            backend, esm, formid, edid, target, json, pretty, raw, None, None, "en", None, resolve,
            true,
        ),
        ReplCommand::List { r#type, limit } => {
            cmd_list(backend, esm, &r#type, limit, None, None, "en", true)
        }
        ReplCommand::Diff {
            file_b,
            record_type,
            json,
            pretty,
        } => cmd_diff(backend, esm, &file_b, record_type.as_deref(), json, pretty),
        ReplCommand::Tree {
            record_type,
            offset,
            limit,
            pretty,
        } => cmd_tree(backend, esm, record_type.as_deref(), offset, limit, pretty),
        ReplCommand::Coverage {
            record_type,
            sample,
            json,
            gate,
        } => cmd_coverage(backend, esm, record_type.as_deref(), sample, json, gate),
        ReplCommand::Refs {
            target,
            formid,
            edid,
            limit,
            json,
            pretty,
        } => cmd_refs(
            backend, esm, formid, edid, target, limit, json, pretty, None, None, "en", true,
        ),
        ReplCommand::Search {
            pattern,
            types,
            search_in,
            limit,
            json,
            pretty,
        } => cmd_search(
            backend, esm, &pattern, types, search_in, limit, json, pretty, None, None, "en", true,
        ),
    }
}

fn cmd_info(backend: &mut Backend, file: &Path) -> anyhow::Result<()> {
    let info: esm::reader::FileInfo = serde_json::from_value(backend.run(file, Op::FileInfo)?)?;
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

fn esm_string_prefix(esm_path: &Path) -> String {
    esm_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "SeventySix".to_string())
}

fn apply_strings_override(
    db: &mut Database,
    esm_path: &Path,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) {
    if let Some(ba2_path) = strings {
        match esm::strings::Localization::from_ba2(&ba2_path, lang, "seventysix") {
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

fn parse_resolve(s: &str) -> ResolveDepth {
    match s {
        "stub" => ResolveDepth::Stub,
        "full" => ResolveDepth::Full,
        _ => ResolveDepth::None,
    }
}

fn record_sel(
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
) -> anyhow::Result<RecordSel> {
    if let Some(fid) = formid {
        Ok(RecordSel::FormId(parse_form_id_input(&fid)?))
    } else if let Some(e) = edid {
        Ok(RecordSel::Edid(e))
    } else if let Some(t) = target {
        RecordSel::from_input(&t)
    } else {
        anyhow::bail!("specify a FormID/EditorID (positional), or --formid/--edid");
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_get(
    backend: &mut Backend,
    file: &Path,
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
    json: bool,
    pretty: bool,
    raw: bool,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    startup_ba2: Option<PathBuf>,
    resolve: String,
    daemon_mode: bool,
) -> anyhow::Result<()> {
    let has_overrides = strings.is_some() || strings_dir.is_some() || startup_ba2.is_some();
    if has_overrides && daemon_mode {
        anyhow::bail!(
            "--strings/--strings-dir/--startup-ba2 are not supported in daemon mode; \
             use --local to open the ESM directly"
        );
    }
    if has_overrides {
        let mut db = Database::open(file)?;
        apply_strings_override(&mut db, file, strings, strings_dir, lang);
        if let Some(ba2_path) = startup_ba2 {
            db.load_curves(&ba2_path)?;
        }
        let sel = record_sel(formid, edid, target)?;
        let depth = parse_resolve(&resolve);
        if raw {
            let form_id = match &sel {
                RecordSel::FormId(f) => *f,
                RecordSel::Edid(e) => {
                    db.index.ensure_edid_index(&db.esm)?;
                    db.index
                        .get_by_edid(e)
                        .ok_or_else(|| anyhow::anyhow!("EditorID '{}' not found", e))?
                }
            };
            let rec = db.record_raw(form_id)?;
            let view = RawRecordView {
                header: rec.header,
                subrecords: rec
                    .subrecords
                    .iter()
                    .map(|sr| esm::RawSubrecordView {
                        signature: sr.signature.to_string(),
                        size: sr.data.len(),
                        hex: sr.data.iter().map(|b| format!("{:02x}", b)).collect(),
                    })
                    .collect(),
            };
            print_json(&serde_json::to_value(&view)?, pretty || !json);
            return Ok(());
        }
        let result = match (&sel, depth) {
            (RecordSel::FormId(f), ResolveDepth::None) => db.record_by_formid(*f)?,
            (RecordSel::Edid(e), ResolveDepth::None) => db.record_by_edid(e)?,
            (RecordSel::FormId(f), d) => db.record_by_formid_resolved(*f, d)?,
            (RecordSel::Edid(e), d) => db.record_by_edid_resolved(e, d)?,
        };
        let out = serde_json::json!({
            "header": result.header,
            "editor_id": result.editor_id,
            "fields": result.fields
        });
        print_json(&out, pretty || !json);
        return Ok(());
    }

    let sel = record_sel(formid, edid, target)?;
    let depth = parse_resolve(&resolve);
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
fn cmd_list(
    backend: &mut Backend,
    file: &Path,
    sig: &str,
    limit: usize,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    daemon_mode: bool,
) -> anyhow::Result<()> {
    if strings.is_some() || strings_dir.is_some() {
        if daemon_mode {
            anyhow::bail!(
                "--strings/--strings-dir are not supported in daemon mode; \
                 use --local to open the ESM directly"
            );
        }
        let mut db = Database::open(file)?;
        apply_strings_override(&mut db, file, strings, strings_dir, lang);
        let entries = db.list_by_type(sig, limit)?;
        print_list_entries(&entries);
        return Ok(());
    }
    let v = backend.run(
        file,
        Op::ListByType {
            sig: sig.to_string(),
            limit,
        },
    )?;
    let entries: Vec<esm::ListEntry> = serde_json::from_value(v)?;
    print_list_entries(&entries);
    Ok(())
}

fn print_list_entries(entries: &[esm::ListEntry]) {
    for e in entries {
        print!(
            "{}  {}",
            e.form_id,
            e.editor_id.as_deref().unwrap_or("<no edid>")
        );
        if let Some(full) = &e.full_lstring_id {
            print!("  FULL={}", full);
        }
        println!();
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_refs(
    backend: &mut Backend,
    file: &Path,
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
    limit: usize,
    json: bool,
    pretty: bool,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    daemon_mode: bool,
) -> anyhow::Result<()> {
    if strings.is_some() || strings_dir.is_some() {
        if daemon_mode {
            anyhow::bail!(
                "--strings/--strings-dir are not supported in daemon mode; \
                 use --local to open the ESM directly"
            );
        }
        let mut db = Database::open(file)?;
        apply_strings_override(&mut db, file, strings, strings_dir, lang);
        let form_id = resolve_form_id_local(&mut db, formid, edid, target)?;
        let ref_list = esm::ipc::referenced_by_enriched(&mut db, form_id, limit)?;
        print_refs(&ref_list, json, pretty);
        return Ok(());
    }
    let sel = record_sel(formid, edid, target)?;
    let v = backend.run(file, Op::ReferencedBy { sel, limit })?;
    let ref_list: RefList = serde_json::from_value(v)?;
    print_refs(&ref_list, json, pretty);
    Ok(())
}

fn print_refs(ref_list: &RefList, json: bool, pretty: bool) {
    if json {
        print_json(&serde_json::to_value(&ref_list.rows).unwrap(), pretty);
    } else {
        for row in &ref_list.rows {
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
        if ref_list.rows.is_empty() {
            eprintln!("note: no records reference {}", ref_list.target);
        }
    }
    if ref_list.capped {
        eprintln!(
            "note: output capped at {} of {} results; use --limit 0 to show all",
            ref_list.rows.len(),
            ref_list.total
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_search(
    backend: &mut Backend,
    file: &Path,
    pattern: &str,
    types: Vec<String>,
    search_in: SearchInArg,
    limit: usize,
    json: bool,
    pretty: bool,
    strings: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    daemon_mode: bool,
) -> anyhow::Result<()> {
    let field = match search_in {
        SearchInArg::Edid => SearchField::Edid,
        SearchInArg::Name => SearchField::Name,
        SearchInArg::Both => SearchField::Both,
    };

    if strings.is_some() || strings_dir.is_some() {
        if daemon_mode {
            anyhow::bail!(
                "--strings/--strings-dir are not supported in daemon mode; \
                 use --local to open the ESM directly"
            );
        }
        let mut db = Database::open(file)?;
        apply_strings_override(&mut db, file, strings, strings_dir, lang);
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

fn print_search_results(results: &[RecordRow], limit: usize, json: bool, pretty: bool) {
    let capped = limit > 0 && results.len() == limit;
    if json {
        print_json(&serde_json::to_value(results).unwrap(), pretty);
    } else {
        for row in results {
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
}

fn resolve_form_id_local(
    db: &mut Database,
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
) -> anyhow::Result<esm::FormId> {
    match record_sel(formid, edid, target)? {
        RecordSel::FormId(fid) => Ok(fid),
        RecordSel::Edid(e) => {
            db.index.ensure_edid_index(&db.esm)?;
            db.index
                .get_by_edid(&e)
                .ok_or_else(|| anyhow::anyhow!("EditorID '{}' not found", e))
        }
    }
}

fn cmd_diff(
    backend: &mut Backend,
    file_a: &Path,
    file_b: &Path,
    record_type: Option<&str>,
    as_json: bool,
    pretty: bool,
) -> anyhow::Result<()> {
    let v = backend.run(
        file_a,
        Op::Diff {
            b: file_b.to_path_buf(),
            record_type: record_type.map(|s| s.to_string()),
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

fn print_field_changes(changes: &Value, indent: &str) {
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
                    println!("{}  {}:", indent, key);
                    print_field_changes(val, &format!("{}  ", indent));
                }
            }
        }
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

fn cmd_tree(
    backend: &mut Backend,
    file: &Path,
    record_type: Option<&str>,
    offset: usize,
    limit: usize,
    pretty: bool,
) -> anyhow::Result<()> {
    let v = if let Some(sig) = record_type {
        backend.run(
            file,
            Op::ListTypeChildren {
                sig: sig.to_string(),
                offset,
                limit,
            },
        )?
    } else {
        backend.run(file, Op::ListGroups)?
    };
    print_json(&v, pretty);
    Ok(())
}

fn print_json(value: &Value, pretty: bool) {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else {
        println!("{}", serde_json::to_string(value).unwrap());
    }
}

fn cmd_coverage(
    backend: &mut Backend,
    file: &Path,
    record_type: Option<&str>,
    sample: usize,
    as_json: bool,
    gate: bool,
) -> anyhow::Result<()> {
    let v = backend.run(
        file,
        Op::Coverage {
            record_type: record_type.map(|s| s.to_string()),
            sample,
        },
    )?;
    let report: CoverageReport = serde_json::from_value(v)?;

    if as_json {
        print_json(&serde_json::to_value(&report)?, true);
    } else {
        let mut rows: Vec<(&String, &Markers)> = report.by_type.iter().collect();
        rows.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(b.0)));

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
        let totals = &report.totals;
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

    if gate && report.totals.raw_fallback > 0 {
        anyhow::bail!(
            "gate check failed: {} raw_fallback marker(s) found",
            report.totals.raw_fallback
        );
    }
    Ok(())
}
