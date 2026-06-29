# Todo: Finish and Harden the N-API Binding

> **Status: partially implemented, rescoped.**
>
> The binding compiles and runs, but several quality gaps remain: the TypeScript
> type file is empty, error handling panics on mutex poison and serialization
> failure, expensive methods block the Electron main process, only one native
> target is declared, and callers cast away all type safety. WASM was never
> implemented and is **closed as not planned** — the ~880 MB mmap-backed ESM
> exceeds practical browser linear-memory limits; the native Electron path is
> the right fit.

## Current implementation status

### Implemented

- Cargo workspace with members `.` and `bindings/napi`.
- `bindings/napi` is a `cdylib` crate using `napi`, `napi-derive`, and `napi-build`.
- `EsmDatabase` wraps a warm `Database` in `Mutex<Database>`.
- `open_database` is async via `tokio::task::spawn_blocking`.
- The binding exposes `fileInfo`, `listGroups`, `listTypeRecords`, `recordByFormid`,
  `recordByEdid`, `referencedBy`, `referencedById`, `sourcesOf`, and `parseFormId`.
  Note: only `open_database` is async; all other methods are synchronous.
- The Electron app depends on `@fo76/esm-napi` and loads it through
  `app/src/main/addon.ts`. Electron IPC calls into the N-API object in
  `app/src/main/ipc.ts`.
- `bindings/napi/smoke.mjs` is environment-gated with `FO76_ESM`.

### N-API Gaps (verified against current code)

- `bindings/napi/index.d.ts` is **empty** even though `package.json` advertises it
  in `"types"`. All IPC callers in `app/src/main/ipc.ts` work around this with loose
  `Record<string, (...args: unknown[]) => unknown>` casts on every native-method call.
- `bindings/napi/src/lib.rs` uses `Mutex::lock().unwrap()` at every method
  (lines 29, 38, 51, 71, 81, 96, 111, 123, 142) and
  `serde_json::to_value(...).unwrap()` at every return (lines 33, 40, 55, 75, 85,
  102, 115, 128, 151). No poison-aware error mapping exists.
- `recordByEdid` and `referencedBy` (and `referencedById`, `sourcesOf`) are
  **synchronous** and can block Electron's main process during first-time index
  builds (EditorID index, xref index).
- `bindings/napi/package.json` declares only `x86_64-unknown-linux-gnu` under
  `"targets"`. Electron packaging targets Linux, macOS, and Windows.
- The `smoke.mjs` test is environment-gated but there is no first-time setup guide
  in README; CLAUDE.md frames the addon build as "after any Rust API change" rather
  than a required first-run step.

## Coordination notes

- **#14 (recursive refs)** changes `referenced_by`'s signature to accept
  `depth: usize`. Async-ify `referencedBy`/`referencedById` as part of that
  implementation, or here — do it once consistently, and keep the `.d.ts` in sync.
- **#06 (Electron IPC)** adds runtime input validation; this todo provides the
  compile-time types. Do #05 first so #06 can drop the loose casts.

## Remaining Work

1. Generate or hand-author `bindings/napi/index.d.ts` so the advertised package
   types match the actual JS API (`napi build` emits this; commit the generated
   file).
2. Replace `Mutex::lock().unwrap()` with poison-aware error mapping to
   `napi::Error` (e.g. `self.inner.lock().map_err(|e| napi::Error::from_reason(e.to_string()))`).
3. Replace `serde_json::to_value(...).unwrap()` with `.map_err(|e| napi::Error::from_reason(e.to_string()))`.
4. Move expensive calls (`recordByEdid`, `referencedBy`, `referencedById`,
   `sourcesOf`) onto async tasks via `tokio::task::spawn_blocking` or napi's
   `AsyncTask` pattern — same as `open_database`. These can build large indexes
   on first call and must not block the Electron main process.
5. Decide supported native targets and update `bindings/napi/package.json` `"targets"`,
   build scripts, release docs, and Electron packaging to match.
6. Replace loose `Record<string, (...args: unknown[]) => unknown>` casts in
   `app/src/main/ipc.ts` with the generated package types once `index.d.ts` is valid.
7. Document how to build the native addon before running the Electron app for the
   first time (README onboarding, not just a CLAUDE.md rebuild reminder).

## Acceptance Criteria

- `@fo76/esm-napi` has accurate, non-empty TypeScript declarations.
- Native binding methods return `napi::Error` instead of panicking on poisoned
  locks or serialization failures.
- The Electron main process does not block on first-time EditorID or xref index
  builds.
- `app/src/main/ipc.ts` uses typed native-method calls instead of loose `unknown` casts.
- The smoke test runs only when `FO76_ESM=/path/to/Game.esm` is set.
- Supported platforms are explicit in `bindings/napi/package.json` and match
  Electron packaging targets.

## Verification

- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `npm run build` in `bindings/napi` (or `napi build --platform --release`)
- Inspect generated `index.d.ts` for `open_database` Promise type, `EsmDatabase`
  method signatures, and async variants.
- `FO76_ESM=/path/to/Game.esm node bindings/napi/smoke.mjs`
- `npm run build` in `app/`
