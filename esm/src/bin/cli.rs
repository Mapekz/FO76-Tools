use anyhow::Context as _;
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
    /// One-shot print mode: run the command and exit (auto-spawns a warm daemon
    /// if none is running, so repeated `-p` calls avoid cold reloads).
    #[arg(short = 'p', long)]
    print: bool,
    /// Force in-process (cold) open, bypassing the daemon entirely.
    #[arg(long)]
    local: bool,
    #[arg(long)]
    addr: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    /// Use the zero-copy mmap form index for FormID lookups (with --local).
    ///
    /// Loads a compact ~24 MiB `.esm.midx` instead of the full ~280 MiB
    /// `.esm.idx` bincode cache, making cold FormID lookups sub-second without
    /// a background daemon.  EditorID / list / search / refs / tree require the
    /// full index and will error in this mode — use the daemon for those.
    /// Env: ESM_MMAP_INDEX=1.
    #[arg(long, env = "ESM_MMAP_INDEX")]
    mmap_index: bool,
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
        #[arg(long = "localization-ba2", conflicts_with = "strings_dir")]
        localization_ba2: Option<PathBuf>,
        #[arg(long, conflicts_with = "localization_ba2")]
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
        #[arg(long = "localization-ba2", conflicts_with = "strings_dir")]
        localization_ba2: Option<PathBuf>,
        #[arg(long, conflicts_with = "localization_ba2")]
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
        /// Localization BA2 for both ESMs.
        /// Mutually exclusive with --strings-dir / --strings-dir-a/b / --localization-ba2-a/b.
        #[arg(long = "localization-ba2", conflicts_with_all = ["strings_dir", "strings_dir_a", "strings_dir_b", "localization_ba2_a", "localization_ba2_b"])]
        localization_ba2: Option<PathBuf>,
        /// Localization BA2 for ESM A only (old side).
        #[arg(long = "localization-ba2-a", conflicts_with_all = ["localization_ba2", "strings_dir", "strings_dir_a"])]
        localization_ba2_a: Option<PathBuf>,
        /// Localization BA2 for ESM B only (new side).
        #[arg(long = "localization-ba2-b", conflicts_with_all = ["localization_ba2", "strings_dir", "strings_dir_b"])]
        localization_ba2_b: Option<PathBuf>,
        /// Directory with loose string files for BOTH ESMs.
        /// Mutually exclusive with --localization-ba2 / --strings-dir-a/b / --localization-ba2-a/b.
        #[arg(long, conflicts_with_all = ["localization_ba2", "strings_dir_a", "strings_dir_b", "localization_ba2_a", "localization_ba2_b"])]
        strings_dir: Option<PathBuf>,
        /// Strings directory for ESM A only (old side).
        #[arg(long, conflicts_with_all = ["localization_ba2", "strings_dir", "localization_ba2_a"])]
        strings_dir_a: Option<PathBuf>,
        /// Strings directory for ESM B only (new side).
        #[arg(long, conflicts_with_all = ["localization_ba2", "strings_dir", "localization_ba2_b"])]
        strings_dir_b: Option<PathBuf>,
        /// Language code for string table lookup.
        #[arg(long, default_value = "en")]
        lang: String,
        /// Startup BA2 for curve tables (both ESMs).
        /// Mutually exclusive with --curves-dir / --startup-ba2-a/b / --curves-dir-a/b.
        #[arg(long, conflicts_with_all = ["curves_dir", "startup_ba2_a", "startup_ba2_b", "curves_dir_a", "curves_dir_b"])]
        startup_ba2: Option<PathBuf>,
        /// Startup BA2 for ESM A only (old side).
        #[arg(long, conflicts_with_all = ["startup_ba2", "curves_dir", "curves_dir_a"])]
        startup_ba2_a: Option<PathBuf>,
        /// Startup BA2 for ESM B only (new side).
        #[arg(long, conflicts_with_all = ["startup_ba2", "curves_dir", "curves_dir_b"])]
        startup_ba2_b: Option<PathBuf>,
        /// Loose misc/ directory for curve tables (both ESMs).
        /// Mutually exclusive with --startup-ba2 / --startup-ba2-a/b / --curves-dir-a/b.
        #[arg(long, conflicts_with_all = ["startup_ba2", "startup_ba2_a", "startup_ba2_b", "curves_dir_a", "curves_dir_b"])]
        curves_dir: Option<PathBuf>,
        /// Loose misc/ directory for ESM A only (old side).
        #[arg(long, conflicts_with_all = ["startup_ba2", "curves_dir", "startup_ba2_a"])]
        curves_dir_a: Option<PathBuf>,
        /// Loose misc/ directory for ESM B only (new side).
        #[arg(long, conflicts_with_all = ["startup_ba2", "curves_dir", "startup_ba2_b"])]
        curves_dir_b: Option<PathBuf>,
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
        /// Reverse-reference walk depth (1 = direct refs only, up to 6).
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        #[arg(long = "localization-ba2", conflicts_with = "strings_dir")]
        localization_ba2: Option<PathBuf>,
        #[arg(long, conflicts_with = "localization_ba2")]
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
        #[arg(long = "localization-ba2", conflicts_with = "strings_dir")]
        localization_ba2: Option<PathBuf>,
        #[arg(long, conflicts_with = "localization_ba2")]
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
        /// Reverse-reference walk depth (1 = direct refs only, up to 6).
        #[arg(long, default_value_t = 1)]
        depth: usize,
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

    // -p  → one-shot print; auto-spawns a warm daemon if none is running
    //        (same as no-p REPL mode, but exits after the single command).
    // no -p → REPL mode; always daemon-backed (spawns one if not running).
    // --local → bypass daemon entirely for both modes (cold in-process open).
    let mut backend = if cli.print && !cli.local {
        Backend::Remote(RemoteBackend::connect_with_override(
            cli.addr.as_deref(),
            cli.port,
        )?)
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
                localization_ba2,
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
                localization_ba2,
                strings_dir,
                &lang,
                startup_ba2,
                resolve,
                daemon_mode,
                cli.mmap_index,
            )?,
            Commands::List {
                file,
                r#type,
                limit,
                localization_ba2,
                strings_dir,
                lang,
            } => cmd_list(
                &mut backend,
                &file,
                &r#type,
                limit,
                localization_ba2,
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
                localization_ba2,
                localization_ba2_a,
                localization_ba2_b,
                strings_dir,
                strings_dir_a,
                strings_dir_b,
                lang,
                startup_ba2,
                startup_ba2_a,
                startup_ba2_b,
                curves_dir,
                curves_dir_a,
                curves_dir_b,
            } => cmd_diff(
                &mut backend,
                &file_a,
                &file_b,
                record_type.as_deref(),
                json,
                pretty,
                localization_ba2,
                localization_ba2_a,
                localization_ba2_b,
                strings_dir,
                strings_dir_a,
                strings_dir_b,
                &lang,
                startup_ba2,
                startup_ba2_a,
                startup_ba2_b,
                curves_dir,
                curves_dir_a,
                curves_dir_b,
                daemon_mode,
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
                depth,
                json,
                pretty,
                localization_ba2,
                strings_dir,
                lang,
            } => cmd_refs(
                &mut backend,
                &file,
                formid,
                edid,
                target,
                limit,
                depth,
                json,
                pretty,
                localization_ba2,
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
                localization_ba2,
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
                localization_ba2,
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
            true,  // daemon_mode (REPL is always daemon-backed)
            false, // mmap_index (not applicable in REPL)
        ),
        ReplCommand::List { r#type, limit } => {
            cmd_list(backend, esm, &r#type, limit, None, None, "en", true)
        }
        ReplCommand::Diff {
            file_b,
            record_type,
            json,
            pretty,
        } => cmd_diff(
            backend,
            esm,
            &file_b,
            record_type.as_deref(),
            json,
            pretty,
            None, // localization_ba2
            None, // localization_ba2_a
            None, // localization_ba2_b
            None, // strings_dir
            None, // strings_dir_a
            None, // strings_dir_b
            "en",
            None, // startup_ba2
            None, // startup_ba2_a
            None, // startup_ba2_b
            None, // curves_dir
            None, // curves_dir_a
            None, // curves_dir_b
            true, // daemon_mode
        ),
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
            depth,
            json,
            pretty,
        } => cmd_refs(
            backend, esm, formid, edid, target, limit, depth, json, pretty, None, None, "en", true,
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
        .unwrap_or_else(|| "game".to_string())
}

fn apply_strings_override(
    db: &mut Database,
    esm_path: &Path,
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
) {
    if let Some(ba2_path) = localization_ba2 {
        match esm::strings::Localization::from_ba2(&ba2_path, lang) {
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
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    startup_ba2: Option<PathBuf>,
    resolve: String,
    daemon_mode: bool,
    mmap_index: bool,
) -> anyhow::Result<()> {
    let has_overrides =
        localization_ba2.is_some() || strings_dir.is_some() || startup_ba2.is_some();

    // ── mmap-index fast path (--local --mmap-index, FormID only) ─────────────
    // Loads the compact ~24 MiB .esm.midx instead of the full .esm.idx.
    // Only active in local mode (--local); ignored when hitting the daemon.
    if mmap_index && !daemon_mode && !has_overrides {
        let sel = record_sel(formid.clone(), edid.clone(), target.clone())?;
        if let RecordSel::Edid(_) = &sel {
            anyhow::bail!(
                "--mmap-index only supports FormID lookups; \
                 for EditorID use the warm daemon (`esm daemon start`) \
                 or remove --mmap-index"
            );
        }
        let form_id = match sel {
            RecordSel::FormId(f) => f,
            RecordSel::Edid(_) => unreachable!(),
        };
        let db = Database::open_lite(file)?;
        let depth = parse_resolve(&resolve);
        if raw {
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
        let result = db.record_by_formid_resolved(form_id, depth)?;
        let out = serde_json::json!({
            "header": result.header,
            "editor_id": result.editor_id,
            "fields": result.fields
        });
        print_json(&out, pretty || !json);
        return Ok(());
    }
    if has_overrides && daemon_mode {
        anyhow::bail!(
            "--localization-ba2/--strings-dir/--startup-ba2 are not supported in daemon mode; \
             use --local to open the ESM directly"
        );
    }
    if has_overrides {
        let esm_path = esm::discover::resolve_sources(file, "en")?.esm;
        let mut db = Database::open(&esm_path)?;
        apply_strings_override(&mut db, &esm_path, localization_ba2, strings_dir, lang);
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
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    daemon_mode: bool,
) -> anyhow::Result<()> {
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
    depth: usize,
    json: bool,
    pretty: bool,
    localization_ba2: Option<PathBuf>,
    strings_dir: Option<PathBuf>,
    lang: &str,
    daemon_mode: bool,
) -> anyhow::Result<()> {
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
        let form_id = resolve_form_id_local(&mut db, formid, edid, target)?;
        let ref_list = esm::ipc::referenced_by_enriched(&mut db, form_id, depth, limit)?;
        print_refs(&ref_list, json, pretty);
        return Ok(());
    }
    let sel = record_sel(formid, edid, target)?;
    let v = backend.run(file, Op::ReferencedBy { sel, limit, depth })?;
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
            if !row.path.is_empty() {
                let chain: Vec<_> = row.path.iter().map(|n| n.form_id.as_str()).collect();
                print!("  via {}", chain.join(" → "));
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
        return Localization::from_ba2(&ba2, lang)
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
        return Localization::from_ba2(&ba2, lang)
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
fn cmd_diff(
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
    daemon_mode: bool,
) -> anyhow::Result<()> {
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
        if daemon_mode {
            anyhow::bail!(
                "--localization-ba2*/--strings-dir*/--startup-ba2*/--curves-dir* are not \
                 supported in daemon mode for diff; use --local to open the ESM files directly"
            );
        }

        // Resolve folder → ESM so that esm_string_prefix/resolve_localization_or_bail
        // receive the actual .esm path (not a folder).
        let esm_a = esm::discover::resolve_sources(file_a, "en")?.esm;
        let esm_b = esm::discover::resolve_sources(file_b, "en")?.esm;

        let mut db_a = Database::open(&esm_a)?;
        let mut db_b = Database::open(&esm_b)?;

        // Load localization per side — each side is independently optional.
        if lba2_a.is_some() || sd_a.is_some() {
            let loc_a = resolve_localization_or_bail(&esm_a, lba2_a, sd_a, lang)?;
            db_a.set_localization(loc_a);
        }
        if lba2_b.is_some() || sd_b.is_some() {
            let loc_b = resolve_localization_or_bail(&esm_b, lba2_b, sd_b, lang)?;
            db_b.set_localization(loc_b);
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

        let mut result = esm::diff::diff_databases(&db_a, &db_b)?;

        // Apply optional --type filter.
        esm::diff::apply_type_filter(&mut result, &record_type.map(str::to_string));

        return print_diff(file_a, file_b, &mut result, record_type, as_json, pretty);
    }

    // No local flags — use the backend path (daemon or local).
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
