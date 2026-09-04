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

## Reorder-only diffs tier DEEP through `substantive_change_major_record_type`

Triage tiers by record type and changed field path, never by value. An `unkeyed` `_array_diff`
(QUST `Virtual Machine Adapter / aliases`) renders a reordered alias as one `removed` plus one
`added` entry, and a `positional` one (RACE `Bone Scale Data`, `Attacks`, VMAD `AnimationStates`
properties) as a wave of `changed` indices — both look like substantive QUST/RACE changes and the
DEEP tier fills with bundles that have no story. Those QUSTs also drag large satellite chains
into their bundles, which is what keeps them out of ROLLOUT.

**Symptom:** every DEEP bundle's only top-level path is `Virtual Machine Adapter` (or `Bone Scale
Data` / `Attacks`), and the `removed`/`added` halves carry the same script names and property
values.
**Fix, before spawning writers:** canonicalise each changed field order-insensitively (sort dict
items, sort lists of dicts, round floats) and compare `removed` vs `added` (or a live `get` on
both snapshots); bundles that are set-equal go to the Under-the-hood line, not to a writer.
**Example:** 20260814→20260821 — all 7 DEEP bundles and 43 ROLLOUT QUSTs were set-equal (50/50);
18 RACE records and the Disturbed Grave ACTI likewise. No writer was spawned.

Two more undecoded-blob shapes from the same patch, both bookkeeping: CELL `Unknown 2` is a
little-endian u64 Unix timestamp (a last-saved stamp — 1,484 cells bumped from 2026-06 to
2026-08), and ACTI/TACT/TERM `Unknown CTRN` bumps only its ID bytes.

## A schema field rename breaks downstream readers silently

A decode rename doesn't error in a consumer — it yields defaults. PCRD card data moved from
`fields['Unknown']` to `fields['Perk Card Data']` (2026-07-14); a reader still on the old name
extracts every card's Special as "Unknown" and minLevel as 0 rather than failing. That symptom is
the tell that a consumer needs the new name. Keep the old name as a fallback when migrating one.
*found 2026-07-14*

## Run the coverage gate before the narrative stage, not after

A new snapshot can introduce record types the schema has never seen. The mechanical diff/triage
stage doesn't care — it happily diffs raw-fallback bytes — so a schema gap only becomes visible
once a writer (or you) reads a decoded field and finds `_unmapped`/`_raw` where a real value
should be, by which point bundles, the mechanics KB pass, and maybe a draft are already built on
top of the gap.

**Symptom:** `esm get`/`esm chase` on an affected record type returns `_unknown_record` or
`_unmapped` keys instead of named fields; nothing upstream (diff, triage) flagged it.
**Fix:** after `create_esm_archive.sh` drops the new `Data/<date>/`, run
`esm coverage --gate` (via `FO76_ESM_PATH` or `--esm`) before starting the narrative stage. Zero
exit means proceed. Non-zero: `esm coverage` (no `--gate`) shows which SIG rows carry
`raw_fallback`/`unmapped`/`unknown_record`; stop, fix the schema gap in `esm/` (a type TES5Edit
already defines in full only needs adding to `esm/tools/extractor/extract.py`'s `SAFELIST`;
anything else is a hand-authored entry in `esm/schema/fo76.overrides.json`), and re-run the gate
until it is clean.
`--gate` checks `raw_fallback`/`unmapped`/`unknown_record` only — `unresolved` is a
missing-localization signal, not a schema gap, and never blocks it.
**Example:** 20260903 (Pets PTS): `--gate` failed with `unknown_record=7, unmapped=2380` from
three new shapes — PGTR and MSCS (new record types) and RACE `CMDE`/`PGTF`; MSCS was a `SAFELIST`
addition, PGTR and RACE needed `fo76.overrides.json` entries.
*found 2026-09-04*
