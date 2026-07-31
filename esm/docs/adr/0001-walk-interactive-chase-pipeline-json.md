# walk is the interactive surface; chase is the JSON-only pipeline evidence contract

Status: accepted (2026-07-31)

`walk` and `chase` began as ports of two prototypes from two consumer repos (dps-76's
`esm-walk.ts`, patch-notes' `chase.py`) and overlapped heavily: on four of chase's five
root types walk was a superset, and walk's OMOD digest was blind to keyword/AVIF-hook
mechanisms — the most common interactive question in this domain. We decided the two
subcommands serve two *contracts*, not two capabilities:

- **`walk`** is the only interactive surface. It digests any record type, and on OMOD
  roots it runs the mechanism classifier inline, rendering each mechanism as path-sliced
  evidence rows (only the consumer's gated `Effects[N]` rows — never a hub perk's full
  digest, which would drown the answer).
- **`chase`** is machine-facing only: it always emits the classified `ChaseTree` JSON,
  hard-errors on non-mechanism root types, and keeps all five root types so the
  patch-notes deep-writer gets one uniform call shape across its `unresolved[]` items.
  Its JSON shape is a frozen contract; its human text renderer was deleted.

Both consume one classifier core in `src/chase.rs` — the verbs differ in contract
(bounded/stable/fail-fast vs expansive/typo-tolerant), not in plumbing.

## Considered options

- **Consolidate into one verb** with fidelity flags (`walk --classify --json`): rejected
  because one flag would have to flip output shape, failure semantics (search fallback vs
  hard error), and expansion behavior at once — a mode smell, and a worse teaching
  surface for agent users than two crisp verbs.
- **Keep both with a cross-referencing hint** (walk's OMOD digest telling the caller to
  run `chase`): rejected because the tool detected its own blind spot and then delegated
  the resolution to the agent, costing extra round-trips on the most common query.
- **Narrow chase back to OMOD-only**: rejected because the deep-writer fans out over
  changed records of all five types and would otherwise need to parse two output shapes.
