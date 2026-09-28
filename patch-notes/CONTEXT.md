# patch-notes

Turns the diff between two ESM snapshots into a Discord patch-notes post: a deterministic
mechanical stage, then agent-written prose that is gated against the data. Record, diff and
array-diff terms (**Element identity**, **Reorder-only array**, …) are esm's; see
[esm/CONTEXT.md](../esm/CONTEXT.md).

## Language

**Change entry**:
One field-level change of one record, as `comprehensive.json` lists it: a path of
`" / "`-joined field names (array rows addressed by `key_display` in brackets), the before
and after values, and a `suppressed` reason when it is noise.
_Avoid_: diff row, delta

**Bundle**:
A cluster of changed records that tell one story — a weapon with its mod slots, drop list
and unique keyword — identified `B0001`, `B0002`, … and the unit every later stage works in.
_Avoid_: group, category

**Anchor**:
The record a **Bundle** is named after and sorted by, chosen by record-type priority
(`build_bundles.DEFAULT_SETTINGS["anchor_priority"]`).
_Avoid_: root, primary record

**Tier**:
What happens to a **Bundle** in the post: `deep` (a written section), `brief` (a templated
one-liner), `rollout` (aggregated into one bulk data-shape note), `drop` (never surfaced,
with a logged reason), or `ambiguous` (resolved to deep, brief or drop by an assessor agent).
_Avoid_: priority, severity

**DEEP slice**:
The DEEP-tier **Bundles** and their lints handed to one deep writer
(`work/deep-slice.json`, or two contiguous halves above 20 bundles).
_Avoid_: bare "slice", which esm uses for an **Evidence slice**

**Claim**:
A structured statement of one figure a writer put in a draft — `changed` (from/to),
`existence` (added/removed) or `value` (one side) — that the gate re-derives from
`comprehensive.json` or a live `esm` lookup. An unverifiable claim fails the gate like a
wrong one.
_Avoid_: fact, assertion

**Deferral**:
A writer's statement that records in its **DEEP slice** belong to another writer's story,
naming the expected owner. A deferred **Bundle** passes the coverage gate only when
another writer's report covers it.
_Avoid_: skip, handoff

**Cut**:
A DEEP **Bundle** deliberately left out of the summary, with a reason, in `work/cuts.json`.
_Avoid_: drop (that's a **Tier**); see Flagged ambiguities

**Cut record**:
A record whose EditorID carries a cut marker (`ZZZ`, `CUT`, `DEPRECATED`, …), classified
with a confidence by `change_entries.classify_cut`. The marker is a hint, not proof of
removal: liveness needs other evidence (see the skill's guardrails), and `POST_` content is
datamined, not cut.
_Avoid_: deleted record; calling content removed on the marker alone

## Relationships

- The mechanical stage turns a diff into **Change entries**, clusters them into **Bundles**
  around an **Anchor**, and assigns each a **Tier**.
- Each deep writer gets a **DEEP slice** and returns a draft plus **Claims** and
  **Deferrals**; the gate checks every **Claim**, and that every DEEP **Bundle** is covered
  by exactly one draft and reaches the summary or is a **Cut**.

## Flagged ambiguities

- "Cut" names both a **Cut** (an editorial omission) and a **Cut record** (game content).
  Say which; `cuts.json` holds only the first.
- "Slice" alone is ambiguous across projects: say **DEEP slice** here and **Evidence
  slice** for esm's rendering of a consumer's gated rows.
