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

**Carrier**:
A record matched by a Carriers-shape `refs` seed selector, emitted as a `depth: 0` seed
row tagged with what it matched (`RefRow.tags` / `CarrierTag`).
_Avoid_: calling the Direct seed itself a carrier (Direct seeds are never emitted as rows)

**Seed selector**:
How `refs` resolves its starting FormID(s): **Direct** (one FormID/EditorID/hardcoded AVIF)
or **Carriers** (zero or more matched records). See [`docs/adr/0004-refs-seed-selectors.md`](docs/adr/0004-refs-seed-selectors.md).
_Avoid_: treating hardcoded AVIFs as a third selector kind

**Enum space**:
An OMOD property's Form-Type-keyed namespace — `weap`, `armo`, or `npc`. Property ids are
only meaningful inside one space; the same id number names a different property in each.
_Avoid_: treating a bare numeric property id as unambiguous across spaces

**Element identity**:
The field(s) that identify one element of a decoded rarray across two snapshots, so a diff
can pair old/new elements instead of reporting the whole array wholesale. Owned solely by
`diff.rs::element_key_spec` — `patchnotes_lib.py` normalizes and renders whatever Rust
decided, it does not decide identity itself. See
[`docs/adr/0005-element-identity-owned-by-rust.md`](docs/adr/0005-element-identity-owned-by-rust.md).
_Avoid_: "array key" (the identity is a domain fact about the record shape, not a diff
implementation detail)

**Unkeyed array**:
An array whose elements have no stable element identity, so a diff reports the two whole
element lists (`removed`/`added`) rather than pairing them. CTDA `Conditions[]` is the
canonical case: a condition's position is semantic (`AND`/`OR` chaining across the whole
list), so keying it would pair unrelated rows and report false mutations — it is
*deliberately* unkeyed, not a gap to be closed by adding a key spec.
_Avoid_: "opaque array" (the old pre-issue name implied the contents were unavailable; an
unkeyed array's elements are fully present, just unpaired)

## Relationships

- An **OMOD** implements its effect via one or more **Mechanisms**
- A **keyword hook**'s behavior lives on a consumer record, never on the keyword itself
- A **tag keyword** is item policy, not a mechanism — it has no consumer to chase
- A **Digest** renders one record; an **Evidence slice** renders only the gated rows of
  a consumer — a **hub keyword/AV**'s consumers are why the slice exists
- **Obtainability signals** are a subset of a record's reverse references
- An **unkeyed array** is the array-diff outcome when no **element identity** applies

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
