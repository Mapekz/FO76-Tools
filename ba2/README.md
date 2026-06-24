# ba2

A Rust CLI and library for reading, extracting, and creating Bethesda **BA2 / BTDX GNRL** archives, as used by Fallout 76 and Fallout 4.

- **Fallout 76** GNRL archives — raw LZ4 block compression or uncompressed stored entries.
- **Fallout 4** GNRL archives — zlib/DEFLATE compressed or stored entries.
- **DX10** texture archives are detected and rejected with a clear error (not supported).

## Requirements

- Rust stable **~1.87 or newer** (uses `u16::is_multiple_of`, stabilised in 1.87; no `rust-toolchain.toml` is present).

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

Prints magic (`BTDX`), version, archive type (`GNRL`), file count, name-table offset, and file size.

### `list` — List entries

```sh
ba2 list SeventySix-Startup.ba2
ba2 list SeventySix-Startup.ba2 --long
```

Without `--long`: one entry path per line, entry count on stderr.  
With `--long`: tab-aligned columns for unpacked size, packed size, codec (`store` or `lz4/zlib`), `NAME_HASH`, `DIR_HASH`, and `.ext`.

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
```

| Flag | Default | Description |
|---|---|---|
| `--from <DIR>` | — | Recursively pack all files under DIR |
| `--files <F...>` | — | Explicit source files to pack |
| `--list <FILE>` | — | Newline-delimited list file |
| `--base <PREFIX>` | — | Strip prefix to derive archive paths (used with `--files` or `--list`) |
| `--compress <CODEC>` | `lz4` | Compression codec: `lz4` (FO76), `zlib` (FO4), `store` |

## Library API

The crate exposes a stable public API. Key re-exports from `ba2`:

```rust
use ba2::{Ba2Archive, Ba2Entry, Codec, ExtractOptions, WriteOptions, write_ba2, extract_all, extract_one};

// Read an archive
let archive = Ba2Archive::open("SeventySix-Startup.ba2")?;

// List all entries
for entry in archive.list() {
    println!("{} ({} bytes)", entry.name, entry.unpacked_size);
}

// Read a specific entry (auto-detects compression)
let bytes = archive.read("strings/en/interface.dlstrings", Codec::Auto)?;

// Extract everything to a directory
let opts = ExtractOptions { codec: Codec::Auto, filter: None };
let count = extract_all(&archive, "./out".as_ref(), &opts)?;

// Create a new archive
let files = vec![
    ("strings\\en\\interface.dlstrings".to_string(), "./out/strings/en/interface.dlstrings".into()),
];
let opts = WriteOptions { codec: Codec::Lz4, ..Default::default() };
write_ba2("output.ba2", &files, &opts)?;
```

## Tests

All tests are colocated inline (`#[cfg(test)]`) in each module and cover the binary format, hashing, compression round-trips, reader, writer, and path-traversal hardening:

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
