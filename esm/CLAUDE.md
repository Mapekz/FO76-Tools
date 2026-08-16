# CLAUDE.md — esm

Guidance for Claude Code when working in this Rust workspace.

## Commands

```sh
cargo build [--release]                             # esm CLI (target/release/esm)
cargo build [--release] --features server           # also builds esm-server
cargo run --bin esm -- <args>                       # run CLI
cargo run --features server --bin esm-server -- <ESM> [--mcp-stdio]
cargo test                                          # ~100 tests; env-gated integration tests skip silently if unset
cargo clippy --all-targets -- -D warnings
cargo fmt [--check]

# Schema tooling (requires ../TES5Edit checkout)
python3 tools/extractor/extract.py                  # regenerate schema/fo76.json
python3 tools/extractor/audit.py --gate             # parity audit (exits non-zero on HIGH drops)

# Patch-notes pipeline (mechanical stage; narrative stage = /patch-notes skill)
just patch-notes OLD NEW                            # diff.json + comprehensive.{md,json} + bundles.json + lints.json + manifest.json
just patch-tools-test                               # Python tooling test suite (tools/tests/)
```

## Before committing

Run `just` (= `just check` = `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` for both default and `--features server` + `cargo test`) and ensure it passes before every commit. Run `just audit` as well whenever you change the schema, the extractor, or anything affecting decode coverage. Never commit with failing or skipped checks.

## Architecture

`docs/architecture.md` owns the full picture: record read flow (bytes → schema decode →
`serde_json::Value`), index/cache lifecycle, process topology (CLI, daemon, HTTP/MCP server,
N-API, Python pipeline), and the feature-layer modules (`diff`, `walk`, `chase`, `lvli`, `refs`).
Its "Where to tweak what" table is the fastest way to find the right edit point for a given
change; domain vocabulary lives in `../CONTEXT.md`, and design decisions are recorded in
`docs/adr/`.

| Area | Entry point |
|---|---|
| Binary parsing | `src/reader.rs`, `src/format.rs` |
| Schema-driven decode | `src/decode.rs` (+ `decode/vmad.rs`, `src/ctda.rs`) |
| Index & disk cache | `src/index.rs`, `src/rkyvcache.rs`, `src/progress.rs` |
| Cross-process daemon path | `src/registry.rs`, `src/backend.rs`, `src/ipc.rs` |
| CLI / HTTP+MCP server / N-API | `src/bin/cli.rs`, `src/bin/server.rs`, `bindings/napi/src/lib.rs` |
| Diff / walk / chase / lvli / refs | `src/diff.rs`, `src/walk/`, `src/chase.rs`, `src/lvli.rs`, `src/refs.rs` |
| Python patch-notes pipeline (mechanical stage) | `tools/` |

The Electron GUI ("FO76 ESM Viewer") that consumes the N-API addon lives in the sibling
`../esm-viewer/` directory, not in this crate — see [`../esm-viewer/CLAUDE.md`](../esm-viewer/CLAUDE.md).

Public API re-exported from `lib.rs`: `Database`, `FormId`, `ResolveDepth`, `DiffResult`, `RecordDiff`, `RecordResult`, `ListEntry`, `GroupNode`, `TreeIndex`, `DatabaseResolver`, `parse_form_id_input`, `RefList`, `RefRow`, `RefPathNode`, `EntryPointSpec`, `EntryPointRef`.

## Conventions to Follow

- **Error handling**: `anyhow::Result<T>` everywhere (lib, CLI, napi). `bail!` for validation, `.context()`/`.with_context()` for context. **No custom error enum** — `anyhow` covers it; adding one would require callers to `match` on variants and a `thiserror` dependency.
- **Serialization**: manual little-endian byte reads (`u*::from_le_bytes`, `byteorder::ReadBytesExt`) for fixed headers; `serde`/`serde_json` for output; zero-copy `rkyv` sections (`src/rkyvcache.rs`) for the index cache. No `binrw`/`nom`.
- **Schema editing**: `schema/fo76.json` is embedded at compile time (`include_str!`). Change the extractor (`tools/extractor/extract.py`) or add overrides to `fo76.overrides.json` — don't hand-edit `fo76.json` directly unless fixing something the extractor can't express.
- **Decoder must never panic**: unknown/malformed bytes → raw hex fallback (`_raw`, `_unknown_record`, `_unmapped`). Do not add unwraps on untrusted input.
- **Tests**: most tests live in `tests/` (one file per module: `wildcard.rs`, `curves.rs`, `diff.rs`, `reader.rs`, `ipc.rs`, `decode_records.rs`, `decode_coverage.rs`). Tests that exercise private or `pub(crate)` symbols stay colocated in `#[cfg(test)]` blocks (`tree.rs`, `decode.rs`, `backend.rs`'s `DaemonHost`/`FakeHost` daemon-lifecycle tests, `registry.rs`'s `RegistryHost`/`FakeHost` cache-policy tests, `diff.rs`'s `lcs_align` alignment/safety-cap tests). All tests use synthetic in-memory byte buffers — no real ESM required. Integration tests that need game data skip silently when the relevant env var is unset (see `tests/diff.rs`, `tests/decode_coverage.rs`).

## Critical Invariants — Do Not Break

- **READ-ONLY: no ESM write path exists.** `compress.rs` only decompresses. The only files written are `Index`'s five rkyv cache sections (`tree`/`forms`/`edid`/`search`/`xref`, inside the shared `esm_cache/` directory, not the source ESM). Do not add ESM mutation without an explicit design.
- **`compress.rs` = decompress only**: `decompress_lz4`, `decompress_zlib`, `decompress_record_data`. No `compress_*` functions.
- **GNRL-only in `ba2.rs`**: DX10 texture archives are detected and rejected. Do not add DX10 support without a separate path.
- **Three `unsafe { Mmap::map }` blocks** (in `reader.rs`, `ba2.rs`, and `rkyvcache.rs`), plus one categorically different `unsafe { rkyv::access_unchecked }` in `rkyvcache.rs::Section::get` (asserts the *validity* of already-mapped bytes, not that a mapping itself is sound — see its 54-line SAFETY comment). All four have `// SAFETY:` comments — keep them accurate if you touch the surrounding code.
- **XXXX oversized-subrecord rule** in `reader.rs` (around line 304): a 6-byte `XXXX` subrecord whose `data_size` field carries the actual size precedes an oversized subrecord with `data_size = 0`. Preserve this when modifying the subrecord scanner.
- **`index.rs` cache**: keyed by path/size/mtime, plus a per-section `layout_fingerprint` (`FORMS_/EDID_/SEARCH_/XREF_LAYOUT_FINGERPRINT` in `index.rs`, `TREE_LAYOUT_FINGERPRINT` in `tree.rs`) folding each section's archived `size_of`/`align_of` — the other half of cache invalidation, alongside `CACHE_VERSION`. **Bump `CACHE_VERSION`** whenever any section's cached data layout changes — the old cache becomes invalid and will be rebuilt.
- **FormID layout**: high byte = master-file index, low 24 bits = object ID. All values little-endian.
- **Decode output key conventions** (must stay consistent): `_record_type`, `_unknown_record`, `_unmapped`, `_raw`, `_unresolved`, and (diff output only) `_array_diff`. These are the flags the `coverage` subcommand, MCP server, and patch-notes tooling rely on.
- **`advance_union` / `RArray` paths in `decode.rs`**: struct union variants advance by real decoded byte counts; fixed scalars still use `field_byte_size`. Change with extra care and verify against real ESM output.
- **Schema `fo76.json` is generated** — treat it as a build artifact. Fix decode coverage by updating the extractor or `fo76.overrides.json`, not by hand-editing the 2.3 MB JSON.
- **Path canonicalization must stay consistent across every consumer of a build lease.** `Registry`, the CLI's progress watcher, and `backend.rs`'s `building_progress`/`watch_path` all key a build by the same canonicalized path (`discover::resolve_esm_path`) — a caller that opens `Database` directly instead of going through `Registry` breaks that invariant. `bindings/napi`'s `EsmDatabase::open_database` already routes through a throwaway `Registry` for this; keep any new direct-open path doing the same.

## N-API Binding and Electron App

The `bindings/napi/` sub-crate (`esm-napi`) builds a `esm-napi.<platform>.node` addon. The Electron app is now at `../esm-viewer/` (sibling directory of `esm/`, tracked separately at repo root) and depends on it via the `@fo76/esm-napi` npm package (local file dep, `"file:../esm/bindings/napi"`). After any Rust API change that affects `EsmDatabase`, rebuild the addon:

```sh
cd bindings/napi && bun run build   # or build:debug
```

The app loads the addon via `esm-viewer/src/main/addon.ts`. Most of the Rust N-API DTOs are mirrored to TypeScript via `ts-rs` (dev-dependency; `#[cfg_attr(test, derive(ts_rs::TS))]` + `#[cfg_attr(test, ts(export))]` on the DTOs in `lib.rs`/`ipc.rs`/`reader.rs`/`tree.rs`/`diff.rs`/`decode.rs`) — run `just gen-types` after changing any of those structs' shape, which regenerates `esm-viewer/src/shared/generated/*.ts`; `just check` fails if that regen produces an uncommitted diff. `esm-viewer/src/shared/api-types.ts` re-exports those generated types (aliasing a few names) and hand-writes only the IPC-contract-specific bits (`CH` channel names, `Fo76Api`, `FilterOp`) — keep *that* in sync when adding/removing `EsmDatabase` methods.

## Game Data

Game data files (`*.esm`, `*.ba2`, and `Index`'s shared `esm_cache/` directory holding its five rkyv cache sections `tree`/`forms`/`edid`/`search`/`xref`, plus `progress.rs`'s `.build.lock`/`.build.json` sidecar files) are **gitignored, non-redistributable**. Never commit them; never hardcode their paths in source — always passed at runtime via `--esm`/`FO76_ESM_PATH`/`Database::open(path)`.

## CLI usage knowledge (for agents querying game data)

Invocation modes, bulk ops, `--resolve stub`, `refs` selectors, daemon lifecycle, MCP, and how
to interpret live-vs-cut game data live in `skills/esm-cli/SKILL.md` — it ships embedded in the
binary (`esm skill`) and is available in this repo via the `.claude/skills/esm-cli` symlink, so
there's no need to duplicate it here.

## Coverage drift handling (vs TES5Edit)

Drift subrecords newer than the TES5Edit reference are handled as follows:

- **LVLI/LVLN/LVPC/LVLP `LVLD`**, **RESO `NAM5`**, **NPC_ `AWPB`+`CTDA`**, **GMRW `XALG`**, **STAT `SNAM`+`ANLD`**, **REFR `MCND`** — mapped in `schema/fo76.overrides.json` (GMRW XALG expands from `$pascal_var: wbXALG`, u64 legendary flags; REFR MCND is an rarray-of-unknown, in no TES5Edit definition at all).
- **CTDA function table** — generated to `schema/fo76.ctda.json` from Pascal; loaded at runtime in `src/ctda.rs`.
- **EFIT**, **Model Information**, **CTDA** — schema kinds (`struct` / `model_info` / `ctda`); no magic-string dispatch in `decode.rs`.
- **QUST `VMAD` (fragmented)** — `decode_vmad_qust` in `src/decode.rs` handles Script Fragments + Aliases tail.
- **INFO/PACK/PERK/SCEN `VMAD` (fragmented)** — `decode_vmad_{info,pack,perk,scen}` in `src/decode.rs` handle each record type's Script Fragments tail; dispatched by `ctx.record_signature`.
- **NPC_ `VMAD` type-0/type-7 properties** — `decode_vmad_property` handles type 0 (None → null) and type 7 (Struct → named-member array). NPC_ is now in `CLEAN_TYPES`.
