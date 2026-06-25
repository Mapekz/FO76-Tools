# esm — FO76 ESM Reader

A Rust workspace for reading and inspecting Fallout 76 `.esm` plugin/master files. Parses the Bethesda binary record format, schema-decodes 173 record types into structured JSON, indexes records by FormID and EditorID, resolves FormID references, loads localized string tables, evaluates curve tables, and supports search, diff, tree browsing, and schema coverage auditing.

> **Read-only.** This tool never modifies your `.esm` files. The only file it writes is a sidecar `<name>.esm.idx` index cache (next to the ESM) to accelerate subsequent opens. Game data files (`*.esm`, `*.ba2`, `*.esm.idx`) are gitignored and non-redistributable — obtain them from your own game install.

## Workspace layout

```
esm/
  src/             Engine library + two binaries (esm CLI, esm-server)
  bindings/napi/   N-API addon (esm-napi) for Electron/Node.js
  app/             Electron GUI ("FO76 ESM Viewer")
  schema/          fo76.json (173 record types, embedded at compile time)
  tools/           Schema extractor (xEdit Pascal → JSON) + patch-note scripts
  static/          Embedded HTML for the HTTP server UI
  todos/           Deferred work backlog
```

## Requirements

- Toolchain pinned to **Rust 1.96.0** via `rust-toolchain.toml` (rustup installs it automatically).
- MSRV **1.82** (`Option::is_none_or`), declared as `rust-version` in `Cargo.toml`.

## Build

```sh
cargo build --release          # esm CLI → target/release/esm
cargo build --release --features server  # also builds esm-server
cargo test                     # run all inline unit tests (~35 tests)
```

## CLI — `esm`

```sh
esm <subcommand> [options] <ESM-FILE> [...]
```

### `info` — TES4 header summary

```sh
esm info SeventySix.esm
```

Prints version, record count, next object ID, ESM/Localization flags, author, description, and master dependencies.

### `get` — Fetch a single record

```sh
# By EditorID (decoded JSON)
esm get SeventySix.esm --edid AssaultRifle --pretty

# By FormID (hex or decimal)
esm get SeventySix.esm --formid 0x463F --pretty

# Raw subrecords (no schema decoding)
esm get SeventySix.esm --formid 0x463F --raw --pretty

# With localized strings resolved
esm get SeventySix.esm --edid AssaultRifle --strings "SeventySix - Localization.ba2"

# Control FormID cross-reference depth
esm get SeventySix.esm --edid AssaultRifle --resolve full   # inline referenced records
esm get SeventySix.esm --edid AssaultRifle --resolve stub   # referenced records as stubs
esm get SeventySix.esm --edid AssaultRifle --resolve none   # leave FormIDs as hex (default)
```

| Flag | Default | Description |
|---|---|---|
| `--formid <ID>` | — | Hex (`0x1234`) or decimal FormID |
| `--edid <ID>` | — | EditorID string |
| `--json` | false | Emit JSON (implied by `--pretty`) |
| `--pretty` | false | Pretty-print JSON |
| `--raw` | false | Skip schema decode; dump raw subrecords |
| `--strings <BA2>` | — | Localization BA2 to resolve LStrings |
| `--strings-dir <DIR>` | — | Directory of loose `.strings` / `.dlstrings` files |
| `--lang <code>` | `en` | Language code for string tables |
| `--startup-ba2 <BA2>` | — | Startup BA2 for curve table evaluation |
| `--resolve <depth>` | `none` | FormID cross-reference depth: `none`, `stub`, `full` |

### `list` — List records of a type

```sh
esm list SeventySix.esm --type WEAP --limit 20
esm list SeventySix.esm --type GLOB --strings "SeventySix - Localization.ba2" --pretty
```

| Flag | Default | Description |
|---|---|---|
| `--type <SIG>` | required | 4-char record type signature |
| `--limit <N>` | 50 | Max records to return |
| `--strings <BA2>` | — | Resolve LStrings |
| `--strings-dir <DIR>` | — | Loose string files |
| `--lang <code>` | `en` | Language |

### `search` — Wildcard search over EditorIDs and names

```sh
esm search SeventySix.esm "*Rifle*" --type WEAP --in both --pretty
esm search SeventySix.esm "Assault*" --in edid
```

| Flag | Default | Description |
|---|---|---|
| `<pattern>` | required | Wildcard pattern (`*` = any substring, case-insensitive) |
| `--type <SIG,...>` | all | Comma-separated record types to search |
| `--in <field>` | `both` | `edid`, `name`, or `both` |
| `--limit <N>` | 100 | Max results |
| `--json` / `--pretty` | — | Output format |
| `--strings`, `--strings-dir`, `--lang` | — | String resolution |

### `refs` — Reverse FormID lookup

```sh
esm refs SeventySix.esm --edid AssaultRifle --limit 50
esm refs SeventySix.esm --formid 0x463F --json --pretty
```

Find all records that reference a given FormID. Builds and caches an xref index on first run.

### `tree` — Browse the GRUP hierarchy

```sh
esm tree SeventySix.esm --type WEAP --limit 50 --pretty
esm tree SeventySix.esm --offset 0 --limit 20
```

### `diff` — Compare two ESM versions

```sh
esm diff old.esm new.esm --type GLOB --json --pretty
esm diff SeventySix_20260612.esm SeventySix_20260619.esm
```

Aligns records by FormID, uses byte-equality fast-path, decodes only changed records, and emits a sparse `{from, to}` diff per changed field. Prints a per-type summary and timing to stderr.

### `coverage` — Schema audit

```sh
esm coverage SeventySix.esm --type WEAP
esm coverage SeventySix.esm --gate   # exits non-zero on any raw_fallback
```

Counts `_raw`, `_unmapped`, `_unknown_record`, and `_unresolved` markers across decoded records. Use `--gate` in CI to enforce full decode coverage.

## Server — `esm-server`

Feature-gated HTTP REST + MCP stdio server. Build with `--features server`:

```sh
cargo run --release --features server --bin esm-server -- SeventySix.esm
cargo run --release --features server --bin esm-server -- SeventySix.esm --compare SeventySix_prev.esm --port 3000
cargo run --release --features server --bin esm-server -- SeventySix.esm --mcp-stdio
```

HTTP routes: `GET /info`, `/records/{formid}`, `/records?edid=|type=&limit=`, `/groups`, `/groups/{sig}/children`, `/stub/{offset}`, `/diff`, `/health`. Serves embedded HTML viewer at `/` and `/compare`.

MCP stdio mode implements JSON-RPC 2.0 with three tools: `esm_file_info`, `esm_get_record`, `esm_list_records`.

## Library API

The `esm` crate exposes a `Database` facade for library consumers:

```rust
use esm::{Database, FormId, ResolveDepth};

let db = Database::open("SeventySix.esm")?;

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

`schema/fo76.json` (2.3 MB) is embedded at compile time via `include_str!`. It covers 173 FO76 record types derived from xEdit Pascal definitions. An `fo76.overrides.json` is merged on top for manual corrections.

To regenerate or extend coverage:

```sh
# Requires a TES5Edit/FO76Edit checkout at ../TES5Edit
python3 tools/extractor/extract.py

# Audit schema parity against Pascal source (exits non-zero on HIGH drops)
python3 tools/extractor/audit.py --gate
```

## Tests

~40 tests across `tests/` (integration test files) and two inline `#[cfg(test)]` blocks (for `tree` and `decode` internals that are not public). Run all:

```sh
cargo test

# Integration test (needs real ESM files via env vars)
RUST_TEST_ESM_A=old.esm RUST_TEST_ESM_B=new.esm cargo test -- --ignored
```

| File | What it covers |
|---|---|
| `tests/wildcard.rs` | Wildcard matching (substring, prefix, suffix, multi-star) |
| `tests/curves.rs` | Curve evaluation: clamping, interpolation, edge cases |
| `tests/diff.rs` | JSON diff logic; `diff_databases` (ignored, needs game data) |
| `tests/reader.rs` | ESM walk: group/record event sequence from a synthetic file |
| `tests/decode_records.rs` | Schema-driven decode of MGEF and OMOD records |
| `src/tree.rs` (inline) | `decode_label` dispatch (`pub(crate)`, not accessible from `tests/`) |
| `src/decode.rs` (inline) | `decode_struct_fields` count-prefix width (private function) |

Tests never need game data — they build synthetic byte buffers in-memory.

## Index cache

On first open, the tool writes a `<name>.esm.idx` file next to the ESM. Subsequent opens skip re-parsing and load the index from this cache (keyed by file path, size, and mtime). The cache regenerates automatically when the ESM changes or `CACHE_VERSION` is bumped. These files are gitignored.
