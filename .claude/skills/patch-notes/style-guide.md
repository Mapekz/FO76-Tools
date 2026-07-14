# Patch notes style guide

Read this before writing a single line. It applies to the deep writers and the orchestrator's
assembled summary alike.

## Voice

- Player-facing, non-technical. Assume the reader has never opened the Creation Kit and does
  not care what a subrecord is.
- Numerically exact. Never round or hand-wave a number that data gives you precisely.
- Present tense ("the drop rate is now 10%", not "the drop rate has been changed to 10%").
- Plain-language mechanic first, exact delta second: "reloads faster — reload speed 5s →
  3.75s (−25%)".
- EditorIDs in backticks only when they add real value (e.g. disambiguating two
  similarly-named items). FormIDs never appear in narrative prose — only in Evidence lines.
- No filler. "Various improvements," "general polish," "quality of life updates" are banned.
- Flag conventions (these carry the post's value — use them consistently):
  - `⚠️ Undocumented:` — a real change the item's own description (or the official notes,
    when provided) doesn't mention.
  - `⚠️ Mismatch:` — a description or official-notes claim the data contradicts.
  - `Unconfirmed:` — something you could not verify; say exactly what's uncertain.

## Document structure (single summary doc)

```
# FO76 Datamine — Patch <YYYY-MM-DD>

## TL;DR
- ≤6 bullets, whole section ≤600 chars — a reader who stops here gets the gist.

## <topic sections, ordered by signal>
### <player-recognizable item/feature name>
One short story sentence: what changed and why it matters.
- Stat bullets: mechanic in plain words, then exact old → new numbers.
- ⚠️ Undocumented / ⚠️ Mismatch bullets where earned.

## Datamined: <feature> (not live)
Standing disclaimer FIRST, every time: "The following is unreleased/datamined content pulled
from game files. It is not live and may change or never ship." POST_/gated content only —
never mixed into live sections.

## Cut / Vaulted
Records renamed into zzz_/CUT_/DEL_ this patch. Short, factual, no eulogy.

## Also this patch
The BRIEF one-liners (templated adds/cuts/renames), pruned of anything covered above.
```

An item gets its own `###` only if it has enough story to justify one; fold small related
changes under a shared heading rather than emitting stub sections.

## Discord rendering constraints

The chunker (`tools/discord_chunker.py`) converts the summary to Discord-safe markdown, then
splits it into ≤1900-char posts. Concretely, it:

- Turns GFM tables into monospace code-block tables — and **strips all inline markdown from
  table cells**. → Prefer bullets to tables; if you must use a table, ≤4 short columns.
- Turns `#`/`##` headers into decorated bold lines; **H3+ all flatten to the same plain
  bold** — no visual hierarchy past two levels. Keep structure to the levels above.
- Splits only at blank lines. → **Always leave a blank line** between sections, between a
  story sentence and its stat bullets, and around any code block.
- Has no support for HTML, images, footnotes, or `[text](url)` links — don't use them.
- Keep any single code block under ~1500 chars.

## Length discipline

- Whole summary: ≤~12,000 chars (roughly 7 Discord chunks after conversion).
- Flagship item section: ≤~1,200 chars. Ordinary item section: ≤~600 chars.
- When over budget, **cut prose, never numbers.** A shorter story sentence is fine; a dropped
  stat bullet or a rounded number is not.
