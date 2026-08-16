mod daemon;
mod diff;
mod inspect;
mod output;
mod progress_ui;
mod query;
mod refs;
mod skill;
mod walk;
mod wire_constants;

use clap::{Parser, Subcommand, ValueEnum};
use esm::BodyDetail;
use esm::backend::{LocalBackend, RemoteBackend};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "esm", about = "Read and inspect Fallout 76 ESM files")]
#[command(subcommand_required = true, arg_required_else_help = true)]
struct Cli {
    /// Force in-process (cold) open, bypassing the daemon entirely.
    #[arg(long)]
    local: bool,
    #[arg(long)]
    addr: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    /// Path to the ESM file or its data folder. If omitted, falls back to the
    /// FO76_ESM_PATH environment variable. Applies to every subcommand except
    /// `diff` (which takes two explicit positionals), `daemon`, and `skill`
    /// (neither needs an ESM at all).
    #[arg(long, global = true, env = "FO76_ESM_PATH")]
    esm: Option<PathBuf>,
    /// If the index cache is already being built by another process, print
    /// its status and exit immediately (status 75) instead of waiting for
    /// it and then running this command's own query. Checked once, up
    /// front, against whatever build is in flight at that moment — it does
    /// not prevent this invocation's own query from triggering (and
    /// blocking on) a *fresh* cold build if none was already running.
    #[arg(long, global = true)]
    no_wait: bool,
    #[command(subcommand)]
    command: Commands,
}

/// Exit code for `--no-wait` bailing out because a build is in flight —
/// `EX_TEMPFAIL` (sysexits.h): the request is valid, but the resource
/// (a warm cache) isn't ready yet and retrying later is the right move.
const EXIT_BUILD_IN_PROGRESS: i32 = 75;

const DEFAULT_LANG: &str = "en";

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
    /// Object Bounds, and — when form_version differs across the two files —
    /// schema-gated and restamp-only appearances/disappearances, including
    /// PTS re-save noise like materialized PNAM chain links and zero-padded
    /// `_raw` growth) instead of suppressing them from `changed` records.
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
    #[arg(long, default_value = DEFAULT_LANG)]
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

#[derive(clap::Args)]
struct LocalizationArgs {
    #[arg(long = "localization-ba2", conflicts_with = "strings_dir")]
    localization_ba2: Option<PathBuf>,
    #[arg(long, conflicts_with = "localization_ba2")]
    strings_dir: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_LANG)]
    lang: String,
}

#[derive(clap::Args)]
struct GetSourceArgs {
    #[command(flatten)]
    localization: LocalizationArgs,
    #[arg(long)]
    startup_ba2: Option<PathBuf>,
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
        #[command(flatten)]
        sources: GetSourceArgs,
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
        #[command(flatten)]
        sources: LocalizationArgs,
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
        /// FormID, EditorID, or PERK entry-point name (auto-detected);
        /// overridden by --formid/--edid/--entry-point/--omod-property. An
        /// entry-point name only matches when it isn't also an EditorID —
        /// e.g. `Blocker01` (a record) always wins over any same-named entry
        /// point.
        #[arg(conflicts_with_all = ["formid", "edid", "entry_point", "omod_property"])]
        target: Option<String>,
        #[arg(long, conflicts_with_all = ["edid", "entry_point", "omod_property"])]
        formid: Option<String>,
        #[arg(long, conflicts_with_all = ["formid", "entry_point", "omod_property"])]
        edid: Option<String>,
        /// PERK "Entry Point" name or numeric id — resolves to every PERK
        /// carrying it (e.g. `39`, `'Mod Percent Blocked'`), each emitted as
        /// a depth-0 carrier row, then walks refs from all of them. Matching
        /// is case-insensitive exact unless the value contains `*`, which
        /// globs (e.g. `'Mod VATS*'`). Multi-match globs print a legend of
        /// every matched id+name and an `EP` column attributing each row to
        /// its entry point(s); `VIA` starts at depth 1 naming the carrier.
        #[arg(long = "entry-point", visible_alias = "ep", conflicts_with_all = ["formid", "edid", "omod_property"])]
        entry_point: Option<String>,
        /// OMOD property name or numeric id, optionally scoped by form-type
        /// (weap:/armo:/npc:) — resolves to every OMOD declaring it (e.g.
        /// `weap:Speed`, `weap:31`, or a bare `Keywords` spanning all three
        /// spaces), each emitted as a depth-0 carrier row, then walks refs from
        /// all of them. A bare numeric id with no scope prefix is rejected —
        /// ids are only meaningful within one form-type space. Name matching
        /// is case- and whitespace-insensitive (`ActorValues`/`Actor Values`
        /// both match); `*` globs. Multi-space bare-name matches print a
        /// legend of every matched scope+id+name and a `PROP` column
        /// attributing each row. Unlike --entry-point, never auto-detected
        /// from a bare positional target (property names are short/generic and
        /// collide with real EditorIDs — see docs/adr/0004).
        #[arg(long = "omod-property", visible_alias = "prop", conflicts_with_all = ["formid", "edid", "entry_point", "to"])]
        omod_property: Option<String>,
        /// FormID or EditorID of a second record — instead of walking the
        /// full reverse-reference graph, find one connecting chain of
        /// reverse-reference hops from the primary target to this one via a
        /// bidirectional search (meets in the middle, so it never
        /// materializes the full closure). Directional, matching `esm refs
        /// <target>` itself: this looks for a chain showing that --to
        /// transitively references the primary target, not the reverse — if
        /// no path is found, the two records may still be connected the
        /// other way (swap which one is the positional target vs --to).
        /// Incompatible with the closure-walk options
        /// (--depth/--limit/--type/--sort/--entry-point/--omod-property)
        /// since there's no "everything found" set to narrow or truncate —
        /// only the one chain (if any).
        #[arg(long, conflicts_with_all = ["entry_point", "omod_property", "limit", "depth", "record_type", "sort"])]
        to: Option<String>,
        /// Combined hop-count ceiling for --to's bidirectional search
        /// (default 12). Only meaningful with --to.
        #[arg(long, default_value_t = 0, requires = "to")]
        max_hops: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Reverse-reference walk depth (1 = direct refs only, up to 8;
        /// 0 = unbounded — no fixed hop cap. An unbounded walk over a
        /// hub-heavy graph like CELL/REFR can return hundreds of thousands
        /// of rows; combine with --limit 0 and a --type filter.
        #[arg(long, default_value_t = 1, value_parser = parse_ref_depth)]
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
        /// Row ordering before --limit truncation. `formid` (default) keeps
        /// today's FormID-ascending order; `depth` yields a breadth-first
        /// prefix under --limit instead of a FormID-lexical slice — use this
        /// when a low --limit might otherwise hide the deepest hops.
        #[arg(long, value_enum, default_value = "formid")]
        sort: RefSortArg,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pretty: bool,
        #[command(flatten)]
        sources: LocalizationArgs,
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
        #[command(flatten)]
        sources: LocalizationArgs,
    },
    /// Pipeline evidence contract: classified mechanism JSON for OMOD/PERK/
    /// SPEL/ALCH/ENCH roots (hard error on other types). For interactive
    /// reading use `walk`.
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
    },
    /// Interactive digest of any record and the chain it references — on
    /// OMOD roots, classifies and slices each mechanism inline
    /// (keyword/AVIF hooks resolved via reverse refs, bounded by
    /// --ref-limit).
    Walk {
        /// FormID or EditorID (auto-detected).
        selector: String,
        /// BFS depth cap (0 = just the root, no chain-following).
        #[arg(long, default_value_t = esm::walk::DEFAULT_DEPTH)]
        depth: usize,
        /// Cap on refs rows fetched per record-type filter for an OMOD
        /// root's keyword/AVIF mechanism consumer lookups.
        #[arg(long = "ref-limit", default_value_t = esm::chase::DEFAULT_REF_LIMIT)]
        ref_limit: usize,
        /// Player level assumed by an LVLI root's drop-odds digest (Curve
        /// Table evaluation, Minimum Level filtering). Ignored by every
        /// other record type.
        #[arg(long, default_value_t = esm::lvli::DEFAULT_LEVEL)]
        level: f32,
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
    /// Inspect the on-disk index cache directly — no daemon, no ESM open,
    /// never triggers a build. Takes an ESM path (unlike `daemon`/`skill`)
    /// but no backend: reads `esm_cache/`'s five section headers and the
    /// build lock/heartbeat straight off disk.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Print the Rust-side constants `tools/esm_gateway.py` hand-mirrors
    /// (timeouts, the `Op` discriminant strings, the FormId display format,
    /// the `diff` subcommand's flag names) as JSON. Takes no ESM path, like
    /// `daemon`/`skill`. Consumed only by `tools/regen_wire_constants.py`,
    /// which writes the checked-in `tools/wire_constants.py` module CI
    /// drift-guards against — see that script's docstring.
    #[command(hide = true)]
    DumpWireConstants,
}

#[derive(Subcommand)]
enum DaemonAction {
    Start,
    Stop,
    Status,
}

#[derive(Subcommand)]
enum CacheAction {
    /// Print which of the five index-cache sections are present, plus a
    /// live build's progress if one is currently in flight.
    Status {
        #[arg(long)]
        json: bool,
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

/// Validates `refs --depth` against `[0, DEFAULT_MAX_DEPTH]` — `usize`
/// doesn't get a ranged `clap::value_parser!`, so this rejects out-of-range
/// values explicitly rather than clamping them silently.
fn parse_ref_depth(s: &str) -> Result<usize, String> {
    let v: usize = s
        .parse()
        .map_err(|_| format!("invalid depth value '{s}'"))?;
    if v > esm::ipc::DEFAULT_MAX_DEPTH {
        Err(format!(
            "{v} is not in 0..={} (0 = unbounded)",
            esm::ipc::DEFAULT_MAX_DEPTH
        ))
    } else {
        Ok(v)
    }
}

/// CLI-facing mirror of `esm::ipc::RefSort` for `refs --sort <formid|depth>`.
#[derive(Clone, Copy, ValueEnum)]
enum RefSortArg {
    Formid,
    Depth,
}

impl From<RefSortArg> for esm::ipc::RefSort {
    fn from(s: RefSortArg) -> Self {
        match s {
            RefSortArg::Formid => esm::ipc::RefSort::Formid,
            RefSortArg::Depth => esm::ipc::RefSort::Depth,
        }
    }
}

/// CLI-side wrapper around `esm::backend::Backend` (the plain local/remote
/// dispatch enum): every real query goes through [`Self::run`], which wraps
/// the inner call with a [`progress_ui::Watcher`] — this is the one place
/// all ~15 `cmd_*` functions' `backend.run(...)` calls funnel through, and
/// `watcher.stop()` (which blocks until any rendered line is erased) runs
/// synchronously before this returns, so whichever `cmd_*` function is
/// about to `println!`/`print_json` its result never races a still-visible
/// progress line. See `progress_ui`'s module doc for why this site, not
/// `dispatch_command`, is the right one.
struct Backend(esm::backend::Backend);

impl Backend {
    fn run(&mut self, esm: &Path, op: esm::ipc::Op) -> anyhow::Result<serde_json::Value> {
        let mut watched = vec![progress_watch_path(esm)];
        if let esm::ipc::Op::Diff { b, .. } = &op {
            watched.push(progress_watch_path(b));
        }
        let watcher = progress_ui::Watcher::spawn(watched);
        let result = self.0.run(esm, op);
        watcher.stop();
        result
    }

    fn is_remote(&self) -> bool {
        matches!(self.0, esm::backend::Backend::Remote(_))
    }
}

fn make_backend(local: bool, addr: Option<&str>, port: Option<u16>) -> anyhow::Result<Backend> {
    if local {
        Ok(Backend(esm::backend::Backend::Local(LocalBackend::new())))
    } else {
        Ok(Backend(esm::backend::Backend::Remote(
            RemoteBackend::connect_with_override(addr, port)?,
        )))
    }
}

/// Resolves the ESM path from `--esm` (clap already applies the
/// `FO76_ESM_PATH` env fallback), erroring with a clear message if neither
/// was set.
fn resolve_esm(esm: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    esm.ok_or_else(|| anyhow::anyhow!("no ESM path — pass --esm <PATH> or set FO76_ESM_PATH"))
}

/// Best-effort canonical ESM path for progress-watching purposes only (the
/// [`progress_ui::Watcher`], `--no-wait`, `esm cache status`) — must match
/// whatever path `Registry::get_or_open_with_key` (daemon and `--local`
/// alike) actually canonicalizes to and keys `esm_cache/`'s sidecar files
/// off, via [`esm::discover::resolve_esm_path`]. This can differ from the
/// raw `--esm`/`file_b` input whenever it's a data folder, a relative path,
/// or a symlink — looking a build up by the raw input instead would poll a
/// location nothing ever writes to. Resolution failure (path doesn't exist
/// yet, a folder with zero/multiple `.esm` files, …) degrades to the raw
/// input rather than erroring here: the watcher then simply finds nothing to
/// show, and the real error (if any) surfaces from `backend.run` itself with
/// a clearer message than this helper could produce.
fn progress_watch_path(esm: &Path) -> PathBuf {
    esm::discover::resolve_esm_path(esm).unwrap_or_else(|_| esm.to_path_buf())
}

#[derive(Clone, Copy)]
struct DispatchOptions {
    daemon_mode: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let esm_opt = cli.esm.clone();

    if let Commands::Daemon { action } = cli.command {
        return match action {
            DaemonAction::Start => daemon::cmd_daemon_start(),
            DaemonAction::Stop => daemon::cmd_daemon_stop(),
            DaemonAction::Status => daemon::cmd_daemon_status(cli.addr.as_deref(), cli.port),
        };
    }

    // `skill` needs no ESM and no backend/daemon at all — handled up front,
    // same as `daemon` above, so it works with no --esm/FO76_ESM_PATH set.
    if let Commands::Skill {
        install,
        dir,
        force,
    } = cli.command
    {
        return skill::cmd_skill(install, dir, force);
    }

    // `dump-wire-constants` needs no ESM either — same exemption as `skill`.
    if let Commands::DumpWireConstants = cli.command {
        return wire_constants::cmd_dump_wire_constants();
    }

    // `cache status` reads `esm_cache/` and the build lock/heartbeat
    // straight off disk — it needs an ESM path (unlike `daemon`/`skill`)
    // but must never construct a `Backend` or contact the daemon, since the
    // whole point is answering instantly even while a build is in flight.
    if let Commands::Cache { action } = cli.command {
        let esm = resolve_esm(esm_opt.clone())?;
        // Resolve folder→ESM and canonicalize here (not `progress_watch_path`'s
        // silent-degrade variant): `cache_inventory`/`progress::read` must key
        // off the exact same path `Registry` does, and if that resolution
        // itself fails (bad path, ambiguous folder), that's a real error worth
        // surfacing rather than reporting a misleading "empty" status.
        let esm = esm::discover::resolve_esm_path(&esm)?;
        return match action {
            CacheAction::Status { json } => daemon::cmd_cache_status(&esm, json),
        };
    }

    // Every subcommand runs once and exits. Daemon-backed by default;
    // --local bypasses the daemon entirely (cold in-process open).
    let cmd = cli.command;
    let esm = match &cmd {
        Commands::Diff(args) => args.file_a.clone(),
        Commands::Daemon { .. } => unreachable!(),
        Commands::Skill { .. } => unreachable!(),
        Commands::Cache { .. } => unreachable!(),
        Commands::DumpWireConstants => unreachable!(),
        _ => resolve_esm(esm_opt.clone())?,
    };

    // `--no-wait` is checked client-side, purely from the build lock's
    // filesystem state, before a `Backend` is even constructed — routing it
    // through the daemon would defeat the point, since the daemon's own
    // per-ESM mutex is exactly what's held for the whole build (see
    // `esm::progress`'s module doc).
    if cli.no_wait {
        let mut watched = vec![progress_watch_path(&esm)];
        if let Commands::Diff(args) = &cmd {
            watched.push(progress_watch_path(&args.file_b));
        }
        if let Some(progress) = watched.iter().find_map(|p| esm::progress::read(p)) {
            eprintln!("esm: index cache is being built by pid {}", progress.pid);
            eprintln!("  {}", progress_ui::format_stage_summary(&progress));
            std::io::Write::flush(&mut std::io::stderr()).ok();
            std::process::exit(EXIT_BUILD_IN_PROGRESS);
        }
    }

    let mut backend = make_backend(cli.local, cli.addr.as_deref(), cli.port)?;
    let daemon_mode = backend.is_remote();
    dispatch_command(&esm, &mut backend, cmd, DispatchOptions { daemon_mode })
}

fn dispatch_command(
    esm: &Path,
    backend: &mut Backend,
    cmd: Commands,
    options: DispatchOptions,
) -> anyhow::Result<()> {
    match cmd {
        Commands::Info => inspect::cmd_info(backend, esm),
        Commands::Get {
            targets,
            formid,
            edid,
            json,
            pretty,
            raw,
            sources:
                GetSourceArgs {
                    localization:
                        LocalizationArgs {
                            localization_ba2,
                            strings_dir,
                            lang,
                        },
                    startup_ba2,
                },
            resolve,
        } => query::cmd_get(
            backend,
            esm,
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
            options.daemon_mode,
        ),
        Commands::List {
            r#type,
            limit,
            json,
            pretty,
            sources:
                LocalizationArgs {
                    localization_ba2,
                    strings_dir,
                    lang,
                },
        } => query::cmd_list(
            backend,
            esm,
            &r#type,
            limit,
            json,
            pretty,
            localization_ba2,
            strings_dir,
            &lang,
            options.daemon_mode,
        ),
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
            diff::cmd_diff(
                backend,
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
                options.daemon_mode,
            )
        }
        Commands::Tree {
            record_type,
            offset,
            limit,
            pretty,
        } => inspect::cmd_tree(backend, esm, record_type.as_deref(), offset, limit, pretty),
        Commands::Coverage {
            record_type,
            sample,
            json,
            gate,
        } => inspect::cmd_coverage(backend, esm, record_type.as_deref(), sample, json, gate),
        Commands::Refs {
            target,
            formid,
            edid,
            entry_point,
            omod_property,
            to,
            max_hops,
            limit,
            depth,
            record_type,
            paths,
            sort,
            json,
            pretty,
            sources:
                LocalizationArgs {
                    localization_ba2,
                    strings_dir,
                    lang,
                },
        } => {
            if let Some(to) = to {
                refs::cmd_ref_path(
                    backend, esm, formid, edid, target, to, max_hops, paths, json, pretty,
                )
            } else {
                refs::cmd_refs(
                    backend,
                    esm,
                    formid,
                    edid,
                    target,
                    entry_point,
                    omod_property,
                    limit,
                    depth,
                    record_type,
                    paths,
                    sort.into(),
                    json,
                    pretty,
                    localization_ba2,
                    strings_dir,
                    &lang,
                    options.daemon_mode,
                )
            }
        }
        Commands::Search {
            pattern,
            types,
            search_in,
            limit,
            json,
            pretty,
            sources:
                LocalizationArgs {
                    localization_ba2,
                    strings_dir,
                    lang,
                },
        } => query::cmd_search(
            backend,
            esm,
            &pattern,
            types,
            search_in,
            limit,
            json,
            pretty,
            localization_ba2,
            strings_dir,
            &lang,
            options.daemon_mode,
        ),
        Commands::Chase {
            selector,
            depth,
            ref_limit,
        } => walk::cmd_chase(backend, esm, &selector, depth, ref_limit),
        Commands::Walk {
            selector,
            depth,
            ref_limit,
            level,
            refs,
            json,
        } => walk::cmd_walk(backend, esm, &selector, depth, ref_limit, level, refs, json),
        Commands::Daemon { .. } => unreachable!(),
        Commands::Skill { .. } => unreachable!(),
        Commands::Cache { .. } => unreachable!(),
        Commands::DumpWireConstants => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_chase_and_walk() {
        let chase = Cli::try_parse_from(["esm", "chase", "0x463F"])
            .expect("chase should parse through the top-level command enum");
        assert!(matches!(chase.command, Commands::Chase { .. }));

        let walk = Cli::try_parse_from(["esm", "walk", "0x463F"])
            .expect("walk should parse through the top-level command enum");
        assert!(matches!(walk.command, Commands::Walk { .. }));
    }

    /// `--no-wait` is global (`--esm`'s existing precedent), defaults to
    /// `false`, and parses both before and after the subcommand.
    #[test]
    fn no_wait_is_a_global_flag() {
        let default = Cli::try_parse_from(["esm", "get", "0x463F"]).unwrap();
        assert!(!default.no_wait);

        let before = Cli::try_parse_from(["esm", "--no-wait", "get", "0x463F"]).unwrap();
        assert!(before.no_wait);

        let after = Cli::try_parse_from(["esm", "get", "0x463F", "--no-wait"]).unwrap();
        assert!(after.no_wait);
    }

    /// `esm cache status [--json]` parses through the top-level command
    /// enum and requires no ESM/backend at all to construct — it's a
    /// third `main()` short-circuit alongside `daemon`/`skill`.
    #[test]
    fn cache_status_parses() {
        let plain =
            Cli::try_parse_from(["esm", "cache", "status"]).expect("cache status should parse");
        assert!(matches!(
            plain.command,
            Commands::Cache {
                action: CacheAction::Status { json: false }
            }
        ));

        let json = Cli::try_parse_from(["esm", "cache", "status", "--json"])
            .expect("cache status --json should parse");
        assert!(matches!(
            json.command,
            Commands::Cache {
                action: CacheAction::Status { json: true }
            }
        ));
    }

    /// A missing subcommand is a usage error, never a silent no-op — the
    /// worst failure mode for a scripted caller would be exiting 0 with
    /// empty stdout.
    #[test]
    fn bare_invocation_is_a_usage_error_not_a_silent_no_op() {
        assert!(Cli::try_parse_from(["esm"]).is_err());
    }

    /// `-p`/`--print` is not a recognized flag — removed as dead surface
    /// area rather than left for callers to still reach for.
    #[test]
    fn dash_p_no_longer_exists() {
        let err = match Cli::try_parse_from(["esm", "-p", "get", "0x463F"]) {
            Ok(_) => panic!("-p should no longer parse"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn one_shot_json_stdout_is_exactly_one_document() {
        let Ok(esm_path) = std::env::var("RUST_TEST_ESM") else {
            return;
        };

        let binary = std::env::var_os("CARGO_BIN_EXE_esm")
            .map(PathBuf::from)
            .or_else(|| {
                let test_exe = std::env::current_exe().ok()?;
                let debug_dir = test_exe.parent()?.parent()?;
                let candidate = debug_dir.join(format!("esm{}", std::env::consts::EXE_SUFFIX));
                candidate.is_file().then_some(candidate)
            });
        let mut command = if let Some(binary) = binary {
            std::process::Command::new(binary)
        } else {
            let mut cargo = std::process::Command::new(env!("CARGO"));
            cargo
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .args(["run", "--quiet", "--bin", "esm", "--"]);
            cargo
        };

        let output = command
            .arg("--local")
            .arg("--esm")
            .arg(esm_path)
            .args(["get", "0x463F", "--json"])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run one-shot esm get");

        assert!(
            output.status.success(),
            "esm get failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout was not one strict JSON value ({error}):\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
    }
}
