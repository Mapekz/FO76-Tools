# CLI arguments are declared beside the op registry, not derived from it

Status: accepted (2026-09-28).

One `ops!` registry (`src/ops/mod.rs`) declares every operation: its wire tag, argument type,
output type and function. From it come the `Op` wire enum, the dispatcher that `Host::run`,
`esm batch` and the N-API addon share, and the TypeScript `Op`/`OpOutput` contract. The CLI could
have been derived from it too.

## Decision

The CLI's `clap` `Commands` enum stays hand-declared. Each subcommand's handler builds its op from
the parsed arguments and renders the result.

- Most of what a subcommand takes is presentation that no op has: a positional target read as a
  FormID or an EditorID, `--decimal`, `--json`/`--pretty`, text renderers and their limits,
  source-override flags, and `refs --to`'s path search. Deriving `clap` from op arguments would
  put that presentation into the ops, or need a per-op annotation layer as large as the
  declarations it replaced.
- The contract that must not drift is the wire one, and the registry derives it: the declared
  output type is checked against each function at compile time, and the TypeScript types are
  regenerated and drift-checked.
- The CLI docs are checked against the built binary (`tests/doc_drift.rs`), so a subcommand or
  flag can't drift from its documentation.

## Consequences

- Adding an operation with CLI reach is an `Args` type and function, one `ops!` line, a
  `Commands` variant and a handler (see `docs/architecture.md`'s change table).
- Parsing enum arguments reuses the serde derives the ops already have; there is no separate
  string-to-enum module.
