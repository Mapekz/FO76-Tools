You are drafting the **{CATEGORY_LABEL}** section of this week's Fallout 76 patch notes for
Discord. You own only this category — one bundle slice in, one draft + one report out. Other
categories are being written in parallel by other agents; don't touch their files.

## INPUTS

- **Slice (read this first — it's your work queue):** `{SLICE_PATH}`
  Contains this category's bundles: `{"bundles": [{"id", "category", "category_label", "title",
  "anchor": {form_id, record_type, editor_id, name, status}, "members": [{form_id, record_type,
  editor_id, name, status: added|changed|removed, role: anchor|satellite|context}], "edges":
  [{from, to, relation, label, via}], "bug_watch": bool, "lint_ids": [...]}], "lints": [{"id",
  "rule", "severity", "form_id", "message", "data"}]}`. `edges[].label` (e.g. "dropped via", "mod
  for", "crafts") is the connective tissue for your story paragraphs — use it, don't reinvent it.

- **Per-record detail:** the slice gives you shape and relationships, not full field diffs. For
  the actual before/after values of any `form_id` in the slice, extract it from the pipeline's
  comprehensive record of changes:
  ```
  python3 tools/slice_bundles.py --extract {OUT} <FORMID> [<FORMID> ...]
  ```
  Pass every FormID you need in one call — it's a single fixed cost either way, so batch it.

- **Live verification** — always through the warm daemon, never `--local`:
  ```
  target/release/esm -p get "{NEW_ESM}" <id> --resolve stub --pretty
  target/release/esm -p refs "{NEW_ESM}" <id> [--depth N] [--limit N]
  target/release/esm -p search "{NEW_ESM}" "<pattern>" --type <T>
  ```
  For old-side checks (what a value used to be, whether a lint is new this patch), run the same
  shape of command against `{OLD_ESM}` instead of `{NEW_ESM}`.

- **Style guide (read before writing anything):** `{STYLE_GUIDE_PATH}` — voice, section
  structure, and the Discord formatting constraints your draft must satisfy.

## FOR EACH BUNDLE

1. **Tell its story using the labeled edges.** What changed, and how the pieces fit together —
   "Kingfisher moved to Linda-Lee's drop pool," not "LVLI 0x123 was modified." The edges
   (`dropped via`/`mod for`/`crafts`/etc.) tell you the relationships; use them to write one
   coherent paragraph per bundle, not a flat list of unrelated record changes.

2. **Exact stats, plain language first.** State the mechanic in player terms, then the precise
   delta: "reload speed 5 → 3.75 (−25%)", "Value MUL+ADD +0.5". Every number must come from the
   slice, an extract, or a live call — never from memory or estimation.

3. **Verify before asserting — nothing reaches the draft unreproduced.**
   - For every lint on this bundle (`bundle.lint_ids`, cross-referenced against `lints[]` in the
     slice): reproduce it with a live `-p get` on the *parent* record before it goes into Bug
     Watch. A lint that only exists in the static slice/lint data and can't be reproduced live is
     dropped from the draft entirely and listed in the report's `not_reproduced`, not silently
     ignored.
   - Every Bug Watch entry carries an **Evidence line**: record type, EditorID/FormID, field
     name, observed value — from the live call you just ran, not the slice.
   - **Description-vs-stats cross-check:** for anchors with a description/FULL text, fetch it and
     compare against the actual effect/OMOD numbers. Mismatches are Bug Watch material — e.g.
     "'Shoots both barrels' but the damage bonus is only +200% additive, not doubled." Don't
     assume the description is accurate; don't assume it's wrong either — check.
   - **PERK liveness:** never present a PERK rank as live without checking whether some `PCRD`
     actually grants it. `esm -p refs <perk-formid> --limit 20` → does a `PCRD` show up? If so,
     `esm -p get <pcrd-formid> --resolve stub --pretty` → does its `Perks[].Perk` list include
     this rank? A rank with no referencing `PCRD`, or one past where its `PCRD` list stops, is
     orphaned — do not narrate it as something a player can obtain, regardless of naming or
     `Playable` flag.

## Cut / deferred handling

- A member whose EditorID was **renamed into** a `zzz_`/`CUT_`/`DEL_`/`deprecated_` prefix this
  patch belongs in "Vaulted / cut this patch" — not in the main bundle story.
- A member that is `POST_`-prefixed (added or edited) belongs **only** under "Datamined / coming
  soon", with the standing not-live disclaimer from the style guide. Never let a `POST_` record
  appear anywhere else, even if it's mechanically interesting.
- The prefix convention is a heuristic, not proof — it's inconsistently applied in the source
  data. When you're not sure whether something renamed/added this patch is actually live, write
  "Unconfirmed:" and say what's uncertain. Never assert liveness you haven't checked.

## OUTPUT

Produce exactly two files — nothing else:

1. **Draft** → `{DRAFT_PATH}` — Markdown, structured per `{STYLE_GUIDE_PATH}`. No absolute
   filesystem paths, no ESM filenames, no local directory names anywhere in the prose.

2. **Report** → `{REPORT_PATH}` — JSON with exactly this shape:
   ```json
   {
     "category": "{CATEGORY}",
     "bundles": <int, count of bundles you wrote up>,
     "claims_verified": <int, count of distinct numeric/factual claims you confirmed live>,
     "lints_confirmed": ["<lint id>", ...],
     "lints_not_reproduced": ["<lint id>", ...],
     "unverified_claims": ["<short description of a claim you could not verify>", ...],
     "cross_category_formids": ["<form_id>", ...]
   }
   ```
   `cross_category_formids` lists any FormID you narrated in this draft whose anchor actually
   lives in a different category's bundle (e.g. you mentioned a keyword's parent weapon in
   passing) — so the assembly step can dedupe or cross-link it.

Do not write any file other than these two.
