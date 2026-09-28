# Pipeline gotchas (orchestrator only)

Ways the patch-notes pipeline itself silently reports the wrong thing or fails, with the recovery
for each. Deep writers never see this file; they get `mechanics.md` and `diff-traps.md`, which
this file points into rather than repeating. Checks the scripts already enforce live in
`SKILL.md`, not here.

Entry format: a claim as the heading, one context sentence or two, **Symptom:**, **Fix:**, one
**Example:**, and a `*found <date>*` line.

---

# Blind spots: changes the record diff never sees

## Loose-file edits are invisible to the record diff

The diff compares ESM record bytes, so a changed string-table entry behind an unchanged lstring ID,
or changed curve points behind an unchanged CURV reference, never enters the changed set. Pet XP,
mutation scaling, legendary magnitudes and most text rewrites live in those files.

**Symptom:** an official note describes a rebalance or rewording and no record in the diff moves; a
Mismatch flag is about to say the data has no counterpart.
**Fix (every patch, before any "no data counterpart" flag):**
1. `diff -rq Data/<old>/misc/curvetables Data/<new>/misc/curvetables`, then compare each changed
   file's `curve` points. Added files belong with the records that reference them.
2. Parse `strings/SeventySix_en.{strings,dlstrings,ilstrings}` on both sides and set-diff by string
   ID. Header `<II` (count, dataSize), then count × `<II` (stringID, offset), then data;
   `.strings` entries are NUL-terminated, `.dlstrings`/`.ilstrings` u32-length-prefixed. Both
   snapshots ship current tables whether or not they are `Localized`, so this stays valid across a
   flip.

**Example:** 20260903→20260914, `worldpets/worldpets_petxp_02.json` ×12 at all 42 points (level 100:
2,900 → 34,800 XP) was the official "Re-balanced Pet XP levels". 20260717→20260724, OMOD
`mod_Custom_Xerxos` (0x008F173D) "Emits Radiation" → "Emits Radiation at 6 RAD/s" was string-only.
*found 2026-07-24; curve tables 2026-09-14*

## ROLLOUT tiering is blind to values on plumbing fields

A change shape is `(record_type, set of changed field paths)`. Triage keeps records with a numeric
delta on a non-plumbing field out of ROLLOUT (`settings.rollout_numeric_exclusion`, the "Kept out
(numeric)" column of `rollouts.md`), but plumbing-pattern fields are still tiered by shape alone, so
a real edit there can hide among schema churn on the same field.

**Symptom:** a `rollouts.md` row reads as editor bookkeeping while its values actually move.
**Fix:** scan values, not shapes: for every rollout record, flag each ChangeEntry where neither side
is null/empty and the sides still differ after Unicode-NFC and whitespace normalization.
**Example:** 20260710→20260717, 100,705 entries → ~6,900 candidates, mostly `Enlighten Auto UV /
Padding?`, which surfaced 353 OMODs that lost their `Attribute Descriptor Keywords`.
*found 2026-07-22*

## The Localized header flag flips every few months

`manifest.json` records each side's TES4 `Localized` flag under `inputs.localized`, and the banner
prints `Localized flag flips` when they differ. Text decodes the same either way, so nothing needs
skipping; what a writer should know is in `diff-traps.md`'s Localized entry.

**Symptom:** `comprehensive.json`'s `meta.suppressed_counts` carries `localization_flip_text`: string
leaves differing only by the tables' NBSP/CRLF rewrites were dropped. Expected, no action.
**Example:** 20260821 → 20260903 (`true` → `false`) dropped 53 leaves; 244,420 of 244,484 inline
strings matched that snapshot's own table exactly.
*found 2026-09-15*

---

# Tiering noise: phantom changes that reach DEEP

## Reorder-only diffs tier DEEP through `substantive_change_major_record_type`

Triage tiers by record type and field path, never by value. An `unkeyed` `_array_diff` (QUST
`Virtual Machine Adapter / aliases`) renders a reordered element as one `removed` plus one `added`,
and a `positional` one (RACE `Bone Scale Data`, `Attacks`, VMAD `AnimationStates`) as a wave of
`changed` indices. Both read as substantive, and the QUSTs' satellite chains keep them out of
ROLLOUT.

**Symptom:** every DEEP bundle's only top-level path is `Virtual Machine Adapter`, `Bone Scale
Data` or `Attacks`, with the same names and values on both halves.
**Fix, before spawning writers:** apply `diff-traps.md`'s permutation test to each bundle
(canonicalize order-insensitively: sort dict items and lists of dicts, round floats); set-equal
bundles go to the Under-the-hood line, not to a writer.
**Example:** 20260814→20260821, all 7 DEEP bundles, 43 ROLLOUT QUSTs, 18 RACE records and the
Disturbed Grave ACTI were set-equal. No writer was spawned.
*found 2026-08-28*

## An array keyed on an unstable field reports as wholly rewritten

`array_diff` pairs elements by a key derived from their shape (`esm/src/diff/array_diff.rs`,
`element_key_spec`), widening onto extra scalar leaves when the key isn't unique. It discards a
proposed key that is null on every element, and VMAD aliases decode their real id, but a key
component a new build renumbers still gives each element a different key per side, so every
element reports `removed` plus `added` and the bundle tiers DEEP.

**Symptom:** an `_array_diff` with `count_from == count_to`, `unchanged_count` at or near 0, and a
`key_fields` list naming serializer bookkeeping (`Rank`, `* Tab Count`, `* Index`, a version
counter).
**Fix:** confirm with two `esm get` calls whether elements really turned over. If not, narrow
`element_key_spec` for that shape in `esm/` rather than writing up the phantom rewrite.
**Example:** 20260821→20260903, PERK `Effects` keyed on a widened key including `Effect
Header.Rank` reported 860 added/removed effect entries across 485 perks; 72 after the key fix.
*found 2026-09-14*

## A header-version bump (branch switch) fakes tens of thousands of changes

When the snapshots come from different editor builds (TES4 header `Version` differs), the newer
build re-serializes most records.

**Symptom:** ROLLOUT > 50K bundles, DEEP > 300 after rules, and `esm info` shows different
`Version` lines for the two ESMs.
**Fix:**
1. Value-level scan with a per-leaf multiset check, so positional reorders (SCOL parts, MSWP/MDSP
   swap lists, ARMA sculpt, VMAD fragments) cancel out.
2. Drop the signatures in `diff-traps.md`'s "Cross-build pairs carry fixed re-serialization
   signatures".
3. Branch-drift check: a `--bodies none` diff of two *older* snapshots against the new one (~20 s
   each); a candidate absent from those diffs is the new snapshot merely equalling an older value.
4. Hand writers a curated slice (`work/deep-slice.<topic>.json`, same shape):
   `--merge-assessment` re-tiers only AMBIGUOUS bundles and cannot demote rule-DEEP.

**Example:** 20260821 (Slasher PTS, v279) → 20260903 (Pets PTS, v283): 66,171 changed records,
54,884 ROLLOUT bundles, 332 rule-DEEP → 81 curated bundles for two writers.
*found 2026-09-04*

## A BRIEF "added" line can be an enemy-only copy of an existing item

`brief-lines.md` templates every added record as `**<name>** (<TYPE>): added`, with no EditorID. A
duplicate made for NPCs carries the original's display name, so it reads as new player apparel.

**Symptom:** a BRIEF "added" ARMO/WEAP whose name is an item players already own.
**Fix:** before writing "New apparel/weapon" from BRIEF lines, `esm search "<name>"`; an EditorID
ending `_NONPLAYABLE`, or `refs` showing only creature/NPC outfit lists, makes it an enemy-only copy.
**Example:** `Clothes_LostHeavyArmourBurnt01_Storm_NONPLAYABLE` (0x0095167D) "Burnt Vault 63 Riot
Control Outfit" copies 0x0076D18B and is referenced only by `HIDE_crLLI_Outfit_LostWildcard`
(0x008F4E83).
*found 2026-09-20*

---

# Failures: runs that break or poison downstream work

## A worldspace rework makes bundling run out of memory on REFR placements

Bundling runs one reverse-reference search per diff record, serially, and keeps every edge in
memory. A landscaping pass adds or moves hundreds of thousands of REFRs, so bundling grows without
bound while the diff itself finishes in seconds.

**Symptom:** 100K+ REFR entries in the diff, no `bundles.json`, and `make_patch_notes.py` growing
~350 MB a minute while its `esm batch` child sits near 100% CPU.
**Fix:** run `make_patch_notes.py ... --exclude-type LAND,NAVM,REFR`. Summarize placements
separately from a REFR-only diff (`esm diff OLD NEW --json --bodies stub --type REFR`) as
counts by base object plus placements of newly added base objects.
**Example:** 20260903→20260914, 225K of 281K diff records were REFR (Skyline Valley rework);
bundling passed 11 GB in 25 minutes. Without REFR: 22K records, mechanical stage in 270 s; the
REFR-only diff showed 150 placements of the new `RTSV_SQ01_BrainInJar` collectible.
*found 2026-09-14*

## Run the coverage gate before the narrative stage

A new snapshot can add record types or fields the schema has never seen. Diff and triage work on
raw-fallback bytes without complaint, so the gap surfaces only when someone reads `_unmapped`/`_raw`
in a decoded record, after bundles and drafts are built on it.

**Symptom:** `esm get`/`esm chase` returns `_unknown_record` or `_unmapped` keys; nothing upstream
flagged it.
**Fix:** after the new `Data/<date>/` lands, run `esm coverage --gate` (via `FO76_ESM_PATH` or
`--esm`); it walks every record (~2 min). Non-zero: `esm coverage` shows which SIG rows carry `raw_fallback`/`unmapped`/
`unknown_record`; fix the schema in `esm/` (a type TES5Edit defines in full goes in
`esm/tools/extractor/extract.py`'s `SAFELIST`; anything else is an entry in
`esm/schema/fo76.overrides.json`) and re-run until clean. `unresolved` counts are missing
localization, not schema gaps, and never block the gate.
**Example:** 20260903 (Pets PTS): `unknown_record=7, unmapped=2380` from PGTR and MSCS (new types)
and RACE `CMDE`/`PGTF`; MSCS needed a `SAFELIST` addition, the rest `fo76.overrides.json` entries.
*found 2026-09-04*

## A schema field rename breaks downstream readers silently

A decode rename doesn't error in a consumer; the reader gets defaults. Keep the old name as a
fallback when migrating a reader.

**Symptom:** a tool reports every record with the same default value ("Unknown", 0).
**Fix:** re-dump one known record, grep the actual field names, and update the reader.
**Example:** PCRD card data moved from `fields['Unknown']` to `fields['Perk Card Data']`; readers
still on the old name reported every card's Special as "Unknown" and minLevel as 0.
*found 2026-07-14*
