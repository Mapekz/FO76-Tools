# Diff traps

Things that look like a change but aren't, plus the false positives our own lints produce. Read
this **before** writing up any diff row. Companion file: `mechanics.md`.

When you hit one of these the correct output is usually *nothing* — or a single "under the hood"
line. Never a gameplay claim.

Same entry format as `mechanics.md`: a claim as the heading, 2-4 sentences, one worked example,
one verified line. No decision history.

---

# Serialization & schema-population churn

## Positional array reindex churn is a reserialization artifact

Many `_array_diff` `changed` entries whose from/to field sets are identical but permuted to new
indices are not new content. **Confirm the same value multiset exists on both sides before
reporting** — compare the serialized multiset, don't eyeball it.

Seen in VMAD `scripts[]`, VMAD `script_fragments`, VMAD alias/property name-casing slots, RACE
`Attacks[]` / `Bone Scale Data[]`, LCTN `Master Reference` / `Master Unique NPCs`, and NPC_
`Attacks[]`.

**VMAD `scripts[]` is the highest-risk surface, because the false positive reads as a feature.**
A permuted script list looks exactly like "a bespoke script replaced the generic one" — the diff
shows `name` changing at an index, and the two scripts at that index have different property
counts, which then reads as "properties 3 → 6." Both readings are artifacts of comparing two
different scripts that happen to share a slot. **Compare script lists as a name multiset with
per-script property counts, never index by index.** A script named in the *from* side is only
removed if its name is absent from the whole *to* side. Corollary: globals or aliases bound by a
script that merely moved are not new bindings — check whether the target records existed in the
old snapshot before calling the logic new.

The tell-tale differs by record type. RACE-style shows mirrored numeric pairs (a `Damage Mult` 1.0
→ 1.5 at one index paired with the exact reverse elsewhere). **NPC_ `Attacks[]` shows no mirrored
pair** — creature attack entries often share identical Attack Data and differ only in `Attack
Event` (`meleeStart_N` / `_Mirrored`), so it surfaces as a wave of string changes instead. Absence
of the mirrored tell is not evidence against the artifact.

A related false positive inside `Attacks[].Conditions`: `Condition Data / Parameter 1` on
`IsPreviousMeleeAttackEvent` is a raw string-pointer int that shifts whenever the record is
re-serialized; the decoded value is the sibling `Parameter #1` (the attack-event name). A
`Parameter 1` delta with `Parameter #1` unchanged is not a change. Example: Sheepsquatch
(0x00479D50), 39 attacks, every `Parameter 1` moved, no event name moved.

**Example:** 20260717→20260724, 17 creature NPC_ records (EncMolerat03 0x001832F8, three
WendigoColossusSpawn variants, DEL_E09A_EncUltraciteAbomination, five RD01_Enc05_* Ultragenetic
creatures, three Burning and two Emperor Radscorpion variants, HTO_LvlMoleMiner_Molerat_BroodMother)
had `Attacks` as their only changed path; bulk-getting both sides proved the multiset identical in
all 17, order alone differing.

**Example (VMAD `scripts[]`):** QUST `Burn_BountyHunt_Headhunt` (0x007EBDF4) showed
`Virtual Machine Adapter / scripts` as its only changed path, with
`defaultquestencounterwavescript` at index 0 replaced by
`Burn:Burn_Bounty:Burn_Bounty_HeadhuntSpawnScript`. Both sides in fact carry the identical six
scripts with identical property counts (3, 6, 12, 2, 5, 5) — the generic wave script moved to
index 5 and the Head Hunt spawn script moved up from index 1. The three
`Burn_BountyHunt_RecentHeadhuntGang_0N` globals it binds (0x00833A0A–0C) already existed at −1.0
in the old snapshot, so no anti-repeat logic was added.
*verified 2026-07-24 vs 20260724*

## `Value Currency` null → Caps001 is schema population

A `Value Currency` field appearing from null to `0x0000000F (CNCY: Caps001 "Cap")` across a huge
range of unrelated purchasable WEAP/ARMO/MISC records is a newly-decoded field defaulting to the
universal currency — never anything else observed. Not an economy story.
*verified 2026-07-22 vs 20260717*

## `Step: 0.0 → null` paired with `Value 2: 1 → 3` is serialization noise

This exact pair on `Material Swaps`-style OMOD-Properties / Object-Mod-Template-Item entries,
appearing identically across dozens of unrelated records with no other content change, is the
signature of a global serialization-format change from a form_version bump. Don't read the `1 → 3`
as a real multiplier change without an independent live cross-check. A lone `Value 2` change with
no `Step` field present (or in either direction, not just 1→3) is not automatically this artifact —
pull every sibling record sharing that exact `Property` name; if they agree on one value and yours
diverges, it's a real outlier worth an Unconfirmed flag, not noise to drop. Run that sibling test
*within one snapshot*: on a `Value Type = FormID,Int` property (Keywords, Material Swaps,
ImpactDataSet, ZoomData, ModelSwap) Value 1 carries the FormID and Value 2's int is never read, so
if unrelated records in the same snapshot already disagree on it the slot carries no meaning.
This holds inside ARMO/WEAP `Object Template / Combinations[].Object Mod Template Item / Properties`
too.

**Example:** 20260710→20260717 across ARMO colour-palette swaps, Letterman's Jacket, Dirty Postman
Uniform, Brotherhood Scribe Outfit, Laundered Dresses, Grafton Monsters Jacket, Keep Out Backpack,
Vault 118 Jumpsuit, and OMOD Bowling Ball Launcher. Counter-example 20260803: Gatling Gun's
Irradiated Paint (0x007AE53F) `Value 2` 3→1 while all 13 sibling Irradiated/Gatling paint OMODs
stayed at 3 — a genuine outlier, flagged Unconfirmed rather than dropped.
*verified 2026-08-03 vs 20260803*

## ARMO/ARMA `First Person Flags` converging to one value is schema population

When unrelated ARMO/ARMA records' `Biped Body Template / First Person Flags` collapse from
heterogeneous multi-flag sets to one identical value (e.g. `0x8000000`) in a single patch, it's
schema population, not a per-item first-person clipping fix — heterogeneous "before" values
converging on one "after" is the tell. The field's `dangling_ref` lint hits are also false
positives: it renders like a FormID but is a bitfield (same shape as NPC_ `Attack Flags`).

**Example:** 20260814 — Wading Jacket (0x0089A8B2) and Enclave Scientist Outfit (0x008D502D) both
land on `0x8000000` from different values; ARMA 0x008D502C lands on `0x4900F838`, flagged
`dangling_ref`.
*verified 2026-08-15 vs 20260814*

## The QUST schema-population cluster

Dozens of unrelated public-event-style QUSTs going from null to all-default on this exact cluster
in one diff: `Actor Reserve Flags` (none), `Actor Reserve Type` (None), `Public Event Data` (Very
Easy / 0 / 0.0), `QQSD - Unknown 4 bytes` (00000000), `QTFS (Repeat Limit?)` (65535 = no limit),
`Quest Modules` (one empty struct), `Quest Start Data` (all-zero hex), and `General / Flags`
gaining exactly `Has Dialogue Data` (raw value +0x8000). Every value is a schema default, and it
co-occurs with the positional-reindex churn above on the same records. Treat the whole cluster as
form_version schema population **unless a field in it carries a non-default value**. It also runs
in reverse (populated defaults collapsing to null) on old hub QUSTs, dragging 100+ satellite
TERM/ACTI/MESG records whose only changes are `Unknown CTRN`/Enlighten padding/`Activator Can Be
Instanced` — still churn (QUST `EMS` 0x0012D5B8 + 176 satellites on 20260903).
*verified 2026-07-22 vs 20260717*

## WEAP `Animation *` fields are cosmetic, not gameplay speed

`Data / Animation Attack Seconds`, `RGW3 / Animation Reload Seconds`, `Bolt Draw Speed` and
`Animation Fire Seconds` are animation-asset timing metadata, decoupled from the DPS-affecting
stats. Live-checking Combat Knife and Hunting Rifle across 20260710→20260717 showed `Speed`,
`Reload Speed` and `Melee Speed` byte-for-byte unchanged while the `Animation *` family moved (many
melee weapons converging on exactly 1.1388938) across a large batch of starter weapons. Related: a `RGW2`/`FNAM` → `RGW3` struct
rename rides along with this churn — a naming shuffle, not new data.
*verified 2026-07-22 vs 20260717*

---

# Text churn

## A `Localized` flag flip makes every text field look changed

The TES4 header's `Localized` flag (0x80) decides whether `FULL`/`DESC` hold a 4-byte lstring ID or
inline text, and it has flipped in both directions: false → true between 20260710 and 20260717
(flags `0x01` → `0x81`), and true → false between 20260821 and 20260903 (`0x81` → `0x01`, the Pets
branch). Check it with `esm info` on both sides. Whichever way it moved, one side stores text
inline — tagged `<ID=xxxxxxxx>`, with non-ASCII arriving as U+FFFD replacement glyphs. Even
with per-side string tables wired correctly the round-trip normalizes text, so one patch yields
tens of thousands of text "changes" that are really mojibake repairs (`Mj?lnir` → `Mjölnir`;
`????` → `¬¬¬¬`, the legendary star-rating prefix), leading/trailing whitespace and newline churn
on `Description`/`Header Text`/`Body Text`, and centering whitespace on terminal headers. **Only
treat a text diff as a story when the ASCII wording changed.**

**Example:** LGDI `RA_LegendaryItems_Weapons_Rank4` (0x00863A9A) reads `¬¬¬¬ Normal Weapon Rewards`
on 20260821 and `���� Normal Weapon Rewards` on 20260903, where the new side stores
`<ID=3929B8F4>` plus raw single-byte text inline. The string table's bytes are identical on both
sides.
*verified 2026-09-14 vs 20260717 and 20260821/20260903*

## Terminal stat tokens migrated to `<STAT=X>` syntax

The personal-terminal "My Stats" family (`X01X_PlayerTerminal_Stats*`) changed its
stat-substitution syntax from `<Token.Name=FishCaught>` to `<STAT=Fish Caught>`, with a
parameterized variant `<STAT=Fish Caught: 007CE4D3>` where the trailing hex is the counted object's
own FormID — so one generic counter key serves 62 fish species (combat instead uses one named key
per line, `<STAT=Deathclaws Killed>`). Substitution is resolved by the terminal-text renderer and
these TERMs carry no VMAD, so there is nothing script-side to chase. Expect every stat line to look
changed on syntax alone.
*verified 2026-07-24 vs 20260724*

## Description text lags the effect chain — verify the magnitude, not the string

An OMOD/ENCH description changing its stated magnitude does **not** imply the effect changed. Chase
the actual magnitude before calling it a buff or nerf — and see `mechanics.md` on `Magnitude: 0.0`
beside a curve table, because "the magnitude" is not always the `Magnitude` field.

**Example:** 20260717, the Head Hunts `Raging` armor mod (`HTO_mod_Legendary_Armor4_Raging`,
0x0085B997) rewrote "Upon being hit, deal +3% Damage for 10 seconds" → "Gain 5% Damage for 10
Seconds When Hit", but its PERK → SPEL → MGEF chain was untouched and carries magnitude 5.0 with no
3.0 anywhere. The old text was wrong; this is a correction, not a buff.
*verified 2026-07-22 vs 20260717*

---

# Semantics traps

## Leveled-list `Chance None` is inverse

A LVLI entry's `Chance None Value` / `Chance None Global` is the percent chance of getting
**nothing** from that slot; the referenced item's own odds are `100 − Chance None`. A GLOB feeding
`Chance None Global` going **up** is therefore a **nerf**. Weighted variant: when sibling entries
each carry their own `Chance None Global` and the list's `Use All` flag is cleared (replaced by an
undecoded bit, e.g. `0x200`), the globals read as relative weights for one weighted pick — often
summing to a round total like 1000 — not inverse percentages; treat them as a ratio (seen on
`HTO_crLLS_Rewards_Legendary_Mob_Weapons_Melee` 0x008F2B1A, weights 470/330/200, in 20260814).

**Example:** `UniqueWeaponSkinDropChance` (0x008FF251) 80.0 → 90.0 between 20260710 and 20260717 —
a unique weapon-skin recipe's real odds fell 20% → 10%.
*verified 2026-07-22 vs 20260717; weighted variant 2026-08-15 vs 20260814*

## A SPECIAL `Maximum Value` of float-max means uncapped, not missing data

Perception (0x000002C3), Charisma (0x000002C5) and Intelligence (0x000002C6) were already capped at
100.0. Strength (0x000002C2), Endurance (0x000002C4), Agility (0x000002C7) and Luck (0x000002C8)
carried `3.4028235e+38` until 20260717, when all four were set to 100.0 — so all seven SPECIALs now
share one ceiling.
*verified 2026-07-22 vs 20260717*

## New legendary effects arrive as renames of recycled Bounty FormIDs

Bethesda reuses FormIDs from long-dead `zzz_BOUNTY_`-prefixed legendary weapon mods/COBJ recipes
(the retired Bounty event) for brand-new legendary content instead of allocating fresh ones — so
they show up in a diff as **"changed" EditorID/Name renames, not "added" records**. When chasing a
changed legendary OMOD/COBJ with a `zzz_BOUNTY_` prev_editor_id, don't assume the old effect was
ever live or obtainable; check the old snapshot's description and property list before writing what
it "used to do".

**Example:** 20260710 — `zzz_BOUNTY_mod_Legendary_Weapon2_Insane` (0x0083DA6D) → Cryologist's,
`..._Melee_Pulsating` (0x00849316) → Pyro-Technician's, `..._Guns_Rebate` (0x00849317) →
Poisoner's, all retargeted to `ma_legendarycrafting_weapon`.
*verified 2026-07-14 vs 20260710*

## An ENCH dropping N → N−1 effects is often a consolidation, not a nerf

When a unique-mod ENCH loses an effect and a surviving effect is a generic/shared MGEF also used
elsewhere (check `refs`), suspect a Script→native archetype consolidation: the bespoke Script MGEF
gets rewritten to the native archetype, making the shared one redundant.

**Example:** `ench_QuickFix` (0x0091995B, Switchblade "The Quick Fix") carried shared MGEF
`AbPerkFortifyMeleeSpeedEffect` (0x003E9567, native Peak Value Modifier on AVIF `weaponSpeedMult`)
plus its own `AbQucikFix_Description` (0x0091995C, Script archetype). In 20260710 the bespoke MGEF
became native on the same AV (flags 0x8A02) and the shared one was dropped, 2 effects → 1. Same
curve both sides: `UniqueMods\Bonus_QuickFix.json`, AddictionCount → swing speed (0=+0%, 10=+50%).
*verified 2026-07-14 vs 20260710*

## A named unique OMOD can be cosmetic-only for patches at a time

Don't assume a named unique weapon mod already has a live mechanic just because its
`CustomItem_SpeciallyNamed` + `CustomItemName_*` keyword tagging, its reward leveled-list entry and
a flavorful `Name` are all wired up. Always diff the OMOD's actual `Data/Properties` count and
contents against the prior snapshot before describing a change as a magnitude tweak — it may be the
mod's first functional effect ever.

**Example:** `mod_Custom_MintyBreather` had exactly that shape — 2 cosmetic keyword-ADDs, zero
functional properties — since at least 20260710; 20260717 added its first gameplay property (a
Perks ADD granting a heal-on-friendly-hit perk, repurposing an unused PERK record via an
EditorID/Description rename plus flipping its `Hidden` flag).
*verified 2026-07-22 vs 20260717*

## A paint OMOD losing `ma_Melee_Appearance` is pool-scoping, not a stat change

`ma_Melee_Appearance` (0x005117B1) is the generic "any melee weapon cosmetic" pool tag; guns use
the analogous `ma_Gun_Appearance` (0x0037D0B2). A unique/quest-reward paint losing one — via a
direct `Target OMOD Keywords` removal or a new `REM Keywords` property — likely scopes that paint
to its own dedicated source instead of the shared random-cosmetic pool. Flag this shape (keyword
removal with no other property change, on a unique-named paint) as a pool-scoping signal, not a
numeric change; the downstream obtainability isn't provable from the diff alone. A `REM Keywords`
can also target the *wrong* pool tag as a bug — a REM of `ma_Melee_Appearance` on a gun mod is a
no-op, and a patch swapping it to `ma_Gun_Appearance` is the real pool-scoping event, not the
REM's mere presence.

**Example:** 20260717 — Blue Ridge Branding Iron Paint, Cultist Piercer Paint, Head Hunter Paint.
Gun-side fix 20260814: `mod_custom_HolyFire_Effect` (0x006E06A3), `mod_custom_TheKabloom_Effect`
(0x006E2242), `mod_custom_EldersMark_Effect` (0x006E2246) corrected `ma_Melee_Appearance` →
`ma_Gun_Appearance`.
*verified 2026-07-22 vs 20260717; gun analog 2026-08-15 vs 20260814*

## The Glowing-Creature leveling migration is not Scorched-exclusive

The `crGlowingCreatureLevelAdjust` perk (entry point "Mod NPC Normalized Level", ADD +10) plus a
swap onto dedicated `Renorm_{Max,Min}LVL_GlowingCreature` GLOBs (min 1 / max 100) and a generic
`CT_Creatures_Health_Universal_TierNN` health curve lands on unrelated NPC categories — in
20260717 on Prime Cave Cricket and Prime Gulper, **and on Grahm**, a friendly non-combat vendor.
Read it as a broad leveling-system migration (old Actor-Scaling-Info + Renorm-offset GLOB model →
perk-based normalized-level adjustment) rolling out record-by-record, not a themed creature rework.
The `zzz`-prefixed legacy duplicates were left on the old system, confirming the live records are
the ones being migrated.
*verified 2026-07-22 vs 20260717*

## A leveled list's newly-appended unconditional entry is a safety net, not a new tier

When every existing entry in an LVLN/LVLI carries a `LocationHasKeyword`/tier condition and a new
entry is appended with **no** Conditions, it's a catch-all so the list never returns nothing when
no condition matches — not a new selectable tier or a stealth buff. Confirm by checking that every
other entry keeps its own condition unchanged.

**Example:** All eight Infestation (`HTO_`) per-faction boss LVLNs (e.g.
`HTO_LChar_Faction_BloodEagle_Boss`, 0x0085A386) gained a `_T5_Fallback` NPC_ entry with no
Conditions, appended after their five keyword-gated Tier 1–5 entries.
*verified 2026-08-03 vs 20260803*

## Two CNDFs testing mutually-exclusive GLOB states in one Conditions list can't both resolve true

A flat `Conditions` array's AND/OR combination isn't reliably invertible by hand when two
referenced CNDF forms require different values of the same GLOB. Report the named conditions and
any unchanged baseline number, and flag the combined semantics Unconfirmed rather than asserting
which branch wins.

**Example:** `SDOW_DailyOps_LL_Rewards_RepeatTier` (0x008FCEA5) combines a 25% `GetRandomPercent`
with `SDOW_DailyOps_SlasherForced_CNDF` (Selection_Index==1) and `SDOW_DailyOps_SlasherPref_CNDF`
(Selection_Index==2), which can never both be true in the same evaluation.
*verified 2026-08-03 vs 20260803*

---

# Lint false positives

## `dangling_ref` on NPC_ `Attack Flags` bitfields

Values `0x80000000`, `0x80000002`, `0x80000004` and `0x80000010` on creature NPC_ records are not
FormIDs — they are `Attacks[].Attack.Attack Data.Attack Flags.value`, where bit `0x80000000`
decodes as `Override Data`. Confirmed by enumerating the flags live on 0x005751A0, 0x0078C584 and
0x0080100A. 32 of 116 lints in one deep slice were this alone. Related: `0xFFFFFFFF` `dangling_ref`
hits on INFO records are the `Responses[].Response Data.Emotion` enum sentinel (verified on
0x0092C628–2B), also not a FormID. AVIF `Flags` bitfields misfire the same way: `0x80000800` on `FollowerState`
(0x00000344) is "Default to 1.0" + "Hardcoded", not a reference. Two more non-FormID sources:
`0x00000014` is the engine's hardcoded PlayerRef, so any `Condition Data / Reference` with
`Run On: Reference` targeting it lints as dangling (`PowerArmorImpactEnchantment` 0x0011D53B,
`DLC01Bot_KnockdownSpell` 0x0010EB2A), and NAVI `Navmesh Info / Edge Links` and `Preferred Edge
Links` hold navmesh-local link ids rather than FormIDs (NAVI 0x00000FF1).
*verified 2026-07-24 vs 20260724; AVIF case 2026-09-03; PlayerRef and NAVI cases 2026-09-14*

## `desc_changed_stats_same` on undecoded hex blobs

The rule reports "description changed but no numeric stat changed" when the only changed path is an
undecoded binary field — notably `Unknown CTRN / hex` on TACT/TERM records and `Unknown / hex`. No
description is involved at all. 77 of 116 lints in one deep slice were this shape (68 CTRN, 9
Unknown, plus 1 on STAT `Distant LOD` binary blobs). The rule should skip paths ending in `/ hex`
or flagged `_raw`. Also fires on a bare `Model / Model FileName` swap with no `Description` field
on the record at all — same fix: check the record's actual description field before trusting the
lint's premise. Spot-verified on TACT `TEST_ENB_ModusSceneTerminal` (0x00006DB5), whose sole change
is `Unknown CTRN / hex`, and on `SDOW_MQ02_Graves_GraveActivator` (0x008F1672), a bare
`GraveActivator01.nif` → `GraveActivator_NoSkeleton.nif` swap with no Description field.
*verified 2026-08-03 vs 20260803*


## `desc_changed_stats_same` misses stats that move through a linked GLOB

The lint only compares fields on the record itself. A description change whose real magnitude
lives on a referenced Magnitude Global (see mechanics: World Pets GLOB-backed magnitudes) reads as
"text only" even when the number genuinely moved. Check every `Effect.Magnitude` / `Quantity
Global` reference before trusting the lint's premise.

**Example:** `WorldPets_Dog_ConsumableBuff` MGEF (0x008B7A75) description "every hour" → "every
30 min", lint called it text-only; the linked GLOB `WorldPets_ConsumableGiftInterval` moved
7000.0 → 1801.0.
*verified 2026-09-03 vs 20260903*

## `unreferenced_perk_rank` on item-granted and Player-attached perks

Perks granted by an OMOD/ENCH `Perks` property legitimately have no PCRD, and `STAT_BeneficialPerk`
(0x0018ADAD) is attached directly to the Player NPC_ record (0x00000007). Verify the grant path
with `refs <perk-id> --type PCRD --paths` instead of calling them orphaned. See `mechanics.md`.
Some ordinary obtainable perk cards also have no PCRD and no reference of any type, so an empty
`refs` result never proves a rank is ungrantable on its own — only call a rank orphaned when the
card is unobtainable in game too. Known members: Lady Killer/Black Widow, Critical Banker,
Pickpocket, Blitz, Intimidation, I'm Cured!. `LadyKiller01` (0x00019AA3) has zero refs and no
`LadyKillerCard` record exists, while `Sneak01` (0x0004C935) resolves to `SneakCard` (0x0034409F).

## `lvli_blocked_entry`'s `quantity_zero` reason is a false positive when the entry has a `Quantity Global`

A `Leveled List Entry` with `Quantity: 0.0` is not dead when it also carries a `Quantity Global` —
the engine reads the runtime quantity from that global; the flat `Quantity` field is a stale
placeholder. Confirm by checking sibling entries in the same list for the identical
zero-Quantity + nonzero-Global shape; if siblings are live, the flagged entry is too.

**Example:** `HTO_crLLD_Mob` (0x0085CDB6)'s Scrap entry (0x00893F7D): Quantity 0.0 with
`Quantity Global` = 3.0, same shape as its confirmed-live ContextualAmmo sibling.
*verified 2026-08-15 vs 20260814*

## HAZD `Data / Flags` has an unmapped bit

An unknown flag bit (cleared on 6 hazard clouds in 20260710) with no derivable gameplay meaning,
and not schema-fixable: xEdit's own `wbDefinitionsFO76.pas` names only bits 0–6, and bit 6 is
itself "Unknown 6". A flag-only HAZD change has no story.
*verified 2026-07-14 vs 20260710*

## SPEL/ENCH effect rows gaining `Effect Item Data / _unknown 2` is cross-build serialization

On a pair spanning two game builds, every SPEL/ENCH effect row can show eight zero bytes as
`Effect Item Data / _unknown 2` while `Effect Flags`, `Cooldown Duration`, `Effect ID` and the
record's `Max Item ID` collapse to null, with magnitudes, durations and conditions byte-identical.
`Area` values moving in those same rows are unreliable for the same reason; a heterogeneous set of
`Area` values all landing on one number is the tell.

**Example:** 20260821→20260903 across 51 unrelated ability spells, including
`abDogmeatHealthBonus` (0x00215CD3) and the eight `MTNM03_ZenSpell*`; `Area` 0 → 21 on nine of
them and 100 → 0 on `DetectLifePATargetCloakSpell` (0x00247A41).
*verified 2026-09-14 vs 20260821/20260903*

## PERK `Effects` diffs on a re-serialised build are slot churn, not balance

Test for a permutation before reading a PERK effect diff: collect each subpath's from/to multiset
across the changed rows, and if they match, the entries only swapped slots. `Perk Entry ID`, entry
point, function, `Float` and whole `Perk Conditions` lists move together and mirror each other.
Two co-occurring fields are pure noise either way: `Entry Point / Perk Condition Tab Count`
shifting by one is editor metadata, and `Actor Value, Float` appearing while `Float` and
`Function Parameter 3 (Actor Value)` go null is one schema struct replacing two fields.

**Example:** `PlayerPerk_Spotlight` (0x0046C7CF) reported 10 of 18 effect rows changed; both sides
carry the same ten `Ab_Spotlight_*` abilities plus eight empty slots, differing only in order.
*verified 2026-09-14 vs 20260903*

## A creature-leveling "reversion" on a Pets-branch pair is a build fork, not a nerf

When the newer snapshot is the Pets PTS branch, creature records look rolled back: the Health
`Properties[]` entry and its `CT_Creatures_Health_Universal_TierNN` curve give way to the legacy
top-level `Health Curve Table` on a `zzzCT_Creatures_Health_*` curve, `crGlowingCreatureLevelAdjust`
(0x008464F5) disappears, `Actor Scaling Info` Level Min/Max and `Renorm_*_TierNN` globals return,
and the `EncounterSkullIndex` (0x007ADDD9) property drops. The branch forked before that migration
landed. This also reverses the `Value Currency` schema-population trap above, in that direction.

**Example:** 55 records including `E02A_LvlCaveCricket_Prime` (0x00553710); `BobbyPin`
(0x0000000A) `Value Currency` Caps001 → null is the same fork.
*verified 2026-09-14 vs 20260821/20260903*
