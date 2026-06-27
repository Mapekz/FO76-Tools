# Todo: Clean Up API Boundaries, Docs, and Repo Hygiene

## Status

Partially implemented and rescoped. The README mostly reflects the current
feature set, and `bindings/napi/smoke.mjs` now uses `FO76_ESM` instead of a
hardcoded local path. Remaining work is targeted hygiene around public
internals, generated files, warning surfaces, and native binding polish.

## Context

The codebase has grown from a Rust parser into a CLI, HTTP server, N-API
binding, and Electron app. Some public internals, generated files, and
library-side warnings now make the project harder to maintain.

## Remaining Scope

- Reduce public mutable internals on `Database` where downstream callers can use
  stable methods instead.
- Replace library `eprintln!` calls for recoverable optional-feature failures
  with returned warnings, structured errors, or a logging facade.
- Handle poisoned mutexes and serialization failures in the N-API binding
  instead of using `lock().unwrap()` and `serde_json::to_value(...).unwrap()`.
- Fill in or generate `bindings/napi/index.d.ts`, which is tracked but empty.
- Remove tracked build metadata such as `app/tsconfig.tsbuildinfo`.
- Ignore generated TypeScript build info files and compact mmap sidecars
  (`*.tsbuildinfo`, `*.esm.midx`).
- Review generated binding loader expectations and reconcile
  `bindings/napi/package.json` platform targets with Electron packaging and
  docs.
- Keep README/docs updates targeted to any behavior changed by the cleanup.

## Files

- `src/lib.rs`
- `src/curves.rs`
- `src/strings.rs`
- `bindings/napi/src/lib.rs`
- `bindings/napi/index.d.ts`
- `bindings/napi/package.json`
- `README.md`
- `.gitignore`
- `app/tsconfig.tsbuildinfo`

## Acceptance Criteria

- Downstream users interact with `Database` through stable methods instead of
  reaching into internals where avoidable.
- Library code does not write directly to stderr for recoverable
  optional-feature failures.
- N-API calls return `napi::Error` on mutex poison or serialization failure.
- Generated local build metadata is not tracked.
- Ignore rules cover generated TypeScript build info and `.esm.midx` sidecars.
- Smoke scripts work on any machine with
  `FO76_ESM=/path/to/SeventySix.esm`.
- Native platform support notes match the actual N-API package targets and
  Electron packaging targets.

## Verification

- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `npm run build` in `app/`
- `FO76_ESM=/path/to/SeventySix.esm node bindings/napi/smoke.mjs` after
  building the native addon.
