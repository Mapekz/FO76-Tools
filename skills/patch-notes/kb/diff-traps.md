# Diff traps

Diff rows that look like changes but aren't, and the known false positives of our own lints.
Check a row here **before** writing it up: a match produces no bullet, at most one "under the
hood" line, never a gameplay claim. Entries follow `mechanics.md`'s entry format; game mechanics
live there.

---

# Serialization & schema-population churn

## Run the permutation test before reading any array diff

Positional and keyed `_array_diff` entries often only move elements between slots. **Permutation
test:** serialize each changed element (or each subpath's from/to values) into a multiset per
side; matching multisets mean nothing changed. Seen on VMAD `scripts[]`/`script_fragments`/alias
slots, RACE `Attacks[]`/`Bone Scale Data[]`, LCTN `Master Reference`/`Master Unique NPCs`, NPC_
`Attacks[]` and PERK `Effects`. RACE shows mirrored numeric pairs (a `Damage Mult` 1.0 → 1.5 at
one index, the reverse at another); NPC_ `Attacks[]` shows only `Attack Event` string changes, so
a missing mirrored pair proves nothing.

**Example:** 20260717→20260724, 17 creature NPC_ records (e.g. EncMolerat03 0x001832F8) had
`Attacks` as their only changed path; both sides held identical multisets.
*verified 2026-07-24 vs 20260724*

## VMAD `scripts[]` compares as a name multiset, never by index

A permuted script list reads as a feature: "a bespoke script replaced the generic one, properties
3 → 6", when two different scripts merely share a slot. Compare script names with per-script
property counts; a script is removed only if its name is absent from the whole new side. Globals
or aliases bound by a script that moved are not new bindings: check they exist in the old snapshot.

**Example:** QUST `Burn_BountyHunt_Headhunt` (0x007EBDF4) carries the same six scripts (property
counts 3, 6, 12, 2, 5, 5) reordered; its `Burn_BountyHunt_RecentHeadhuntGang_0N` globals
(0x00833A0A–0C) already existed at −1.0.
*verified 2026-07-24 vs 20260724*

## PERK effect `Rank`, `Tab Count` and `Actor Value, Float` are bookkeeping

After the permutation test, three PERK effect fields are noise either way. `Entry Point / Perk
Condition Tab Count` shifting is editor metadata. `Actor Value, Float` appearing while `Float` and
`Function Parameter 3 (Actor Value)` go null is one schema struct replacing two fields. `Effect
Header / Rank` is not a rank gate: live single-rank perks carry 0, 1, 10, 40, 70/71 or 153
(Retaliator's `RTSV_StormRender_Rebuttal` 0x008DB74B has its ×1.4 row at 1), and ranks are
separate PERK records, so never call a row rank-locked or inert from it.

**Example:** `PlayerPerk_Spotlight` (0x0046C7CF) reported 10 of 18 effect rows changed; both sides
carry the same ten `Ab_Spotlight_*` abilities plus eight empty slots.
*verified 2026-09-14 vs 20260903*

## `Parameter 1` on `IsPreviousMeleeAttackEvent` is a string pointer

That condition's `Condition Data / Parameter 1` is a raw string offset that shifts on every
re-serialization; the decoded value is the sibling `Parameter #1` (the attack-event name). A
`Parameter 1` delta with `Parameter #1` unchanged is no change.

**Example:** Sheepsquatch (0x00479D50): all 39 attacks' `Parameter 1` moved, no event name did.
*verified 2026-07-24 vs 20260724*

## REGN `Region Areas` point lists re-serialize reversed or rotated

A REGN diff with every `Points[N]` X/Y changed is usually the same polygon with its winding
reversed and its start point rotated, within ≤10 units of jitter. Compare each area as a cyclic
point sequence in both directions before calling a boundary redrawn.

**Example:** 20260903→20260914, 96 of 97 changed Skyline Valley/Burning Springs region areas
matched after reversal/rotation (max deviation 9.91); only `StormObjectRegion_Forest01`
(0x006FCB0B) moved.
*verified 2026-09-14 vs 20260914*

## Cross-build pairs carry fixed re-serialization signatures

When the two snapshots' TES4 header `Version` differs (e.g. 279 → 283), skip these:
- SPEL/ENCH/PERK effect rows gain `Effect Item Data / _unknown 2` (eight zero bytes) while
  `Effect Flags`, `Cooldown Duration`, `Effect ID` and the record's `Max Item ID` go null. The
  rows' magnitudes, durations and conditions stay byte-identical, so a move there is a real
  candidate; `Area` is unreliable (heterogeneous values landing on one number is the tell).
- PERK effect rows: `Float`/`Perk Entry ID` "changes" are fake until the permutation test or a
  live `get` says otherwise; `Perk Condition Tab Count` 4 → 3.
- OMOD `Properties[] / Value 2` 2 → 3; script `extra_bind_data_version` 4 → 3 and script-name
  case normalization; QUST objective flag bit 0x10 cleared; INFO `Previous INFO` and REFR `Layer`
  relinks.
- WEAP `Sneak Attack Multiplier` and OMOD `Attribute Descriptor Keywords` appearing or vanishing
  wholesale. On a same-build pair, `Attribute Descriptor Keywords` changes are real data.

**Example:** 20260821→20260903 across 51 unrelated ability spells (e.g. `abDogmeatHealthBonus`
0x00215CD3); `Area` 100 → 0 on `DetectLifePATargetCloakSpell` (0x00247A41).
*verified 2026-09-14 vs 20260821/20260903*

## `Value 2` on a FormID,Int property is never read

On a `Value Type = FormID,Int` OMOD property (Keywords, MaterialSwaps, ImpactDataSet, ZoomData,
ModelSwap), `Value 1` holds the FormID and `Value 2` carries no meaning, so any `Value 2` delta
there is noise, including the form_version pair `Step: 0.0 → null` with `Value 2: 1 → 3` that
repeats across unrelated records. On a property whose `Value 2` is read, a lone change is real if
it diverges from every sibling carrying the same `Property` within one snapshot (flag it
Unconfirmed). The same holds inside ARMO/WEAP `Object Template / Combinations[]` properties.

**Example:** 20260710→20260717, `Step`/`Value 2` churn on ARMO colour-palette swaps, Letterman's
Jacket, Vault 118 Jumpsuit and OMOD Bowling Ball Launcher.
*verified 2026-07-22 vs 20260717; FormID,Int rule found 2026-09-14*

## `Value Currency` null → Caps001 is schema population

A `Value Currency` field appearing as `0x0000000F (CNCY: Caps001 "Cap")` across unrelated
purchasable WEAP/ARMO/MISC records is a newly decoded field defaulting to the universal currency.
No other currency has been observed there; it is not an economy story.
*verified 2026-07-22 vs 20260717*

## ARMO/ARMA `First Person Flags` converging to one value is schema population

Unrelated ARMO/ARMA records' `Biped Body Template / First Person Flags` collapsing from
heterogeneous multi-flag sets to one identical value in one patch is schema population, not a
clipping fix.

**Example:** 20260814, Wading Jacket (0x0089A8B2) and Enclave Scientist Outfit (0x008D502D) both
land on `0x8000000` from different values.
*verified 2026-08-15 vs 20260814*

## The QUST schema-population cluster

Unrelated QUSTs going null → default on this cluster are form_version population **unless a field
carries a non-default value**: `Actor Reserve Flags` (none), `Actor Reserve Type` (None), `Public
Event Data` (Very Easy / 0 / 0.0), `QQSD - Unknown 4 bytes` (00000000), `QTFS (Repeat Limit?)`
(65535), `Quest Modules` (one empty struct), `Quest Start Data` (all-zero hex), and `General /
Flags` gaining `Has Dialogue Data` (+0x8000). It co-occurs with permutation churn and also runs in
reverse on old hub QUSTs, dragging satellite TERM/ACTI/MESG records whose only changes are
`Unknown CTRN`, Enlighten padding or `Activator Can Be Instanced`.

**Example:** QUST `EMS` (0x0012D5B8) plus 176 satellites on 20260903.
*verified 2026-07-22 vs 20260717*

## Undecoded blobs that change on every save are bookkeeping

CELL `Unknown 2` is a little-endian u64 Unix timestamp (a last-saved stamp), and ACTI/TACT/TERM
`Unknown CTRN` bumps only its ID bytes. Neither carries gameplay.

**Example:** 20260814→20260821, 1,484 cells' `Unknown 2` moved from 2026-06 to 2026-08.
*verified 2026-08-28 vs 20260821*

## WEAP `Animation *` fields are cosmetic, not gameplay speed

`Data / Animation Attack Seconds`, `RGW3 / Animation Reload Seconds`, `Bolt Draw Speed` and
`Animation Fire Seconds` are animation-asset timing, decoupled from `Speed`, `Reload Speed` and
`Melee Speed`. A `RGW2`/`FNAM` → `RGW3` struct rename riding along is a naming shuffle.

**Example:** 20260710→20260717, Combat Knife and Hunting Rifle kept `Speed`/`Reload Speed`/`Melee
Speed` byte-identical while many melee weapons' `Animation *` values converged on 1.1388938.
*verified 2026-07-22 vs 20260717*

---

# Text churn

## A `Localized` flag flip changes how text is stored, not what it says

The TES4 `Localized` flag (0x80) flips every few months: PTS builds keep text inline until content
freezes. The decoder renders inline text identically to the string tables and the diff drops the
tables' NBSP → space and LF → CRLF rewrites, so a text change on a flip pair is real. If one
differs only in non-ASCII glyphs, compare both snapshots' string tables by ID before reporting it.

**Example:** LGDI `RA_LegendaryItems_Weapons_Rank4` (0x00863A9A) reads `¬¬¬¬ Normal Weapon Rewards`
from the table on 20260821 and inline on 20260903, and does not appear in the diff.
*verified 2026-09-15 vs 20260821/20260903*

## Terminal `<STAT=X>` tokens are renderer substitutions

Personal-terminal "My Stats" TERMs (`X01X_PlayerTerminal_Stats*`) use `<STAT=Fish Caught>`, or
`<STAT=Fish Caught: 007CE4D3>` where the trailing hex is the counted object's FormID. The terminal
renderer resolves them and these TERMs carry no VMAD, so there is nothing to chase. A migration
from the older `<Token.Name=FishCaught>` form makes every stat line look changed.
*verified 2026-07-24 vs 20260724*

## Description text lags the effect chain: verify the magnitude, not the string

A description changing its stated magnitude does not imply the effect changed. Chase the actual
magnitude (which may be a curve or GLOB, see `mechanics.md`) before calling a buff or nerf.

**Example:** 20260717, `HTO_mod_Legendary_Armor4_Raging` (0x0085B997) text "+3% Damage" → "5%
Damage", but its PERK → SPEL → MGEF chain was untouched at 5.0: a text correction, not a buff.
*verified 2026-07-22 vs 20260717*

---

# Semantics traps

## Leveled-list `Chance None` is inverse

A LVLI entry's `Chance None Value`/`Chance None Global` is the percent chance of **nothing**; the
item's odds are `100 − Chance None`, so a feeding GLOB going **up** is a **nerf**. Exception: when
sibling entries each carry their own `Chance None Global` and the list's `Use All` flag is cleared
(replaced by an undecoded bit such as `0x200`), the globals are relative weights for one pick,
often summing to a round total; read them as a ratio (`HTO_crLLS_Rewards_Legendary_Mob_Weapons_Melee`
0x008F2B1A: 470/330/200). LVLN lists do the same, but their per-entry `LVOC` globals decode under
`_unmapped` as little-endian GLOB FormIDs (`HIDE_LChar_RobotWildcard` 0x0094F8E4: 85/15).

**Example:** `UniqueWeaponSkinDropChance` (0x008FF251) 80.0 → 90.0: the skin recipe's odds fell
20% → 10%.
*verified 2026-07-22 vs 20260717; weighted variant 2026-08-15 vs 20260814; LVLN 2026-09-20 vs 20260918*

## A reward row gated on `GetRandomPercent <=` a 0-valued GLOB was already off

When a GMRW/LVLI loses a row whose only condition is `GetRandomPercent <= <GLOB>` and that GLOB is
0.0 on the old side, the removal cleans up a drop that could never roll. Get the GLOB on the OLD
snapshot before writing any loss; a `DEL_`/`zzz` rename of the GLOB in the same diff confirms it.

**Example:** 20260914, seven Workshop Attack/Pitt GMRWs (e.g. 0x0063124B) drop
`P62_LLS_Rewards_TheDrifter_ActivationKeyCard` (0x00824D11), gated on GLOBs all at 0.0.
*verified 2026-09-14 vs 20260903*

## A leveled list's newly appended unconditional entry is a safety net

When every entry in an LVLN/LVLI carries a tier or `LocationHasKeyword` condition and a new entry
arrives with **no** Conditions, it is a catch-all so the list never returns nothing, not a new
tier. Confirm every other entry kept its condition.

**Example:** all eight Infestation (`HTO_`) faction boss LVLNs (e.g.
`HTO_LChar_Faction_BloodEagle_Boss` 0x0085A386) gained an unconditioned `_T5_Fallback` entry.
*verified 2026-08-03 vs 20260803*

## Two CNDFs testing exclusive GLOB states in one Conditions list can't both be true

A flat `Conditions` array's AND/OR combination isn't reliably invertible by hand when two
referenced CNDFs require different values of the same GLOB. Report the named conditions and any
unchanged baseline number, and flag the combined semantics Unconfirmed.

**Example:** `SDOW_DailyOps_LL_Rewards_RepeatTier` (0x008FCEA5) combines a 25% `GetRandomPercent`
with `SDOW_DailyOps_SlasherForced_CNDF` (Selection_Index==1) and `..._SlasherPref_CNDF` (==2).
*verified 2026-08-03 vs 20260803*

## New content often arrives as renames of recycled records

A diff shows it as **changed** EditorID/Name, not added. Retired `zzz_BOUNTY_` legendary OMODs and
COBJs get reused for new legendary effects, so never assume the old side was live: read its
description and properties before writing what it "used to do". A seasonal quest made permanent
is a QUST rename plus a `QTFS (Repeat Limit?)` change (65535 = no limit).

**Example:** 20260710, `zzz_BOUNTY_mod_Legendary_Weapon2_Insane` (0x0083DA6D) → Cryologist's;
`SDOW_SQ01_Graves_Repeatable` (0x008F1665) "(Seasonal) Laid to Unrest" → "(Repeatable) Disturbed
Grave", QTFS 65535 → 50.
*verified 2026-07-14 vs 20260710*

## An ENCH dropping N → N−1 effects is often a consolidation, not a nerf

When a unique-mod ENCH loses an effect and the survivor is a shared MGEF (check `refs`), suspect a
Script → native archetype rewrite: the bespoke MGEF went native and made the shared one redundant.

**Example:** 20260710, `ench_QuickFix` (0x0091995B): `AbQucikFix_Description` (0x0091995C) became
native on `weaponSpeedMult` and shared `AbPerkFortifyMeleeSpeedEffect` (0x003E9567) was dropped;
same curve `UniqueMods\Bonus_QuickFix.json` both sides.
*verified 2026-07-14 vs 20260710*

## A named unique OMOD can be cosmetic-only for patches at a time

`CustomItem_SpeciallyNamed`/`CustomItemName_*` tagging, a reward-list entry and a flavorful name
don't prove a live mechanic. Diff the OMOD's `Data/Properties` count and contents against the old
snapshot before calling a change a tweak: it may be the mod's first functional effect.

**Example:** `mod_Custom_MintyBreather` had only two cosmetic keyword ADDs until 20260717 added a
Perks ADD granting a repurposed, un-hidden PERK.
*verified 2026-07-22 vs 20260717*

## A paint OMOD losing its `ma_*_Appearance` tag is pool-scoping, not a stat change

`ma_Melee_Appearance` (0x005117B1) and `ma_Gun_Appearance` (0x0037D0B2) tag the shared random
cosmetic pools. A unique paint losing one (keyword removal or a new `REM Keywords`, no other
change) is scoped to its own source; flag the signal, since obtainability isn't provable from the
diff. A REM of the wrong pool tag is a no-op, so a swap to the right tag is the real scoping event.

**Example:** 20260717, Blue Ridge Branding Iron, Cultist Piercer and Head Hunter Paint lost
`ma_Melee_Appearance`; 20260814, gun paints `mod_custom_HolyFire_Effect` (0x006E06A3),
`..._TheKabloom_Effect` (0x006E2242) and `..._EldersMark_Effect` (0x006E2246) swapped their REM to
`ma_Gun_Appearance`.
*verified 2026-07-22 vs 20260717; gun swap 2026-08-15 vs 20260814*

## A legendary-combination `Attach Point Index` fix is a mesh-attach correction

Within `Object Template / Combinations[N]...Includes[]`, base-part mods share `Attach Point Index:
0`; a legendary row at `1` is a visual attachment outlier, and correcting it is cosmetic plumbing.

**Example:** 20260803, seven base weapons (Hunting Rifle, Knuckles, Laser Gun, Sickle, .44,
Sledgehammer, Pump Action Shotgun) each had one 1★ Include corrected `1` → `0`.
*verified 2026-08-03 vs 20260803*

## Armor/weapon combinations keyed on `ATX_if_tmp_Loadout_*` are Character Boost gear

New `Object Template / Combinations` rows whose `Keywords` hold
`ATX_if_tmp_Loadout_Level{50,100,150}Boost` are gear templates for the Atomic Shop Character Boost
loadout lists, not craftable variants, and unrelated to any mod or lining rollout in the same
patch. Resolve the keyword before attributing the rows.

**Example:** 20260914, Combat Armor pieces 0x0011D3C3–C7 gain rows used by
`ATX_LL_Loadout_Level100Boost_Armor`/`Level150Boost_Armor` (0x008F4766/0x008F476C).
*verified 2026-09-14 vs 20260914*

## Creature leveling migrates record by record, and a cross-branch pair shows it reversed

Creatures move from `Actor Scaling Info` Level Min/Max plus `Renorm_*_TierNN` GLOBs and a top-level
`zzzCT_Creatures_Health_*` curve to perk `crGlowingCreatureLevelAdjust` (0x008464F5, "Mod NPC
Normalized Level" +10), a Health `Properties[]` entry on `CT_Creatures_Health_Universal_TierNN`,
and `Renorm_{Max,Min}LVL_GlowingCreature` (min 1, max 100). The rollout spans unrelated NPCs, even friendly
vendors, so it is not a themed rework; `zzz` duplicates stay on the old system. A snapshot from a
branch forked before the migration shows the reverse (plus `EncounterSkullIndex` 0x007ADDD9 and
`Value Currency` dropping): a build fork, not a nerf.

**Example:** 20260717, Prime Cave Cricket, Prime Gulper and the vendor Grahm migrated; 20260821 →
20260903 (Pets branch) reverted 55 records including `E02A_LvlCaveCricket_Prime` (0x00553710).
*verified 2026-09-14 vs 20260821/20260903*

## A new mod COBJ's `Created Object` can point at the wrong record

On a mod rollout, check each added COBJ's `Created Object` against its EditorID: copy-paste leaves
recipes crafting another weapon's OMOD (whose `Target OMOD Keywords` won't match) or the loose-mod
MISC instead of the OMOD. Report these as wiring bugs, not as new mods.

**Example:** `co_mod_M79_Stock_SecondaryDMG` (0x0094F164) creates `mod_PlasmaGun_Grip_SecondaryDMG`
(0x0018C468); `co_mod_GaussPistol_Grip_HipAccuracy` (0x0094F7EA) creates MISC 0x0094F7F8.
*verified 2026-09-20 vs 20260918*

## A WAVE losing a spawn definition can be a gender-pair collapse

A `Wave Encounter Definitions` count of 2 → 1 is not half the enemies when the old pair was a
fixed-male and a fixed-female NPC_ and the replacement templates Traits from a mixed face LVLN.
Check the removed Spawn References' EditorIDs for `_M`/`_F` before writing a spawn-count change.

**Example:** `BountyHunt_Grunt_Toxic_WaveEnc` (0x00833A19) dropped 0x007D107D and 0x007D1083 for
0x007CFA9D, which rolls its face from `BountyHunt_Face_GhoulAndHuman_GenderAll` (0x007CFA80).
*verified 2026-09-20 vs 20260918*

## A SPECIAL `Maximum Value` of float-max means uncapped

`3.4028235e+38` is "no cap", not missing data. Since 20260717 all seven SPECIALs share a 100.0
ceiling (Strength, Endurance, Agility and Luck moved from float-max).
*verified 2026-07-22 vs 20260717*

---

# Lint false positives

## `dangling_ref` on engine forms and navmesh links

The lint reads `esm diff`'s typed `dangling_refs`: FormID fields a record newly points at that
resolve in neither snapshot. Bitfields, enum sentinels and hashes never lint. Two typed
references resolve nowhere without being broken:
- `0x00000014` is the engine's hardcoded PlayerRef, so a new `Run On: Reference` condition
  targeting it lints (`PowerArmorImpactEnchantment` 0x0011D53B, `DLC01Bot_KnockdownSpell`
  0x0010EB2A).
- NAVI `Navmesh Info / Edge Links` and `Preferred Edge Links` hold navmesh-local link ids (NAVI
  0x00000FF1).

**Example:** a new SPEL condition `Run On: Reference` → `0x00000014` lints; it is the player.
*verified 2026-09-14 vs 20260914; typed lint 2026-09-27*

## `desc_changed_stats_same` fires on records with no description change

The rule counts any string change with a side longer than 20 characters as a description change,
so it fires when the only changes are an undecoded blob (`Unknown CTRN / hex` on TACT/TERM,
`Unknown / hex`, STAT `Distant LOD`), a bare `Model / Model FileName` swap, or an `Editor
ID`/`Filter` rename. Check that the record has a
description field that actually changed before trusting the lint.

**Example:** 77 of 116 lints in one deep slice were blob-only; also TACT
`TEST_ENB_ModusSceneTerminal` (0x00006DB5), `SDOW_MQ02_Graves_GraveActivator` (0x008F1672, model
swap) and FISH `Fishing_Fish_Small_Axolotl_Gold` (0x0091391B, zzz rename).
*verified 2026-09-14 vs 20260914*

## `desc_changed_stats_same` misses stats that move through a linked GLOB or a swapped include

The lint only compares the record's own fields. An OMOD whose `Data / Includes` row swaps one
`_PARENT_` template for another (`mod_GaussPistol_Barrel_Suppressed_Base` 0x0054A170) changes every
stat the templates carry while its own `Properties` stay put; diff the two templates. A description change whose magnitude lives on a
referenced GLOB (`Effect.Magnitude`, `Quantity Global`) reads as text-only even when the number
moved; check every such reference before trusting the lint.

**Example:** `WorldPets_Dog_ConsumableBuff` MGEF (0x008B7A75) "every hour" → "every 30 min", called
text-only; its GLOB `WorldPets_ConsumableGiftInterval` moved 7000.0 → 1801.0.
*verified 2026-09-03 vs 20260903; includes 2026-09-20 vs 20260918*

## `unreferenced_perk_rank` on perks granted outside a PCRD

Perks granted by an OMOD/ENCH `Perks` property have no PCRD, and `STAT_BeneficialPerk`
(0x0018ADAD) is attached directly to the Player NPC_ (0x00000007); verify with `refs <perk-id>
--type PCRD --paths`. Some obtainable cards have no PCRD and no reference at all (Lady
Killer/Black Widow, Critical Banker, Pickpocket, Blitz, Intimidation, I'm Cured!), so an empty
`refs` never proves a rank ungrantable: call it orphaned only when the card is unobtainable in game.

**Example:** `LadyKiller01` (0x00019AA3) has zero refs and no `LadyKillerCard` record, while
`Sneak01` (0x0004C935) resolves to `SneakCard` (0x0034409F).
*found 2026-09-14*

## `lvli_blocked_entry`'s `quantity_zero` on an entry that isn't dead

`Quantity: 0.0` does not disable an entry. With a `Quantity Global` the global is the count and the
flat field is a stale placeholder; without one, 0 means "use the sublist's own count". Sibling
entries with the same shape that are known live confirm it.

**Example:** `HTO_crLLD_Mob` (0x0085CDB6)'s Scrap entry (0x00893F7D): Quantity 0.0 with `Quantity
Global` = 3.0, the same shape as its live ContextualAmmo sibling.
*verified 2026-08-15 vs 20260814*

## HAZD `Data / Flags` has an unmapped bit

A flag-only HAZD change has no story: the changed bit has no derivable meaning, and xEdit's
`wbDefinitionsFO76.pas` names only bits 0–6, with bit 6 itself "Unknown 6".

**Example:** 20260710, the bit cleared on 6 hazard clouds.
*verified 2026-07-14 vs 20260710*
