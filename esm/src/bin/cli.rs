use anyhow::Context as _;
use clap::{Parser, Subcommand, ValueEnum};
use esm::backend::{
    daemon_fresh, read_daemon_info, start_daemon_process, stop_daemon, LocalBackend, QueryBackend,
    RemoteBackend,
};
use esm::ipc::{Op, RecordSel};
use esm::{
    BodyDetail, CoverageReport, Database, DiffResult, Markers, RecordRow, RefList, ResolveDepth,
    SearchField,
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
    /// Path to the ESM file or its data folder. If omitted, falls back to the
    /// FO76_ESM_PATH environment variable. Applies to every subcommand except
    /// `diff` (which takes two explicit positionals), `daemon`, and `skill`
    /// (neither needs an ESM at all).
    #[arg(long, global = true, env = "FO76_ESM_PATH")]
    esm: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Args)]
struct DiffArgs {
    file_a: PathBuf,
    file_b: PathBuf,
    #[arg(long = "type")]
    record_type: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    pretty: bool,
    /// Detail level for decoded fields attached to added/removed record stubs.
    #[arg(long, value_enum, default_value = "full")]
    bodies: BodiesArg,
    /// Keep noisy fields (placement transforms, CELL precombine bookkeeping,
    /// Object Bounds) instead of suppressing them from `changed` records.
    #[arg(long)]
    keep_noise: bool,
    /// Record-type signature(s) to omit entirely from added/removed/changed
    /// (repeatable and/or comma-delimited, e.g. `--exclude-type LAND,NAVM`).
    #[arg(long = "exclude-type", value_delimiter = ',')]
    exclude_type: Vec<String>,
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
}

#[derive(Subcommand)]
enum Commands {
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    Info,
    Get {
        /// FormID(s) and/or EditorID(s) (auto-detected per token); mix
        /// freely, e.g. `0x0000463F 0x000228AB co_Weapon_...`. A single
        /// target preserves the classic single-record output; two or more
        /// emit a JSON array (one entry per selector, in the order given,
        /// each tagged with its own `sel`). Overridden by --formid/--edid
        /// for the classic single-selector form.
        #[arg(conflicts_with_all = ["formid", "edid"])]
        targets: Vec<String>,
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
        #[arg(long)]
        r#type: String,
        #[arg(long, default_value_t = 50)]
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
    /// Boxed to keep `Commands` from ballooning in size (`diff` carries far
    /// more fields — per-side BA2/strings/curves overrides — than every
    /// other variant combined).
    Diff(Box<DiffArgs>),
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
        /// Narrow rows to referencing records of this 4-character type
        /// (e.g. `OMOD`); case-insensitive. Applied server-side, so `--limit`/
        /// `--depth` interact correctly with the filter.
        #[arg(long = "type")]
        record_type: Option<String>,
        /// Annotate each row with the JSON field path(s) where it references
        /// its predecessor in the hop chain (e.g.
        /// `Effects[2].Conditions[0].Parameter 1`). Decodes every emitted row
        /// — off by default.
        #[arg(long)]
        paths: bool,
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
    /// Automate the "chase pattern" (see .claude/skills/patch-notes/mechanics-kb.md):
    /// for an OMOD, classify its Data.Properties[] rows into
    /// direct-property/perk-grant/keyword-hook mechanisms; for a PERK/SPEL/
    /// ALCH/ENCH, walk its own Effects[] array directly. Either way, forward-
    /// or reverse-fetches whatever record carries the mechanic (including one
    /// extra hop through an MGEF's "Perk to Apply"/"Equip Ability") and emits
    /// a compact evidence tree.
    Chase {
        /// OMOD/PERK/SPEL/ALCH/ENCH FormID or EditorID (auto-detected).
        selector: String,
        /// Reverse-ref walk depth for keyword/AVIF consumer lookups
        /// (OMOD selectors only — ignored for PERK/SPEL/ALCH/ENCH).
        #[arg(long, default_value_t = esm::chase::DEFAULT_DEPTH)]
        depth: usize,
        /// Cap on refs rows fetched per record-type filter
        /// (OMOD selectors only — ignored for PERK/SPEL/ALCH/ENCH).
        #[arg(long = "ref-limit", default_value_t = esm::chase::DEFAULT_REF_LIMIT)]
        ref_limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Print one compact indented digest of a record and the chain it
    /// references, instead of a series of raw `get` dumps (native port of
    /// dps-76/scripts/esm-walk.ts). BFS out to `--depth` hops, annotating
    /// GLOB/keyword/AVIF/MGEF/PERK chains as it goes.
    Walk {
        /// FormID or EditorID (auto-detected).
        selector: String,
        /// BFS depth cap (0 = just the root, no chain-following).
        #[arg(long, default_value_t = esm::walk::DEFAULT_DEPTH)]
        depth: usize,
        /// Print the root record's grouped reverse-reference summary
        /// (obtainability signal) after the chain digest.
        #[arg(long)]
        refs: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print the embedded `esm-cli` usage-knowledge doc, or install it into a
    /// consumer repo's `.claude/skills/esm-cli/` for Claude Code to
    /// auto-discover. Takes no ESM path — like `daemon`, it is exempt from
    /// `--esm`/`FO76_ESM_PATH`.
    Skill {
        /// Write the doc to `<dir or cwd>/.claude/skills/esm-cli/SKILL.md`
        /// instead of printing it to stdout.
        #[arg(long)]
        install: bool,
        /// Target repo root for `--install` (defaults to the current directory).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Overwrite an existing installed copy.
        #[arg(long)]
        force: bool,
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
        /// FormID(s) and/or EditorID(s) (auto-detected per token); mix
        /// freely. A single target preserves the classic single-record
        /// output; two or more emit a JSON array. Overridden by
        /// --formid/--edid for the classic single-selector form.
        #[arg(conflicts_with_all = ["formid", "edid"])]
        targets: Vec<String>,
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
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
    },
    Diff {
        file_b: PathBuf,
        #[arg(long = "type")]
        record_type: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        /// Detail level for decoded fields attached to added/removed record stubs.
        #[arg(long, value_enum, default_value = "full")]
        bodies: BodiesArg,
        /// Keep noisy fields (placement transforms, CELL precombine bookkeeping,
        /// Object Bounds) instead of suppressing them from `changed` records.
        #[arg(long)]
        keep_noise: bool,
        /// Record-type signature(s) to omit entirely from added/removed/changed
        /// (repeatable and/or comma-delimited, e.g. `--exclude-type LAND,NAVM`).
        #[arg(long = "exclude-type", value_delimiter = ',')]
        exclude_type: Vec<String>,
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
        /// Narrow rows to referencing records of this 4-character type
        /// (e.g. `OMOD`); case-insensitive.
        #[arg(long = "type")]
        record_type: Option<String>,
        /// Annotate each row with the JSON field path(s) where it references
        /// its predecessor in the hop chain. Decodes every emitted row.
        #[arg(long)]
        paths: bool,
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

/// CLI-facing mirror of `esm::BodyDetail` for `--bodies <none|stub|full>`.
///
/// A separate type (rather than implementing `ValueEnum` on `BodyDetail`
/// itself) because `BodyDetail` lives in `diff.rs`, which this crate doesn't
/// own — clap's derive can't be added there without touching that file.
#[derive(Clone, Copy, ValueEnum)]
enum BodiesArg {
    None,
    Stub,
    Full,
}

impl From<BodiesArg> for BodyDetail {
    fn from(b: BodiesArg) -> Self {
        match b {
            BodiesArg::None => BodyDetail::None,
            BodiesArg::Stub => BodyDetail::Stub,
            BodiesArg::Full => BodyDetail::Full,
        }
    }
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

/// Resolves the ESM path from `--esm` (clap already applies the
/// `FO76_ESM_PATH` env fallback), erroring with a clear message if neither
/// was set.
fn resolve_esm(esm: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    esm.ok_or_else(|| anyhow::anyhow!("no ESM path — pass --esm <PATH> or set FO76_ESM_PATH"))
}

/// The `esm-cli` usage-knowledge skill doc, embedded at compile time (same
/// `include_str!` pattern as `schema/fo76.json` in `src/schema.rs`). `esm
/// skill` prints it verbatim; `esm skill --install` writes it into a
/// consumer repo's `.claude/skills/esm-cli/` for Claude Code to auto-discover.
const SKILL_MD: &str = include_str!("../../skills/esm-cli/SKILL.md");

/// Where `esm skill --install [--dir <DIR>]` writes the doc, relative to
/// `dir` (or the current directory when `dir` is `None` upstream).
fn skill_dest_path(dir: &Path) -> PathBuf {
    dir.join(".claude/skills/esm-cli/SKILL.md")
}

/// Pure overwrite-guard decision for `esm skill --install`: refuses to
/// clobber an existing install unless `--force` was passed. Split out from
/// `cmd_skill` so the decision is unit-testable without touching the
/// filesystem (precedent: `mmap_index_supports` above).
fn skill_install_allowed(dest_exists: bool, force: bool) -> Result<(), &'static str> {
    if dest_exists && !force {
        Err("destination already exists; pass --force to overwrite")
    } else {
        Ok(())
    }
}

fn cmd_skill(install: bool, dir: Option<PathBuf>, force: bool) -> anyhow::Result<()> {
    if !install {
        print!("{SKILL_MD}");
        return Ok(());
    }
    let base = dir.unwrap_or_else(|| PathBuf::from("."));
    let dest = skill_dest_path(&base);
    if let Err(msg) = skill_install_allowed(dest.exists(), force) {
        anyhow::bail!("{}: {msg}", dest.display());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&dest, SKILL_MD).with_context(|| format!("writing {}", dest.display()))?;
    println!("wrote {}", dest.display());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let esm_opt = cli.esm.clone();

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
                let mut status = remote.status()?;
                // Best-effort: annotate whether the resident daemon is still
                // running the binary it started with (see `daemon_fresh` in
                // `backend.rs`). A `false` here means a rebuild happened since
                // it started and the next `-p`/REPL call will respawn it.
                if let Ok(info) = read_daemon_info() {
                    if let Some(obj) = status.as_object_mut() {
                        obj.insert("binary_current".to_string(), daemon_fresh(&info).into());
                    }
                }
                println!("{}", serde_json::to_string_pretty(&status)?);
                Ok(())
            }
        };
    }

    // `skill` needs no ESM and no backend/daemon at all — handled up front,
    // same as `daemon` above, so it works with no --esm/FO76_ESM_PATH set.
    if let Some(Commands::Skill {
        install,
        dir,
        force,
    }) = cli.command
    {
        return cmd_skill(install, dir, force);
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
            Commands::Diff(args) => args.file_a.clone(),
            Commands::Daemon { .. } => unreachable!(),
            Commands::Skill { .. } => unreachable!(),
            _ => resolve_esm(esm_opt.clone())?,
        };
        match cmd {
            Commands::Info => cmd_info(&mut backend, &esm_for_repl)?,
            Commands::Get {
                targets,
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
                &esm_for_repl,
                formid,
                edid,
                targets,
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
                r#type,
                limit,
                json,
                pretty,
                localization_ba2,
                strings_dir,
                lang,
            } => cmd_list(
                &mut backend,
                &esm_for_repl,
                &r#type,
                limit,
                json,
                pretty,
                localization_ba2,
                strings_dir,
                &lang,
                daemon_mode,
            )?,
            Commands::Diff(args) => {
                let DiffArgs {
                    file_a,
                    file_b,
                    record_type,
                    json,
                    pretty,
                    bodies,
                    keep_noise,
                    exclude_type,
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
                } = *args;
                cmd_diff(
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
                    bodies.into(),
                    keep_noise,
                    exclude_type,
                    daemon_mode,
                )?
            }
            Commands::Tree {
                record_type,
                offset,
                limit,
                pretty,
            } => cmd_tree(
                &mut backend,
                &esm_for_repl,
                record_type.as_deref(),
                offset,
                limit,
                pretty,
            )?,
            Commands::Coverage {
                record_type,
                sample,
                json,
                gate,
            } => cmd_coverage(
                &mut backend,
                &esm_for_repl,
                record_type.as_deref(),
                sample,
                json,
                gate,
            )?,
            Commands::Refs {
                target,
                formid,
                edid,
                limit,
                depth,
                record_type,
                paths,
                json,
                pretty,
                localization_ba2,
                strings_dir,
                lang,
            } => cmd_refs(
                &mut backend,
                &esm_for_repl,
                formid,
                edid,
                target,
                limit,
                depth,
                record_type,
                paths,
                json,
                pretty,
                localization_ba2,
                strings_dir,
                &lang,
                daemon_mode,
            )?,
            Commands::Search {
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
                &esm_for_repl,
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
            Commands::Chase {
                selector,
                depth,
                ref_limit,
                json,
            } => cmd_chase(
                &mut backend,
                &esm_for_repl,
                &selector,
                depth,
                ref_limit,
                json,
            )?,
            Commands::Walk {
                selector,
                depth,
                refs,
                json,
            } => cmd_walk(&mut backend, &esm_for_repl, &selector, depth, refs, json)?,
            Commands::Daemon { .. } => unreachable!(),
            Commands::Skill { .. } => unreachable!(),
        }
        if !cli.print {
            return run_repl(&esm_for_repl, &mut backend);
        }
        return Ok(());
    }

    // No subcommand: pure REPL
    let esm = resolve_esm(esm_opt)?;
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
            targets,
            formid,
            edid,
            json,
            pretty,
            raw,
            resolve,
        } => cmd_get(
            backend, esm, formid, edid, targets, json, pretty, raw, None, None, "en", None,
            resolve, true,  // daemon_mode (REPL is always daemon-backed)
            false, // mmap_index (not applicable in REPL)
        ),
        ReplCommand::List {
            r#type,
            limit,
            json,
            pretty,
        } => cmd_list(
            backend, esm, &r#type, limit, json, pretty, None, None, "en", true,
        ),
        ReplCommand::Diff {
            file_b,
            record_type,
            json,
            pretty,
            bodies,
            keep_noise,
            exclude_type,
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
            bodies.into(),
            keep_noise,
            exclude_type,
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
            record_type,
            paths,
            json,
            pretty,
        } => cmd_refs(
            backend,
            esm,
            formid,
            edid,
            target,
            limit,
            depth,
            record_type,
            paths,
            json,
            pretty,
            None,
            None,
            "en",
            true,
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

fn parse_resolve(s: &str) -> anyhow::Result<ResolveDepth> {
    esm::query::resolve_depth(Some(s), ResolveDepth::None)
}

fn record_sel(
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
) -> anyhow::Result<RecordSel> {
    RecordSel::from_parts(formid.as_deref(), edid.as_deref(), target.as_deref())
}

/// Whether `--mmap-index` (lite mode: the mmap-only `.esm.midx` FormID index,
/// no full `.esm.idx` HashMap load) can serve this selector. Only a bare
/// `FormId` lookup works in lite mode — `Edid` needs the full EditorID index,
/// and `Auto`'s EditorID-fallback half is equally unavailable even though its
/// FormID half alone would work, so it's rejected the same as `Edid` rather
/// than silently only ever taking the FormID branch.
fn mmap_index_supports(sel: &RecordSel) -> bool {
    matches!(sel, RecordSel::FormId(_))
}

#[allow(clippy::too_many_arguments)]
fn cmd_get(
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
    mmap_index: bool,
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

    // ── mmap-index fast path (--local --mmap-index, FormID only) ─────────────
    // Loads the compact ~24 MiB .esm.midx instead of the full .esm.idx.
    // Only active in local mode (--local); ignored when hitting the daemon.
    if mmap_index && !daemon_mode && !has_overrides {
        let sel = record_sel(formid.clone(), edid.clone(), target.clone())?;
        if !mmap_index_supports(&sel) {
            anyhow::bail!(
                "--mmap-index only supports FormID lookups; \
                 for EditorID use the warm daemon (`esm daemon start`) \
                 or remove --mmap-index"
            );
        }
        let mut db = Database::open_lite(file)?;
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
fn cmd_list(
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
fn cmd_refs(
    backend: &mut Backend,
    file: &Path,
    formid: Option<String>,
    edid: Option<String>,
    target: Option<String>,
    limit: usize,
    depth: usize,
    record_type: Option<String>,
    paths: bool,
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
        let sel = record_sel(formid, edid, target)?;
        let op = Op::ReferencedBy {
            sel,
            limit,
            depth,
            type_filter: record_type,
            paths,
        };
        let v = esm::ipc::dispatch_op(&mut db, &op)?;
        let ref_list: RefList = serde_json::from_value(v)?;
        print_refs(&ref_list, json, pretty);
        return Ok(());
    }
    let sel = record_sel(formid, edid, target)?;
    let v = backend.run(
        file,
        Op::ReferencedBy {
            sel,
            limit,
            depth,
            type_filter: record_type,
            paths,
        },
    )?;
    let ref_list: RefList = serde_json::from_value(v)?;
    print_refs(&ref_list, json, pretty);
    Ok(())
}

fn print_refs(ref_list: &RefList, json: bool, pretty: bool) {
    if json {
        print_json(&serde_json::to_value(&ref_list.rows).unwrap(), pretty);
    } else {
        if ref_list.rows.is_empty() {
            eprintln!("note: no records reference {}", ref_list.target);
        } else {
            // Include a VIA column only when at least one row has a multi-hop path,
            // and a PATHS column only when --paths was requested (field_paths is
            // Some(...) on every row in that case, even if the inner Vec is empty).
            let has_via = ref_list.rows.iter().any(|r| !r.path.is_empty());
            let has_paths = ref_list.rows.iter().any(|r| r.field_paths.is_some());
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
                    if has_via {
                        let via = if !row.path.is_empty() {
                            let chain: Vec<_> =
                                row.path.iter().map(|n| n.form_id.as_str()).collect();
                            chain.join(" → ")
                        } else {
                            String::new()
                        };
                        cells.push(via);
                    }
                    if has_paths {
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
            if has_via {
                headers.push("VIA");
            }
            if has_paths {
                headers.push("PATHS");
            }
            print_record_table(&headers, &table_rows);
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

/// [`esm::chase::ChaseFetcher`] implementation that composes `chase`'s two
/// primitive ops (`Op::RecordBulk`, `Op::ReferencedBy`) over an existing
/// `Backend` — the warm daemon for free when `-p`/REPL mode is in play, a
/// cold in-process open under `--local`. Holds the `&Path` to the ESM being
/// queried so `esm::chase::chase` itself only deals in selectors/FormIDs.
struct BackendFetcher<'a> {
    backend: &'a mut Backend,
    file: &'a Path,
}

impl esm::chase::ChaseFetcher for BackendFetcher<'_> {
    fn bulk_get(
        &mut self,
        sels: &[RecordSel],
        depth: ResolveDepth,
    ) -> anyhow::Result<Vec<esm::BulkRecordEntry>> {
        let v = self.backend.run(
            self.file,
            Op::RecordBulk {
                sels: sels.to_vec(),
                depth,
            },
        )?;
        Ok(serde_json::from_value(v)?)
    }

    fn refs(
        &mut self,
        target: esm::FormId,
        depth: usize,
        limit: usize,
        type_filter: &str,
        paths: bool,
    ) -> anyhow::Result<RefList> {
        let v = self.backend.run(
            self.file,
            Op::ReferencedBy {
                sel: RecordSel::FormId(target),
                limit,
                depth,
                type_filter: Some(type_filter.to_string()),
                paths,
            },
        )?;
        Ok(serde_json::from_value(v)?)
    }
}

fn cmd_chase(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: usize,
    ref_limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input(selector)?;
    let opts = esm::chase::ChaseOptions { depth, ref_limit };
    let mut fetcher = BackendFetcher { backend, file };
    let tree = esm::chase::chase(&mut fetcher, sel, &opts)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&tree)?);
    } else {
        println!("{}", esm::chase::render_text(&tree));
    }
    Ok(())
}

/// Beyond `esm::walk::walk`'s two `ChaseFetcher` primitives (bulk_get/refs
/// with a mandatory type filter), this driver makes two more raw `Backend`
/// calls neither fits through that seam — see `esm::walk`'s module docs:
/// - not-found → `Op::Search` (fills in `WalkResult::not_found.matches`).
/// - `--refs` → one *unfiltered* `Op::ReferencedBy` (every referencing record
///   type, not just SPEL/PERK), reduced client-side by
///   `esm::walk::build_refs_digest`.
fn cmd_walk(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: usize,
    want_refs: bool,
    json: bool,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input(selector)?;
    let opts = esm::walk::WalkOptions { depth };
    let mut fetcher = BackendFetcher { backend, file };
    let mut result = esm::walk::walk(&mut fetcher, sel, &opts)?;

    if let Some(nf) = result.not_found.as_mut() {
        let v = fetcher.backend.run(
            fetcher.file,
            Op::Search {
                pattern: nf.target.clone(),
                types: Vec::new(),
                field: SearchField::Both,
                limit: 10,
            },
        )?;
        nf.matches = serde_json::from_value(v)?;
    } else if want_refs {
        if let Some(root) = result.nodes.first() {
            let root_fid = esm::parse_form_id_input(&root.formid)?;
            let v = fetcher.backend.run(
                fetcher.file,
                Op::ReferencedBy {
                    sel: RecordSel::FormId(root_fid),
                    limit: 0,
                    depth: 1,
                    type_filter: None,
                    paths: false,
                },
            )?;
            let ref_list: RefList = serde_json::from_value(v)?;
            result.refs = Some(esm::walk::build_refs_digest(&ref_list.rows));
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", esm::walk::render_text(&result));
    }
    Ok(())
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

fn print_record_table(headers: &[&str], rows: &[Vec<String>]) {
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
/// the "capped" stderr note.
fn print_record_rows(rows: &[RecordRow], limit: usize, json: bool, pretty: bool) {
    let capped = limit > 0 && rows.len() == limit;
    if json {
        print_json(&serde_json::to_value(rows).unwrap(), pretty);
    } else {
        let table_rows: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                vec![
                    r.form_id.clone(),
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

fn print_search_results(results: &[RecordRow], limit: usize, json: bool, pretty: bool) {
    print_record_rows(results, limit, json, pretty);
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
    let strategy = array_diff
        .get("strategy")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let count_from = array_diff.get("count_from").and_then(Value::as_u64);
    let count_to = array_diff.get("count_to").and_then(Value::as_u64);
    let added = array_diff.get("added").and_then(Value::as_array);
    let removed = array_diff.get("removed").and_then(Value::as_array);
    let changed = array_diff.get("changed").and_then(Value::as_array);

    let added_count = added.map(Vec::len).unwrap_or(0);
    let removed_count = removed.map(Vec::len).unwrap_or(0);
    let changed_count = changed.map(Vec::len).unwrap_or(0);

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
    let summary = if buckets.is_empty() {
        "no changes".to_string()
    } else {
        buckets.join(" ")
    };

    let strategy_desc = match strategy {
        "keyed" => {
            let key_fields: Vec<String> = array_diff
                .get("key_fields")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if key_fields.is_empty() {
                "keyed".to_string()
            } else {
                format!("keyed by {}", key_fields.join(", "))
            }
        }
        "positional" => "positional".to_string(),
        "set" => "set".to_string(),
        other => other.to_string(),
    };

    match (count_from, count_to) {
        (Some(from), Some(to)) => {
            println!("{indent}  {field}: {summary} entries ({from} \u{2192} {to}, {strategy_desc})")
        }
        _ => println!("{indent}  {field}: {summary} entries ({strategy_desc})"),
    }

    let elem_indent = format!("{indent}    ");
    if let Some(added) = added {
        for elem in added {
            println!("{elem_indent}+ {}", compact_value(elem));
        }
    }
    if let Some(removed) = removed {
        for elem in removed {
            println!("{elem_indent}- {}", compact_value(elem));
        }
    }
    if let Some(changed) = changed {
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

    // Gate on decode/schema coverage only — not `unresolved`, which indicates
    // missing localization BA2 strings rather than a decode failure.
    if gate {
        let totals = &report.totals;
        let mut failures = Vec::new();
        if totals.raw_fallback > 0 {
            failures.push(format!("{} raw_fallback", totals.raw_fallback));
        }
        if totals.unmapped > 0 {
            failures.push(format!("{} unmapped", totals.unmapped));
        }
        if totals.unknown_record > 0 {
            failures.push(format!("{} unknown_record", totals.unknown_record));
        }
        if !failures.is_empty() {
            anyhow::bail!("gate check failed: {}", failures.join(", "));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--mmap-index` (lite mode) only has a FormID index — `Edid` and the
    /// EditorID-fallback half of `Auto` both require the full index and must
    /// be rejected, exactly like plain `Edid` selectors already are.
    #[test]
    fn mmap_index_supports_formid_only() {
        assert!(mmap_index_supports(&RecordSel::FormId(esm::FormId(1))));
        assert!(!mmap_index_supports(&RecordSel::Edid("Foo".to_string())));
        assert!(!mmap_index_supports(&RecordSel::Auto("18000".to_string())));
    }

    /// `esm skill --install` writes to `<dir>/.claude/skills/esm-cli/SKILL.md`.
    #[test]
    fn skill_dest_path_is_under_dot_claude_skills() {
        assert_eq!(
            skill_dest_path(Path::new("/repo")),
            PathBuf::from("/repo/.claude/skills/esm-cli/SKILL.md")
        );
        assert_eq!(
            skill_dest_path(Path::new(".")),
            PathBuf::from("./.claude/skills/esm-cli/SKILL.md")
        );
    }

    /// The overwrite guard only blocks an existing destination without `--force`.
    #[test]
    fn skill_install_allowed_guards_existing_without_force() {
        assert!(skill_install_allowed(false, false).is_ok());
        assert!(skill_install_allowed(false, true).is_ok());
        assert!(skill_install_allowed(true, true).is_ok());
        assert!(skill_install_allowed(true, false).is_err());
    }

    /// The embedded doc is non-empty and starts with the expected frontmatter,
    /// so `esm skill`/`esm skill --install` never ship a stale/empty file.
    #[test]
    fn skill_md_has_frontmatter() {
        assert!(SKILL_MD.starts_with("---\nname: esm-cli"));
    }
}
