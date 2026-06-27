# Todo: Harden HTTP and Electron Surfaces

## Status

Partially implemented. `src/bin/server.rs` now binds both legacy UI mode and
daemon mode to `127.0.0.1`, so the original `0.0.0.0` default is no longer the
remaining risk.

## Context

The optional HTTP server and Electron main process still expose powerful local
file parsing operations. Loopback binding narrows exposure, but permissive CORS,
legacy `/op` behavior, and unchecked IPC inputs should still be tightened.

## Remaining Scope

- Replace `CorsLayer::permissive()` in `src/bin/server.rs` with a same-origin or
  explicit local-development policy.
- Resolve the `#13` overlap in `src/bin/server.rs`: legacy UI mode currently
  registers `/op` with an empty auth token. Either do not register generic RPC
  routes in legacy UI mode, or require a generated token there as well.
- Validate Electron IPC inputs in `app/src/main/ipc.ts` before calling native
  code: database ids, paths, record signatures, offsets, limits, FormID/EditorID
  targets, resolve depth, and optional depth values.
- Tighten `app/src/shared/api-types.ts` and `app/src/preload/index.ts` where
  useful so callers use constrained resolve/depth values instead of loose
  strings and numbers.
- Validate external URL schemes in `app/src/main/index.ts` before calling
  `shell.openExternal`; only `http:` and `https:` should be opened.
- Keep existing CLI, daemon, legacy static UI, and Electron workflows usable for
  local development.

## Files

- `src/bin/server.rs`
- `app/src/main/index.ts`
- `app/src/main/ipc.ts`
- `app/src/shared/api-types.ts`
- `app/src/preload/index.ts`

## Acceptance Criteria

- Legacy UI mode does not expose unauthenticated generic `/op` RPC calls.
- CORS is no longer permissive by default.
- IPC handlers reject invalid `id`, `path`, `sig`, `offset`, `limit`, `formid`,
  `target`, `resolve`, and depth values before calling native code.
- `shell.openExternal` only opens `http:` and `https:` URLs.
- Error messages remain useful to the renderer without leaking unnecessary host
  details.

## Verification

- `cargo fmt --check`
- `cargo clippy --all-targets --features server -- -D warnings`
- `cargo test --features server`
- `npm run build` in `app/`
