# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **Root `AGENTS.md`, "Contexts"**: which project owns which vocabulary and how the projects relate.
- **The glossary of the project you touch**: `esm/CONTEXT.md` (records, decoding, diffs, walks) or `patch-notes/CONTEXT.md` (the pipeline's bundles, tiers and claims). `ba2/` and `esm-viewer/` have none; they borrow no domain terms beyond `esm/`'s.
- **`esm/docs/adr/`**: decisions, including the ones that cross project lines (the BA2 reader duplication, in-process-only access).

A project gains a glossary or an ADR directory when a term or decision first needs one; `/grill-with-docs` creates it. Don't flag the absence or create one upfront.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in the relevant project's `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids, and don't blend vocabulary across projects that don't share it (e.g. don't describe an `esm/` concept using a `ba2/` term).

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/grill-with-docs`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts `esm/docs/adr/0009-ba2-duplication-is-deliberate.md` — but worth reopening because…_

The root `AGENTS.md` records one durable decision the same way: `esm/` and `esm-viewer/` are read-only, and ESM write or serialize support is permanently out of scope. Treat it as ADR-equivalent.
