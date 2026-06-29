# Todo: Harden Binary Parsing, Decompression, and Cache Handling

## Status

Still relevant. Some parsing paths already return controlled errors or raw
fallbacks, but decompression limits and `.esm.idx` cache handling remain broad
trust boundaries.

The `.esm.midx` corrupt-sidecar validation (overflow-safe `try_load`, exact-size
length guard, OOB-safe hot path) is tracked and implemented by **#13**.

## Context

The parser processes large untrusted binary files and sidecar caches. Bounds
checks, declared-size handling, and cache reads should be consistent enough that
malformed data cannot panic, allocate excessively, or leave partial cache files.

## Remaining Scope

- **Decompression size cap** (`src/compress.rs:8-23,29`): neither `decompress_lz4`
  nor `decompress_zlib` has a maximum output-size limit. Declared sizes from the
  binary (`u32::from_le_bytes` cast to `usize`, `compress.rs:29`) flow straight to
  `Vec::with_capacity` / `lz4_flex::decompress`. Add an explicit `MAX_DECOMP_SIZE`
  constant and reject unreasonable declared sizes (e.g. > 64 MiB for a single
  record) before allocation in both functions and in callers
  (`reader.rs:134` → `decompress_record_data`, `ba2.rs:167`).
- **`.esm.idx` atomic write** (`src/index.rs:176-179`): `save_cache` truncates the
  real cache path with `File::create` and writes directly. A crash mid-write leaves
  a corrupt `.esm.idx`. Switch to write-to-temp-file + `fs::rename` for atomicity.
- **`.esm.idx` pre-read sanity limit** (`src/index.rs:324-346`): `try_load_cache`
  reads the entire file into RAM unconditionally (`fs::read`) then deserializes. Add
  a file-size check before reading to reject obviously oversized caches (e.g. > 1 GiB)
  so corrupt or adversarial caches cannot force large allocations.
- **Checked arithmetic on attacker-controlled sizes**: several hot paths use raw
  `+`/`* as usize` that can overflow in release builds (debug panics; release wraps
  to a small value, passing the subsequent `> data.len()` guard). Convert to
  `checked_mul`/`checked_add` (returning an error on overflow) for:
  - `ba2.rs:80`: `records_start + file_count as usize * RECORD_SIZE`
  - `strings.rs:61-64`: `count as usize * 8` (index size) and `index_start + index_size`,
    `data_start + data_size as usize`
  - `reader.rs:127-128`: `offset as usize + HEADER_SIZE + hdr.data_size as usize`
  - `format.rs:65` (`total_size`): `HEADER_SIZE + self.data_size as u64`
  - Note: `walk_structure`/`parse_subrecords` (`reader.rs:218,236,323`) already use
    `saturating_*`; leave those as-is.
- **Strict vs best-effort policy**: document (in a code comment or module-level doc)
  the chosen behavior for malformed record and subrecord payloads — currently
  "controlled error or raw fallback, never panic" per the decoder invariant. Confirm
  the decompression and cache paths follow the same policy.

## Files

- `src/compress.rs`
- `src/index.rs`
- `src/reader.rs`
- `src/ba2.rs`
- `src/strings.rs`
- `src/format.rs`

## Acceptance Criteria

- Truncated TES4 payloads, record payloads, GRUPs, subrecords, BA2 tables, and
  string tables return controlled errors or raw fallbacks.
- Decompression refuses declared output sizes above `MAX_DECOMP_SIZE`; callers
  reject oversized declared sizes before calling into decompress functions.
- `.esm.idx` cache files are written via temp-file plus `fs::rename`.
- Oversized or corrupt `.esm.idx` files are rejected before large allocations.
- Attacker-controlled size multiplications use `checked_*` and return errors on overflow.
- Existing valid ESM, BA2, `.esm.idx` files still parse or load successfully.

## Verification

- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- New unit tests (no real game data required):
  - Decompression with an unreasonably large declared output size is rejected before
    allocation.
  - Corrupt or oversized `.esm.idx` file is rejected without large allocations.
  - Atomic `.esm.idx` save: simulate a crash (or just verify temp+rename code path)
    — no partial cache file left after a failed write.
  - Truncated record header / subrecord / BA2 table entry / string-table index:
    returns an error, does not panic.
  - BA2 table with `file_count` large enough to overflow `records_start + file_count * RECORD_SIZE`
    is rejected.
  - String-table with `count` large enough to overflow `count * 8` is rejected.
