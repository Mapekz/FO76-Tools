# FO76-Tools

An umbrella for independent Fallout 76 tooling. Each subproject has its own language, toolchain, and build pipeline — there is no shared workspace. The one cross-project dependency is `esm-viewer/`, which consumes the native addon built from `esm/bindings/napi`.

| Project | Language | Description |
|---|---|---|
| [`ba2/`](ba2/README.md) | Rust | CLI and library for reading, extracting, and creating Bethesda BA2/BTDX GNRL archives (FO76 LZ4 / FO4 zlib) |
| [`esm/`](esm/README.md) | Rust | Read-only FO76 ESM engine: `esm` CLI, HTTP/MCP server, and the `esm-napi` N-API addon |
| [`esm-viewer/`](esm-viewer/) | TypeScript / Electron | "FO76 ESM Viewer" desktop GUI for browsing, searching, and diffing game records; built on `esm-napi` |

Deferred work for every subproject is tracked in [GitHub Issues](https://github.com/Mapekz/FO76-Tools/issues).

## Downloads

Prebuilt `esm`, `esm-server` and `ba2` binaries for **Linux x86-64 (glibc)** and **Windows 10/11 x86-64** are published to [Releases](https://github.com/Mapekz/FO76-Tools/releases):

| Channel | Where | Built from |
|---|---|---|
| Rolling | the [`latest`](https://github.com/Mapekz/FO76-Tools/releases/tag/latest) prerelease — same URL every time, replaced in place | every push to `main` that touches Rust sources |
| Pinned | any `v*` release | a tagged commit |

Unpack and put the three binaries somewhere on `PATH`. **Keep `esm-server` beside `esm`** — the CLI is daemon-backed by default and spawns the server from its own directory; without it, only `esm --local` works. Each archive carries a `BUILD_INFO.txt` naming the exact commit and toolchain, and every release lists `SHA256SUMS`.

`esm-viewer` is not distributed as an installer yet — build it from source.

## Agent skill bundle

This repository also contains Fallout 76 HUD-mod authoring skills and focused domain references.
They are plain Markdown and can be loaded by a native skill system or adapted into a system prompt
for Codex, Claude, Grok, or another harness. The `.claude/skills/` directory is the canonical source
layout, not a requirement of the consuming harness.

- [Harness portability](.claude/skills/agent-skill-portability/SKILL.md)
- [HUD research](.claude/skills/fo76-hud-research/SKILL.md)
- [HUD permissions](.claude/skills/fo76-hud-permissions/SKILL.md)
- [HUD Mod Loader](.claude/skills/fo76-hudmodloader/SKILL.md)
- [Vanilla UI](.claude/skills/fo76-vanilla-ui/SKILL.md)
- [Scaleform/GFx](.claude/skills/fo76-scaleform-ui/SKILL.md)
- [Haxe UI authoring](.claude/skills/fo76-haxe-ui/SKILL.md)
- [UI effects and animation](.claude/skills/fo76-ui-effects/SKILL.md)
- [xScal](.claude/skills/fo76-xscal/SKILL.md)
- [ZFE](.claude/skills/fo76-zfe/SKILL.md)

See the [harness compatibility guide](docs/harness-compatibility.md). Use
[`skills.manifest.yaml`](skills.manifest.yaml) for path mapping, dependency loading, and tool
adaptation rules. Game files, extracted assets, downloaded mod payloads, binaries, credentials, and
decompiler output stay outside the repository.

## License

MIT — see [LICENSE](LICENSE).
