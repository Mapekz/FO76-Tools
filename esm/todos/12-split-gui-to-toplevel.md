# 12 — Split GUI to top-level FO76-Tools folder

## Goal

Extract the Electron app (`app/`) from inside `esm/` into its own top-level folder under the FO76-Tools monorepo (e.g. `FO76-Tools/esm-viewer-app/` or `FO76-Tools/fo76-esm/`), so the product ("FO76 ESM Viewer") and the headless engine (`esm` lib + CLI + server) are visually and structurally distinct at the repo root.

## Precondition

**esm is already in the FO76-Tools monorepo** (merged via filter-repo in Jun 2026), so atomic commits that span the engine and the app remain possible after the folder split — they're still in the same git repo. This makes the split safe to do now.

## What stays in `esm/`

- `src/` — engine lib + `esm` CLI + `esm-server`
- `bindings/napi/` — `esm-napi` N-API addon
- `schema/` — embedded `fo76.json`
- `tools/` — schema extractor, patch-note scripts
- `static/` — embedded HTTP server UI
- `todos/`, `todos.md`
- `Cargo.toml`, `Cargo.lock`

## What moves to the new top-level folder

- `app/` → e.g. `fo76-esm/` (or keep the folder name `app/` inside the new top-level dir)

## Coupling to preserve

The app is coupled to the engine via two seams:

1. **N-API addon binary**: `app/` depends on `@fo76/esm-napi` (a local file dep pointing to `bindings/napi/`). After the split, the path dep in `app/package.json` changes from `file:../bindings/napi` to `file:../esm/bindings/napi` (or `file:../../esm/bindings/napi` depending on folder depth). Update `app/package.json` `dependencies["@fo76/esm-napi"]`.
2. **`app/src/shared/api-types.ts`**: TypeScript types mirror the Rust N-API surface. This file moves with the app; the import path is local so no change needed, but when updating the Rust N-API types, update this file too.

## Steps

1. Move `app/` to the new top-level location (e.g. `git mv esm/app fo76-esm-viewer` from FO76-Tools root, or `mkdir -p fo76-esm-viewer && git mv esm/app/* fo76-esm/`).
2. Update `app/package.json` `dependencies["@fo76/esm-napi"]` path.
3. Verify `npm install && npm run build` (or `electron-vite build`) in the new location.
4. Update the root `FO76-Tools/README.md` and `FO76-Tools/CLAUDE.md` to list the new folder.
5. Consider whether the new folder needs its own `.gitignore` (for `node_modules/`, `out/`, `dist/`).

## Why not a separate git repo?

The app and engine are v0.1.0 and actively co-evolving. A separate repo would require publishing/pinning the `esm-napi` addon on every Rust API change. The monorepo keeps this atomic. Revisit after the API stabilizes.
