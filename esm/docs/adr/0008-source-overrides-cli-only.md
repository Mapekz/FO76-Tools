# Source-override flags are deliberately CLI-only, never given a daemon wire representation

Status: accepted (2026-08-13)

`list`, `get`/`refs`, `search`, and `diff` each accept source-override flags —
`--localization-ba2`/`--strings-dir`/`--startup-ba2`/`--curves-dir` (`diff` doubles these into
`_a`/`_b` per-side variants, since the two files being compared can legitimately need different
sources) — that point at a Localization BA2, a loose `strings/` folder, a Startup BA2, or a loose
curve-table folder instead of whatever the daemon would auto-load from beside the ESM. All four
commands implement the identical pattern independently: if any override flag is present and the
call would otherwise go through the daemon, hard-error and tell the caller to add `--local`;
otherwise force an in-process `Database::open` plus the override, bypassing `Backend::run`/`Op`
entirely.

This looked, on the surface, like a gap — four commands reaching for the same daemon-bypassing
workaround usually means the daemon's wire protocol (`Op` in `src/ipc.rs`) is missing a variant it
should have. It is not.

## Decision

Source-override flags stay CLI-only. They will not get an `Op::*` variant, and `RemoteBackend`
will not grow a way to carry them to the daemon over HTTP.

The reason is `src/registry.rs`'s `Registry`: it caches exactly one warm `Database` per canonical
ESM path, shared across every client that asks for that path. That cache is the entire reason the
daemon is faster than `--local` — the mmap, the schema, and (once built) the five `rkyv` index
sections are paid for once and reused by every subsequent request. A source override is
per-request, not per-path: two callers hitting the same daemon for the same ESM could legitimately
pass different `--localization-ba2` values (or none at all), and the `Database` that answers a
call with an override applied is no longer the one every other client is sharing. There is no
cache key that makes "the shared `Database` for this path, but with these one-off sources"
coherent — either the override poisons the shared instance for every other caller, or the daemon
would need to cold-open and hold a second, unshared `Database` just for that request.

That second option — proxy the override through the daemon, which cold-opens an unshared
`Database` server-side and answers from it — is strictly worse than what `--local` already does
today: it still cold-opens (nothing was warmed), it is still unshared (thrown away after the one
response), and it now pays for an
HTTP round-trip and JSON (de)serialization on top of the same open-then-query work `--local`
already does in-process. Wiring source overrides through the daemon would add `Op`-level surface
across four commands — a new field on `Op::ListTypeRecords`/`Op::Record`/`Op::Search`/`Op::Diff`,
plus `RegistryHost`-side handling for something the `Registry`'s caching model has nothing to offer
— in exchange for a code path that is slower than the direct open it would replace, not faster.

Today's shape — bail in daemon mode, else open in-process — is the correct place for this to
live.

## Consequences

- `list`/`get`/`search`/`diff` share one small helper (`bail_if_daemon_mode_overrides` in
  `src/bin/cli/output.rs`) for the "overrides present + daemon mode → error" check, instead of repeating
  the same `if ... { if daemon_mode { bail!(...) } }` shape four times. The helper only owns the
  guard; each command still builds its own `Database::open` + override-application logic
  afterward, since that part genuinely differs per command (`diff`'s two-sided load is not a
  refactor of `list`'s single load).
- A future fifth command that wants a source-override flag should follow the same shape (compute
  "any override present", call the shared guard, fall through to an in-process open) rather than
  inventing a daemon-side path for it.
- If the daemon's caching model ever changes — e.g. a per-request, explicitly-unshared `Database`
  becomes a supported daemon capability for some other reason — this decision should be revisited
  then, against whatever that new capability actually offers, not reopened speculatively now.
