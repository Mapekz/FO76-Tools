# Mechanics KB

Durable Fallout 76 game mechanics derived from the ESM. Read this **before** chasing — chase
only what isn't here. Companion file: `diff-traps.md` (things that look like changes but aren't).

Entries are point-in-time. Treat anything verified more than ~2 months ago as a hint and
re-verify with one live `get` before asserting it in a draft.

**Entry format** — keep new entries to this shape, ≤10 lines:

```
## <Rule stated as a claim, not a topic>
<2-4 sentences: how it works and what to do with it.>
**Example:** <one line, FormIDs inline>
*verified <YYYY-MM-DD> vs <snapshot>*
```

No decision history, no provenance, no "this used to be called X" narrative — if a rename still
matters, it's an alias line, not a paragraph.

---

## Chasing a unique-weapon effect

`esm -p chase <FORMID_OR_EDID>` automates this (always emits classified JSON) — run it first, hand-walk only what it
misses (`esm/src/chase.rs`'s module docstring lists the limits). It accepts an OMOD, or a PERK /
SPEL / ALCH / ENCH directly (walking that record's own `Effects[]`), and auto-follows one extra
hop through an MGEF's `Perk to Apply` / `Equip Ability`.

A `mod_Custom_*` OMOD implements its mechanic one of four ways:

1. **Direct property** — ADD/SET on a weapon stat or actor value in `Data/Properties`. An AVIF's
   name is not its semantics: find its consumer (`refs --type SPEL|PERK --paths`).
2. **Perk grant** — Property `116`/`Perks` ADD of a PERK. Item-granted perks have **no PCRD**;
   `unreferenced_perk_rank` is a false positive on them.
3. **Keyword hook** — the OMOD only ADDs a `CustomItemName_*` / `dn_*` KYWD; the mechanic lives
   in a SPEL/PERK effect gated on `WornHasKeyword(<that keyword>)`. `refs --type SPEL --paths` on
   the keyword points straight at the gating `Effects[N].Conditions[...]`.
4. **Projectile override** — Property `80` `OverrideProjectile` SET to a dedicated PROJ; the
   magnitude lives on that PROJ's linked EXPL's `Data / Damage Curve Table`, not the OMOD. Curves
   are swapped wholesale by FormID+name (`CT_Player_Damage_Universal_Tier28` → `..._Tier40`), so
   a bulk get across old/new curve FormIDs quantifies the delta.

Empty-shell OMODs pull their effect from `Data/Includes[]` (`_PARENT_*` building blocks) — `chase`
returns nothing useful on those; chase the include instead.

**Example:** `RD01_Mod_Custom_ResolveBreaker_CustomName` (0x007934FE) → PROJ 0x007CA02E → EXPL
0x007CA02D.
*verified 2026-07-14 vs 20260710*

## A "+X% damage" is one of three distinct mechanisms

Identify which before writing any number, and name it in prose — **never write a bare "+X%
damage"**. The mechanism determines how the number stacks, which is exactly what build-crafter
readers need.

1. **Additive damage bonus (DBM)** — a contribution to the damage-bonus-multiplier pool, stacking
   additively with every other bonus (so a build dilutes it). Sources: ADD to a `STAT_DmgMult*` AV
   (unconditional) or a `STAT_DmgVs*` AV (conditional on target status); OMOD property
   `DamageBonusMult`; PERK entry point "Mod Weapon DMG Bonus Mult". Report values ×100.
2. **Base damage increase** — changes to `AttackDamage` or `DamageTypeValues` (directly or via an
   OMOD MUL+ADD on property 77). Multiplies through everything downstream.
3. **Damage multiplier** — multiplies total outgoing damage after bonuses: power attack,
   body-part/weakpoint mults, Taking One for the Team, Follow Through. Rare in legendary mods,
   strongest per point.

**Example:** BoomStick (0x00680832) property 106 `DamageBonusMult` 1.5 → 0.75 = +150% → +75%, a
DBM contribution — not a base-damage cut.
*verified 2026-07-15 vs 20260702/20260710*

## Exact `DamageTypeValues` fold

`final(X) = max(0, (lastSET ?? base(X)) + Σ(MUL × ORIGINAL base(X)) + ΣADD)`. MULs scale off the
type's *original* base, never a running total; a SET discards the base entirely. `dtPhysical` ≡ the
weapon's own `AttackDamage`. SET/ADD are flat with no level scaling.

This matters most for a type the weapon **lacks** (`base(X) = 0`): a positive MUL materialises a
new component, scaled off a fallback base (ballistic if the weapon has any physical damage, else
its primary elemental type — never an explosion component) and level-scaled by the fallback's own
curve. A **negative** MUL on a missing type multiplies zero and vanishes, evaluated **per-modifier,
not netted** — so in a batch of "−X% on all damage types" mods, each independently yields 0 and
does *not* cancel a sibling mod's positive MUL on the same type.
*verified 2026-07-15 vs 20260702/20260710*

## `STAT_*` AVs route through four shared plumbing perks

`STAT_*` actor values are never read ad hoc. Each is translated into engine behaviour by an
entry-point row on one of four hidden perks. Check a new `STAT_*` AV against these before
inferring its effect from the name alone.

| Plumbing perk | Covers |
|---|---|
| `STAT_DamagePerk` | additive damage |
| `STAT_CritDamagePerk` | crit damage |
| `STAT_DamageVsPerk` | conditional / target-state damage |
| `STAT_BeneficialPerk` (0x0018ADAD) | 17 non-damage rows: sneak detection, spell magnitude/duration, cone of fire, item condition loss, lockpick sweet spot, VATS hit chance, ricochet, evasion, sprint AP drain, incoming limb damage |

Each row is either `Multiply 1 + Actor Value Mult` (Float 0.01 → ×(1 + AV/100)) or `Add Actor
Value Mult` (Float 1.0 → +AV flat); the target AV is in `Function Parameter 3 (Actor Value)`.
**Flipping a row between those two forms silently rebalances every source feeding that AV**, with
no change to any of those source records. Weakpoint/limb-scoped damage rows are "Multiply 1+AV".
`STAT_DmgVsTorso` is the one exception with no plumbing row — read by the `DamageVsNonWeakpoint_DO`
default object instead. `STAT_BeneficialPerk` has no PCRD (attached directly to the Player NPC_,
0x00000007).

**Example:** 20260717→20260724, the `Mod Detection Sneak Skill` row on `STAT_Sneak` (0x0008D1BF)
went `Multiply 1+AV` 0.01 → `Add AV` 1.0 — the single change behind the official "Sneak bonus
fixed for Sneak Bobblehead / Chinese Stealth Armor / Secret Agent's / Nuka-Inspiration: Dark /
Garb of Mysteries / Thorn Armor" line. None of the six records was itself touched.
*verified 2026-07-24 vs 20260724*

## `STAT_Dmg*` families and the enchantment→AV migration

- `STAT_DmgVs{Bleeding,Burning,Poisoned,Freezing}` — +X% damage vs targets currently in that
  status; the 4★ legendary family (Severing's, Pyromaniac's, Viper's, Icemen's).
- `STAT_DmgMult{Cryo,Fire,Poison}` — unconditional elemental DBM. Cryo/Fire are fortified by
  Science! ranks `ScienceMaster01` ("Cryologist") and `ScienceExpert01` ("Pyro-Technician"); no
  live perk consumes the Poison variant.

Since 20260710 these replace bespoke ENCH→MGEF→PERK script chains on several legendary mods —
same numbers, new plumbing. A blank OMOD Description alongside a `STAT_*` ADD means the tooltip
auto-generates from the AV's own text. **Don't call the migration semantics-preserving without
checking the old implementation per mod:** Icemen's was a MUL+ADD on `DamageTypeValues` dtCryo
(+20% *base* cryo damage, always on, and new cryo damage on non-cryo weapons) and became +50
`STAT_DmgVsFreezing` (0x0085A2F1), a conditional DBM. Different axis — the 20→50 numbers are not
comparable.

**Example:** Severing's — old (20260702) OMOD → ENCH 0x008E0681 → MGEF (Perk to Apply) → PERK
0x008E0723 "Mod Weapon DMG Bonus Mult" ADD 0.5, gated on bleed. New side ADDs `STAT_DmgVsBleeding`
(0x00837DFC) 50.0 directly; the old ENCH is `zzz`-vaulted. Same magnitude.
*verified 2026-07-14 vs 20260702/20260710*

## OMOD property semantics

- `MUL+ADD`: effective = base × (1 + Value1) + Value2. Standard FO4/76 convention, inferred from
  worked examples, not confirmed against engine code.
- Known property IDs: `77` = `DamageTypeValues`, `80` = `OverrideProjectile`, `106` =
  `DamageBonusMult`, `116` = `Perks`.
- **A curve table on a property overrides Value2 as the magnitude source.** Curve removed + Value2
  changed = scaling replaced by a flat value. The x-axis on armor carry-weight-style curves is
  **item level** (break points 1/10/20/30/40/50).
- **`SET` vs `ADD` on a list-valued property is clobber vs append.** SET on `Enchantments` erases
  every other enchantment on the item; SET on `Keywords` erases every other keyword, including the
  weapon-type and `ma_*` mod-association tags other systems and mods depend on. A diff row whose
  only content is `Function Type SET → ADD` on the same property and value is that bug being
  fixed, and it is always more consequential than it looks.
- `Attribute Descriptor Keywords` (NAM3) holds `MAD_*` (Modification **A**ttribute **D**escriptor,
  e.g. `MAD_Superior`) and `MAN_*` (Attribute **N**ame, e.g. `MAN_Range`) keywords composing the
  crafting menu's blurb ("Superior Critical Shot Damage"). They carry no stat of their own —
  losing them costs the blurb, not the effect. No `from_version` gate, so churn here is real data.

**Example:** 20260717→20260724, `_PARENT_mod_melee_weapon_Hooked` flipped `Keywords` SET → ADD —
the official "Pipe Wrench Hooked mod prevented further modifications" fix, since the SET had wiped
the wrench's keywords so no other mod's `ma_*` target matched it.
*verified 2026-07-24 vs 20260724*

## `Magnitude: 0.0` beside a `Curve Table` is a live effect, not a dead one

When an ENCH/SPEL effect carries a `Curve Table` alongside its `Effect Item Data`, the curve is the
value source and the flat `Magnitude` is meaningless — commonly authored as `0.0`. Reading the
magnitude alone produces a confident false negative ("grants nothing / is cut / a balance change
can't reach it"). Always check for a sibling curve before saying any of those.

The curve's x-axis is the effect's sibling `Actor Value`, frequently **not** a level — two curve
points are often two *states*, not a ramp.

**Example:** `MoM_ench_GarbofMysteries` (0x0052192E) `Effects[1]` `abFortifySneak`, Magnitude 0.0,
curve `CT_Armor_MoM_GarbofMysteriesSneak` = `[(0, 5), (1, 20)]` keyed on AVIF `MoM_EyeOfRa`
(0x006DE64A) — the Garb grants **5 or 20** Sneak depending on the Eye of Ra set bonus. Nothing in
the ESM writes `MoM_EyeOfRa`, so the toggle is engine-side; say that rather than inventing a
trigger.
*verified 2026-07-24 vs 20260724*

## Shared engine counters live at hardcoded AV slots

Bullet Storm, Kill Streak and Onslaught are native-engine counters with **no queryable AVIF** —
`esm get 0x399` 404s even though `refs` displays a synthesized stub. Build and decay are
engine-side and unmodeled by any record: the data exposes only steady-state inputs (cap, per-stack
bonus), never the ramp.

| Counter | AV | Stacks gained by |
|---|---|---|
| Bullet Storm | `0x39B` | spending ammo |
| Kill Streak | `0x399` | kills |
| Onslaught | `0x395` | consecutive hits |

### Bullet Storm

Stacks come from **spending ammo** (GMST `uAmmoSpenderAmmoUsePerStack` sets the rate) — not kills,
not hits. Cap is AVIF `AmmoSpenderMaxStacks` (0x0083C3CB), fortifiable; base **20** = 10
unconditional + 10 gated on `HasPerk(HeavyGunnerMaster01)`, both effects on SPEL `AbPerkHeavyGunner`
(0x0031BE58) via MGEF `abAmmoSpenderFortifyStacks` (0x0083C3D1). Floor: `AmmoSpenderMinStacks`
(0x00919957). Per-kill gain switch: `EnableAmmoSpenderOnKill` (0x00924DB9), a boolean AVIF whose
consumer is native code — its description is the authoritative text. Damage scaling: curves
`Perks\HeavyDamageBonus{,2,3}.json` on the same SPEL.

**Example:** Foundation's Vengeance (0x0064781F) adds an `AbPerkHeavyGunner` effect (Magnitude 5.0)
gated on `WornHasKeyword(CustomItemName_FoundationsVengeance, 0x0064781E)` AND `GetHealthPercentage
<= 0.25` — +5 max stacks under 25% HP.
*verified 2026-07-14 vs 20260710*

### Kill Streak

Base +1/kill, cap 10, decays after ~30s without a kill. Enabled via AVIF `EnableKillStreak`
(0x0080B56A) / MGEF `abEnableKillStreak`; `KillStreakPerKillCount` (0x00924E31) adds extra stacks
per kill on top. Read by Adrenaline (+10% damage/stack) and several unique-item perks.

**Don't conflate it with the generic on-kill hook.** Perks that *read the counter* (via
`curve.input:"killStreak"` or a condition on AV `0x399`) are a different system from PERK entry
point **187 "Apply On Kill Spell"**, which is stateless — a one-shot spell every kill, no counter,
timer, cap, or shared AV. Exactly 32 PERKs use EP187. Two lookalikes to keep separate: Psychopath
(all 3 ranks — 0x0027A86F/72, 0x003701AD) is **EP119** "Mod VATS Critical Charge" gated on
`GetIsInVATS=0`, i.e. crit-meter charge on a non-VATS **hit**, not a kill; Grim Reaper's Sprint is
**EP107** "Mod VATS Player AP On Kill Chance", unique to that family.

**Example:** Inertial — `mod_Legendary_Weapon2_APViaKill` (0x00606B72) → keyword →
`LegendaryAPViaKillPerk` (0x00606B75), EP187, +15 AP/kill.
*verified 2026-07-20 vs 20260710 (all 1991 PERK records scanned)*

### Onslaught

Base max is **0** — every source ADDs to a single shared max via PERK entry point 190 "Mod Max
Consecutive Hits Allowed"; per-stack bonuses come from EP189 "Mod Damage on Consecutive Hits" or
from curves reading `0x395` directly. Contributors (max / per-stack): Furious +9 / +1% dbm,
Pounder's +10 / +1% dbm, Gunslinger Master +10 / —, Gunslinger Expert +3 / +1% weakpoint damage,
Guerrilla Expert +3 / +1% reload speed, Guerrilla Master +5 / +5% dbm at close range, Whacker
Smacker +0 / +5% power-attack bonus. **Combo-Breaker's is not Onslaught** despite the flavor — it's
EP79/EP27, a chance-to-not-consume-AP mechanic.
*verified 2026-07-20 vs 20260710*

## The Cheat Death revive family shares one cooldown framework

AVIF `CheatDeathResetOnWeakPointChance` (0x00924E29) — "Attacks Against Weak Points Have a <VALUE>
Chance to Reset a Revive Effect Cooldown", percentage-flagged, so +30.0 = +30% chance per
weak-point hit. Known members: Life Saver, E.M.T., Power Armor Reboot, Scout Banner (found by
EditorID search, not proven exhaustive).
*verified 2026-07-13 vs 20260710*

## Charge weapons (Gauss family)

- `Data / Full Power Seconds` = time to reach full charge (Gauss Rifle base 1.0s).
- `Data / Full Power Damage Mult` = the full-charge damage multiplier (Gauss Rifle base 2.0).
- **Name aliases:** this one field has been called `MinPowerPerShot`, then `MaxPowerPerShot`, now
  `FullPowerDamageMult` (and `Min Power Per Shot` in the raw WEAP `Data` struct, patched via
  `schema/fo76.overrides.json`). Treat all of them as the same field — data captured before
  2026-07-15 may carry an old name.

**Example:** Flatliner (`RD01_Mod_Custom_StrikeBreaker_CustomName`, 0x00793512) ADDs +1.0 Full
Power Damage Mult (2.0→3.0, full-charge bonus +100%→+200%) and +0.5 Full Power Seconds (1.0→1.5s),
replacing an ADD Perks 116 grant of `mod_weapon_penetrating`.
*verified 2026-07-14 vs 20260710*

## Creature weapon damage curves are keyed on wielder level

An enemy WEAP's `Damage Curve` (e.g. `CT_Creatures_Damage_Universal_TierNN`) has **x = wielder
level**. Never quote the curve's first point as "the damage" — evaluate at the wielding NPC_'s
actual level(s): its fixed level plus the `Renorm_MinLVL_TierNN` / `Renorm_MaxLVL_TierNN` GLOB
bounds (get the GLOBs). Interpolate linearly, as the engine's `Curve::eval` does.

**Combat inventory ≠ loot.** An NPC_'s inventory / Object Template is what it *fights with*; only
the death-item/reward LVLI chain (e.g. `*_LL_BountyDrop_*`) is player-obtainable. An inventory-only
weapon is described as "the boss attacks with it", never as a drop, and never as "can roll
legendary mods".

**Example:** Slasher Knife / Throwing Knife (0x00927375/76) share
`CT_Creatures_Damage_Universal_Tier30` → 104 damage at boss default level 100, ≈245 at its Tier07
max level 175.
*verified 2026-07-15 vs 20260710*

## Epic creatures & epic rank

`EpicRankData` on the NPC_ carries `HealthMult` 2.0–4.8 across ranks 1–5, gated by the
`EpicCreatureDisallowedKeywords` FLST. Two distinct VMAD shapes assign a boss's rank — check for
either: QUST `EncounterWaves[].BossEpicLevel` (only meaningful when that wave's `BossEpicChance ==
100`; a nonzero-but-not-100 chance means the rank is conditional), or a boss-alias
`defaultforcelegendaryalias.minRank`. Some well-known bosses carry neither shape.

- **Loot-list rank ≠ epic rank.** A creature's community "★-rank" is usually read off its *loot*
  LVLI/LGDI EditorID ("…3Star…", "…Rank4…"), an unrelated data path. Citing it as proof of epic
  rank is a common false positive.
- **The "~32k HP" community figure is the game's old signed-int cap (32767), lifted circa 2023.**
  Per-nearby-player HP scaling is a **myth** — nothing in the data scales HP off player count.
  ESM-derived HP (base curve × the rank's `HealthMult`) is authoritative and can exceed 1M at high
  rank and level; don't reintroduce the cap or a player-count model when a number looks large.
*verified 2026-07-19 vs 20260710*

## COBJ `Constructible Instantiation Filter Keyword` picks the crafted item's template

The keyword is matched against the created object's
`Object Template / Combinations[].Combination.Object Mod Template Item.Keywords[]`; whichever
combination carries it supplies the mod loadout the crafted instance is stamped with. With the
field null the engine falls back to the combination flagged `Default: True`. Combinations are
human-named ("Default", "Standard", "Standard Epic", "Simple"), the fastest way to read intent.
The same keyword family also gates LVLI `Filter Keyword Chances`.

A COBJ dropping this keyword is neither automatically a no-op nor automatically meaningful — you
must compare the two combinations' `Includes` lists on the created object.

**Example:** 20260717→20260724, 48 base weapon recipes dropped `if_tmp_Melee_Simple_Restricted` /
`if_tmp_Minigun_Simple_Restricted`. On 40 the "Simple" and "Default" combinations are
byte-identical (pure bookkeeping); on 8 (Ripper, Power Fist, Chinese Officer Sword, Grognak's Axe,
Bowie Knife, Guitar Sword, Revolutionary Sword, Rolling Pin) "Simple" omitted
`mod_Shared_Melee_Paint_None`, so the crafted weapon now ships with that slot filled.
*verified 2026-07-24 vs 20260724*

## The "Cursed" weapon line lives entirely in one `_PARENT_` include

Six weapons have a `*_Custom_Cursed` OMOD (Shovel, Pickaxe, Harpoon Gun, Rolling Pin, Sickle,
Broadsider). None carries the mechanic — each is an empty shell whose `Data/Includes[]` pulls in
`_PARENT_mod_WEAPON_Cursed` (0x008AC233), holding the whole effect: `Speed` MUL+ADD +0.15,
`Durability` MUL+ADD −0.15, `DamageBonusMult` ADD 0.35 (a DBM), and a `Keywords` ADD of
`dn_HasCustomMod_Cursed` (0x005A70B5 — read only by two INNR naming-rule lists, a display tag with
no SPEL/PERK consumer). `chase` on the per-weapon OMOD returns nothing useful.

Acquisition: `E06_Colossus_LLS_Quest_Rewards_Unique` (A Colossal Problem / Earle) for Shovel,
Pickaxe and Harpoon Gun; `LLS_TreasureHunt_Rewards_Rare_Common` for Sickle, Broadsider and Rolling
Pin, with `LL_DailyOps_Rewards_CursedRollingPin` nested under it. All six select the cursed
template via the LVLI `Filter Keyword Chances` keyword `if_tmp_EN06_Cursed` (0x005A70B4).
*verified 2026-07-24 vs 20260724*

## World Pets is built but gated off (as of 20260710)

A C.A.M.P. pet (Cat/Dog/Deathclaw/Radhog) that follows you with commands and a hidden 1–200 "Pet
Prowess Level" (`WorldPets_PetProwessLevel` AVIF family). Prowess perk ranks live on
`CAMPPets_Actor_*` NPC templates (item/NPC-granted, so no PCRD — expected): damage ×1→×8 and
incoming damage ×1→×0.2 across level brackets 1-49/50-99/100-149/150-199/200.

Not live because the `IsWorldPet` KYWD gating the follow package is applied to nothing, the World
Pet faction has zero refs, a kill-switch spell ("Pet buffs are disabled") exists, and the four
command emotes (0x00916200–0x00916203) were added to FLST `ATX_HideFromStoreList` (0x004875A1) in
20260710. Distinct from the older `PETS_`-prefixed adoptable-companion quest system.
*verified 2026-07-14 vs 20260710*

## Seasonal content converts by rename, not by adding records

A seasonal one-off promoted to a permanent repeatable shows up as a QUST rename plus a
`QTFS (Repeat Limit?)` flip, never a new record. `0xffff` (65535) reads as "no limit".

**Example:** `SDOW_SQ01_Graves_Repeatable` (0x008F1665) in 20260710 — "(Seasonal) Laid to Unrest" →
"(Repeatable) Disturbed Grave", QTFS 65535 → 50. For reference, Slasher Season Y2's chain is
`SDOW_MQ01_Bodies` (0x008F15C1) → `MQ02_Graves` (0x008F15A1) → MQ04 (0x008F15C2) → MQ05
(0x008F15C3), tracked by radio quest `SDOW_SQ_DebunkerRadio` (0x008EDF32) via `LCP_SDOW_*` GLOB
toggles.
*verified 2026-07-14 vs 20260710*
