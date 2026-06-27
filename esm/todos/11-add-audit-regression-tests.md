# Todo: Add Audit Regression Tests

## Status

Partially implemented and rescoped. There is substantial decode coverage in
`tests/decode_records.rs`, ignored real-ESM coverage in
`tests/decode_coverage.rs` and `tests/diff.rs`, and source-chain coverage in
`tests/sources.rs`. Remaining work should focus on missing hardening and
determinism tests rather than duplicating existing decode fixtures.

## Context

The audit identified behavior that should stay fixed once remediated:
malformed binary inputs must not panic, exposed surfaces should validate inputs,
and decode/index behavior should remain deterministic.

## Remaining Scope

- Add Rust unit tests for malformed/truncated binary parsing in `src/reader.rs`
  and `src/format.rs`.
- Add BA2 table/data bounds tests in `src/ba2.rs`.
- Add string-table offset, size, and truncation tests in `src/strings.rs`.
- Add decompression tests for unreasonable declared output sizes in
  `src/compress.rs` once limits exist.
- Add `.esm.idx` cache rejection and atomic-save tests in `src/index.rs`.
- Add `.esm.midx` corrupt-sidecar tests if they are not fully covered while
  resolving `13-fix-last-36h-review-findings.md`.
- Add xref extraction tests for duplicate elimination and false positives from
  non-FormID hex strings.
- Add frontend or lightweight DOM tests for static escaping where practical.
- Keep the real game-data test convention documented and env-gated.

## Files

- `src/format.rs`
- `src/reader.rs`
- `src/ba2.rs`
- `src/strings.rs`
- `src/compress.rs`
- `src/index.rs`
- `src/mindex.rs`
- `src/decode.rs`
- `src/diff.rs`
- `static/index.html`
- `static/compare.html`
- `README.md`

## Acceptance Criteria

- Fresh checkout tests pass without game data.
- Real ESM/BA2 tests are `#[ignore]` and opt in via documented env vars.
- Tests cover truncated headers, malformed subrecords, oversized declarations,
  BA2/string-table bounds, cache corruption, cache atomicity, decode union
  selection, nested structs, xref extraction, xref duplicate handling, and
  static HTML escaping.
- Regression tests are focused and do not require committing proprietary game
  data.

## Verification

- `cargo test`
- `cargo test -- --ignored` with documented local env vars set.
- `cargo clippy --all-targets -- -D warnings`
- Optional frontend/static test command if a JS test harness is introduced.
