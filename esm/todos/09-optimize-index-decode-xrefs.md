# Todo: Optimize Index, Decode, and Xref Hot Paths

## Status

Still relevant. The compact `.esm.midx` path improved cold FormID lookup, but it
does not address repeated type listing, decoder subrecord consumption, reverse
reference harvesting, or Electron responsiveness during first-time index builds.

## Context

Several interactive operations still repeatedly scan or allocate across large
indexes. This is noticeable with a multi-million-record ESM and becomes more
visible in the Electron browser when expanding groups or building reverse
references.

## Remaining Scope

- Add a cached records-by-type index instead of scanning and sorting
  `form_index` in `Index::records_by_type` on each call.
- Avoid `Vec::remove(0)` in `decode.rs` subrecord consumption.
- Reduce full-payload copies for uncompressed records where borrowed payloads
  are safe.
- Deduplicate reverse references per referencer while preserving deterministic
  output.
- Replace JSON-wide xref harvesting with a schema-guided or typed FormID
  collection path so non-FormID hex strings do not become false references.
- Add progress, async execution, or other responsiveness improvements around
  expensive first-time EditorID and xref builds for N-API/Electron callers.

## Files

- `src/index.rs`
- `src/decode.rs`
- `src/reader.rs`
- `src/lib.rs`
- `src/diff.rs`
- `bindings/napi/src/lib.rs`
- `app/src/renderer/src/App.tsx`
- `app/src/renderer/src/components/RecordTree.tsx`

## Acceptance Criteria

- Listing records by type avoids full-index scans on repeated calls.
- Subrecord consumption in `decode.rs` is linear in the number of subrecords.
- Reverse-reference output contains no duplicates and avoids false positives
  from non-FormID hex strings.
- Diff and record decode paths avoid unnecessary allocations for uncompressed
  records where practical.
- Interactive UI operations remain responsive during expensive index work.

## Verification

- `cargo test`
- Add focused tests for records-by-type ordering, duplicate xref elimination,
  and FormID harvesting correctness.
- Compare representative `list`, `tree`, `get`, `referenced_by`, and `diff`
  timings before and after.
- `cargo clippy --all-targets -- -D warnings`
