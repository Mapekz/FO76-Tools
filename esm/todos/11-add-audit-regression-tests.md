# Todo: Add Audit Regression Tests

## Context

The audit identified important behavior that should stay fixed once remediated: malformed binary inputs must not panic, exposed surfaces should validate inputs, and decode/index behavior should remain deterministic.

## Scope

- Add Rust unit tests for pure binary parsing and decode behavior.
- Add ignored integration tests for real ESM/BA2 workflows using environment variables.
- Add tests for cache rejection and atomic cache save behavior.
- Add tests for xref extraction and duplicate handling.
- Add frontend or lightweight DOM tests for static escaping where practical.
- Document the test-data convention for real game data.

## Files

- `src/format.rs`
- `src/reader.rs`
- `src/ba2.rs`
- `src/strings.rs`
- `src/compress.rs`
- `src/index.rs`
- `src/decode.rs`
- `src/diff.rs`
- `static/index.html`
- `static/compare.html`
- `README.md`

## Acceptance Criteria

- Fresh checkout tests pass without game data.
- Real ESM/BA2 tests are `#[ignore]` and opt in via documented env vars.
- Tests cover truncated headers, malformed subrecords, oversized declarations, cache corruption, decode union selection, nested structs, xref extraction, and static HTML escaping.
- Regression tests are focused and do not require committing proprietary game data.

## Verification

- `cargo test`
- `cargo test -- --ignored` with documented local env vars set.
- `cargo clippy --all-targets -- -D warnings`
- Optional frontend/static test command if a JS test harness is introduced.
