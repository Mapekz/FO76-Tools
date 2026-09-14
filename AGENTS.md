# FO76-Tools

Independent builds in one repository. Run commands from the relevant subdirectory;
there is no root build. Before working in a subproject, read its scoped guidance:

| Directory | Purpose | Guidance |
|---|---|---|
| `ba2/` | Rust archive reader/writer and CLI | [ba2/AGENTS.md](ba2/AGENTS.md) |
| `esm/` | Rust ESM reader, CLI, HTTP/MCP server, native addon | [esm/AGENTS.md](esm/AGENTS.md) |
| `esm-viewer/` | Bun/Electron GUI consuming `esm/bindings/napi` | [esm-viewer/AGENTS.md](esm-viewer/AGENTS.md) |

`esm/` and `esm-viewer/` are read-only. ESM mutation and serialization are
permanently out of scope, not deferred. BA2 archive writing is supported.
The standalone BA2 crate and ESM's minimal archive reader deliberately share no
code; see [the duplication decision](esm/docs/adr/0009-ba2-duplication-is-deliberate.md).

## Validation map

Use each subproject's `just check` for its full local gate; its `justfile` owns
the commands. Select checks for the changed code rather than rerunning every
subproject for unrelated edits.

- ESM schema, extractor, or decode-coverage changes also need `just audit`.
  It reads `FO76-Tools/TES5Edit`, a symlink to the sibling workspace checkout.
- ESM Python tooling changes need `just patch-tools-test` and `just patch-tools-lint`.
- ESM's default Cargo checks omit `bindings/napi`. After editing it, run
  `cargo build -p esm-napi` from `esm/`; rebuild the addon with
  `bun run build` from `esm/bindings/napi` when its API changes.
- DTO changes need `just gen-types` from `esm/`; it regenerates
  `esm-viewer/src/shared/generated/`, which is drift-checked by `just check`.
  `esm-viewer/src/shared/api-types.ts` re-exports those types; update its
  hand-written IPC contract (`CH`, `Fo76Api`, `FilterOp`) when methods change.
  Rebuild the addon and run the viewer checks for affected cross-project changes.
- Game-data integration tests skip when their environment variables are unset.
  Report that coverage gap; a passing synthetic suite does not validate real data.

## Dependencies

Both Rust crates share root `deny.toml`, enforced by the dependency CI job.
For dependency changes, run `cargo deny --config ../deny.toml --all-features check`
from the affected crate. This differs from the TES5Edit schema audit.

Keep each crate's `Cargo.toml` `rust-version`, `clippy.toml` `msrv`, and
`rust-toolchain.toml` aligned. The declared Rust version intentionally tracks the
toolchain so MSRV-aware dependency resolution does not hold upgrades back.

## Repository conventions

- Conventional Commits: [docs/agents/commits.md](docs/agents/commits.md).
- Shared backlog: GitHub Issues on `Mapekz/FO76-Tools` via `gh`;
  [issue workflow](docs/agents/issue-tracker.md) and
  [triage labels](docs/agents/triage-labels.md).
- Domain terms and decisions: start with [CONTEXT-MAP.md](CONTEXT-MAP.md), then
  the relevant subproject's glossary and ADRs. See [domain docs](docs/agents/domain.md).
- Record deliberate scope exclusions beside the code they constrain, in present tense.
