# FO76-Tools

An umbrella for independent Fallout 76 tooling. The Rust crates (`esm`, its `esm-napi` addon, and `ba2`) share one Cargo workspace at the repo root, building into `target/`; the TypeScript and Python projects have their own toolchains. `esm-viewer/` consumes the native addon built from `esm/bindings/napi`, and `patch-notes/` drives the `esm` CLI.

| Project | Language | Description |
|---|---|---|
| [`ba2/`](ba2/README.md) | Rust | CLI and library for reading, extracting, and creating Bethesda BA2/BTDX archives, GNRL and DX10 (FO76 LZ4 / FO4 zlib) |
| [`esm/`](esm/README.md) | Rust | Read-only FO76 ESM engine: the `esm` CLI and the `esm-napi` N-API addon |
| [`esm-viewer/`](esm-viewer/) | TypeScript / Electron | "FO76 ESM Viewer" desktop GUI for browsing, searching, and diffing game records; built on `esm-napi` |
| [`patch-notes/`](patch-notes/AGENTS.md) | Python | Patch-notes pipeline turning two ESM snapshots into a Discord post, and the agent skill that writes it |

Deferred work for every subproject is tracked in [GitHub Issues](https://github.com/Mapekz/FO76-Tools/issues).

## Downloads

Prebuilt `esm` and `ba2` binaries for **Linux x86-64 (glibc)** and **Windows 10/11 x86-64** are published to [Releases](https://github.com/Mapekz/FO76-Tools/releases):

| Channel | Where | Built from |
|---|---|---|
| Rolling | the [`latest`](https://github.com/Mapekz/FO76-Tools/releases/tag/latest) prerelease — same URL every time, replaced in place | every push to `main` that touches Rust sources |
| Pinned | any `v*` release | a tagged commit |

Unpack and put both binaries somewhere on `PATH`. Each archive carries a `BUILD_INFO.txt` naming the exact commit and toolchain, and every release lists `SHA256SUMS`.

`esm-viewer` is not distributed as an installer yet — build it from source.

## License

MIT — see [LICENSE](LICENSE).
