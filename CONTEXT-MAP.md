# Context Map

Three independent Fallout 76 tools live in this repo (see `docs/agents/domain.md` for how
to consume these docs). Per-subproject `CONTEXT.md` files are created lazily — only `esm/`
has one so far.

## Contexts

- [esm](./esm/CONTEXT.md) — reading, diffing, and explaining FO76 ESM records
- ba2 — BA2 archive reading (no `CONTEXT.md` yet)
- esm-viewer — Electron GUI over esm's native addon (no `CONTEXT.md` yet)

## Relationships

- **esm → esm-viewer**: esm-viewer consumes esm's N-API addon (`esm/bindings/napi`); they
  share esm's record/decode vocabulary
- **ba2 ↔ esm**: esm reads strings/curve tables out of BA2 archives via its own independent,
  minimal, read-only reader (`esm/src/ba2.rs`), not the `ba2` crate — a deliberate decision to
  avoid pulling `ba2`'s write-side dependencies and `Codec::Auto` zlib-tolerance into esm's build,
  recorded in `esm/docs/adr/0009-ba2-duplication-is-deliberate.md`; otherwise the two share no
  vocabulary
