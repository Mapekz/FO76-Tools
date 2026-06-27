# Todo: Harden Binary Parsing, Decompression, and Cache Handling

## Status

Still relevant. Some parsing paths already return controlled errors or raw
fallbacks, but decompression limits and `.esm.idx` cache handling remain broad
trust boundaries. `.esm.midx` corrupt-sidecar validation is tracked more
specifically in `13-fix-last-36h-review-findings.md`.

## Context

The parser processes large untrusted binary files and sidecar caches. Bounds
checks, declared-size handling, and cache reads should be consistent enough that
malformed data cannot panic, allocate excessively, or leave partial cache files.

## Remaining Scope

- Add explicit maximum output sizes for zlib and LZ4 decompression in
  `src/compress.rs`, and make callers reject unreasonable declared sizes before
  allocation.
- Audit record, GRUP, TES4, subrecord, BA2, and string-table arithmetic for
  checked or saturating operations where malformed sizes can overflow.
- Decide and document strict vs best-effort behavior for malformed record and
  subrecord payloads.
- Make `.esm.idx` writes in `src/index.rs` atomic via temp-file plus rename.
- Add sanity limits before reading or deserializing `.esm.idx` files so corrupt
  or oversized caches are rejected without large unnecessary allocations.
- Preserve the decoder rule: malformed input should produce errors or raw
  fallbacks, never panics.

## Files

- `src/reader.rs`
- `src/ba2.rs`
- `src/strings.rs`
- `src/compress.rs`
- `src/index.rs`
- `src/format.rs`
- `src/mindex.rs` only for the `#13` corrupt-sidecar overlap

## Acceptance Criteria

- Truncated TES4 payloads, record payloads, GRUPs, subrecords, BA2 tables, and
  string tables return controlled errors or raw fallbacks.
- Decompression refuses unreasonable declared output sizes.
- `.esm.idx` cache files are written via temp-file plus rename.
- Oversized or corrupt `.esm.idx` files are rejected without large unnecessary
  allocations.
- Existing valid ESM, BA2, `.esm.idx`, and `.esm.midx` files still parse or load
  successfully.

## Verification

- `cargo test`
- New unit tests for truncated headers, oversized subrecords, bad BA2 offsets,
  bad string-table offsets, bad decompression sizes, corrupt `.esm.idx` files,
  and the `.esm.midx` cases owned by `#13`.
- `cargo clippy --all-targets -- -D warnings`
