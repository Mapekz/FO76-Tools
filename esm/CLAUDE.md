# CLAUDE.md — esm

Guidance for Claude Code when working in this Rust workspace.

## Commands

```sh
cargo build [--release]                             # esm CLI (target/release/esm)
cargo build [--release] --features server           # also builds esm-server
cargo run --bin esm -- <args>                       # run CLI
cargo run --features server --bin esm-server -- <ESM> [--mcp-stdio]
cargo test                                          # ~100 tests (2 ignored integration tests)
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
| `src/mindex.rs` | Zero-copy mmap'd FormID index (`*.esm.midx`); 40-byte header + 24-byte sorted entries; `MmapFormIndex` (binary search, O(log n)); written opportunistically in `build_fresh` |
| `src/registry.rs` | `Registry`: lazily opens and caches `Database` per canonical path; stale-file eviction via `FileSig` (one `fs::metadata` check per cache hit); `auto_warm` flag for daemon mode |
| `src/tree.rs` | GRUP tree arena (`TreeIndex`); `GroupNode`, `RecordStub`, `GroupLabel` enum |
| `src/schema.rs` | Serde model for `schema/fo76.json`; `MemberDef` enum (18 variants, `#[serde(tag="kind")]`); `load_embedded()` |
| `src/decode.rs` | Schema-driven decoder → `serde_json::Value`; `DecodeContext<'a>`, `FormIdRefResolver` trait; never panics |
| `src/ctda.rs` | CTDA condition decoder; function-index table (binary search); imports `crate::decode::{hex, resolve_formid}` |
| `src/diff.rs` | `diff_databases(a,b)` — byte-equality fast-path, sparse `{from,to}` JSON diff |
| `src/wildcard.rs` | Case-insensitive `*`-wildcard matcher; has rustdoc doctest |
| `src/lib.rs` | `Database` facade (all public API); `Database::open_lite` (mmap index only, no 280 MiB bincode load); `DatabaseResolver` (depth-limited FormID expansion to 2 levels) |
| `src/bin/cli.rs` | Thin clap CLI: `info`, `get`, `list`, `search`, `refs`, `tree`, `diff`, `coverage`, `daemon {start,stop,status}`; `-p` (one-shot via warm daemon), `--local` (cold in-process), `--mmap-index` |
| `src/bin/server.rs` | Axum HTTP + MCP-stdio server (feature `server`); five MCP tools: `esm_file_info`, `esm_get_record`, `esm_list_records`, `esm_search`, `esm_refs`; `--daemon` mode with idle-TTL watchdog (`ESM_DAEMON_IDLE_SECS`) |
| `bindings/napi/src/lib.rs` | N-API class `EsmDatabase` (`Mutex<Database>`); `#[napi]` async methods |
| `app/` | Electron GUI ("FO76 ESM Viewer"); main/preload/renderer; consumes the N-API addon |

Public API re-exported from `lib.rs`: `Database`, `FormId`, `ResolveDepth`, `DiffResult`, `RecordDiff`, `RecordResult`, `ListEntry`, `GroupNode`, `TreeIndex`, `DatabaseResolver`, `parse_form_id_input`.

## Conventions to Follow

- **Error handling**: `anyhow::Result<T>` everywhere (lib, CLI, napi). `bail!` for validation, `.context()`/`.with_context()` for context. **No custom error enum** — `thiserror` is declared but unused; don't add enums unless the public API requires callers to `match` on variants.
- **Serialization**: manual little-endian byte reads (`u*::from_le_bytes`, `byteorder::ReadBytesExt`) for fixed headers; `serde`/`serde_json` for output; `bincode` for the index cache. No `binrw`/`nom`.
- **Schema editing**: `schema/fo76.json` is embedded at compile time (`include_str!`). Change the extractor (`tools/extractor/extract.py`) or add overrides to `fo76.overrides.json` — don't hand-edit `fo76.json` directly unless fixing something the extractor can't express.
- **Decoder must never panic**: unknown/malformed bytes → raw hex fallback (`_raw`, `_unknown_record`, `_unmapped`). Do not add unwraps on untrusted input.
- **Tests**: most tests live in `tests/` (one file per module: `wildcard.rs`, `curves.rs`, `diff.rs`, `reader.rs`, `ipc.rs`, `decode_records.rs`, `decode_coverage.rs`). Tests that exercise private or `pub(crate)` symbols stay colocated in `#[cfg(test)]` blocks (`tree.rs`, `decode.rs`). All tests use synthetic in-memory byte buffers — no real ESM required. Integration tests that need game data go under `#[ignore]` with an env-var gate (see `tests/diff.rs`, `tests/decode_coverage.rs`).

## Critical Invariants — Do Not Break

- **READ-ONLY: no ESM write path exists.** `compress.rs` only decompresses. The only files written are `*.esm.idx` and `*.esm.midx` (index caches, not the source ESM). Do not add ESM mutation without an explicit design.
- **`compress.rs` = decompress only**: `decompress_lz4`, `decompress_zlib`, `decompress_record_data`. No `compress_*` functions.
- **GNRL-only in `ba2.rs`**: DX10 texture archives are detected and rejected. Do not add DX10 support without a separate path.
- **Three `unsafe { Mmap::map }` blocks** (in `reader.rs`, `ba2.rs`, and `mindex.rs`). All three have `// SAFETY:` comments — keep them accurate if you touch the surrounding code.
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

`SeventySix.esm`, `SeventySix - Localization.ba2`, `SeventySix - Startup.ba2`, `*.esm.idx`, and `*.esm.midx` are **gitignored, non-redistributable**. Never commit them; never hardcode their paths in source — always passed at runtime via CLI args or `Database::open(path)`.

## Bulk / sweep workflow (for agents)

AI agents that scan many records must avoid cold per-record process spawns. Each cold `esm get` / `esm -p get` invocation reads and deserializes the **entire ~280 MiB `.esm.idx`** bincode cache into heap HashMaps just to perform one lookup, then exits. 1000 sweeps = 1000× (read 280 MiB + allocate ~280 MiB of HashMaps) — 5–10 s per record, heavy swap thrash.

### Recommended: warm daemon (fastest, no extra flags)

Build `esm-server` once, then use `-p` for every single-record lookup:

```sh
# Build both binaries (server must be alongside esm for auto-spawn to work)
cargo build --release --features server

# Every -p call auto-spawns the daemon on first use; subsequent calls are fast HTTP round-trips
esm -p get SeventySix.esm 0x463F --pretty
esm -p get SeventySix.esm AssaultRifle --pretty
```

The daemon warms the index once on first load and serves all subsequent lookups in memory. It self-manages:
- **Auto-spawns** on the first `-p` call (no manual `daemon start` needed).
- **Auto-shuts-down** after 10 min idle (`ESM_DAEMON_IDLE_SECS=0` to disable).
- **Stale-evicts** if the ESM changes on disk — no manual restart needed.
- **Parallel-agent safe** — advisory spawn-lock (`esm-daemon.lock`) prevents double-spawn; multiple agents share one daemon instance.

Use `esm daemon status` to check, `esm daemon stop` to kill early.

### Prefer bulk ops over N single gets

Every round-trip has overhead. When you need many records of the same type, use bulk ops:

```sh
esm -p list SeventySix.esm --type WEAP --limit 500 --pretty   # all weapons in one call
esm -p search SeventySix.esm "*Rifle*" --type WEAP --pretty   # search by name/EditorID
esm -p refs SeventySix.esm 0x463F --limit 100 --pretty        # reverse FormID lookup
esm -p coverage SeventySix.esm --type WEAP                    # schema decode audit
```

### Gotcha: `--strings` / `--startup-ba2` bypass the daemon

Passing `--strings`, `--strings-dir`, or `--startup-ba2` to `get` forces a cold in-process open (the daemon doesn't load BA2 args from per-call flags). For sweeps that need localized strings, place `SeventySix - Localization.ba2` and `SeventySix - Startup.ba2` next to the ESM — the daemon auto-loads them on open, and warm lookups return localized output without per-call BA2 flags.

### Daemonless option: `--mmap-index`

For cold FormID lookups without a background process, use the zero-copy mmap index:

```sh
# Loads a ~24 MiB .esm.midx table instead of the 280 MiB bincode cache
esm --local --mmap-index get SeventySix.esm 0x463F --pretty
# Or set the env var so every --local call uses it
ESM_MMAP_INDEX=1 esm --local get SeventySix.esm 0x463F --pretty
```

Limitations: FormID lookups only. EditorID (`--edid`), `list`, `search`, `refs`, and `tree` require the full index — use the daemon for those.

The `.esm.midx` file is written automatically whenever the `.esm.idx` is freshly built, so it's always available alongside the bincode cache.

### MCP opt-in (for AI clients that support it)

`esm-server --mcp-stdio` speaks JSON-RPC 2.0 over stdin/stdout. Wire it up in your AI client's MCP config — **do not commit** the config file (it hardcodes a date-stamped, non-redistributable ESM path):

```jsonc
// .mcp.json (gitignored — fill in your actual ESM path)
{
  "mcpServers": {
    "fo76-esm": {
      "command": "/path/to/esm-server",
      "args": ["--mcp-stdio", "/path/to/SeventySix_YYYYMMDD.esm"]
    }
  }
}
```

The server exposes five tools: `esm_file_info`, `esm_get_record`, `esm_list_records`, `esm_search`, `esm_refs`. Under the hood MCP-stdio proxies to the same HTTP daemon, so the warm-index benefit applies automatically.

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
| QUST | `VMAD` (fragmented) | Six quests use `wbVMADFragmentedQUST` payloads the flat VMAD decoder does not parse yet. |
