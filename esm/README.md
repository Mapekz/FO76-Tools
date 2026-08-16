# esm — FO76 ESM Reader

A Rust workspace for reading and inspecting Fallout 76 `.esm` plugin/master files. Parses the Bethesda binary record format, schema-decodes 181 record types into structured JSON, indexes records by FormID and EditorID, resolves FormID references, loads localized string tables, evaluates curve tables, and supports search, diff, tree browsing, mechanics digests, and schema coverage auditing.

> **Read-only.** This tool never modifies your `.esm` files. The only files it writes live in a shared sidecar directory next to the ESM, `esm_cache/`, holding five zero-copy rkyv sections per ESM (`<name>.esm.tree`, `<name>.esm.forms`, `<name>.esm.edid`, `<name>.esm.search`, `<name>.esm.xref`) — see [Index cache](#index-cache) below. Game data files (`*.esm`, `*.ba2`, and `esm_cache/`) are gitignored and non-redistributable — obtain them from your own game install.

## Workspace layout

```
esm/
  src/             Engine library + two binaries (esm CLI, esm-server)
  bindings/napi/   N-API addon (esm-napi) for Electron/Node.js
  schema/          fo76.json (181 record types, embedded at compile time)
  tools/           Schema extractor (xEdit Pascal → JSON) + patch-note scripts
  static/          Embedded HTML for the HTTP server UI
```

The Electron GUI ("FO76 ESM Viewer") that consumes the N-API addon lives in the sibling
[`../esm-viewer/`](../esm-viewer/) directory, not in this crate.

## Requirements

- Toolchain pinned to **Rust 1.97.1** via `rust-toolchain.toml` (rustup installs it automatically).
- Edition **2024**.
- `rust-version` in `Cargo.toml` tracks the pinned toolchain (**1.97**) rather than the true language
  floor. Edition 2024 selects Cargo's MSRV-aware dependency resolver, which treats `rust-version` as
  a ceiling on dependency selection — a lower value would silently hold dependencies back at older
  releases. This crate has no external consumers, so there is nothing to gain from a low MSRV. The
  `bindings/napi` member declares the same value.

## Build

```sh
cargo build --release          # esm CLI → target/release/esm
cargo build --release --features server  # also builds esm-server
cargo test                     # run all tests (~100 run; 2 env-gated ignored)
```

## Quickstart

```sh
esm --esm path/to/data get AssaultRifle --pretty
# equivalently, set it once for the session:
export FO76_ESM_PATH=path/to/data
esm get AssaultRifle --pretty
esm walk AssaultRifle          # interactive mechanics digest instead of a raw dump
```

Every subcommand takes its ESM path from `--esm` (a global flag — works before or after the
subcommand name) or, if omitted, from `FO76_ESM_PATH`. `diff` is the one exception — it always
takes two explicit positional paths (`esm diff <old> <new>`) and ignores `--esm`/`FO76_ESM_PATH`.

Pass either a `.esm` file or a data folder. When given a folder, the tool auto-discovers the
single `.esm` inside it, then looks for localization strings (`strings/<stem>_<locale>.strings`
or any `*localization*.ba2`) and curve tables (`misc/curvetables/json/` or any `*startup*.ba2`).
Override with `--localization-ba2`/`--strings-dir`/`--startup-ba2`/`--curves-dir` when the
auto-detected sources aren't what you want.

If a query has to wait on a cold cache build, it shows live progress on stderr and still returns
the real result once the cache is ready. Pass the global `--no-wait` flag to instead print the
in-flight build's status and exit immediately (status 75) — useful for scripts that would rather
retry later than block.

## CLI — `esm`

```sh
esm [--esm <ESM-or-folder>] <subcommand> [options] [...]
```

| Subcommand | What it does |
|---|---|
| `info` | TES4 header summary — version, record count, master dependencies |
| `get <target>` | Fetch one record by FormID/EditorID (`--raw`, `--resolve none\|stub\|full`) |
| `list --type <SIG>` | List records of a type |
| `search <pattern>` | Wildcard search over EditorIDs and names (`--in edid\|name\|both`) |
| `refs <target>` | Reverse FormID lookup — who references this record (`--depth`, `--ep`, `--prop`, `--paths`) |
| `tree` | Browse the GRUP hierarchy |
| `diff <old> <new>` | Compare two ESM versions; sparse `{from, to}` diff per changed record |
| `coverage --type <SIG>` | Schema decode audit; `--gate` exits non-zero on any raw fallback |
| `walk <target>` | Interactive per-record-type mechanics digest (OMOD chains, LVLI drop odds, …) |
| `chase <target>` | Machine-readable JSON mechanism classification (pipeline contract, not for reading by hand) |
| `daemon {start,stop,status}` | Manage the background warm daemon (see [Daemon](#daemon) below) |
| `cache status [--json]` | Inspect the on-disk index cache without opening the ESM |
| `skill [--install]` | Print (or install into another repo's `.claude/skills/`) the agent usage-knowledge doc |

A bare positional `<target>` auto-detects FormID (`0x`-prefixed, decimal, or bare hex) vs
EditorID; explicit `--formid`/`--edid` skip the ambiguity. `--limit 0` means unlimited on
`list`/`search`/`refs`.

For full per-flag depth, bulk-operation patterns, `refs` selector rules, and how to read a
`walk`/`chase` digest, run `esm skill` or see [`skills/esm-cli/SKILL.md`](skills/esm-cli/SKILL.md)
— the same document ships embedded in the binary for downstream agents.

### Daemon

The first `esm` call auto-spawns `esm-server` as a warm background daemon; every subsequent call
is a fast HTTP round-trip instead of a cold in-process open (`--local` forces cold, useful for
one-off debugging). It self-manages:

- **Auto-shuts-down** after 10 minutes idle (`ESM_DAEMON_IDLE_SECS=0` disables this).
- **Stale-evicts** and reopens when the ESM changes on disk.
- **Rebuild-evicts** when the `esm-server` binary itself changes (e.g. after `cargo build`) — no
  manual `daemon stop` needed after a rebuild.
- **Parallel-agent safe** — an advisory spawn-lock lets multiple concurrent callers share one
  instance without double-spawning.

`esm daemon status` reports whether it's running (and whether a rebuild has made it stale);
`esm daemon stop` shuts it down early.

## Server — `esm-server`

Feature-gated HTTP REST + MCP stdio server. Build with `--features server`:

```sh
cargo run --release --features server --bin esm-server -- path/to/data
cargo run --release --features server --bin esm-server -- path/to/data --compare path/to/prev --port 3000
cargo run --release --features server --bin esm-server -- path/to/data --mcp-stdio
```

HTTP routes: `GET /info`, `/records/{formid}`, `/records?edid=|type=&limit=`, `/groups`, `/groups/{sig}/children`, `/stub/{offset}`, `/diff`, `/health`. Serves an embedded HTML viewer at `/` and `/compare`.

MCP-stdio mode speaks JSON-RPC 2.0 over stdin/stdout, proxying the same warm daemon the CLI uses. Wire it into an AI client's MCP config — **do not commit** the config file, since it hardcodes a non-redistributable, machine-local ESM path:

```jsonc
// .mcp.json (gitignored — fill in your actual paths)
{
  "mcpServers": {
    "fo76-esm": {
      "command": "/path/to/esm-server",
      "args": ["--mcp-stdio", "/path/to/data"]
    }
  }
}
```

Nine read-only tools are exposed (`esm_file_info`, `esm_search`, `esm_get_record`,
`esm_list_groups`, `esm_list_records`, `esm_refs`, `esm_walk`, `esm_chase`,
`esm_lvli_drop_table`) — see `skills/esm-cli/SKILL.md`'s MCP section for the full per-tool
argument reference.

## Library API

The `esm` crate exposes a `Database` facade for library consumers:

```rust
use esm::{Database, FormId, ResolveDepth};

let db = Database::open("path/to/data")?;  // data folder or explicit .esm file

// File metadata
let info = db.file_info();

// Fetch by EditorID (decoded JSON)
let record = db.record_by_edid("AssaultRifle", ResolveDepth::None)?;

// Fetch by FormID
let record = db.record_by_formid(FormId(0x463F), ResolveDepth::Stub)?;

// List all records of a type
let weapons = db.list_by_type("WEAP")?;

// Reverse FormID lookup
let referencing = db.referenced_by(FormId(0x463F), 100)?;

// Diff two databases
use esm::diff::diff_databases;
let diff = diff_databases(&db_a, &db_b)?;
```

Key re-exports: `Database`, `FormId`, `ResolveDepth`, `DiffResult`, `RecordDiff`, `RecordResult`, `ListEntry`, `GroupNode`, `TreeIndex`, `DatabaseResolver`.

## Schema

`schema/fo76.json` (2.3 MB) is embedded at compile time via `include_str!`. It covers all 181 FO76 record types derived from xEdit Pascal definitions — every type currently decodes `full` (no unmapped subrecords against the reference ESM); test coverage is 3 `robust` (hand-picked, end-to-end: `NPC_`, `PERK`, `WEAP`), 59 `basic`, and 119 `none` (still covered by the exhaustive env-gated sweep test). An `fo76.overrides.json` is merged on top for manual corrections (newer-than-reference drift subrecords TES5Edit doesn't define — see `CLAUDE.md`'s "Coverage drift handling").

Decode status is measured against a reference ESM via `esm coverage`; run it (or `esm coverage --type <SIG>`) for live per-type status instead of a checked-in snapshot.

To regenerate or extend coverage:

```sh
# Requires a TES5Edit/FO76Edit checkout at ../TES5Edit
python3 tools/extractor/extract.py

# Audit schema parity against Pascal source (exits non-zero on HIGH drops)
python3 tools/extractor/audit.py --gate
```

## Tests

~100 tests across `tests/` (integration test files, one per module — `wildcard.rs`, `curves.rs`, `diff.rs`, `reader.rs`, `ipc.rs`, `decode_records.rs`, `decode_coverage.rs`) plus inline `#[cfg(test)]` blocks for `tree`/`decode` internals not public outside the crate. `tests/decode_records.rs` uses verbatim subrecord bytes captured from `esm get --raw`, so it runs entirely in CI with no game data. Run all:

```sh
cargo test

# Exhaustive decode sweep over CLEAN_TYPES (needs real ESM — skips silently if unset)
RUST_TEST_ESM=path/to/data cargo test

# Diff integration test (needs two ESM versions — skips silently if unset)
RUST_TEST_ESM_A=old.esm RUST_TEST_ESM_B=new.esm cargo test
```

## Index cache

`Index`'s cache is five independent, zero-copy [rkyv](https://rkyv.org/) sections, each its own mmap'd file inside `esm_cache/` (one shared directory sibling to the ESM), read via `rkyv::access_unchecked` rather than deserialized into heap HashMaps. Two are eager, built together on `Database::open` whenever either is missing or stale:

- **`.esm.forms`** (~200 MiB) — FormID→[`RecordMeta`] table plus the per-type FormID directory.
- **`.esm.tree`** (~140 MiB) — the GRUP structural tree (`tree` / `list-groups`).

Three are lazy, built on first use of the matching operation and persisted for later processes to reuse:

- **`.esm.edid`** (~15 MiB) — EditorID→FormID map (`--edid` lookups).
- **`.esm.search`** (~30 MiB) — FormID→name/description map (`search`).
- **`.esm.xref`** (~55 MiB) — FormID→referencing-FormIDs map (`refs`).

Every section carries its own header (magic, version, layout fingerprint, source ESM size+mtime)
validated before any bytes are trusted — a stale, foreign, or corrupt file degrades to "rebuild
that section," never a crash. Building the `xref` section from scratch (a full schema decode of
every record) can take tens of seconds to a couple of minutes on the full FO76 ESM; every builder
publishes a live heartbeat any process can read instantly via `esm cache status [--json]` — see
`docs/adr/0003-cache-build-progress-heartbeat.md`. The whole `esm_cache/` directory is gitignored.

## Electron GUI

The sibling `../esm-viewer/` directory (repo root, not inside `esm/`) contains the FO76 ESM Viewer, an Electron desktop application. It depends on the `bindings/napi/` N-API addon (`@fo76/esm-napi`) which must be compiled from Rust before the app can run.

### Building the native addon

Before running the Electron app for the first time, build the N-API addon:

```sh
cd bindings/napi
bun install
bun run build          # or: bun run build:debug for a debug build
```

This compiles the Rust library into `bindings/napi/esm-napi.<platform>.node` and is required before `bun install` / `bun run dev` in `../esm-viewer/`.

### Running the app

```sh
cd ../esm-viewer
bun install
bun run dev            # start in development mode
bun run build          # production build
```

## Further reading

- [`docs/architecture.md`](docs/architecture.md) — record read flow, index/cache lifecycle, process topology, feature-layer modules.
- [`docs/adr/`](docs/adr/) — design decisions and their rationale.
- [`CLAUDE.md`](CLAUDE.md) — conventions and invariants for agents working on this codebase.
- `esm skill` / [`skills/esm-cli/SKILL.md`](skills/esm-cli/SKILL.md) — usage knowledge for agents *using* the CLI (bulk workflows, `refs` gotchas, mechanics-digest reading, obtainability verdicts, curve-table conventions).
