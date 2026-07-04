# CLAUDE.md — esm-viewer

Guidance for Claude Code when working in this Electron app.

"FO76 ESM Viewer" is a desktop GUI over the `esm` Rust crate's record browser: it lists,
searches, and displays decoded FO76 record data. It is **strictly read-only** — no write
path exists, matching the `esm/` core invariant (see [`../esm/CLAUDE.md`](../esm/CLAUDE.md)).
Do not add any feature that mutates an ESM file.

## Commands

```sh
npm install                # install deps; relinks the @fo76/esm-napi symlink dependency
npm run build:addon        # rebuild ../esm/bindings/napi (native addon this app consumes)
npm run dev                # electron-vite dev (runs build:addon first via "predev")
npm run build              # electron-vite build (runs build:addon first via "prebuild")
npm run typecheck          # tsc --noEmit against both tsconfig.json and tsconfig.node.json
just                        # = just check = npm run typecheck
just dev / just build       # thin wrappers over the npm scripts above
```

## Dependency on `esm/bindings/napi`

This app depends on `@fo76/esm-napi` via `"file:../esm/bindings/napi"` in `package.json` —
a symlinked local dependency, not a published package. It is a Rust workspace member of
`esm/Cargo.toml`, so it cannot move into this directory; only the Electron app relocated
(from `esm/app/` to repo-root `esm-viewer/`, via `git mv`, preserving history).

**After any Rust API change to `EsmDatabase` in `esm/bindings/napi/src/lib.rs`, rebuild the
addon** (`npm run build:addon`, or just let `predev`/`prebuild` do it automatically) and keep
`src/shared/api-types.ts` (the TypeScript mirror of the N-API types) in sync by hand.

If `node_modules/@fo76/esm-napi` ever fails to resolve (e.g. after moving either directory
again), `rm -rf node_modules package-lock.json && npm install` to force a clean relink, then
verify with `readlink node_modules/@fo76/esm-napi`.

## Type-checking

Nothing in the electron-vite/esbuild build pipeline checks types — it strips them. `npm run
typecheck` is the actual gate, run separately (and via `just check`). There are two tsconfigs
because main/preload and renderer target different environments:

- `tsconfig.json` — renderer (DOM lib, `composite: true`).
- `tsconfig.node.json` — main + preload (Node-oriented; extends `tsconfig.json`, overrides
  `lib`/`jsx`; also picks up `src/shared/**/*` since main/preload import shared types).

## Architecture

| Path | Purpose |
|---|---|
| `src/main/` | Electron main process: window creation (`index.ts`), addon loading (`addon.ts`), per-file `EsmDatabase` cache (`db-registry.ts`), IPC handlers (`ipc.ts`) |
| `src/preload/` | Context-isolated preload bridge exposed to the renderer |
| `src/renderer/` | React UI (record tree, detail panel, referenced-by panel, open-files panel, nav history), Zustand store |
| `src/shared/api-types.ts` | TypeScript mirror of the Rust N-API types — keep in sync with `esm/bindings/napi` |
