# 12 — Split GUI to top-level FO76-Tools folder

## Status

Still relevant; still not implemented. The Electron app remains at `esm/app/`
and `app/package.json` still has `"@fo76/esm-napi": "file:../bindings/napi"`.

## Goal

Extract the Electron app (`app/`) from inside `esm/` into its own top-level folder
under the FO76-Tools monorepo (e.g. `FO76-Tools/fo76-esm-viewer/`), so the product
("FO76 ESM Viewer") and the headless engine (`esm` lib + CLI + server) are visually
and structurally distinct at the repo root.

## Precondition

`esm` is already in the FO76-Tools monorepo (merged via filter-repo in Jun 2026),
so atomic commits that span the engine and the app remain possible after the folder
split — they're still in the same git repo. This makes the split safe to do now.

## What stays in `esm/`

- `src/` — engine lib + `esm` CLI + `esm-server`
- `bindings/napi/` — `esm-napi` N-API addon
- `schema/` — embedded `fo76.json`
- `tools/` — schema extractor, patch-note scripts
- `static/` — embedded HTTP server UI
- `todos/`, `todos.md`
- `Cargo.toml`, `Cargo.lock`

## What moves to the new top-level folder

- `app/` → e.g. `fo76-esm-viewer/` (or keep the folder name `app/` inside the new
  top-level dir)

## Coupling to preserve (verified against current tree)

The app is coupled to the engine via two seams:

1. **N-API addon binary**: `app/package.json` declares
   `"@fo76/esm-napi": "file:../bindings/napi"`. After the split the path changes
   to `"file:../esm/bindings/napi"` (if the new top-level folder is a sibling of
   `esm/`). Update `app/package.json` and regenerate the lockfile.
2. **`app/src/shared/api-types.ts`**: TypeScript types mirror the Rust N-API surface.
   This file moves with the app; import paths are local so no change needed, but
   keep it in sync whenever the Rust N-API types change (see **#05**).

## Dependencies

- **#10** adds `.gitignore` rules for `*.tsbuildinfo` and `*.esm.midx`. The new
  top-level app folder needs those same rules to apply — verify after the move
  (glob patterns in the root `.gitignore` should cover it).
- **#05** may update the N-API method signatures; the generated `bindings/napi/index.d.ts`
  and `app/src/shared/api-types.ts` should stay in sync through the split.

## Steps

1. Move `app/` to the new top-level location (e.g. `git mv esm/app fo76-esm-viewer`
   from the FO76-Tools root).
2. Update `app/package.json` `dependencies["@fo76/esm-napi"]` path from
   `file:../bindings/napi` to `file:../esm/bindings/napi`.
3. Run `npm install` in the new location to regenerate the lockfile with the
   updated local dep path.
4. Verify `npm run build` (or `electron-vite build`) in the new location.
5. Update `FO76-Tools/README.md` and `FO76-Tools/CLAUDE.md` to list the new folder.
6. Update `esm/README.md` and `esm/CLAUDE.md` to describe the headless engine,
   server, static UI, and N-API binding — remove any implication the Electron app
   lives under `esm/app`.
7. Confirm `node_modules/`, `out/`, `dist/`, and `*.tsbuildinfo` are covered by
   ignore rules in the new location.

## Why not a separate git repo?

The app and engine are v0.1.0 and actively co-evolving. A separate repo would
require publishing/pinning the `esm-napi` addon on every Rust API change. The
monorepo keeps this atomic. Revisit after the API stabilizes.
