# FO76-Tools

An umbrella for independent Fallout 76 tooling. Each subproject is self-contained with its own language, toolchain, and build pipeline — there is no shared workspace or cross-project dependency.

| Project | Language | Description |
|---|---|---|
| [`ba2/`](ba2/README.md) | Rust | CLI and library for reading, extracting, and creating Bethesda BA2/BTDX GNRL archives (FO76 LZ4 / FO4 zlib) |
| [`dps-76/`](dps-76/README.md) | TypeScript / React | Fallout 76 outgoing-DPS calculator web app with Live and PTS game data |

> **Note:** `dps-76/` is a separate, nested git repository (remote `Mapekz/dps-76`) with its own commit history. It lives here for convenience but is tracked independently.
