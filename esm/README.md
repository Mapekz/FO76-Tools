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
cargo test                     # run all tests (~100 run; 2 env-gated ignored)
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
# Auto-detected positional: FormID (0x-prefixed / hex / decimal) vs EditorID
esm get SeventySix.esm AssaultRifle --pretty
esm get SeventySix.esm 0x463F --pretty

# Explicit flags still work and override the positional
esm get SeventySix.esm --edid AssaultRifle --pretty
esm get SeventySix.esm --formid 0x463F --pretty

# Raw subrecords (no schema decoding)
esm get SeventySix.esm 0x463F --raw --pretty

# With localized strings resolved
esm get SeventySix.esm --edid AssaultRifle --strings "SeventySix - Localization.ba2"

# Control FormID cross-reference depth
esm get SeventySix.esm --edid AssaultRifle --resolve full   # inline referenced records
esm get SeventySix.esm --edid AssaultRifle --resolve stub   # referenced records as stubs
esm get SeventySix.esm --edid AssaultRifle --resolve none   # leave FormIDs as hex (default)
```

| Flag | Default | Description |
|---|---|---|
| `<target>` | — | Positional FormID or EditorID, auto-detected (see note) |
| `--formid <ID>` | — | Hex (`0x1234`) or decimal FormID (overrides positional) |
| `--edid <ID>` | — | EditorID string (overrides positional) |
| `--json` | false | Emit JSON (implied by `--pretty`) |
| `--pretty` | false | Pretty-print JSON |
| `--raw` | false | Skip schema decode; dump raw subrecords |
| `--strings <BA2>` | — | Localization BA2 to resolve LStrings |
| `--strings-dir <DIR>` | — | Directory of loose `.strings` / `.dlstrings` files |
| `--lang <code>` | `en` | Language code for string tables |
| `--startup-ba2 <BA2>` | — | Startup BA2 for curve table evaluation |
| `--resolve <depth>` | `none` | FormID cross-reference depth: `none`, `stub`, `full` |

> Auto-detection: a positional `<target>` is treated as a FormID when it is `0x`-prefixed, pure decimal, or a bare run of hex digits up to 8 chars; anything else is an EditorID. Precedence is `--formid` > `--edid` > positional. Short all-hex EditorIDs (e.g. `cafe`) are read as FormIDs — pass `--edid` to disambiguate.

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
# Auto-detected positional (FormID or EditorID), same rules as `get`
esm refs SeventySix.esm AssaultRifle --limit 50
esm refs SeventySix.esm 0x463F --json --pretty

# Explicit flags still work and override the positional
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

### Supported record types

**Decode** — `full`: every subrecord and field consumed with no fallbacks; `partial`: some subrecords or fields hit a raw-bytes fallback or are left unmapped (schema gaps); `partial†`: only documented newer-than-reference drift subrecords remain `_unmapped` (see [Known coverage drift](CLAUDE.md#known-coverage-drift-vs-tes5edit)); `none`: record type has no schema entry — all subrecords are unmapped.  
**Tests** — `robust`: ≥ 3 handpicked records tested end-to-end; `basic`: 1–2 records or covered by the exhaustive env-gated sweep; `none`: no dedicated test.

Decode status is measured against `SeventySix_20260619.esm` via `esm coverage`. Run the exhaustive integration test locally with `RUST_TEST_ESM=<path> cargo test -- --ignored`.

| Sig | Name | Decode | Tests |
|-----|------|:------:|:-----:|
| `AACT` | Action | full | none |
| `AAMD` | Aim Assist Model Data | full | none |
| `AAPD` | Aim Assist Pose Data | partial | none |
| `ACHR` | Placed NPC | partial | none |
| `ACTI` | Activator | partial | none |
| `ADDN` | Addon Node | full | none |
| `AECH` | Audio Effect Chain | full | none |
| `ALCH` | Ingestible | full | basic |
| `AMDL` | Aim Model | full | basic |
| `AMMO` | Ammunition | full | basic |
| `ANIO` | Animated Object | full | none |
| `AORU` | Attraction Rule | full | none |
| `ARMA` | Armor Addon | full | none |
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
| `CELL` | Cell | none | none |
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
| `COLL` | Collision Layer | partial | none |
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
| `DLBR` | Dialog Branch | none | none |
| `DLVW` | Dialog View | full | none |
| `DMGT` | Damage Type Resist | full | basic |
| `DOBJ` | Default Object Manager | full | none |
| `DOOR` | Door | partial | none |
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
| `GMRW` | Gameplay Reward | partial | basic |
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
| `KEYM` | Key | partial | none |
| `KSSM` | Sound Keyword Mapping | full | none |
| `KYWD` | Keyword | full | basic |
| `LAYR` | Layer | full | none |
| `LCRT` | Location Reference Type | full | none |
| `LCTN` | Location | full | none |
| `LENS` | Lens Flare | full | none |
| `LGDI` | Legendary Item | full | basic |
| `LGTM` | Lighting Template | partial | none |
| `LIGH` | Light | partial | none |
| `LOUT` | Loadout | full | none |
| `LSCR` | Load Screen | full | none |
| `LTEX` | Landscape Texture | full | none |
| `LVLI` | Leveled Item | partial | basic |
| `LVLN` | Leveled NPC | partial† | basic |
| `LVLP` | Leveled Pack In | partial† | basic |
| `LVPC` | Leveled Perk Card | partial† | basic |
| `MATO` | Material Object | full | none |
| `MATT` | Material Type | full | none |
| `MDSP` | Model Swap | full | basic |
| `MESG` | Message | full | none |
| `MGEF` | Magic Effect | full | basic |
| `MISC` | Misc. Item | full | basic |
| `MOVT` | Movement Type | full | none |
| `MSTT` | Moveable Static | partial | none |
| `MSWP` | Material Swap | full | basic |
| `MUSC` | Music Type | full | none |
| `MUST` | Music Track | full | none |
| `NAVI` | Navmesh Info Map | full | none |
| `NAVM` | Navigation Mesh | none | none |
| `NOCM` | Navmesh Obstacle Manager | full | none |
| `NOTE` | Note | full | basic |
| `NPC_` | Non-Player Character | partial | basic |
| `OMOD` | Object Modification | full | basic |
| `OTFT` | Outfit | full | basic |
| `OVIS` | Object Visibility Manager | full | none |
| `PACH` | Power Armor Chassis | full | none |
| `PACK` | Package | full | none |
| `PCRD` | Perk Card | full | basic |
| `PEPF` | Event Playlist | full | basic |
| `PERK` | Perk | full | robust |
| `PGRE` | Placed Grenade | none | none |
| `PHZD` | Placed Hazard | none | none |
| `PKIN` | Pack-In | full | none |
| `PLYR` | Player Reference | none | none |
| `PLYT` | Player Title | full | basic |
| `PMFT` | Photo Mode Feature | full | none |
| `PMIS` | Placed Missile | none | none |
| `PPAK` | Perk Card Pack | full | none |
| `PROJ` | Projectile | full | basic |
| `QMDL` | Quest Module | full | basic |
| `QUST` | Quest | partial† | basic |
| `RACE` | Race | full | basic |
| `REFR` | Placed Object | partial | none |
| `REGN` | Region | full | none |
| `RELA` | Relationship | full | none |
| `RESO` | Resource | partial† | basic |
| `REVB` | Reverb Parameters | full | none |
| `RFCT` | Visual Effect | full | none |
| `RFGP` | Reference Group | full | none |
| `SCCO` | Scene Collection | partial | none |
| `SCEN` | Scene | full | none |
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
| `STAT` | Static | partial | none |
| `STHD` | Spell Threshold Data | full | none |
| `STMP` | Snap Template | full | none |
| `STND` | Snap Template Node | full | none |
| `TACT` | Talking Activator | partial | none |
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
| `WRLD` | Worldspace | none | none |
| `WSPR` | Workshop Permissions | full | none |
| `WTHR` | Weather | full | basic |
| `ZOOM` | Zoom | full | none |

## Tests

~100 tests across `tests/` (integration test files) and two inline `#[cfg(test)]` blocks (for `tree` and `decode` internals that are not public). Run all:

```sh
cargo test

# Exhaustive decode sweep (needs real ESM — 51 clean types, ~181k records)
RUST_TEST_ESM=SeventySix.esm cargo test -- --ignored decode_all_clean_types_fully

# Diff integration test (needs two ESM versions)
RUST_TEST_ESM_A=old.esm RUST_TEST_ESM_B=new.esm cargo test -- --ignored diff_two_esm_versions_glob
```

| File | What it covers |
|---|---|
| `tests/wildcard.rs` | Wildcard matching (substring, prefix, suffix, multi-star) |
| `tests/curves.rs` | Curve evaluation: clamping, interpolation, edge cases |
| `tests/diff.rs` | JSON diff logic; `diff_databases` (ignored, needs two ESM versions) |
| `tests/reader.rs` | ESM walk: group/record event sequence from a synthetic file |
| `tests/ipc.rs` | IPC dispatch: `Op` routing, `RecordSel` auto-detection, `Registry`, `LocalBackend` parity, `looks_like_formid` |
| `tests/decode_records.rs` | Schema-driven decode of MGEF, OMOD, GLOB, KYWD, FLST, AMMO, ALCH, PROJ, ARMO, AVIF, ENCH, BOOK, WEAP, PERK, RACE, GMRW/LVLI/NPC_ (drift-locked), TERM, FLOR, FURN, INFO, MISC, QMDL, NOTE, LVLN/LVPC/LVLP/RESO (drift-locked), QUST (alias fill) using verbatim record bytes |
| `tests/decode_coverage.rs` | Exhaustive full-decode sweep over all 51 clean types (ignored, needs game data) |
| `src/tree.rs` (inline) | `decode_label` dispatch (`pub(crate)`, not accessible from `tests/`) |
| `src/decode.rs` (inline) | `decode_struct_fields` count-prefix width; VMAD object decoding (both object formats, FormID offset); VMAD array property types 11–15 and struct types 6/17 (count + elements); COED `FormIdTargetType` owner-decider with and without resolver; `RArray` `CountPath` boundary |

`tests/decode_records.rs` tests use verbatim subrecord bytes from `esm get --raw` and run entirely in CI without game data. See the **Supported record types** table in [Schema](#schema) for per-type coverage status.

## Index cache

On first open, the tool writes a `<name>.esm.idx` file next to the ESM. Subsequent opens skip re-parsing and load the index from this cache (keyed by file path, size, and mtime). The cache regenerates automatically when the ESM changes or `CACHE_VERSION` is bumped. These files are gitignored.
