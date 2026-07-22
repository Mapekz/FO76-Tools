# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

FO76-Tools is a **multi-context** repo: three independent Fallout 76 tools (`ba2/`, `esm/`, `esm-viewer/`) that share almost no code or vocabulary — a BA2 archive reader, an ESM record/subrecord decoder + server, and an Electron viewer UI. Each already has its own `CLAUDE.md`; domain docs follow the same per-subproject split.

## Before exploring, read these

- **`CONTEXT-MAP.md`** at the repo root, once it exists — it points at one `CONTEXT.md` per subproject. Read the one(s) relevant to the topic (e.g. only `esm/CONTEXT.md` for an ESM-decoding task; both `esm/CONTEXT.md` and `esm-viewer/CONTEXT.md` for a viewer task that touches the native addon boundary).
- **`docs/adr/`** at the repo root — system-wide decisions that span subprojects (rare, given how little they share).
- **`ba2/docs/adr/`**, **`esm/docs/adr/`**, **`esm-viewer/docs/adr/`** — per-subproject decisions. Read the ones for the subproject(s) you're about to touch.

If any of these files don't exist yet, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The producer skill (`/grill-with-docs`) creates them lazily when terms or decisions actually get resolved.

## File structure

```
/
├── CONTEXT-MAP.md                     ← points at the three CONTEXT.md files below
├── docs/adr/                          ← system-wide decisions (cross-subproject)
├── ba2/
│   ├── CONTEXT.md
│   └── docs/adr/
├── esm/
│   ├── CONTEXT.md
│   └── docs/adr/
└── esm-viewer/
    ├── CONTEXT.md
    └── docs/adr/
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in the relevant subproject's `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids, and don't blend vocabulary across subprojects that don't share it (e.g. don't describe an `esm/` concept using a `ba2/` term).

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/grill-with-docs`).

## Flag ADR conflicts

If your output contradicts an existing ADR (root `docs/adr/` or a subproject's `docs/adr/`), surface it explicitly rather than silently overriding:

> _Contradicts `esm/docs/adr/0003-read-only-by-design.md` — but worth reopening because…_

Note: the root `CLAUDE.md`'s **Scope** section already documents one durable decision this way — `esm/` and `esm-viewer/` are read-only by design, and ESM write/serialize support is permanently out of scope. Treat that as ADR-equivalent even before it's formalized into a `docs/adr/` file.
