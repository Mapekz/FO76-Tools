# CLAUDE.md — esm-viewer

Guidance for Claude Code when working in this Electron app.

"FO76 ESM Viewer" is a desktop GUI over the `esm` Rust crate's record browser: it lists,
searches, and displays decoded FO76 record data. It is **strictly read-only** — no write
path exists, matching the `esm/` core invariant (see [`../esm/CLAUDE.md`](../esm/CLAUDE.md)).
Do not add any feature that mutates an ESM file.

See [`README.md`](README.md) for the human-facing overview (what this app is, requirements,
build/run); this file covers agent-facing build/architecture detail instead.

## Commands

```sh
bun install                # install deps; relinks the @fo76/esm-napi symlink dependency
bun run build:addon        # rebuild ../esm/bindings/napi (native addon this app consumes)
bun run dev                # electron-vite dev (runs build:addon first via "predev")
bun run build              # electron-vite build (runs build:addon first via "prebuild")
bun run lint                # oxlint (see .oxlintrc.json)
bun run lint:fix            # oxlint --fix
bun run format              # oxfmt, writes in place (see .oxfmtrc.json)
bun run format:check        # oxfmt --check
bun run typecheck          # tsc --noEmit against both tsconfig.json and tsconfig.node.json
bun run test                # bun test (unit tests for renderer/src/lib/*)
just                        # = just check = lint:ci -> format:check -> typecheck -> test
just dev / just build       # thin wrappers over the bun scripts above
```

Package manager is Bun (`bun.lock`), not npm — see `package.json`'s `trustedDependencies` and
its own `postinstall`.

### Electron binary download gotcha

`electron`'s own `postinstall` (`node install.js`) has been observed to silently truncate the
zip extraction — it exits 0 and produces a `node_modules/electron/dist/` with only 1-2 files
(no `version` file) instead of the full ~20-file distribution — when the `node` resolved from
`PATH` is a sufficiently new major (reproduced with system Node v26.4.0; a plain `npm install`
on the same machine fails identically, so this is not Bun-specific). `package.json`'s own
`"postinstall": "bun node_modules/electron/install.js"` works around it by forcing the
extraction to run under Bun's own runtime instead of whatever `node` is first on `PATH`. If
`bun run dev`/`build` ever fails with an Electron binary error again, check
`node_modules/electron/dist/version` exists before assuming a dependency or lockfile change
broke something — rerun `bun run postinstall` (or `bun install`) first.

## Dependency on `esm/bindings/napi`

This app depends on `@fo76/esm-napi` via `"file:../esm/bindings/napi"` in `package.json` —
a symlinked local dependency, not a published package. The addon is a Rust workspace member
of `esm/Cargo.toml`, so it lives under `esm/` rather than in this directory, and this app
consumes it via that `file:` symlink dependency.

**After any Rust API change to `EsmDatabase` in `esm/bindings/napi/src/lib.rs`, rebuild the
addon** (`bun run build:addon`, or just let `predev`/`prebuild` do it automatically). Most DTO
shapes are generated, not hand-mirrored: run `just gen-types` in `esm/` (part of `esm/`'s
`just check`) to regenerate `src/shared/generated/*.ts` from the `ts-rs`-derived Rust structs.
`src/shared/api-types.ts` re-exports those under their existing names and hand-writes only the
IPC-contract pieces that aren't Rust types (`CH` channel names, `Fo76Api`, `FilterOp`) — update
`Fo76Api` by hand when adding/removing/reshaping an `EsmDatabase` method.

If `node_modules/@fo76/esm-napi` ever fails to resolve (e.g. after moving either directory
again), `rm -rf node_modules bun.lock && bun install` to force a clean relink. Bun links this
`file:` dependency as a real directory of per-file symlinks (not one directory symlink like
npm) — verify with `ls -la node_modules/@fo76/esm-napi/` and check the entries point back into
`../esm/bindings/napi/`, since `readlink node_modules/@fo76/esm-napi` itself will report nothing.

## Type-checking

Nothing in the electron-vite/esbuild build pipeline checks types — it strips them. `bun run
typecheck` is the actual gate, run separately (and via `just check`). There are two tsconfigs
because main/preload and renderer target different environments:

- `tsconfig.json` — renderer (DOM + ES2023 lib, `composite: true`). ES2023 (not ES2022) is
  deliberate — it's what makes `Array.prototype.toSorted()`/`toReversed()` etc. available;
  Electron's bundled V8 already supports them at runtime.
- `tsconfig.node.json` — main + preload (Node-oriented; extends `tsconfig.json`, overrides
  `lib`/`jsx`; also picks up `src/shared/**/*` since main/preload import shared types).

`typescript` is pinned to `^7.0.2` (the Go-ported compiler; same `tsc` binary, just native).

## Lint & format

`oxlint` (`.oxlintrc.json`) and `oxfmt` (`.oxfmtrc.json`) are both pinned to exact versions
(`--save-exact`) since oxfmt is still pre-1.0. Both exclude `src/shared/generated/` — it's
`ts-rs` output and CI drift-guards it (`git diff --exit-code` in the `rust` job), so a
formatter touching it would turn that job red.

`.oxlintrc.json` enables `correctness`/`suspicious`/`perf` plus the `react`/`import`/`vitest`
plugins; `style` stays off since `oxfmt` owns formatting. Type-aware linting
(`oxlint-tsgolint`) is intentionally not wired up — it's a separate devDependency
regardless of the `typescript` version installed, since TS 7.0 ships no programmatic
compiler API for it to call. `oxlint --type-check` is also intentionally not used as a
`tsc` replacement — this app's two tsconfigs (divergent `lib`/`jsx`) are the shape oxlint's
single-program discovery is least tested against.

`.oxfmtrc.json` sets `semi: false` / `singleQuote: true` to match the pre-existing house
style; everything else is oxfmt's defaults (`printWidth: 100`, `trailingComma: "all"`).
`sortPackageJson` (on by default) is explicitly disabled — it reorders `package.json` keys.
oxfmt does not respect `.gitignore` (oxlint does), so its `ignorePatterns` duplicates
`out/`/`dist/`/`node_modules/` explicitly, plus `*.md`/`*.yml`/`*.yaml`/`*.html` since this
app's docs/config files were never brought under formatter control.

## Architecture

| Path | Purpose |
|---|---|
| `src/main/` | Electron main process: window creation (`index.ts`), addon loading (`addon.ts`), per-file `EsmDatabase` cache (`db-registry.ts`), IPC handlers (`ipc.ts`) |
| `src/preload/` | Context-isolated preload bridge exposed to the renderer |
| `src/renderer/` | React UI (record tree, detail panel, referenced-by panel, open-files panel, nav history), Zustand store |
| `src/shared/api-types.ts` | Re-exports the `ts-rs`-generated Rust N-API DTOs (`./generated/`) plus hand-written IPC-contract types (`CH`, `Fo76Api`, `FilterOp`) |
| `src/shared/generated/` | Generated TypeScript mirrors (`ts-rs` + two hand-written generators) — regenerate via `just gen-types` in `esm/`; never hand-edit |
