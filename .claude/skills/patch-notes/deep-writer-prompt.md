You are drafting the deep-analysis sections of this week's Fallout 76 datamine post
(patch {OLD_TOKEN} → {NEW_TOKEN}) for Discord. You own the bundles in your slice — chase
every one to ground truth. Run all commands from the repo root.

## INPUTS

- **Your work queue:** `{SLICE_PATH}` — DEEP-tier bundles (same shape as before:
  `{"bundles": [{"id", "title", "anchor", "members", "edges", "bug_watch", "lint_ids"}],
  "lints": [...]}`). Edges (`dropped via`/`mod for`/`crafts`) are your story connective tissue.
- **Mechanics KB (read FIRST):** `{KB_PATH}` — known mechanic derivations, the OMOD chase
  pattern, property semantics, schema errata. Consult before chasing; only chase mechanics
  it doesn't cover. Entries are dated — re-verify stale-looking ones with one live call.
- **Per-record structured diff** (batch FormIDs in one call):
  `python3 esm/tools/slice_bundles.py --extract {OUT} <FORMID> [<FORMID>...]`
- **Live verification via warm daemon (NEVER `--local`):**
  - `esm/target/release/esm -p get "{NEW_ESM}" <id-or-edid> [<id2-or-edid> ...] --resolve stub --pretty`
    — batch every FormID/EditorID you need into ONE call. 2+ selectors return a JSON array
    (one `{"sel": ..., ...}` entry per selector, mixed FormID/EditorID, errors isolated
    per-selector); never loop single `get`s.
  - `esm/target/release/esm -p refs "{NEW_ESM}" <id> --type <SIG> --paths [--depth N] [--limit N] --pretty`
    — `--type` takes ONE 4-char record-type signature per call (run it once per referencing
    type, e.g. once for `SPEL`, once for `PERK` — not comma-joined); `--paths` annotates each
    row with the exact field path (e.g. `Effects[2].Conditions[0]`) that references your
    target, so you can jump straight to the gating field instead of dumping the whole record.
  - `esm/target/release/esm -p search "{NEW_ESM}" "<pattern>" [--type T] --pretty`
  - Old-side (pre-patch values): same commands against `{OLD_ESM}`. Batch all changed anchors
    for a bundle into one bulk `get` against `{OLD_ESM}` rather than querying value-by-value.
- **Style guide:** `{STYLE_GUIDE_PATH}` — voice and Discord formatting constraints.
{OFFICIAL_NOTES_BLOCK}

## FOR EACH BUNDLE

1. **Chase the mechanic to ground truth** (KB "chase pattern" section). For
   `mod_Custom_*`/unique-effect OMODs, run
   `esm/target/release/esm -p chase "{NEW_ESM}" <OMOD> --json` FIRST — it automates the
   keyword/perk-grant/direct-property walk in a handful of bulk calls and returns just the
   gating `Effects[N]` entry, not full record dumps. Hand-walk with
   `refs --type <SIG> --paths` (and a bulk `get` on whatever it turns up) only for mechanics
   chase doesn't cover: resolve every PERK/ENCH/SPEL/AVIF/KYWD a changed property touches
   until you can state what the change does in player terms. An AVIF's name is not its
   semantics — find its consumer.
2. **Plain language first, exact delta second**: "reload speed 5s → 3.75s (−25%)". Old → new
   wherever the diff alone is ambiguous — batch every changed anchor's before-value into one
   bulk `get` against the OLD esm rather than querying them one at a time. Never round, never
   estimate; every number comes from the slice, an extract, or a live call.
3. **Hunt silent changes — this is the post's core value:**
   - Any property change NOT reflected in the item's Description → `⚠️ Undocumented:` bullet.
   - Any Description claim contradicted by the numbers → `⚠️ Mismatch:` bullet.
   - Fetch every anchor's description in ONE bulk `get` call, then compare; assume nothing
     either way.
4. **Verify before asserting**: reproduce every lint on your bundles with live `get`s — batch
   all the affected FormIDs for a lint into one bulk call rather than looping — before writing
   it up (irreproducible lints go in the report's `lints_not_reproduced`, never the draft).
   Item-granted perks have no PCRD (see KB) — don't call them orphaned; verify the grant path
   via `refs "{NEW_ESM}" <perk-id> --type PCRD --paths --pretty` instead. Never assert
   liveness from an EDID prefix alone (`zzz_`/`CUT_`/`DEL_`/`POST_` are heuristics); POST_
   content goes only under the datamined section with the standing disclaimer.

## DEFERRALS — do not silently skip

If a bundle's story genuinely belongs to another writer's slice (check `{OTHER_SLICES}`),
write a one-line cross-reference in the draft AND list it under `deferred` in your report
with the FormIDs and one sentence on what you expect the other writer to cover. The
orchestrator reconciles every deferral — an unlisted skip is a dropped story.

## OUTPUT — exactly two files

1. **Draft** → `{DRAFT_PATH}` — Markdown per the style guide. No absolute filesystem paths,
   ESM filenames, or local directory names anywhere.
2. **Report** → `{REPORT_PATH}` — JSON:
   ```json
   {
     "bundles": <int>,
     "claims_verified": <int>,
     "lints_confirmed": ["..."], "lints_not_reproduced": ["..."],
     "unresolved": [{"what": "...", "tried": "..."}],
     "deferred": [{"form_ids": ["..."], "expected_owner": "...", "note": "..."}],
     "kb_proposals": ["<new mechanics-kb entry in the KB's own format, as a string>", ...]
   }
   ```
   `unresolved` = anything you could not fully derive (the orchestrator chases these
   interactively). `kb_proposals` = mechanics you derived that the KB doesn't cover yet.

Your final text reply: ≤10 lines — headline findings, unresolved count, deferred count.
