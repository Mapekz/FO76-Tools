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

No tracked follow-ups beyond the issue queue.

Scope change (2026-07-29): the GNRL-only stance is retired — the CLI should handle all BA2
archives. DX10 (texture) read/list/extract support is planned in
[#21](https://github.com/Mapekz/FO76-Tools/issues/21) (separate code path per the reader
invariant; DX10 *write* remains out of scope pending its own design). Until #21 lands, the
reader still detects and rejects DX10 with a clear error.

---

## `esm-viewer/`

No tracked follow-ups.

Scope note (2026-07-24): the vitest include glob deliberately excludes `.tsx`, so component
tests are structurally absent — a considered choice, not a gap. Enabling them is tracked at
low priority in [#16](https://github.com/Mapekz/FO76-Tools/issues/16) (`P3`,
`ready-for-human`), kept open in case component testing becomes desirable later.

The 2026-07-28 follow-up (ReferencedByPanel hides the depth-1 `VIA` carrier that
`esm refs --ep` now provides) is promoted to
[#20](https://github.com/Mapekz/FO76-Tools/issues/20).

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
