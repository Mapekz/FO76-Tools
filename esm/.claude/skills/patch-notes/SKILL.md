---
name: patch-notes
description: >
  Weekly Fallout 76 patch-notes narrative stage. Resolves the latest two snapshots
  from $FO76_DATA_DIR, runs (or reuses) the deterministic diff pipeline, fans out one
  Sonnet writer subagent per category over sliced bundles, verifies claims against the
  warm esm daemon, reviews and assembles player-facing notes per category, chunks them
  for Discord, and updates the manifest. Use when asked to write, refresh, or re-run
  weekly patch notes.
argument-hint: "[old-snapshot] [new-snapshot] [--out-dir DIR] [--category NAME]... [--force-pipeline] [--force]"
---

You are the orchestrator for the narrative stage of the FO76 patch-notes pipeline. The
mechanical stage (diffing, bundling, linting) is already deterministic Python; your job is
steps 1-8 below. Run every command from the `esm/` repo root.

## 1. Resolve inputs

Parse positional args (ignore `--out-dir`, `--category`, `--force-pipeline`, `--force`, handled
below).

- **0 positional args** → `NEW_TOKEN`/`OLD_TOKEN` = the last two entries of
  `ls "$FO76_DATA_DIR" | sort`.
- **1 positional arg** (call it `A`) → `NEW_TOKEN=A`; `OLD_TOKEN` = the snapshot immediately
  before it in `ls "$FO76_DATA_DIR" | sort`. Fail if `A` isn't in that listing or is the first
  entry (no older snapshot to diff against).
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

**Never write the expanded value of `$FO76_DATA_DIR` (or any absolute path) into a file under
`$OUT` — not in drafts, notes, discord chunks, or the manifest.** Tokens (`20260626`) are fine;
full paths are not.

## 2. Run or reuse the pipeline

Build the binaries first if missing: `test -x target/release/esm && test -x
target/release/esm-server || cargo build --release --features server`.

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

If `REUSE=no` or `--force-pipeline` was passed, run the pipeline fresh:

```sh
python3 tools/make_patch_notes.py "$OLD_DIR" "$NEW_DIR" --out-dir "$OUT"
```

This writes `diff.json`, `comprehensive.{md,json}`, `bundles.json`, `lints.json`,
`manifest.json` into `$OUT`.

## 3. Prewarm the daemon

Before fanning out any writers, make one warm call so the index is loaded once, up front,
instead of N writers racing a cold spawn:

```sh
target/release/esm -p info "$NEW_ESM"
```

The spawn-lock makes concurrent autostart safe, but a cold index takes minutes — always do this
serially first.

## 4. Slice

```sh
python3 tools/slice_bundles.py "$OUT"
```

Read `$OUT/work/categories.json` — an ordered list of `{id, label, slug, slices, bundle_ids,
bundle_count, bug_watch_count, bytes, post_order}`. `slices` is a list of one or more
`work/bundles.<slug>[.partN].json` paths (a category is split into parts only when its combined
slice is oversized — a single bundle is never split across parts).

If `--category NAME` was given (repeatable), keep only categories whose `slug` matches (or a
supplied `NAME`) case-insensitively. If a given name matches nothing, error out listing every
available slug from `categories.json`.

## 5. Fan out writers

**Resume rule:** on a plain re-run, skip any category whose `$OUT/notes/<slug>.md` is newer than
`$OUT/bundles.json` (it's already fresh) — go straight to including it in the final summary.
`--force` disables this skip for all selected categories. Drafts already on disk are never
deleted, only overwritten by the writer that regenerates them.

For every remaining category, for every part in its `slices` list, launch one
`patch-notes-writer` subagent (via the Agent tool, `subagent_type: "patch-notes-writer"`). Build
the prompt by taking `.claude/skills/patch-notes/writer-prompt.md` and substituting:

| Placeholder | Value |
|---|---|
| `{CATEGORY}` | category `id` |
| `{CATEGORY_LABEL}` | category `label` |
| `{SLICE_PATH}` | this part's slice file, e.g. `$OUT/work/bundles.<slug>.json` |
| `{OUT}` | `$OUT` |
| `{NEW_ESM}` | `$NEW_ESM` |
| `{OLD_ESM}` | `$OLD_ESM` |
| `{STYLE_GUIDE_PATH}` | `.claude/skills/patch-notes/style-guide.md` |
| `{DRAFT_PATH}` | `$OUT/drafts/<slug>.md` (single-part) or `$OUT/drafts/<slug>.partN.md` (multi-part) |
| `{REPORT_PATH}` | `$OUT/drafts/<slug>.report.json` (single-part) or `$OUT/drafts/<slug>.partN.report.json` (multi-part) |

Multi-part categories get one writer *per part*, each with its own draft/report — Step 6 merges
them before writing the single `notes/<slug>.md`.

Cap **6 concurrent writers**. Put all Agent calls for one batch in a single assistant message so
they run in parallel; if the total (category × part) count exceeds 6, split into sequential
batches and wait for each batch to fully return before starting the next.

## 6. Review & assemble (main agent, per category)

For each category, read every draft and report file it produced (all parts). Then:

- **Cross-category dedup**: for each `cross_category_formids` entry in a report, keep the full
  story in the FormID's own anchor category; reduce other mentions to a one-line cross-reference.
- **Tone/structure normalization** against `.claude/skills/patch-notes/style-guide.md` — section
  order, voice, length budgets.
- **Independently spot-verify** the 2-3 highest-impact numeric claims per category yourself, via
  `target/release/esm -p get "$NEW_ESM" <id> --resolve stub --pretty` (never `--local`).
- Every claim a report lists as `unverified_claims`: verify it now, soften to "Unconfirmed:", or
  cut it — never pass it through silently.
- **Audit**: no `POST_`/`zzz_`-prefixed content appears outside "Datamined / coming soon" /
  "Vaulted / cut this patch"; every Bug Watch entry has an Evidence line (record type,
  EditorID/FormID, field, observed value).

Write the merged, reviewed result to `$OUT/notes/<slug>.md` (one file per category, regardless
of how many parts fed it).

## 7. Chunk for Discord

For each category with a `notes/<slug>.md`:

```sh
python3 tools/discord_chunker.py "$OUT/notes/<slug>.md" "$OUT/discord/<slug>"
```

If the chunker's stderr warns about hard truncation (`WARNING: N chunks exceeded 2000 chars`),
that's a review failure, not an acceptable output — go back to Step 6, shorten the offending
section (cut prose, never numbers or facts), and re-run the chunker.

## 8. Manifest + summary

```sh
python3 tools/update_manifest.py "$OUT"
```

Then print a per-category table (label, bundle count, chunk count, bug-watch count) plus the
output paths (`$OUT/notes/`, `$OUT/discord/`, `$OUT/manifest.json`) — no game-data paths, no
`$FO76_DATA_DIR` expansion, in that printed summary either.

## Guardrails

- Never assert a record's liveness from an EDID prefix alone (`zzz_`/`CUT_`/`DEL_`/`POST_`) —
  it's a heuristic, not proof. For PERK ranks, the only clean signal is a `PCRD` actually listing
  that rank in `Perks[].Perk`.
- Every lint that reaches a final note must be re-verified against the live daemon in this run;
  one that can't be reproduced is dropped, not asserted on stale/static data.
- Every number in a final note traces to the slice, an `--extract`, or a live `esm -p` call —
  never memory, never estimation, never rounding.
- No absolute filesystem paths, ESM filenames, or `$FO76_DATA_DIR` expansions in any file under
  `$OUT` — drafts, notes, discord chunks, or manifest.
- This skill only ever writes inside `$OUT`. It never modifies game data, the repo, or anything
  outside the output directory — no exceptions.
