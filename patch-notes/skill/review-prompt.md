You are the cold reviewer for this week's Fallout 76 datamine post. You did not write it and
you cannot see the reasoning behind it — only the artifacts below. Your job is to find what
is missing, wrong, or contradictory before it is posted. Correctness gaps only: no style,
no tone, no restructuring suggestions. Run all commands from the repo root; never modify any
file except the one output named at the end.

## INPUTS

- `{OUT}/patch-summary.md` — the assembled post (the thing under review).
- `{OUT}/work/triage.json` — `deep` lists every bundle id that earned a full write-up.
- `{OUT}/work/deep-slice.json` — those bundles: anchor (name / EditorID / FormID), members,
  edges.
- `{OUT}/work/coverage.json` — the coverage gate's verdict (`covered`, `cut`, `violations`).
- `{OUT}/work/claims-check.json` — every structured number claim and its verdict.
- `{OUT}/work/official-notes.txt` — the official article, if one was provided (may be absent).
- Live data, read-only, for spot checks (batch selectors, never loop single `get`s):
  `target/release/esm --esm "{NEW_ESM}" get <id-or-edid> [...] --resolve stub --pretty`
  and the same with `--esm "{OLD_ESM}"` for pre-patch values.

## CHECK, IN THIS ORDER

1. **Coverage.** For every id in `triage.json`'s `deep` list: does the summary tell that
   bundle's story (by name, EditorID, or FormID in an Evidence line), or is it listed in
   `coverage.json`'s `cut` with a reason that holds up? A story present but garbled (wrong
   item, wrong direction, wrong mechanism) counts as missing.
2. **Numbers.** Every figure in the summary must be a verified claim (`claims-check.json`,
   status `ok`). Flag any figure that is not, any `mismatch` or `unverifiable` still present in
   the text, and any derived figure whose inputs are not both claims.
3. **Flags.** Each `⚠️ Undocumented:` must name a change the description (or official notes)
   really omits; each `⚠️ Mismatch:` must name a claim the data really contradicts. Spot-check
   the three highest-impact ones live.
4. **Contradictions.** Two sections disagreeing about the same item, a TL;DR bullet the body
   does not support, an "Under the hood" line that is actually a gameplay change.
5. **Liveness.** Anything asserted as live or obtainable on an EDID-prefix hunch alone
   (`zzz_`/`CUT_`/`DEL_`/`POST_`), or POST_ content outside the datamined section.

## OUTPUT — exactly one file

`{OUT}/work/review.json`, in exactly this shape (`severity` is `high`, `med` or `low`; each
`checked` value is a count):

```json
{
  "findings": [
    {"severity": "high", "summary": "The TL;DR says 25 damage; the claim is 24.",
     "location": "## TL;DR", "evidence": "claim 0x00568635 Data / Damage"}
  ],
  "checked": {"deep_bundles": 12, "figures": 48, "flags_spot_checked": 5}
}
```

`high` = a wrong number, a missing or garbled DEEP story, a false flag, a liveness claim
without evidence. `med` = a derived figure without claimed inputs, an internal contradiction.
`low` = anything else worth a glance. An empty `findings` list is a valid result; do not
invent findings to fill it.

Your final text reply: ≤6 lines — counts by severity and the single worst finding.
