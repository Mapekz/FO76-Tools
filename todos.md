# FO76-Tools — Backlog

The single backlog for every subproject. Add follow-ups here, grouped under the project they
belong to — do not reintroduce per-project `todos.md` files or a `todos/` directory.

Items are ordered by priority (P1 highest). Each states what it is and why it sits where it
does. Scope checks are dated so a stale claim is obvious on sight. All items below were
re-verified against the code on 2026-07-14; none is partially implemented.

---

## `esm/`

- [ ] **P2 — `refs --paths`.** Annotate each ref row with the JSON field path inside the
      referencing record (e.g. `Effects[2].Conditions[0].Parameter 1`). Highest-leverage
      `/patch-notes` cost cut: deep agents currently re-`get` a whole record just to find
      *where* the reference lives. The decode machinery to produce the path already exists;
      extend `RefRow` (`esm/src/ipc.rs:182`) and the `Refs` command
      (`esm/src/bin/cli.rs:183`).

- [ ] **P3 — `refs --type` filter.** Narrow reverse lookups to a record type. Trivial; same
      command surface as P2 — land them together.

- [ ] **P4 — Bulk `get`** (multiple FormIDs per call). Cuts round-trips out of the weekly deep
      pass. `RecordSel` (`esm/src/ipc.rs:49`) is still single-select (`FormId` | `Edid`) as of
      2026-07-14; extend it and `Op::Record`/`Op::RecordRaw`.

- [ ] **P5 — `esm chase <omod>`.** Automate the unique-effect walk (keyword ADDs → reverse-refs
      → keyword-conditioned effects → compact evidence tree), written up under *"How
      unique-weapon effects are implemented (the chase pattern)"* in
      `esm/.claude/skills/patch-notes/mechanics-kb.md`. Prototype in Python over the CLI before
      committing to a Rust subcommand. Sequenced after P2–P4: the walk composes `refs --paths`,
      `refs --type`, and bulk `get`.

  *Conditional, not a checkbox:* **server-side subtree filter** — re-evaluate only if token
  pressure persists after P2 and P4 land.

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
