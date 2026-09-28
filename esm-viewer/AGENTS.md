# ESM viewer

Scoped Electron guidance. Shared policy and validation mapping live in
[../AGENTS.md](../AGENTS.md).

"FO76 ESM Viewer" is a desktop GUI over the `esm` Rust crate's record browser: it lists,
searches, and displays decoded FO76 record data. It is **strictly read-only** — no write
path exists, matching the `esm/` core invariant (see [`../esm/AGENTS.md`](../esm/AGENTS.md)).
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
bun run typecheck          # tsc --noEmit against tsconfig.{web,node,test}.json
bun run test                # bun test (every src/**/*.test.ts)
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
a symlinked local dependency, not a published package. The addon is the `esm-napi` crate in
the repo-root Cargo workspace; it lives under `esm/` rather than in this directory, and this
app consumes it via that `file:` symlink dependency.

Follow [the root validation map](../AGENTS.md#validation-map) for addon builds,
DTO regeneration, and IPC synchronization. `predev` and `prebuild` rebuild the
addon automatically; the build pipeline does not type-check the app.

If `node_modules/@fo76/esm-napi` fails to resolve after moving either directory,
run `bun install` and inspect the links before changing dependencies or the lockfile. Bun links this
`file:` dependency as a real directory of per-file symlinks (not one directory symlink like
npm) — verify with `ls -la node_modules/@fo76/esm-napi/` and check the entries point back into
`../esm/bindings/napi/`, since `readlink node_modules/@fo76/esm-napi` itself will report nothing.

## Type-checking

Nothing in the electron-vite/esbuild build pipeline checks types — it strips them. `bun run
typecheck` is the actual gate, run separately (and via `just check`). The renderer and
main/preload target different environments, and tests run under Bun, so there are three
projects on `tsconfig.base.json`; `tsconfig.json` only references them, for editors:

- `tsconfig.web.json` — renderer app code plus `src/shared/` (DOM + ES2023 lib, no Node or Bun
  types, so renderer code cannot reach for `process` or `require`). ES2023 (not ES2022) is
  deliberate — it's what makes `Array.prototype.toSorted()`/`toReversed()` etc. available;
  Electron's bundled V8 already supports them at runtime.
- `tsconfig.node.json` — main + preload plus `src/shared/` (ES2022 lib, Node types, no DOM).
- `tsconfig.test.json` — everything under `src/`, tests included, with Bun's types
  (`src/test-support/bun-types.d.ts`).

Read `package.json` for the TypeScript version; the native TS7 compiler still uses `tsc`.

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
`tsc` replacement — this app's two projects (divergent `lib`/`jsx`) are the shape oxlint's
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
| `src/main/` | Electron main process: window creation and navigation lockdown (`index.ts`), the typed `EsmHost` (`addon.ts`), open-database ids (`db-registry.ts`), every IPC handler over injected dependencies (`handlers.ts`, wired to Electron by `ipc.ts`), trust-boundary checks (`ipc-validators.ts`) |
| `src/preload/` | Preload bridge exposed to the renderer. The renderer is sandboxed (`sandbox: true`, context isolation, no Node integration), so the preload can require only `electron`. Relative imports are bundled into it, but electron-vite externalizes `dependencies` packages, so the preload must not import one |
| `src/renderer/` | React UI (record tree, detail panel, referenced-by panel, open-files panel, nav history) and the Zustand store, which owns navigation (`navigate`, `goBack`, `goForward`); `RecordRef` is the shared clickable record row |
| `src/shared/api-types.ts` | The renderer ↔ main contract: re-exports the generated types (`Op`, `OpOutput`, DTOs) and defines `CH`, `Fo76Api` (`run(id, op)` typed by `OpOutput`), `DbHandle`, and `sel()` |
| `src/shared/generated/` | Generated TypeScript mirrors (`ts-rs` + two hand-written generators) — follow the root validation map; never hand-edit |
