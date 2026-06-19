# fo76-esm-parser

Cross-platform Rust library and CLI for reading Fallout 76 plugin files (`*.esm`). Parses the binary record format, indexes records by FormID and EditorID, and decodes fields to JSON using a schema derived from [xEdit](https://github.com/TES5Edit/TES5Edit) FO76 definitions.

Designed as a reusable core for tooling (DPS calculators, data browsers, MCP agents) without the Windows-only Pascal UI.

## Features

- **Memory-mapped parsing** of large plugins (~880 MB `SeventySix.esm`)
- **Fast structural index** — FormID → file offset for all records (~5.8M in the main ESM)
- **EditorID index** — built on demand and cached to disk
- **Zlib decompression** for compressed records (`0x00040000` flag)
- **Schema-driven decode** — human-readable JSON for AMMO, ARMO, PROJ, EXPL, WEAP, SPEL, MGEF, PERK
- **Graceful fallback** — unknown records/fields emit raw hex instead of failing

### Not yet implemented

See [todos.md](todos.md) for the full roadmap. Highlights:

- Localized strings (`FULL`, `DESC`) from `SeventySix - Localization.ba2`
- Curve tables from `SeventySix - Startup.ba2` via `CURV` records
- Multi-plugin load order, reference following, write support

## Requirements

- [Rust](https://rustup.rs/) 1.70+ (2021 edition)
- A copy of `SeventySix.esm` (and optionally the BA2 archives for future string/curve support)

## Build

```bash
cargo build --release
```

The CLI binary is `target/release/fo76`.

## CLI usage

```bash
ESM=SeventySix.esm

# TES4 header: version, record count, masters, flags
./target/release/fo76 info "$ESM"

# List weapons (FormID, EditorID, FULL string id)
./target/release/fo76 list "$ESM" --type WEAP --limit 20

# Fetch a record by EditorID or FormID (hex or decimal)
./target/release/fo76 get "$ESM" --edid AssaultRifle --pretty
./target/release/fo76 get "$ESM" --formid 0x463F --pretty

# Raw subrecord dump (no schema decode)
./target/release/fo76 get "$ESM" --formid 0x463F --raw --pretty
```

### Index cache

The first query builds a sidecar index at `SeventySix.esm.idx` (keyed by file path, size, and mtime). Subsequent runs reuse it. The EditorID index is added to the same cache after the first `--edid` lookup or `list` call.

## Library API

```rust
use fo76_esm_parser::{Database, FormId};

let mut db = Database::open("SeventySix.esm")?;

let info = db.file_info()?;
println!("records: {}", info.record_count);

let record = db.record_by_edid("AssaultRifle")?;
println!("{}", serde_json::to_string_pretty(&record.fields)?);

let by_id = db.record_by_formid(FormId::new(0x463F))?;
let weapons = db.list_by_type("WEAP", 10)?;
```

`Database::open` embeds `schema/fo76.json` at compile time. Decoding is read-only.

## Project layout

```
esm-parser/
├── src/
│   ├── lib.rs          # Database API
│   ├── format.rs       # Record / GRUP / subrecord headers
│   ├── reader.rs       # mmap walk, TES4 header, subrecord parse
│   ├── compress.rs     # zlib record decompression
│   ├── formid.rs       # FormID parse/display
│   ├── index.rs        # FormID + EditorID index, bincode cache
│   ├── schema.rs       # JSON schema types
│   ├── decode.rs       # Schema-driven decoder → serde_json::Value
│   └── bin/cli.rs      # `fo76` CLI
├── schema/
│   └── fo76.json       # Record field definitions (committed)
├── tools/
│   └── extractor/
│       └── extract.py  # Pascal DSL → fo76.json (build-time tool)
└── todos.md            # Post-POC follow-ups
```

## Schema

Field layouts are defined in `schema/fo76.json`, initially hand-crafted for eight record types and expandable via `tools/extractor/extract.py`, which reads `TES5Edit/Core/wbDefinitionsFO76.pas`.

Decoded output uses `serde_json::Value`. LString fields currently show unresolved IDs when string tables are not loaded:

```json
"Name": { "lstring_id": "0x00012DE9", "_unresolved": true }
```

## Game data files

| File | Purpose | Status |
|------|---------|--------|
| `SeventySix.esm` | Main plugin | Required |
| `SeventySix - Localization.ba2` | English string tables | Planned ([todos.md](todos.md)) |
| `SeventySix - Startup.ba2` | Curve JSON (`misc/curvetables/json/…`) | Planned ([todos.md](todos.md)) |

These files are large (hundreds of MB to ~1 GB total) and are not redistributable; obtain them from your own game install.

## Binary format reference

Ported from xEdit / FO76Edit:

- Record header: 24 bytes (signature, dataSize, flags, formID, formVersion, …)
- GRUP header: 24 bytes; `groupSize` includes the header
- Subrecords: 6-byte header + payload; **XXXX** oversized rule supported
- FO76 FormIDs: full-slot (`high byte` = master index, low 24 bits = object ID)

See `TES5Edit/Core/wbImplementation.pas` and `TES5Edit/Core/wbDefinitionsFO76.pas` for the canonical definitions.

## Regenerating the schema

```bash
python3 tools/extractor/extract.py
# writes schema/fo76.json (requires TES5Edit sources at ../TES5Edit)
```

The extractor is best-effort; some Pascal closure deciders (conditions, perk effects) still need hand-modeling.

## License

No license file is included yet. xEdit/TES5Edit is used read-only as a reference for format and field definitions.
