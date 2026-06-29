# Todo: Add Audit Regression Tests

## Status

**Closed (2026-06-29) — items folded into the hardening todos they verify.**

The standalone test tracker is no longer needed. All remaining test items have been
distributed to the todo that owns the hardening work being tested:

| Test category | Now tracked in |
|---|---|
| Truncated/malformed binary parsing (`reader.rs`, `format.rs`) | **#08** |
| BA2 table / data bounds (`ba2.rs`) | **#08** |
| String-table offset, size, truncation (`strings.rs`) | **#08** |
| Decompression: unreasonable declared output sizes (`compress.rs`) | **#08** |
| `.esm.idx` cache rejection + atomic-save (`index.rs`) | **#08** |
| Records-by-type ordering determinism | **#09** |
| Xref false-positive elimination (non-FormID hex strings) | **#09** |
| Static HTML-escaping correctness (`index.html`, `compare.html`) | **#07** |

## Already done

- **xref dedup test** — `tests/refs.rs::referenced_by_deduplicates_within_record`
  (added in commit `93d2894`). ✓
- **`.esm.midx` corrupt-sidecar tests** — owned and being implemented by **#13**.
- **Existing decode coverage** — `tests/decode_records.rs` (49 tests, synthetic byte
  buffers per record type), `tests/decode_coverage.rs` (env-gated real-ESM audit).

## Convention to preserve

Tests that require real game data must skip silently when the relevant env var is
unset (see `tests/diff.rs`, `tests/decode_coverage.rs` for the pattern). Never
commit proprietary game data; always gate on a documented env var.
