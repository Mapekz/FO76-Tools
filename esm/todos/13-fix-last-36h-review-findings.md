# Todo: Fix Last-36-Hours Review Findings

## Context

A review of commits from `5e30163^..f63baff` found five follow-up issues introduced or exposed by the recent daemon, mmap-index, auto-detection, Electron, and documentation work. Prioritize security and integrity first, then UX/API completeness and style cleanup.

## Findings to Resolve

1. High: legacy `esm-server <ESM>` mode registers the new generic `/op` endpoint with an empty auth token and permissive CORS.
2. Medium: corrupt `.esm.midx` sidecars can trigger unchecked size arithmetic or out-of-bounds lookup panics instead of being rejected and rebuilt.
3. Medium UX: all-hex EditorIDs are ambiguous in the new auto-detect path, and the Electron app has no explicit EditorID escape hatch.
4. Low UX/API: `sources_of` exists in Rust/N-API but is not exposed through the Electron preload/shared API.
5. Low style: `README.md` has trailing whitespace in the new decode status paragraph.

## Scope

- Harden `src/bin/server.rs` so powerful RPC operations are never exposed unauthenticated in legacy UI mode.
- Harden `src/mindex.rs` so malformed `.esm.midx` files are rejected without panic and valid files still load zero-copy.
- Improve Electron navigation and IPC API shape around explicit FormID vs EditorID lookup.
- Expose the new terminal drop-source feature through Electron's typed API surface.
- Clean up the README whitespace issue found by `git diff --check`.

## Implementation Plan

### 1. Lock down `/op` in legacy server mode

- Decide the least surprising policy for legacy UI mode:
  - Prefer not registering `/op`, `/health`, or `/status` for legacy UI unless needed by the embedded pages.
  - If `/op` must remain registered, generate a token for legacy mode too and require it for `/op`; do not rely on `token.is_empty()` bypass.
- Replace `CorsLayer::permissive()` with a tighter policy. For loopback UI, allow only the server's own origin where practical, or omit CORS for same-origin routes.
- Keep daemon mode behavior intact: bind to `127.0.0.1`, write the discovery token, and require `Authorization: Bearer <token>`.
- Add server-feature tests or handler-level tests for:
  - daemon `/op` rejects missing or wrong token;
  - legacy mode cannot accept unauthenticated arbitrary `/op` calls;
  - same-origin legacy UI routes still load.

Relevant files:

- `src/bin/server.rs`
- `src/backend.rs`
- `tests/ipc.rs` or a new server-feature test module

### 2. Make `.esm.midx` validation overflow-safe

- In `MmapFormIndex::try_load`, compute expected file size with checked arithmetic:
  - `count.checked_mul(ENTRY_SIZE)`
  - `HEADER_SIZE.checked_add(entries_bytes)`
- Reject and rebuild when arithmetic overflows.
- Require the mapped file to be exactly the expected size, or document and test why trailing bytes are allowed.
- In `get_by_formid`, derive the entry offset with checked arithmetic or rely on the exact-size invariant after validation.
- Add tests for:
  - huge `count` value does not panic;
  - short file with valid magic/version is rejected;
  - trailing-garbage policy is enforced;
  - valid round-trip still works.

Relevant files:

- `src/mindex.rs`
- `src/index.rs` only if `.midx` write behavior changes

### 3. Fix all-hex EditorID UX in Electron

- Keep CLI auto-detection and explicit `--edid` behavior as documented.
- Add explicit Electron calls for record and reference lookup by EditorID where navigation originates from a known EditorID, or implement a renderer-level fallback:
  - try `recordById(target)` first;
  - if the target looked FormID-like but returns not found, retry `recordByEdid(target)`;
  - do the same for `referencedById` via a new explicit `referencedByEdid` API if needed.
- Preserve FormID-first behavior for actual FormID links so existing navigation stays fast and deterministic.
- Add renderer or preload tests if the app has a test harness; otherwise verify manually with an all-hex synthetic EditorID.

Relevant files:

- `bindings/napi/src/lib.rs`
- `app/src/main/ipc.ts`
- `app/src/preload/index.ts`
- `app/src/shared/api-types.ts`
- `app/src/renderer/src/App.tsx`
- `app/src/renderer/src/components/RecordTree.tsx`

### 4. Expose `sources_of` through Electron

- Add typed API support:
  - new channel in `app/src/shared/api-types.ts`;
  - `sourcesOf(id, target, maxDepth?)` on `Fo76Api`;
  - preload bridge in `app/src/preload/index.ts`;
  - IPC handler in `app/src/main/ipc.ts` calling the existing N-API `sourcesOf`.
- Add shared TypeScript types for `SourceList`, `Source`, `SourceKind`, and path nodes to mirror Rust output.
- Decide first UI integration point:
  - minimal: expose the API only for future components;
  - better: add a small panel or action near `ReferencedByPanel` showing terminal sources.
- Validate `maxDepth` in main-process IPC before calling native code.

Relevant files:

- `bindings/napi/src/lib.rs`
- `app/src/shared/api-types.ts`
- `app/src/preload/index.ts`
- `app/src/main/ipc.ts`
- optional renderer component under `app/src/renderer/src/components/`

### 5. Clean docs whitespace

- Remove the trailing spaces in the decode status paragraph of `README.md`.
- Run `git diff --check` after the fix.

Relevant file:

- `README.md`

## Acceptance Criteria

- No unauthenticated browser-origin request can call the generic `/op` RPC in legacy UI mode.
- Daemon clients still work with bearer-token discovery and `esm -p`.
- Corrupt or adversarial `.esm.midx` files are rejected and rebuilt without panics.
- Electron can still navigate by FormID, can handle all-hex EditorIDs through an explicit path or fallback, and exposes `sourcesOf` in the typed preload API.
- README has no trailing whitespace.
- Existing daemon, CLI, mmap-index, and Electron workflows remain compatible unless intentionally tightened for security.

## Verification

- `git diff --check`
- `cargo fmt --check`
- `cargo clippy --all-targets --features server -- -D warnings`
- `cargo test --features server`
- `cargo test mindex`
- `npm run build` in `app/`
- Manual smoke:
  - `esm -p get path/to/data 0x463F --pretty`
  - `esm --local --mmap-index get path/to/data 0x463F --pretty`
  - start legacy `esm-server path/to/data` and confirm `/op` is blocked or authenticated according to the chosen policy
  - Electron lookup for an all-hex EditorID and a normal FormID
