# Cache-build progress and cross-process dedup live on the filesystem, not the daemon

Status: accepted (2026-08-04)

A cold `esm get/walk/refs/search` against an ESM with no `esm_cache/` yet blocks for tens of
seconds to a couple of minutes (`Index::build`'s `build_tree_and_forms`, or one of the three lazy
`ensure_*_index` builds — `xref` in particular decodes every record in the file) with **no output
of any kind** until the result finally prints — indistinguishable from a hang on the full FO76 ESM.
Nothing coordinated concurrent builders either: two processes that both hit a cold ESM each did the
full duplicated work (two whole-ESM walks, ~200 MB of rkyv serialization apiece), invisible to one
another. Several Claude Code sessions run against this workspace concurrently in practice, so this
wasn't a hypothetical.

## Decision

A per-ESM **advisory build lock** (`<esm file name>.build.lock`) plus an atomically-published
**JSON heartbeat** (`<esm file name>.build.json`), both living inside the existing `esm_cache/`
directory alongside the five rkyv sections (see `src/rkyvcache.rs`). Whichever process is
building — the daemon, a `--local` CLI invocation, or the N-API host — publishes to it via
`progress::BuildLease`; any observer reads it via `progress::read`, which is instant and never
blocks: it takes a non-blocking `try_lock_exclusive` on the lock file, so "is a build in flight" is
one syscall, answerable even while the daemon's own per-ESM `Mutex<Database>` (`registry.rs`) is
held for the whole build.

This protocol lives on the filesystem, **not** a daemon HTTP endpoint: an endpoint would only cover
the daemon path — `--local`, the N-API/Electron host, and `tools/esm_gateway.py` would stay blind,
and a second `--local` process couldn't dedup against a building daemon at all. One filesystem
protocol covers every caller uniformly with no IPC.

Each of `Index`'s four build entry points (`Index::build`'s `build_tree_and_forms`,
`ensure_edid_index`, `ensure_search_index`, `ensure_xref_index`) acquires a `BuildLease` *before*
doing any real work and **re-checks `Section::map` immediately after the lock is granted** —
another process may have finished building and published the section while this call was blocked
waiting. Only if the section still doesn't exist does it actually rebuild. This is what makes the
lock double as dedup, not just a coordination signal: a second process pays only the cost of
waiting for the lock, never a redundant walk.

The lock is **per-ESM, not per-section**. A builder mid-`xref` blocks a second process that only
wants `edid`, even though the two don't share data dependencies in that specific case: the two
would otherwise fight over the same mmap'd ESM and CPU for no real concurrency benefit, and five
independent locks (one per `SectionKind`) would be meaningfully more machinery for a benefit that
mostly doesn't materialize in practice — builds against the same ESM cluster in time (a fresh
snapshot gets queried repeatedly right after landing), so the common case is exactly the one this
doesn't help.

CLI consumers (`src/bin/cli/progress_ui.rs`) build on `progress::read` alone:

- Every `backend.run()` call (wrapped once, in `impl QueryBackend for Backend`, not scattered
  across each `cmd_*` function) spawns a watcher thread that renders the heartbeat to stderr — a
  `\r`-updated bar on a TTY, one throttled plain line otherwise — after a 500 ms grace period so a
  warm call never flickers anything. `stop()` blocks until any rendered line is erased,
  synchronously, before control returns to whichever `cmd_*` function is about to print its result,
  so a progress line can never race the real output.
- `--no-wait` checks `progress::read` client-side, before a `Backend` is even constructed, and
  exits immediately (status 75) if a build is already in flight; it never routes through the
  daemon, for the same reason `read` itself doesn't block.
- `esm cache status [--json]` combines `progress::read` with `index::cache_inventory` (the same
  O(1) `Section::map` header check `Index::build` uses, without its rebuild fallback) to report
  `empty`/`building`/`partial`/`complete` — a pure read that never opens the ESM or contacts the
  daemon.
- `RemoteBackend::post_op` no longer puts a single opaque deadline on the daemon HTTP round-trip. A
  per-attempt timeout (`ESM_OP_ATTEMPT_TIMEOUT_SECS`, default 20s) is retried — not surfaced as an
  error — for as long as `progress::read` shows a live, non-stalled build on the requested ESM, up
  to the pre-existing `ESM_OP_TIMEOUT_SECS` overall budget (default 300s, now reinterpreted as a
  retry ceiling rather than one call's deadline). Safe unconditionally: every op is read-only, and
  a retried request simply re-queues behind the daemon's mutex rather than restarting the build,
  which keeps running server-side regardless of whether an earlier attempt's connection was
  abandoned.

`ESM_NO_PROGRESS=1` disables heartbeat *publishing* only (for hosts like the N-API/Electron
embedding, where a stray file write is undesirable) — the lock itself, and the dedup it provides,
is unconditional.

## Considered options

- **A daemon `/progress` HTTP endpoint instead of a filesystem protocol**: rejected — see above.
  Doesn't cover `--local`, N-API, or a second `--local` process racing the daemon.
- **Restructure the daemon's `Registry`/`Mutex<Database>` locking so a build-in-progress request
  returns early instead of blocking**: rejected for this change. Blocked callers still block; they
  gain visibility (a live heartbeat), not concurrency. A genuinely non-blocking daemon dispatch path
  would be a materially larger, higher-risk change for a benefit `--no-wait` and the retry-aware
  `RemoteBackend` timeout already capture from the client side.
- **A wall-clock-age heuristic for heartbeat staleness** (e.g. "no update in N seconds means the
  writer crashed"): rejected as the primary staleness signal. The lock already gives an exact
  answer — an advisory `flock` releases the instant its owning process's file descriptors close,
  on a clean exit or a crash alike — so `read` treats "can I take this lock right now" as ground
  truth and deletes any heartbeat left over from a builder that never got to run its `Drop`. A
  wall-clock threshold (`BuildProgress::is_stalled`, 60s) still exists, but only for the narrower,
  different question "is a *live* builder wedged," not "is anyone building at all."
- **Sweep `write_section`'s orphaned `*.tmp.<hex>` write debris as part of this change**: considered
  and left out. `BuildLease::acquire` holding the per-ESM lock is exactly the safe moment to prove
  no other section write is in flight for that ESM, which would make a bounded sweep cheap — but
  it's a pre-existing, separate gap (nothing before this change cleaned that debris up either), and
  folding it in here would widen this change's blast radius for an unrelated fix. Left as a
  candidate follow-up, not filed as a blocker.
