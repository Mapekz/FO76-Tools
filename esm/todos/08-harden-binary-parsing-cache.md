# Todo: Harden Binary Parsing, Decompression, and Cache Handling

## Context

The parser processes large untrusted binary files and sidecar caches. Several paths have uneven bounds checks, trust declared decompression sizes, or read cache files before applying size limits.

## Scope

- Make record, GRUP, TES4, BA2, and string-table bounds checks consistent.
- Add explicit maximum sizes for zlib/LZ4 decompression outputs.
- Validate size arithmetic with checked or saturating operations as appropriate.
- Decide and document strict vs best-effort behavior for malformed records.
- Make `.esm.idx` cache writes atomic.
- Add sanity limits before reading or deserializing cache files.
- Preserve the decoder rule: malformed input should produce errors or raw fallbacks, never panics.

## Files

- `src/reader.rs`
- `src/ba2.rs`
- `src/strings.rs`
- `src/compress.rs`
- `src/index.rs`
- `src/format.rs`

## Acceptance Criteria

- Truncated TES4 payloads, record payloads, GRUPs, subrecords, BA2 tables, and string tables return controlled errors or raw fallbacks.
- Decompression refuses unreasonable declared output sizes.
- Cache files are written via temp-file plus rename.
- Oversized or corrupt `.esm.idx` files are rejected without large unnecessary allocations.
- Existing valid ESM and BA2 files still parse successfully.

## Verification

- `cargo test`
- New unit tests for truncated headers, oversized subrecords, bad BA2 offsets, bad string-table offsets, bad decompression sizes, and corrupt cache files.
- `cargo clippy --all-targets -- -D warnings`
