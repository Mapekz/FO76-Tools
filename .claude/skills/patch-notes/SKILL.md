---
name: patch-notes
description: >
  Weekly Fallout 76 patch-notes narrative stage, tiered edition. Resolves the latest two
  snapshots from $FO76_DATA_DIR, runs (or reuses) the deterministic diff pipeline, triages
  bundles into DEEP / BRIEF / DROP / ROLLOUT (rules + one cheap assessor agent for the
  ambiguous middle; ROLLOUT aggregates bulk form_version field churn into one line per
  change shape), fans out 1-2 Sonnet deep writers armed with the mechanics KB over the DEEP tier
  only, reconciles deferrals and unresolved chases in the orchestrator, assembles a single
  patch-summary.md, and chunks it for Discord. Use when asked to write, refresh, or re-run
  weekly patch notes.
argument-hint: "[old-snapshot] [new-snapshot] [--out-dir DIR] [--official-notes URL_OR_FILE] [--force-pipeline] [--force]"
---

You are the orchestrator for the narrative stage of the FO76 patch-notes pipeline. The
mechanical stage (diffing, bundling, linting, triage) is deterministic Python; your job is
steps 1-8 below. Run every command from the repo root — the esm crate's tools and binaries
live under `esm/` (`esm/target/release/esm`, `python3 esm/tools/<script>.py`).

## 1. Resolve inputs

Parse positional args (ignore `--out-dir`, `--official-notes`, `--force-pipeline`, `--force`,
handled below).

The snapshot listing is **only the 8-digit date directories** — `$FO76_DATA_DIR` also holds
`notes/` (this pipeline's own output), which sorts last and would otherwise be picked as the
newest snapshot:

```sh
snapshots() { ls "$FO76_DATA_DIR" | grep -E '^[0-9]{8}$' | sort; }
```

- **0 positional args** → `NEW_TOKEN`/`OLD_TOKEN` = the last two entries of `snapshots`.
- **1 positional arg** (call it `A`) → `NEW_TOKEN=A`; `OLD_TOKEN` = the snapshot immediately
  before it in `snapshots`. Fail if `A` isn't in that listing or is the first entry (no older
  snapshot to diff against).
- **2 positional args** (`A B`) → `OLD_TOKEN_OR_PATH=A`, `NEW_TOKEN_OR_PATH=B`.

Resolve each token/path to a `(TOKEN, DIR)` pair — absolute paths (`/*`) are accepted verbatim
(token = basename); anything else is a date token under `$FO76_DATA_DIR`:

```sh
resolve_snapshot() {  # $1 = token or absolute path -> echoes "TOKEN DIR"
  local x="$1"
  if [[ "$x" == /* ]]; then echo "$(basename "$x") $x"
  else echo "$x $FO76_DATA_DIR/$x"; fi
}
read -r OLD_TOKEN OLD_DIR <<< "$(resolve_snapshot "$OLD_ARG")"
read -r NEW_TOKEN NEW_DIR <<< "$(resolve_snapshot "$NEW_ARG")"
```

Fail fast, with a clear message, unless both hold:

```sh
[ -f "$OLD_DIR/SeventySix.esm" ] && [ -f "$NEW_DIR/SeventySix.esm" ]
```

`OLD_ESM="$OLD_DIR/SeventySix.esm"`, `NEW_ESM="$NEW_DIR/SeventySix.esm"`.

`OUT` = `--out-dir` value if given, else:

```sh
OUT="$FO76_DATA_DIR/notes/${OLD_TOKEN}_to_${NEW_TOKEN}"
```

If `--official-notes` was given: a URL → fetch it now (WebFetch) and save the article text to
`$OUT/work/official-notes.txt`; a file path → copy it there. This is optional input; absence
changes nothing downstream except the discrepancy callouts.

**Never write the expanded value of `$FO76_DATA_DIR` (or any absolute path) into a file under
`$OUT` — not in the summary, discord chunks, or the manifest.** Tokens (`20260626`) are fine;
full paths are not.

## 2. Run or reuse the pipeline

Build the binaries first if missing: `test -x esm/target/release/esm && test -x
esm/target/release/esm-server || (cd esm && cargo build --release --features server)` — cargo
needs to run inside the crate dir, but both binaries land in `esm/target/release/`, alongside
each other (required for daemon auto-spawn) and reachable from the repo root.

Reuse the existing pipeline output iff `$OUT/manifest.json` exists, its
`inputs.old_token`/`inputs.new_token` match `$OLD_TOKEN`/`$NEW_TOKEN`, and
`inputs.new_esm_size`/`inputs.new_esm_mtime` match a fresh `stat` of `$NEW_ESM`:

```sh
REUSE=no
if [ -f "$OUT/manifest.json" ]; then
  REUSE=$(python3 -c '
import json, os, sys
m = json.load(open(sys.argv[1])).get("inputs", {})
sz, mt = os.path.getsize(sys.argv[2]), int(os.path.getmtime(sys.argv[2]))
ok = (m.get("old_token") == sys.argv[3] and m.get("new_token") == sys.argv[4]
      and m.get("new_esm_size") == sz and m.get("new_esm_mtime") == mt)
print("yes" if ok else "no")
' "$OUT/manifest.json" "$NEW_ESM" "$OLD_TOKEN" "$NEW_TOKEN")
fi
```

If `REUSE=no` or `--force-pipeline` was passed:

```sh
python3 esm/tools/make_patch_notes.py "$OLD_DIR" "$NEW_DIR" --out-dir "$OUT"
```

**Check the strings banner it prints.** Two snapshots means two string tables, so the header
must show `strings-dir-a:` and `strings-dir-b:` pointing at *different* dirs. A single
`strings-dir:` line — or a `WARNING: --strings-dir ... BOTH sides` — means every localized
FULL/DESC on the new side is being resolved against the other snapshot's table: renames and
description rewrites silently vanish and stale text is reported as current. Stop and re-run
with `--strings-dir-a`/`--strings-dir-b` rather than writing up that diff. (Corroborating tell,
after the pipeline finishes: an `_unresolved` count in the hundreds instead of low double
digits.)

## 3. Prewarm the daemon

Before any agent work, one warm call so the index loads once, up front:

```sh
esm/target/release/esm -p --esm "$NEW_ESM" info
```

## 4. Triage

```sh
python3 esm/tools/triage_bundles.py "$OUT"
```

This writes `$OUT/work/triage.json` (tier assignment + per-bundle reasons),
`$OUT/work/deep-slice.json` (DEEP bundles in writer-slice shape),
`$OUT/work/ambiguous.json` (compact field-diff digests for the middle tier),
`$OUT/work/brief-lines.md` (templated one-liners for the BRIEF tier), and
`$OUT/work/rollouts.md` (one row per bulk data change — see below).

**The ROLLOUT tier.** When Bethesda bumps a record's `form_version` they add or drop
fields across tens of thousands of records at once. That is one story, not tens of
thousands. Triage groups every changed record by its *change shape* — `(record_type,
set of top-level changed field paths)` — and any shape recurring at least
`settings.rollout_min_records` times (default 20) tiers its bundles ROLLOUT, keeping
them out of DEEP/BRIEF/AMBIGUOUS entirely. A bundle containing any added or removed
record is never ROLLOUT: a genuinely new record is always a real story.

This is load-bearing, not cosmetic. On the 20260710→20260717 pair it took AMBIGUOUS
from 34,327 bundles to 546 — the difference between "one assessor agent" and
"impossible".

If `ambiguous.json` has entries, spawn **one assessor subagent** (Agent tool,
`model: haiku`) with this prompt shape — paste the digests inline, give it NO daemon
access and no other tools than Read/Write:

> You are triaging Fallout 76 patch-diff bundles. For each bundle below you get the actual
> field-level before/after values. Assign each a tier: `deep` (real gameplay meaning —
> stats, drops, costs, spawns, quest logic, new obtainable content, datamined features;
> a reader would want the full story), `brief` (existence is the story — a one-liner
> suffices), or `drop` (bookkeeping churn a player can never observe). When in doubt
> between drop and brief, pick brief; between brief and deep, pick deep. Write
> `<OUT>/work/assessment.json`: `{"tiers": {"<bundle_id>": {"tier": "...", "reason":
> "<one line>"}}}` covering every bundle. Reply with just the tier counts.

Then merge:

```sh
python3 esm/tools/triage_bundles.py "$OUT" --merge-assessment "$OUT/work/assessment.json"
```

Sanity-check the final tier stats in `triage.json` — if DEEP exceeds ~40 bundles or DROP
swallowed a record type you'd expect to matter (WEAP/PERK/OMOD), inspect `reasons` before
proceeding; the config (`esm/tools/patch_notes_tiers.json`) may need a rule fix, and silent
mis-tiering is exactly the failure mode this step exists to catch.

Sanity-check ROLLOUT the same way, in the opposite direction: skim `rollouts.md` and
confirm each row really is uniform bulk churn. A shape that recurs often can still
matter — "1,056 weapons gained a sneak-attack multiplier" is a headline, not noise.
Anything that reads like a gameplay change gets a line in the summary (Step 6), and if
a rollout row hides something a player would feel, pull those bundles back by raising
`rollout_min_records` and re-running this step. The tier exists to aggregate the story,
never to discard it.

## 5. Deep pass

**Resume rule:** on a plain re-run, skip straight to Step 6 if `$OUT/drafts/deep.md` is newer
than `$OUT/work/triage.json`; `--force` disables the skip.

Count DEEP bundles. **≤20** → one writer owns the whole `deep-slice.json`. **>20** → split
the slice in two by bundle (keep related bundles together; `esm/tools/triage_bundles.py`
emits them in dependency-sorted order, so a simple contiguous split is fine) and launch two
writers in one message.

For each writer, spawn a subagent (Agent tool, `model: sonnet`) with
`.claude/skills/patch-notes/deep-writer-prompt.md`, substituting:

| Placeholder | Value |
|---|---|
| `{OLD_TOKEN}` / `{NEW_TOKEN}` | snapshot tokens |
| `{SLICE_PATH}` | `$OUT/work/deep-slice.json` (or its part file) |
| `{KB_PATH}` | `.claude/skills/patch-notes/mechanics-kb.md` |
| `{OUT}` | `$OUT` |
| `{NEW_ESM}` / `{OLD_ESM}` | resolved ESM paths |
| `{STYLE_GUIDE_PATH}` | `.claude/skills/patch-notes/style-guide.md` |
| `{OTHER_SLICES}` | the other writer's slice path, or "none — you own everything DEEP" |
| `{DRAFT_PATH}` / `{REPORT_PATH}` | `$OUT/drafts/deep[.partN].{md,report.json}` |
| `{OFFICIAL_NOTES_BLOCK}` | if official notes were provided: a bullet pointing at `$OUT/work/official-notes.txt` with the instruction "cross-reference every claim: data contradicting the article → `⚠️ Mismatch (official notes):`; significant changes the article omits → `⚠️ Undocumented:`". Otherwise empty. |

## 6. Review & assemble (orchestrator — you)

Read every draft + report. Then, in order:

1. **Reconcile every deferral.** For each report's `deferred[]` entry, confirm the expected
   owner's draft actually covers those FormIDs (search the draft text). Anything uncovered:
   chase it yourself now — extract the record diff, then for `mod_Custom_*`/unique-effect
   OMODs (or a PERK/SPEL/ALCH/ENCH selector directly) run
   `esm/target/release/esm -p --esm "$NEW_ESM" chase <OMOD_OR_PERK_OR_SPEL_OR_ALCH_OR_ENCH>
   --json`; for anything else, `esm/target/release/esm -p --esm "$NEW_ESM" refs <id> --type
   <SIG> --paths --pretty` (one 4-char type per call) plus a bulk `get` for whatever it turns
   up — write the missing bullets. This step exists because deferrals DO fall through; never
   skip it.
2. **Chase every `unresolved[]` item** worth a story: resolve it live via `esm -p chase` / bulk
   `get --resolve stub` / `refs --type <SIG> --paths` (never a loop of single-selector
   `get`s), soften it to "Unconfirmed:", or cut it. Never pass one through silently.
3. **Spot-verify the 2-3 highest-impact numeric claims** per draft yourself in ONE bulk call —
   `esm/target/release/esm -p --esm "$NEW_ESM" get <id1> <id2> <id3> --resolve stub --pretty`.
4. **Merge `kb_proposals[]`** into `.claude/skills/patch-notes/mechanics-kb.md` (dedupe
   against existing entries; keep the KB's format and verified-date convention). This is the
   only file outside `$OUT` this skill may write.
5. **Assemble `$OUT/patch-summary.md`** — ONE document, sections ordered by signal:
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

## 7. Chunk for Discord

```sh
python3 esm/tools/discord_chunker.py "$OUT/patch-summary.md" "$OUT/discord"
```

If stderr warns about hard truncation (`WARNING: N chunks exceeded 2000 chars`), fix the
summary (cut prose, never numbers) and re-run.

## 8. Manifest + summary

```sh
python3 esm/tools/update_manifest.py "$OUT"
```

Print: tier counts (deep/brief/drop/rollout, plus how many the assessor promoted/demoted),
the rollout-shape count and how many records they cover, writer
count, chunk count, unresolved-chased count, KB entries added, and the output paths
(`$OUT/patch-summary.md`, `$OUT/discord/`) — no game-data paths or `$FO76_DATA_DIR`
expansions in the printed summary either.

## Guardrails

- Never assert a record's liveness from an EDID prefix alone (`zzz_`/`CUT_`/`DEL_`/`POST_`).
  For PCRD-granted perks the clean signal is a PCRD listing the rank; item-granted perks
  (OMOD/ENCH Perks property) legitimately have no PCRD — verify the grant path instead via
  `esm/target/release/esm -p --esm "$NEW_ESM" refs <perk-id> --type PCRD --paths --pretty`.
- Every number in the final summary traces to the slice, an `--extract`, or a live `esm -p`
  call this run — never memory, never estimation, never rounding.
- Every lint reaching the summary was re-verified live this run.
- DROP-tier bundles are dropped *with logged reasons* (`triage.json`); the printed summary
  states the drop count so the user can audit `work/triage.json` when something seems missing.
- ROLLOUT-tier bundles are *aggregated, never discarded*: every one is reachable from
  `triage.json`'s `rollout` list and summarised in `work/rollouts.md` with its record count
  and example FormIDs. A rollout that carries gameplay meaning must reach the summary as a
  line of its own — collapsing the row count is the point, silence is not.
- No absolute filesystem paths, ESM filenames, or `$FO76_DATA_DIR` expansions in any file
  under `$OUT`.
- This skill writes only inside `$OUT`, plus exactly one repo file:
  `.claude/skills/patch-notes/mechanics-kb.md` (KB merges in Step 6). It never modifies game
  data or anything else in the repo.
