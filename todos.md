# FO76-Tools — Backlog

Tracked, actionable work lives in [GitHub Issues](https://github.com/Mapekz/FO76-Tools/issues)
(`gh`, see `docs/agents/issue-tracker.md`) — that's the live, priority-triaged queue as of
2026-07-24, ranked with `P1`/`P2`/`P3` labels (see `docs/agents/triage-labels.md`). This file
is the complement: informal dated notes not yet promoted to an issue and
scope decisions deliberately *not* tracked as work items, grouped under the project they belong
to. Do not reintroduce per-project `todos.md` files or a `todos/` directory — this stays the
one file.

Scope checks are dated so a stale claim is obvious on sight.

---

## `esm/`

No tracked follow-ups — outstanding work lives in [GitHub Issues](https://github.com/Mapekz/FO76-Tools/issues).

One 2026-07-22 architecture-review finding was deliberately **not** filed: the seven IPC methods
carved out of `esm-viewer`'s `CONTRACT` table and hand-written in three places. The carve-out is
documented and intentional (those methods have non-uniform shapes — `diff` needs two registry
lookups), so it reads as a considered partial refactor rather than an oversight.

---

## `ba2/`

No tracked follow-ups.

Note: DX10 texture archives are **deliberately** detected and rejected — that is a documented
invariant (GNRL-only), not a gap. Adding DX10 support needs an explicit design and a separate
code path, so it is not a backlog item.

---

## `esm-viewer/`

No tracked follow-ups.

Scope note (2026-07-24): the vitest include glob deliberately excludes `.tsx`, so component
tests are structurally absent — a considered choice, not a gap. Enabling them is tracked at
low priority in [#16](https://github.com/Mapekz/FO76-Tools/issues/16) (`P3`,
`ready-for-human`), kept open in case component testing becomes desirable later.

Follow-up (2026-07-28): `esm refs --ep` now seeds `path[0]` with the originating carrier so
`VIA` is populated at depth 1, but `esm-viewer/src/renderer/src/components/ReferencedByPanel.tsx`
still renders `HopChain` as `null` for `depth <= 1` — the GUI will not show the new depth-1
carrier link without a follow-up change there. Out of scope for the EP-attribution work that
landed the column in the CLI/JSON.

---

## Cross-cutting

No tracked follow-ups.

The one cross-project seam is `esm-viewer/` → `esm/bindings/napi` (the `@fo76/esm-napi` addon,
a local `file:` dependency). Anything that changes the `EsmDatabase` N-API surface has to land
on both sides — run `just gen-types` in `esm/` to regenerate the shared TypeScript DTOs. As of
2026-07-22 the drift guard runs in CI too, so forgetting the regen now fails the build rather
than passing silently.

The typed-envelope gap at that boundary — N-API returns `serde_json::Value`, so
`bun run typecheck` can't verify the seam — is written up in full in
[#9](https://github.com/Mapekz/FO76-Tools/issues/9).
