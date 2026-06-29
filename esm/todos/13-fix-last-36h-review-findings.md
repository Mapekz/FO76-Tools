# Todo: Fix Last-36-Hours Review Findings (rescoped 2026-06-29)

## Context

A review of commits from `5e30163^..f63baff` found five follow-up issues introduced or exposed by
the recent daemon, mmap-index, auto-detection, Electron, and documentation work.

**Reassessment (2026-06-29):** two of the five findings remain open; three are resolved or
superseded. See the [Closed items](#reassessment-2026-06-29--closed-items) section for details.

## Open Findings

### 1. High — Lock down `/op` in legacy server mode

**Root cause (verified):** `build_router` (`src/bin/server.rs:692-709`) registers `/op` under
both modes from the same router. Legacy mode creates `AppState` with `token: String::new()`
(`server.rs:742`); `check_auth` has a `token.is_empty()` short-circuit that returns `Ok(())`
(`server.rs:110`); and `CorsLayer::permissive()` covers the whole router (`server.rs:706`).
`op_handler` dispatches arbitrary `Op` variants over an attacker-controlled `req.esm: PathBuf`
(`src/ipc.rs:221`) — including `Op::Shutdown` and all read ops against arbitrary local file paths.
A malicious web page can POST to `http://127.0.0.1:<port>/op` with no credentials and read
arbitrary local files or shut down the server.

**Key insight:** the legacy embedded UI (`static/index.html`, `static/compare.html`) only fetches
the GET data routes (`/info`, `/records/*`, `/groups`, `/diff`) and **never calls `/op`** at all.
`/op` is dead weight in legacy mode.

**Fix approach:**

- **Split the router.** Create a legacy variant (`build_legacy_router`) with UI pages + GET data
  routes and **no `/op`** route. The daemon variant keeps `/op` behind token auth. Remove the
  `token.is_empty()` bypass from `check_auth` entirely so an empty token never authenticates.
- **Tighten CORS.** Replace `CorsLayer::permissive()` with a same-origin policy for legacy mode
  (embedded UI is same-origin; cross-origin access to game data routes is unnecessary). Daemon
  mode may keep a restrictive CORS policy or none at all (loopback-only access from CLI/Electron).
- **Keep daemon mode intact:** 127.0.0.1, OS-assigned port, 256-bit generated token
  (`server.rs:770`), `Authorization: Bearer` header.

**Tests to add:**

- Daemon `/op` rejects requests with missing or wrong bearer token.
- Legacy mode returns 404 (or similar) for any POST to `/op` — no authenticated path exists.
- Legacy UI GET routes (`/info`, `/records/{formid}`, `/diff`) still respond correctly.

**Relevant files:** `src/bin/server.rs`, `src/ipc.rs` (read-only reference), `tests/ipc.rs` or a
new server-feature test module.

---

### 2. Medium — `.esm.midx` overflow-safe validation

**Root cause (verified):** `MmapFormIndex::try_load` (`src/mindex.rs:86-141`) reads `count` from
the header as an unbounded `u64` cast to `usize` (`:112`), then computes expected file size with
plain unchecked arithmetic: `HEADER_SIZE + count * ENTRY_SIZE` (`:135`). The length guard uses
`<` not `==` (`:136`), so files longer than expected are accepted (trailing garbage passes). In
debug builds a huge `count` panics on overflow; in release builds the multiply wraps to a small
value, the guard passes, and the bogus `count` is returned as `Some`.

`get_by_formid` (`:148-156`) and `read_entry` (`:166-187`) then index the mmap with offsets
derived from that `count` with no bounds guard — an OOB slice index panics.

**Fix approach:**

- In `try_load`: replace `count * ENTRY_SIZE` with `count.checked_mul(ENTRY_SIZE)` and
  `HEADER_SIZE.checked_add(entries_bytes)` — return `Ok(None)` (reject-and-rebuild) on overflow.
- Change the length guard to require an **exact** size match (`mmap.len() != expected`), or
  explicitly document and test the trailing-bytes policy if relaxed.
- In `get_by_formid` / `read_entry`: the validated invariant `HEADER_SIZE + count*ENTRY_SIZE ==
  mmap.len()` is sufficient for safety once `try_load` guarantees it — no extra guard needed in
  the hot path, but ensure the invariant is documented.

**Tests to add** (none of these exist today):

- Huge `count` in header: no panic, returns `Ok(None)`.
- Short file with valid magic/version: returns `Ok(None)`.
- Trailing-garbage file (longer than expected): rejected per the chosen policy.
- Valid round-trip still works (`:304` covers this; keep it).

**Relevant files:** `src/mindex.rs` (primary); `src/index.rs` only if `.midx` write behavior
changes.

---

## Acceptance Criteria

- No unauthenticated or cross-origin request can reach `/op` in legacy server mode.
- Daemon clients still work with bearer-token discovery and `esm -p`.
- Corrupt or adversarial `.esm.midx` files are rejected with `Ok(None)` and rebuilt; no panics.
- Existing daemon, CLI, mmap-index, and Electron workflows remain compatible.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets --features server -- -D warnings
cargo test --features server
cargo test mindex
# Manual:
#   esm -p get path/to/data 0x463F --pretty         # daemon still works
#   esm --local --mmap-index get ... 0x463F          # mmap-index still works
#   start legacy esm-server path/to/data, curl -X POST http://127.0.0.1:<port>/op → blocked
```

---

## Reassessment (2026-06-29) — Closed Items

The following three findings from the original review are closed. Recorded here for traceability.

**Finding 3 — All-hex EditorID escape hatch in Electron: resolved/moot.**
Explicit `recordByEdid` is already plumbed end-to-end: `api-types.ts:11,81`; `ipc.ts:69-73`;
`preload/index.ts:17`. CLI already has `--edid`. The renderer has no free-text lookup field, so
the all-hex ambiguity is unreachable today. Residual note: if a search/input box is added later,
prefer `recordByEdid` over pure auto-detect, or offer a FormID ↔ EditorID toggle.

**Finding 4 — Expose `sources_of` through Electron: superseded → todo #14.**
On reassessment, `sources_of` hardcodes a fixed "terminal source" taxonomy
(`CONT/COBJ/QUST/NPC_/VEND/world`) and leveled-list traversal in Rust — the wrong shape for a
general composable tool surface. Decision: generalize into a recursive `refs --depth N` primitive
and retire `sources_of`. Tracked in todo #14.

**Finding 5 — README trailing whitespace: resolved.**
`git diff --check` is clean. The trailing spaces on `README.md:229` are intentional Markdown hard
line-breaks, not errant whitespace.
