# CLAUDE.md — ba2

Guidance for Claude Code when working in this Rust crate.

## Commands

```sh
just                                                # fmt + clippy + test (full local CI pass)
cargo build                                         # debug build
cargo build --release                               # release build (binary: target/release/ba2)
cargo run --bin ba2 -- <args>                       # run CLI (e.g. -- info archive.ba2)
cargo test                                          # run all tests (~121 across tests/ and inline modules)
cargo clippy --all-targets -- -D warnings           # lint (deny warnings; matches CI)
cargo fmt --check                                   # verify formatting (matches CI)
```

No test framework beyond `cargo test` is used.

## Before committing

Run `just` (fmt + clippy + test) and make sure it passes before every commit. Clippy runs with `-D warnings` — fix warnings rather than silencing them with `#[allow]` without cause. Never commit with failing or skipped checks.

## Architecture

Clean layering — edit at the right level:

| Module | Purpose |
|---|---|
| `format.rs` | Binary (de)serialization: `Header`/`Record` (GNRL), `TexRecord`/`TexChunk` (DX10), `ArchiveKind`, magic/tag constants, `read_*/write_*` |
| `dds.rs` | DDS header synthesis (`synth_header`) and parsing (`parse_header`) for the 15 `DXGI_FORMAT` values FO76 ships |
| `hash.rs` | Bethesda path hashing: `beth_crc`, `hash_path` |
| `compress.rs` | Codec dispatch: `Codec` enum, `compress_entry`, `decompress`, LZ4/zlib helpers |
| `reader.rs` | `Ba2Archive` (memory-mapped read, name index), `Ba2Entry`, `EntryData` (GNRL vs `Texture`), `TextureInfo` |
| `writer.rs` | `write_ba2`, `WriteOptions` — two-pass streaming writer, GNRL and DX10 |
| `extract.rs` | `extract_all`, `extract_one`, `ExtractOptions`, `safe_output_path` |
| `bin/cli.rs` | Thin CLI over the library API — clap subcommands `info`, `list`, `extract`, `create` |

Public API re-exported from `lib.rs`: `ArchiveKind`, `Codec`, `Ba2Archive`, `Ba2Entry`, `EntryData`, `TextureInfo`, `extract_all`, `extract_one`, `ExtractOptions`, `write_ba2`, `WriteOptions`, plus the `dds` module.

esm maintains its own minimal read-only BA2 reader (BTDX header, GNRL record layout, LZ4
decompress) instead of depending on this crate — see
`../esm/docs/adr/0009-ba2-duplication-is-deliberate.md`. The two share no code.

## Conventions to Follow

- **Error handling**: `anyhow` everywhere — `Result<T>` (no `Box<dyn Error>`), `bail!` for validation failures, `.context()`/`.with_context()` to attach path/operation info. **No custom error enum** — do not add one.
- **Serialization**: explicit little-endian byte reads/writes (no `serde`, no `binrw`) — this keeps the on-disk layout clear and directly testable. Fixed-offset field reads go through the local `read_u16`/`read_u32` helpers in `format.rs`/`dds.rs` while offsets stay explicit. Do not introduce derive-based serialization.
- **Documentation**: every module gets a `//!` module-level doc comment explaining purpose and design rationale; public items get `///` doc comments. Maintain this density when adding code.
- **Tests**: most tests live in `tests/` (one file per module: `format`, `dds`, `hash`, `compress`, `reader`, `writer`, `extract`), plus shared helpers in `tests/common/mod.rs` (`make_test_archive` for GNRL, `make_test_texture_archive` for DX10).  Tests that exercise **private** symbols stay colocated as `#[cfg(test)]` blocks: `extract.rs` (`safe_output_path`), `writer.rs` (`dx10_chunk_count`), and `bin/cli.rs` (source collectors).  All tests use synthetic in-memory data — no real BA2 file required (real archives are several GiB; validate DX10 changes against them manually via the CLI, not in the test suite).  Run with `cargo test`.
- **Style**: section-divider comments (`// ── ... ─`) used throughout — match existing style.

## Critical Invariants — Do Not Break

- **GNRL and DX10 are both supported for read and write; other archive types are not**: `Ba2Archive::open` dispatches on `ArchiveKind::from_tag` and rejects anything else with an explicit error. Do not silently skip the type check. The read/write paths are symmetric — don't special-case one direction without the other.
- **`packed_size == 0` means stored uncompressed**: this is the on-disk sentinel (not a bug), for both a GNRL entry's blob and a DX10 chunk. `Ba2Entry::is_compressed()` and each `TexChunk` follow this. Do not change this convention.
- **DX10 chunk sentinel and `chunk_header_size`**: every chunk record ends in the same `0xBAADF00D` sentinel as GNRL (`format::PADDING`), and every texture header's `chunk_header_size` field must read as `24` (`TEX_CHUNK_HEADER_SIZE`) — `read_tex_record` bails otherwise rather than trusting an unexpected value.
- **DX10 mip-chunking policy is an area rule, not xEdit's `w>=512 && h>=512`**: `dx10_chunk_count` in `writer.rs` starts a texture at 1 chunk and adds one per mip level while `chunk_count < 4`, mips remain, and `width*height >= 512*512` (halving both after each step); cubemaps are never chunked. This was ground-truthed against all 250,722 texture entries shipped with FO76 (0 mismatches) — xEdit's per-axis rule diverges on non-square textures. Do not "simplify" this back to the per-axis rule.
- **A DX10 entry is not a `.dds` file on disk**: `Ba2Archive::read` synthesizes a full `DDS_HEADER` (+ `DDS_HEADER_DXT10` where the format needs it, see `dds.rs`) and concatenates decompressed mip chunks in order. Every decompressed chunk's length is asserted against its on-disk `unpacked_size` before appending — `decompress_zlib` treats that value as a capacity hint only, so a silent length mismatch would otherwise produce a corrupt `.dds` rather than an error.
- **`dds.rs` only supports the 15 `DXGI_FORMAT` values FO76 actually ships**: an unrecognized `dxgi_format` (read) or DDS pixel format (parse, for `create`) is a hard error naming the value, never a guess. Do not add speculative format support without ground-truthing it the way the existing table was.
- **Bethesda CRC is non-standard**: poly `0xEDB88320`, init 0, **no final XOR**. It differs from standard CRC-32 (which uses init `0xFFFFFFFF` and final XOR). Do not "fix" it to standard CRC-32 — the hashes must match the game's own values.
- **`unsafe { Mmap::map(...) }` in `reader.rs`**: the SAFETY comment documents why the invariant holds (the mmap lifetime is tied to `Ba2Archive`). Keep this comment accurate if you touch the reader.
- **`safe_output_path` in `extract.rs`**: rejects `..` components, absolute paths, and Windows drive/prefix specifiers, then prefix-checks that the resolved path stays under `out_dir`. **Do not weaken these checks** — they prevent path-traversal when extracting untrusted archives.
- **Two-pass writer**: `write_ba2` compresses each source into a `tempfile::NamedTempFile` (Pass 1), then streams blobs while writing the header+records (Pass 2). Offsets are computed arithmetically — no seeking. Do not introduce seeking or in-memory buffering of all blobs. This applies to both `write_gnrl` and `write_dx10`.

## Toolchain

Pinned to **Rust 1.97.1** via `rust-toolchain.toml` (components: `rustfmt`, `clippy`), edition **2024**.

`rust-version = "1.97"` in `Cargo.toml`, mirrored as `msrv` in `clippy.toml`, deliberately tracks the pinned toolchain instead of the true language floor. Edition 2024 selects Cargo's MSRV-aware dependency resolver, which reads `rust-version` as a ceiling when picking dependency versions — declaring a lower floor silently resolves dependencies to older releases. Keep the two values in lockstep with the pin when bumping.
