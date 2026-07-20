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

*Conditional, not a checkbox:* **`chase`/`walk` exposure via N-API/HTTP/MCP** — both are
possible (they're already pure functions over the `ChaseFetcher` seam) but deferred; today
they're CLI-only (`esm chase`, `esm walk`). Add only if an agent-facing surface other than the
CLI (esm-viewer, the HTTP/MCP server, a chatbot front end) actually needs one-shot chase/walk
digests instead of composing `get`/`refs` itself.

- [ ] **P3 — CNDF condition decoding: verify against a non-stub record** *(suspected
      misparse, 2026-07-19)*. `RA_SCORE_ContextualSeasonCollectAll_Condition` (CNDF
      0x0086A8A5, 20260710 dump) decodes as 64 byte-identical rows — `GetDead == 1.0`,
      OR-flagged, Run On Subject, ALL param/reference slots zeroed, trailing `0xFFFFFFFF`.
      Raw evidence: every CTDA subrecord is exactly
      `010000000000803f2e00000000000000000000000000000000000000ffffffff` (32 bytes), so the
      decoder is at least faithful to the bytes at the offsets it reads — but a
      "collect all" condition made of 64 identical no-param stubs is semantically absurd,
      so either (a) the record is a server-populated live-service placeholder (SCORE
      seasons), or (b) CNDF-context CTDA uses a different layout / function-index table
      than standard CTDA and the decoder maps it wrong. To settle it: decode several OTHER
      CNDF records and check whether non-SCORE CNDFs produce varied, parameterized,
      sensible conditions — if CNDFs read as stubs across the board, suspect (b) and
      re-derive the layout from a record with known-good semantics.
- [ ] **P4 — `--json` stdout hygiene in daemon mode** *(confirmed bug, 2026-07-19)*.
      `esm get <id> --json` via the warm daemon appends a trailing `esm> ` REPL prompt
      after the JSON document on stdout, which breaks strict parsers (`json.load` →
      "Extra data"). Prompt belongs on stderr (or suppressed entirely for one-shot `-p`
      calls).
- [ ] **P4 — Investigate elevated diff Changed count post-LString fix** *(2026-07-20)*.
      `diff_two_esm_versions_glob` (20260710→20260717) reports `Changed: 129323` after the
      LString id-0 and table-kind fixes landed (down from 156009 before — the fixes accounted
      for ~26,686 records of spurious diff noise from SeventySix.esm's Localized-flag flip in
      20260717). The remaining ~129K is presumed normal per-patch content churn but hasn't been
      confirmed — spot-check a sample of "changed" records before the next `/patch-notes` run
      to rule out a further decode-shape artifact from the localized-string transition.
- [ ] **P5 — INFO `Comment?` (RNAM) LString mislabeling** *(2026-07-20)*. After the LString
      table-kind fix, 12 residual `_unresolved` markers remain in the 20260717 coverage sweep,
      all in INFO's `Comment?` field (RNAM — the one INFO subrecord xEdit's
      `LocalizedValueDecider` keeps in `.strings` rather than `.ilstrings`). In every case the
      `lstring_id` exactly equals the record's own FormID, a strong signal RNAM isn't really an
      lstring field at all (the schema's own `?` suffix on the name already flags it as a
      low-confidence extractor guess). Verify against `../TES5Edit/Core/wbDefinitionsFO76.pas`'s
      actual INFO RNAM definition and fix the schema/extractor if it's misclassified.
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
