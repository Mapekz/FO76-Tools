# FO76 ESM Viewer

A desktop GUI for browsing, searching, diffing, and cross-referencing decoded Fallout 76 ESM
(game data) records. It's built on the `esm` Rust crate's native addon
([`../esm/bindings/napi`](../esm/bindings/napi)), which does all the parsing and schema
decoding; this app is the presentation layer over it — record tree/table navigation, full
record detail, search, filtering, referenced-by lookups, snapshot diffing, and schema
decode-coverage reporting. It is strictly read-only: no write/save path exists or is planned.

Positioning: a faster, cross-platform alternative to TES5Edit/xEdit for FO76 datamining —
Electron instead of Pascal/Wine, and a warm background daemon (from the `esm` engine) instead
of reloading the ESM per query.

## Requirements

- [Bun](https://bun.sh) (package manager and test runner)
- A Rust toolchain (to build the native addon this app depends on — see
  [`../esm/CLAUDE.md`](../esm/CLAUDE.md) for the pinned version)
- [`just`](https://github.com/casey/just) (optional; thin wrapper over the `bun run` scripts
  below)

## Build & run

```sh
bun install     # install deps; links the @fo76/esm-napi native addon dependency
just dev        # dev mode (electron-vite dev); rebuilds the addon first
just build      # production build (electron-vite build); rebuilds the addon first
just            # full check: lint, format check, typecheck, test
```

Without `just`, the equivalent `bun run` scripts are `dev`, `build`, `lint:ci`,
`format:check`, `typecheck`, and `test` — see `package.json`.

## Documentation map

- [`PRODUCT.md`](PRODUCT.md) — product spec: users, purpose, positioning, constraints.
- [`DESIGN.md`](DESIGN.md) — design system ("The Hex Workbench"): colors, typography, layout,
  components.
- [`CLAUDE.md`](CLAUDE.md) — agent-facing build/architecture detail: native-addon rebuild
  workflow, generated-type regeneration, lint/format/typecheck tooling, known gotchas.
