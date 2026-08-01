# esm

Read-only engine for explaining Fallout 76 ESM records: decoding, diffing, and answering
"what does this record actually do in game."

## Language

**Mechanism**:
One of the four ways an OMOD implements its gameplay effect: direct property, perk grant,
keyword hook, or projectile override. That four-way *domain* taxonomy is a different axis
from `HopKind`'s *resolution* taxonomy (`DirectProperty` / `PerkGrant` / `KeywordHook`, plus
`TagKeyword` for the non-mechanism case): `kind` describes *how* a mechanism is resolved
(forward fetch vs reverse refs), not *what* it is. `OverrideProjectile` stays
`DirectProperty` because the property name already names the specialization; `TagKeyword`
is a real fourth kind because `KeywordHook` was actively wrong for it, not merely coarse.
_Avoid_: effect (that's the record's outcome, not how it's wired), behavior

**Chase pattern**:
Resolving a record's mechanisms by classified forward and reverse walks.
_Avoid_: using "chase" for any generic reference-following (that's a walk)

**Keyword hook**:
A mechanism where the OMOD only adds a keyword and the behavior lives on a consumer
(SPEL/PERK) whose effect is gated on wearing that keyword.
_Avoid_: keyword effect (the keyword itself carries no behavior); treating a
**Tag keyword** as a keyword hook

**Tag keyword**:
A KYWD whose purpose is item policy (droppable/tradable/sellable/UI grouping) rather than
a gameplay gate — identified by a populated `Type` field (anything other than `"None"`) or
by zero SPEL/PERK consumers after reverse chase; never rendered as a keyword hook.
_Avoid_: keyword hook (a tag has no SPEL/PERK consumer; the dead-end caveat must not fire)

**Digest**:
A compact per-record-type rendering of a record's gameplay-relevant fields.
_Avoid_: dump (a dump is the raw decoded record, exactly what a digest replaces)

**Evidence slice**:
Only the effect rows of a consumer record that are gated on the record under study,
located by reference path.
_Avoid_: rendering a consumer's full digest as "evidence"

**Hub keyword/AV**:
A keyword or actor value read by many unrelated consumers, whose reverse references are
therefore mostly noise for any one chase.

**Obtainability signal**:
A reverse reference from a player-facing source type (recipe, reward, legendary pool,
quest, container, loot list) indicating a record is actually reachable in game.
_Avoid_: treating any reverse reference as proof of obtainability

## Relationships

- An **OMOD** implements its effect via one or more **Mechanisms**
- A **keyword hook**'s behavior lives on a consumer record, never on the keyword itself
- A **tag keyword** is item policy, not a mechanism — it has no consumer to chase
- A **Digest** renders one record; an **Evidence slice** renders only the gated rows of
  a consumer — a **hub keyword/AV**'s consumers are why the slice exists
- **Obtainability signals** are a subset of a record's reverse references

## Example dialogue

> **Dev:** "The walk of Furious only showed an enchantment chain — is that the whole
> mechanic?"
> **Domain expert:** "No — Furious has two **mechanisms**. The second is a **keyword
> hook**: the OMOD adds a keyword, and a hub perk's effects are gated on wearing it. You
> want the **evidence slice** of that perk, not its full **digest** — it carries a dozen
> unrelated legendary effects."

## Flagged ambiguities

- "chase" was used both for the mechanism-classification pattern and for generic
  reference-following — resolved: the **chase pattern** is only the former; everything
  else is a walk.
