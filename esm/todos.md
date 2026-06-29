# FO76 Parser — Out-of-scope / follow-ups

## Curve tables
- [ ] Optional on-disk cache for the FormID→curve index, keyed by Startup.ba2
      mtime/size (mirror the `*.esm.idx` cache in `src/index.rs`). The in-memory
      index is built and used on every open today; this only avoids the cold-open
      rebuild — the warm daemon already keeps it resident. `CurveIndex`
      (`src/curves.rs`) already derives Serialize/Deserialize but nothing
      persists/reloads it.

## Schema coverage
- [ ] Consider compile-and-introspect extraction via Free Pascal for full fidelity
      (a small program that runs DefineFO76 and serializes the def tree) — higher
      fidelity than the current source-parsing extractor
      (`tools/extractor/extract.py`), but Windows-API portability risk. The
      source-parser already covers 168 record types with raw_fallback=0, so this is
      a fidelity hedge, not a known gap.

## FormID / load order
- [ ] Cross-file FormID resolution & load-order fixup across masters (multi-plugin).
      `Database` currently wraps a single `EsmFile`; the TES4 masters list is exposed
      via `file_info()`, but there is no multi-plugin load or cross-file FormID fixup.

## Productization (post-POC)
- [ ] Chatbot front page over the HTTP/MCP server. The static UI is a record browser;
      the MCP server already exposes the six read-only tools a chatbot would call.
