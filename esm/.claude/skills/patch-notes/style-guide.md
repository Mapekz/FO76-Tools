# Patch notes style guide

Read this before writing a single line of a draft. It applies to every category writer.

## Voice

- Player-facing, non-technical. Assume the reader has never opened the Creation Kit and does
  not care what a subrecord is.
- Numerically exact. Never round or hand-wave a number that data gives you precisely.
- Present tense ("the drop rate is now 10%", not "the drop rate has been changed to 10%").
- EditorIDs in backticks only when they add real value (e.g. disambiguating two similarly-named
  items). Don't decorate every noun with backticks.
- FormIDs never appear in narrative prose — only inside Bug Watch Evidence lines.
- No filler. "Various improvements," "general polish," "quality of life updates" are banned.
  If a change is boring, it still gets exactly one plain bullet stating what changed — never a
  vague summary standing in for specifics.

## Per-category file structure

Write to the draft path as GitHub-flavored Markdown; the chunker (see Discord constraints below)
converts it before posting. Use exactly this section order, and **omit any section that has no
content** — don't emit an empty heading.

```
# <Category> — Patch <YYYY-MM-DD>

## TL;DR
- 3-5 bullets, whole section ≤600 chars total
- The highlights only — a reader who stops here still gets the gist

## <player-recognizable bundle title>
One short story paragraph: what changed and why it matters, using the bundle's labeled edges
("Kingfisher moved to Linda-Lee's drop pool", "now crafted from 2x Adhesive instead of 3x").
Then compact stat bullets with the plain-language mechanic first, exact delta second:
- Reload speed: 5s → 3.75s (−25%)
- Value: +0.5 (MUL then ADD)

(repeat one `##` section per bundle, ordered flagship-first)

## Bug watch
Verified claims only — nothing here that wasn't reproduced live. One entry per confirmed lint:
- **Claim** — plain statement of the bug/quirk.
- **Player impact** — what this means for someone playing, in one sentence.
- **Evidence** — `record_type` `EditorID`/FormID, field, observed value.

## Vaulted / cut this patch
Records renamed into zzz_/CUT_/DEL_/deprecated_ this patch. Short, factual, no eulogy.

## Datamined / coming soon
Standing disclaimer FIRST, every time this section appears:
"The following is unreleased/datamined content pulled from game files. It is not live and may
change or never ship." Then POST_ additions/edits only — never mixed into other sections.
```

A bundle only gets its own `##` section if it has enough story to justify one; otherwise fold
small related bundles under a shared heading rather than emitting a stub section.

## Discord rendering constraints

The chunker (`tools/discord_chunker.py`) converts this file to Discord-safe markdown, then
splits it into ≤1900-char posts. Concretely, it:

- Turns GFM tables into monospace code-block tables — and **strips all inline markdown from
  table cells** in the process (bold/italic/code/links inside a cell become plain text).
  → **Don't rely on inline markdown inside table cells.** Prefer bullets to tables where you'd
  want bold or code formatting on a value; if you do use a table, keep it to ≤4 short columns.
- Turns `#`/`##` headers into decorated bold lines; **H3+ all flatten to the same plain bold** —
  there is no visual hierarchy past two levels.
  → Don't rely on H3+ to convey structure. Stick to the two levels in the structure above.
- Turns `---` into a Unicode rule line.
- Splits only at blank lines (falls back to closing/reopening code fences mid-block if it must).
  → **Blank lines between logical blocks are the only reliable split points.** Always leave one
  between bundle sections, between a story paragraph and its stat bullets, and around Bug Watch
  entries.
- Has no support for HTML, images, footnotes, or `[text](url)` markdown links — they either
  vanish or render as raw brackets. Don't use them.
- Keep any single code block under ~1500 chars — a code block cannot be split mid-fence without
  the chunker closing and reopening it, and an oversized single block risks the 2000-char hard
  truncation.

## Length discipline

- Category file: ≤~12,000 chars total (roughly 7 Discord chunks after conversion).
- Flagship bundle section: ≤~1,200 chars.
- Ordinary bundle section: ≤~600 chars.
- When over budget, **cut prose, never numbers.** A shorter story paragraph is fine; a dropped
  stat bullet or a rounded number is not.
