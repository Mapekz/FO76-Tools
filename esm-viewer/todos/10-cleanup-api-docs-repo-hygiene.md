# Todo: Clean Up API Boundaries, Docs, and Repo Hygiene

## Context

The codebase has grown from a Rust parser into a CLI, HTTP server, N-API binding, and Electron app. Some public internals, stale docs, generated files, and library-side warnings now make the project harder to maintain.

## Scope

- Reduce public mutable internals on `Database`.
- Replace library `eprintln!` calls with returned warnings, structured errors, or a logging facade.
- Handle poisoned mutexes in the N-API binding instead of using `lock().unwrap()`.
- Update stale `README.md` sections for string tables, curves, schema coverage, app/server surfaces, and bindings.
- Remove tracked build metadata such as `app/tsconfig.tsbuildinfo`.
- Ignore generated TypeScript build info files.
- Make `bindings/napi/smoke.mjs` use an environment variable instead of a hardcoded local path.
- Review generated binding loader expectations and platform support notes.

## Files

- `src/lib.rs`
- `src/curves.rs`
- `src/strings.rs`
- `bindings/napi/src/lib.rs`
- `bindings/napi/smoke.mjs`
- `README.md`
- `.gitignore`
- `app/tsconfig.tsbuildinfo`

## Acceptance Criteria

- Downstream users interact with `Database` through stable methods instead of reaching into internals where avoidable.
- Library code does not write directly to stderr for recoverable optional-feature failures.
- N-API calls return `napi::Error` on mutex poison or serialization failure.
- README reflects the current feature set and project layout.
- Generated local build metadata is not tracked.
- Smoke scripts work on any machine with `ESM=/path/to/SeventySix.esm`.

## Verification

- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `npm run build` in `app/`
- `node bindings/napi/smoke.mjs` with `ESM` set after building the native addon.
