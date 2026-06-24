# Todo: Harden HTTP and Electron Surfaces

## Context

The audit found that the optional HTTP server and Electron main process expose powerful local-file parsing operations with broad defaults. This is convenient for local development, but it increases risk if the server is reachable from other hosts or if compromised renderer content can send unexpected IPC calls.

## Scope

- Restrict `src/bin/server.rs` to loopback by default instead of `0.0.0.0`.
- Replace permissive CORS with an explicit local/dev policy.
- Add an explicit opt-in flag for remote binding, such as `--host`.
- Validate Electron IPC inputs in `app/src/main/ipc.ts`.
- Validate external URL schemes in `app/src/main/index.ts` before calling `shell.openExternal`.
- Keep the existing CLI/server workflows usable for local development.

## Files

- `src/bin/server.rs`
- `app/src/main/index.ts`
- `app/src/main/ipc.ts`
- `app/src/shared/api-types.ts`
- `app/src/preload/index.ts`

## Acceptance Criteria

- HTTP server binds to `127.0.0.1` unless the user explicitly chooses another host.
- CORS is no longer permissive by default.
- IPC handlers reject invalid `id`, `path`, `sig`, `offset`, `limit`, `formid`, and `resolve` values before calling native code.
- `shell.openExternal` only opens `http:` and `https:` URLs.
- Error messages remain useful to the renderer without leaking unnecessary host details.

## Verification

- `cargo fmt --check`
- `cargo clippy --all-targets --features server -- -D warnings`
- `cargo test --features server`
- `npm run build` in `app/`
