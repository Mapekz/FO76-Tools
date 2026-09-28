# Schema overrides are one reasoned list, and the parity audit applies it

Status: accepted (2026-09-27).

`schema/fo76.overrides.json` had three mechanisms: `records` (whole-record definitions),
`record_patches` (path-addressed replace or insert-after) and `record_additions` (appends to a
record's members). The reason for each lived in an optional `_comment`, and the parity audit
kept a separate allowlist (`parity-exceptions.json`) whose entries restated those reasons against
audit paths. An override without an allowlist entry failed the audit; an allowlist entry
outlived the override it described.

## Decision

`fo76.overrides.json` is one ordered list, `overrides`, of `{record, op, path, reason, node}`:

- `op` is `replace`, `insert_after` or `append`. `path` names members by sig or name, with
  `element` entering an array's element; an empty `path` addresses the record itself, so a
  record-level `replace` defines a record xEdit lacks (ADR 0012). `node` may be a list for
  `insert_after` and `append`.
- `reason` is required: it says why the game data diverges from xEdit's definition.
- `tools/extractor/extract.py` validates every entry and applies them in order.

`tools/extractor/audit.py` applies the same entries to its Pascal-derived tree before comparing
it with the shipped schema, so every divergence an override causes is explained by that
override, and the allowlist is gone. The audit reports a `replace` that changes nothing as
`redundant-override`: xEdit caught up, and the entry should be deleted.

## Consequences

The audit's remaining findings are divergences no override explains: a stale `fo76.json`,
integer tokens the extractor defaults, stub downgrades, and Pascal helpers it drops.
