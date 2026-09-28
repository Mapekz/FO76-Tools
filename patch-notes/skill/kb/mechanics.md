# Mechanics KB

Durable Fallout 76 game mechanics derived from the ESM. Read this **before** chasing, and chase
only what isn't here. Changes that look real but aren't live in `diff-traps.md`.

Entries are point-in-time. Treat anything verified more than ~2 months ago as a hint and
re-verify it with one live `get` before asserting it in a draft.

**Entry format.** Both KB files use it; keep entries to ≤10 lines:

```
## <Rule stated as a claim, not a topic>
<2-4 sentences: how it works and what to do with it.>
**Example:** <one line, FormIDs inline>
*verified <YYYY-MM-DD> vs <snapshot>*
```

Present tense only: no decision history or provenance. A rename that still matters is an alias
clause, not a paragraph.

**Rules for reading `esm` output are owned by `esm skill`** (flat-vs-curve-vs-GLOB values, curve
axes, `Data/Includes[]` inheritance, drop-chance math). Entries here restate only the clause the
writing task needs; correct the rule there first, then mirror it.

---

# Chasing effects

## Chasing a unique-weapon effect

Run `esm chase <FORMID_OR_EDID>` first and hand-walk only what it misses (`esm/src/chase.rs`'s
module docstring lists the limits). It accepts an OMOD, or a PERK/SPEL/ALCH/ENCH directly
(walking its own `Effects[]`), and follows one extra hop through an MGEF's `Perk to Apply` /
`Equip Ability`. A `mod_Custom_*` OMOD implements its mechanic one of four ways:

1. **Direct property**: ADD/SET on a weapon stat or actor value in `Data/Properties`. An AVIF's
   name is not its semantics; read the hop's `hop.resolution` (`"reverse"` = an AV hook resolved
   to its gating SPEL/PERK `Effects[N]` row, `"forward"` = a plain SPEL/ENCH/PROJ attachment).
2. **Perk grant**: Property `116`/`Perk` ADD of a PERK. Item-granted perks have no PCRD.
3. **Keyword hook**: the OMOD only ADDs a `CustomItemName_*`/`dn_*` KYWD; the mechanic is a
   SPEL/PERK effect gated on `WornHasKeyword(<keyword>)`, found by `refs --type SPEL --paths`.
4. **Projectile override**: Property `80` `OverrideProjectile` SET to a dedicated PROJ; the
   magnitude is on its EXPL's `Data / Damage Curve Table`. Curves swap wholesale by FormID and
   name (`..._Tier28` → `..._Tier40`), so a bulk get of old and new curves quantifies the delta.

An empty-shell OMOD pulls its effect from the `_PARENT_*` templates in `Data/Includes[]`;
`chase` folds their rows in as hops carrying `source_omod`. A `modcol_*` collection's includes
are `alternatives` in `chase` output, each a separate mod to chase.

**Example:** `RD01_Mod_Custom_ResolveBreaker_CustomName` (0x007934FE) → PROJ 0x007CA02E → EXPL
0x007CA02D.
*verified 2026-07-14 vs 20260710*

## `Magnitude: 0.0` beside a real value source is a live effect

A sibling `Curve Table` is always the value source; a `Magnitude` GLOB is the source when the flat
`Magnitude` is 0.0, and a nonzero flat value wins over its GLOB. Reading the flat 0.0 alone
produces a confident false negative ("grants nothing", "is cut"). `chase` JSON hands you the raw
rows, so re-read any zero magnitude with `get --resolve stub`, which inlines the curve points and
the keying `Actor Value` — that axis is often a state toggle, not a level.

**Example:** `MoM_ench_GarbofMysteries` (0x0052192E) `Effects[1]` reads Magnitude 0.0 and pays 5 or
20 Sneak off `CT_Armor_MoM_GarbofMysteriesSneak`, keyed on the Eye of Ra set bonus.
*verified 2026-07-24 vs 20260724*

---

# Damage & stats

## A "+X% damage" is one of three distinct mechanisms

Identify which before writing any number and name it in prose; never write a bare "+X% damage".
The mechanism decides how the number stacks, which is what build-crafters need.

1. **Additive damage bonus (DBM)**: joins the damage-bonus pool, stacking additively with every
   other bonus (so a build dilutes it). Sources: ADD to a `STAT_DmgMult*` or `STAT_DmgVs*` AV,
   OMOD property `DamageBonusMult`, PERK entry point "Mod Weapon DMG Bonus Mult". Report ×100.
2. **Base damage increase**: `AttackDamage` or `DamageTypeValues` (directly or via an OMOD
   MUL+ADD on property 77). Multiplies through everything downstream.
3. **Damage multiplier**: multiplies total outgoing damage after bonuses (power attack,
   weakpoint mults, Taking One for the Team, Follow Through). Rare on legendaries, strongest per
   point.

**Example:** BoomStick (0x00680832) property 106 `DamageBonusMult` 1.5 → 0.75 = +150% → +75% DBM,
not a base-damage cut.
*verified 2026-07-15 vs 20260702/20260710*

## Exact `DamageTypeValues` fold

`final(X) = max(0, (lastSET ?? base(X)) + Σ(MUL × ORIGINAL base(X)) + ΣADD)`. MULs scale off the
type's original base, never a running total; a SET discards the base. `dtPhysical` ≡ the weapon's
own `AttackDamage`. SET/ADD are flat with no level scaling. For a type the weapon **lacks**
(`base(X) = 0`), a positive MUL materializes a new component off a fallback base (ballistic if the
weapon has physical damage, else its primary elemental type, never explosion), level-scaled by
that fallback's curve. A **negative** MUL on a missing type yields 0 per modifier and does not
cancel a sibling mod's positive MUL on that type.
*verified 2026-07-15 vs 20260702/20260710*

## OMOD property semantics

- **`Value Type` decides the slots.** `Float`/`Int`: `MUL+ADD` effective = base × (1 + Value 1) +
  Value 2 (FO4/76 convention, inferred from worked examples). `FormID,Float` (e.g.
  `DamageTypeValues`): Value 1 is the FormID, Value 2 the magnitude. `FormID,Int` (Keywords,
  MaterialSwaps, ImpactDataSet, ZoomData, ModelSwap): Value 1 is the FormID, Value 2 is never read.
- Property IDs seen raw: `77` `DamageTypeValues`, `80` `OverrideProjectile`, `106`
  `DamageBonusMult`, `116` `Perk`.
- **A curve table on a property overrides Value 2.** Curve removed + Value 2 changed = scaling
  replaced by a flat value. Armor carry-weight-style curves key on item level (1/10/20/30/40/50).
- **`SET` vs `ADD` on a list-valued property is clobber vs append.** SET on `Keywords` or
  `Enchantments` erases every other entry, including the `ma_*` tags other mods target, so a lone
  `Function Type SET → ADD` row is that bug being fixed and matters more than it looks.
- `Attribute Descriptor Keywords` (NAM3) hold `MAD_*`/`MAN_*` keywords composing the crafting
  blurb ("Superior Critical Shot Damage"); losing them costs the blurb, not the effect.

**Example:** 20260717→20260724, `_PARENT_mod_melee_weapon_Hooked` flipped `Keywords` SET → ADD:
the official "Pipe Wrench Hooked mod prevented further modifications" fix.
*verified 2026-07-24 vs 20260724*

## `STAT_*` AVs route through four shared plumbing perks

A `STAT_*` AV does nothing on its own: an entry-point row on one of four hidden perks translates
it. Check a new `STAT_*` AV against them before inferring its effect from the name.

| Plumbing perk | Covers |
|---|---|
| `STAT_DamagePerk` | additive damage |
| `STAT_CritDamagePerk` | crit damage |
| `STAT_DamageVsPerk` | conditional / target-state damage |
| `STAT_BeneficialPerk` (0x0018ADAD) | 17 non-damage rows: sneak detection, spell magnitude/duration, cone of fire, item condition loss, lockpick sweet spot, VATS hit chance, ricochet, evasion, sprint AP drain, incoming limb damage |

Each row is `Multiply 1 + Actor Value Mult` (Float 0.01 → ×(1 + AV/100)) or `Add Actor Value Mult`
(+AV × Float); the AV is in `Function Parameter 3 (Actor Value)`. **Flipping a row between those
forms silently rebalances every source feeding that AV** without touching them. Weakpoint/limb
damage rows are Multiply. `STAT_DmgVsTorso` has no row: the `DamageVsNonWeakpoint_DO` default
object reads it.

**Example:** 20260717→20260724, the `Mod Detection Sneak Skill` row on `STAT_Sneak` (0x0008D1BF)
went Multiply 0.01 → Add 1.0: the official Sneak-bonus fix for six items, none of them touched.
*verified 2026-07-24 vs 20260724*

## `STAT_Dmg*` families

- `STAT_DmgVs{Bleeding,Burning,Poisoned,Freezing}`: +X% DBM vs targets in that status; the 4★
  family (Severing's, Pyromaniac's, Viper's). `STAT_DmgVsFreezing` (0x0085A2F1) has one OMOD
  consumer, Ice Breaker.
- `STAT_DmgMult{Cryo,Fire,Poison}`: unconditional elemental DBM. Science! ranks `ScienceMaster01`
  (Cryologist) and `ScienceExpert01` (Pyro-Technician) fortify Cryo/Fire; nothing reads Poison.
- `STAT_DmgThrown` (0x0090F5E4): a `STAT_DamagePerk` Add row (Float 0.01) gated on
  `WeaponTypeThrown`, so 1 point = +1% DBM on thrown weapons; fed only by the Spring Assisted arm
  lining (`_PARENT_mod_Armor_ThrownDamage` 0x0090F5F3, ADD 100.0).

Since 20260710 these replace bespoke ENCH → MGEF → PERK chains on some legendary mods. A blank OMOD
Description beside a `STAT_*` ADD means the tooltip comes from the AV. Check each mod's old
implementation: the axis can change, so a `20 → 50` pair across implementations is no comparison,
and Icemen's never migrated (MUL+ADD `DamageTypeValues` dtCryo 0.2 = +20% base cryo, materializing
cryo on weapons that lack it).

**Example:** Severing's, old: ENCH 0x008E0681 → PERK 0x008E0723 "Mod Weapon DMG Bonus Mult" ADD 0.5
gated on bleed; new: ADD `STAT_DmgVsBleeding` (0x00837DFC) 50.0. Same magnitude.
*verified 2026-07-14 vs 20260702/20260710; STAT_DmgThrown 2026-09-14 vs 20260914*

## Charge weapons (Gauss family)

- `Data / Full Power Seconds` = time to full charge (Gauss Rifle base 1.0 s).
- `Data / Full Power Damage Mult` = the full-charge damage multiplier (Gauss Rifle base 2.0).
  `MinPowerPerShot`, `MaxPowerPerShot` and `Min Power Per Shot` are older names for this field.
- Fast Trigger-family receivers (`_PARENT_mod_WEAPON_Receiver_FastTrigger_Solo`/`_Dual`) carry
  `FullPowerSeconds` MUL+ADD −0.25 beside `AttackDelaySec` −0.25: 25% faster full charge on a
  charge weapon, ignored elsewhere. The Gauss Shotgun and Gauss Pistol have such receivers (e.g.
  "Hair Trigger Receiver", 0x00573741).

**Example:** Flatliner (`RD01_Mod_Custom_StrikeBreaker_CustomName`, 0x00793512) ADDs +1.0 Full
Power Damage Mult (full-charge bonus +100% → +200%) and +0.5 Full Power Seconds (1.0 → 1.5 s).
*verified 2026-07-14 vs 20260710; receivers 2026-09-20 vs 20260918*

## Shared engine counters live at hardcoded AV slots

Bullet Storm, Kill Streak and Onslaught have **no queryable AVIF** (`esm get 0x399` 404s though
`refs` shows a stub). Build and decay are engine-side: the data exposes only caps and per-stack
bonuses, never the ramp.

| Counter | AV | Stacks gained by |
|---|---|---|
| Bullet Storm | `0x39B` | spending ammo (rate: GMST `uAmmoSpenderAmmoUsePerStack`) |
| Kill Streak | `0x399` | kills |
| Onslaught | `0x395` | consecutive hits |

**Bullet Storm.** Cap AVIF `AmmoSpenderMaxStacks` (0x0083C3CB), base 20 = 10 unconditional + 10
gated on `HasPerk(HeavyGunnerMaster01)`, both on SPEL `AbPerkHeavyGunner` (0x0031BE58) via MGEF
0x0083C3D1; floor `AmmoSpenderMinStacks` (0x00919957); per-kill switch `EnableAmmoSpenderOnKill`
(0x00924DB9, native consumer, its description is authoritative); damage curves
`Perks\HeavyDamageBonus{,2,3}.json`. Foundation's Vengeance (0x0064781F): +5 max stacks under 25% HP.

**Kill Streak.** +1/kill, cap 10, decays after ~30 s without a kill. Enabled by AVIF
`EnableKillStreak` (0x0080B56A); `KillStreakPerKillCount` (0x00924E31) adds stacks per kill. Read
by Adrenaline (+10% damage/stack) and unique-item perks, via `curve.input:"killStreak"` or a
condition on AV `0x399`. Different system: PERK entry point
**187 "Apply On Kill Spell"** is stateless (32 PERKs, e.g. Inertial's `LegendaryAPViaKillPerk`
0x00606B75, +15 AP/kill). Psychopath is EP119 (crit charge on a non-VATS hit, not a kill); Grim
Reaper's Sprint is EP107.

**Onslaught.** Base max 0; sources ADD to one shared max via EP190 "Mod Max Consecutive Hits
Allowed", with per-stack bonuses from EP189 or curves on `0x395`. Max / per-stack: Furious +9 / +1%
DBM, Pounder's +10 / +1% DBM, Gunslinger Master +10 / none, Gunslinger Expert +3 / +1% weakpoint,
Guerrilla Expert +3 / +1% reload, Guerrilla Master +5 / +5% DBM close range, Whacker Smacker +0 /
+5% power-attack bonus. Combo-Breaker's is EP79/EP27 (chance to not consume AP), not Onslaught.
*verified 2026-07-20 vs 20260710 (all 1991 PERK records scanned)*

## The Cheat Death revive family shares one cooldown framework

AVIF `CheatDeathResetOnWeakPointChance` (0x00924E29), percentage-flagged: +30.0 = +30% chance per
weak-point hit to reset a revive cooldown. Members found by EditorID search (not proven
exhaustive): Life Saver, E.M.T., Power Armor Reboot, Scout Banner.
*verified 2026-07-13 vs 20260710*

## Diet mutations zero chem-keyworded effects on matching food

Herbivore and Carnivore each grant three perks; the third ("Safe Veggies" 0x003C4059 / "Safe
Meat") multiplies to 0 any effect keyworded `RadiationInjestion`, `SURV_EffectTypeDiseaseVector` or
`ChemEffect` on matching items (Vegetable/Herb/Fruit; Meat). A food buff built on the chem MGEF
pattern (`ChemEffect` + its own Stack keyword + `ChemDispelEffects`) is nullified by the matching mutation, while the doubler (×2/×2.5, needs
`SURV_EffectTypeFood*`) never touches it. Audit all three perks before calling a buff
mutation-proof.

**Example:** Lucky-Leaf Tea (0x008FBA0C, Herb): its +1 LCK MGEF (0x008FBA0E) carries `ChemEffect`, so
Herbivore zeroes it; the unkeyworded +1/teammate MGEF (0x00905315) survives both.
*verified 2026-08-15 vs 20260814*

---

# Creatures

## Creature weapon damage curves are keyed on wielder level

An enemy WEAP's `Damage Curve` (e.g. `CT_Creatures_Damage_Universal_TierNN`) has x = wielder
level. Evaluate it at the NPC_'s real levels (its fixed level and its `Renorm_MinLVL_TierNN` /
`Renorm_MaxLVL_TierNN` GLOBs), interpolating linearly, never at the first point. **Combat
inventory is not loot:** only the death-item/reward LVLI chain is obtainable, so an inventory-only
weapon is "the boss attacks with it", never a drop or a legendary-mod roll.

**Example:** Slasher Knife / Throwing Knife (0x00927375/76), `CT_Creatures_Damage_Universal_Tier30`:
104 damage at boss level 100, ≈245 at Tier07 max level 175.
*verified 2026-07-15 vs 20260710*

## ACBS `Template Flags` bits gate which per-record fields the engine reads

Bits follow the order of `Template Actors`: `0x1` Use Traits, `0x2` Use Stats, `0x100` Use
Inventory, `0x1000` Use Keywords (a child's own `Keywords` edit is dead data while it is set). While `0x2` is set, stats come from the `Default Template` chain and a record's own
`Properties[]` curve is dead data; an inventory link clearing needs its `0x100` bit read too. XOR old
vs new flags on every record of a batch before reporting a curve swap or "decoupled from
template". ACBS `Flags` bit `Auto-calc stats + PC Level Mult` makes `Level Mult` (value/1000) the
live scaling knob instead of `Level`.

**Example:** Pint-Sized Phantom Ringleader (0x008E06D5) swapped Health curve Tier31 → Tier33 while
clearing `0x2`; 47 `HTO_` bosses' Tier52 → Tier54 on 20260903 kept `0x2` set and are inert.
*verified 2026-09-03 vs 20260903*

## Epic creatures & epic rank

`EpicRankData` on the NPC_ carries `HealthMult` 2.0–4.8 across ranks 1–5, gated by FLST
`EpicCreatureDisallowedKeywords`. A boss's rank comes from QUST `EncounterWaves[].BossEpicLevel`
(fixed only when `BossEpicChance == 100`; a nonzero chance below 100 makes the rank conditional)
or a boss-alias `defaultforcelegendaryalias.minRank`;
some bosses carry neither. A creature's community "★-rank" read off its loot LVLI/LGDI EditorID is
not epic rank. ESM-derived HP (base curve × `HealthMult`) is authoritative and can exceed 1M: the
"~32k HP" figure is the old signed-int cap, and no data scales HP by player count.
*verified 2026-07-19 vs 20260710*

---

# Items, crafting & vendors

## COBJ `Constructible Instantiation Filter Keyword` picks the crafted item's template

The keyword matches the created object's `Object Template / Combinations[]...Keywords[]`, and that
combination's mod loadout is stamped on the crafted item; with the field null the `Default: True`
combination applies. Combination names ("Default", "Simple", "Standard Epic") read intent fastest.
The same keyword family gates LVLI `Filter Keyword Chances`. A COBJ dropping the keyword matters
only if the two combinations' `Includes` differ.

**Example:** 20260724, 48 recipes dropped `if_tmp_Melee_Simple_Restricted`/`..._Minigun_...`; on 8
(e.g. Ripper, Power Fist) "Simple" lacked `mod_Shared_Melee_Paint_None`, so crafts now fill that slot.
*verified 2026-07-24 vs 20260724*

## The shared ranged mod set lives in `_PARENT_mod_WEAPON_*` includes

Per-weapon mods named True, Stabilized, Aligned, Hardened, Severe, Calibrated, Hair Trigger, Swift
or Stinging are shells whose numbers come from shared includes: get the include once and reuse it
across weapons. A shell whose name promises a stat but lacks the include is a Mismatch.

**Example:** M79 Severe Receiver (0x0094F183) includes `Damage_Tier1` (0x0027AC25, DBM +0.25) and
`CritDMG_dual` (0x0027AC23, +0.5); Gauss Pistol Severe Receiver (0x00951626) includes neither.
*verified 2026-09-20 vs 20260918*

## The "Cursed" weapon line lives entirely in one `_PARENT_` include

The six `*_Custom_Cursed` OMODs (Shovel, Pickaxe, Harpoon Gun, Rolling Pin, Sickle, Broadsider) are
empty shells including `_PARENT_mod_WEAPON_Cursed` (0x008AC233): `Speed` MUL+ADD +0.15,
`Durability` −0.15, `DamageBonusMult` ADD 0.35 (DBM), and display tag `dn_HasCustomMod_Cursed`.
Sources: `E06_Colossus_LLS_Quest_Rewards_Unique` (Shovel, Pickaxe, Harpoon Gun) and
`LLS_TreasureHunt_Rewards_Rare_Common` (Sickle, Broadsider, Rolling Pin via
`LL_DailyOps_Rewards_CursedRollingPin`), all selecting the template via `if_tmp_EN06_Cursed`
(0x005A70B4).
*verified 2026-07-24 vs 20260724*

## Resolving a special-currency vendor's price and gate

Follow `refs` from the item to its vendor LVLI tier entry, the CONT/NPC selling it, and that
vendor's FACT, whose `Vendor Buy Currency` is the real currency. The tier entry's `Conditions` (a
`Rep_Tier_<Location>_N_<Rank>` CNDF on an AVIF like `Reputation_AV_Crater`) name the reputation
gate. For the amount, read the item's `Gold Bullion Value` first: when it points at an
`Econ_GoldVendor_Tier_NN` GLOB, that GLOB's value is the bullion price and `Value` is only the
Caps worth. `Value` is the price magnitude only when `Gold Bullion Value` is absent. Never quote a
vendor price without both fields read and the FACT currency named; an EditorID suffix such as
`_StampVendor` says nothing about currency.

**Example:** `Plan: Piercing Love` (BOOK 0x00930841, Value 1000, no `Gold Bullion Value`) → LVLI
`W05_LLV_GoldVendor_Raider_Mortimer_6_Ally` → CONT → FACT `Vendor Buy Currency = GoldBullion`, gated
`Reputation_AV_Crater >= 12000`: "1000 Gold Bullion from Mortimer at Ally".
`SCORE_S26_Recipe_Cryolator_Blasted_StampVendor` (BOOK 0x00918210) has Value 15 and `Gold Bullion
Value` → `Econ_GoldVendor_Tier_12` = 2000: "2,000 Gold Bullion", not 15.
*verified 2026-09-20 vs 20260918*

---

# World Pets

## World Pets passives gate on progression-track entries

Each species (Cat/Dog/Deathclaw/Radhog) has two tiered passives built from 3-4 SPEL/PERK/LVLI/GMRW
rows, each gated on condition function 942 (absent from xEdit's table). Its `Parameter #1` (CIS1)
is base64 of an 8-byte little-endian PGTR `Entry UID`: decode it, map it to a
`WorldPets_ProgressionTrack_<species>_NEW` entry, and check each gate's species and tier, since
some point at another pet's track. Entry UIDs are `1783090000 + 100 × species + entry index` (Cat 0,
Deathclaw 1, Dog 2, Radhog 3) and base64 is case-sensitive, so one wrong letter lands on another
species; also read `Comparison Value`, since `== 0.0` on a tier's own entry inverts the gate. Nothing reads a pet level (`WorldPets_PetProwessLevel` 0x00921E47
and `WorldPets_PetLevelling_Level_*` are unused), and the `WorldPets_ENTM_*_BUFF/PERK_*`
entitlements and `WorldPets_LvReward_*` GMRWs are `zzz` and empty. Magnitudes sit on `Magnitude`
GLOBs beside a flat 0.0.
Pet Prowess by tier: outgoing ×2/×3.5/×5.5/×8, incoming ×0.8/×0.6/×0.4/×0.2.

**Example:** `WorldPets_CatBuff_Buff01` (0x0093BD1C) CIS1 `UMtHagAAAAA=` → Entry UID 1783090000 =
Cat track "Baits Finder 1"; `WorldPets_DogBuff_Buff01` (Stimpak Fetcher, 0x0093BD1E)
`Effects[0].Magnitude` → GLOB `WorldPets_ConsumableBuff_Dog01` (0x008D1875, 2.0).
*verified 2026-09-14 vs 20260914; UID formula 2026-09-20 vs 20260918*

## A World Pets progression track is one PGTR record per species

`WorldPets_ProgressionTrack_<species>_NEW` holds 30 Track Entries keyed by `Level Threshold`
(formerly `Progress Threshold`), pet level 5 to 200. An entry's authored `Name`/`Description Text`
says what the tier does; its `VPRR` points at the granted `GMRW`, and its `NAME` points at the
prerequisite entry's `PGTI`. A second reward slot is the alternate when a cross-track reward (e.g.
a Player Icon) was already claimed. RACE `PGTF` links `CAMPPets_<Species>Race` to its track, and
RACE `Pet Commands` carries a `Command Emote` (`EMOT`) per command. Gift timers share GLOB
`WorldPets_ConsumableGiftInterval` (0x008B4291, 1801 s).
*verified 2026-09-14 vs 20260914*

## Refs tell whether a snapshot has World Pets switched on

The Pets PTS branch (20260903, header v283) has pets live for testing; on the Slasher line KYWD
`IsWorldPet` (which gates the follow package) is applied to nothing and the four command emotes
sit in FLST `ATX_HideFromStoreList` (0x004875A1). Check those refs to tell a snapshot's branch.
*verified 2026-09-03 vs 20260903*

## A World Pets loot passive ramps star rank or item count, never both

An activity-reward passive adds one gated entry per tier to the activity's reward list. A `Use
First Object That Matches All Conditions` list of 1/2/3-star templates ramps the rank; several
entries pointing at one shared list with per-tier `Quantity Global`s ramp the count at that list's
fixed rank. The two read identically in a diff, so check the shape before quoting stars.

**Example:** Bounty Sniffer (0x0093D70B) ramps 1→2→3 star; Fun-Festation's entries on
`HTO_crLLD_Boss` (0x00863220) all point at 3-star-only 0x008FD890 and ramp 1→2→3 items.
*verified 2026-09-14 vs 20260903*
