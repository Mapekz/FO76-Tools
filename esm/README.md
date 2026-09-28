# esm — FO76 ESM Reader

A Rust crate (plus its N-API addon) for reading and inspecting Fallout 76 `.esm` plugin/master files. Parses the Bethesda binary record format, schema-decodes 183 record types into structured JSON, indexes records by FormID and EditorID, resolves FormID references, loads localized string tables, evaluates curve tables, and supports search, diff, tree browsing, mechanics digests, and schema coverage auditing.

> **Read-only.** This tool never modifies your `.esm` files. The only files it writes live in a shared sidecar directory next to the ESM, `esm_cache/`, holding zero-copy rkyv cache sections per ESM (`<name>.esm.tree`, `.forms`, `.edid`, `.search`, `.xref`, `.lstrings`, `.curves`) — see [Cache](#cache) below. Game data files (`*.esm`, `*.ba2`, and `esm_cache/`) are gitignored and non-redistributable — obtain them from your own game install.

## Layout

```
esm/
  src/             Engine library + the `esm` CLI
  bindings/napi/   N-API addon (esm-napi) for Electron/Node.js
  schema/          fo76.json (183 record types, embedded at compile time)
  tools/           Schema extractor (xEdit Pascal → JSON), parity audit, curve lookup
```

The Electron GUI ("FO76 ESM Viewer") that consumes the N-API addon lives in the sibling
[`../esm-viewer/`](../esm-viewer/) directory, not in this crate.

## Requirements

- Toolchain pinned by the repo-root `rust-toolchain.toml` (rustup installs it automatically).
- Edition **2024**.
- `rust-version` in `Cargo.toml` tracks the pinned toolchain rather than the true language
  floor. Edition 2024 selects Cargo's MSRV-aware dependency resolver, which treats `rust-version` as
  a ceiling on dependency selection — a lower value would silently hold dependencies back at older
  releases. This crate has no external consumers, so there is nothing to gain from a low MSRV. The
  `bindings/napi` member declares the same value.

## Build

```sh
cargo build --release -p esm   # builds ../target/release/esm (repo-root workspace target/)
cargo test -p esm -p esm-napi  # every test; game-data tests skip without their env vars
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

A bare (no `0x` prefix) FormID token is always read as hex, never decimal — `esm get 00568635`
means `0x00568635`, not decimal 568635. If that hex reading has no record, resolution falls
straight to EditorID; there is no implicit decimal fallback. The global `--decimal` flag is the
explicit override for the rare case where the decimal reading is the one you actually want: a
bare digit token is read as decimal instead (hex is never attempted), and identity FormIDs in
output (the FORMID column, `get`'s header, `refs`/`diff`/`tree`/`walk` stubs) render as decimal —
FormIDs inside decoded field bodies stay hex either way, and `chase`'s JSON is exempt (a machine
pipeline contract requiring literal `0x########`).

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
| `curve <target>...` | Ad-hoc lookup/sum over any Curve Table record's points (`--at X...`, `--sum FROM TO [--step N]`) |
| `cache status [--json]` | Inspect the on-disk index cache without opening the ESM |
| `cache build [--section S]`, `cache clear` | Build cache sections now, or delete them all |
| `batch` | Answer one JSON `{"esm", "op"}` request per stdin line, keeping databases open (for scripts) |
| `skill [--install [--target codex,claude]]` | Print the agent usage-knowledge doc, or install it into another repo's `.agents/skills/` (default) and/or `.claude/skills/` |

A bare positional `<target>` auto-detects FormID (`0x`-prefixed or bare hex — see `--decimal`
above for the decimal reading) vs EditorID; explicit `--formid`/`--edid` skip the ambiguity.
`--limit 0` means unlimited on `list`/`search`/`refs`.

For full per-flag depth, bulk-operation patterns, `refs` selector rules, and how to read a
`walk`/`chase` digest, run `esm skill` or see [`skills/esm-cli/SKILL.md`](skills/esm-cli/SKILL.md)
— the same document ships embedded in the binary for downstream agents.

### Performance

Every call opens the ESM in-process. Once the cache exists, opening maps its sections without
parsing anything, so a warm `esm get` takes a few milliseconds end to end. Scripts that make many
calls can keep one `esm batch` child instead of launching one process per call: it reads one
`{"esm": <path>, "op": {...}}` JSON request per stdin line, answers each with one
`{"status": "ok"|"err", ...}` line, and keeps each ESM open until stdin closes. The request and
response shapes are `Request`, `Op` and `Response` in `src/ops/mod.rs`.

## Library API

The `esm` crate exposes a `Database` facade for library consumers:

```rust
use esm::{Database, FormId, ResolveDepth};

let db = Database::open("path/to/data")?;  // data folder or explicit .esm file

// File metadata
let info = db.file_info()?;

// Fetch by EditorID or FormID (decoded JSON)
let record = db.record_by_edid_resolved("AssaultRifle", ResolveDepth::None)?;
let record = db.record_by_formid_resolved(FormId::new(0x463F), ResolveDepth::Stub)?;

// List records of a type (0 = no limit)
let weapons = db.list_by_type("WEAP", 0)?;

// Reverse FormID lookup
let referencing = db.referenced_by(FormId::new(0x463F))?;

// Diff two databases
let diff = esm::diff::diff_databases(&db_a, &db_b)?;
```

Every query takes `&self`, so one `Database` can serve several threads. `esm::host::Host` keeps
several databases open by canonical path and runs `esm::ops::Op`s against them — the entry point the
CLI, `esm batch` and the N-API addon share.

## Schema

`schema/fo76.json` (2.3 MB) is embedded at compile time via `include_str!`. It covers all 183 FO76 record types — 182 derived from xEdit Pascal definitions plus `PGTR`, hand-authored whole because xEdit has no definition for it — and every type currently decodes `full` (no unmapped subrecords against the reference ESM); test coverage is 3 `robust` (hand-picked, end-to-end: `NPC_`, `PERK`, `WEAP`), 61 `basic`, and 119 `none` (still covered by the exhaustive env-gated sweep test). An `fo76.overrides.json` is merged on top for manual corrections (newer-than-reference drift subrecords TES5Edit doesn't define — see `AGENTS.md`'s "Coverage drift handling").

Decode status is measured against a reference ESM via `esm coverage`; run it (or `esm coverage --type <SIG>`) for live per-type status instead of a checked-in snapshot.

To regenerate or extend coverage:

```sh
# Requires a TES5Edit/FO76Edit checkout at ../TES5Edit
python3 tools/extractor/extract.py

# Audit schema parity against Pascal source (exits non-zero on HIGH drops)
python3 tools/extractor/audit.py --gate
```

## Tests

Integration test targets live in `tests/` (one per module) alongside inline `#[cfg(test)]` blocks for internals not public outside the crate. `tests/decode_records/` is a directory-backed target — one `main.rs` plus a module per record family — whose fixtures are verbatim subrecord bytes captured from `esm get --raw`, so it runs entirely in CI with no game data. Run all:

```sh
cargo test -p esm -p esm-napi

# Exhaustive decode sweep over CLEAN_TYPES (needs real ESM — skips silently if unset)
RUST_TEST_ESM=path/to/data cargo test

# Diff integration test (needs two ESM versions — skips silently if unset)
RUST_TEST_ESM_A=old.esm RUST_TEST_ESM_B=new.esm cargo test
```

## Cache

The cache is a set of independent, zero-copy [rkyv](https://rkyv.org/) sections, each its own
mmap'd file inside `esm_cache/` (one shared directory sibling to the ESM), read via
`rkyv::access_unchecked` rather than deserialized into heap maps. Four are built on
`Database::open` whenever missing or stale:

- **`.esm.forms`** (~200 MiB) — FormID→[`RecordMeta`] table plus the per-type FormID directory.
- **`.esm.tree`** (~140 MiB) — the GRUP structural tree (`tree` / `list-groups`).
- **`.esm.lstrings`** (~15 MiB) — the three localization string tables.
- **`.esm.curves`** (~1 MiB) — every CURV record's curve points.

Three are built on first use of the matching operation:

- **`.esm.edid`** (~15 MiB) — EditorID→FormID map (`--edid` lookups).
- **`.esm.search`** (~30 MiB) — FormID→name/description map (`search`).
- **`.esm.xref`** (~60 MiB) — FormID→referencing-FormIDs map (`refs`).

Every section carries its own header (magic, version, layout fingerprint, source ESM size+mtime)
validated before any bytes are trusted — a stale, foreign, or corrupt file degrades to "rebuild
that section," never a crash. `lstrings` and `curves` also record a stamp of the files they were
read from and rebuild when those change; a curve file rewritten in place inside an existing
`curvetables/json/` tree is not detected, so run `esm cache clear` after editing one.

Builds are shared across processes: one per-ESM build lock means concurrent callers build a
missing section once and the rest wait for it. The CLI runs each cold build as a detached
`esm cache build`, so the build finishes even if the command that started it is killed. Building
`xref` (a full schema decode of every record) takes about a minute; every builder publishes a
live heartbeat any process can read instantly via `esm cache status [--json]` — see
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
- [`AGENTS.md`](AGENTS.md) — conventions and invariants for agents working on this codebase.
- `esm skill` / [`skills/esm-cli/SKILL.md`](skills/esm-cli/SKILL.md) — usage knowledge for agents *using* the CLI (bulk workflows, `refs` gotchas, mechanics-digest reading, obtainability verdicts, curve-table conventions).
