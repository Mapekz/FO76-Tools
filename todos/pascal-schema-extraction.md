> **Status (2026-07-12, migrated from dps-76/dps-todos/):** NOT STARTED, and arguably **obsolete**.
> No `tools/pascal-extract/` exists; the schema still comes from the Python source-parser
> `esm/tools/extractor/extract.py`. But that pipeline (plus hand-maintained
> `schema/fo76.overrides.json` / `schema/fo76.ctda.json`) already reaches full fidelity — 181 record
> types, **zero** `raw_fallback` entries (verified: `grep -c raw_fallback schema/fo76.json` → 0), and
> the `coverage --gate` CLI check enforces that stays true. The original motivation (closure deciders
> that regex/bracket parsing can't reach) has been solved another way — see `hard-union-deciders`
> (now closed) for how unions got decided via runtime `byte_offset`/`width_bytes` branching instead.
> Only worth reviving if a future record type reintroduces an unresolvable closure decider.

# TODO: Pascal Compile-and-Introspect Schema Extraction

## What
Alternative to source-parsing `wbDefinitionsFO76.pas`: build a small Free Pascal program that runs `DefineFO76`, walks the resulting def tree in memory, and serializes it to JSON — higher fidelity than regex/bracket parsing in `extract.py`.

## Trade-offs
| Approach | Pros | Cons |
|----------|------|------|
| `extract.py` (current) | Cross-platform, no Pascal toolchain | Misses closure deciders; parsing edge cases |
| Pascal introspect | Full def tree fidelity | Requires FPC; Windows-API portability risk for FO76 xEdit codebase |

## References
- `esm-parser/tools/extractor/extract.py` — current extractor
- `TES5Edit/xEdit/xeInit.pas` — `DefineFO76` dispatch (~1383)

## Where to implement
- New tool under `esm-parser/tools/pascal-extract/` or `TES5Edit/Tools/`
- Output must match `esm-parser/src/schema.rs` JSON schema shape
