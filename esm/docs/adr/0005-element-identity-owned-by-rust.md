# Array element identity is owned by Rust; CTDA `Conditions[]` is deliberately unkeyed

Status: accepted (2026-08-10); extended (2026-08-10) — see "Key uniqueness is validated, not
assumed" below.

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

**`Conditions[]` stays unkeyed because element order is semantic.** A condition's position in the
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

## Key uniqueness is validated, not assumed

`element_key_spec`'s heuristics *propose* an identity from one sample element's shape — they never
inspect the actual pair of arrays. Two follow-on defects fell out of that gap, both measured on the
same `20260724 → 20260803` run this ADR's original numbers came from:

- **Missing keys.** LCTN reference-list elements have two or more FormID-shaped members — `Master
  Special References` carries `(Ref, Loc Ref Type, World/Cell)`, `Master Persist Location
  References` only `(Ref, World/Cell)`, `Master Enable Parent References` `(Ref, Enable Parent)`,
  `Master Unique NPCs` `(Actor Ref, NPC)` — so the old "exactly one FormID-shaped member" fallback
  never matched any of them: a pure reshuffle (order changed, no field edited) fell to `positional`
  and read as every element mutating. This was **81% of all array-diff envelopes** in the run
  (1,278 of 1,576), **68,067 flagged index rows**, and **91% of `comprehensive.md`'s line count**.
  The fix generalizes the fallback itself rather than hand-curating one heuristic per exact field
  combination: it now composes *every* FormID-shaped member (sorted by name) instead of requiring
  exactly one, covering all four LCTN shapes — plus any future one — with a single rule. A separate
  new heuristic keys quest aliases on `Alias ID` (present in every alias-kind wrapper
  `unwrap_wrapper` already strips) — not FormID-shaped, so the composite fallback can't reach it. A
  third gap surfaced only after the composite fix landed and was re-measured against real data:
  `RACE`/`NPC_` `Attacks[]` entries with no `Required Slot` sibling decode as the single-key wrapper
  `{"Attack": {...}}`, which `unwrap_wrapper` strips to an inner struct with *no* FormID-shaped
  members at all (`Attack Event` is a plain animation-event string) — the composite fallback can't
  reach these either. This was the largest remaining source of positional-permutation noise (74 of
  78 cases after the LCTN fix). A fourth heuristic keys this shape on `Attack Event` directly,
  matching the existing `wrapper == Some(...)` pattern (`Leveled List Entry`/`Effect`/`Objective`/
  `Stage`) rather than the FormID-based rules.
- **Non-unique keys.** The "one FormID-shaped member" heuristic can propose a key many elements
  legitimately share — `RACE`/`NPC_` `Attacks[]` keyed on `Required Slot` is the motivating case:
  **30 of 83 keyed envelopes (36%)** had duplicate keys, `SheepsquatchRace/Attacks` alone producing
  32 rows of scrambled `Attack Event: A -> B` churn from `keyed_diff`'s FIFO-within-group pairing —
  silently, with no signal in the output that the key wasn't actually unique.

`widen_key_spec_until_unique` (`src/diff/array_diff.rs`) runs between `element_key_spec` and `keyed_diff`: if
a proposed key isn't unique on both sides, it appends further scalar leaf fields (sorted,
deterministic order) one at a time until it is, or falls back to `unkeyed_array_diff` if no
widening achieves it. The widened key is visible in `diff.json` via the existing `key_fields`
member — nothing hides that the original proposal needed help.

**Widening can only make a pairing more honest, never more wrong.** A stricter key turns an
unsupported `changed` guess into an `added` + `removed` pair — it can't invent a false pairing the
original (looser) key wasn't already risking. This has one real behavioral cost: for an array
whose duplicate-key elements really are the *same conceptual entry* with one mutable field (LVLI
entries sharing `(Reference, Minimum Level)` but differing `Count` is the case a regression test
covers), widening onto that mutable
field means a genuine "this entry's count changed" can no longer render as a `changed` row — it
renders as the old-count row removed and the new-count row added. There is no way to tell, from the
array data alone, whether two duplicate-key rows are "the same entity, edited" or "two different
rows that happen to collide" — `keyed_diff`'s old FIFO-within-group pairing silently assumed the
former with no evidence; widening refuses to assume either, which is why it's the safer default
even though it changes previously-observed output for a case like this.

`unkeyed_array_diff` was originally a whole-list dump (see above). It now runs the two lists
through an order-preserving alignment (longest common subsequence — `lcs_align` in `src/diff/array_diff.rs`)
and reports only the elements outside that alignment, plus an `unchanged_count` member so a reader
isn't left assuming the whole array turned over. **The alignment must stay order-preserving, not
collapse to a multiset diff:** a multiset diff would report a pure reorder as no change at all,
which is exactly wrong for an order-significant unkeyed array — CTDA `Conditions[]`, this ADR's
canonical case, or a `GetRandomPercent` cascade in a first-match list, where swapping two entries
changes behavior even
though the set of entries is unchanged. LCS keeps one copy of a moved element aligned and reports
the move as removed + added instead, so a reorder that matters stays visible. Measured on the same
run: unkeyed elements reported dropped from 1,418 to roughly 838 (a plain insertion/removal trims
to just the real delta; a reorder stays fully visible, by design, at whatever cost that adds).

This ADR's original text said "this ADR does not touch `element_key_spec`'s existing ten
heuristics — none of them changes." That line is now stale: two heuristics were added (quest
aliases, `Attacks[]` wrapper), and the old "exactly one FormID-shaped member" fallback was
generalized to "every FormID-shaped member, composed" — twelve heuristics total, and every one's
proposed key is now subject to the uniqueness check above. The original ten's field sets are
unchanged; only the final catch-all was widened, and two new wrapper/domain-specific heuristics
were added ahead of it.

**A confirmed side effect: a `GMRW` example cited early in this work as a Part-A success story was
itself a product of the bug this section fixes.**
Before uniqueness validation existed, `GMRW`'s `Rewards List[]` keyed on `Quest Reward Currency
Object` — a field mostly `null` — so two unrelated reward tiers paired FIFO-within-the-null-group
and produced a plausible-looking single-field `Conditions` diff (old index 7 against new index 0).
That looked like a clean, real gating change and was reported as one. After widening, the same
record correctly reports the two reward tiers as removed + added instead, since nothing in the data
actually links those two indices — the intended, more honest behavior, and a concrete reminder
that a diff which *looks* clean is not proof it's correctly paired. The `PiercingLove` `COBJ`
example above (`HasLearnedRecipe`) is unaffected and remains accurate.
