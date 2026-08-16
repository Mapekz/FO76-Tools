#[path = "cli/progress_ui.rs"]
mod progress_ui;

use anyhow::Context as _;
use clap::{Args as _, Parser, Subcommand, ValueEnum};
use esm::backend::{
    CONNECT_TIMEOUT, DAEMON_FILENAME, HEALTH_POLL_INTERVAL, HEALTH_POLL_MAX, LocalBackend,
    OP_TIMEOUT_DEFAULT_SECS, RemoteBackend, daemon_fresh, read_daemon_info, start_daemon_process,
    stop_daemon,
};
use esm::ipc::{DEFAULT_MAX_DEPTH, Op, RecordSel, RefSort};
use esm::{
    BodyDetail, CacheInventory, CarrierKind, CoverageReport, Database, DiffOptions, DiffResult,
    FilterOp, FormId, Markers, RecordRow, RefList, ResolveDepth, SearchField,
};
use serde_json::Value;
use std::collections::BTreeMap;
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
    fn run(&mut self, esm: &Path, op: Op) -> anyhow::Result<Value> {
        let mut watched = vec![progress_watch_path(esm)];
        if let Op::Diff { b, .. } = &op {
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
/// filesystem.
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

/// One representative value per [`Op`] variant, purely so
/// [`op_wire_names`] has something to serialize and pattern-match against.
/// Field values are otherwise arbitrary/empty — nothing here is ever
/// dispatched.
fn sample_ops() -> Vec<Op> {
    vec![
        Op::FileInfo,
        Op::Record {
            sel: RecordSel::FormId(FormId::new(0)),
            depth: ResolveDepth::default(),
        },
        Op::RecordBulk {
            sels: vec![],
            depth: ResolveDepth::default(),
        },
        Op::RecordRaw {
            sel: RecordSel::FormId(FormId::new(0)),
        },
        Op::ListByType {
            sig: String::new(),
            limit: 0,
        },
        Op::ListTypeRecords {
            sig: String::new(),
            offset: 0,
            limit: 0,
        },
        Op::FilterTypeRecords {
            sig: String::new(),
            path: None,
            filter_op: FilterOp::Exists,
            value: None,
            limit: 0,
        },
        Op::ListTypeFieldPaths { sig: String::new() },
        Op::Search {
            pattern: String::new(),
            types: vec![],
            field: SearchField::Both,
            limit: 0,
        },
        Op::ReferencedBy {
            sel: RecordSel::FormId(FormId::new(0)),
            limit: 0,
            depth: 0,
            type_filter: None,
            paths: false,
            sort: RefSort::Formid,
        },
        Op::RefPath {
            from: RecordSel::FormId(FormId::new(0)),
            to: RecordSel::FormId(FormId::new(0)),
            max_hops: 0,
            paths: false,
        },
        Op::Walk {
            sel: RecordSel::FormId(FormId::new(0)),
            depth: 0,
            ref_limit: 0,
            level: 0.0,
            want_refs: false,
        },
        Op::Chase {
            sel: RecordSel::FormId(FormId::new(0)),
            depth: 0,
            ref_limit: 0,
        },
        Op::DropTable {
            sel: RecordSel::FormId(FormId::new(0)),
            level: 0.0,
            max_depth: 0,
            strict: false,
        },
        Op::ListGroups,
        Op::ListTypeChildren {
            sig: String::new(),
            offset: 0,
            limit: 0,
        },
        Op::ListGroupChildren {
            group_offset: 0,
            offset: 0,
            limit: 0,
        },
        Op::RecordStubAt { offset: 0 },
        Op::Coverage {
            record_type: None,
            sample: 0,
        },
        Op::Diff {
            b: PathBuf::new(),
            record_type: None,
            options: DiffOptions::default(),
        },
        Op::Shutdown,
    ]
}

/// Exists purely as a compile-time completeness guard on [`sample_ops`]:
/// this `match` has no wildcard arm, so adding, removing, or renaming an
/// [`Op`] variant fails to compile here until `sample_ops` (and this match)
/// are updated — `cmd_dump_wire_constants`'s `op_names` list can't
/// silently fall out of sync with the real enum. Never actually called for
/// its behavior (every arm is a no-op); `sample_ops`' construction is what
/// does the real work, this only proves it's exhaustive.
#[allow(dead_code)]
fn assert_op_variant_covered(op: &Op) {
    match op {
        Op::FileInfo => {}
        Op::Record { .. } => {}
        Op::RecordBulk { .. } => {}
        Op::RecordRaw { .. } => {}
        Op::ListByType { .. } => {}
        Op::ListTypeRecords { .. } => {}
        Op::FilterTypeRecords { .. } => {}
        Op::ListTypeFieldPaths { .. } => {}
        Op::Search { .. } => {}
        Op::ReferencedBy { .. } => {}
        Op::RefPath { .. } => {}
        Op::Walk { .. } => {}
        Op::Chase { .. } => {}
        Op::DropTable { .. } => {}
        Op::ListGroups => {}
        Op::ListTypeChildren { .. } => {}
        Op::ListGroupChildren { .. } => {}
        Op::RecordStubAt { .. } => {}
        Op::Coverage { .. } => {}
        Op::Diff { .. } => {}
        Op::Shutdown => {}
    }
}

/// The `op` wire-tag string (`#[serde(tag = "op", rename_all =
/// "snake_case")]`) for every [`Op`] variant, derived from a real
/// `serde_json::to_value` round-trip over [`sample_ops`] rather than
/// hand-typed — a rename that changes serde's actual output would change
/// this list too, not just the doc comment claiming it.
fn op_wire_names() -> Vec<String> {
    sample_ops()
        .iter()
        .map(|op| {
            assert_op_variant_covered(op);
            let value = serde_json::to_value(op).expect("Op always serializes");
            value
                .get("op")
                .and_then(|t| t.as_str())
                .expect("tagged Op enum always has an \"op\" field")
                .to_string()
        })
        .collect()
}

/// Long-form flag names `esm --local diff` accepts, introspected from
/// `DiffArgs`' own `clap::Args` impl (the same struct `Commands::Diff`
/// carries) rather than hand-typed — a renamed/added/removed `#[arg(...)]`
/// changes this list automatically, no separate string table to forget to
/// update.
fn diff_flag_names() -> Vec<String> {
    let cmd = DiffArgs::augment_args(clap::Command::new("diff"));
    let mut names: Vec<String> = cmd
        .get_arguments()
        .filter_map(|a| a.get_long().map(|s| format!("--{s}")))
        .collect();
    names.sort();
    names
}

/// `esm dump-wire-constants` — prints the Rust-side facts
/// `tools/esm_gateway.py` hand-mirrors as JSON. See that module's docstring
/// and `tools/regen_wire_constants.py`, which consumes this to (re)write
/// the checked-in `tools/wire_constants.py`.
fn cmd_dump_wire_constants() -> anyhow::Result<()> {
    // Worked examples of FormId::display()'s "0x{:08X}" format, keyed by the
    // raw u32 (as a decimal string, since JSON object keys must be strings)
    // -- lets the Python side assert its own hex formatting against real
    // Rust output instead of just trusting the doc comment describing it.
    let mut form_id_display_examples = serde_json::Map::new();
    for raw in [0u32, 0x0000463F, 0x00ABCDEF, 0xFFFFFFFF] {
        form_id_display_examples.insert(raw.to_string(), FormId::new(raw).display().into());
    }

    let out = serde_json::json!({
        "daemon_filename": DAEMON_FILENAME,
        "connect_timeout_secs": CONNECT_TIMEOUT.as_secs_f64(),
        "health_poll_interval_secs": HEALTH_POLL_INTERVAL.as_secs_f64(),
        "health_poll_max_secs": HEALTH_POLL_MAX.as_secs_f64(),
        "op_timeout_secs": OP_TIMEOUT_DEFAULT_SECS as f64,
        "default_max_depth": DEFAULT_MAX_DEPTH,
        "op_names": op_wire_names(),
        "form_id_display_examples": form_id_display_examples,
        "diff_flags": diff_flag_names(),
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// One-word summary of `esm cache status`'s overall state — the four
/// states called out in the design: no cache at all, a build in flight
/// (regardless of how much is already on disk), fully built, or partially
/// built (the common steady state once a build has run but `xref`, say,
/// has never been triggered).
fn cache_state_label(inventory: &CacheInventory, building: bool) -> &'static str {
    if building {
        "building"
    } else if inventory.is_empty() {
        "empty"
    } else if inventory.is_complete() {
        "complete"
    } else {
        "partial"
    }
}

fn cmd_cache_status(esm: &Path, as_json: bool) -> anyhow::Result<()> {
    let inventory = esm::cache_inventory(esm)?;
    let building = esm::progress::read(esm);
    let state = cache_state_label(&inventory, building.is_some());

    if as_json {
        let sections: BTreeMap<&str, bool> = esm::progress::BuildStage::ALL
            .iter()
            .map(|s| (s.label(), inventory.present.contains(s)))
            .collect();
        let build = building.as_ref().map(|p| {
            serde_json::json!({
                "pid": p.pid,
                "stage": p.stage.label(),
                "stage_index": p.stage_index,
                "stage_count": p.stage_count,
                "percent": p.percent(),
                "done": p.done,
                "total": p.total,
                "eta_secs": p.eta().map(|d| d.as_secs()),
            })
        });
        print_json(
            &serde_json::json!({
                "esm": esm,
                "state": state,
                "sections": sections,
                "build": build,
            }),
            true,
        );
        return Ok(());
    }

    println!("{}: {state}", esm.display());
    if let Some(p) = &building {
        println!("  {}", progress_ui::format_stage_summary(p));
    }
    print!("  sections:");
    for stage in esm::progress::BuildStage::ALL {
        let mark = if inventory.present.contains(&stage) {
            "+"
        } else {
            "-"
        };
        print!(" {mark}{}", stage.label());
    }
    println!();
    Ok(())
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
                // it started and the next call will respawn it.
                if let Ok(info) = read_daemon_info()
                    && let Some(obj) = status.as_object_mut()
                {
                    obj.insert("binary_current".to_string(), daemon_fresh(&info).into());
                }
                println!("{}", serde_json::to_string_pretty(&status)?);
                Ok(())
            }
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
        return cmd_skill(install, dir, force);
    }

    // `dump-wire-constants` needs no ESM either — same exemption as `skill`.
    if let Commands::DumpWireConstants = cli.command {
        return cmd_dump_wire_constants();
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
            CacheAction::Status { json } => cmd_cache_status(&esm, json),
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
        Commands::Info => cmd_info(backend, esm),
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
        } => cmd_get(
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
        } => cmd_list(
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
            cmd_diff(
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
        } => cmd_tree(backend, esm, record_type.as_deref(), offset, limit, pretty),
        Commands::Coverage {
            record_type,
            sample,
            json,
            gate,
        } => cmd_coverage(backend, esm, record_type.as_deref(), sample, json, gate),
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
                cmd_ref_path(
                    backend, esm, formid, edid, target, to, max_hops, paths, json, pretty,
                )
            } else {
                cmd_refs(
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
        } => cmd_search(
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
        } => cmd_chase(backend, esm, &selector, depth, ref_limit),
        Commands::Walk {
            selector,
            depth,
            ref_limit,
            level,
            refs,
            json,
        } => cmd_walk(backend, esm, &selector, depth, ref_limit, level, refs, json),
        Commands::Daemon { .. } => unreachable!(),
        Commands::Skill { .. } => unreachable!(),
        Commands::Cache { .. } => unreachable!(),
        Commands::DumpWireConstants => unreachable!(),
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

/// Bails when source-override flags were passed while daemon mode is active.
///
/// Source-override flags (`--localization-ba2`/`--strings-dir`/`--startup-ba2`/
/// `--curves-dir`, including `diff`'s per-side `_a`/`_b` variants) are
/// CLI-only — see `docs/adr/0008-source-overrides-cli-only.md`. The
/// daemon's `Registry` caches exactly one warm `Database` per canonical ESM
/// path, shared across every client; a per-request source override can't be
/// warmed into that shared cache, so there is no `Op` this could ever dispatch
/// to. Each call site computes `has_overrides` itself — a single flag-presence
/// check for `list`/`get`/`search`, an 8-way `_a`/`_b` coalesce for `diff` — and
/// passes the flag names to mention in the error.
fn bail_if_daemon_mode_overrides(
    has_overrides: bool,
    daemon_mode: bool,
    flags: &str,
) -> anyhow::Result<()> {
    if has_overrides && daemon_mode {
        anyhow::bail!(
            "{flags} are not supported in daemon mode; use --local to open the ESM directly"
        );
    }
    Ok(())
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
fn cmd_refs(
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
fn cmd_ref_path(
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

/// `chase` is JSON-only — a pipeline evidence contract, not something meant
/// to be read directly (see `esm::chase`'s module docs and `docs/adr/0001`).
/// The classifier itself runs server-side (`Op::Chase`, dispatched inside the
/// daemon or `--local`'s in-process `Database` — see `esm::ipc::dispatch_op`);
/// this is now just one wire call and a pretty-print.
fn cmd_chase(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: usize,
    ref_limit: usize,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input(selector)?;
    let v = backend.run(
        file,
        Op::Chase {
            sel,
            depth,
            ref_limit,
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// Interactive digest driver. The BFS, per-node digest computation, the
/// not-found search fallback, and the `--refs` reverse-reference summary all
/// run server-side in one `Op::Walk` call (`esm::ipc::dispatch_op`) — this
/// only resolves the CLI's own flags into the request and renders the
/// result, matching `--json` vs plain text either way (see `esm::walk`'s
/// module docs: only the *computation* moved server-side, `render.rs` is
/// still the sole place a `Digest`/`WalkResult` becomes text, so `--local`
/// and daemon output stay byte-identical).
#[allow(clippy::too_many_arguments)]
fn cmd_walk(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: usize,
    ref_limit: usize,
    level: f32,
    want_refs: bool,
    json: bool,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input(selector)?;
    let v = backend.run(
        file,
        Op::Walk {
            sel,
            depth,
            ref_limit,
            level,
            want_refs,
        },
    )?;
    let result: esm::walk::WalkResult = serde_json::from_value(v)?;

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
        bail_if_daemon_mode_overrides(
            force_local,
            daemon_mode,
            "--localization-ba2*/--strings-dir*/--startup-ba2*/--curves-dir*",
        )?;

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

/// Typed form of an `_array_diff` envelope's `"strategy"` field (see
/// `diff.rs`'s `array_diff`/`unkeyed_array_diff`/`keyed_array_diff` etc. for
/// the four cases this crate's diff pipeline actually produces — `keyed`,
/// `positional`, `set`, `unkeyed`, per `esm/CLAUDE.md`'s `diff.rs` entry).
/// `diff.rs` itself never keeps a Rust-side enum for this — every strategy
/// is written straight to an untyped `serde_json::Value` string at the point
/// it's decided, so this is CLI-local: a named, testable home for the
/// strategy dispatch, kept out of `print_array_diff` itself. `Other` covers
/// a future/unrecognized strategy string rather than panicking or dropping
/// it silently.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArrayDiffStrategy {
    Keyed { key_fields: Vec<String> },
    Positional,
    Set,
    Unkeyed,
    Other(String),
}

impl ArrayDiffStrategy {
    fn parse(array_diff: &serde_json::Map<String, Value>) -> Self {
        match array_diff.get("strategy").and_then(Value::as_str) {
            Some("keyed") => {
                let key_fields = array_diff
                    .get("key_fields")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                ArrayDiffStrategy::Keyed { key_fields }
            }
            Some("positional") => ArrayDiffStrategy::Positional,
            Some("set") => ArrayDiffStrategy::Set,
            Some("unkeyed") => ArrayDiffStrategy::Unkeyed,
            Some(other) => ArrayDiffStrategy::Other(other.to_string()),
            None => ArrayDiffStrategy::Other("?".to_string()),
        }
    }

    /// Human-readable description used in the summary line's parenthetical,
    /// e.g. "keyed by Reference, Minimum Level" / "positional" / "unkeyed".
    fn describe(&self) -> String {
        match self {
            ArrayDiffStrategy::Keyed { key_fields } if !key_fields.is_empty() => {
                format!("keyed by {}", key_fields.join(", "))
            }
            ArrayDiffStrategy::Keyed { .. } => "keyed".to_string(),
            ArrayDiffStrategy::Positional => "positional".to_string(),
            ArrayDiffStrategy::Set => "set".to_string(),
            ArrayDiffStrategy::Unkeyed => "unkeyed".to_string(),
            ArrayDiffStrategy::Other(s) => s.clone(),
        }
    }
}

/// The decided contents of an `_array_diff` envelope's one-line summary —
/// pulled out of `print_array_diff` so the bucket-counting/strategy-dispatch
/// logic is unit-testable without capturing stdout.
#[derive(Debug, PartialEq, Eq)]
struct ArrayDiffSummary {
    /// "+3 −1 ~2" (only non-zero buckets, space-joined) or "no changes".
    counts: String,
    strategy_desc: String,
    count_from: Option<u64>,
    count_to: Option<u64>,
}

fn summarize_array_diff(array_diff: &serde_json::Map<String, Value>) -> ArrayDiffSummary {
    let added_count = array_diff
        .get("added")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let removed_count = array_diff
        .get("removed")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let changed_count = array_diff
        .get("changed")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

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
    let counts = if buckets.is_empty() {
        "no changes".to_string()
    } else {
        buckets.join(" ")
    };

    ArrayDiffSummary {
        counts,
        strategy_desc: ArrayDiffStrategy::parse(array_diff).describe(),
        count_from: array_diff.get("count_from").and_then(Value::as_u64),
        count_to: array_diff.get("count_to").and_then(Value::as_u64),
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
    let summary = summarize_array_diff(array_diff);

    match (summary.count_from, summary.count_to) {
        (Some(from), Some(to)) => println!(
            "{indent}  {field}: {} entries ({from} \u{2192} {to}, {})",
            summary.counts, summary.strategy_desc
        ),
        _ => println!(
            "{indent}  {field}: {} entries ({})",
            summary.counts, summary.strategy_desc
        ),
    }

    let elem_indent = format!("{indent}    ");
    if let Some(added) = array_diff.get("added").and_then(Value::as_array) {
        for elem in added {
            println!("{elem_indent}+ {}", compact_value(elem));
        }
    }
    if let Some(removed) = array_diff.get("removed").and_then(Value::as_array) {
        for elem in removed {
            println!("{elem_indent}- {}", compact_value(elem));
        }
    }
    if let Some(changed) = array_diff.get("changed").and_then(Value::as_array) {
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
    use esm::RefRow;

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

    #[test]
    fn cache_state_label_covers_all_four_states() {
        let empty = CacheInventory {
            present: vec![],
            missing: esm::progress::BuildStage::ALL.to_vec(),
        };
        assert_eq!(cache_state_label(&empty, false), "empty");
        assert_eq!(cache_state_label(&empty, true), "building");

        let partial = CacheInventory {
            present: vec![
                esm::progress::BuildStage::Forms,
                esm::progress::BuildStage::Tree,
            ],
            missing: vec![
                esm::progress::BuildStage::Edid,
                esm::progress::BuildStage::Search,
                esm::progress::BuildStage::Xref,
            ],
        };
        assert_eq!(cache_state_label(&partial, false), "partial");

        let complete = CacheInventory {
            present: esm::progress::BuildStage::ALL.to_vec(),
            missing: vec![],
        };
        assert_eq!(cache_state_label(&complete, false), "complete");
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

    // ── ArrayDiffStrategy / summarize_array_diff: the four `_array_diff`
    // strategies `diff.rs` produces ───────────────────────────────────────

    fn array_diff_obj(json: Value) -> serde_json::Map<String, Value> {
        json.as_object().unwrap().clone()
    }

    #[test]
    fn array_diff_strategy_keyed_with_key_fields() {
        let ad = array_diff_obj(serde_json::json!({
            "strategy": "keyed",
            "key_fields": ["Reference", "Minimum Level"],
        }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(
            strategy,
            ArrayDiffStrategy::Keyed {
                key_fields: vec!["Reference".to_string(), "Minimum Level".to_string()]
            }
        );
        assert_eq!(strategy.describe(), "keyed by Reference, Minimum Level");
    }

    #[test]
    fn array_diff_strategy_keyed_without_key_fields_describes_bare() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "keyed" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Keyed { key_fields: vec![] });
        assert_eq!(strategy.describe(), "keyed");
    }

    #[test]
    fn array_diff_strategy_positional() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "positional" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Positional);
        assert_eq!(strategy.describe(), "positional");
    }

    #[test]
    fn array_diff_strategy_set() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "set" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Set);
        assert_eq!(strategy.describe(), "set");
    }

    #[test]
    fn array_diff_strategy_unkeyed() {
        let ad = array_diff_obj(serde_json::json!({
            "strategy": "unkeyed",
            "count_from": 2,
            "count_to": 1,
        }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(strategy, ArrayDiffStrategy::Unkeyed);
        assert_eq!(strategy.describe(), "unkeyed");
    }

    #[test]
    fn array_diff_strategy_unrecognized_falls_back_to_raw_string() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "future_strategy" }));
        let strategy = ArrayDiffStrategy::parse(&ad);
        assert_eq!(
            strategy,
            ArrayDiffStrategy::Other("future_strategy".to_string())
        );
        assert_eq!(strategy.describe(), "future_strategy");
    }

    #[test]
    fn summarize_array_diff_reports_buckets_and_counts() {
        let ad = array_diff_obj(serde_json::json!({
            "strategy": "keyed",
            "key_fields": ["Reference"],
            "count_from": 12,
            "count_to": 13,
            "added": [{"x": 1}, {"x": 2}, {"x": 3}],
            "removed": [{"x": 4}],
            "changed": [
                {"key": {"Reference": "0x1"}, "changes": {}},
                {"key": {}, "changes": {}},
            ],
        }));
        let summary = summarize_array_diff(&ad);
        assert_eq!(summary.counts, "+3 \u{2212}1 ~2");
        assert_eq!(summary.strategy_desc, "keyed by Reference");
        assert_eq!(summary.count_from, Some(12));
        assert_eq!(summary.count_to, Some(13));
    }

    #[test]
    fn summarize_array_diff_no_changes_when_all_buckets_empty() {
        let ad = array_diff_obj(serde_json::json!({ "strategy": "set" }));
        let summary = summarize_array_diff(&ad);
        assert_eq!(summary.counts, "no changes");
        assert_eq!(summary.strategy_desc, "set");
        assert_eq!(summary.count_from, None);
        assert_eq!(summary.count_to, None);
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
        serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout was not one strict JSON value ({error}):\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
    }
}
