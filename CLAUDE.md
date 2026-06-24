# CLAUDE.md — FO76-Tools

This repository contains two **independent** Fallout 76 tools. They share no code, no workspace configuration, and no build tooling. Per-project guidance lives in each subdirectory.

## Subprojects

| Directory | Language | Guidance |
|---|---|---|
| [`ba2/`](ba2/CLAUDE.md) | Rust / Cargo | BA2 archive CLI + library |
| [`esm/`](esm/CLAUDE.md) | Rust / Cargo + Electron | FO76 ESM reader: CLI, HTTP/MCP server, Electron GUI |
| [`dps-76/`](dps-76/CLAUDE.md) | TypeScript / pnpm | FO76 DPS calculator web app |

## Working in this repo

- **Always `cd` into the relevant subdirectory** before running commands — there are no root-level build scripts.
- `ba2/` uses `cargo`. See [`ba2/CLAUDE.md`](ba2/CLAUDE.md).
- `esm/` uses `cargo` (Rust workspace) + `npm` (Electron app in `app/`). See [`esm/CLAUDE.md`](esm/CLAUDE.md).
- `dps-76/` uses `pnpm`. See [`dps-76/CLAUDE.md`](dps-76/CLAUDE.md).

## Git boundary

`dps-76/` is a **nested git repository** with its own remote (`github.com/Mapekz/dps-76`). Files inside `dps-76/` are tracked by that repo, not by the root `FO76-Tools` repo. Do not stage or commit `dps-76/` files from the root repo.
