# CLAUDE.md — ba2

Guidance for Claude Code when working in this Rust crate.

## Commands

```sh
cargo build                    # debug build
cargo build --release          # release build (binary: target/release/ba2)
cargo run --bin ba2 -- <args>  # run CLI (e.g. -- info archive.ba2)
cargo test                     # run all tests (~71 across tests/ and inline modules)
cargo clippy                   # lint
cargo fmt                      # format
```

No test framework beyond `cargo test` is used.

## Architecture

Clean layering — edit at the right level:

| Module | Purpose |
|---|---|
| `format.rs` | Binary (de)serialization: `Header`, `Record`, magic/tag constants, `read_*/write_*` |
| `hash.rs` | Bethesda path hashing: `beth_crc`, `hash_path` |
| `compress.rs` | Codec dispatch: `Codec` enum, `compress_entry`, `decompress`, LZ4/zlib helpers |
| `reader.rs` | `Ba2Archive` (memory-mapped read, name index), `Ba2Entry` |
| `writer.rs` | `write_ba2`, `WriteOptions` — two-pass streaming writer |
| `extract.rs` | `extract_all`, `extract_one`, `ExtractOptions`, `safe_output_path` |
| `bin/cli.rs` | Thin CLI over the library API — clap subcommands `info`, `list`, `extract`, `create` |

Public API re-exported from `lib.rs`: `Codec`, `Ba2Archive`, `Ba2Entry`, `extract_all`, `extract_one`, `ExtractOptions`, `write_ba2`, `WriteOptions`.

## Conventions to Follow

- **Error handling**: `anyhow` everywhere — `Result<T>` (no `Box<dyn Error>`), `bail!` for validation failures, `.context()`/`.with_context()` to attach path/operation info. **No custom error enum** — do not add one.
- **Serialization**: explicit little-endian byte reads/writes (no `serde`, no `binrw`). This keeps the on-disk layout "crystal-clear and testable" — do not introduce derive-based serialization.
- **Documentation**: every module gets a `//!` module-level doc comment explaining purpose and design rationale; public items get `///` doc comments. Maintain this density when adding code.
- **Tests**: most tests live in `tests/` (one file per module: `format`, `hash`, `compress`, `reader`, `writer`, `extract`), plus shared helpers in `tests/common/mod.rs`.  Tests that exercise **private** symbols stay colocated as `#[cfg(test)]` blocks: `extract.rs` (`safe_output_path`) and `bin/cli.rs` (source collectors).  All tests use synthetic in-memory data — no real BA2 file required.  Run with `cargo test`.
- **Style**: section-divider comments (`// ── ... ─`) used throughout — match existing style.

## Critical Invariants — Do Not Break

- **GNRL only**: `Ba2Archive::open` rejects DX10 and any non-GNRL archive type with an explicit error. Do not add DX10 support without a separate path; do not silently skip the type check.
- **`packed_size == 0` means stored uncompressed**: this is the on-disk sentinel (not a bug). In `Ba2Entry`, `is_compressed()` returns `packed_size != 0`. Do not change this convention.
- **Bethesda CRC is non-standard**: poly `0xEDB88320`, init 0, **no final XOR**. It differs from standard CRC-32 (which uses init `0xFFFFFFFF` and final XOR). Do not "fix" it to standard CRC-32 — the hashes must match the game's own values.
- **`unsafe { Mmap::map(...) }` in `reader.rs`**: the SAFETY comment documents why the invariant holds (the mmap lifetime is tied to `Ba2Archive`). Keep this comment accurate if you touch the reader.
- **`safe_output_path` in `extract.rs`**: rejects `..` components, absolute paths, and Windows drive/prefix specifiers, then prefix-checks that the resolved path stays under `out_dir`. **Do not weaken these checks** — they prevent path-traversal when extracting untrusted archives.
- **Two-pass writer**: `write_ba2` compresses each source into a `tempfile::NamedTempFile` (Pass 1), then streams blobs while writing the header+records (Pass 2). Offsets are computed arithmetically — no seeking. Do not introduce seeking or in-memory buffering of all blobs.

## Toolchain

Pinned to **Rust 1.96.0** via `rust-toolchain.toml` (components: `rustfmt`, `clippy`). MSRV **1.87** (`u16::is_multiple_of`), declared via `rust-version = "1.87"` in `Cargo.toml` and mirrored in `clippy.toml`.
