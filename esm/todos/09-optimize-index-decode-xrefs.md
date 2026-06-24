# Todo: Optimize Index, Decode, and Xref Hot Paths

## Context

Several interactive operations repeatedly scan or allocate across large indexes. This is noticeable with a multi-million-record ESM and becomes more visible in the Electron browser when expanding groups or building reverse references.

## Scope

- Add a cached records-by-type index instead of scanning and sorting `form_index` on each call.
- Avoid `Vec::remove(0)` in decode subrecord consumption.
- Reduce full-payload copies for uncompressed records where borrowed payloads are safe.
- Deduplicate reverse references while preserving deterministic output.
- Replace JSON-wide xref harvesting with a schema-guided or typed FormID collection path.
- Add progress or asynchronous behavior around expensive first-time xref builds for UI callers.

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
- Reverse-reference output contains no duplicates and avoids false positives from non-FormID hex strings.
- Diff and record decode paths avoid unnecessary allocations for uncompressed records where practical.
- Interactive UI operations remain responsive during expensive index work.

## Verification

- `cargo test`
- Add focused tests for records-by-type ordering, duplicate xref elimination, and FormID harvesting correctness.
- Compare representative `list`, `tree`, `get`, `referenced_by`, and `diff` timings before and after.
- `cargo clippy --all-targets -- -D warnings`
