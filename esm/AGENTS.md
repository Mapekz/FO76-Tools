# ESM

Scoped guidance for the ESM Rust workspace. Shared policy and validation mapping
live in [../AGENTS.md](../AGENTS.md).

## Build and validation

Run commands from `esm/`; `justfile` owns the complete recipe list.

- Rebuild with `just release` (a plain `cargo build --release`) before querying
  the CLI after changes; every call runs in-process, so the next one uses it.
- `just check` covers formatting, clippy and tests across the workspace,
  generated-type drift, and schema formatting. `just audit` adds TES5Edit schema
  parity.
- `just patch-tools-test` and `just patch-tools-lint` validate Python tooling.
- `just patch-notes OLD NEW` runs the mechanical patch-notes pipeline; use the
  patch-notes procedure for narrative output.

## Architecture

`docs/architecture.md` owns the full picture: record read flow (bytes → schema decode →
typed `Node` tree → `serde_json::Value`), cache lifecycle, process topology (CLI, `esm batch`, N-API, Python
pipeline), and the feature-layer modules (`diff`, `walk`, `chase`, `lvli`, `refs`).
Its "Where to tweak what" table is the fastest way to find the right edit point for a given
change; domain vocabulary lives in `CONTEXT.md`, and design decisions are recorded in
`docs/adr/`.

| Area | Entry point |
|---|---|
| Binary parsing | `src/reader.rs`, `src/format.rs` |
| Schema-driven decode | `src/decode/mod.rs` (+ `decode/vmad.rs`, `src/ctda.rs`) |
| Index & disk cache | `src/index.rs`, `src/rkyvcache.rs`, `src/progress.rs` |
| Op dispatch (every surface) | `src/host.rs`, `src/ops/mod.rs` |
| CLI / N-API | `src/bin/cli/main.rs` (+ per-family handler modules), `bindings/napi/src/lib.rs` |
| Diff / walk / chase / lvli / refs | `src/diff/`, `src/walk/`, `src/chase.rs`, `src/lvli.rs`, `src/refs.rs` |
| Python patch-notes pipeline (mechanical stage) | `tools/` |

The Electron GUI ("FO76 ESM Viewer") that consumes the N-API addon lives in the sibling
`../esm-viewer/` directory, not in this crate — see [`../esm-viewer/AGENTS.md`](../esm-viewer/AGENTS.md).

Public API re-exported from `lib.rs`: `Database`, `FormId`, `FormIdBase`, `ResolveDepth`, `DiffResult`, `RecordDiff`, `RecordResult`, `ListEntry`, `GroupNode`, `TreeIndex`, `DatabaseResolver`, `parse_form_id_input`, `RefList`, `RefRow`, `RefPathNode`, `EntryPointSpec`, `EntryPointRef`.

## Conventions to Follow

- **Error handling**: `anyhow::Result<T>` everywhere (lib, CLI, napi). `bail!` for validation, `.context()`/`.with_context()` for context. **No custom error enum** — `anyhow` covers it; adding one would require callers to `match` on variants and a `thiserror` dependency.
- **Serialization**: manual little-endian byte reads (`u*::from_le_bytes`, `byteorder::ReadBytesExt`) for fixed headers; `serde`/`serde_json` for output; zero-copy `rkyv` sections (`src/rkyvcache.rs`) for the index cache. No `binrw`/`nom`.
- **Schema editing**: `schema/fo76.json` is embedded at compile time (`include_str!`). Change the extractor (`tools/extractor/extract.py`) or add overrides to `schema/fo76.overrides.json`; regenerate `fo76.json` rather than editing it directly.
- **Decoder must never panic**: unknown/malformed bytes → raw hex fallback (`_raw`, `_unknown_record`, `_unmapped`). Do not add unwraps on untrusted input.
- **Tests**: most tests live in `tests/` (one target per module: `wildcard.rs`, `curves.rs`, `diff.rs`, `reader.rs`, `ops.rs`, `decode_coverage.rs`). A target that outgrows one file becomes a directory with a `main.rs` declaring its submodules — `tests/decode_records/` splits its whole-record goldens by record family (`weapons.rs`, `perks.rs`, `races.rs`, …), and `cargo test --test decode_records` still selects the whole binary. Tests that exercise private or `pub(crate)` symbols stay colocated in `#[cfg(test)]` blocks (`tree.rs`, `decode/mod.rs`, `host.rs`'s `Opener`/`FakeHost` reopen and race tests, `diff.rs`'s `lcs_align` alignment/safety-cap tests). Synthetic tests use in-memory byte buffers. Integration tests that need game data skip silently when the relevant env var is unset (see `tests/diff.rs`, `tests/decode_coverage.rs`).

## Critical Invariants — Do Not Break

- **Source ESMs stay read-only.** Generated caches live separately in `esm_cache/`; writing cache or export artifacts does not permit source mutation.
- **`compress.rs` = decompress only**: `decompress_lz4`, `decompress_zlib`, `decompress_record_data`. No `compress_*` functions.
- **GNRL-only in `ba2.rs`**: DX10 texture archives are detected and rejected. Do not add DX10 support without a separate path.
- **Unsafe access**: preserve the `SAFETY` contracts on memory mapping and `rkyv::access_unchecked` in `reader.rs`, `ba2.rs`, and `rkyvcache.rs`. The latter requires valid archived bytes, not merely a sound mapping.
- **XXXX oversized subrecords**: the 6-byte `XXXX` header declares a 4-byte little-endian size payload, which supplies the length of the following subrecord when its header size is zero. Preserve this in `reader.rs`.
- **`index.rs` cache**: keyed by path/size/mtime, plus a per-section `layout_fingerprint` (`FORMS_/EDID_/SEARCH_/XREF_LAYOUT_FINGERPRINT` in `index.rs`, `TREE_LAYOUT_FINGERPRINT` in `tree.rs`) folding each section's archived `size_of`/`align_of` (plus, for `xref`, the build-time digest of the embedded schema) — the other half of cache invalidation, alongside `CACHE_VERSION`. **Bump `CACHE_VERSION`** whenever any section's cached data layout changes — the old cache becomes invalid and will be rebuilt.
- **FormID layout**: high byte = master-file index, low 24 bits = object ID. All values little-endian.
- **Decode output key conventions** (must stay consistent): `_record_type`, `_unknown_record`, `_unmapped`, `_raw`, `_unresolved`, `_trailing`, and (diff output only) `_array_diff`. Every `_raw` value carries a `reason` (`unknown` for bytes the schema itself declares unknown); `_trailing` holds a struct subrecord's bytes left after its fields. These are the flags the `coverage` subcommand and patch-notes tooling rely on. Which record types get extra keys inlined onto a `--resolve stub` FormID reference (currently GLOB's `Value`, CURV's `curve_path`/`curve`) is a separate registry, `src/decode/leaf_values.rs` — see `docs/adr/0011-value-bearing-leaf-inlining.md`.
- **`advance_union` / `RArray` decoder paths**: struct union variants advance by real decoded byte counts; fixed scalars still use `field_byte_size`. Change with extra care and verify against real ESM output.
- **Every consumer of a build lease keys it by the canonical ESM path** (`discover::resolve_esm_path`). `Database::open` canonicalizes its input itself; anything that watches a build without opening a `Database` (the CLI's progress watcher, `--no-wait`, `esm cache`) must call `resolve_esm_path` too.

## N-API Binding and Electron App

The `bindings/napi/` sub-crate produces the addon consumed by
`../esm-viewer/src/main/addon.ts` through the local `@fo76/esm-napi` dependency. It exposes
one `EsmHost` (a `host::Host`) whose async `run(esm, op)` takes any `Op` as JSON, so adding an
op needs no binding code.
Follow [the root validation map](../AGENTS.md#validation-map) for addon builds,
DTO regeneration, and IPC synchronization. DTOs use `ts-rs` test-only derives
and exports in Rust; generated TypeScript is not hand-edited.

## Game Data

Game data files (`*.esm`, `*.ba2`, and `Index`'s shared `esm_cache/` directory holding its five rkyv cache sections `tree`/`forms`/`edid`/`search`/`xref`, plus `progress.rs`'s `.build.lock`/`.build.json` sidecar files) are **gitignored, non-redistributable**. Never commit them; never hardcode their paths in source — always passed at runtime via `--esm`/`FO76_ESM_PATH`/`Database::open(path)`.

## CLI usage knowledge (for agents querying game data)

Invocation, cache behaviour, bulk ops, `--resolve stub`, `refs` selectors, and how
to interpret live-vs-cut game data live in `skills/esm-cli/SKILL.md` — it ships embedded in the
binary (`esm skill`). Read the procedure directly when querying game data.

## Coverage drift handling (vs TES5Edit)

Drift subrecords newer than the TES5Edit reference are handled as follows:

- **LVLI/LVLN/LVPC/LVLP `LVLD`**, **RESO `NAM5`**, **NPC_ `AWPB`+`CTDA`**, **GMRW `XALG`**, **STAT `SNAM`+`ANLD`**, **REFR `MCND`**, **COEN `ETGR`**, **COBJ `ENAM`** — mapped in `schema/fo76.overrides.json` (GMRW XALG expands from `$pascal_var: wbXALG`, u64 legendary flags; REFR MCND is an rarray-of-unknown, in no TES5Edit definition at all).
- **QUST objective `QOST`**, **REGN weather-entry `RDWC`** — nested drift, inserted beside an existing member by a `record_patches` entry with `"op": "insert_after"`.
- **PKIN Child Pack-In** — the game repeats the `HNAM`+`INAM` pair `GNAM` times; xEdit models a single pair, so a `record_patches` entry turns it into an rarray.
- **CTDA function table** — generated to `schema/fo76.ctda.json` from Pascal; loaded at runtime in `src/ctda.rs`.
- **EFIT**, **Model Information**, **CTDA** — schema kinds (`struct` / `model_info` / `ctda`); no magic-string dispatch in `src/decode/mod.rs`.
- **Fragmented `VMAD`** (QUST, INFO, PACK, PERK, SCEN, TERM) — the extractor carries xEdit's `wbVMADFragmented*` layout as the vmad member's `fragments`; `src/decode/vmad.rs` reads that Script Fragments tail (plus QUST's Aliases).
- **NPC_ `VMAD` type-0/type-7 properties** — `decode_vmad_property` handles type 0 (None → null) and type 7 (Struct → named-member array). NPC_ is now in `CLEAN_TYPES`.
- **RACE `CMDT`/`CMDN`/`CMDI`/`CMDE`+`PGTF`** — the Pet Commands rarray and the Progression Track pointer, mapped via `record_additions` in `schema/fo76.overrides.json`; absent from every `wbDefinitions*.pas`.
- **PGTR (whole record)** — the first use of the overrides file's `"records"` whole-record mechanism: no TES5Edit definition exists; field names come from the game's own UI model in the decompiled Scaleform movie `interface/petprogressiontrackmenu.swf`; fields that model doesn't name stay `Unknown` with the observed constant in a `_comment`. See `docs/adr/0012-whole-record-schema-overrides.md`.

When a record type has no TES5Edit definition, decompile the Scaleform UI in `$FO76_DATA_DIR/<snapshot>/interface/*.swf` (`ffdec -export script <out-dir> <file.swf>`; `ffdec` is on PATH) and take field names from the game's own data model before inventing any; if the UI doesn't name a field, leave it `Unknown` and record the observed constant plus sample size in the member's `_comment`.
