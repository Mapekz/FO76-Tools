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
  Its JSON shape is a frozen, additive-tolerant contract — new optional fields and new enum
  variants only, never a rename or removal, unless a future ADR explicitly revisits it (extended
  2026-08-01); its human text renderer was deleted.

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

## Where the walk/chase computation runs (extended 2026-08-10)

`walk`/`chase` run via `Op::Walk`/`Op::Chase` (plus `Op::DropTable` for
`crate::lvli::drop_table`, reachable before this only through `walk`'s LVLI digest) in
`src/ipc.rs`, dispatched against an already-open `Database` the same way every other `Op` variant
is. The BFS and the classifier run inside whatever process is already handling the op — the
daemon, `--local`'s in-process `Database`, or the N-API addon's `EsmDatabase` — via an in-process
`ChaseFetcher` adapter (`ipc.rs`'s `DbFetcher`) that reads straight off the open `Database`, no
serialization or round-trip per fetch. This is a relocation, not a reversal of this ADR's
decision: `walk` is still the only interactive surface (its digest/rendering split, OMOD
mechanism slicing, and LVLI drop-odds wrapping are unchanged), and `chase` still always emits the
same frozen `ChaseTree` JSON and hard-errors on non-mechanism root types — only *where* the BFS
and classifier run changed. `walk/render.rs` remains the sole place a `Digest`/`WalkResult`
becomes text or `--json` output, so `--local` and daemon output stay byte-identical. The MCP
server (`esm_walk`/`esm_chase`/`esm_lvli_drop_table`) and the N-API binding
(`EsmDatabase::walk`/`chase`/`lvli_drop_table`) now reach the same computation the CLI always
could.
