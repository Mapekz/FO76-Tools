# Pipeline gotchas (orchestrator only)

Failure modes of the patch-notes pipeline itself — blind spots where the diff silently reports the
wrong thing, and the recovery step for each. **Not handed to deep writers**: these are checks the
orchestrator runs while driving `/patch-notes`. Game mechanics live in `mechanics.md`, diff-reading
traps in `diff-traps.md`.

Same entry format: a claim as the heading, the symptom, the fix, one worked example.

---

## Per-snapshot string tables must be matched per side

Both FO76 snapshots name their ESM `SeventySix.esm`, so a strings directory belonging to snapshot A
satisfies a "does this dir have string files for token X" check for **both** sides. When that
happens the newer snapshot's `FULL`/`DESC` lstring IDs resolve against the **older** snapshot's
table, which silently hides every localized text change and reports stale values as current.

**Symptom:** a rename you can see live via two `esm get` calls is absent from the diff, and the
diff's `_unresolved` count is in the hundreds instead of low double digits (192 vs 12 on
20260710→20260717).
**Fix:** always pass `--strings-dir-a`/`--strings-dir-b` per side, or let the daemon auto-detect
per ESM. The pipeline's banner must show two *different* dirs — a single `strings-dir:` line, or a
`WARNING: --strings-dir ... BOTH sides`, means stop and re-run.
*found 2026-07-22*

## String-table-only text edits are invisible to the record diff

A localized text field can change with **no record change at all**: the record keeps the same
lstring ID and Bethesda edits the string-table entry it points at. The record-level diff compares
record bytes, so such a record never enters the changed set and its text delta is never reported.
Only records that ALSO changed structurally show their text delta.

This is a *different* blind spot from the per-side-strings-dir gotcha above — that one hides
changes on records that DID change; this one hides records that did not.

**Recovery (run every patch as a second pass):** parse
`strings/SeventySix_en.{strings,dlstrings,ilstrings}` for both snapshots and set-diff by string ID.
Format: header `<II` (count, dataSize), then count × `<II` (stringID, offset), then the data block
— `.strings` entries are NUL-terminated, `.dlstrings`/`.ilstrings` are u32-length-prefixed.

**Example:** on 20260717→20260724 this surfaced 4 changed + 5 added dlstrings and 26 changed + 79
added + 4 removed strings (`.ilstrings` had zero), including OMOD `mod_Custom_Xerxos` (0x008F173D)
Description "Emits Radiation" → "Emits Radiation at 6 RAD/s" — matching its ENCH magnitude 3.0 →
6.0, and absent from `comprehensive.json` entirely. Also the Cyberdog "Generates 1, 2, or 3 Star
Legendary Items" claim, the Ghost Boy invisibility rewording, every Slasher rename, and the whole
My Stats terminal expansion.
*found 2026-07-24*

## ROLLOUT shapes are blind to values

A change shape is `(record_type, set of changed field paths)` — it never looks at the before/after
**values**. So a genuine `3.4028235e+38 → 100.0` edit has the same shape as the `null →
3.4028235e+38` schema churn around it, and a real balance change can be tiered ROLLOUT purely
because ≥20 other records touched the same field.

The mandated ROLLOUT sanity check is therefore a **value-level scan**, not a skim of
`rollouts.md`: for every rollout record, flag any ChangeEntry where neither side is null/empty and
the two sides still differ after Unicode-NFC and whitespace normalization.

**Example:** on 20260710→20260717 that reduced 100,705 entries to ~6,900 candidates — nearly all
`Model / Enlighten Auto UV / Padding?` garbage-to-zeros, but it is what surfaced the 353 OMODs that
silently lost their `Attribute Descriptor Keywords`, which a shape skim had called "editor
bookkeeping".
*found 2026-07-22*

## A schema field rename breaks downstream readers silently

A decode rename doesn't error in a consumer — it yields defaults. PCRD card data moved from
`fields['Unknown']` to `fields['Perk Card Data']` (2026-07-14); a reader still on the old name
extracts every card's Special as "Unknown" and minLevel as 0 rather than failing. That symptom is
the tell that a consumer needs the new name. Keep the old name as a fallback when migrating one.
*found 2026-07-14*
