# FO76-Tools

Four projects in one repository. The Rust crates share a root Cargo workspace (one
`Cargo.lock`, one `target/`), and the root `justfile` runs each project's recipes.
Before working in a project, read its scoped guidance:

| Directory | Purpose | Guidance |
|---|---|---|
| `ba2/` | Rust archive reader/writer and CLI | [ba2/AGENTS.md](ba2/AGENTS.md) |
| `esm/` | Rust ESM reader, CLI, native addon | [esm/AGENTS.md](esm/AGENTS.md) |
| `esm-viewer/` | Bun/Electron GUI consuming `esm/bindings/napi` | [esm-viewer/AGENTS.md](esm-viewer/AGENTS.md) |
| `patch-notes/` | Python patch-notes pipeline and the `/patch-notes` skill | [patch-notes/AGENTS.md](patch-notes/AGENTS.md) |

`esm/` and `esm-viewer/` are read-only. ESM mutation and serialization are
permanently out of scope, not deferred. BA2 archive writing is supported.

## Contexts

Each project has its own vocabulary; `esm/CONTEXT.md` and `patch-notes/CONTEXT.md` are
the glossaries, and [domain docs](docs/agents/domain.md) says how to use them.

- **esm → patch-notes**: the pipeline runs the `esm` CLI (`esm diff`, `esm batch`) and
  reads its JSON, so it shares esm's diff and record vocabulary.
- **esm → esm-viewer**: the viewer runs esm's N-API addon (`esm/bindings/napi`) and shares
  its record and decode vocabulary.
- **ba2 ↔ esm**: none. esm reads strings and curve tables through its own minimal read-only
  BA2 reader (`esm/src/ba2.rs`), not the `ba2` crate;
  see [the duplication decision](esm/docs/adr/0009-ba2-duplication-is-deliberate.md).

## Validation map

Each subproject's `justfile` owns its commands; the root `justfile` loads them as
modules (`just esm::check`, `just viewer::check`, …) and `just` at the root runs
every project's check, the same recipes CI runs. Select checks for the changed code
rather than rerunning every subproject for unrelated edits.

- ESM schema, extractor, or decode-coverage changes also need `just audit`
  (`just esm::audit`: the parity gate plus the schema and hardcoded-forms drift
  guards). It reads `FO76-Tools/TES5Edit`, a symlink to the sibling workspace checkout.
- ESM schema-tooling (`esm/tools/`) changes need `just esm::tools-check`.
- ESM's `just check` covers `bindings/napi` too; rebuild the addon with
  `bun run build` from `esm/bindings/napi` after changing it.
- DTO or `Op` changes need `just gen-types` from `esm/`; it regenerates
  `esm-viewer/src/shared/generated/` (including `Op` and `OpOutput`), which is
  drift-checked by `just check`. The viewer runs every op through one typed
  `api.run(id, op)`, so a new op needs no IPC code — only a place in
  `esm-viewer/src/main/ipc-validators.ts`'s `RUNNABLE_OPS`, which the compiler
  enforces. Run the viewer checks for affected cross-project changes.
- Game-data integration tests skip when their environment variables are unset.
  Report that coverage gap; a passing synthetic suite does not validate real data.

## Dependencies

The workspace's one `Cargo.lock` is checked against root `deny.toml` by the `deps` CI
job. For dependency changes, run `cargo deny --all-features check` from the repo root.
This differs from the TES5Edit schema audit.

The repo-root `rust-toolchain.toml` pins the toolchain and `rustfmt.toml` sets formatting for
every crate; clippy takes its MSRV from each `Cargo.toml` `rust-version`. Keep `rust-version`
aligned with the pinned toolchain: it intentionally tracks the toolchain so MSRV-aware dependency
resolution does not hold upgrades back.

## Repository conventions

- Conventional Commits: [docs/agents/commits.md](docs/agents/commits.md).
- Shared backlog: GitHub Issues on `Mapekz/FO76-Tools` via `gh`;
  [issue workflow](docs/agents/issue-tracker.md) and
  [triage labels](docs/agents/triage-labels.md).
- Domain terms and decisions: "Contexts" above, then the relevant project's glossary
  and ADRs.
- Documentation policy: `just policy` (`repo-policy/`) fails on a markdown link or a
  cited repo path (in markdown code or a code comment) that does not resolve, and on any
  file naming a downstream consumer project. Each CLI's own docs are checked against
  its `--help` by that crate's `tests/doc_drift.rs`.
- Record deliberate scope exclusions beside the code they constrain, in present tense.
