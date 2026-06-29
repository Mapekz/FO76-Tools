# Todo: Clean Up API Boundaries, Docs, and Repo Hygiene

## Status

Partially implemented and rescoped. The README mostly reflects the current feature
set, and `bindings/napi/smoke.mjs` now uses `FO76_ESM` instead of a hardcoded local
path. Remaining work is targeted hygiene around public internals, library-side
warnings, generated/ignored files, and doc-drift corrections.

The three N-API-polish items that were originally here (`index.d.ts`, mutex/serialize
unwraps, `package.json` targets) have been **moved to #05**, which is the single
owner of N-API hardening.

## Context

The codebase has grown from a Rust parser into a CLI, HTTP server, N-API binding,
and Electron app. Public `Database` internals, library-side stderr writes, tracked
generated files, and several doc-vs-code mismatches now make the project harder to
navigate and maintain.

## Remaining Scope

### API boundaries

- Reduce public mutable internals on `Database` where downstream callers can use
  stable methods instead. Currently all 7 fields are `pub` (`lib.rs:57-74`):
  `esm`, `index`, `schema`, `is_localized`, `localization`, `curves`,
  `mmap_index`. `diff.rs` reaches directly into `a.index.form_index` (line 100),
  `a.esm` (line 137), and `b.localization` (line 194). Expose stable accessors or
  methods that cover these call sites so direct field access can be phased out.

### Library stderr

- Replace library `eprintln!` calls for recoverable optional-feature failures with
  returned warnings, structured errors, or a logging facade (e.g. `tracing` or a
  thin `warn!` callback). Verified locations (10 total):
  - `src/lib.rs:140` — localization BA2 load failed (falls back to `None`)
  - `src/lib.rs:150-153` — localization loose-files load failed
  - `src/lib.rs:166` — curves loose-dir load failed
  - `src/lib.rs:175` — curves BA2 load failed
  - `src/curves.rs:97` — per-CURV parse failure (`continue`)
  - `src/curves.rs:158` — per-CURV parse failure (`continue`)
  - `src/ba2.rs:58` — unexpected BA2 version (continues)
  - `src/index.rs:403` — failed to write `.esm.midx` (non-fatal)
  - `src/diff.rs:353` — `sources_of` failed for one record (continues)
  - `src/registry.rs:108` — registry warning

### Repo hygiene

- Remove `app/tsconfig.tsbuildinfo` from git tracking (currently tracked —
  `git ls-files app/tsconfig.tsbuildinfo` returns it). Run `git rm --cached`.
- Add `.gitignore` rules for:
  - `*.tsbuildinfo` — not currently ignored; `app/tsconfig.tsbuildinfo` is tracked
  - `*.esm.midx` — not currently ignored despite `README.md:5` claiming it is
    (`.gitignore` has `*.esm` and `*.esm.idx` but not `*.esm.midx`)

### Doc-drift fixes (found during 2026-06-29 audit)

- `esm/CLAUDE.md` architecture table: `CACHE_VERSION` still says `8`; current value
  in `src/index.rs:18` is `9`. Update the table.
- `esm/CLAUDE.md` N-API section (line ~50): describes the binding methods as
  "`#[napi]` async methods" — but only `open_database` is async; all others are
  synchronous. Correct the description.
- `README.md:5` (gitignore claim): states `*.esm.midx` is gitignored — it is not.
  Fix the claim or add the ignore rule (the ignore rule is the primary fix; this
  doc update follows from it).

## Files

- `src/lib.rs`
- `src/curves.rs`
- `src/strings.rs`
- `src/diff.rs`
- `src/registry.rs`
- `src/ba2.rs`
- `src/index.rs`
- `README.md`
- `esm/CLAUDE.md`
- `.gitignore`
- `app/tsconfig.tsbuildinfo` (to untrack)

## Acceptance Criteria

- Downstream users interact with `Database` through stable methods instead of
  reaching into public fields where avoidable.
- Library code does not write directly to stderr for recoverable optional-feature
  failures.
- `app/tsconfig.tsbuildinfo` is not tracked in git.
- `.gitignore` covers `*.tsbuildinfo` and `*.esm.midx`.
- `CLAUDE.md` `CACHE_VERSION` and N-API async-method claims match the code.
- `README.md` gitignore claim matches `.gitignore`.

## Verification

- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `git ls-files '*.tsbuildinfo'` returns nothing after untracking.
- `grep '*.esm.midx' .gitignore` returns a match.
- `grep 'CACHE_VERSION' src/index.rs esm/CLAUDE.md` — values agree.
