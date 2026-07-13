# FO76-Tools

An umbrella for independent Fallout 76 tooling. Each subproject has its own language, toolchain, and build pipeline — there is no shared workspace. The one cross-project dependency is `esm-viewer/`, which consumes the native addon built from `esm/bindings/napi`.

| Project | Language | Description |
|---|---|---|
| [`ba2/`](ba2/README.md) | Rust | CLI and library for reading, extracting, and creating Bethesda BA2/BTDX GNRL archives (FO76 LZ4 / FO4 zlib) |
| [`esm/`](esm/README.md) | Rust | Read-only FO76 ESM engine: `esm` CLI, HTTP/MCP server, and the `esm-napi` N-API addon |
| [`esm-viewer/`](esm-viewer/) | TypeScript / Electron | "FO76 ESM Viewer" desktop GUI for browsing, searching, and diffing game records; built on `esm-napi` |

Deferred work for every subproject is tracked in a single repo-root backlog: [`todos.md`](todos.md).
