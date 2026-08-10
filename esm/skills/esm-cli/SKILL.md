---
name: esm-cli
description: Using the FO76 `esm` CLI effectively — invocation modes, bulk get, refs gotchas, walk/chase mechanics digests, obtainability verdicts, curve-table conventions, field-name churn. Use when querying SeventySix.esm records, decoding a perk/OMOD/legendary mechanic, or wrapping the CLI in scripts.
---

# esm CLI knowledge

Hard-won usage knowledge for the `esm` CLI (FO76-Tools/esm). This file ships
embedded in the binary: `esm skill` prints it, `esm skill --install` writes it
into a consumer repo's `.claude/skills/esm-cli/`. The binary is authoritative —
this crate changes fast, so re-verify anything here against `esm --help` /
`esm <subcommand> --help` before documenting or wrapping a subcommand.

## Invocation & path resolution

- Subcommands: `daemon, cache, info, get, list, search, refs, tree, diff,
  coverage, chase, walk, skill`.
- ESM path comes from the global `--esm <PATH>` flag (long only; works before
  or after the subcommand) with `FO76_ESM_PATH` env fallback — a plain process
  env var, there is no `.env` parser. The path may be the `.esm` file or its
  containing data folder. Exceptions: `diff <FILE_A> <FILE_B>` (two
  positionals), `daemon` (no path; resolves at spawn), and `skill` (no path —
  it only reads its own embedded doc).
- Every subcommand is one-shot: it auto-spawns/uses a warm daemon and exits
  after printing. There is no interactive mode — a missing subcommand is a
  usage error, not a REPL. `--local` runs cold in-process (seconds per open —
  never use it for bulk work).
- Rebuilding the binary self-heals the daemon (a size+mtime fingerprint check
  respawns it automatically) — no manual step needed. Changing loose files
  next to the dump (strings/curvetables) does *not* self-heal: run
  `esm daemon stop` after adding or changing those.
- A cold call against an ESM with no `esm_cache/` yet can take tens of
  seconds to a couple of minutes (worst case: `refs`/`walk`/`chase` on a
  first-ever query, which builds the `xref` index — a full schema decode of
  every record). It shows live progress on stderr while waiting rather than
  hanging silently, then still returns the real result — no flag needed. Use
  `esm cache status [--json]` to check what's built/building without
  triggering anything, or pass the global `--no-wait` flag to bail (exit 75)
  instead of blocking when a build is already in flight — useful in a script
  that would rather retry later. A second concurrent query against the same
  ESM waits on (and reuses) whichever build is already running rather than
  starting a redundant one.

## Fetching records

- Selectors are `0x...` formids or EditorIDs. Bare tokens that *look* numeric
  auto-resolve FormID-first with EditorID fallback; scripts should still pass
  explicit `--formid`/`--edid` where the flag exists to skip the ambiguity.
- **Bulk get**: `esm get <sel1> <sel2> … --json` — one target returns the
  classic single object; 2+ return a JSON array in input order, each entry
  tagged with its own `sel`. A bad selector becomes `{"sel":…, "error":…}` in
  the array instead of failing the call (the single-target form throws).
- `get --resolve none|stub|full` inlines FormID references — `stub` gives
  `{formid, editor_id, record_type}` per ref (cheap); `full` recursively
  inlines the record. A CURV record always inlines its own curve points.
- `list` never returns display names — use `search --in name` or `get`.
  `search` needs `"*"` to match all (`""` matches nothing).
- `--limit 0` means unlimited for `list`, `search`, and `refs`.

## Reverse references (`refs`)

- `esm refs --formid <0x...> [--depth N] [--type SIG] [--paths] --json`.
- `--depth` 1–8 (0 = unbounded, use with `--limit 0` and a `--type` filter — an
  unbounded walk over a hub-heavy graph like CELL/REFR can return hundreds of
  thousands of rows): direct referrers at 1; raise it to reach a target through
  an intermediary (e.g. a quest alias).
- `--type <SIG>` filters to ONE 4-char referrer type server-side (not a
  comma list) and composes correctly with `--limit`/`--depth`.
- `--paths` annotates each row with the JSON field path(s) from referrer to
  target (e.g. `Effects[2].Conditions[0].Parameter 1`). It decodes every
  emitted row, so it's opt-in.
- The default `--limit 100` truncates popular targets. The "output capped"
  note goes to **stderr** — stdout stays valid JSON under `--json`. Pass
  `--limit 0` when you need everything.
- `--entry-point <name|id>` (alias `--ep`) answers "what uses this hook?" —
  the reverse of reading a PERK's own Entry Point off `get`/`walk`. It
  resolves to every PERK carrying that entry point, each emitted as its own
  `depth: 0` row (a `D` column appears in table output), then walks refs from
  all of them at once: `esm refs --ep 'Mod Percent Blocked'` surfaces the
  Blocker perks, the Ogua Gauntlet/Defender's leggo perks, and (one more
  `--depth`) the OMODs/weapons/perk cards that reach them. Matching is exact
  and case-insensitive unless the value contains `*` (`--ep 'Mod VATS*'`
  fans out across every VATS entry point). A bare positional target also
  auto-detects an entry-point name when it isn't a real EditorID — `refs
  'Mod Percent Blocked'` works without the flag — but a real EditorID always
  wins over a same-named entry point. Numeric ids matter: some entry-point
  *names* are wrong (see below), and a few ids have no name in the schema at
  all — `--ep 212` still finds its (unnamed) carrier, `--ep 0x...` is
  rejected (that's a FormID, not an entry point).
- **EP attribution (glob-aware):** a multi-match glob's stderr legend lists
  every matched entry point as `id name` (e.g. `entry point 'Mod Weapon*'
  (2 matched: 44 Mod Weapon Reload Speed, 45 Mod Weapon Spread)`). When more
  than one distinct id appears in the printed rows, an `EP` column shows
  comma-joined numeric ids per row (carriers grouped by primary EP; BFS rows
  inherit the originating carrier's tag). `VIA` is populated from depth 1 in
  EP mode and starts with the originating carrier FormID. Attribution is
  first-reach at minimum depth; equal-depth ties **union** EP tags (so a
  record referenced by two carriers shows both ids) rather than picking one
  arbitrarily — but an overlap first discovered at depth ≥ 2 that was already
  reached shallower by a different carrier stays attributed only to the
  shallower carrier. Caveat: carriers are emitted before referencers, so a
  broad glob at the default `--limit 100` may show only carrier rows — use
  `--limit 0` (or a larger limit) for EP walks when you need the referencers.
- `--omod-property <[scope:]name-or-id>` (alias `--prop`) answers "which
  OMODs declare this property?" — then walks refs from all of them at once.
  Each matching OMOD is a `depth: 0` carrier row; a `PROP` column shows
  `scope:id` (e.g. `weap:31`) when more than one distinct tag appears.
  Syntax:
  - **Scope-qualified** — `--prop weap:Speed` / `--prop weap:31` narrows to
    one enum space (`weap` / `armo` / `npc`). Property ids are only
    meaningful inside one space.
  - **Bare name** — `--prop Keywords` matches across all three spaces;
    each carrier is tagged with which space it came from, and a stderr
    legend names every space that matched (same shape as a multi-match
    `--ep` glob legend).
  - **Bare numeric id is rejected** — `--prop 31` errors; write
    `--prop weap:31`. An id alone is ambiguous across spaces.
  Name matching is case-insensitive and whitespace-insensitive
  (`ActorValues` and `'Actor Values'` both match). Cross-space name
  collisions (same spelling, different ids per space) include at least:
  `Keywords` (weap:31 / armo:3 / npc:0), `Enchantments` (weap:65 / armo:0 /
  npc:3), `Perk` (weap:116 / armo:18), `ActorValues`/`Actor Values`, and
  `Health`. Unlike `--ep`, `--prop` is **flag-only** — never auto-detected
  from a bare positional (`Health` is also a real AVIF EditorID; a bare
  `refs Health` must keep resolving to that AVIF). Caveat: carriers are
  emitted before referencers, so a broad `--prop` (e.g. `Keywords`,
  ~7,500+ carriers) at the default `--limit 100` may show only carrier
  rows — use `--limit 0` (or a larger limit) when you need the referencers.

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

- `esm walk <selector> [--refs] [--depth N] [--ref-limit N] [--json]` —
  the interactive tool for *any* record type: one compact digest instead of
  a chain of raw `get` dumps. `--json` serializes the same computed
  [`Digest`](../../src/walk/mod.rs) values `walk` prints as text — one typed
  shape per record type (FormID ref stubs, numbers, classified mechanism
  hops), not the plain-text lines wrapped in JSON. Resolves AVs/GLOBs/
  keywords to editor ids, prints curve points with the flat-wins rule
  applied, and falls back to a search when the selector doesn't resolve.
  Walking a KYWD or AVIF root reverse-chases its SPEL/PERK consumers instead
  of dumping the mostly-empty record. On an OMOD root every mechanism is
  classified and rendered inline: a directly-attached ENCH or PROJ property
  is forward-fetched (`direct property → ENCH/PROJ …`) *and* followed into
  the BFS; keyword/AVIF hooks are resolved by a reverse walk and rendered as
  *path-sliced* evidence rows (only the consumer's gated `Effects[N]` rows —
  a hub perk's dozen unrelated effects never print, and `AV hook → AVIF …`
  is the same reverse-chase rendering, distinguished from a forward-fetched
  direct property by the hop's typed `resolution` field, not by
  eyeballing the target's record type); perk grants render the granted
  perk's effect rows; and `Data.Includes[]` stubs are named so `_PARENT_*`
  empty-shell OMODs point at the include carrying the real mechanic.
  A hook keyword no SPEL/PERK condition references renders a `dead end`
  note instead (tag keywords: `FeaturedItem`, `NonDroppable`, naming
  keywords, …). `--depth` caps BFS chain-following (default 2; use 3 for
  OMOD → ENCH → MGEF → granted-perk); the root's mechanism slice renders at
  any depth. `--refs` appends grouped reverse references (see obtainability
  below). On an LVLI root, `walk` resolves the actual drop odds instead of
  dumping raw entries — see "Drop-chance math" below; `--level N` (default
  50) feeds Curve Table evaluation and Minimum Level filtering.

**Worked example** (`mod_Legendary_Weapon1_DmgConsecutiveHits` / "Furious",
`0x004F577D`, a directly-attached ENCH property plus a KYWD-hook property —
both mechanisms, one call):

```
$ esm walk mod_Legendary_Weapon1_DmgConsecutiveHits --depth 3
▸ OMOD 0x004F577D mod_Legendary_Weapon1_DmgConsecutiveHits "Furious"
  direct property → ENCH 0x006C3173 ench_Legendary_Weapon_DmgConsecutiveHits
    Effects[0] AbLegendary_Weapon_DmgConsecutiveHits  Magnitude=0  Actor Value=LGND_Furious
    Perk to Apply → PERK 0x006C3175 Legendary_Weapon_DmgConsecutiveHits
  tags
      FeaturedItem
  keyword hook → KYWD 0x001EF480 HasLegendary_Weapon_DamageConsecutiveHits
    gates PERK 0x00578B06 LegendaryCommonWeaponPerkBACKUP
      Effects[11] Set Damage on Consecutive Hits/Set Value  Float=10  Conditions: WornHasKeyword(HasLegendary_Weapon_DamageConsecutiveHits) Equal To 1
      Effects[12] Mod Max Consecutive Hits Allowed/Set Value  Float=9  Conditions: WornHasKeyword(HasLegendary_Weapon_DamageConsecutiveHits) Equal To 1
      Effects[13] Mod Damage on Consecutive Hits/Set Value  Float=0.05  Conditions: WornHasKeyword(HasLegendary_Weapon_DamageConsecutiveHits) Equal To 1
  include → OMOD 0x004519F4 _PARENT_mod_Legendary_Weapon_WEIGHTVALUE_1

▸▸ ENCH 0x006C3173 ench_Legendary_Weapon_DmgConsecutiveHits  (via OMOD property)
  effect[0] → MGEF 0x006C3174 AbLegendary_Weapon_DmgConsecutiveHits (Script)
    magnitude 0  duration 0
    Perk to Apply → 0x006C3175 Legendary_Weapon_DmgConsecutiveHits

▸▸ OMOD 0x004519F4 _PARENT_mod_Legendary_Weapon_WEIGHTVALUE_1  (via include of mod_Legendary_Weapon1_DmgConsecutiveHits)

▸▸▸ PERK 0x006C3175 Legendary_Weapon_DmgConsecutiveHits  (via Perk to Apply)
  ranks ?  playable False
  effect[0] Entry Point "Mod Max Consecutive Hits Allowed"  fn Add Value  value 9
  effect[1] Entry Point "Mod Damage on Consecutive Hits"  fn Add Actor Value Mult  value 0.01  AV 0x006C3172 LGND_Furious
```

Other mechanism headers you'll see: `perk grant → PERK …` (granted perk's
effect rows inline), `AV hook → AVIF …` (reverse-chased like a keyword
hook), `direct property → SPEL …` (forward-fetched), and bare-number
properties as before.

- `esm chase <selector> [--depth N] [--ref-limit N]` — the pipeline
  evidence contract, not an interactive tool: always emits classified
  mechanism JSON (`direct_property` / `perk_grant` / `keyword_hook` per
  `Data.Properties[]` row for an OMOD; an own-`Effects[]` walk for
  PERK/SPEL/ALCH/ENCH roots; hard error on any other type, no search
  fallback). The JSON shape is stable — the patch-notes deep-writer parses
  it. Reach for it only when you need machine-parseable classification
  (scripts, fan-out agents); when *you* are reading a record, use `walk`.

**Gotcha — hub AVIF/KYWD blowup**: a property targeting a widely-read AV
(e.g. `Health`) makes the reverse hook-resolution return dozens of unrelated
consumers (survival hunger/thirst, Daily Ops mutations, unrelated legendary
armor perks). `--ref-limit` (default 25) bounds it on both commands; walk's
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

- **PERK entry points that read like "weapon damage" are actually
  character-wide.** Entry point 167 (`Mod Weapon DMG Bonus Mult`) and
  siblings (`Mod Incoming Weapon Damage`, `Mod Target Damage Resistance`,
  `Mod My Critical Hit Damage Mult`, `Mod Percent Blocked`, `Mod Power Attack
  Damage`, `Mod Max Consecutive Hits Allowed`, `Mod Projectile Bounce Count`,
  `Apply Friendly/Combat Melee Hit Spell`) modify whatever damage instance is
  happening on the actor right now, not "this weapon's damage." A PERK
  granted via an OMOD's `Property 116`/`Perk` (chase's "PERK grant") stays active
  while the granting item is equipped and applies to every simultaneous
  damage source — thrown grenades/mines, Pain Train, VATS crits, blocking.
  Example: `MedicalMalpractice_Perk` (0x0050D7FD) is an entry-point-167
  effect whose only conditions are on the actor's own AV — nothing ties it
  to the granting weapon.
- **The fix, when the devs bother, is a self-referential `WornHasKeyword`/
  `HasKeyword` condition** naming a keyword unique to that item/roll — either
  its own `CustomItemName_X` naming keyword or a legendary-effect keyword
  like `HasLegendary_Weapon_APViaKill`. Example: `RD01_Weapon_LicketySplit`
  gates `Mod Projectile Bounce Count` on
  `HasKeyword(RD01_CustomItemName_LicketySplit)`. A condition on a *shared*
  category keyword (`WeaponTypeRanged`, `HasSilencer`) or an unrelated AV
  (`KillStreak`) is NOT a self-scope — the bonus still leaks, just narrower.
- **OMOD `Property 106` (`DamageBonusMult`) is the same character-wide hook
  as entry point 167 — not a per-weapon stat**, despite living on the
  weapon's own OMOD. Its `Value Type` is bare `Float` (no AV/FormID pointer),
  consistent with a hardcoded engine target rather than a WEAP field.
  Example: `mod_Legendary_Weapon1_Guns_TwoShot` sets `DamageBonusMult
  +0.75` — ordinary "Two Shot" rolls carry this leak, not just one weapon.
- **`Property 28` (`AttackDamage`) and `Property 77` (`DamageTypeValues`) DO
  write into the WEAP record's own fields** (`Data.Base Damage` / top-level
  `Damage Types[]` — see Curve tables below) and are genuinely weapon-scoped.
  `Property 94` (`ActorValues`) sits in between: a while-equipped character
  AV bonus (e.g. `mod_Custom_CivilUnrest` → +50 Action Points) — same
  equip-gated scope as the PERK path, but usually not a damage stat, so it
  doesn't compound like `DamageBonusMult`/entry-point-167.
- **A `Property 65` (`Enchantments`) attach can go either way — check the
  target MGEF's `Casting Type`/`Delivery`/`Archetype`, not just that an
  enchantment exists.** `Archetype: Damage`, `Casting Type: Fire and Forget`,
  `Delivery: Contact` is a genuine on-hit proc fired by that weapon's own
  attack — safe by construction (e.g. `TheKabloom`'s poison DoT). `Casting
  Type: Constant Effect`, `Delivery: Self` is a standing character buff —
  same leak risk as entry point 167 unless gated by a condition true only
  while wielding that weapon (e.g. `GetInIronSights`, since you can't ADS
  with a grenade). No condition at all is a confirmed leak — e.g.
  `ench_ThePeacemaker` (`STAT_DmgExplosive`, Constant Effect/Self, zero
  conditions).
- **Cross-check a suspiciously broad AV against sibling AVs before assuming
  it's "the everything" stat** — some are narrower than they look. FO76 has
  both `STAT_DmgExplosive` (every explosion: Fat Man, launchers, mines,
  grenades) and a separate, narrower `STAT_DmgGrenade` (thrown grenades
  only). Chase the actual `Actor Value` FormID on the MGEF; don't infer
  scope from the AV editor-id prefix alone.
- **A timed buff can outlive the weapon that triggered it.** An unconditioned
  entry point that selects a Spell (`Apply Friendly Hit Spell`, `Apply
  Combat Melee Spell`) is bad enough on its own, but if the Spell's effect
  carries a duration (the perk's Description often states it — "for 30
  Seconds" — even when the record's own `Duration` field reads empty),
  swapping weapons *after* the proc doesn't end the buff. Contrast an
  instantaneous on-target proc like a bleed DoT, which applies once with no
  persistent buff.

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

## Drop-chance math (LVLI chains)

**`esm walk <lvli-selector>` computes this automatically** (`src/lvli.rs`) —
pool/`Use All`/`Use First Match` selection, flat-vs-GLOB-vs-Curve-Table
chance-none, recursion through nested sublists to leaf items, all as one
ranked table. Reach for it instead of hand-tracing a chain; the rules below
are for reading its output (and for the handful of things it deliberately
doesn't model — `Filter Keyword Chances`, `Epic Loot Chance`, list-level
`Max Count`/`Max Global`/`Max Curve Table`, COED owner/rank gates — flagged
as `unresolved` notes on the affected rows rather than silently guessed).

- **A zero `Chance None Value` does not mean "guaranteed" — check the sibling
  `Chance None Global` on the same entry.** Each `Leveled List Entry` carries
  both a flat `Chance None Value` and an optional `Chance None Global` FormID;
  the flat value wins when nonzero, otherwise the GLOB is the real chance-none
  (same flat-wins rule as MGEF magnitudes above). Reading only the flat field
  makes gated rewards look like 100% drops: `TWZ07_LL_QuestReward_Event` reports
  flat `0.0` but points at `RA_Rewards_Activities_UniqueWeapon_DropRate_Cnone`
  = 85, i.e. a 15% drop. The list-level `Chance None Value` has no GLOB sibling
  — that one really is flat. A `Chance None Curve Table` sibling, when present,
  outranks both (see Curve tables below).
- **Flags decide how to combine entries, and neither no-flag nor `Use First
  Object That Matches All Conditions` is a 1/N pick.** `Use All` rolls every
  entry independently (multiply the per-entry miss chances). **No flag is a
  pool, not a uniform pick from the entry count**: every entry's own gate
  rolls independently, the passing subset is pooled, and one member of *that
  subset* is chosen uniformly — so an entry's real odds depend on how many
  siblings are also passing at the same time, not just its own gate or a flat
  `1/entry_count`. Confirmed against `SCORE_S22_Resources_Collector_
  SoulSoupServer_Food` (0x008308D7): six entries on a descending
  `GetRandomPercent ≥ {92,80,63,45,25}` ladder plus an unconditioned catch-all
  read like a hand-authored 8/12/17/18/20/25% split, but the *actual* pool
  odds are 2.20/5.66/10.96/17.19/25.22/38.77% — the rarest item is ~3.6×
  rarer than the ladder implies, because it only wins when it's the *sole*
  passing entry. `Use First Object That Matches All Conditions` walks entries
  in order and takes the first whose conditions pass — so an entry gated on
  `GetRandomPercent ≤ 10` genuinely is a flat 10%, and later entries are only
  reachable when every earlier gate fails (their true probability is the
  product of the preceding misses).
- **Entry-level `Conditions` gate the roll too** — `GetRandomPercent ≤ N` (flat
  or GLOB comparison value) and `HasLearnedRecipe(...) == 0` are the common
  ones. The recipe check is why plan-then-weapon lists hand out the plan first:
  `RD01_LLS_Raids_Rewards_Enc01_Weapons_Valkyrie` is `Use First Match` with the
  BOOK at `rand ≤ 5` (while unlearned) ahead of the weapon LVLI at `rand ≤ 10`.
  Any gate that isn't `GetRandomPercent` (`GetLevel`, `HasLearnedRecipe`, …) is
  real but not a probability the tool can compute — it renders as a `gated:`
  note (assume-pass by default; `--strict` isn't exposed on `walk` yet, only
  used internally).
- `Quantity: 0` on an entry means "use the sublist's own count", not "disabled" —
  creature death-item lists are full of them.
- **A `Minimum Level`/`Minimum Level Global` above the assumed player level
  (`--level`, default 50) excludes an entry outright.** Whether FO76 further
  collapses multiple *qualifying* Minimum Level tiers down to just the highest
  one when `Calculate from all levels <= player's level` is unset is
  unverified here — `walk` shows every qualifying tier and flags the ambiguity
  rather than picking a side.

## OMOD `Data.Includes[]` — inherited properties

- **An OMOD's own `Properties[]` is only half the story: `Data.Includes[]`
  pulls in `_PARENT_` building blocks whose properties also apply.** Sweeping
  `Properties[]` alone silently drops real mechanics. Confirmed: the "Black
  Diamond" Ski Sword's `mod_Custom_BlackDiamond` carries nothing but three
  keyword rows, yet includes `_PARENT_mod_WEAPON_GENERIC_Cryo_Split2`
  (`AttackDamage` −40%, `DamageTypeValues dtCryo` +60%) — the entire physical→cryo
  split lives in the parent. In a 90-weapon sweep, 60 weapons had properties
  reachable only through `Includes[]`. Resolve the chain recursively (parents can
  include parents) before concluding an OMOD "does nothing".
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
  the previous dump (tier tables rarely change), then `esm daemon stop`
  before re-querying.
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
Shot`, and `Full Power Damage Mult` at different times, once renaming
mid-session after a daemon restart. After any esm rebuild, re-dump one known
record (e.g. `esm get GaussRifle`) and grep the actual field names before
trusting fixtures, extractor code, or prior notes.
