# FO76-Tools — Backlog

The single backlog for every subproject. Add follow-ups here, grouped under the project they
belong to — do not reintroduce per-project `todos.md` files or a `todos/` directory.

Items are ordered by priority (P1 highest). Each states what it is and why it sits where it
does. Scope checks are dated so a stale claim is obvious on sight. All items below were
re-verified against the code on 2026-07-14; none is partially implemented.

---

## `esm/`

*Conditional, not a checkbox:* **server-side subtree filter** — P2 (`refs --paths`) and P4
(bulk `get`) have landed; add this only if `/patch-notes` token pressure persists in practice.

- [ ] **P6 — Chatbot front page over the HTTP/MCP server** *(post-POC productization)*. The
      static UI (`esm/static/index.html`, `esm/static/compare.html`) is a record browser; the
      MCP server (`esm/src/bin/server.rs`) already exposes the six read-only tools a chatbot
      would call. No urgency — the one "someday" item.

---

## `ba2/`

No tracked follow-ups.

Note: DX10 texture archives are **deliberately** detected and rejected — that is a documented
invariant (GNRL-only), not a gap. Adding DX10 support needs an explicit design and a separate
code path, so it is not a backlog item.

---

## `esm-viewer/`

No tracked follow-ups.

---

## Cross-cutting

No tracked follow-ups.

The one cross-project seam is `esm-viewer/` → `esm/bindings/napi` (the `@fo76/esm-napi` addon,
a local `file:` dependency). Anything that changes the `EsmDatabase` N-API surface has to land
on both sides — run `just gen-types` in `esm/` to regenerate the shared TypeScript DTOs.

---

## Removed 2026-07-14

Two items were dropped as no longer worth carrying (full write-ups in git history at 668ceb1):

- **FPC compile-and-introspect schema extraction** — its motivating problem (unreachable
  closure deciders) was solved by runtime `ByteAtOffset` branching; the Python parser covers
  181 record types with 0 failures and `coverage --gate` holds the line. If the parser breaks
  on a future TES5Edit revision, that failure is the signal to reconsider.
- **Curve-table on-disk cache** — the warm daemon keeps `CurveIndex` resident, so only cold
  in-process opens (`get --startup-ba2`) would benefit. Revisit only if cold-open latency
  starts mattering.
