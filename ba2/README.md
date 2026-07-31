# ba2

A Rust CLI and library for reading, extracting, and creating Bethesda **BA2 / BTDX** archives — both
**GNRL** (general files) and **DX10** (textures) — as used by Fallout 76 and Fallout 4.

- **Fallout 76** GNRL archives — raw LZ4 block compression or uncompressed stored entries.
- **Fallout 4** GNRL archives — zlib/DEFLATE compressed or stored entries.
- **DX10** texture archives — each entry is a texture header (dimensions, mip count, `DXGI_FORMAT`)
  plus zlib-compressed mip chunks, not a `.dds` file on disk. `ba2 extract` synthesizes a complete
  `.dds` file per entry; `ba2 create --type dx10` does the inverse, splitting a source `.dds` into
  chunks per Archive2's own mip-chunking policy.

## Requirements

- Toolchain pinned to **Rust 1.97.1** via `rust-toolchain.toml` (rustup installs it automatically).
- Edition **2024**.
- `rust-version` in `Cargo.toml` tracks the pinned toolchain (**1.97**) rather than the true language
  floor. Edition 2024 selects Cargo's MSRV-aware dependency resolver, which treats `rust-version` as
  a ceiling on dependency selection — a lower value would silently hold dependencies back at older
  releases. This crate has no external consumers, so there is nothing to gain from a low MSRV.

## Build

```sh
cargo build --release
# Binary is at: target/release/ba2
```

## CLI Usage

```sh
ba2 <subcommand> [options]
```

### `info` — Print archive header summary

```sh
ba2 info SeventySix-Startup.ba2
```

Prints magic (`BTDX`), version, archive type (`GNRL` or `DX10`), file count, name-table offset, and
file size. For a DX10 archive, also prints total chunk count and a `DXGI_FORMAT` histogram.

### `list` — List entries

```sh
ba2 list SeventySix-Startup.ba2
ba2 list SeventySix-Startup.ba2 --long
```

Without `--long`: one entry path per line, entry count on stderr.
With `--long`, the columns depend on archive kind:
- **GNRL**: unpacked size, packed size, codec (`store` or `lz4/zlib`), `NAME_HASH`, `DIR_HASH`, `.ext`.
- **DX10**: pixel dimensions, format name, mip count, chunk count, packed/unpacked size — with a
  trailing `+cube` marker on cubemap entries.

### `extract` — Extract entries

```sh
# Extract everything to ./out/
ba2 extract SeventySix-Startup.ba2 --out ./out

# Extract only entries matching a glob
ba2 extract SeventySix-Startup.ba2 --out ./out --filter "strings/*"

# Extract specific named entries
ba2 extract SeventySix-Startup.ba2 --out ./out strings/en/interface.dlstrings

# Force a specific decompression codec (default: auto-detect)
ba2 extract SeventySix-Startup.ba2 --out ./out --format lz4
```

| Flag | Default | Description |
|---|---|---|
| `--out <DIR>` | `.` | Output directory (created if absent) |
| `--filter <GLOB>` | — | Glob pattern to filter entries (e.g. `strings/*`) |
| `--format <CODEC>` | `auto` | Decompression hint: `auto`, `lz4`, `zlib`, `store` |
| `[FILES...]` | all | Specific archive paths to extract |

### `create` — Create a new BA2 archive

Three mutually exclusive source modes:

```sh
# From a directory — archive paths are relative to DIR, lowercased, backslash-joined
ba2 create output.ba2 --from ./assets/

# From explicit files — archive path = file name (or relative to --base)
ba2 create output.ba2 --files data/strings.dlstrings data/other.txt
ba2 create output.ba2 --files /abs/path/strings.dlstrings --base /abs/path

# From a list file (newline-separated; # comments and blank lines ignored)
# Each line: `source_path`  OR  `archive_path<TAB>source_path`
ba2 create output.ba2 --list filelist.txt

# DX10 texture archive — every source must be a .dds
ba2 create textures.ba2 --type dx10 --from ./textures/
```

| Flag | Default | Description |
|---|---|---|
| `--from <DIR>` | — | Recursively pack all files under DIR |
| `--files <F...>` | — | Explicit source files to pack |
| `--list <FILE>` | — | Newline-delimited list file |
| `--base <PREFIX>` | — | Strip prefix to derive archive paths (used with `--files` or `--list`) |
| `--type <KIND>` | `gnrl` | Archive kind: `gnrl` or `dx10` (textures; sources must be `.dds`) |
| `--compress <CODEC>` | `lz4` for `gnrl`, `zlib` for `dx10` | Compression codec: `lz4`, `zlib`, `store` |

## Library API

The crate exposes a stable public API. Key re-exports from `ba2`:

```rust
use ba2::{
    ArchiveKind, Ba2Archive, Ba2Entry, Codec, ExtractOptions, WriteOptions, write_ba2, extract_all,
    extract_one,
};

// Read an archive (works for both GNRL and DX10)
let archive = Ba2Archive::open("SeventySix-Startup.ba2")?;

// List all entries
for entry in archive.list() {
    println!("{} ({} bytes)", entry.name, entry.unpacked_size());
}

// Read a specific entry (auto-detects compression). For a DX10 entry this
// returns a complete synthesized .dds file — header + concatenated mip data.
let bytes = archive.read("strings/en/interface.dlstrings", Codec::Auto)?;

// Extract everything to a directory
let opts = ExtractOptions { codec: Codec::Auto, filter: None };
let count = extract_all(&archive, "./out".as_ref(), &opts)?;

// Create a new GNRL archive
let files = vec![
    ("strings\\en\\interface.dlstrings".to_string(), "./out/strings/en/interface.dlstrings".into()),
];
let opts = WriteOptions { codec: Codec::Lz4, ..Default::default() };
write_ba2("output.ba2", &files, &opts)?;

// Create a new DX10 (texture) archive — sources must be .dds files
let files = vec![("textures\\foo_d.dds".to_string(), "./out/textures/foo_d.dds".into())];
let opts = WriteOptions { kind: ArchiveKind::Dx10, codec: Codec::Zlib, ..Default::default() };
write_ba2("textures.ba2", &files, &opts)?;

// Inspect a texture entry's dimensions/format without extracting
if let Some(t) = archive.list()[0].texture() {
    println!("{}x{} {:?}, {} mips, {} chunks", t.width, t.height, t.dxgi_format, t.mip_count, t.chunks.len());
}
```

## Tests

~121 tests across `tests/` (integration, one file per module) and three inline
`#[cfg(test)]` modules for private-symbol coverage:

| File | What it covers |
|---|---|
| `tests/format.rs` | Header, GNRL record, and DX10 texture/chunk record (de)serialization, byte-layout pins |
| `tests/dds.rs` | DDS header synthesis per format (legacy FourCC, RGB/luminance masks, DXT10 extension), `parse_header` round-trips |
| `tests/hash.rs` | `beth_crc` golden value, `hash_path` vectors + edge cases |
| `tests/compress.rs` | LZ4/zlib round-trips, `is_zlib` sniffing, store-fallback |
| `tests/reader.rs` | Happy-path reads, compressed entries, all `open()` error branches, DX10 multi-chunk reassembly, cubemap flag, unknown-format and corrupt-chunk error paths |
| `tests/writer.rs` | Codec round-trips, empty archive, mixed compress+store, DX10 create round-trips (small/multi-chunk/zlib/cubemap/DXT10-extension) |
| `tests/extract.rs` | `extract_all`, `extract_one`, glob filter, DX10 texture entry extracted as `.dds` |
| `src/extract.rs` (inline) | `safe_output_path` path-traversal hardening (private fn) |
| `src/writer.rs` (inline) | `dx10_chunk_count` — the mip-chunking policy table (private fn) |
| `src/bin/cli.rs` (inline) | `collect_from_dir/files/list`, `derive_archive_path` (binary-private) |

All tests use synthetic in-memory data — no real BA2 archive file required (real DX10 archives are
several GiB; changes touching `dds.rs`/`reader.rs`/`writer.rs`'s DX10 paths should also be spot-checked
against a real archive via the CLI — see `ba2/CLAUDE.md`).

```sh
cargo test
```

## BA2 Format Primer

A GNRL BA2 file is laid out as follows:

| Section | Size | Notes |
|---|---|---|
| Header | 24 bytes | Magic `BTDX`, version 1, `GNRL` type, file count, name table offset |
| Records | N × 36 bytes | Per-entry: name hash, ext tag, dir hash, flags, data offset, packed/unpacked sizes, `0xBAADF00D` sentinel |
| Data blobs | variable | Back-to-back; `packed_size == 0` means stored uncompressed |
| Name table | variable | `u16 LE` length prefix + UTF-8 path per entry |

The Bethesda name-hashing uses a CRC-32 variant (poly `0xEDB88320`, init 0, no final XOR) over the lowercased file stem (name hash) and directory (dir hash). Constants and hashing were ground-truthed against real FO76 archives (`SeventySix - Localization.ba2`, 4,507 entries, 100% hash match).

### DX10 (texture) archives

Same 24-byte header (type tag `DX10`) and trailing name table, but the per-file records are a
texture header followed by a variable number of mip-chunk records — **not** a fixed 36-byte stride
like GNRL, so the record area must be walked sequentially rather than bulk-bounds-checked:

| Section | Size | Notes |
|---|---|---|
| Texture header | 24 bytes | `name_hash`, `ext` (always `dds\0`), `dir_hash`, `unk8`, `chunk_count`, `chunk_header_size` (always 24), `height`, `width`, `mip_count`, `dxgi_format`, `cubemap`, `tile_mode` |
| Chunk records | `chunk_count` × 24 bytes | Per-chunk: `data_offset`, `packed_size`/`unpacked_size`, `mip_first`/`mip_last`, `0xBAADF00D` sentinel |
| Data blobs | variable | Each chunk's bytes are zlib-compressed (or stored, `packed_size == 0`) |
| Name table | variable | Same format as GNRL, but real archives store forward slashes |

A texture entry is not a `.dds` file — `ba2::dds::synth_header` builds a standard `DDS_HEADER` (plus
`DDS_HEADER_DXT10` for formats without a legacy FourCC) from `dxgi_format`/dimensions/mips/cubemap,
and `Ba2Archive::read` decompresses and concatenates the chunks after it in order.

`create --type dx10` derives `chunk_count` from Archive2's own mip-chunking policy — start at 1
chunk, add one per mip level while `chunk_count < 4` and the *current* mip's pixel area is still
`>= 512x512` (cubemaps are never chunked) — ground-truthed against every DX10 archive shipped with
FO76 (250,722 texture entries, 0 mismatches). This is an area rule; it deliberately diverges from
xEdit's per-axis `width >= 512 && height >= 512` rule on non-square textures.

Format coverage is restricted to the 15 `DXGI_FORMAT` values FO76 actually ships (verified against
a live game install): `R16G16B16A16_FLOAT`/`_UNORM`, `R8G8B8A8_UNORM`/`_UNORM_SRGB`, `R8_UNORM`,
`BC1_UNORM`/`_UNORM_SRGB`, `BC3_UNORM`/`_UNORM_SRGB`, `BC4_UNORM`, `BC5_UNORM`/`_SNORM`,
`B8G8R8A8_UNORM`, `BC7_UNORM`/`_UNORM_SRGB`. `BC5_SNORM` is the dominant real-world normal-map
format, not `BC5_UNORM`. Anything else is a hard error naming the unhandled value.
