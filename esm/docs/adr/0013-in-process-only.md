# Every surface runs in-process; there is no daemon

Status: accepted (2026-09-27). Supersedes ADR 0008.

A resident `esm-server` daemon used to hold one warm `Database` per ESM, and the CLI, the MCP
server and the Python pipeline all reached it over local HTTP. It existed because opening a
database was expensive: on the 20260918 snapshot a warm in-process call took ~80 ms, against
~2 ms through the daemon. Measuring that 80 ms showed it was not the rkyv index (mapped in ~30 µs)
but three pieces of eager parsing on every open: all ~4,300 curve JSON files (~37 ms), the 14 MB
of string tables (~19 ms) and the embedded 2.6 MB schema (~6 ms).

The daemon's cost had outgrown that saving. It needed a second binary behind a `server` Cargo
feature (and a `just release` rule so the two were rebuilt together); a CLI could reach a daemon
from a different build or worktree; a per-ESM `Mutex` serialized even warm reads across
concurrent sessions; and its HTTP protocol was mirrored into Python by a generated constants
module with its own CI drift check.

## Decision

Every surface opens databases in its own process, through `host::Host`:

- The string tables and curves became cache sections (`lstrings`, `curves`) that are parsed once
  and then mapped, stamped with the files they came from so a changed source rebuilds them. The
  schema's record definitions parse on first use. A warm open now takes ~1.5 ms and a warm
  `esm get` ~5 ms end to end.
- `Database` queries take `&self`, so one open database serves concurrent callers without a lock.
- Scripts that make many calls use `esm batch`, one child process per script that answers one
  JSON `{"esm", "op"}` request per stdin line and keeps databases open until stdin closes. The
  Python gateway owns one; it can only ever talk to the binary it launched.
- The one thing the daemon uniquely provided — a cold build (xref takes about a minute) surviving
  the death of the process that needed it — is kept by `progress::delegate_builds`: the CLI runs
  each missing section as a detached `esm cache build` and waits for it. The existing build lease
  (ADR 0003) still makes concurrent callers share one build.
- Source-override flags configure the `Database` a single command opens, on every command that
  has them, including bulk `get` (ADR 0008's restriction existed only because of the daemon's
  shared cache).

Removed with the daemon: the `esm-server` binary and `server` feature (tokio, axum, tower-http,
ureq), `esm daemon`, the `--local`/`--addr`/`--port` flags and daemon env vars, the MCP stdio
server, the legacy HTTP viewer pages, the Python wire-constants mirror, and `Op::Shutdown`.

## Considered options

- **Keep the daemon; fix the wrong-build identity gap and make `server` a default feature.**
  Cheaper, but keeps every other cost above, and still leaves ~80 ms on every non-daemon open
  (napi, `esm batch`, tests) that the cache sections remove anyway.
- **Keep the daemon for bulk work only.** `esm batch` gives a script a warm database for its own
  lifetime without a shared process, discovery file or wire protocol.
- **Keep MCP as an in-process `esm mcp` subcommand.** Nothing in the workspace configures an MCP
  client for it; it can be re-added as a plain JSON-RPC loop over `Host` if one appears.
