---
name: patch-notes
description: >
  Weekly Fallout 76 patch-notes narrative stage, tiered edition. Resolves the latest two
  snapshots from $FO76_DATA_DIR, runs (or reuses) the deterministic diff pipeline, triages
  bundles into DEEP / BRIEF / DROP / ROLLOUT (rules + one assessor agent for the
  ambiguous middle; ROLLOUT aggregates bulk form_version field churn into one line per
  change shape), fans out 1-2 deep writers armed with the mechanics KB over the DEEP tier
  only, gates every drafted number and every DEEP story, reconciles deferrals in the
  orchestrator, assembles a single patch-summary.md, has it cold-reviewed in a fresh
  context, and chunks it for Discord. Use when asked to write, refresh, or re-run weekly
  patch notes.
---

Arguments: `[old-snapshot] [new-snapshot] [--out-dir DIR] [--official-notes URL_OR_FILE] [--force-pipeline] [--force]`.

You are the orchestrator for the narrative stage of the FO76 patch-notes pipeline. The
mechanical stage (diffing, bundling, linting, triage) is deterministic Python; your job is
steps 1-7 below. Run every command from the repo root. The pipeline is
`python3 patch-notes/cli.py <verb>` (below, `pn <verb>`); `prepare`, `merge-assessment`,
`gate` and `publish` each print a JSON summary on stdout. The `esm` binary is
`target/release/esm`; build it first, which is a no-op when it is current:
`cargo build --release -p esm`.

Use the client's available delegation capability for the roles below; tool names
are not part of this procedure. If delegation is unavailable, perform triage and
drafting locally with the same input/output contracts, and report independent
review as unavailable rather than presenting self-review as a fresh review.

**Read `patch-notes/skill/kb/pipeline-gotchas.md` before Step 1.** It catalogues the
ways this pipeline silently reports the wrong thing or fails (diff blind spots, tiering noise,
memory and schema failures) and the recovery step for each. It is orchestrator-only — the deep
writers get `kb/mechanics.md` and `kb/diff-traps.md` instead.

**Any test, sanity-check, or partial run uses a scratch `--out-dir`** (e.g.
`/tmp/pn-<OLD_TOKEN>_to_<NEW_TOKEN>`), and a subagent asked to run any pipeline command gets
that scratch path in its brief. The pipeline refuses to overwrite an out-dir whose manifest
records a finished narrative stage unless `--force-pipeline` is passed.

**Never write the expanded value of `$FO76_DATA_DIR` (or any absolute path) into a file under
`$OUT`** — not in the summary, discord chunks, or the manifest. Tokens (`20260626`) are fine;
full paths are not. The verbs print absolute paths only to stdout.

## 1. Prepare

```sh
python3 patch-notes/cli.py prepare [OLD] [NEW] [--out-dir DIR] [--official-notes URL_OR_FILE] [--force-pipeline]
```

`--exclude-type LIST` (default `LAND,NAVM`) sets the record types the diff omits; see
`kb/pipeline-gotchas.md` for when to add `REFR`.

Pass the positional arguments through: none means the newest two snapshots under
`$FO76_DATA_DIR`, one names NEW (OLD is the snapshot before it), two name both; each is a
date token or an absolute snapshot directory. The out-dir defaults to
`$FO76_DATA_DIR/notes/<OLD>_to_<NEW>`. Pass `--force-pipeline` only when the user asked for it.

`prepare` reuses the out-dir's mechanical output when its manifest matches the two tokens,
the NEW ESM's size and mtime, the excluded types, and the pipeline version; otherwise it runs
the pipeline. It then builds the new snapshot's `esm` cache so no writer's first query waits
on it, triages (reused output keeps its triage, merged assessment included, unless
`--retriage`), and slices the DEEP tier for the writers. From its JSON, take `out_dir`
(`OUT`), `old`/`new` (`token`, `esm`: `OLD_ESM`/`NEW_ESM`), `tiers`, `deep_slices`,
`official_notes`, and `warnings`.

`official_notes` reports the `--official-notes` input: `fetched`/`copied` (in
`$OUT/work/official-notes.txt`), `client_rendered` (use an available browser or page-fetch
tool with the prompt "Return as plain text, without summarizing, only the part of the article
before the first horizontal rule; stop at the first `<hr>`", and save that to the same file),
`failed`, or `none`. Only the newest section matters: Bethesda's "Inside the Vault" PTS pages
stack several weeks on one page and a summarizing fetch blends them. The notes are optional;
their absence changes nothing downstream except the discrepancy callouts.

A `Localized flag flips` line from the pipeline is expected every few months and needs no
action; see `kb/pipeline-gotchas.md`.

**The ROLLOUT tier.** When Bethesda bumps a record's `form_version` they add or drop
fields across tens of thousands of records at once. That is one story, not tens of
thousands. Triage groups every changed record by its *change shape* — `(record_type,
set of top-level changed field paths)` — and any shape recurring at least
`settings.rollout_min_records` times (default 20) tiers its bundles ROLLOUT, keeping
them out of DEEP/BRIEF/AMBIGUOUS entirely. A bundle containing any added or removed
record is never ROLLOUT: a genuinely new record is always a real story. Triage keeps any
changed record with a real numeric delta on a non-plumbing field out of ROLLOUT
(`stats.rollout_numeric_excluded` in `triage.json`, the "Kept out (numeric)" column in
`rollouts.md`), so a genuine balance change hiding inside a bulk shape tiers on its own.

## 2. Assess the ambiguous tier

If `tiers.ambiguous` is non-zero, spawn **one assessor subagent** pointed at
`$OUT/work/ambiguous.json` — restrict it to reading that input and writing the assessment, with
no `esm` access:

> You are triaging Fallout 76 patch-diff bundles. Read `<OUT>/work/ambiguous.json`; for each
> bundle you get the actual field-level before/after values. Assign each a tier: `deep` (real gameplay meaning —
> stats, drops, costs, spawns, quest logic, new obtainable content, datamined features;
> a reader would want the full story), `brief` (existence is the story — a one-liner
> suffices), or `drop` (bookkeeping churn a player can never observe). When in doubt
> between drop and brief, pick brief; between brief and deep, pick deep. Write
> `<OUT>/work/assessment.json`: `{"tiers": {"<bundle_id>": {"tier": "deep|brief|drop",
> "reason": "<one line>"}}}` covering every bundle, no other keys. Reply with just the tier
> counts.

Then:

```sh
python3 patch-notes/cli.py merge-assessment "$OUT"
```

It rejects a malformed assessment (a tier outside deep/brief/drop, a missing reason, an
unknown key) with the JSON path at fault; have the assessor fix it. A digest that hit the size
cap is flagged `truncated`; the merge promotes a `drop` verdict on those to `brief` (a partial
view may shorten a story, never erase it). Its JSON gives the new `tiers` and `deep_slices`.
Record the assessor's token usage for Step 6 only if the client reports it.

**Check the tiers before any writer runs.** `warnings` flags a DEEP tier over 40 bundles and
a DROP tier holding WEAP/PERK/OMOD records: inspect those bundles' `reasons` in
`work/triage.json`; the config (`patch-notes/pn/patch_notes_tiers.json`) may need a rule fix.
Silent mis-tiering is the failure mode this step exists to catch. Check ROLLOUT the opposite
way: skim `work/rollouts.md` and confirm each row really is uniform bulk churn. A shape that
recurs often can still matter — "1,056 weapons gained a sneak-attack multiplier" is a
headline, not noise. Anything that reads like a gameplay change gets a line in the summary
(Step 4); if a rollout row hides something a player would feel, raise `rollout_min_records`
and re-run `prepare --retriage` (then Step 2 again). The tier exists to aggregate the story,
never to discard it.

## 3. Deep pass

**Resume rule:** on a plain re-run, skip straight to Step 4 if every slice's draft
(`$OUT/drafts/deep.md`, or `deep.partN.md` for each part) is newer than
`$OUT/work/triage.json`; `--force` disables the skip.

Spawn one writer per file in `deep_slices`: one slice (up to 20 DEEP bundles) gets one writer;
two slices (`deep-slice.part1.json`, `deep-slice.part2.json`, contiguous halves of the tier
in bundle-id order, so related bundles can land in different halves; the writers defer to
each other through `{OTHER_SLICES}`) get two writers, launched concurrently. Each gets
`patch-notes/skill/deep-writer-prompt.md`, substituting:

| Placeholder | Value |
|---|---|
| `{OLD_TOKEN}` / `{NEW_TOKEN}` | snapshot tokens |
| `{SLICE_PATH}` | its slice file |
| `{MECHANICS_KB}` | `patch-notes/skill/kb/mechanics.md` |
| `{TRAPS_KB}` | `patch-notes/skill/kb/diff-traps.md` |
| `{OUT}` | `$OUT` |
| `{NEW_ESM}` / `{OLD_ESM}` | the ESM paths from `prepare` |
| `{STYLE_GUIDE_PATH}` | `patch-notes/skill/style-guide.md` |
| `{OTHER_SLICES}` | the other writer's slice path, or "none — you own everything DEEP" |
| `{DRAFT_PATH}` / `{REPORT_PATH}` | `$OUT/drafts/deep.{md,report.json}`, or `deep.partN.{md,report.json}` for part N |
| `{OFFICIAL_NOTES_BLOCK}` | if official notes were provided: a bullet pointing at `$OUT/work/official-notes.txt` with the instruction "cross-reference every claim: data contradicting the article → `⚠️ Mismatch (official notes):`; significant changes the article omits → `⚠️ Undocumented:`". Otherwise empty. |

Record each writer's token usage for Step 6 only if the client reports it.

### Gate — run before reading a single draft

```sh
python3 patch-notes/cli.py gate "$OUT" --old-esm "$OLD_ESM" --new-esm "$NEW_ESM"
```

It re-derives every `claims[]` entry from `comprehensive.json` (and live `esm` lookups for
values outside the diff), and checks every DEEP bundle id is covered by exactly one draft (or
deferred to one) that names it. A `mismatch` is a wrong number; an `unverifiable` is a number
nobody can stand behind (details in `work/claims-check.json`). Send each back to its writer
with the detail line, or chase it yourself and fix the draft AND its claim. A report with zero
claims means the writer prompt drifted — re-dispatch that writer. A report that breaks the
writer contract (a missing or unknown key, a malformed claim) counts as invalid, with the JSON
path at fault: fix the file or re-dispatch its writer. The gate may not be skipped or
overridden; loop until it exits 0.

## 4. Review & assemble (orchestrator — you)

Read every draft + report. Then, in order:

1. **Reconcile every deferral the gate flagged** (`deferred_uncovered`) and every
   `unresolved[]` item worth a story: chase it yourself now — extract the record diff, then
   for `mod_Custom_*`/unique-effect OMODs (or a PERK/SPEL/ALCH/ENCH selector directly) run
   `target/release/esm --esm "$NEW_ESM" chase <OMOD_OR_PERK_OR_SPEL_OR_ALCH_OR_ENCH>
   --json`; for anything else, `target/release/esm --esm "$NEW_ESM" refs <id> --type
   <SIG> --paths --pretty` (one 4-char type per call) plus a bulk `get` for whatever it turns
   up. Write the missing bullets into `$OUT/drafts/deep.orchestrator.md` with a matching
   `$OUT/drafts/deep.orchestrator.report.json` (`bundles_covered` + `claims`, same contract as
   the writers) so the gate covers your additions too; soften what you cannot resolve to
   "Unconfirmed:", or cut it. Never pass one through silently.
2. **Re-run the gate** after any edit to a draft or report.
3. **Merge `kb_proposals[]`** into the KB, routing by each proposal's `kind`: `mechanic` →
   `patch-notes/skill/kb/mechanics.md`, `trap` →
   `patch-notes/skill/kb/diff-traps.md`. These are the only files outside `$OUT` this
   skill may write. Before appending, enforce `mechanics.md`'s entry format yourself — writers
   drift and the KB is re-read whole by every future run:
   - **Rewrite, don't paste** into that format; strip all history — how it was found, what was
     believed first, when a tool or schema was fixed, which run it came from. None of it changes
     what a future writer does.
   - **Merge into the existing entry** when one already covers the topic (the writer may flag
     this as `refines: <heading>`). Never append a near-duplicate; a KB with two entries on one
     mechanic is worse than one stale entry.
   - Anything about the *pipeline itself* failing (diff blind spots, tiering artifacts, string
     resolution) belongs in `kb/pipeline-gotchas.md` — write it there yourself, not into the
     writer-facing files.
4. **Assemble `$OUT/patch-summary.md`** — ONE document, sections ordered by signal:
   `# FO76 Datamine — Patch <date>` / `## TL;DR` (≤6 bullets) / unique & legendary changes /
   balance / events & quests / new items / `## Datamined: <feature> (not live)` (standing
   disclaimer first) / `## Cut / Vaulted` / then append the BRIEF one-liners from
   `work/brief-lines.md` under `## Also this patch` (prune any line a deep section already
   covers) / and finally `## Under the hood` distilled from `work/rollouts.md` — at most a
   handful of lines, each naming the record type, the field, and the record count
   ("1,056 weapons gained a per-weapon sneak-attack multiplier"). Promote a rollout row
   into a real section above if it has gameplay meaning; drop rows that are purely
   structural (padding, model relinks, editor bookkeeping). Never paste the table
   wholesale. Style guide applies throughout; cut prose over numbers when over budget.
   Every DEEP story you leave out of the summary goes into `$OUT/work/cuts.json` as
   `{"cuts": [{"bundle_id": "B0123", "reason": "<why>"}]}` — then:

   ```sh
   python3 patch-notes/cli.py gate "$OUT" --old-esm "$OLD_ESM" --new-esm "$NEW_ESM" --summary
   ```

   Loop until it exits 0: a DEEP anchor absent from the summary and absent from `cuts.json`
   is a dropped story.

## 5. Cold review (one read-only subagent)

Spawn one subagent with `patch-notes/skill/review-prompt.md`,
substituting `{OUT}`, `{OLD_ESM}`, `{NEW_ESM}`. It reads only the artifacts — never your
reasoning — and writes `$OUT/work/review.json`. Then: fix every `high` finding in the summary
and in the draft + claim it came from; decide `med` on merit; ignore `low`. Re-run
the gate with `--summary` after the fixes. Record the reviewer's token usage for Step 6 only if
the client reports it.

## 6. Publish

Write `work/usage.json` only from usage the client actually reports. Its shape is
`{"assessor": {"tokens": N}, "writers": [{"tokens": N}], "reviewer": {"tokens": N}}`.
Omit roles that did not run or whose usage is unavailable; do not use zero as a
placeholder. If none is reported, omit the file. On reruns, replace or remove the
previous usage file so old counts cannot be attributed to this run. Then:

```sh
python3 patch-notes/cli.py publish "$OUT"
```

It refuses to run without `work/review.json`; when no independent reviewer was available,
pass `--no-review` and say so in the report.

It validates `work/review.json` and `work/usage.json`, chunks the summary into
`$OUT/discord/`, and records the narrative stage in the manifest. It exits 1 when any chunk
had to be hard-truncated (content lost): fix the summary (cut prose, never numbers) and re-run
until it exits 0. `--allow-oversize` exists only for a truncation you knowingly accept — say
so in the printed summary if you use it.

## 7. Report

Print: tier counts (deep/brief/drop/rollout, how many the assessor resolved, how many
numeric records were kept out of rollouts), the rollout-shape count and how many records they
cover, writer count, claims checked and how many came back as mismatch/unverifiable before
fixes, DEEP coverage (covered / cut), review findings by severity, chunk count, KB entries
added, total reported subagent tokens and any missing usage, and the output paths (`$OUT/patch-summary.md`,
`$OUT/discord/`) — no game-data paths or `$FO76_DATA_DIR` expansions in the printed summary
either.

## Guardrails

- Never assert a record's liveness from an EDID prefix alone (`zzz_`/`CUT_`/`DEL_`/`POST_`).
  For PCRD-granted perks the clean signal is a PCRD listing the rank; item-granted perks
  (OMOD/ENCH Perks property) legitimately have no PCRD — verify the grant path instead via
  `target/release/esm --esm "$NEW_ESM" refs <perk-id> --type PCRD --paths --pretty`.
- Every number in the final summary traces to the slice, a `pn extract`, or a live `esm`
  call this run — never memory, never estimation, never rounding — and is a `claims[]` entry
  the gate verified this run.
- Every DEEP bundle reaches the summary or `work/cuts.json` with a reason
  (`gate --summary` exits 0).
- Every lint reaching the summary was re-verified live this run.
- DROP-tier bundles are dropped *with logged reasons* (`triage.json`); the printed summary
  states the drop count so the user can audit `work/triage.json` when something seems missing.
- ROLLOUT-tier bundles are *aggregated, never discarded*: every one is reachable from
  `triage.json`'s `rollout` list and summarised in `work/rollouts.md` with its record count
  and example FormIDs. A rollout that carries gameplay meaning must reach the summary as a
  line of its own — collapsing the row count is the point, silence is not.
- No absolute filesystem paths, ESM filenames, or `$FO76_DATA_DIR` expansions in any file
  under `$OUT`.
- This skill writes only inside `$OUT`, plus the KB files under
  `patch-notes/skill/kb/` (merges in Step 4). It never modifies game data or anything
  else in the repo.
