# FO76-Tools — Backlog

The single backlog for every subproject. Add follow-ups here, grouped under the project they
belong to — do not reintroduce per-project `todos.md` files or a `todos/` directory.

Each item states what it is, and — where it matters — why it is or isn't worth doing now.
Scope checks are dated so a stale claim is obvious on sight.

---

## `esm/`

### Schema & extractor

- [ ] **Property-name errata: `MinPowerPerShot` → `MaxPowerPerShot`.** The engine renamed this
      property (~2025); the extractor still emits the old name from its property-name list
      (`esm/tools/extractor/extract.py:301`), so every WEAP/OMOD consumer reads a field whose
      name means the opposite of what it holds. Fix the name in the list, regenerate
      `schema/fo76.json`, and re-run `just audit`. Small, but it silently misleads any analysis
      of power-weapon stats. Already documented as errata in the patch-notes mechanics KB
      (`esm/.claude/skills/patch-notes/mechanics-kb.md`).

- [ ] **Compile-and-introspect schema extraction via Free Pascal** *(dormant)*. Instead of
      source-parsing `wbDefinitionsFO76.pas` with `esm/tools/extractor/extract.py`, build a small
      FPC program that runs `DefineFO76` (dispatch at `TES5Edit/xEdit/xeInit.pas:~1383`), walks
      the resulting def tree in memory, and serializes it to JSON matching the `esm/src/schema.rs`
      shape. Full def-tree fidelity, but needs an FPC toolchain and carries Windows-API
      portability risk for the xEdit codebase.

      **Why it's dormant:** the original motivation was closure deciders that regex/bracket
      parsing can't reach — and those got solved another way, via the runtime
      `byte_offset`/`width_bytes` branching in `ByteAtOffset` (`esm/src/schema.rs:245-273`).
      *Scope check 2026-07-13:* the Python parser covers **181 record types, 0 failed**, the
      schema has **zero `raw_fallback` entries**, `audit.py` reports no active parity findings,
      and `coverage --gate` keeps it that way. Revive only if a future record type reintroduces
      an unresolvable closure decider, or if the parser starts breaking on new TES5Edit
      revisions.

### CLI — patch-notes fast-follows

Scoped during the 2026-07-13 patch-notes redesign; not started. Ordered by leverage — each cuts
agent tokens or round-trips out of the weekly `/patch-notes` deep pass.

- [ ] **`refs --paths`** — annotate each ref row with the JSON field path inside the referencing
      record (e.g. `Effects[2].Conditions[0].Parameter 1`). The decode machinery to produce the
      path already exists; this is the highest-leverage item, since deep agents currently re-`get`
      a whole record just to find *where* the reference lives.
- [ ] **`esm chase <omod>`** — automate the unique-effect walk (keyword ADDs → reverse-refs →
      keyword-conditioned effects → compact evidence tree). Prototype in Python over the CLI
      before committing to a Rust subcommand. The walk is written up under *"How unique-weapon
      effects are implemented (the chase pattern)"* in
      `esm/.claude/skills/patch-notes/mechanics-kb.md`.
- [ ] **`refs --type` filter** — trivial; narrows reverse lookups to a record type.
- [ ] **Bulk `get`** (multiple FormIDs per call) — `RecordSel` in `esm/src/ipc.rs` is close to
      supporting this already.
- [ ] **Server-side subtree filter** — only worth doing if token pressure persists after the
      first two land.

### Curve tables

- [ ] **Optional on-disk cache for the FormID→curve index**, keyed by Startup BA2 mtime/size and
      mirroring the `*.esm.idx` cache in `esm/src/index.rs` (own `CACHE_VERSION`). `CurveIndex`
      (`esm/src/curves.rs`) already derives `Serialize`/`Deserialize`; nothing persists or
      reloads it.

      *Scope check 2026-07-13:* the warm daemon keeps the index resident, so this no longer helps
      the common path. The remaining beneficiary is the cold in-process open — chiefly
      `get --startup-ba2` / `--localization-ba2`, which bypass the daemon by design. Low priority
      unless cold-open latency starts mattering.

### Productization (post-POC)

- [ ] **Chatbot front page over the HTTP/MCP server.** The static UI (`esm/static/index.html`,
      `esm/static/compare.html`) is a record browser; the MCP server already exposes the six
      read-only tools a chatbot would call.

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
