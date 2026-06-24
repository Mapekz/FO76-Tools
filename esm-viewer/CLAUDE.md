# fo76-esm-parser — CLAUDE.md

Cross-platform Rust library (`fo76_esm_parser`) + CLI (`fo76`) for reading Fallout 76 `.esm` plugin files. Parses the Bethesda binary record format, indexes records by FormID and EditorID, and decodes fields to `serde_json::Value` using a schema derived from xEdit/FO76Edit definitions. Designed as a reusable core for downstream tooling (MCP agents, data browsers).

## Commands

```bash
# Build
cargo build --release          # binary → target/release/fo76
cargo build                    # debug build

# Lint & format (always run before finishing)
cargo fmt                      # format in-place
cargo fmt --check              # check only (non-destructive)
cargo clippy --all-targets -- -D warnings   # treat all warnings as errors

# Test (currently 0 tests — see Testing section below)
cargo test

# CLI (game data must be present locally; see Game Data below)
ESM=SeventySix.esm
./target/release/fo76 info "$ESM"                           # TES4 header
./target/release/fo76 list "$ESM" --type WEAP --limit 20   # list weapons
./target/release/fo76 get  "$ESM" --edid AssaultRifle --pretty
./target/release/fo76 get  "$ESM" --formid 0x463F --pretty
./target/release/fo76 get  "$ESM" --formid 0x463F --raw --pretty  # no schema
```

## Architecture

Data flow:
```
bin/cli.rs
  └─ Database::open(path)          [lib.rs]
       ├─ EsmFile::open → mmap     [reader.rs]
       ├─ Index::build or load     [index.rs]  ← bincode cache (*.esm.idx)
       └─ Schema::load_embedded    [schema.rs] ← include_str!("../schema/fo76.json")
            └─ decode_record       [decode.rs] → serde_json::Value
```

### Module map

| File | Responsibility |
|---|---|
| `src/lib.rs` | `Database` facade: `open`, `record_by_formid`, `record_by_edid`, `list_by_type`, `record_raw` |
| `src/format.rs` | On-disk header structs (`RecordHeader`, `GroupHeader`, `SubrecordHeader`, `Signature`) and `parse` fns |
| `src/reader.rs` | `EsmFile` (mmap), record/GRUP tree walker, TES4 header, subrecord extraction |
| `src/compress.rs` | Zlib decompression of compressed records (`0x00040000` flag) |
| `src/formid.rs` | `FormId(u32)` newtype, hex/decimal parsing, `Display` |
| `src/index.rs` | FormID→offset map + EditorID→FormID map; `bincode` disk cache at `*.esm.idx` |
| `src/schema.rs` | Serde model for `schema/fo76.json`; `MemberDef` enum (internally tagged `kind`) |
| `src/decode.rs` | Schema-driven decoder → `serde_json::Value`; heuristic `advance_union` / `RArray` paths |
| `src/bin/cli.rs` | `clap` derive CLI with `info` / `get` / `list` subcommands |
| `schema/fo76.json` | Record field definitions — **embedded at compile time** |
| `tools/extractor/extract.py` | Python: Pascal DSL (xEdit) → `schema/fo76.json`; run manually |

## Repo-specific rules & gotchas

### Schema is compile-time embedded
`schema/fo76.json` is inlined via `include_str!` in `schema.rs`. **Editing it requires a rebuild.** For new record-type coverage, edit the JSON first rather than the Rust decode logic — most work is a JSON-editing task driven by `MemberDef` variants.

Regenerate from xEdit sources:
```bash
python3 tools/extractor/extract.py   # requires ../TES5Edit checkout as sibling
```
The extractor covers only 8 record types (AMMO, ARMO, PROJ, EXPL, WEAP, SPEL, MGEF, PERK). Hard Pascal closure deciders (conditions, VMAD, perk effects) emit `{"kind": "raw_fallback"}` — they still need hand-modeling.

### Decode output conventions
Internal JSON metadata keys are underscore-prefixed and must stay consistent:
- `_record_type`, `_unknown_record` — unknown record/field
- `_unmapped` — subrecords not consumed by the schema
- `_raw` — bytes emitted as hex because no schema matched
- `_unresolved` — LString ID with no loaded string table: `{"lstring_id": "0x...", "_unresolved": true}`

**The decoder must never panic on malformed input.** Unknown/malformed bytes → raw hex fallback, not a crash.

### Index cache (`*.esm.idx`)
Written next to the ESM on first run; keyed by file path, size, and mtime. Validated against `CACHE_VERSION` in `index.rs`. **Bump `CACHE_VERSION` whenever the cache layout changes** to force a cache rebuild.

### Binary format rules
- **All multi-byte values are little-endian** — use `byteorder::ReadBytesExt::read_u*::<LittleEndian>()` for fixed headers and `u*::from_le_bytes(...)` for payload slices.
- Record and GRUP headers are exactly **24 bytes**; subrecord headers are **6 bytes**.
- The **`XXXX` oversized subrecord rule**: a 6-byte `XXXX` subrecord whose `data_size` field carries the actual size precedes an oversized subrecord with `data_size = 0`. The reader handles this at `reader.rs:181-190` — preserve it when modifying the subrecord scanner.
- FormID layout: high byte = master-file index, low 24 bits = object ID.
- `decode.rs`'s `advance_union` and `RArray` paths are **acknowledged heuristics** (byte-count estimates, not exact counts). Change these with extra care and verify against real ESM output.

### Game data files
`SeventySix.esm`, `SeventySix.esm.idx`, `SeventySix - Localization.ba2`, `SeventySix - Startup.ba2` are **gitignored, large (hundreds of MB each), and non-redistributable**. Obtain from your own game install. **Never commit them. Never hardcode their paths in source** — they are always supplied at runtime via `Database::open(path)` / CLI args.

## Rust conventions (match the existing code)

### Error handling
- Use `anyhow::Result<T>` for all fallible functions (both library and CLI).
- Use `bail!(...)` for early validation errors; `.context("…")` / `.with_context(|| …)` to annotate errors at call sites.
- `thiserror` is declared as a dependency but currently unused. Use it when the public library API needs callers to `match` on specific error variants. Until then, keep `anyhow` throughout.
- **Do not `unwrap()` or `expect()` on untrusted input** (byte slices, malformed records). The existing pattern — bounds-check the slice, then `slice.try_into().unwrap()` — is acceptable only when the preceding bounds check makes the unwrap infallible; keep the check visually adjacent.

### Naming
| Item | Convention | Example |
|---|---|---|
| Types, traits, enums | `UpperCamelCase` | `FormId`, `MemberDef`, `DecodeContext` |
| Functions, methods, variables | `snake_case` | `parse_record_at`, `decode_struct_fields` |
| Modules | short lowercase | `format`, `reader`, `decode` |
| Constants | `SCREAMING_SNAKE_CASE` | `COMPRESSED_FLAG`, `CACHE_VERSION`, `HEADER_SIZE` |
| Domain signatures | match the ESM format | `TES4_SIG`, `EDID`, `XXXX` |

### Idioms to use
- **Newtype pattern** for domain wrappers: see `FormId(pub u32)`, `Signature(pub [u8; 4])`.
- **Serde attribute DSL** for the schema model: `#[serde(tag = "kind")]`, `#[serde(untagged)]`, `#[serde(default)]`, `#[serde(rename_all = "snake_case")]`.
- **Lifetime-borrowing** in hot-path structs (`Subrecord<'a>`, `DecodeContext<'a>`) to avoid allocation.
- **Closure generics**: `fn walk_records<F: FnMut(RecordMeta) -> anyhow::Result<()>>`.
- **Defensive arithmetic** on offsets: `saturating_add`, `.min(data.len())`, `.unwrap_or(default)`.
- Keep modules small and single-purpose; avoid cross-module reach-throughs when the `Database` facade suffices.

### `unsafe`
There is exactly one `unsafe` block in the codebase: `Mmap::map(&file)` in `reader.rs:66`. **Every `unsafe` block must have a `// SAFETY:` comment** explaining why the invariants are upheld — this one currently lacks it and should be fixed:
```rust
// SAFETY: We hold the file open for the lifetime of `Mmap`; no other process
// is expected to truncate the file while it is mapped.
let mmap = unsafe { Mmap::map(&file)? };
```

### Documentation
The crate exposes a public library API (`Database`, `FormId`, `RecordResult`, `ListEntry`). These should carry `///` doc comments so downstream consumers have rustdoc. Modules should have `//!` top-of-file summaries. Inline `//` comments are appropriate only for non-obvious logic (existing use: heuristic byte advances, the hand-rolled hex encoder, intentional size-mismatch ignores).

## Testing

**There are currently no tests.** The standard going forward:

- **Unit tests** (`#[cfg(test)]` in each module) for pure logic that doesn't need `SeventySix.esm`:
  - `formid.rs`: hex/decimal `FormId` parsing round-trips
  - `format.rs`: header `parse` on synthetic 24-byte / 6-byte slices
  - `schema.rs`: `Schema::load_embedded()` deserializes without error
  - `decode.rs`: decode of a hand-crafted `OwnedSubrecord` slice against a minimal schema
- **Integration tests** that need the real ESM: gate with `#[ignore]` and document the env-var convention so they can be opted-in locally (e.g. `RUST_TEST_ESM=SeventySix.esm cargo test -- --ignored`).

Goal: `cargo test` is always green on a fresh checkout with no game data.

## Commit conventions

- Short, capitalized, period-terminated subject line (≤ 72 chars), imperative body, `Co-authored-by:` trailer.
- Never stage `SeventySix.esm`, `*.esm.idx`, `*.ba2`, or `target/` — they are gitignored for a reason.

```
Add BA2 archive reader for localization string tables.

Implement a minimal FO76 BA2 parser sufficient to extract
.STRINGS/.DLSTRINGS/.ILSTRINGS files and resolve LString IDs.

Co-authored-by: Claude Opus 4 <noreply@anthropic.com>
```
