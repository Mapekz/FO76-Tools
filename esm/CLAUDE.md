# CLAUDE.md — esm

Guidance for Claude Code when working in this Rust workspace.

## Commands

```sh
cargo build [--release]                             # esm CLI (target/release/esm)
cargo build [--release] --features server           # also builds esm-server
cargo run --bin esm -- <args>                       # run CLI
cargo run --features server --bin esm-server -- <ESM> [--mcp-stdio]
cargo test                                          # ~35 unit tests + 1 ignored integration test
cargo clippy --all-targets -- -D warnings
cargo fmt [--check]

# Schema tooling (requires ../TES5Edit checkout)
python3 tools/extractor/extract.py                  # regenerate schema/fo76.json
python3 tools/extractor/audit.py --gate             # parity audit (exits non-zero on HIGH drops)
```

## Architecture

Clean layering — edit at the right level:

| Module | Purpose |
|---|---|
| `src/format.rs` | On-disk structs: `RecordHeader` (24B), `GroupHeader` (24B), `SubrecordHeader` (6B), `Signature`; constants |
| `src/formid.rs` | `FormId(u32)` newtype, hex/decimal parse, `Display` |
| `src/compress.rs` | **Decompress only** — `decompress_zlib` (records), `decompress_lz4` (BA2), `decompress_record_data` |
| `src/reader.rs` | `EsmFile` (mmap), TES4 parse, `walk_records`/`walk_structure`, `parse_subrecords` (XXXX rule), `parse_record_at` |
| `src/ba2.rs` | Minimal BTDX/GNRL BA2 reader (memory-mapped); used by strings + curves |
| `src/strings.rs` | `.strings`/`.dlstrings`/`.ilstrings` parser; `Localization::from_ba2` / `from_loose_files` |
| `src/curves.rs` | `CurveIndex` (FormID → `Curve`); loads JSON from Startup BA2; `Curve::eval` (linear interp) |
| `src/index.rs` | `Index`: FormID→offset, lazy EDID/xref/search indexes; `bincode` disk cache (`*.esm.idx`, `CACHE_VERSION = 8`) |
| `src/tree.rs` | GRUP tree arena (`TreeIndex`); `GroupNode`, `RecordStub`, `GroupLabel` enum |
| `src/schema.rs` | Serde model for `schema/fo76.json`; `MemberDef` enum (18 variants, `#[serde(tag="kind")]`); `load_embedded()` |
| `src/decode.rs` | Schema-driven decoder → `serde_json::Value`; `DecodeContext<'a>`, `FormIdRefResolver` trait; never panics |
| `src/ctda.rs` | CTDA condition decoder; function-index table (binary search); imports `crate::decode::{hex, resolve_formid}` |
| `src/diff.rs` | `diff_databases(a,b)` — byte-equality fast-path, sparse `{from,to}` JSON diff |
| `src/wildcard.rs` | Case-insensitive `*`-wildcard matcher; has rustdoc doctest |
| `src/lib.rs` | `Database` facade (all public API); `DatabaseResolver` (depth-limited FormID expansion to 2 levels) |
| `src/bin/cli.rs` | Thin clap CLI: `info`, `get`, `list`, `search`, `refs`, `tree`, `diff`, `coverage` |
| `src/bin/server.rs` | Axum HTTP + MCP-stdio server (feature `server`); three MCP tools: `esm_file_info`, `esm_get_record`, `esm_list_records` |
| `bindings/napi/src/lib.rs` | N-API class `EsmDatabase` (`Mutex<Database>`); `#[napi]` async methods |
| `app/` | Electron GUI ("FO76 ESM Viewer"); main/preload/renderer; consumes the N-API addon |

Public API re-exported from `lib.rs`: `Database`, `FormId`, `ResolveDepth`, `DiffResult`, `RecordDiff`, `RecordResult`, `ListEntry`, `GroupNode`, `TreeIndex`, `DatabaseResolver`, `parse_form_id_input`.

## Conventions to Follow

- **Error handling**: `anyhow::Result<T>` everywhere (lib, CLI, napi). `bail!` for validation, `.context()`/`.with_context()` for context. **No custom error enum** — `thiserror` is declared but unused; don't add enums unless the public API requires callers to `match` on variants.
- **Serialization**: manual little-endian byte reads (`u*::from_le_bytes`, `byteorder::ReadBytesExt`) for fixed headers; `serde`/`serde_json` for output; `bincode` for the index cache. No `binrw`/`nom`.
- **Schema editing**: `schema/fo76.json` is embedded at compile time (`include_str!`). Change the extractor (`tools/extractor/extract.py`) or add overrides to `fo76.overrides.json` — don't hand-edit `fo76.json` directly unless fixing something the extractor can't express.
- **Decoder must never panic**: unknown/malformed bytes → raw hex fallback (`_raw`, `_unknown_record`, `_unmapped`). Do not add unwraps on untrusted input.
- **Tests**: most tests live in `tests/` (one file per module: `wildcard.rs`, `curves.rs`, `diff.rs`, `reader.rs`, `decode_records.rs`). Tests that exercise private or `pub(crate)` symbols stay colocated in `#[cfg(test)]` blocks (`tree.rs`, `decode.rs`). All tests use synthetic in-memory byte buffers — no real ESM required. Integration tests that need game data go under `#[ignore]` with an env-var gate (see `tests/diff.rs`).

## Critical Invariants — Do Not Break

- **READ-ONLY: no ESM write path exists.** `compress.rs` only decompresses. The only file written is `*.esm.idx` (index cache, not source ESM). Do not add ESM mutation without an explicit design.
- **`compress.rs` = decompress only**: `decompress_lz4`, `decompress_zlib`, `decompress_record_data`. No `compress_*` functions.
- **GNRL-only in `ba2.rs`**: DX10 texture archives are detected and rejected. Do not add DX10 support without a separate path.
- **Two `unsafe { Mmap::map }` blocks** (in `reader.rs` and `ba2.rs`). Both have `// SAFETY:` comments — keep them accurate if you touch the surrounding code.
- **XXXX oversized-subrecord rule** in `reader.rs` (around line 304): a 6-byte `XXXX` subrecord whose `data_size` field carries the actual size precedes an oversized subrecord with `data_size = 0`. Preserve this when modifying the subrecord scanner.
- **`index.rs` cache**: keyed by path/size/mtime. **Bump `CACHE_VERSION`** whenever the cached data layout changes — the old cache becomes invalid and will be rebuilt.
- **FormID layout**: high byte = master-file index, low 24 bits = object ID. All values little-endian.
- **Decode output key conventions** (must stay consistent): `_record_type`, `_unknown_record`, `_unmapped`, `_raw`, `_unresolved`. These are the flags the `coverage` subcommand and MCP server rely on.
- **`advance_union` / `RArray` paths in `decode.rs`** are heuristics (byte-count estimates). Change with extra care and verify against real ESM output.
- **Schema `fo76.json` is generated** — treat it as a build artifact. Fix decode coverage by updating the extractor or `fo76.overrides.json`, not by hand-editing the 2.3 MB JSON.

## N-API Binding and Electron App

The `bindings/napi/` sub-crate (`esm-napi`) builds a `esm-napi.<platform>.node` addon. The Electron `app/` depends on it via the `@fo76/esm-napi` npm package (local file dep). After any Rust API change that affects `EsmDatabase`, rebuild the addon:

```sh
cd bindings/napi && npm run build   # or build:debug
```

The app loads the addon via `app/src/main/addon.ts`. The `app/src/shared/api-types.ts` file is the TypeScript mirror of the Rust N-API types — keep them in sync when changing `EsmDatabase` methods.

## Game Data

`SeventySix.esm`, `SeventySix - Localization.ba2`, `SeventySix - Startup.ba2`, and `*.esm.idx` are **gitignored, non-redistributable**. Never commit them; never hardcode their paths in source — always passed at runtime via CLI args or `Database::open(path)`.

## Known coverage drift (vs TES5Edit)

These `_unmapped` markers are intentional — the live ESM contains subrecords newer than or version-gated relative to the TES5Edit Pascal reference (`../TES5Edit/Core/wbDefinitionsFO76.pas`). Do not treat them as decode bugs:

| Record | Subrecord | Reason |
|---|---|---|
| LVLI | `LVLD` | `wbBelowVersion(174, LVLD …)` — live data is form-version ≥174, so LVLD is correctly out of schema scope. |
| LVLN | `LVLD` | Same as LVLI — empty `LVLD` on form-version ≥174 records. |
| LVPC | `LVLD` | Same as LVLI/LVLN. |
| LVLP | `LVLD` | Same as LVLI/LVLN. |
| RESO | `NAM5` | Absent from the TES5Edit reference; newer than the reference. |
| NPC_ | `AWPB`, `CTDA` | Absent from the entire TES5Edit reference; newer than the reference. |
| GMRW | `XALG` | Absent from the TES5Edit GMRW definition (EDID/FTAGs/ANAM/RWDS/Rewards only); newer than the reference. |
