# CLAUDE.md — FO76-Tools

This repository contains four Fallout 76 tools. Most share no code, no workspace configuration, and no build tooling — the one exception is `esm-viewer/`, which is a separate app but depends on `esm/bindings/napi` (its native addon) and is not fully independent of `esm/`. Per-project guidance lives in each subdirectory.

## Subprojects

| Directory | Language | Guidance |
|---|---|---|
| [`ba2/`](ba2/CLAUDE.md) | Rust / Cargo | BA2 archive CLI + library |
| [`esm/`](esm/CLAUDE.md) | Rust / Cargo | FO76 ESM reader: CLI, HTTP/MCP server |
| [`esm-viewer/`](esm-viewer/CLAUDE.md) | TypeScript / npm + Electron | FO76 ESM Viewer GUI; depends on `esm/bindings/napi` |
| [`dps-76/`](dps-76/CLAUDE.md) | TypeScript / pnpm | FO76 DPS calculator web app |

## Working in this repo

- **Always `cd` into the relevant subdirectory** before running commands — there are no root-level build scripts.
- `ba2/` uses `cargo`. See [`ba2/CLAUDE.md`](ba2/CLAUDE.md).
- `esm/` uses `cargo` (Rust workspace). See [`esm/CLAUDE.md`](esm/CLAUDE.md).
- `esm-viewer/` uses `npm` + `just`. See [`esm-viewer/CLAUDE.md`](esm-viewer/CLAUDE.md).
- `dps-76/` uses `pnpm`. See [`dps-76/CLAUDE.md`](dps-76/CLAUDE.md).

## Before committing

Before committing in any subproject, run that subproject's full check suite and only commit when everything passes — formatting, lint with `-D warnings`, and tests:

- **`esm/` and `ba2/`**: `just` (fmt + clippy + test). For `esm/`, also run `just audit` when you change the schema, the extractor, or anything affecting decode coverage.
- **`esm-viewer/`**: `just check` (= `npm run typecheck`).
- **`dps-76/`**: run its own checks in that repo (separate remote).

Fix formatting and clippy warnings rather than committing around them. Never commit with failing or skipped checks.

## Git boundary

`dps-76/` is a **nested git repository** with its own remote (`github.com/Mapekz/dps-76`). Files inside `dps-76/` are tracked by that repo, not by the root `FO76-Tools` repo. Do not stage or commit `dps-76/` files from the root repo.

`esm-viewer/` is, unlike `dps-76/`, tracked normally in this root repo (it was relocated from `esm/app/` via `git mv`, preserving history) — no special handling needed when staging or committing it.
