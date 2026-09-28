# Architecture

How raw `SeventySix.esm` bytes become decoded JSON, and how the three consumer surfaces (CLI,
N-API addon, Python patch-notes pipeline) all reach the same engine. The
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
decode::node::Node  — typed value tree (FormIds, lstring ids, enums, flags stay typed)
  │  Node::into_json(ctx)  resolves FormIDs (--resolve) and lstrings while rendering
  ▼
serde_json::Value   — one JSON object per record
```

`decode_record` looks up the record's shape in `ctx.schema` (`src/schema.rs`'s `Schema`,
loaded once via `Schema::load_embedded()` from the compiled-in `schema/fo76.json`).
`decode/bind.rs` then binds the record's subrecords to its members in file order, the way
xEdit does: a record or `rstruct` keeps a cursor over its member list and each subrecord goes
to the next member at or after the cursor that can take its signature; an `rstruct` opens on
its first member's signature (any member's when the schema marks it `any_member`) and ends at
the first subrecord it can't place or after a member listed in its `terminators`; an `rarray` takes elements while its element can open on
the next subrecord; a signature-less union takes the first variant that can. Records and
rstructs marked `unordered` look members up by signature instead. The flags come from xEdit's
own definitions via the extractor. Each bound subrecord's bytes are decoded by
`decode/walk.rs` (`decode_struct_fields`, and `decode_member` for payload-level union variants
and array elements) into a `decode::node::Node` tree: leaf scalar codecs (`scalar_int`,
`scalar_formid`, …) in `decode/scalars.rs`,
self-describing Model Information blobs in `decode/model_info.rs`, `VMAD` script-attachment blobs to
`src/decode/vmad.rs` (including the Script Fragments tail whose layout the schema member's
`fragments` names), and `CTDA` condition blocks to `src/ctda.rs`'s
`decode_ctda`, which looks up the condition function by index in a compiled-in table
(`schema/fo76.ctda.json`) and decodes each parameter by its class character. After a record's
fields are in, `src/decode/derived.rs` adds the derived values, the one place fields are
computed rather than read: `apply_crafting_quantity` (struct-level, resolves a component's
`Count` + `Curve Table` into an effective `Quantity`), `apply_weapon_bash_curve` (record-level,
WEAP only, synthesizes `Bash Damage` from `Damage Curve` + `Secondary Damage`), and the curve
points inlined on a curve-table reference or a CURV record. Curve points come from the loaded
curve index, looked up by the typed FormID.

A FormID stays `Node::FormId` and a localized string stays `Node::LString` until
`Node::into_json` renders it at the caller's `--resolve` depth. Union deciders and post-passes
that read a sibling field take integers, enum values and FormIDs from the typed node; the
`FormIdTargetType` decider asks the resolver for the target's record type, so it only picks a
variant at `--resolve stub`/`full`.

The decoder **never panics**: unknown record types get `_unknown_record: true`, unmapped
leftover subrecords land under `_unmapped`, bytes that don't decode to fields fall back to
`_raw` hex with a `reason` (`decode::node::RawReason`: schema-declared unknown bytes,
unmodelled schema, an unresolved union, malformed data), a struct subrecord with bytes left
after its fields gets `_trailing`, and an LString whose ID has no match in the loaded string
tables gets `_unresolved`. The marker keys are the single source of truth in `decode::markers`;
the `coverage` subcommand counts them from the typed tree by reason, and its `--gate` fails on
every kind except schema-declared unknown bytes.

**Where the embedded schema comes from**: `schema/fo76.json` is a build artifact, not
hand-written. `tools/extractor/extract.py` reads the sibling `../TES5Edit` checkout's Pascal
record definitions (`Core/wbDefinitionsFO76.pas`, `Core/wbDefinitionsCommon.pas`) and emits
`schema/fo76.json` plus `schema/fo76.ctda.json` (the CTDA function table) and consults
`schema/fo76.overrides.json` for subrecords TES5Edit doesn't define at all (see AGENTS.md's
"Coverage drift handling" table — LVLI `LVLD`, REFR `MCND`, etc.). `tools/extractor/audit.py
--gate` is the parity gate: it applies the same overrides to the Pascal-derived tree and fails
when the shipped schema diverges from xEdit in a way no override (and its `reason`) explains. Fix decode coverage by changing the extractor or the overrides file — never by
hand-editing the 2.3 MB generated JSON.

## Index & cache lifecycle

`Database::open(path)` (`src/database.rs`) mmaps the ESM, loads the schema, and eagerly builds
`Index`'s `tree`/`forms` sections. Three further sections are lazy — built on first use, not at
open:

```
Database (src/database.rs)
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
`index::CACHE_VERSION` bump (content changes) or its own per-section `LAYOUT_FINGERPRINT` (a
layout version a golden test pins to the section's archived bytes) — the
`SectionSpec` trait (ADR 0007) binds a section's `SectionKind`, fingerprint, and archived type
together in one `impl` next to the type itself, so a kind/fingerprint mismatch is no longer
expressible as a silent bug. Every section build goes through
`progress::BuildLease::acquire_or_recheck` (ADR 0007) — acquire the per-ESM advisory lock, then
re-check whether another process already finished the same section before doing any real work —
so the lock doubles as cross-process dedup, not just coordination.

Cross-process visibility into a build in flight is a **filesystem protocol** (ADR 0003):
`src/progress.rs` publishes an atomically-written `.build.json` heartbeat next to a
`.build.lock` advisory lock, both siblings of the rkyv sections inside `esm_cache/`.
`progress::read` is instant and never blocks (`try_lock_exclusive`, one syscall), so any caller
can answer "is someone already building this ESM" without being the one holding the build.
A process can also hand its builds to another process (`progress::delegate_builds`): the CLI
runs each missing section as a detached `esm cache build`, so a build outlives the command that
started it, and `rkyvcache::map_or_build` falls back to building in-process if that fails.

`src/discover.rs` resolves what an ESM's canonical identity is — `resolve_esm_path` turns a
folder or relative input into one canonical `.esm` path, and `resolve_sources` locates the
sibling `strings/`/curve-table sources (loose files or BA2) next to it. `Database::open` does
both itself, so every caller that names the same file shares one cache and one build lock.

## Process topology

Every surface reaches the engine through `src/host.rs`'s `Host::run(esm, op)`, which keeps the
databases a process has opened (keyed by canonical path, reopened if the file changes) and runs
`ops::run` — driven by the `Op` enum (`Op::Record`, `Op::Search`, `Op::Walk`, `Op::Chase`,
`Op::DropTable`, …). `src/ops/mod.rs` declares every op once in its `ops!` list (wire tag, `Args`
struct, output type, function); the enum, the dispatcher and the TypeScript `Op`/`OpOutput` types
are generated from that list, and each op's `Args` struct and function live in a family module
(`records.rs`, `refs.rs`, `analysis.rs`, `tree.rs`, `coverage.rs`, `diff.rs`):

```
                    src/host.rs  Host::run  →  src/ops/  ops::run
                                   │
         ┌─────────────────────────┼──────────────────────────┐
         ▼                         ▼                          ▼
 CLI (bin/cli/)             esm batch (bin/cli/)        bindings/napi
   one command, one Host      one JSON request per       EsmHost.run(esm, op)
                              stdin line; used by        (async, one Host)
                              ../patch-notes/
```

Every surface runs in-process; there is no server. Opening a database maps its cache sections
without parsing them (~1.5 ms warm), so a one-shot CLI command costs a few milliseconds, and
long-lived hosts (`esm batch`, the Electron app) keep databases open between requests.
`Database`'s queries all take `&self`, so one open database serves concurrent callers without a
lock. `src/bin/cli/main.rs` wraps its `Host` in a `Backend` newtype whose `run` layers a
`progress_ui::Watcher` (`src/bin/cli/progress_ui.rs`) around every call: it renders the build
heartbeat (`progress::read`) to stderr after a grace period, so a cold build shows visible
progress instead of looking hung.

`esm batch` is how scripts make many calls cheaply: the patch-notes pipeline's `EsmGateway` owns one
`esm batch` child, sends it one `{"esm", "op"}` request per line (`ops::Request`), and reads one
`ops::Response` envelope per line back.

**Source overrides** (`--localization-ba2`/`--strings-dir`/`--startup-ba2`/`--curves-dir` on
`list`/`get`/`search`/`refs`/`diff`) open a `Database` configured with those sources for that one
command instead of going through the `Host`, whose databases always use the sources discovered
next to the ESM.

## Feature layer

Beyond record decode, five modules implement FO76-specific analysis over an already-open
`Database`:

**`src/diff/`** — `diff_databases_with(a, b, opts)` (`mod.rs`) compares two `Database`s: a
byte-equality fast path per record, then a sparse `{from, to}` JSON diff (`json_diff`) for
records that changed, split compute the same shape `decode/vmad.rs`/`walk/` use into two
self-contained submodules. `array_diff.rs` is every array field's per-element treatment —
`json_diff`'s array arm runs through `array_diff`, which picks one of four pairing strategies:
`keyed` (paired by an identity `element_key_spec` proposes from a sample element — composing
every FormID-shaped member, or a handful of named heuristics like quest alias IDs),
`positional`, `set`, or `unkeyed` (an order-preserving LCS alignment, `lcs_align`, reporting only
what falls outside it, plus an `unchanged_count`). A proposed key is never trusted on the
sample's shape alone — `widen_key_spec_until_unique` appends further scalar fields until the key
is actually unique on both sides, falling back to `unkeyed` if nothing achieves that (ADR 0005).
Arrays with no stable per-element identity classify as `unkeyed` outright — CTDA `Conditions[]`
is the canonical case, fenced by ADR 0005. Whatever the strategy, a non-empty diff of two arrays
holding the same elements in a new order (compared order-insensitively at every depth; condition
lists and the order-significant fields in `ORDER_SIGNIFICANT_FIELDS` excepted) carries
`reorder_only: true`. `noise.rs` suppresses noise in a
`changed` record's `field_changes` (off via `--keep-noise` on the CLI): `suppress_record` runs the
per-record stages and `apply_restamp_calibrated_suppression` the cross-record one, in the
load-bearing order its module docs list.

**`src/walk/`** — the sole interactive/human-readable surface (ADR 0001), split compute
(`mod.rs`) from render (`render.rs`), the same shape `decode/vmad.rs` uses. `walk::walk` does a
BFS over a `RecordSource` (`src/source.rs`) and computes one typed `Digest` per visited record — `Glob`, `Avif`,
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
`RecordSource` seam, computing per-leaf `DropRow`s (`expected_count`, `p_at_least_one`) under
`Use All` / `Use First Match` / pool (exact subset enumeration up to 16 entries, else a flagged
mean-field approximation) selection and flat/GLOB/Curve-Table `Chance None` resolution.
Anything the model doesn't cover (`Filter Keyword Chances`, `Epic Loot Chance`, list-level `Max
*`, COED) surfaces as a `DropNote::Unresolved` on the affected row rather than being silently
dropped. With `DropOptions::tree_depth > 0` it also returns `DropTable::tree`, the same odds as
`DropList`/`DropBranch` nesting cut off at that depth (deeper sublists become subtotal rows);
`walk` renders only the tree, bounded by its `--depth`, while `rows` stay fully flattened.

**`src/source.rs`** — `RecordSource`, the bulk-get and reverse-refs seam walk, chase and the
drop table fetch through: `ops::analysis`'s `DbSource` reads the open `Database`, and
`MemorySource` answers from records supplied up front (every traversal test uses it).
**`src/fields.rs`** holds the decoded-JSON readers the three share (reference stubs, schema
enums, condition rows), so none of them imports another for a helper.

**`src/refs/`** (`mod.rs`, with the entry-point/OMOD-property carrier seeds in `seeds.rs`) — the reverse-reference graph engine: `referenced_by_enriched`/
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

## Schema tooling (`tools/`)

The patch-notes pipeline lives in the repo-root `../patch-notes/` project, which reaches `esm`
only through the CLI.

**`tools/extractor/`**: `extract.py`
(schema generation, described above), `audit.py --gate` (the parity gate `just audit` runs),
`hardcoded.py` (emits `schema/hardcoded_fo76.json` from xEdit's hardcoded pseudo-plugin, backing
`src/hardcoded.rs`), and `pascal_stubs.py` (extractor support data). `tools/curvelookup.py`
(with `tools/curvelib.py`) is a standalone curve-table lookup.

## Where to tweak what

| Want to... | Look in |
|---|---|
| Add or fix a decoded field | `schema/fo76.overrides.json` or `tools/extractor/extract.py` (member order and the `unordered`/`any_member` binding flags decide which subrecord a member binds), then `src/decode/bind.rs` for binding, `src/decode/walk.rs` for payload decoding, `src/decode/derived.rs` for a derived value |
| Add a new CLI subcommand | `src/bin/cli/main.rs` (`Commands` enum + `dispatch_command`); its handler body goes in the matching `src/bin/cli/*.rs` module (`query.rs`, `refs.rs`, `walk.rs`, `diff.rs`, `cache.rs`, `inspect.rs`, …); add the op itself (an `Args` struct and function in a `src/ops/` family module, plus one `ops!` line in `src/ops/mod.rs`) if it needs `esm batch`/N-API reach too |
| Change diff noise suppression | `src/diff/noise.rs` (`suppress_record` and the stage it names) / `DiffOptions` |
| Change array-pairing behavior | `src/diff/array_diff.rs`'s `element_key_spec` / `widen_key_spec_until_unique` — read ADR 0005 first, especially before touching CTDA `Conditions[]` |
| Add an N-API method | `bindings/napi/src/lib.rs`, then `just gen-types` and `cd bindings/napi && bun run build` |
| Change a cache section's on-disk shape | its `impl SectionSpec` block (next to the type, in `index.rs` or `tree.rs`) and bump `index::CACHE_VERSION` |
| Change OMOD mechanism classification | `src/chase.rs` |
| Change LVLI drop-probability math | `src/lvli.rs` |
| Change how `walk` renders a digest | `src/walk/render.rs` (never the compute side, `mod.rs`, for pure formatting changes) |
