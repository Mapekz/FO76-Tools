# Todo: Harden Electron IPC Surfaces

## Status

Still relevant. All items below are verified undone against current code.

The two server-side items that were originally in this todo (replace
`CorsLayer::permissive()`; lock down `/op` in legacy mode) are now owned by
**#13** and will be fixed there.

## Context

The Electron main process passes IPC inputs from the renderer directly to native
code without validation. Type constraints in the shared API surface are too loose,
making it easy for a buggy or malicious renderer payload to reach native code with
out-of-range values. One URL-open path also accepts arbitrary schemes.

## Remaining Scope

- Validate Electron IPC inputs in `app/src/main/ipc.ts` before calling native
  code: database ids, paths, record signatures (`sig`), offsets, limits,
  FormID/EditorID targets, and `resolve` values. Currently all values are passed
  through with no range or allowlist checks; the only guard is a registry-entry
  existence check.
- Tighten `app/src/shared/api-types.ts` and `app/src/preload/index.ts` so callers
  use constrained types instead of loose `string` and `number` — e.g. `resolve`
  should be a union literal type, not bare `string`; `offset`/`limit` should have
  documented range semantics.
- Validate external URL schemes in `app/src/main/index.ts` before calling
  `shell.openExternal`; currently `url` is passed with no check
  (`index.ts:18-21`). Only `http:` and `https:` should be opened.
- Keep existing CLI, daemon, legacy static UI, and Electron workflows usable for
  local development.

## Coordination notes

- `depth` IPC validation and types are **#14's** (recursive refs). Once #14 adds
  `depth` to `referencedById`, validate it here (clamp to `[1, DEFAULT_MAX_DEPTH]`).
- Compile-time types for the native binding are **#05's**. The loose
  `Record<string, (...args: unknown[]) => unknown>` casts in `ipc.ts` should be
  replaced once `bindings/napi/index.d.ts` is generated. Do runtime validation
  here; drop the casts there.

## Files

- `app/src/main/index.ts`
- `app/src/main/ipc.ts`
- `app/src/shared/api-types.ts`
- `app/src/preload/index.ts`

## Acceptance Criteria

- IPC handlers reject invalid `id`, `path`, `sig`, `offset`, `limit`, `formid`,
  `target`, and `resolve` values before calling native code.
- `shell.openExternal` only opens `http:` and `https:` URLs.
- `resolve` and similar constrained parameters use literal union types, not bare
  `string`.
- Error messages remain useful to the renderer without leaking unnecessary host
  details.

## Verification

- `npm run build` in `app/`
- `npm run typecheck` (or `tsc --noEmit`) in `app/` — constrained types should
  surface any callers that pass out-of-range literals.
