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
| `src/discover.rs` | Generic ESM+strings+curves discovery from a file or folder input; `resolve_sources` locates the `.esm` plus sibling `strings/`/curvetables sources (loose or BA2); `resolve_esm_path` is the folder→ESM + canonicalize resolution `Registry`, `cli.rs`'s progress watcher, and `backend.rs`'s `building_progress`/`watch_path` must all key off the same way (`bindings/napi` is the one known gap — `EsmDatabase::open_database` now routes through a throwaway `Registry` for this, but nothing yet forces every future direct `Database::open` caller through it) |
| `src/index.rs` | `Index`: FormID→offset, plus the five independent zero-copy `rkyv` disk-cache sections (`tree`/`forms`/`edid`/`search`/`xref`, `CACHE_VERSION = 15`) inside the shared `esm_cache/` directory, named `<esm file name>.<section>`, built on `src/rkyvcache.rs` (`cache_dir_for`/`section_path_for`); `Index` itself only holds the sections and the pure reads over them (`get_by_formid`, `records_by_type`, `tree()`, …) — the three lazy builds (`ensure_edid_index`/`ensure_search_index`/`ensure_xref_index`) live on `Database` (`lib.rs`), since building a section needs the mmap'd ESM/schema/localization/curves only `Database` holds (see `docs/adr/0006`); each `Database` build method plus `Index::build`'s `build_tree_and_forms` uses `progress::BuildLease::acquire_or_recheck` (see `docs/adr/0007`), which folds the "did another process finish while I waited" recheck into the acquire call itself instead of leaving it to caller convention; every section's `(SectionKind, LAYOUT_FINGERPRINT, archived type)` triple is bound once via `rkyvcache::SectionSpec`, next to that section's own type; `cache_inventory` is the pure "which sections exist" read `esm cache status` uses (never triggers a build) |
| `src/progress.rs` | Cross-process cache-build coordination: a per-ESM advisory `.build.lock` plus an atomically-published `.build.json` heartbeat inside `esm_cache/`, both siblings of the five rkyv sections (see `docs/adr/0003`); `BuildLease` (acquire/tick/writing, held by whichever process is building — daemon, `--local` CLI, or N-API host), `BuildLease::acquire_or_recheck` (the enforced acquire-then-recheck wrapper, returning an `Acquired<T>` of `AlreadyBuilt`/`NeedsBuild`), and `read` (instant, never blocks — `Some` iff a live process holds the lock) are the three pieces; `ESM_NO_PROGRESS=1` disables heartbeat *publishing* only, dedup via the lock stays on; `BuildStage` is the single source of truth `rkyvcache::SectionKind` aliases (`pub(crate) use BuildStage as SectionKind`) — the same five variants are no longer declared twice |
| `src/registry.rs` | `Registry`: lazily opens and caches `Database` per canonical path (via `discover::resolve_esm_path`); stale-file eviction via `FileSig` (one `fs::metadata` check per cache hit); `auto_warm` flag for daemon mode; the private `RegistryHost` seam (mirrors `backend.rs`'s `DaemonHost`) separates the two OS-facing primitives (`read_sig`, `open`) from this caching policy, so stale-eviction, the double-open race guard, and the `new()`/`with_warm_xref(_)` warm-policy divergence are all covered by colocated `#[cfg(test)]` tests with no real ESM on disk |
| `src/ipc.rs` | Wire types (`Op`, `Request`/`Response`, `RecordSel`) and the canonical `dispatch_op`/`dispatch_inner` — the one query-dispatch surface shared by the daemon, CLI, HTTP/MCP server, and N-API bindings; `diff_locked` (post-lock diff + type-filter, shared by the registry-backed and N-API diff paths); `Op::Walk`/`Op::Chase`/`Op::DropTable` run `walk::walk`/`chase::chase`/`lvli::drop_table` server-side (inside whichever process is handling the op) against the private `DbFetcher` adapter, an in-process `ChaseFetcher` impl reading straight off the already-open `Database` — no serialization, no round-trip per BFS/classifier fetch (see `docs/adr/0001`'s dated update) |
| `src/backend.rs` | `QueryBackend` trait — `run(esm, Op)` is its only method; callers build an `Op` and read the result back with `serde_json::from_value`, the same idiom everywhere (`cli.rs`, `server.rs`, `napi`) — no convenience methods mirroring individual `Op` variants, since that mirror silently drops whatever field it doesn't repeat (e.g. `referenced_by` once hardcoded `sort: RefSort::Formid`, unreachable from MCP); `LocalBackend` (in-process, no daemon) / `RemoteBackend` (HTTP client to the warm daemon; `/op` responses read with no size ceiling via `read_json_unlimited`, and a large `Op::RecordBulk` is transparently split across multiple round-trips via `run_bulk_chunked`, `ESM_BULK_CHUNK`); `post_op` retries a per-attempt timeout (`ESM_OP_ATTEMPT_TIMEOUT_SECS`, default 20s) for as long as `building_progress` (which canonicalizes via `watch_path`/`discover::resolve_esm_path` before calling `progress::read` — a raw folder/relative `--esm` input must resolve to the same path the daemon's build lease used, or a folder-shaped `--esm` against a cold daemon times out instead of showing progress) shows a live, non-stalled build, up to the overall `ESM_OP_TIMEOUT_SECS` budget (default 300s) — turns the old opaque single-timeout failure into a self-explaining wait; daemon lifecycle — spawn/stop, staleness detection via `exe_sig`/`daemon_fresh` |
| `src/tree.rs` | GRUP tree arena (`TreeIndex`); `GroupNode`, `RecordStub`, `GroupLabel` enum |
| `src/schema.rs` | Serde model for `schema/fo76.json`; `MemberDef` enum (18 variants, `#[serde(tag="kind")]`); `load_embedded()` |
| `src/decode.rs` | Schema-driven decoder → `serde_json::Value`; `DecodeContext<'a>`, `FormIdRefResolver` trait; never panics |
| `src/ctda.rs` | CTDA condition decoder; function-index table (binary search); imports `crate::decode::{hex, resolve_formid}` |
| `src/diff.rs` | `diff_databases_with(a,b,opts)` — byte-equality fast-path, sparse `{from,to}` JSON diff; per-element array diffs (`_array_diff`) in one of four strategies — `keyed`/`positional`/`set`/`unkeyed` (`element_key_spec` is the sole owner of element identity, see `docs/adr/0005-element-identity-owned-by-rust.md`; `unkeyed` is a deliberate classification for arrays with no stable per-element identity, e.g. CTDA `Conditions[]`, not a fallback of last resort — whole `removed`/`added` element lists, never a bare `{from,to}` leaf); `DiffOptions` (bodies for all added/removed records, placement/CELL noise suppression, type exclusion); `ref_names` sidecar |
| `src/wildcard.rs` | Case-insensitive `*`-wildcard matcher; has rustdoc doctest |
| `src/lib.rs` | `Database` facade (all public API); embeds `Index` by value plus `esm`/`schema`/`localization`/`curves` (all `pub(crate)` — only `is_localized` stays `pub`, read directly by `cli.rs`'s `diff` command across the bin/lib crate boundary); owns the three lazy index builders (`ensure_edid_index`/`ensure_search_index`/`ensure_xref_index`, see `docs/adr/0006`) and the ensure-then-get wrappers (`xref_lookup`, `resolve_edid_indexed`, `filter_cache_entries`) that are the only way this crate reads a lazy index's data — each guarantees the matching `ensure_*` ran first, so there is no code path left that can silently read "not built yet" as "genuinely absent"; `effective_take` is the one `limit == 0` = unlimited helper every paginated query method shares; `DatabaseResolver` (depth-limited FormID expansion to 2 levels) |
| `src/chase.rs` | The mechanism classifier core and the pipeline's JSON-only evidence contract (see `docs/adr/0001`): classifies an OMOD's `Data.Properties[]` rows into direct-property/perk-grant/keyword-hook mechanisms — each `Hop`'s `resolution: FetchDirection` (`Forward`/`Reverse`) records which fetch direction actually resolved it, since `HopKind::DirectProperty` alone conflates a plain direct SPEL/ENCH/PROJ attachment (forward-fetched) with an AV hook (reverse-chased like a keyword hook); evidence is path-sliced to the gated `Effects[N]` rows — or walks a PERK/SPEL/ALCH/ENCH root's own `Effects[]`; `esm chase` always emits the `ChaseTree` JSON (frozen chase.py-compatible shape) and hard-errors on other root types; owns no rendering — `walk/render.rs` is the only place that turns this module's classified data into text (`summarize_effect`/`fmt_stub` live there, not here, since chase's own JSON path never called them) |
| `src/lvli.rs` | Leveled-item (LVLI) drop-probability engine consumed by `walk/mod.rs`'s LVLI digest and, standalone, by `Op::DropTable`/the `esm_lvli_drop_table` MCP tool/the N-API `lvliDropTable` method (see `docs/adr/0001`'s "one classifier core" shape): `drop_table` recurses a `Leveled List Entries` tree via `ChaseFetcher` into per-leaf `DropRow`s (`expected_count`/`p_at_least_one`), implementing pool (exact subset enumeration up to 16 entries, then a flagged mean-field fallback) / `Use All` / `Use First Match` selection, flat-vs-GLOB-vs-Curve-Table `Chance None` resolution (same precedence for `Quantity`; deliberately NOT applied to `Minimim Level Curve Table` — spot-checked data shows that axis isn't player level), the form_version 174 `Base Data` legacy entry bridge, and a cycle guard; anything unmodeled (`Filter Keyword Chances`, `Epic Loot Chance`, list-level `Max *`, COED) surfaces as a `DropNote::Unresolved` on the affected rows rather than silently being ignored |
| `src/walk/` | The interactive surface, split compute (`mod.rs`) / render (`render.rs` submodule) — the same shape `decode.rs`/`decode/vmad.rs` already uses in this crate. `mod.rs`'s `walk` does a BFS over `ChaseFetcher` (same seam as `chase.rs`) and computes one typed `Digest` per visited record (`Glob`/`Avif`/`Kywd`/`Mgef`/`MagicItem`/`Perk`/`Weap`/`Proj`/`Expl`/`Lvli`/`Omod`/`Generic`) — real values (FormID ref stubs, numbers, classified `chase::Hop`s), never pre-formatted lines, so `--json` serializes computed data directly. `render.rs`'s `render_digest`/`render_text` are the only place that turns a `Digest` into printed text; an OMOD root's `Digest::Omod` embeds `chase.rs`'s classified `Hop`s directly (a directly-attached ENCH or PROJ property renders through the same `DirectProperty` path as any other forward attachment and is enqueued into the BFS from that same classification, not a separate re-scan), rendered as path-sliced evidence rows (bounded by `--ref-limit`) with `Data.Includes[]` stubs named for `_PARENT_` shells; an LVLI root's digest wraps `lvli.rs`'s resolved `DropTable` verbatim (`--level` feeds it) and enqueues each direct sublist entry as its own BFS node; `build_refs_digest` groups a `--refs` reverse-reference summary by record type |
| `src/bin/cli.rs` | Thin clap CLI: `info`, `get`, `list`, `search`, `refs` (`--depth N` recursive walk; `--entry-point`/`--ep <name\|id>` seeds the walk from every PERK carrying a given "Entry Point" instead of one FormID/EditorID; `--omod-property`/`--prop <[scope:]name-or-id>` seeds from every OMOD declaring a given property), `chase` (JSON-only; `--depth`/`--ref-limit`), `walk` (`--refs`/`--depth N`/`--ref-limit N`/`--level N`/`--json`), `tree`, `diff`, `coverage`, `skill` (`--install`/`--dir`/`--force`), `daemon {start,stop,status}`, `cache status [--json]`; every subcommand is one-shot and daemon-backed by default, `--local` forces cold in-process; ESM path from global `--esm`/`FO76_ESM_PATH` (except `diff`, which keeps two positional paths, and `skill`/`daemon`/`cache`, which take none or resolve their own); global `--no-wait` bails (status 75) instead of blocking on an in-flight cache build; `impl QueryBackend for Backend::run` wraps every query in a `progress_ui::Watcher` (`src/bin/cli/progress_ui.rs`, loaded via `#[path]` so it isn't autodiscovered as its own binary) that renders `progress::read`'s heartbeat to stderr |
| `src/bin/server.rs` | Axum HTTP + MCP-stdio server (feature `server`); nine read-only MCP tools: `esm_file_info`, `esm_search`, `esm_get_record` (supports `resolve=none\|stub\|full`, default `stub`), `esm_list_groups`, `esm_list_records`, `esm_refs` (depth-bound BFS reverse walk, default depth=1, max 8, 0=unbounded), `esm_walk`/`esm_chase`/`esm_lvli_drop_table` (proxy to `Op::Walk`/`Op::Chase`/`Op::DropTable`); `--daemon` mode with idle-TTL watchdog (`ESM_DAEMON_IDLE_SECS`); the MCP-stdio `initialize` response carries a condensed `instructions` string (`MCP_INSTRUCTIONS`) as ambient gotcha context for every client |
| `skills/esm-cli/SKILL.md` | Hard-won `esm`-CLI usage-knowledge doc, embedded at compile time into `cli.rs` (`include_str!`, same pattern as `schema/fo76.json`); `esm skill` prints it, `esm skill --install [--dir <repo>] [--force]` writes it into a consumer repo's `.claude/skills/esm-cli/`; this repo's own `.claude/skills/esm-cli` is a symlink to this directory |
| `bindings/napi/src/lib.rs` | N-API class `EsmDatabase` (`Arc<Mutex<Database>>`); async: `open_database`, `record_by_edid`, `record_by_id`, `referenced_by`, `referenced_by_id`, `walk`, `chase`, `lvli_drop_table`; sync: `file_info`, `list_groups`, `list_type_records`, `record_by_formid` |

The Electron GUI ("FO76 ESM Viewer") that consumes this addon lives in the sibling `../esm-viewer/` directory, not in this crate — see [`../esm-viewer/CLAUDE.md`](../esm-viewer/CLAUDE.md).

Public API re-exported from `lib.rs`: `Database`, `FormId`, `ResolveDepth`, `DiffResult`, `RecordDiff`, `RecordResult`, `ListEntry`, `GroupNode`, `TreeIndex`, `DatabaseResolver`, `parse_form_id_input`, `RefList`, `RefRow`, `RefPathNode`, `EntryPointSpec`, `EntryPointRef`.

## Conventions to Follow

- **Error handling**: `anyhow::Result<T>` everywhere (lib, CLI, napi). `bail!` for validation, `.context()`/`.with_context()` for context. **No custom error enum** — don't add enums unless the public API requires callers to `match` on variants (which would mean taking on a `thiserror` dependency; it is deliberately not declared).
- **Serialization**: manual little-endian byte reads (`u*::from_le_bytes`, `byteorder::ReadBytesExt`) for fixed headers; `serde`/`serde_json` for output; zero-copy `rkyv` sections (`src/rkyvcache.rs`) for the index cache. No `binrw`/`nom`.
- **Schema editing**: `schema/fo76.json` is embedded at compile time (`include_str!`). Change the extractor (`tools/extractor/extract.py`) or add overrides to `fo76.overrides.json` — don't hand-edit `fo76.json` directly unless fixing something the extractor can't express.
- **Decoder must never panic**: unknown/malformed bytes → raw hex fallback (`_raw`, `_unknown_record`, `_unmapped`). Do not add unwraps on untrusted input.
- **Tests**: most tests live in `tests/` (one file per module: `wildcard.rs`, `curves.rs`, `diff.rs`, `reader.rs`, `ipc.rs`, `decode_records.rs`, `decode_coverage.rs`). Tests that exercise private or `pub(crate)` symbols stay colocated in `#[cfg(test)]` blocks (`tree.rs`, `decode.rs`, `backend.rs`'s `DaemonHost`/`FakeHost` daemon-lifecycle tests, `registry.rs`'s `RegistryHost`/`FakeHost` cache-policy tests). All tests use synthetic in-memory byte buffers — no real ESM required. Integration tests that need game data skip silently when the relevant env var is unset (see `tests/diff.rs`, `tests/decode_coverage.rs`).

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

## N-API Binding and Electron App

The `bindings/napi/` sub-crate (`esm-napi`) builds a `esm-napi.<platform>.node` addon. The Electron app is now at `../esm-viewer/` (sibling directory of `esm/`, tracked separately at repo root) and depends on it via the `@fo76/esm-napi` npm package (local file dep, `"file:../esm/bindings/napi"`). After any Rust API change that affects `EsmDatabase`, rebuild the addon:

```sh
cd bindings/napi && bun run build   # or build:debug
```

The app loads the addon via `esm-viewer/src/main/addon.ts`. Most of the Rust N-API DTOs are mirrored to TypeScript via `ts-rs` (dev-dependency; `#[cfg_attr(test, derive(ts_rs::TS))]` + `#[cfg_attr(test, ts(export))]` on the DTOs in `lib.rs`/`ipc.rs`/`reader.rs`/`tree.rs`/`diff.rs`/`decode.rs`) — run `just gen-types` after changing any of those structs' shape, which regenerates `esm-viewer/src/shared/generated/*.ts`; `just check` fails if that regen produces an uncommitted diff. `esm-viewer/src/shared/api-types.ts` re-exports those generated types (aliasing a few names) and hand-writes only the IPC-contract-specific bits (`CH` channel names, `Fo76Api`, `FilterOp`) — keep *that* in sync when adding/removing `EsmDatabase` methods.

## Game Data

Game data files (`*.esm`, `*.ba2`, and `Index`'s shared `esm_cache/` directory holding its five rkyv cache sections `tree`/`forms`/`edid`/`search`/`xref`, plus `progress.rs`'s `.build.lock`/`.build.json` sidecar files) are **gitignored, non-redistributable**. Never commit them; never hardcode their paths in source — always passed at runtime via `--esm`/`FO76_ESM_PATH`/`Database::open(path)`.

## Bulk / sweep workflow (for agents)

AI agents that scan many records must avoid cold per-record process spawns. Each cold `esm --local get` invocation maps `Index`'s rkyv cache sections fresh (measured warm on the 20260724 snapshot: ~0.08 s / ~120 MiB, per `README.md`) and then exits — round-trip and decode overhead still add up across a large sweep, so the daemon (below) stays the right call for bulk work.

The ESM path itself comes from `--esm <PATH>` (works before or after the subcommand) or, if omitted, the `FO76_ESM_PATH` environment variable — set once (see `CLAUDE.local.md`) and every example below can drop the path entirely. `diff` is the exception: it always takes two explicit positional paths and ignores `--esm`/`FO76_ESM_PATH`.

### Recommended: warm daemon (fastest, on by default)

Build `esm-server` once; every subsequent call is daemon-backed automatically:

```sh
# Build both binaries (server must be alongside esm for auto-spawn to work)
cargo build --release --features server

# The first call auto-spawns the daemon; subsequent calls are fast HTTP round-trips
esm get 0x463F --pretty
esm get AssaultRifle --pretty
```

The daemon warms the index once on first load and serves all subsequent lookups in memory. It self-manages:
- **Auto-spawns** on the first call (no manual `daemon start` needed).
- **Auto-shuts-down** after 10 min idle (`ESM_DAEMON_IDLE_SECS=0` to disable).
- **Stale-evicts** if the ESM changes on disk — no manual restart needed.
- **Rebuild-evicts** if the `esm-server` binary itself changes on disk (new schema, new decode logic, any `cargo build`) — a call against a stale-but-alive daemon stops it and respawns a fresh one before serving the request, and the daemon's own watchdog self-evicts within ~30s even with no client polling it. No manual `daemon stop` needed after a rebuild.
- **Parallel-agent safe** — advisory spawn-lock (`esm-daemon.lock`) prevents double-spawn; multiple agents share one daemon instance.
- **No response-size ceiling** — `/op` responses are read with no size limit, so a large bulk `get`, an unbounded `refs --limit 0`, or a wide `search`/`list` never hits an artificial cap. A bulk `get` over more than `ESM_BULK_CHUNK` selectors (default 512, `0` disables) is transparently split across multiple round-trips and reassembled — invisible to callers, just smooths daemon peak memory per request.

Use `esm daemon status` to check (includes a `binary_current` flag — `false` means a rebuild happened and the daemon is about to self-evict/respawn), `esm daemon stop` to kill early.

### Use `--resolve stub` to avoid follow-up lookups

Any record containing FormID references (COBJ, NPC_, WEAP, …) returns raw hex FormIDs by default. Pass `--resolve stub` to annotate every reference inline with `editor_id` and `record_type` in a single call — no follow-up `get` calls needed:

```sh
# Without --resolve: components are raw FormIDs → requires N follow-up gets
esm get 0x008B33D7 --pretty

# With --resolve stub: all references annotated inline in one call
esm get 0x008B33D7 --resolve stub --pretty

# --resolve full recursively expands references to their complete decoded record
esm get 0x008B33D7 --resolve full --pretty
```

Default to `--resolve stub` when the record you're reading is reference-heavy (recipes, NPCs, leveled lists, quests). Use `--resolve full` only when you need the complete sub-record data. Bare `get` is fine only when you specifically want raw FormID values.

### Prefer bulk ops over N single gets

Every round-trip has overhead. When you need many records of the same type, use bulk ops:

```sh
esm list --type WEAP --limit 500 --pretty       # all weapons in one call
esm search "*Rifle*" --type WEAP --pretty       # search by name/EditorID
esm refs 0x463F --limit 100 --pretty            # direct reverse lookup (depth=1)
esm refs 0x463F --depth 8 --pretty              # recursive walk to depth 8
esm coverage --type WEAP                        # schema decode audit
```

### Selectors: FormID vs EditorID vs Auto

A bare positional selector with no `0x` prefix that still *looks* like a FormID (e.g. `18000`) resolves as `RecordSel::Auto`: the FormID interpretation is tried first, with an EditorID lookup as fallback. An explicit `0x`-prefixed token, or an explicit `--formid`/`--edid` flag, skips the ambiguity entirely and never becomes `Auto`.

### Gotcha: capped-output notes print to stderr, not stdout

`list`, `search`, and `refs` print a `note: output capped at N of M results; use --limit 0 to show all` line when the result count hits `--limit` — always to **stderr**, never stdout, so `--json` output stays valid, parseable JSON even when the result was capped. Pass `--limit 0` when you need the uncapped result instead of relying on stderr to notice truncation.

### Gotcha: `--localization-ba2` / `--startup-ba2` bypass the daemon

Passing `--localization-ba2`, `--strings-dir`, or `--startup-ba2` to `get` forces a cold in-process open (the daemon doesn't load BA2 args from per-call flags). For sweeps that need localized strings, place the Localization BA2 (or a `strings/` folder) and the Startup BA2 (or a `misc/curvetables/` folder) next to the ESM — the daemon auto-loads them on open, and warm lookups return localized output without per-call BA2 flags.

### MCP opt-in (for AI clients that support it)

`esm-server --mcp-stdio` speaks JSON-RPC 2.0 over stdin/stdout. Wire it up in your AI client's MCP config — **do not commit** the config file (it hardcodes a date-stamped, non-redistributable ESM path):

```jsonc
// .mcp.json (gitignored — fill in your actual ESM path)
{
  "mcpServers": {
    "fo76-esm": {
      "command": "/path/to/esm-server",
      "args": ["--mcp-stdio", "/path/to/data"]
    }
  }
}
```

The server exposes nine read-only tools (all proxy to the warm daemon): `esm_file_info`, `esm_search`, `esm_get_record` (supports `resolve=none|stub|full`, default `stub` — references are annotated with EditorID+name inline), `esm_list_groups` (type inventory / table of contents), `esm_list_records`, `esm_refs` (depth-bound BFS reverse-reference walk; default `depth=1` for a single-level lookup, up to `depth=8` — or `depth=0` for unbounded — to walk the full reference graph — use this for "where does X drop?" questions), `esm_walk` (interactive per-record-type digest — the primary "what does this do" tool, computed server-side), `esm_chase` (the same mechanism classification as a fixed-shape JSON pipeline contract), `esm_lvli_drop_table` (resolved LVLI drop-probability table). Each `esm_refs` result includes a hop `depth` and an intermediate-node `path` array. Under the hood MCP-stdio proxies to the same HTTP daemon, so the warm-index benefit applies automatically.

## Coverage drift handling (vs TES5Edit)

Drift subrecords newer than the TES5Edit reference are handled as follows:

- **LVLI/LVLN/LVPC/LVLP `LVLD`**, **RESO `NAM5`**, **NPC_ `AWPB`+`CTDA`**, **GMRW `XALG`**, **STAT `SNAM`+`ANLD`**, **REFR `MCND`** — mapped in `schema/fo76.overrides.json` (GMRW XALG expands from `$pascal_var: wbXALG`, u64 legendary flags; REFR MCND is an rarray-of-unknown, in no TES5Edit definition at all).
- **CTDA function table** — generated to `schema/fo76.ctda.json` from Pascal; loaded at runtime in `src/ctda.rs`.
- **EFIT**, **Model Information**, **CTDA** — schema kinds (`struct` / `model_info` / `ctda`); no magic-string dispatch in `decode.rs`.
- **QUST `VMAD` (fragmented)** — `decode_vmad_qust` in `src/decode.rs` handles Script Fragments + Aliases tail.
- **INFO/PACK/PERK/SCEN `VMAD` (fragmented)** — `decode_vmad_{info,pack,perk,scen}` in `src/decode.rs` handle each record type's Script Fragments tail; dispatched by `ctx.record_signature`.
- **NPC_ `VMAD` type-0/type-7 properties** — `decode_vmad_property` handles type 0 (None → null) and type 7 (Struct → named-member array). NPC_ is now in `CLEAN_TYPES`.

## Interpreting game data: live vs. cut/deferred content (for agents doing lookups)

This is guidance about the *game data itself*, not the codebase — it matters whenever an agent uses `esm` to answer "what does X do in the current game."

FO76's EditorIDs use informal prefixes to mark content that isn't part of the live game:

- **`zzz_`** — deprioritized/superseded. Usually an older implementation of a perk/effect that has been reworked; the unprefixed sibling (if any) is the live one.
- **`CUT_`**, **`DEL_`**, **`deprecated_`** (or similar) — cut content, never shipped or removed.
- **`POST_`** — deferred/not-yet-released content (future update material sitting in the current ESM).
- **`zzz_Babylon_*`** — an internal test-branch duplicate, not the live record.

**Do not treat these as ground truth for "what does the game currently do."** Only use them when the task is explicitly historical/comparative: diffing snapshots, tracing how a mechanic evolved, or investigating cut content on request.

**The prefix is a heuristic, not proof — the naming convention is inconsistent.** Several currently-dead PERK ranks have *no* prefix at all (e.g. `BearArms02`/`BearArms03`, `TankKiller03` — plain names, but orphaned). Conversely, some unprefixed records are simply broken/vestigial (e.g. `BearArms01` has a description string copy-pasted from an unrelated perk and carries no `Conditions`/`Effects` at all). Naming alone is not sufficient to confirm a record is live.

**Authoritative check for perks specifically: does a `PCRD` (Perk Card) record reference it?** A `PERK` rank is only actually reachable by a player if some `PCRD`'s `Perks` array lists it. Verify with:

```sh
esm refs <perk-formid> --limit 20   # look for a PCRD in the results
esm get <pcrd-formid> --resolve stub --pretty   # inspect its Perks[].Perk["Male Perk"] list — only these ranks are live
```

A `PERK` record with no referencing `PCRD` (e.g. `Deadeye01/02`, `Bandito01`) is orphaned — the record decodes fine and may even have `Playable: true`, but nothing in the game ever grants it to a player. A `PCRD` whose `Perks` array stops at rank N means ranks N+1 onward (even if unprefixed, even if not `CUT_`) are dead. Other record types don't have as clean an authoritative signal as `PCRD` — for those, the prefix heuristic above is what's available; flag uncertainty rather than asserting liveness.
