# Array element identity is owned by Rust; CTDA `Conditions[]` is deliberately unkeyed

Status: accepted (2026-08-10)

`diff.rs`'s `array_diff` classifies every decoded rarray into one of four pairing strategies —
`keyed`, `positional`, `set`, `unkeyed` — and `element_key_spec` is the one table that decides
which field(s) identify an element (OMOD properties by `(Function Type, Property)`, leveled-list
entries by `(Reference, Minimum Level)`, and so on). Before this decision, an array that
`element_key_spec` couldn't classify and whose lengths differed between the two sides fell back
to a bare `{"from": [...], "to": [...]}` leaf — indistinguishable, to every downstream
`_array_diff` reader, from an ordinary scalar leaf whose values happened to be lists.

`patchnotes_lib.py` independently duplicated a coarser version of the same table
(`smart_array_diff`'s six detectors), for reading *pre-`_array_diff`* `diff.json` still on disk.
Neither table had an entry for CTDA `Conditions[]`. Measured on the `20260724 → 20260803`
production run: 54 of 1579 array changes rendered as a content-free header in
`comprehensive.md` — 26 of them `Conditions / Conditions` — because `_render_change_bullet`'s
`kind == "array"` branch had no fallback for an empty `array` block, and the bare leaf shape
never populated one. `triage_bundles.summarize_change` compounded it: the same entry summarized
as `1->4 items (+0 -0 ~0)`, telling the tier-assessor agent nothing had changed. A live example
this defect actually dropped: the `PiercingLove` recipe (`COBJ`) gaining a `HasLearnedRecipe == 1`
gate — an acquisition change.

## Decision

Rust is the sole owner of element identity. `element_key_spec` is not extended to cover
`Conditions[]` or any other array whose element order carries meaning; instead, `array_diff`'s
fallback is a real fourth strategy, `unkeyed_array_diff`, wrapped in the ordinary `_array_diff`
envelope (`strategy: "unkeyed"`, whole `removed`/`added` element lists, no `changed`). Every
`_array_diff` reader — `patchnotes_lib.extract_changes`, `triage_bundles.summarize_change`,
`run_lints`, `esm-viewer`'s `DiffPanel` — inherits real content for these arrays instead of a
blob, without having to special-case a fifth shape.

**`Conditions[]` staying unkeyed is not a gap — it is correct.** A condition's position in the
array is semantic: consecutive rows chain via their own `AND/OR` field, so pairing element *i* of
the old list against element *i* of the new list by any synthetic key would report false
mutations whenever a condition is inserted, removed, or reordered anywhere but the tail. Reporting
the two whole lists is the only shape that doesn't lie. **A future architecture review must not
re-propose "add a CTDA key spec to `element_key_spec`"** — this ADR exists specifically so that
suggestion doesn't get re-litigated without this context.

`patchnotes_lib.smart_array_diff` and its six legacy differs are *not* deleted by this change —
they remain the reader for `diff.json` files already on disk from before this ADR, which still
carry the bare `{from, to}` shape. Their deletion is a separate, evidence-gated follow-up: once
production has run enough patch-notes cycles on the new shape, confirm nothing still reaches
`smart_array_diff` and remove it then.

## Consequences

- `diff.json`'s array-diff envelope has four `strategy` values, not three. Any code reading
  `_array_diff.strategy` by exhaustive match (Rust or Python) must add the `unkeyed` arm.
- `patchnotes_lib._struct_display` unwraps single-key wrapper elements (e.g.
  `{"Condition": {"Condition Data": {...}}}`) before rendering, since `unkeyed` array elements are
  full raw decoded values rather than already-summarized keyed rows — without this, an unkeyed
  element renders as the unhelpful `` Condition=`(struct: Condition Data)` ``.
- This ADR does not touch `element_key_spec`'s existing ten heuristics — none of them changes.
