# esm — FO76 ESM Reader

A Rust workspace for reading and inspecting Fallout 76 `.esm` plugin/master files. Parses the Bethesda binary record format, schema-decodes 181 record types into structured JSON, indexes records by FormID and EditorID, resolves FormID references, loads localized string tables, evaluates curve tables, and supports search, diff, tree browsing, and schema coverage auditing.

> **Read-only.** This tool never modifies your `.esm` files. The only files it writes live in a shared sidecar directory next to the ESM, `esm_cache/`, holding five zero-copy rkyv sections per ESM (`<name>.esm.tree`, `<name>.esm.forms`, `<name>.esm.edid`, `<name>.esm.search`, `<name>.esm.xref`) — see [Index cache](#index-cache) for sizes and what each holds. Game data files (`*.esm`, `*.ba2`, and the `esm_cache/` directory) are gitignored and non-redistributable — obtain them from your own game install.

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

## CLI — `esm`

```sh
esm [--esm <ESM-or-folder>] <subcommand> [options] [...]
```

Every subcommand takes its ESM path from `--esm` (a global flag — it works
before or after the subcommand name) or, if `--esm` is omitted, from the
`FO76_ESM_PATH` environment variable:

```sh
esm --esm path/to/data get AssaultRifle --pretty
# equivalently, set it once for the session:
export FO76_ESM_PATH=path/to/data
esm get AssaultRifle --pretty
```

`diff` is the one exception — it always takes two explicit positional paths (`esm diff <old> <new>`) and ignores `--esm`/`FO76_ESM_PATH`.

Pass either a `.esm` file or a data folder. When given a folder, the tool auto-discovers the single `.esm` inside it, then looks for localization strings (`strings/<stem>_<locale>.strings` or any `*localization*.ba2`) and curve tables (`misc/curvetables/json/` or any `*startup*.ba2`). Override with `--localization-ba2`/`--strings-dir`/`--startup-ba2`/`--curves-dir` when the auto-detected sources aren't what you want.

The examples below assume `FO76_ESM_PATH` is set and omit `--esm`.

If a query has to wait on a cold cache build (see
[Cold-build progress and cross-process coordination](#cold-build-progress-and-cross-process-coordination)
below), it shows live progress on stderr and still returns the real result once the cache is ready.
Pass the global `--no-wait` flag to instead print the in-flight build's status and exit immediately
(status 75) — useful for scripts that would rather retry later than block.

### `info` — TES4 header summary

```sh
esm info
```

Prints version, record count, next object ID, ESM/Localization flags, author, description, and master dependencies.

### `get` — Fetch a single record

```sh
# Auto-detected positional: FormID (0x-prefixed / hex / decimal) vs EditorID
esm get AssaultRifle --pretty
esm get 0x463F --pretty

# Explicit flags still work and override the positional
esm get --edid AssaultRifle --pretty
esm get --formid 0x463F --pretty

# Raw subrecords (no schema decoding)
esm get 0x463F --raw --pretty

# With localized strings resolved (override auto-discovery)
esm get --edid AssaultRifle --localization-ba2 path/to/localization.ba2

# Control FormID cross-reference depth
esm get --edid AssaultRifle --resolve full   # inline referenced records
esm get --edid AssaultRifle --resolve stub   # referenced records as stubs
esm get --edid AssaultRifle --resolve none   # leave FormIDs as hex (default)
```

| Flag | Default | Description |
|---|---|---|
| `<target>` | — | Positional FormID or EditorID, auto-detected (see note) |
| `--formid <ID>` | — | Hex (`0x1234`) or decimal FormID (overrides positional) |
| `--edid <ID>` | — | EditorID string (overrides positional) |
| `--json` | false | Emit JSON (implied by `--pretty`) |
| `--pretty` | false | Pretty-print JSON |
| `--raw` | false | Skip schema decode; dump raw subrecords |
| `--localization-ba2 <BA2>` | — | Localization BA2 override (auto-discovered if omitted) |
| `--strings-dir <DIR>` | — | Loose `.strings` / `.dlstrings` directory override |
| `--lang <code>` | `en` | Language code for string tables |
| `--startup-ba2 <BA2>` | — | Startup BA2 for curve table evaluation |
| `--resolve <depth>` | `none` | FormID cross-reference depth: `none`, `stub`, `full` |

> Auto-detection: a positional `<target>` is treated as a FormID when it is `0x`-prefixed, pure decimal, or a bare run of hex digits up to 8 chars; anything else is an EditorID. Precedence is `--formid` > `--edid` > positional. Short all-hex EditorIDs (e.g. `cafe`) are read as FormIDs — pass `--edid` to disambiguate.

### `list` — List records of a type

```sh
esm list --type WEAP --limit 20
esm list --type GLOB --localization-ba2 path/to/localization.ba2 --pretty
```

| Flag | Default | Description |
|---|---|---|
| `--type <SIG>` | required | 4-char record type signature |
| `--limit <N>` | 50 | Max records to return |
| `--localization-ba2 <BA2>` | — | Localization BA2 override |
| `--strings-dir <DIR>` | — | Loose string files override |
| `--lang <code>` | `en` | Language |

### `search` — Wildcard search over EditorIDs and names

```sh
esm search "*Rifle*" --type WEAP --in both --pretty
esm search "Assault*" --in edid
```

| Flag | Default | Description |
|---|---|---|
| `<pattern>` | required | Wildcard pattern (`*` = any substring, case-insensitive) |
| `--type <SIG,...>` | all | Comma-separated record types to search |
| `--in <field>` | `both` | `edid`, `name`, or `both` |
| `--limit <N>` | 100 | Max results |
| `--json` / `--pretty` | — | Output format |
| `--localization-ba2`, `--strings-dir`, `--lang` | — | String resolution |

### `refs` — Reverse FormID lookup

```sh
# Auto-detected positional (FormID or EditorID), same rules as `get`
esm refs AssaultRifle --limit 50
esm refs 0x463F --json --pretty

# Explicit flags still work and override the positional
esm refs --edid AssaultRifle --limit 50
esm refs --formid 0x463F --json --pretty

# OMOD property carriers (flag-only — never auto-detected from a bare positional)
esm refs --prop weap:Speed --depth 2 --type WEAP
esm refs --omod-property Keywords --limit 0
```

Find all records that reference a given FormID. Builds and caches an xref index on first run.

`--prop`/`--omod-property <[scope:]name-or-id>` seeds the walk from every OMOD that declares
the given property (e.g. `weap:Speed`, or a bare `Keywords` across all weap/armo/npc spaces).
Unlike entry-point names, property names are not auto-detected from a bare positional — short
generics like `Health` collide with real EditorIDs.
<!-- Several other refs flags (--ep, --depth, --type, --paths, --sort, --to) are also undocumented here; see skills/esm-cli/SKILL.md. -->

### `tree` — Browse the GRUP hierarchy

```sh
esm tree --type WEAP --limit 50 --pretty
esm tree --offset 0 --limit 20
```

### `diff` — Compare two ESM versions

```sh
esm diff path/to/old path/to/new --type GLOB --json --pretty
esm diff path/to/old path/to/new
```

`diff` always takes two explicit positional paths and ignores `--esm`/`FO76_ESM_PATH`. Aligns records by FormID, uses byte-equality fast-path, decodes only changed records, and emits a sparse `{from, to}` diff per changed field. Prints a per-type summary and timing to stderr.

### `coverage` — Schema audit

```sh
esm coverage --type WEAP
esm coverage --gate   # exits non-zero on any raw_fallback
```

Counts `_raw`, `_unmapped`, `_unknown_record`, and `_unresolved` markers across decoded records. Use `--gate` in CI to enforce full decode coverage.

### `daemon` — Manage the background warm daemon

```sh
esm daemon start    # explicitly pre-warm (optional — any call auto-spawns on demand)
esm daemon status   # check if running, which ESMs are resident and their record counts
esm daemon stop     # graceful shutdown
```

The daemon is normally transparent: the first `esm` call auto-spawns it, subsequent calls use it as a fast HTTP backend, and it shuts itself down after 10 minutes of idle. Use these subcommands only when you need explicit control.

### `cache status` — Inspect the on-disk index cache

```sh
esm cache status          # human-readable: state + which sections are present
esm cache status --json   # same, as JSON — includes live build progress when building
```

Reads `esm_cache/`'s section headers and the build lock/heartbeat directly off disk — no ESM open,
no daemon contact, so it answers instantly even while another process is mid-build. See
[Cold-build progress and cross-process coordination](#cold-build-progress-and-cross-process-coordination)
above.

## Server — `esm-server`

Feature-gated HTTP REST + MCP stdio server. Build with `--features server`:

```sh
cargo run --release --features server --bin esm-server -- path/to/data
cargo run --release --features server --bin esm-server -- path/to/data --compare path/to/prev --port 3000
cargo run --release --features server --bin esm-server -- path/to/data --mcp-stdio
```

HTTP routes: `GET /info`, `/records/{formid}`, `/records?edid=|type=&limit=`, `/groups`, `/groups/{sig}/children`, `/stub/{offset}`, `/diff`, `/health`. Serves embedded HTML viewer at `/` and `/compare`.

MCP stdio mode implements JSON-RPC 2.0 with six tools: `esm_file_info`, `esm_get_record`, `esm_list_groups`, `esm_list_records`, `esm_search`, `esm_refs` (depth-bound BFS reverse-reference walk; `depth=1` default, up to 6).

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

`schema/fo76.json` (2.3 MB) is embedded at compile time via `include_str!`. It covers 181 FO76 record types derived from xEdit Pascal definitions. An `fo76.overrides.json` is merged on top for manual corrections.

To regenerate or extend coverage:

```sh
# Requires a TES5Edit/FO76Edit checkout at ../TES5Edit
python3 tools/extractor/extract.py

# Audit schema parity against Pascal source (exits non-zero on HIGH drops)
python3 tools/extractor/audit.py --gate
```

### Supported record types

**Decode** — `full`: every subrecord and field consumed with no fallbacks; `partial`: some subrecords or fields hit a raw-bytes fallback or are left unmapped (schema gaps); `partial†`: only documented newer-than-reference drift subrecords remain `_unmapped` (see [Known coverage drift](CLAUDE.md#known-coverage-drift-vs-tes5edit)); `none`: record type has no schema entry — all subrecords are unmapped.  
**Tests** — `robust`: ≥ 3 handpicked records tested end-to-end; `basic`: 1–2 records or covered by the exhaustive env-gated sweep; `none`: no dedicated test.

Decode status is measured against a reference ESM via `esm coverage`. Run the exhaustive integration test locally with `RUST_TEST_ESM=<path> cargo test`.

| Sig | Name | Decode | Tests |
|-----|------|:------:|:-----:|
| `AACT` | Action | full | none |
| `AAMD` | Aim Assist Model Data | full | none |
| `AAPD` | Aim Assist Pose Data | full | none |
| `ACHR` | Placed NPC | full | none |
| `ACTI` | Activator | full | none |
| `ADDN` | Addon Node | full | none |
| `AECH` | Audio Effect Chain | full | none |
| `ALCH` | Ingestible | full | basic |
| `AMDL` | Aim Model | full | basic |
| `AMMO` | Ammunition | full | basic |
| `ANIO` | Animated Object | full | none |
| `AORU` | Attraction Rule | full | none |
| `ARMA` | Armor Addon | full | basic |
| `ARMO` | Armor | full | basic |
| `ARTO` | Art Object | full | none |
| `ASPC` | Acoustic Space | full | none |
| `ASTM` | Unknown - ASTM | full | none |
| `ASTP` | Association Type | full | none |
| `ATXO` | ATX Default Object | full | none |
| `AUVF` | AUVF - Unknown | full | none |
| `AVIF` | Actor Value Information | full | basic |
| `AVTR` | Avatar | full | none |
| `BNDS` | Bendable Spline | full | none |
| `BOOK` | Book | full | basic |
| `BPTD` | Body Part Data | full | basic |
| `CAMS` | Camera Shot | full | none |
| `CELL` | Cell | full | none |
| `CHAL` | Challenge | full | basic |
| `CLAS` | Class | full | none |
| `CLFM` | Color | full | none |
| `CLMT` | Climate | full | none |
| `CMPO` | Component | full | basic |
| `CMPT` | Camp Title | full | basic |
| `CNCY` | Currency | full | none |
| `CNDF` | Condition Form | full | none |
| `COBJ` | Constructible Object | full | basic |
| `COEN` | Consumable Entitlement | full | basic |
| `COLL` | Collision Layer | full | none |
| `CONT` | Container | full | basic |
| `CPRD` | Challenge Pass Reward Data | full | none |
| `CPTH` | Camera Path | full | none |
| `CSEN` | Crate Service Entitlement | full | none |
| `CSTY` | Combat Style | full | none |
| `CURV` | Curve Table | full | basic |
| `DCGF` | Daily Content Group | full | none |
| `DEBR` | Debris | full | none |
| `DFOB` | Default Object | full | basic |
| `DIAL` | Dialog Topic | full | none |
| `DIST` | District | full | none |
| `DLBR` | Dialog Branch | full | none |
| `DLVW` | Dialog View | full | none |
| `DMGT` | Damage Type Resist | full | basic |
| `DOBJ` | Default Object Manager | full | none |
| `DOOR` | Door | full | none |
| `ECAT` | Emote Category | full | none |
| `EFSH` | Effect Shader | full | none |
| `EMOT` | Emote | full | none |
| `ENCH` | Enchantment | full | basic |
| `ENTM` | Entitlement | full | basic |
| `EQUP` | Equip Type | full | none |
| `EXPL` | Explosion | full | basic |
| `FACT` | Faction | full | basic |
| `FISH` | Fish | full | basic |
| `FLOR` | Flora | full | basic |
| `FLST` | FormID List | full | basic |
| `FSTP` | Footstep | full | none |
| `FSTS` | Footstep Set | full | none |
| `FURN` | Furniture | full | basic |
| `GCVR` | Ground Cover | full | none |
| `GDRY` | God Rays | full | none |
| `GLOB` | Global | full | basic |
| `GMRW` | Gameplay Reward | full | basic |
| `GMST` | Game Setting | full | basic |
| `GRAS` | Grass | full | none |
| `HAZD` | Hazard | full | basic |
| `HDPT` | Head Part | full | none |
| `IDLE` | Idle Animation | full | none |
| `IDLM` | Idle Marker | full | none |
| `IMAD` | Image Space Adapter | full | none |
| `IMGS` | Image Space | full | none |
| `INFO` | Dialog response | full | basic |
| `INGR` | Ingredient | full | none |
| `INNR` | Instance Naming Rules | full | basic |
| `IPCT` | Impact | full | none |
| `IPDS` | Impact Data Set | full | none |
| `KEYM` | Key | full | none |
| `KSSM` | Sound Keyword Mapping | full | none |
| `KYWD` | Keyword | full | basic |
| `LAYR` | Layer | full | none |
| `LCRT` | Location Reference Type | full | none |
| `LCTN` | Location | full | none |
| `LENS` | Lens Flare | full | none |
| `LGDI` | Legendary Item | full | basic |
| `LGTM` | Lighting Template | full | none |
| `LIGH` | Light | full | none |
| `LOUT` | Loadout | full | none |
| `LSCR` | Load Screen | full | none |
| `LTEX` | Landscape Texture | full | none |
| `LVLI` | Leveled Item | full | basic |
| `LVLN` | Leveled NPC | full | basic |
| `LVLP` | Leveled Pack In | full | basic |
| `LVPC` | Leveled Perk Card | full | basic |
| `MATO` | Material Object | full | none |
| `MATT` | Material Type | full | none |
| `MDSP` | Model Swap | full | basic |
| `MESG` | Message | full | none |
| `MGEF` | Magic Effect | full | basic |
| `MISC` | Misc. Item | full | basic |
| `MOVT` | Movement Type | full | none |
| `MSTT` | Moveable Static | full | none |
| `MSWP` | Material Swap | full | basic |
| `MUSC` | Music Type | full | none |
| `MUST` | Music Track | full | none |
| `NAVI` | Navmesh Info Map | full | none |
| `NAVM` | Navigation Mesh | full | none |
| `NOCM` | Navmesh Obstacle Manager | full | none |
| `NOTE` | Note | full | basic |
| `NPC_` | Non-Player Character | full | robust |
| `OMOD` | Object Modification | full | basic |
| `OTFT` | Outfit | full | basic |
| `OVIS` | Object Visibility Manager | full | none |
| `PACH` | Power Armor Chassis | full | none |
| `PACK` | Package | full | none |
| `PCRD` | Perk Card | full | basic |
| `PEPF` | Event Playlist | full | basic |
| `PERK` | Perk | full | robust |
| `PGRE` | Placed Grenade | full | none |
| `PHZD` | Placed Hazard | full | none |
| `PKIN` | Pack-In | full | none |
| `PLYR` | Player Reference | full | none |
| `PLYT` | Player Title | full | basic |
| `PMFT` | Photo Mode Feature | full | none |
| `PMIS` | Placed Missile | full | none |
| `PPAK` | Perk Card Pack | full | none |
| `PROJ` | Projectile | full | basic |
| `QMDL` | Quest Module | full | basic |
| `QUST` | Quest | full | basic |
| `RACE` | Race | full | basic |
| `REFR` | Placed Object | full | none |
| `REGN` | Region | full | none |
| `RELA` | Relationship | full | none |
| `RESO` | Resource | full | basic |
| `REVB` | Reverb Parameters | full | none |
| `RFCT` | Visual Effect | full | none |
| `RFGP` | Reference Group | full | none |
| `SCCO` | Scene Collection | full | none |
| `SCEN` | Scene | full | basic |
| `SCOL` | Static Collection | full | none |
| `SCSN` | Sound Category Snapshot | full | none |
| `SECH` | Sound Echo Marker | full | none |
| `SMBN` | Story Manager Branch Node | full | none |
| `SMEN` | Story Manager Event Node | full | none |
| `SMQN` | Story Manager Quest Node | full | none |
| `SNCT` | Sound Category | full | none |
| `SNDR` | Sound Descriptor | full | none |
| `SOPM` | Sound Output Model | full | none |
| `SOUN` | Sound Marker | full | none |
| `SPEL` | Spell | full | basic |
| `SPGD` | Shader Particle Geometry | full | none |
| `STAG` | Animation Sound Tag Set | full | none |
| `STAT` | Static | full | none |
| `STHD` | Spell Threshold Data | full | none |
| `STMP` | Snap Template | full | none |
| `STND` | Snap Template Node | full | none |
| `TACT` | Talking Activator | full | none |
| `TEPF` | Infestation Event Playlist | full | basic |
| `TERM` | Terminal | full | basic |
| `TRAP` | Trap | full | basic |
| `TREE` | Tree | full | none |
| `TRNS` | Transform | full | none |
| `TXST` | Texture Set | full | none |
| `UTIL` | Utility | full | none |
| `VOLI` | Volumetric Lighting | full | none |
| `VTYP` | Voice Type | full | none |
| `WATR` | Water | full | none |
| `WAVE` | Wave Encounter | full | basic |
| `WEAP` | Weapon | full | robust |
| `WRLD` | Worldspace | full | none |
| `WSPR` | Workshop Permissions | full | none |
| `WTHR` | Weather | full | basic |
| `ZOOM` | Zoom | full | none |

## Tests

~100 tests across `tests/` (integration test files) and two inline `#[cfg(test)]` blocks (for `tree` and `decode` internals that are not public). Run all:

```sh
cargo test

# Exhaustive decode sweep (needs real ESM — skips silently if unset)
RUST_TEST_ESM=path/to/data cargo test

# Diff integration test (needs two ESM versions — skips silently if unset)
RUST_TEST_ESM_A=old.esm RUST_TEST_ESM_B=new.esm cargo test
```

| File | What it covers |
|---|---|
| `tests/wildcard.rs` | Wildcard matching (substring, prefix, suffix, multi-star) |
| `tests/curves.rs` | Curve evaluation: clamping, interpolation, edge cases |
| `tests/diff.rs` | JSON diff logic; `diff_databases` (ignored, needs two ESM versions) |
| `tests/reader.rs` | ESM walk: group/record event sequence from a synthetic file |
| `tests/ipc.rs` | IPC dispatch: `Op` routing, `RecordSel` auto-detection, `Registry`, `LocalBackend` parity, `looks_like_formid` |
| `tests/decode_records.rs` | Schema-driven decode of MGEF, OMOD, GLOB, KYWD, FLST, AMMO, ALCH, PROJ, ARMO, ARMA, AVIF, ENCH, BOOK, WEAP, PERK, RACE, GMRW, LVLI, NPC_, SPEL, EXPL, COBJ, CONT, PCRD, TERM, FLOR, FURN, INFO, MISC, QMDL, NOTE, LVLN, LVPC, LVLP, RESO, SCEN, QUST (alias fill) using verbatim record bytes |
| `tests/decode_coverage.rs` | Exhaustive full-decode sweep over `CLEAN_TYPES` (178 types; needs game data, skips if unset) |
| `src/tree.rs` (inline) | `decode_label` dispatch (`pub(crate)`, not accessible from `tests/`) |
| `src/decode.rs` (inline) | `decode_struct_fields` count-prefix width; VMAD object decoding (both object formats, FormID offset); VMAD array property types 11–15 and struct types 6/17 (count + elements); COED `FormIdTargetType` owner-decider with and without resolver; `RArray` `CountPath` boundary |

`tests/decode_records.rs` tests use verbatim subrecord bytes from `esm get --raw` and run entirely in CI without game data. See the **Supported record types** table in [Schema](#schema) for per-type coverage status.

## Bulk / sweep workflow (for agents)

AI agents scanning many records should still avoid cold per-record process spawns. `Index`'s disk cache is zero-copy rkyv now (mmap'd, not fully deserialized into heap HashMaps), so a cold `esm get` is far cheaper than it once was — but it still maps `tree`+`forms` on every invocation (~0.08 s / ~120 MiB warm on a 5.6M-record ESM, measured on the 20260724 snapshot), and that cost repeats per process. 1000 cold sweeps still means 1000× that overhead; the warm daemon (below) avoids it entirely.

### Warm daemon (fastest, no extra flags)

```sh
# Build both binaries once (server binary must be adjacent to esm for auto-spawn)
cargo build --release --features server

# The first call auto-spawns and warms the daemon; all subsequent calls are fast
# (assumes FO76_ESM_PATH is set — or pass --esm path/to/data on each call)
esm get 0x463F --pretty
esm get AssaultRifle --pretty
```

The daemon keeps the index in memory, self-shuts-down after 10 min idle, stale-evicts if the ESM changes, and is safe for concurrent agents (advisory spawn-lock prevents double-spawn).

**Prefer bulk ops** over N single `get`s — each round-trip has overhead:

```sh
esm list --type WEAP --limit 500 --pretty   # all weapons in one call
esm search "*Rifle*" --type WEAP --pretty   # name/EditorID wildcard
esm refs 0x463F --limit 100 --pretty        # reverse FormID lookup
```

**Gotcha:** `--localization-ba2`, `--strings-dir`, and `--startup-ba2` on `get` force a cold open (the daemon doesn't accept per-call source overrides). Pass a data folder or place the Localization/Startup BA2 files (or `strings/`/`misc/curvetables/` directories) next to the ESM so the daemon auto-loads them on open, and drop per-call flags in sweeps.

### MCP opt-in

Wire up `esm-server --mcp-stdio` in your AI client's MCP config. **Do not commit** this file — it hardcodes a non-redistributable, date-stamped ESM path:

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

Five tools exposed: `esm_file_info`, `esm_get_record`, `esm_list_records`, `esm_search`, `esm_refs`. MCP-stdio proxies to the same warm daemon, so the warm-index benefit applies automatically.

## Index cache

`Index`'s cache is five independent, zero-copy [rkyv](https://rkyv.org/) sections, each its own
mmap'd file inside `esm_cache/` — one shared, fixed-name directory sibling to the ESM, holding one
file per ESM per section, named `<esm file name>.<section>` (e.g. `SeventySix.esm.forms`) so
multiple plugins in one directory never collide. Sections are read via `rkyv::access_unchecked`
rather than deserialized into heap HashMaps (see `src/rkyvcache.rs`,
`cache_dir_for`/`section_path_for`). This replaced a single bincode-encoded `.esm.idx` blob;
`bincode` is no longer a dependency of this crate at all (it was permanently unmaintained —
RUSTSEC-2025-0141 — see `deny.toml`'s git history for the full rationale).

Two are eager — both built together on `Database::open` whenever either is missing or stale, since
`get_by_formid` and the GRUP tree browser are core paths:

- **`<name>.esm.forms`** (~200 MiB) — FormID→[`RecordMeta`] table (sorted `Vec`, binary-searched)
  plus the per-type FormID directory. This is the zero-copy replacement for what used to be the
  bulk of `.esm.idx`'s ~280 MiB and its cold-load cost.
- **`<name>.esm.tree`** (~140 MiB) — the GRUP structural tree (`tree` / `list-groups` / `list_type_children`).

Three are lazy — built on first use of the matching operation, same as before this migration, just
persisted to their own file instead of one shared blob (so, e.g., `ensure_edid_index` only ever
writes `<name>.esm.edid`, not the other two). A fresh process opening the same ESM later still
picks up whichever of these a prior process already built, exactly as before:

- **`<name>.esm.edid`** (~15 MiB) — EditorID→FormID map (`--edid` lookups).
- **`<name>.esm.search`** (~30 MiB) — FormID→name/description map (`search`).
- **`<name>.esm.xref`** (~55 MiB) — FormID→referencing-FormIDs map (`refs`).

Every section carries its own header (magic, format/section/cache version, a layout fingerprint, and
the source ESM's size+mtime) validated before any bytes are trusted — a stale, foreign, or corrupt
file degrades to "rebuild that section," never a crash. `CACHE_VERSION` (`src/index.rs`) still gates
all five as a shared semantic-layout counter; bump it whenever a section's on-disk *meaning* changes.

The whole `esm_cache/` directory is gitignored. Never commit it.

### Cold-build progress and cross-process coordination

Building any of the five sections above from scratch can take tens of seconds to a couple of
minutes on the full FO76 ESM (the `xref` section in particular decodes every record). Rather than
blocking silently, every builder — the daemon, a `--local` CLI call, or the N-API host — publishes
a live heartbeat (`<esm file name>.build.json`) alongside an advisory lock
(`<esm file name>.build.lock`) inside `esm_cache/`. Any process can read that state instantly, with
no daemon round-trip:

- The `esm` CLI shows a live progress line on stderr (stdout stays untouched) whenever a query it
  issues has to wait on a build — a `\r`-updated bar on a TTY, one plain line every ~10s otherwise.
  Pass `--no-wait` to print the build's status and exit immediately (status 75) instead of waiting.
- `esm cache status [--json]` inspects `esm_cache/` and the build lock/heartbeat directly — no
  ESM open, no daemon contact, answers instantly even mid-build. Reports one of `empty` / `building`
  / `partial` (the common steady state — e.g. `tree`+`forms` built, `xref` never triggered) /
  `complete`.
- A second process that needs the same section a build is already producing waits on that same
  build via the lock rather than starting a redundant one; once granted the lock it re-checks
  whether the section now exists before doing any work.

Set `ESM_NO_PROGRESS=1` to suppress heartbeat *publishing* (e.g. embedding contexts where a stray
file write is unwanted) — the lock-based dedup keeps working regardless. See `src/progress.rs` and
`docs/adr/0003-cache-build-progress-heartbeat.md`.

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
