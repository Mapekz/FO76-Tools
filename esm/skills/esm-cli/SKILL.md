---
name: esm-cli
description: Using the FO76 `esm` CLI effectively and reading what it returns — invocation and cache behaviour, refs gotchas, walk/chase mechanics digests, curve tables, drop-chance math, and live-vs-cut game data. Use when querying SeventySix.esm records, decoding a perk/OMOD/legendary mechanic, deciding whether a record is live or player-obtainable, reading a damage or drop-chance number, or wrapping the CLI in scripts.
---

# esm CLI knowledge

Usage knowledge for the `esm` CLI (FO76-Tools/esm), embedded in the binary —
`esm skill` prints it, `esm skill --install` installs it into a consumer repo.

**This doc never repeats what `esm <cmd> --help` says about a flag.** Run help
for flag syntax and semantics; what follows is only what help cannot say —
traps, judgment, and how to read game data. The binary is authoritative and
this crate moves fast: re-verify here against `--help` before wrapping a
subcommand.

## Invocation & path resolution

- Subcommands: `cache, info, get, list, search, refs, tree, diff, coverage,
  chase, walk, curve, batch, skill`.
- `FO76_ESM_PATH` is a plain process env var — there is no `.env` parser.
- Every subcommand is one-shot and runs in-process; a missing subcommand is a
  usage error, not a REPL. Once an ESM's cache exists, a call costs a few
  milliseconds, so loops of individual calls are fine; a bulk `get` with many
  selectors is still one call instead of N.
- **A cold call against an ESM with no `esm_cache/` yet can take tens of
  seconds to a couple of minutes** — worst case a first-ever
  `refs`/`walk`/`chase`, which builds the `xref` index (a full schema decode of
  every record). It streams progress to stderr rather than hanging silently and
  still returns the real result — just wait. The build runs as a detached
  `esm cache build`, so it finishes even if the calling command is killed (a
  tool timeout), and a second concurrent query waits for that build instead of
  starting another. `esm cache status [--json]` inspects without triggering
  anything; `esm cache build` builds ahead of time and `esm cache clear`
  deletes an ESM's cache.
- The cache rebuilds by itself when the ESM, its `strings/`, or a rebuilt
  binary's schema changes. Editing a curve file inside an existing
  `misc/curvetables/json/` tree is not detected — `esm cache clear` after that.
- Scripts making many calls can keep one `esm batch` child: it reads one
  `{"esm": <path>, "op": {...}}` JSON request per line and answers each with
  one `{"status": "ok"|"err", ...}` line, keeping each ESM open in between.
- `ESM_NO_PROGRESS=1` suppresses heartbeat *publishing* only (e.g. in an
  embedding context where a stray file write is unwanted) — lock-based dedup
  between concurrent builders keeps working regardless.

## Fetching records

- Selectors are FormIDs or EditorIDs, auto-detected. **A bare token is read as
  hex, never decimal** — `esm get 00568635` means `0x00568635`. When that hex
  reading has no record, resolution falls to EditorID and then the
  engine-hardcoded table, never to a decimal reading; `--decimal` is the
  explicit override. Scripts should pass `--formid`/`--edid` to skip the
  ambiguity.
- **Bulk get**: pass several selectors to one `get`. A bad selector becomes
  `{"sel":…, "error":…}` in the array instead of failing the call; the
  single-target form throws.
- **Default to `--resolve stub` for reference-heavy records** (recipes, NPCs,
  leveled lists, quests) — it avoids N follow-up `get` calls, and a GLOB
  reference carries its own `Value` on the stub, so magnitudes, durations,
  required counts and condition thresholds read straight off it. Reach for
  `--resolve full` only when the complete nested body is needed; bare `get` is
  fine when the raw FormID values are what you actually want.
- `list` never returns display names — use `search --in name` or `get`.
  `search` needs `"*"` to match all; `""` matches nothing.
- `--limit 0` means unlimited on `list`/`search`/`refs`. All three print
  `note: output capped at N of M results; use --limit 0 to show all` to
  **stderr**, never stdout, so `--json` stays parseable when capped. `refs
  --json` is the whole walk result — `rows` plus `total`, `capped` and the
  depth fields — not a bare row array.
- **Source-override flags (`--localization-ba2`/`--strings-dir`, plus
  `--startup-ba2` on `get`/`diff`) parse those sources for this one call**,
  skipping the `lstrings`/`curves` cache. For sweeps needing localized strings, put the
  Localization BA2 (or a `strings/` folder) and the Startup BA2 (or a
  `misc/curvetables/` folder) beside the ESM instead — they are discovered and
  cached on open, with no per-call flags.
## Reverse references (`refs`)

- The default `--limit 100` truncates popular targets (see the stderr capped-
  output note above) — pass `--limit 0` when you need everything. Raise
  `--depth` to reach a target through an intermediary (e.g. a quest alias).
- `--entry-point`/`--ep` answers "what uses this hook?" — the reverse of
  reading a PERK's own Entry Point off `get`/`walk`. `esm refs --ep 'Mod
  Percent Blocked'` surfaces the Blocker perks and the Ogua Gauntlet/Defender's
  leggo perks; one more `--depth` reaches the OMODs/weapons/perk cards that
  reach *them*. Prefer the **numeric id** over the name: some entry-point names
  are wrong (see below), and a few ids have no name in the schema at all —
  `--ep 212` still finds its unnamed carrier. `--ep 0x...` is rejected (that's
  a FormID, not an entry point).
- **EP attribution:** a multi-match glob's stderr legend lists every matched
  entry point as `id name` — e.g. `entry point 'Mod Weapon*' (2 matched: 44 Mod
  Weapon Reload Speed, 45 Mod Weapon Spread)`. When more than one distinct id
  appears in the printed rows an `EP` column shows comma-joined numeric ids per
  row, carriers grouped by primary EP and BFS rows inheriting the originating
  carrier's tag; `VIA` is populated from depth 1 and starts with the
  originating carrier FormID. Attribution is
  first-reach at minimum depth; equal-depth ties **union**
  EP tags (a record referenced by two carriers shows both ids), but an overlap
  first discovered at depth ≥ 2 that was already reached shallower stays
  attributed only to the shallower carrier. Carriers are emitted *before*
  referencers, so a broad glob at the default `--limit 100` may show only
  carrier rows — use `--limit 0` when you need the referencers.
- `--omod-property`/`--prop` answers "which OMODs declare this property?".
  Unlike `--ep` it is **flag-only**, never auto-detected from a bare positional
  — `Health` is also a real AVIF EditorID, and a bare `refs Health` must keep
  resolving to that AVIF. Cross-space name collisions (same spelling, different id per space) include
  at least `Keywords` (weap:31 / armo:3 / npc:0), `Enchantments` (weap:65 /
  armo:0 / npc:3), `Perk` (weap:116 / armo:18), `ActorValues`/`Actor Values`,
  and `Health` — scope-qualify whenever the space matters. Same
  carriers-before-referencers caveat as `--ep`, and `Keywords` alone has
  ~7,500+ carriers, so the default `--limit 100` shows nothing but carriers.

**Worked example** (`Data.Includes[]` inheritance needs no special flag —
`--paths` already labels the edges that include a Speed-declaring OMOD):

```
$ esm refs 0x00121D54 --depth 1 --paths          # mod_Minigun_Barrel_short, declares Speed
FORMID      TYPE  EDID                                PATHS
0x00188A08  COBJ  co_mod_Minigun_BarrelMinigun_short  Created Object
0x001C6BC5  OMOD  modcol_Minigun_Barrels_Any          Data.Includes[1].Mod
0x007AC747  WEAP  crMinigun                           Object Template.Combinations[8]....Includes[0].Mod
```

`esm refs --prop weap:Speed --depth 1 --paths` surfaces the same three edge
kinds from every Speed carrier: a COBJ crafting recipe (`Created Object`),
a `modcol_*`/`_PARENT_*` collection (`Data.Includes[N].Mod`), and a WEAP
craftable-mod slot (`Object Template.Combinations[N]...Includes[0].Mod`).

## Mechanics digests: `walk` (interactive) and `chase` (pipeline JSON)

One rule: **read records with `walk`; `chase` is the machine contract.**

- `walk` is the interactive tool for *any* record type: one compact digest
  instead of a chain of raw `get` dumps. It resolves AVs/GLOBs/keywords to
  editor ids, prints curve points with the flat-wins rule applied, and falls
  back to a search when the selector doesn't resolve. `--json` serializes the
  same computed [`Digest`](../../src/walk/mod.rs) values it prints as text —
  one typed shape per record type (FormID ref stubs, numbers, classified
  mechanism hops), not plain-text lines wrapped in JSON.
  - **KYWD or AVIF root**: reverse-chases its SPEL/PERK consumers instead of
    dumping the mostly-empty record.
  - **OMOD root**: every mechanism is classified and rendered inline. A
    directly-attached ENCH or PROJ property is forward-fetched (`direct
    property → ENCH/PROJ …`) *and* followed into the BFS. Keyword/AVIF hooks
    are resolved by a reverse walk and rendered as *path-sliced* evidence rows
    — only the consumer's gated `Effects[N]` rows, so a hub perk's dozen
    unrelated effects never print. `AV hook → AVIF …` is that same
    reverse-chase rendering; what distinguishes it from a forward-fetched
    direct property is the hop's typed `resolution` field, not the target's
    record type. Perk grants render the granted perk's effect rows. The
    properties of an included mod template (`_PARENT_*`) are the includer's
    own and render under `from OMOD … _PARENT_…`; a mod collection
    (`modcol_*`) or selector lists its includes as `alternative →` lines
    (with any minimum level) and walks each as its own node. A hook keyword that no SPEL/PERK
    condition references renders a `dead end` note instead (tag keywords:
    `FeaturedItem`, `NonDroppable`, naming keywords, …).
  - **LVLI root**: resolves the actual drop odds as a tree bounded by
    `--depth` (default 1 for an LVLI root) — see "Drop-chance math" below.
  - **Loot-bag items** (ALCH/SPEL/ENCH or MGEF): an LVLI named by the effect
    MGEF's Papyrus script property (`script
    Creatures:FestiveGiftAddItem.FestiveLeveledList → LVLI …`) is walked as
    its own node, so the bag's drop odds print in the same call. `refs` on
    such an LVLI shows only the MGEF; the item sits one hop further back.
  - Raise `--depth` to 3 for OMOD → ENCH → MGEF → granted-perk; the root's
    mechanism slice renders at any depth.
  - For how to read `--refs` output, see "Obtainability verdicts" below.

**Worked example** (`mod_Legendary_Weapon1_DmgConsecutiveHits` / "Furious",
`0x004F577D`, a directly-attached ENCH property, a KYWD-hook property and an
included template's property — every mechanism, one call):

```
$ esm walk mod_Legendary_Weapon1_DmgConsecutiveHits --depth 3

▸ OMOD 0x004F577D mod_Legendary_Weapon1_DmgConsecutiveHits "Furious"
  direct property → ENCH 0x006C3173 ench_Legendary_Weapon_DmgConsecutiveHits  (ADD 1)
    Effects[0] AbLegendary_Weapon_DmgConsecutiveHits  Magnitude=0  Actor Value=LGND_Furious
    Perk to Apply → PERK 0x006C3175 Legendary_Weapon_DmgConsecutiveHits
  tags
      FeaturedItem
  keyword hook → KYWD 0x001EF480 HasLegendary_Weapon_DamageConsecutiveHits
    gates PERK 0x00578B06 LegendaryCommonWeaponPerkBACKUP
      Effects[11] Set Damage on Consecutive Hits/Set Value  Float=10  Conditions: WornHasKeyword(HasLegendary_Weapon_DamageConsecutiveHits) Equal To 1
      Effects[12] Mod Max Consecutive Hits Allowed/Set Value  Float=9  Conditions: WornHasKeyword(HasLegendary_Weapon_DamageConsecutiveHits) Equal To 1
      Effects[13] Mod Damage on Consecutive Hits/Set Value  Float=0.05  Conditions: WornHasKeyword(HasLegendary_Weapon_DamageConsecutiveHits) Equal To 1
  from OMOD 0x004519F4 _PARENT_mod_Legendary_Weapon_WEIGHTVALUE_1
    properties
        Value  MUL+ADD  1

▸▸ ENCH 0x006C3173 ench_Legendary_Weapon_DmgConsecutiveHits  (via OMOD property)
  effect[0] → MGEF 0x006C3174 AbLegendary_Weapon_DmgConsecutiveHits (Script)
    magnitude 0  duration 0
    Perk to Apply → 0x006C3175 Legendary_Weapon_DmgConsecutiveHits

▸▸▸ PERK 0x006C3175 Legendary_Weapon_DmgConsecutiveHits  (via Perk to Apply)
  ranks ?  playable False
  effect[0] Entry Point "Mod Max Consecutive Hits Allowed"  fn Add Value  value 9
  effect[1] Entry Point "Mod Damage on Consecutive Hits"  fn Add Actor Value Mult  value 0.01  AV 0x006C3172 LGND_Furious
```

Other mechanism headers you'll see: `perk grant → PERK …` (granted perk's
effect rows inline), `AV hook → AVIF …` (reverse-chased like a keyword
hook), `direct property → SPEL …` (forward-fetched; a row that also carries a
value shows it, e.g. `direct property → DMGT … dtCryo  (MUL+ADD 0.6)`), and a
`properties` block for rows with no record to follow
(`AttackDamage  MUL+ADD  -0.4, -0.4`: function, then `value1, value2`).

- `chase` is the pipeline evidence contract, not an interactive tool: one
  classified mechanism per `Data.Properties[]` row for an OMOD
  (`direct_property` / `perk_grant` / `keyword_hook`), an own-`Effects[]` walk
  for PERK/SPEL/ALCH/ENCH roots, and no search fallback. The JSON shape is
  stable — the patch-notes deep-writer parses it. Reach for it only when you
  need machine-parseable classification (scripts, fan-out agents).

**Gotcha — hub AVIF/KYWD blowup**: a property targeting a widely-read AV
(e.g. `Health`) makes the reverse hook-resolution return dozens of unrelated
consumers (survival hunger/thirst, Daily Ops mutations, unrelated legendary
armor perks). `--ref-limit` bounds it on both commands; walk's
KYWD/AVIF-root digest additionally caps display at 10 rows per consumer
type.

## Reading the digests

- **GLOB magnitudes — the flat-wins rule**: when an effect has a nonzero flat
  Magnitude AND a sibling Magnitude GLOB, the flat value wins and the GLOB is
  noise (survival-scale constants). The GLOB is the real value only when the
  flat magnitude is 0. The walk digest annotates each case; trust the
  annotations.
- **Curves**: `curve (x,y)…` with an input-axis AV (`curve INPUT axis: AV
  <name>`). Some engine AVs have no AVIF record (e.g. 0x392 healthFraction,
  0x395 onslaught stacks).
- **Conditions**: GLOB comparison values resolve inline
  (`GetRandomPercent() ≤ 0x…<SomeGlob=50>`). `WornHasKeyword(HasLegendary_*)`
  is a self-gate the OMOD's own keyword satisfies.
- **PERK with NO effects**: the bonus is engine/script-side; only the
  description states it.

## Damage-scope traps: character-wide vs weapon-scoped bonuses

- **PERK entry points that read like "weapon damage" are character-wide.**
  Entry point 167 (`Mod Weapon DMG Bonus Mult`) and siblings (`Mod Incoming
  Weapon Damage`, `Mod Target Damage Resistance`, `Mod My Critical Hit Damage
  Mult`, `Mod Percent Blocked`, `Mod Power Attack Damage`, `Mod Max Consecutive
  Hits Allowed`, `Mod Projectile Bounce Count`, `Apply Friendly/Combat Melee
  Hit Spell`) modify whatever damage instance is happening on the actor right
  now, not "this weapon's damage". A PERK granted via an OMOD's `Property
  116`/`Perk` (chase's "PERK grant") stays active while the granting item is
  equipped and applies to every simultaneous damage source — thrown
  grenades/mines, Pain Train, VATS crits, blocking. `MedicalMalpractice_Perk`
  (0x0050D7FD) is an entry-point-167 effect whose only conditions read the
  actor's own AV; nothing ties it to the granting weapon.
- **The fix, when the devs bother, is a self-referential `WornHasKeyword`/
  `HasKeyword` condition** naming a keyword unique to that item or roll —
  either its own `CustomItemName_X` naming keyword or a legendary-effect
  keyword like `HasLegendary_Weapon_APViaKill`. `RD01_Weapon_LicketySplit`
  gates `Mod Projectile Bounce Count` on
  `HasKeyword(RD01_CustomItemName_LicketySplit)`. A condition on a *shared*
  category keyword (`WeaponTypeRanged`, `HasSilencer`) or an unrelated AV
  (`KillStreak`) is NOT a self-scope — the bonus still leaks, just narrower.
- **OMOD `Property 106` (`DamageBonusMult`) is the same character-wide hook as
  entry point 167 — not a per-weapon stat**, despite living on the weapon's own
  OMOD. Its `Value Type` is a bare `Float` with no AV/FormID pointer,
  consistent with a hardcoded engine target rather than a WEAP field.
  `mod_Legendary_Weapon1_Guns_TwoShot` sets `DamageBonusMult +0.75` — ordinary
  "Two Shot" rolls carry this leak, not just one weapon.
- **`Property 28` (`AttackDamage`) and `Property 77` (`DamageTypeValues`) DO
  write into the WEAP record's own fields** (`Data.Base Damage` / top-level
  `Damage Types[]` — see Curve tables below) and are genuinely weapon-scoped.
  `Property 94` (`ActorValues`) sits in between: a while-equipped character AV
  bonus (`mod_Custom_CivilUnrest` → +50 Action Points) — same equip-gated scope
  as the PERK path, but usually not a damage stat, so it doesn't compound like
  `DamageBonusMult`/entry-point-167.
- **A `Property 65` (`Enchantments`) attach can go either way — check the
  target MGEF's `Casting Type`/`Delivery`/`Archetype`, not just that an
  enchantment exists.** `Archetype: Damage`, `Casting Type: Fire and Forget`,
  `Delivery: Contact` is a genuine on-hit proc fired by that weapon's own
  attack — safe by construction (e.g. `TheKabloom`'s poison DoT). `Casting
  Type: Constant Effect`, `Delivery: Self` is a standing character buff — same
  leak risk as entry point 167 unless gated by a condition true only while
  wielding that weapon (e.g. `GetInIronSights`, since you can't ADS with a
  grenade). No condition at all is a confirmed leak — e.g. `ench_ThePeacemaker`
  (`STAT_DmgExplosive`, Constant Effect/Self, zero conditions).
- **Cross-check a suspiciously broad AV against sibling AVs before assuming
  it's "the everything" stat.** FO76 has both `STAT_DmgExplosive` (every
  explosion: Fat Man, launchers, mines, grenades) and a separate, narrower
  `STAT_DmgGrenade` (thrown grenades only). Chase the actual `Actor Value`
  FormID on the MGEF; don't infer scope from the AV editor-id prefix alone.
- **A timed buff can outlive the weapon that triggered it.** An unconditioned
  entry point that selects a Spell (`Apply Friendly Hit Spell`, `Apply Combat
  Melee Spell`) is bad enough on its own, but if the Spell's effect carries a
  duration — the perk's Description often states it ("for 30 Seconds") even
  when the record's own `Duration` field reads empty — swapping weapons *after*
  the proc doesn't end the buff. Contrast an instantaneous on-target proc like
  a bleed DoT, which applies once with no persistent buff.
## Perk rank verification (PCRD)

**PCRD is the perk-card source of truth, not the PERK rank chain.** Each card carries a `Special`
enum, per-rank `Card Rank Cost`, a `Race Restriction`, and a `Perks[]` array of rank → PERK
FormIDs. `Perks[]` reflects the live, rebalanced card shape — a card's effective max rank clamps
DOWN to its entry count. Ranks have been compressed without the PERK EditorID numbering being
updated, so counting PERK records is not a reliable rank count.

Rank chains and ability SPELs linger as cut content after a compression, including
engine-attached-looking orphaned spells. Cross-check any rank or orphaned spell against `Perks[]`
(and `refs` the spell to see whether a live rank references it) before treating a record-graph tier
as live. Confirmed via the "Lock and Load" family: PCRD lists 1 rank against a 3-rank record chain
plus a cut orphaned spell.

**Authoritative check for perks specifically: does any `PCRD` reference it?** A `PERK` rank is
only reachable by a player if some `PCRD`'s `Perks` array lists it — verify with
`esm refs <perk-formid> --limit 20` (look for a `PCRD` in the results), then
`esm get <pcrd-formid> --resolve stub --pretty` (inspect `Perks[].Perk["Male Perk"]` — only these
ranks are live). A `PERK` with no referencing `PCRD` (e.g. `Deadeye01/02`, `Bandito01`) is
orphaned — it decodes fine and may even carry `Playable: true`, but nothing in the game ever
grants it to a player. A `PCRD` whose `Perks` array stops at rank N means ranks N+1 onward (even
if unprefixed, even if not `CUT_`) are dead.

## Interpreting game data: live vs. cut/deferred content

This is guidance about the game data itself, not the tooling — it matters whenever a query is
answering "what does X do in the current game."

FO76's EditorIDs use informal prefixes to mark content that isn't part of the live game:

- **`zzz_`** — deprioritized/superseded. Usually an older implementation of a perk/effect that's
  been reworked; the unprefixed sibling (if any) is the live one.
- **`CUT_`**, **`DEL_`**, **`deprecated_`** (or similar) — cut content, never shipped or removed.
- **`POST_`** — deferred/not-yet-released content (future update material sitting in the current
  ESM).
- **`zzz_Babylon_*`** — an internal test-branch duplicate, not the live record.

Treat these as a heuristic for historical/comparative tasks (diffing snapshots, tracing how a
mechanic evolved, investigating cut content on request) — not as ground truth for "what does the
game currently do."

**The prefix is a heuristic, not proof — the naming convention is inconsistent.** Several
currently-dead PERK ranks have *no* prefix at all (e.g. `BearArms02`/`BearArms03`,
`TankKiller03` — plain names, but orphaned). Conversely, some unprefixed records are simply
broken/vestigial (e.g. `BearArms01` has a description string copy-pasted from an unrelated perk
and carries no `Conditions`/`Effects` at all). Naming alone doesn't confirm a record is live —
the PCRD check above is the reliable signal for perks specifically; other record types don't have
as clean an authoritative signal, so flag uncertainty rather than asserting liveness.

## Drop-chance math (LVLI chains)

**`esm walk <lvli-selector>` computes this automatically** (`src/lvli.rs`) —
pool/`Use All`/`Use First Match` selection, flat-vs-GLOB-vs-Curve-Table
chance-none, and recursion through nested sublists to leaf items. Reach for it
instead of hand-tracing a chain.

Read the text like `du -d`. `--depth` counts record hops plus nesting levels
(default 1 on an LVLI root; an LVLI reached through a record hop expands
whatever depth remains, always at least its direct entries). Every number is per
roll of the root list:

- `▸ LVLI … (pool, 419 items)` is a collapsed sublist: `expected` is items out
  of it, `p(>=1)` the chance it yields anything. Raise `--depth` to open it.
- `▾ LVLI …` is expanded; its entries follow, indented.
- The `notes` column holds footnote numbers, explained once under the table;
  `list notes` on the header line apply to every entry of the root list. A
  collapsed row's numbers include caveats from inside it.
- `--json` carries the same tree under `table.tree` plus `table.rows`, which
  always flattens every level to leaf items (a leaf reached by one path matches
  its tree row).

The rules below are for reading that output, and for the handful of things it
deliberately doesn't model (`Filter Keyword Chances`, `Epic Loot Chance`,
list-level `Max Count`/`Max Global`/`Max Curve Table`, COED owner/rank gates) —
those surface as notes rather than being silently guessed.

- **A zero `Chance None Value` does not mean "guaranteed" — check the sibling
  `Chance None Global` on the same entry.** Each `Leveled List Entry` carries
  both a flat `Chance None Value` and an optional `Chance None Global` FormID;
  the flat value wins when nonzero, otherwise the GLOB is the real chance-none
  (the same flat-wins rule as MGEF magnitudes). Reading only the flat field
  makes gated rewards look like 100% drops: `TWZ07_LL_QuestReward_Event` reports
  flat `0.0` but points at `RA_Rewards_Activities_UniqueWeapon_DropRate_Cnone`
  = 85, i.e. a 15% drop. The *list*-level `Chance None Value` has no GLOB
  sibling — that one really is flat. A `Chance None Curve Table` sibling, when
  present, outranks both.
- **Flags decide how entries combine, and neither no-flag nor `Use First Object
  That Matches All Conditions` is a 1/N pick.** `Use All` rolls every entry
  independently (multiply the per-entry miss chances). **No flag is a pool, not
  a uniform pick from the entry count**: every entry's own gate rolls
  independently, the passing subset is pooled, and one member of *that subset*
  is chosen uniformly — so an entry's real odds depend on how many siblings are
  passing at the same time. Confirmed against
  `SCORE_S22_Resources_Collector_SoulSoupServer_Food` (0x008308D7): six entries
  on a descending `GetRandomPercent ≥ {92,80,63,45,25}` ladder plus an
  unconditioned catch-all read like a hand-authored 8/12/17/18/20/25% split,
  but the *actual* pool odds are 2.20/5.66/10.96/17.19/25.22/38.77% — the
  rarest item is ~3.6× rarer than the ladder implies, because it only wins when
  it is the *sole* passing entry. `Use First Object That Matches All
  Conditions` instead walks entries in order and takes the first whose
  conditions pass, so an entry gated on `GetRandomPercent ≤ 10` genuinely is a
  flat 10%, and later entries are reachable only when every earlier gate fails
  (their true probability is the product of the preceding misses).
- **Entry-level `Conditions` gate the roll too** — `GetRandomPercent ≤ N` (flat
  or GLOB comparison value) and `HasLearnedRecipe(...) == 0` are the common
  ones. The recipe check is why plan-then-weapon lists hand out the plan first:
  `RD01_LLS_Raids_Rewards_Enc01_Weapons_Valkyrie` is `Use First Match` with the
  BOOK at `rand ≤ 5` (while unlearned) ahead of the weapon LVLI at `rand ≤ 10`.
  Any gate that isn't `GetRandomPercent` (`GetLevel`, `HasLearnedRecipe`, …) is
  real but not a probability the tool can compute — it renders as a "can't be
  computed — assumed to pass" note, assume-pass by default (`--strict` is internal, not exposed on `walk`).
- `Quantity: 0` on an entry means "use the sublist's own count", not "disabled"
  — creature death-item lists are full of them.
- **A `Minimum Level`/`Minimum Level Global` above the assumed player level
  (`--level`) excludes an entry outright.** Whether FO76 further collapses
  multiple *qualifying* Minimum Level tiers down to the highest one when
  `Calculate from all levels <= player's level` is unset is unverified here —
  `walk` shows every qualifying tier and flags the ambiguity rather than
  picking a side.
## OMOD `Data.Includes[]` — inherited properties

- **An OMOD's own `Properties[]` is only half the story: `Data.Includes[]`
  pulls in `_PARENT_` building blocks whose properties also apply.** Sweeping
  `Properties[]` alone silently drops real mechanics. Confirmed: the "Black
  Diamond" Ski Sword's `mod_Custom_BlackDiamond` carries nothing but three
  keyword rows, yet includes `_PARENT_mod_WEAPON_GENERIC_Cryo_Split2`
  (`AttackDamage` −40%, `DamageTypeValues dtCryo` +60%) — the entire physical→cryo
  split lives in the parent. In a 90-weapon sweep, 60 weapons had properties
  reachable only through `Includes[]`. `walk` and `chase` fold an included
  template's properties in (`from OMOD …` / `source_omod`), so read those rows
  before concluding an OMOD "does nothing".
- **A mod collection's or selector's includes are alternatives, not parents.**
  The record flags say which: `Mod Collection` (`modcol_*`) and `Mod Selector`
  OMODs have no properties and include the mods they pick among (collections
  gate each by `Minimum Level`); every other OMOD's includes are `Mod Template`
  building blocks. `chase` lists a collection's includes as `alternatives`;
  chase each one separately.
- The `dn_UniqueEffect<Type>Damage` keyword family is a **display label**, not a
  mechanism — when it appears with no matching damage row, the damage is in an
  included `_PARENT_` mod, not engine-side magic. Chase `Includes[]` before
  writing something off as cosmetic.
- Distinguish **identity mods from stock mods**. A unique weapon's shipped roll
  drags in ordinary receiver/barrel/magazine mods that inherit their own parents
  (`_PARENT_mod_WEAPON_GENERIC_Damage_Tier2` = `DamageBonusMult +0.35`,
  `ArmorPiercing_Dual` = `ArmorPenetration +25`,
  `Receiver_Automatic_BaseProperties` = −30% across every damage type). Those are
  properties of the *base weapon's mods*, not of the unique — don't credit them
  as the unique's effect.

## Entry-point names can be wrong (FO4 enum inherited wholesale)

Entry-point 28 decodes as `Mod Power Attack Damage` but is really the **block**
hook in FO76: both `NailerPerk` ("Blocking Attacks Inflicts Bleeding") and the
`LGN_Retribution_Perk01` legendary armor perk ("Blocking a melee attack restores
1 HP and 1 AP") route through it with `Select Spell`. When an entry point's name
contradicts every consumer's own Description, trust the descriptions and treat
the schema name as inherited-from-FO4 drift. Some entry points have no name at
all in the current schema (e.g. the one `mod_custom_V63-BERTHA_Perk` uses) —
report the numeric id and the value rather than guessing.

`refs --ep <id>` (see above) is the tool this section presupposes: it
enumerates every consumer of an entry point so you can actually compare their
Descriptions instead of guessing from the schema name alone. Prefer the
numeric id over the name when a name's trustworthiness is in doubt — `--ep 28`
finds the same two carriers regardless of what the name claims, and `--ep 212`
still works for an id with no name at all.

## Obtainability verdicts (`walk --refs`)

- Player-facing referrer types: COBJ, GMRW, LGDI, QUST, CONT, MISC, FLST.
  LVLI counts only through player-facing chains (NPC-loadout-only lists
  don't); modcol OMOD chains and obtainable-WEAP inheritance count too;
  referrers with NONPLAYABLE in the editor id are flagged.
- **No reverse refs at all is normal** for script/VMAD quest rewards,
  vendor/gold-bullion grants, and account-side (ATX) items — absence of refs
  is not evidence of junk. Same goes for keyword-attached stock mods (a mod
  gated purely by a `WornHasKeyword`/`HasKeyword` condition has no direct
  record-level referrer to the item it modifies).
- **The record graph cannot distinguish shipped from unshipped content** —
  cut or unreleased items can look perfectly obtainable on-record. Confirm
  release status externally before treating an unfamiliar record as real.
- **COBJ eligibility is a trap: a recipe existing does not mean a weapon is
  eligible for it.** COBJ records carry no CTDA/BNAM naming the target
  weapon — the join has to come from the target's own keywords/eligibility,
  not from "a COBJ referencing this exists." `Learn Recipe From` is
  polymorphic by `Learn Method`: `4` → the recipe is learned from a plan
  BOOK, `1` → learned from a scrap source (the WEAP/item itself, i.e.
  self-scrapping teaches it). A `Learn Recipe From` pointing at the dummy
  MISC `recipe_Dummy_Uncraftable_Item_NOCRAFT` is a field-based "this is
  NOCRAFT" signal — don't treat it as a real learn source. `Repair Method 5`
  is NOT a nocraft signal (common false-positive read). `CUT_`-prefixed
  EditorIDs are a junk-referencer convention — a record referenced only by
  `CUT_*` stand-ins is not evidence of a live path. For a mod that's gated by
  a "mod-box" MISC (a physical unlock item), the mod is slottable exactly
  when a matching mod-box MISC is present in inventory — the OMOD/COBJ graph
  alone won't show that gate.

## Effect-graph reachability (grant roots & dead-end AVs)

Item obtainability (above) and effect liveness are different questions: an
obtainable item can carry effect arms that never fire. Before treating any
effect arm as live, traverse both directions and refs-check the **ends**:

- **The leaf-refs trap.** In a grant chain (OMOD/ENCH → Perk-to-Apply MGEF →
  PERK → Ability SPEL → Perk-to-Apply MGEF → PERK), every hop references the
  next — so `refs` on any *middle* node always shows exactly one referrer and
  looks alive. Only the **root** (the ENCH/SPEL that something must attach,
  or the OMOD property that must exist) tells the truth. A keyword-gated
  apply-perk whose gate is satisfiable still does nothing unless something
  actually grants the perk. Worked case (20260821 dump): Pin-Pointer's
  `ench_LegendaryWeapon_PinPointers` 0x007ACA02 has zero references — its
  whole ENCH→AddPerk→SPEL→ApplyPerk island (a +50% striking-appendage row)
  is authoring debris; the shipped OMOD wires only a keyword and a STAT_* AV.
  Recipe: `walk <leaf>` upward to the root, then `refs <root formid>`.
- **Dead-end AV writes.** An `ActorValues` write is only meaningful if the AV
  has a downstream consumer — a PERK/MGEF entry point reading it (`Multiply
  1 + Actor Value Mult`, curve input, condition). `refs <AVIF>` with only
  writers and no readers ⇒ a native-engine flag or VFX enable (KillStreak
  counters, VATSCriticalMultAdjust bounds, BloodyMess gib toggles), not a
  stat.
- **Neither direction is sufficient alone.** Zero refs can still be live
  (script/VMAD grants, above) and a fully-wired graph can be unshipped
  (P62). Cut-marker naming (`CUT_`/`zzz_`/`POST-`/`DEL`) and release history
  break the ties — copy-paste debris names on intermediate records (a spell
  named after a different legendary) are a strong orphan-island tell.

## Curve tables

- **A `Curve Table` sibling means the flat scalar next to it is NOT the
  effective value — the curve is.** This holds wherever the pair appears:
  `Effects[].Effect.Effect Item Data.Magnitude` on ENCH/SPEL, `Value 2` on an
  OMOD property, `Amount` on a `Damage Types[]` entry. Read the inlined
  `curve` points, not the scalar.
  - **`Magnitude: 0.0` with a `Curve Table` present does not mean "grants
    nothing."** It reads as an inert or cut effect and isn't — never call an
    effect dead or unaffected by a balance change on a zero magnitude without
    checking for a sibling `Curve Table`. Example: `MoM_ench_GarbofMysteries`
    (0x0052192E) `Effects[1]` has `Magnitude: 0.0` but curve
    `CT_Armor_MoM_GarbofMysteriesSneak` resolves to **5 → 20**.
  - **The curve's input axis is the sibling `Actor Value`, and it is often not
    a level.** Creature-damage and armor-mod curves key on wielder/item level,
    but some key on a gameplay AV with a tiny domain — the Garb curve above
    keys on `MoM_EyeOfRa` over `{0, 1}`, a set-bonus toggle rather than a
    level ramp. Resolve the `Actor Value` before describing what moves the
    number; an AV nothing in the ESM writes is engine-side — say so rather
    than guessing a trigger.
  - `get --resolve stub --pretty` already inlines the `curve` points,
    `curve_path`, and keying `Actor Value` on the same effect — one call
    gives you everything above; don't quote a bare magnitude.
  - **LVLI's own curve-table siblings don't all key on player level.** A
    `Chance None Curve Table` or `Quantity Curve Table` on a `Leveled List
    Entry` reads level-shaped in spot-checked data (`Container_Item2_
    ChanceNone`: x 0-100, y falls 100→0; `CT_Creatures_Loot_WeaponUser_
    Steel_Base`: x 1-50, y climbs 3→7) and `esm walk`'s drop-odds digest
    (`src/lvli.rs`) evaluates both at `--level`. The `Minimim Level Curve
    Table` sibling (schema typo, present as-is) does not — `MinLevel_Armor_
    Metal_CT`'s points `(0,1)(1,10)(2,25)(3,35)(99,35)(100,100)` read as an
    item-quality-tier index (0-3, with 99/100 sentinel rows), not a level.
    `walk` deliberately does not evaluate it — treat any "min level curve"
    finding the same way: check whether the x-domain actually looks
    level-shaped before assuming it does.
- Out-of-domain inputs clamp to the curve's own first/last point — no
  extrapolation, no implied zero. A zero floor is an authoring choice encoded
  as an explicit `{x:0, y:0}` point; some legitimate curves deliberately omit
  it. Never "fix" clamp behavior engine-side; if a zero floor seems missing,
  that's an ESM-data question.
- Curve resolution needs `<dump>/misc/curvetables/json/` next to the ESM.
  Missing curvetables degrade silently: `Damage Curve` refs stay raw formids
  and curve-driven values vanish. If a fresh dump lacks the dir, copy it from
  the previous dump (tier tables rarely change); the next query picks it up.
- WEAP records may include a derived `"Bash Damage"` object (top-level sibling
  of `Data` and `Damage Curve`, not inside `Data`). It is computed automatically
  during decode — no CLI flag — from `Data.Secondary Damage` and the primary
  `Damage Curve`:
  `bash_damage(level) = Secondary Damage × [primary_curve(level) ÷ primary_curve(1)]`.
  The `source` field is one of:
  - `"curve"` — table present under `curve` as `[{level, damage}, …]`, following
    each weapon's own curve domain (uncapped; creature/NPC tiers run past 50).
  - `"ineligible"` — secondary damage and a resolved curve exist, but the weapon
    is not eligible (not `Weapon Type` = Gun and lacks the
    `WeaponTypeAutomaticMelee` keyword, `0x006D5081`). Ground-truthed via the
    "Stable Tools" perk's `HasKeyword` condition — power tools: Auto Axe,
    Chainsaw, Drill, Ripper, Buzz Blade.
  - `"unresolved_curve"` — `Damage Curve` is a bare FormID (curves not loaded).
  - `"curve_zero_reference"` — level-1 primary curve evaluates to zero; no
    damage table is emitted (avoids non-finite/null values).
  Records with zero/absent secondary damage, or no damage curve at all, stay
  silent (no `"Bash Damage"` key). Distinct from `"Bash Condition Loss Scale"`,
  which is a durability wear-rate curve, not bash damage.
- **`esm curve <edid|formid>... [--at X ...] [--sum FROM TO [--step N]]`** reads
  any `CURV` record's points and can interpolate/sum them — reach for it instead
  of writing an inline interpolation script for a one-off table (leveling/XP
  progression curves etc.) that isn't part of `curvelookup.py`'s hardcoded
  tiered-file set. `--at`/`--sum` are mutually exclusive; multiple targets emit
  a JSON array tagged with `sel`; it errors clearly if curve tables aren't
  loaded rather than reporting zero points.
- **`Data.Base Damage` is the weapon's physical-damage value**, overridden by
  a top-level `Damage Curve` (sibling of `Data`) when that curve resolves to
  real points; if curvetables are missing the curve stays a raw FormID and
  `Base Damage` is the effective value (see the missing-curvetable note
  above). Verified via a full 1549-record WEAP sweep.
- **`Damage Types[]` (DTVL) is a separate top-level array**, also a sibling
  of `Data`, adding non-physical components (energy/fire/poison/cryo/
  radiation/electrical). It commonly *stacks* with physical `Base Damage`
  rather than replacing it: `PlasmaGun` (24 physical + `dtEnergy` curve),
  `Shishkebab` (13 physical + `dtFire` 13), `RadiumRifle` (27 physical +
  `dtRadiationExposure` curve) all deal both at once — don't assume
  either/or. Each entry has `Type`, a scalar `Amount` fallback, and an
  optional `Curve Table` override, but the curve does NOT reliably zero the
  `Amount`: 43% of resolved-curve DTVL entries in the sweep also carry a
  nonzero `Amount`, so don't assume curve-replaces-scalar without checking
  the specific record. `Type` is normally elemental but CAN be `dtPhysical`
  — one live case, `crSuperMutantBoss_AssaultRifle_DailyOps_Boss`
  (`Base Damage: 0`, damage entirely via a `dtPhysical` DTVL curve) — rare,
  not purely theoretical.
- **A WEAP record can carry no damage fields at all and still deal damage**
  — e.g. `GammaGun`: `Base Damage: 0`, no `Damage Curve`, no `Damage Types`
  field. Its real damage lives on the downstream `EXPL` record reached via
  `Data.Ammo` → AMMO `Projectile` → PROJ `Explosion` → EXPL, which carries
  its *own* top-level `Damage Types[]`. Chase the ammo/projectile/explosion
  chain (not an Enchantment/MGEF) when a WEAP record itself is a dead end.

## Field-name churn

Decoded field names come from the schema layer and can change across
rebuilds — the same WEAP field has been `Min Power Per Shot`, `Max Power Per
Shot`, and `Full Power Damage Mult` at different times. After any esm
rebuild, re-dump one known
record (e.g. `esm get GaussRifle`) and grep the actual field names before
trusting fixtures, extractor code, or prior notes.
