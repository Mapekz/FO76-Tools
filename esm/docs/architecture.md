# Architecture

How raw `SeventySix.esm` bytes become decoded JSON, and how the four consumer surfaces (CLI,
HTTP/MCP server, N-API addon, Python patch-notes pipeline) all reach the same engine. The
decisions this map rests on are recorded in `docs/adr/`; domain vocabulary is in
`../CONTEXT.md`.

`esm` is **read-only by design** — it inspects, decodes, and diffs `.esm` files, never writes
one. Every write this crate performs targets its own disk cache (`esm_cache/`), never the
source ESM.

## Record read flow

```
SeventySix.esm (mmap)
  │  src/reader.rs   EsmFile::open, walk_records / walk_structure, parse_subrecords
  │                  (the XXXX oversized-subrecord rule), parse_record_at
  │  src/compress.rs decompress_zlib (per-record), decompress_record_data
  ▼
Vec<OwnedSubrecord>   — one raw (signature, bytes) pair per subrecord
  │  src/decode/mod.rs  decode_record(ctx, signature, subrecords)
  ▼
serde_json::Value   — one JSON object per record
```

`decode_record` looks up the record's shape in `ctx.schema` (`src/schema.rs`'s `Schema`,
loaded once via `Schema::load_embedded()` from the compiled-in `schema/fo76.json`) and walks
its `MemberDef` tree with `decode_member`/`decode_struct_fields`, dispatching per field kind:
scalars and arrays inline in `decode/mod.rs`, `VMAD` script-attachment blobs to
`src/decode/vmad.rs` (`decode_vmad`, plus type-specific `decode_vmad_{qust,info,pack,perk,scen}`
for each record type's Script Fragments tail), and `CTDA` condition blocks to `src/ctda.rs`'s
`decode_ctda`, which looks up the condition function by index in a compiled-in table
(`schema/fo76.ctda.json`) and decodes each parameter by its class character. After a record's
fields are in, `src/decode/rules.rs`'s `apply_post_decode_rules` runs a small, named set of
FO76-specific post-passes over the assembled map — `apply_crafting_quantity` (struct-level,
resolves a component's `Count` + `Curve Table` into an effective `Quantity`) and
`apply_weapon_bash_curve` (record-level, WEAP only, synthesizes `Bash Damage` from `Damage
Curve` + `Secondary Damage`).

The decoder **never panics**: unknown record types get `_unknown_record: true`, unmapped
leftover subrecords land under `_unmapped`, malformed bytes fall back to `_raw` hex, and an
LString whose ID has no match in the loaded string tables gets `_unresolved` — the four marker
keys are the single source of truth in `decode::markers` and are what the `coverage` subcommand,
MCP server, and patch-notes tooling all key off of.

**Where the embedded schema comes from**: `schema/fo76.json` is a build artifact, not
hand-written. `tools/extractor/extract.py` reads the sibling `../TES5Edit` checkout's Pascal
record definitions (`Core/wbDefinitionsFO76.pas`, `Core/wbDefinitionsCommon.pas`) and emits
`schema/fo76.json` plus `schema/fo76.ctda.json` (the CTDA function table) and consults
`schema/fo76.overrides.json` for subrecords TES5Edit doesn't define at all (see CLAUDE.md's
"Coverage drift handling" table — LVLI `LVLD`, REFR `MCND`, etc.). `tools/extractor/audit.py
--gate` is the parity gate: it fails the build when decode coverage regresses against the
Pascal source. Fix decode coverage by changing the extractor or the overrides file — never by
hand-editing the 2.3 MB generated JSON.

## Index & cache lifecycle

`Database::open(path)` (`src/lib.rs`) mmaps the ESM, loads the schema, and eagerly builds
`Index`'s `tree`/`forms` sections. Three further sections are lazy — built on first use, not at
open:

```
Database (src/lib.rs)
  ├─ esm, schema, localization, curves           (mmap'd / loaded once)
  └─ index: Index (src/index.rs)
       ├─ tree     — GRUP arena, built eagerly by Index::build
       ├─ forms    — FormID → offset,  "
       ├─ edid     — EditorID → FormID,  ensure_edid_index()    (lazy, on Database)
       ├─ search   — name/EditorID search index, ensure_search_index()  (lazy, on Database)
       └─ xref     — reverse-reference graph, ensure_xref_index()  (lazy, decodes every record)
```

`Index` itself only holds the five `Section<...>` fields and the pure reads over them
(`get_by_formid`, `records_by_type`, `tree()`, …); the three `ensure_*_index` **build** methods
live on `Database` instead, because building a section needs the mmap'd ESM, schema,
localization, and curves that only `Database` holds — `Index` and `Database` share one
lifecycle (ADR 0006).

Each section is a zero-copy `rkyv`-archived blob (`src/rkyvcache.rs`'s `Section<A>`), mmap'd
back on later opens instead of re-decoded. Sections live in a shared `esm_cache/` directory
(`rkyvcache::cache_dir_for`), one file per `(esm file name, section)` pair
(`rkyvcache::section_path_for`). A section is invalidated by either a crate-wide
`index::CACHE_VERSION` bump (currently `15`) or its own per-section `LAYOUT_FINGERPRINT` — the
`SectionSpec` trait (ADR 0007) binds a section's `SectionKind`, fingerprint, and archived type
together in one `impl` next to the type itself, so a kind/fingerprint mismatch is no longer
expressible as a silent bug. Every section build goes through
`progress::BuildLease::acquire_or_recheck` (ADR 0007) — acquire the per-ESM advisory lock, then
re-check whether another process already finished the same section before doing any real work —
so the lock doubles as cross-process dedup, not just coordination.

Cross-process visibility into a build in flight is a **filesystem protocol, not a daemon
endpoint** (ADR 0003): `src/progress.rs` publishes an atomically-written `.build.json` heartbeat
next to a `.build.lock` advisory lock, both siblings of the five rkyv sections inside
`esm_cache/`. `progress::read` is instant and never blocks (`try_lock_exclusive`, one syscall),
so every caller — the daemon, a `--local` CLI process, the N-API host, or `tools/esm_gateway.py`
— can answer "is someone already building this ESM" uniformly, without needing to be the one
holding the build.

`src/registry.rs`'s `Registry` sits above all of this for the daemon/N-API path: it lazily
opens and caches exactly one warm `Database` per canonical ESM path, evicting on a stale
`FileSig` (path/size/mtime) so a snapshot swap is picked up automatically. `src/discover.rs`
resolves what path/sources that canonical identity actually is —
`resolve_esm_path` turns a folder or relative input into one canonical `.esm` path, and
`resolve_sources` locates the sibling `strings/`/curve-table sources (loose files or BA2) next
to it.

## Process topology

Four surfaces read the same engine through one dispatch point, `src/ipc.rs`'s `dispatch_op`
(driven by the `Op` enum — `Op::Record`, `Op::Search`, `Op::Walk`, `Op::Chase`,
`Op::DropTable`, …):

```
                         src/ipc.rs  Op + dispatch_op
                                   │
      ┌───────────────┬───────────┼────────────────┬──────────────────────┐
      ▼               ▼           ▼                 ▼                     ▼
 CLI (bin/cli.rs)  --local     daemon (bin/server.rs)   bindings/napi        tools/esm_gateway.py
   Backend::run       in-proc     HTTP + MCP-stdio       EsmDatabase           (HTTP client,
   (Local/Remote)                Registry-backed         Arc<Mutex<Database>>  same wire format)
```

`src/backend.rs`'s `Backend` enum (`Local(LocalBackend)` / `Remote(RemoteBackend)`) has one
method, `run(esm, Op) -> Value` — every caller builds an `Op` and reads the result back with
`serde_json::from_value`, so there's no convenience-method surface that can silently drop a field
an `Op` variant carries. A plain enum match, not a trait, since the two backends are a closed set
with no trait-object or generic-bound caller. `LocalBackend` runs `dispatch_op` in-process against
a cold `Database::open`; `RemoteBackend` posts the same `Op` as JSON to a daemon's `/op` endpoint.
`src/bin/cli.rs` wraps this enum in its own `Backend` newtype to layer the progress-UI watcher
around every call.

`src/bin/cli.rs`'s `main()` decides which backend a subcommand gets purely from `--local` and
the global `--esm`/`FO76_ESM_PATH`/`--addr`/`--port` flags (`make_backend`) — every subcommand
except `diff` (two positional ESM paths), and `skill`/`daemon`/`cache` (need no `Backend` at
all), resolves one ESM path and one backend, then calls `dispatch_command`. Daemon mode is the
default: `RemoteBackend::connect_or_spawn` health-checks the daemon info file the OS runtime
directory holds (`esm-daemon.json`, via `backend::daemon_info_path`), and if nothing answers,
spawns the `esm-server` binary
found as a sibling of the running `esm` executable (`esm_server_exe`), coalesced by an advisory
spawn lock so concurrent callers don't double-spawn. `daemon_fresh`/`exe_sig` detect a stale
daemon (binary changed since it started, e.g. a rebuild) and force a stop-then-respawn
transparently — no manual `daemon stop` needed. Every `Backend::run` call is wrapped in a
`progress_ui::Watcher` (`src/bin/cli/progress_ui.rs`) that renders the build heartbeat
(`progress::read`) to stderr after a grace period, so a cold build shows visible progress instead
of looking hung.

`src/bin/server.rs` is an Axum HTTP server plus an MCP-stdio mode (`--mcp-stdio`, feature
`server`), both backed by one `Registry`-cached `Database`. It exposes nine read-only MCP tools
(`esm_file_info`, `esm_search`, `esm_get_record`, `esm_list_groups`, `esm_list_records`,
`esm_refs`, `esm_walk`, `esm_chase`, `esm_lvli_drop_table`), all proxying to the same `Op`
dispatch the CLI uses. `--daemon` mode adds an idle-TTL watchdog (`ESM_DAEMON_IDLE_SECS`) that
self-exits when nothing has queried it recently.

`bindings/napi/src/lib.rs`'s `EsmDatabase` (an `Arc<Mutex<Database>>`) is the fourth surface: it
opens its own in-process `Database` (via a throwaway `Registry`, so it shares the same
stale-eviction logic) rather than talking to a daemon over HTTP — the Electron app in
`../esm-viewer/` is a single long-lived process, so there's no separate process boundary to
cross.

**Source overrides are CLI-only** (ADR 0008): `--localization-ba2`/`--strings-dir`/
`--startup-ba2`/`--curves-dir` on `list`/`get`/`search`/`diff` force a cold in-process
`Database::open` (via `bail_if_daemon_mode_overrides`, one shared guard those four commands
call) instead of going through the daemon — the daemon's `Registry` caches one shared `Database`
per path, and a per-request override has no coherent way to join that shared instance.

## Feature layer

Beyond record decode, five modules implement FO76-specific analysis over an already-open
`Database`:

**`src/diff.rs`** — `diff_databases_with(a, b, opts)` compares two `Database`s: a
byte-equality fast path per record, then a sparse `{from, to}` JSON diff (`json_diff`) for
records that changed. Every array field runs through `array_diff`, which picks one of four
pairing strategies: `keyed` (paired by an identity `element_key_spec` proposes from a sample
element — composing every FormID-shaped member, or a handful of named heuristics like quest
alias IDs), `positional`, `set`, or `unkeyed` (an order-preserving LCS alignment,
`lcs_align`, reporting only what falls outside it, plus an `unchanged_count`). A proposed key is
never trusted on the sample's shape alone — `widen_key_spec_until_unique` appends further scalar
fields until the key is actually unique on both sides, falling back to `unkeyed` if nothing
achieves that (ADR 0005). Arrays with no stable per-element identity classify as `unkeyed`
outright — CTDA `Conditions[]` is the canonical case, fenced by ADR 0005.
`strip_noise_fields` suppresses placement-transform/CELL-precombine/Object-Bounds
churn by default (`--keep-noise` on the CLI disables it).

**`src/walk/`** — the sole interactive/human-readable surface (ADR 0001), split compute
(`mod.rs`) from render (`render.rs`), the same shape `decode/vmad.rs` uses. `walk::walk` does a
BFS over a `ChaseFetcher` and computes one typed `Digest` per visited record — `Glob`, `Avif`,
`Kywd`, `Mgef`, `MagicItem`, `Perk`, `Weap`, `Proj`, `Expl`, `Lvli`, `Omod`, or `Generic` —
carrying real values (FormID stubs, numbers, classified `chase::Hop`s), never pre-formatted
strings, so `--json` output is exactly the computed data. `render.rs`'s `render_digest`/
`render_text` are the only place a `Digest` becomes printed text. An OMOD root's digest embeds
`chase.rs`'s classified mechanisms directly, sliced to just the gated evidence rows.

**`src/chase.rs`** — the mechanism-classifier core, and `chase`'s JSON-only, machine-facing
contract (ADR 0001). `chase::chase` classifies an OMOD's `Data.Properties[]` rows into
direct-property, perk-grant, or keyword/AVIF-hook mechanisms; each `Hop` records a `HopKind`
(`DirectProperty`, `TagKeyword`, …) and a `FetchDirection` (`Forward`/`Reverse`) noting which
direction actually resolved it. The output `ChaseTree` is a frozen, additive-only JSON shape —
new optional fields and enum variants are fine, renames and removals are not, without a new ADR.

**`src/lvli.rs`** — `lvli::drop_table` recurses a Leveled List's `Entries[]` tree via the same
`ChaseFetcher` seam, computing per-leaf `DropRow`s (`expected_count`, `p_at_least_one`) under
`Use All` / `Use First Match` / pool (exact subset enumeration up to 16 entries, else a flagged
mean-field approximation) selection and flat/GLOB/Curve-Table `Chance None` resolution.
Anything the model doesn't cover (`Filter Keyword Chances`, `Epic Loot Chance`, list-level `Max
*`, COED) surfaces as a `DropNote::Unresolved` on the affected row rather than being silently
dropped.

**`src/refs.rs`** — the reverse-reference graph engine: `referenced_by_enriched`/
`_multi` (BFS from one or more seeds) and `find_ref_path` (bidirectional path search between two
records). `RefSeeds` resolves a CLI selector to BFS seeds along exactly two shapes (ADR 0004):
**Direct** (a FormID, a real EditorID, or an engine-hardcoded EditorID from `src/hardcoded.rs`'s
~228-entry table of FormIDs the game executable defines but no ESM record does — e.g. AVIF
`KillStreak`) resolves to one seed; **Carriers** (`--entry-point`/`--ep`, `--omod-property`/
`--prop`) resolves to every matching record, each emitted as a `depth: 0` seed row. Only a
Carriers namespace with zero measured EditorID collisions (entry points) earns bare positional
auto-detection; a colliding namespace (OMOD properties — `Health` collides with a real AVIF)
stays behind an explicit flag permanently.

**`src/strings.rs`/`src/curves.rs`/`src/hardcoded.rs`** are the support data these all read
through: `Localization` (`StringTable`, loaded via `from_ba2` or `from_loose_files`) resolves
LString IDs to display text; `CurveIndex` maps a FormID to a `Curve` and `Curve::eval` does the
linear interpolation used by crafting quantities, weapon bash damage, and LVLI counts alike;
`hardcoded.rs` is the small lookup both `decode`'s FormID resolver and `refs`'s Direct-selector
resolution fall back to on an index miss.

## Python mechanical pipeline (`esm/tools/`)

The patch-notes pipeline has a **mechanical stage** (deterministic Python, no LLM) and a
**narrative stage** (the repo-root `/patch-notes` skill). The mechanical stage runs as `just
patch-notes OLD NEW`, which drives `tools/make_patch_notes.py` through a fixed order:

```
esm --local diff (subprocess)         → diff.json
  │
render_comprehensive.py  (Tool 1)     → comprehensive.json + comprehensive.md
  │   uses change_entries.py's ChangeEntry construction + array-diff reading
  ▼
build_bundles.py         (Tool 2)     → bundles.json
  │   clusters related records (weapon + mod slots + drop list + unique keyword)
  ▼
run_lints.py              (Tool 3)    → lints.json, rewrites bundles.json's lint_ids/bug_watch
  │   rule registry, consults esm_gateway.py's EsmGateway for reference-graph checks
  ▼
patchnotes_lib.py manifest helpers    → manifest.json
```

`triage_bundles.py` runs after this (also mechanical) to assign each bundle a tier — `rollout`,
`deep`, `brief`, `drop`, or `ambiguous` — against `patch_notes_tiers.json`'s rules, writing
`work/triage.json`, `work/deep-slice.json`, `work/ambiguous.json`, `work/brief-lines.md`, and
`work/rollouts.md`. `esm_gateway.py`'s `EsmGateway` is the one seam every stage above uses to
reach the `esm` CLI/daemon — `bulk_get`, `list_type`, `refs`, `diff` — so nothing else in
`tools/` shells out to `esm` directly.

The **narrative stage** takes over from `work/deep-slice.json`/`ambiguous.json` onward: the
`/patch-notes` skill (`FO76-Tools/.claude/skills/patch-notes/`, run with `FO76-Tools/` as cwd)
fans out Sonnet deep-writer agents armed with `deep-writer-prompt.md`/`style-guide.md`/`kb/`
over the DEEP tier, resolves the `ambiguous` tier with a cheap assessor pass, and assembles the
final `patch-summary.md`, chunked for Discord by `tools/discord_chunker.py` and finalized via
`tools/update_manifest.py`.

**`tools/extractor/`** is the schema side of the pipeline, not the diff side: `extract.py`
(schema generation, described above), `audit.py --gate` (the parity gate `just audit` runs),
`hardcoded.py` (emits `schema/hardcoded_fo76.json` from xEdit's hardcoded pseudo-plugin, backing
`src/hardcoded.rs`), and `pascal_stubs.py`/`parity-exceptions.json` (extractor support data).

## Where to tweak what

| Want to... | Look in |
|---|---|
| Add or fix a decoded field | `schema/fo76.overrides.json` or `tools/extractor/extract.py`, then `src/decode/mod.rs`'s `decode_member` / `src/decode/rules.rs` for any post-decode synthesis |
| Add a new CLI subcommand | `src/bin/cli.rs` (`Commands` enum + `dispatch_command`); add an `Op` variant in `src/ipc.rs` if it needs daemon/MCP/N-API reach too |
| Change diff noise suppression | `src/diff.rs`'s `strip_noise_fields` / `DiffOptions` |
| Change array-pairing behavior | `src/diff.rs`'s `element_key_spec` / `widen_key_spec_until_unique` — read ADR 0005 first, especially before touching CTDA `Conditions[]` |
| Add a new patch-notes lint rule | `tools/run_lints.py`'s rule registry |
| Change bundle clustering | `tools/build_bundles.py` |
| Change tier assignment (DEEP/BRIEF/DROP) | `tools/patch_notes_tiers.json`, `tools/triage_bundles.py` |
| Add an MCP/HTTP tool | `src/bin/server.rs` (tool list + dispatch); add the matching `Op` in `src/ipc.rs` if it's a new query shape |
| Add an N-API method | `bindings/napi/src/lib.rs`, then `just gen-types` and `cd bindings/napi && bun run build` |
| Change a cache section's on-disk shape | its `impl SectionSpec` block (next to the type, in `index.rs` or `tree.rs`) and bump `index::CACHE_VERSION` |
| Change OMOD mechanism classification | `src/chase.rs` |
| Change LVLI drop-probability math | `src/lvli.rs` |
| Change how `walk` renders a digest | `src/walk/render.rs` (never the compute side, `mod.rs`, for pure formatting changes) |
