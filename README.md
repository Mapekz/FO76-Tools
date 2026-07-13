# FO76-Tools

An umbrella for independent Fallout 76 tooling. Each subproject is self-contained with its own language, toolchain, and build pipeline — there is no shared workspace or cross-project dependency.

| Project | Language | Description |
|---|---|---|
| [`ba2/`](ba2/README.md) | Rust | CLI and library for reading, extracting, and creating Bethesda BA2/BTDX GNRL archives (FO76 LZ4 / FO4 zlib) |
| [`esm/`](esm/README.md) | Rust + Electron | Read-only FO76 ESM reader: CLI, HTTP/MCP server, and Electron GUI ("FO76 ESM Viewer") for inspecting game records |
