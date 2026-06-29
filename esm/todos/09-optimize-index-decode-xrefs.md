# Todo: Optimize Index, Decode, and Xref Hot Paths

## Status

Still relevant. The compact `.esm.midx` path improved cold FormID lookup. Reverse-
reference dedup is now **done** (commit `93d2894`, `index.rs:231-236`,
`tests/refs.rs::referenced_by_deduplicates_within_record`) and is removed from
scope. The remaining items cover repeated type listing, decoder subrecord
consumption, unnecessary payload copies, xref false positives, and Electron
responsiveness.

## Context

Several interactive operations still repeatedly scan or allocate across large
indexes. This is noticeable with a multi-million-record ESM and becomes more
visible in the Electron browser when expanding groups or building reverse
references.

## Remaining Scope

- **Cached records-by-type index** (`src/index.rs:103-112`): `records_by_type`
  iterates all of `form_index`, filters by signature, and sorts on every single
  call. It is called from `lib.rs:306`, `lib.rs:487`, `ipc.rs:464`, and twice in
  `curves.rs`. Build a secondary `HashMap<Signature, Vec<FormId>>` alongside
  `form_index` at index-build time and maintain it on updates.
- **`Vec::remove(0)` in `decode.rs`** (`decode.rs:1383-1394`): `take_first` removes
  the front element of a `Vec<&OwnedSubrecord>` with `v.remove(0)`, an O(n) shift.
  It is the primary subrecord-consumption primitive, called from ~14 sites
  (`decode.rs:260,288,302,314,329,381,397,408,422,529,656,668,681,703`). Replace
  with a VecDeque (pop_front) or an index cursor over a shared slice.
- **Full-payload copies for uncompressed records** (`src/reader.rs:133-138`):
  `parse_record_at` unconditionally calls `raw.to_vec()` for uncompressed records
  (a full copy of the mmap slice) then copies again in `parse_subrecords_owned`
  (`reader.rs:287`: `data: s.data.to_vec()`). A borrowed variant exists
  (`parse_subrecords` returning `Subrecord<'a>`) but is unused on this path. Also,
  `src/diff.rs:136-145` copies both record payloads via `record_payload_at` purely
  for a byte-equality comparison — pass borrowed slices instead.
- **Schema/typed xref harvesting** (`src/index.rs:416-437`): xref collection walks
  decoded JSON and grabs any string starting with `0x`/`0X` that parses as a
  FormID. Non-FormID hex values (hashes, color values, flag masks) can produce false
  positive references. The `form_index.contains_key` filter (`index.rs:233`) catches
  most false positives at query time, but the harvest is broader than necessary.
  Replace with a schema-guided or typed FormID collection path so the harvest
  considers only fields whose schema kind is `FormId` or a known reference type.
  (Same pattern in `diff.rs:277-300` `collect_formid_refs`.)
  Note: **distinct from #14** — #14 generalizes the traversal direction (multi-hop
  reverse walk); this is about the accuracy of how refs are *harvested* during index
  build.
- **Async/progress for expensive first-time builds**: first `record_by_edid` call
  builds the EditorID index over ~5.8M records; first xref build is similarly heavy.
  Add progress reporting, async execution, or cancellation so the Electron UI
  remains responsive. Coordinates with **#05** (async N-API methods) — implement
  the async pattern once, consistently.

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
- Reverse-reference output contains no duplicates (done) and minimizes false
  positives from non-FormID hex strings.
- Diff and record decode paths avoid unnecessary allocations for uncompressed
  records where practical.
- Interactive UI operations remain responsive during expensive index work.

## Verification

- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- New focused tests:
  - Records-by-type: calling `records_by_type` twice returns the same sorted order
    and does not re-scan (add a counter or check determinism across multiple calls).
  - Xref false-positive elimination: a decoded hex string that is not a valid
    FormID in `form_index` does not appear in the reverse-reference index.
  - (xref dedup already covered by `tests/refs.rs::referenced_by_deduplicates_within_record`)
- Compare representative `list`, `tree`, `get`, `referenced_by`, and `diff`
  timings before and after the records-by-type and decode-copy changes.
