# Invocation mode is decided by argv shape alone — no REPL, no `-p`

Status: accepted (2026-07-31)

`esm` was originally modelled on single-verb agent CLIs (`claude -p`, `codex`): a `-p`/`--print`
flag meant "run once and exit", its absence meant "open an interactive session". That model never
actually fit and had already stopped working before anyone noticed:

- `esm` is verb-dispatched (git/docker shape), not single-verb. `esm get 0x463F` has exactly one
  reading — the subcommand's presence already carries the "one-shot" bit. `-p` encoded the same
  bit a second time, on top of it.
- The dispatch code (`src/bin/cli/main.rs`) had already drifted so that a subcommand always ran once
  and exited regardless of `-p` — a fix for a real bug (a subcommand used to fall through into the
  REPL and write its `esm> ` prompt to **stdout** right after JSON output, breaking strict parsers;
  fixed by exiting after any subcommand and moving the prompt to stderr). After that fix, `-p`'s
  branch in `main()` produced an identical `Backend` to the default path — a silent no-op that
  nothing caught for nine days, while two docs (including the string every MCP client receives on
  `initialize`) kept teaching the dead contract.
- The REPL itself was never developed past its initial cold-daemon convenience. Its only behavioral
  difference from one-shot mode was rejecting per-call source overrides; there was no terser
  rendering, no cursor, no state, no line editing (`stdin.read_line` + `shlex`, no history, no
  completion). Its one plausible advantage — a warm index — the daemon already gives every process,
  one-shot or not. Meanwhile bare `esm` under piped/redirected stdin printed a banner, read EOF, and
  exited **0 with empty stdout** — indistinguishable from success to a scripted caller, the worst
  failure mode available.

## Decision

Mode is decided by argv shape alone, with no separate flag or heuristic:

- Every subcommand runs once and exits. Daemon-backed by default; `--local` forces a cold
  in-process open.
- There is no interactive session. Bare `esm` (no subcommand) is a usage error — clap's
  `arg_required_else_help`/`subcommand_required` print usage and exit non-zero. This replaces the
  silent exit-0 above with a signal a script can actually detect.
- `-p`/`--print` no longer exists as a flag at all — it is a hard parse error to pass it, not a
  silently-accepted no-op.

Human record exploration lives entirely in `esm walk` (terse per-record-type digests, see
`docs/adr/0001-walk-interactive-chase-pipeline-json.md`) and in `esm-viewer`, not in a CLI session.

## Considered options

- **Keep `-p` as a documented no-op / hidden alias**: rejected. A flag that does nothing is worse
  than no flag — it invites new call sites to keep writing it "for one-shot mode" on the strength
  of stale docs, recreating the exact confusion this ADR retires. A hard parse error is
  self-documenting the moment someone reaches for it.
- **TTY-gate bare `esm`** (drop into a REPL only on an interactive terminal, print help otherwise):
  rejected as a heuristic. Agent harnesses that allocate a PTY (tmux-driven sessions, `script`,
  some CI runners) would still land in the REPL even though they're not a human at a keyboard —
  exactly the failure mode `-p` was invented to route around, reintroduced one layer down.
- **An explicit `esm repl` subcommand**: rejected for now, not because the surface is undesirable
  in principle, but because the REPL as it exists doesn't earn a permanent command slot — it has no
  documented use, no issue, no ADR, and no capability the daemon-backed one-shot form lacks. If a
  real interactive surface is wanted later (record cursor, numbered result back-refs, tab
  completion), it should be designed as new work against a stated need, not preserved by default
  under a new name. That design would also need a story against `esm-viewer`, which already covers
  GUI-side interactive exploration.

## Note on terminology

ADR 0001 calls `walk` "the sole interactive surface." That phrase was written while a REPL still
existed and meant *human-readable output*, as opposed to `chase`'s machine-JSON contract — not
*stateful session*, which the REPL (however thin) technically was. With the REPL gone, the two
senses no longer collide: "interactive" in this codebase now means exactly one thing, `walk`'s
digest rendering (see `CONTEXT.md`'s glossary — digest, evidence slice, mechanism).
