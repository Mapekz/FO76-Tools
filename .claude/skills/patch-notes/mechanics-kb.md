# FO76 Mechanics Knowledge Base

Accumulated derivations of game mechanics, for patch-notes deep writers. Consult this
BEFORE chasing refs — chase only mechanics not covered here, and propose new entries (same
format) in your report when you derive one. Entries are point-in-time: each carries the
snapshot it was verified against. Treat entries older than a few months as hints — re-verify
the backing records with one live `get` before asserting their semantics in a draft.

## How unique-weapon effects are implemented (the chase pattern)

A `mod_Custom_*` / `*_mod_Custom_*` OMOD usually implements its mechanic in one of four ways:

1. **Direct property** — an ADD/SET on a weapon stat or actor value in the OMOD's own
   `Data/Properties`. Read the AVIF's name AND find its consumer (`refs --type SPEL --paths`
   / `--type PERK --paths` on the AVIF) before asserting what it does.
2. **Perk grant** — Property `116`/`Perks` ADD of a PERK. Item-granted perks have **no PCRD**;
   the `unreferenced_perk_rank` lint is a false positive for them. Verify the grant path via
   `refs --type PCRD --paths` on the perk instead.
3. **Keyword hook** — the OMOD only ADDs a `CustomItemName_*` / `dn_*` KYWD; the real mechanic
   lives elsewhere in a SPEL/PERK effect conditioned on `WornHasKeyword(<that keyword>)`.
   Chase: `refs --type SPEL --paths` (then `--type PERK --paths`) on the keyword → the
   `field_paths` on each hit already points at the gating `Effects[N].Conditions[...]` entry,
   so a bulk `get` on the hits resolves magnitude/curve/other conditions in one call.
4. **Projectile/explosion override chain** — the OMOD SETs Property `80`
   (`OverrideProjectile`) to a dedicated PROJ FormID; the real magnitude lives on that PROJ's
   linked EXPL record's `Data / Damage Curve Table`, not in the OMOD's own Properties. Chase:
   find the `OverrideProjectile` SET, `get` the PROJ, follow its Explosion field to the EXPL,
   read the damage curve there. Curve tables in this pattern are swapped wholesale by
   CurveTable FormID+name (e.g. `CT_Player_Damage_Universal_Tier28` → `..._Tier40`), so a
   bulk get across the old/new curve FormIDs quantifies the delta without touching the
   OMOD's Properties. Example: `RD01_Mod_Custom_ResolveBreaker_CustomName` (0x007934FE) →
   PROJ 0x007CA02E → EXPL 0x007CA02D. Verified: 2026-07-14 vs 20260710.

This walk is automated: `esm/target/release/esm -p chase <OMOD_FORMID_OR_EDID>` (ESM path from
`FO76_ESM_PATH`, or pass `--esm <PATH>`) runs all three patterns against a real OMOD and prints a
compact evidence tree (add `--json` for
agent consumption). Prefer it over chasing by hand; fall back to manual `refs`/`get` only for
patterns it doesn't cover (see `src/chase.rs`'s module docstring for limitations).

## Bullet Storm (Heavy Gunner perk mechanic)

- Stacks are earned by **spending ammo** — GMST `uAmmoSpenderAmmoUsePerStack` sets the
  ammo-per-stack rate. Not kills, not hits.
- Stack cap: AVIF `AmmoSpenderMaxStacks` (0x0083C3CB), fortifiable. Base cap **20** = 10
  unconditional + 10 conditioned on `HasPerk(HeavyGunnerMaster01)` — both effects live on
  SPEL `AbPerkHeavyGunner` (0x0031BE58) via MGEF `abAmmoSpenderFortifyStacks` (0x0083C3D1).
- Stack floor: AVIF `AmmoSpenderMinStacks` (0x00919957, "Minimum Bullet Storm Stack Count").
- Per-kill gain switch: AVIF `EnableAmmoSpenderOnKill` (0x00924DB9) — **boolean** (min 0/max 1,
  "Boolean" + "Default to 0" AVIF flags), SET to 1 = "gain 1 stack on kill". Consumer is
  native engine code (dead-end DFOB registration in ESM) — the AVIF description is the
  authoritative text.
- Damage scaling: curves `Perks\HeavyDamageBonus{,2,3}.json` on the same SPEL map stacks →
  damage bonus.
- Foundation's Vengeance conditional cap bonus (since 20260710): SPEL `AbPerkHeavyGunner`
  gained an Effect (Perk Entry ID 8, MGEF `abAmmoSpenderFortifyStacks`, Magnitude 5.0)
  conditioned on `WornHasKeyword(CustomItemName_FoundationsVengeance, 0x0064781E)` AND
  `GetHealthPercentage <= 0.25` — the E08B unique mod (0x0064781F) grants +5 max Bullet
  Storm stacks under 25% HP; its description matches the data exactly.
- Verified: 2026-07-13 vs snapshot 20260710 (Foundation's Vengeance addendum 2026-07-14).

## Kill Streak

- A shared counter: kills add stacks (base +1/kill), **cap 10**, decays after ~30s without a
  kill (per Adrenaline's description). Enabled via AVIF `EnableKillStreak` (0x0080B56A) /
  MGEF `abEnableKillStreak`.
- AVIF `KillStreakPerKillCount` (0x00924E31, "Kills Grant <+VALUE> Killstreak Count") adds
  extra stacks per kill on top of the base +1. Consumer is native engine (DFOB dead-end).
- Readers of the counter include Adrenaline (+10% damage per stack) and several unique-item
  perks (Unstoppable Monster, etc. — verify per case).
- Verified: 2026-07-13 vs 20260710.

## "Cheat Death" revive-effect family

- AVIF `CheatDeathResetOnWeakPointChance` (0x00924E29, "Revive Effect Cooldown"): "Attacks
  Against Weak Points Have a <VALUE> Chance to Reset a Revive Effect Cooldown" —
  percentage-flagged, so +30.0 = +30% chance per weak-point hit.
- Known revive effects sharing the cooldown framework: Life Saver, E.M.T., Power Armor
  Reboot, Scout Banner. (Found by EditorID search; not proven exhaustive.)
- Verified: 2026-07-13 vs 20260710.

## STAT_* damage-stat family (native actor values)

- `STAT_DmgVsBleeding` / `STAT_DmgVsBurning` / `STAT_DmgVsPoisoned` / `STAT_DmgVsFreezing`:
  "+X% damage vs targets currently under that status." Used by 4★ legendary weapon effects
  (Severing's, Pyromaniac's, Viper's, Icemen's since 20260710).
- `STAT_DmgMultCryo` / `STAT_DmgMultFire` / `STAT_DmgMultPoison`: unconditional elemental
  **additive damage bonus** (DBM family — see "Damage-bonus mechanisms" above; stacks
  additively with other damage bonuses, does not touch base damage). Cryo/Fire are also
  fortified by the Science! INT perk family ranks named
  "Cryologist" (`ScienceMaster01`) and "Pyro-Technician" (`ScienceExpert01`); no live perk
  consumes the Poison variant as of 20260710.
- Since 20260710 these stats replace bespoke enchantment→perk script chains on several
  legendary mods (tech migration — same numbers, new plumbing). A blank OMOD Description
  alongside a STAT_* ADD usually means the tooltip auto-generates from the stat's own text.
- Severing's confirmed chase (worked example of the migration): old side (20260702) was
  OMOD ADD Enchantments → ENCH `SDOW_ench_LegendaryWeapon_Severing` (0x008E0681) → MGEF
  (Script archetype, Perk to Apply) → PERK `SDOW_Legendary_Weapon_SeveringPerk` (0x008E0723),
  Entry Point "Mod Weapon DMG Bonus Mult" ADD 0.5, gated on
  `HasKeyword(SDOW_HasLegendary_Weapon_Severing)` AND
  `GetNumActiveEffectsWithKeyword(DamageTypeBleed)>=1`. New side ADDs `STAT_DmgVsBleeding`
  (0x00837DFC) Value2=50.0 directly; the old enchantment is `zzz`-vaulted. Same magnitude.
- **Exception — Icemen's is a mechanic swap, not a re-plumb.** Its pre-20260710
  implementation was a direct OMOD MUL+ADD on `DamageTypeValues` targeting `dtCryo`
  (Value2 0.2), not an enchantment chain — a **base damage increase**: +20% to the weapon's
  own base Cryo damage, or (on weapons with no Cryo damage) new base Cryo damage scaled off
  the weapon's existing base damage; always on. The 20260710 version ADDs +50
  `STAT_DmgVsFreezing` (0x0085A2F1) — a conditional **additive damage bonus**, only vs
  targets already in Freezing status. Base-damage self-buff became a conditional DBM
  contribution; the 20→50 magnitude change is on a different axis, so pre/post numbers are
  not comparable. Don't describe the
  STAT_* migration as semantics-preserving without checking the OLD implementation per mod.
- Verified: 2026-07-13 vs 20260710 (Severing's chase + Icemen's exception 2026-07-14 vs
  20260702/20260710).

## Damage-bonus mechanisms (reporting taxonomy)

A "+X% damage" in the data is one of three distinct mechanisms. Always identify which one
before writing a number — and name it in prose using this standard terminology:

1. **Additive damage bonus (DBM)** — a contribution to the damage-bonus-multiplier pool;
   stacks additively with all other damage bonuses (so its real value is diluted by the rest
   of the build). Implementations: ADD to a `STAT_DmgMult*` AV (unconditional — e.g. the
   20260710 2★ mods, +0.2 `STAT_DmgMultCryo/Fire/Poison`); ADD to a `STAT_DmgVs*` AV
   (conditional on target status — the 4★ family); the OMOD weapon property
   `DamageBonusMult` (e.g. BoomStick 0x00680832 Property 106, 1.5 → 0.75 = +150% → +75%);
   PERK Entry Point "Mod Weapon DMG Bonus Mult". Report property/AV values ×100 as
   percentages.
2. **Base damage increase** — changes to the weapon's own `AttackDamage` or
   `DamageTypeValues` (directly or via OMOD MUL+ADD on Property 77). Multiplies through
   everything downstream. A MUL+ADD on a damage type the weapon *lacks* ADDs new base damage
   of that type scaled off the weapon's existing base damage — so old Icemen's (+0.2 dtCryo)
   both boosted cryo weapons and added cryo damage to non-cryo weapons.
3. **Damage multiplier** — multiplies total outgoing damage after bonuses: power attack
   mult, body-part/weakpoint mults, Taking One for the Team, Follow Through, etc. Rare in
   legendary mods; strongest per point.

Never write a bare "+X% damage" in a draft — the mechanism determines how the number stacks
and is exactly the kind of distinction build-crafter readers need.
Verified: 2026-07-15 vs snapshots 20260702/20260710.

## Creature weapons & damage curves (enemy-only items)

- An enemy WEAP's `Damage Curve` (e.g. `CT_Creatures_Damage_Universal_TierNN`) has
  **x = wielder level**. Never quote the curve's first point as "the damage" — evaluate at
  the wielding NPC's actual level(s): the NPC_ record's fixed level plus its
  `Renorm_MinLVL_TierNN` / `Renorm_MaxLVL_TierNN` GLOB bounds (get the GLOBs — e.g. Tier06
  min = 25, Tier07 max = 175 as of 20260710). Interpolate linearly between curve points
  (that's what the engine's `Curve::eval` does). Worked example: Slasher Knife/Throwing
  Knife (0x00927375/76) share `CT_Creatures_Damage_Universal_Tier30` → 104 dmg at boss
  default level 100, ≈245 at its Tier07 max level 175.
- **Combat inventory ≠ loot.** An NPC_'s inventory/Object Template lists what it *fights
  with*; only the death-item/reward LVLI chain (e.g. `*_LL_BountyDrop_*`) is
  player-obtainable. Check both before writing "drops" — an inventory-only weapon is
  described as "the boss attacks with it", never as a drop, and never as "can roll
  legendary mods" (template wiring on an enemy instance is irrelevant to players).
- Verified: 2026-07-15 vs snapshots 20260702/20260710.

## OMOD property semantics

- Property `116` = `Perks` (grant perk; see item-granted-perk note above).
- `MUL+ADD`: effective = base × (1 + Value1) + Value2 — standard FO4/76 convention, inferred
  from worked examples, not confirmed against engine code.
- Curve-table x-axis on armor-mod carry-weight/etc. curves = **item level** (break points
  1/10/20/30/40/50).
- A curve table on a property overrides Value2 as the magnitude source; curve removed +
  Value2 changed = scaling replaced by a flat value.

## Charge weapons (Gauss family)

- WEAP `Data / Full Power Seconds` = time to reach full charge (Gauss Rifle base: 1.0s).
- WEAP `Data / Max Power Per Shot` = the full-charge damage multiplier (Gauss Rifle base: 2.0).
  The engine renamed this property from `MinPowerPerShot` to `MaxPowerPerShot` (~2025); our
  schema emitted the stale `MinPowerPerShot` name (and the same stale name on the OMOD property
  enum entry) until the extractor's property-name list was fixed 2026-07-14. Older
  analyses/data captured before that date may still show `MinPowerPerShot` — treat it as the
  same field.
- Worked example: Flatliner (`RD01_Mod_Custom_StrikeBreaker_CustomName`, 0x00793512) as of
  20260710 ADDs +1.0 Max Power Per Shot (2.0→3.0, full-charge bonus +100%→+200%) and +0.5
  Full Power Seconds (1.0s→1.5s to full charge), replacing an ADD Perks 116 grant of
  `mod_weapon_penetrating` ("Projectiles penetrate up to three targets").
- Verified: 2026-07-13 vs 20260710 (base values live-confirmed 2026-07-14).

## World Pets (NOT LIVE as of 20260710)

- Built-but-gated system: C.A.M.P. pet (Cat/Dog/Deathclaw/Radhog) follows you with
  commands and a hidden 1–200 "Pet Prowess Level" (AVIF `WorldPets_PetProwessLevel`-family).
- Pet Prowess perk ranks live on the `CAMPPets_Actor_*` NPC templates (item/NPC-granted, no
  PCRD — expected). Damage ×1→×8 and incoming damage ×1→×0.2 across level brackets
  1-49/50-99/100-149/150-199/200.
- Not live because: the `IsWorldPet` KYWD gating the follow package is applied to nothing;
  the World Pet faction has zero refs; a kill-switch spell ("Pet buffs are disabled")
  exists; the four command emotes are on the Atomic Shop hide list — added to FLST
  `ATX_HideFromStoreList` (0x004875A1) specifically in the 20260710 patch (emotes
  0x00916200–0x00916203, absent from the list on 20260702).
- Distinct from the older `PETS_`-prefixed adoptable-companion quest system (relationship
  unconfirmed).
- Verified: 2026-07-13 vs 20260710 (hide-list dating 2026-07-14).

## Legendary mod FormID recycling (retired "Bounty" event slots)

- Bethesda reuses FormIDs from long-dead, already `zzz_BOUNTY_`-prefixed legendary weapon
  mods/COBJ recipes (retired Bounty event) for brand-new legendary content instead of
  allocating fresh FormIDs — these show up in diffs as "changed" EditorID/Name renames, not
  "added" records.
- 20260710 examples: `zzz_BOUNTY_mod_Legendary_Weapon2_Insane` (0x0083DA6D) → Cryologist's
  (+20% Cryo via `STAT_DmgMultCryo`); `zzz_BOUNTY_mod_Legendary_Weapon2_Melee_Pulsating`
  (0x00849316) → Pyro-Technician's (+20% Fire via `STAT_DmgMultFire`);
  `zzz_BOUNTY_mod_Legendary_Weapon2_Guns_Rebate` (0x00849317) → Poisoner's (+20% Poison via
  `STAT_DmgMultPoison`). All three retarget to `ma_legendarycrafting_weapon` (any weapon).
- When chasing a "changed" legendary OMOD/COBJ with a `zzz_BOUNTY_` prev_editor_id, don't
  assume the old effect was ever live/obtainable — check the description and property list
  on the OLD snapshot before asserting what it "used to do" for players.
- Verified: 2026-07-14 vs snapshots 20260702/20260710.

## Script→native archetype consolidation (unique-mod enchantments)

- `ench_QuickFix` (0x0091995B, Switchblade "The Quick Fix") used to carry two effects: the
  generic shared MGEF `AbPerkFortifyMeleeSpeedEffect` (0x003E9567, native "Peak Value
  Modifier" targeting AVIF `weaponSpeedMult` 0x00000312) plus its own
  `AbQucikFix_Description` (0x0091995C, Script archetype, tooltip-curve only). As of
  20260710 the bespoke MGEF is itself converted to the native archetype targeting
  `weaponSpeedMult` (same flags 0x8A02), making the shared effect redundant — dropped
  (2 effects → 1). Same curve both sides: `UniqueMods\Bonus_QuickFix.json`, AVIF
  AddictionCount → swing-speed bonus (0=+0%, 1=+5%, 10=+50% cap).
- General pattern: when a unique-mod ENCH drops from N effects to N−1 and one survivor is a
  generic/shared MGEF also used elsewhere (check refs), suspect a Script→native archetype
  consolidation rather than a nerf.
- Verified: 2026-07-14 vs snapshots 20260702/20260710.

## Slasher Season (seasonal event) — Year 2 structure

- Quest chain: `SDOW_MQ01_Bodies` "(Seasonal) Masked Truth" (0x008F15C1) → `SDOW_MQ02_Graves`
  "(Seasonal) Secrets to the Grave" (0x008F15A1) → MQ04 "Out of the Shadows" (0x008F15C2) →
  MQ05 "Blood Will Have Blood" (0x008F15C3), tracked by radio quest `SDOW_SQ_DebunkerRadio`
  "The Debunker News" (0x008EDF32) via VMAD script properties and GLOB toggles named
  `LCP_SDOW_Slasher` / `LCP_SDOW_LTC_<Activity>Toggle` (GraveDigging/HeadHunts/DailyOps/
  MischiefNight).
- Repeatable side activity: `SDOW_SQ01_Graves_Repeatable`, renamed in 20260710 from
  "(Seasonal) Laid to Unrest" to "(Repeatable) Disturbed Grave" (0x008F1665) — a seasonal
  one-off converted into a permanent repeatable; its `QTFS (Repeat Limit?)` field flipped
  "no limit" (65535) → 50 (this is the concrete example behind the QTFS schema-gap closure
  below).
- Bosses: `SDOW_LvlSlasherFanBossPowerArmorHeavyAuto` "Pint-Sized Phantom Destroyer" (Daily
  Ops, 0x008E06B3) and `SDOW_Burn_BountyTarget_BIG_Slasher` "The Reborn Pint-Sized Slasher"
  (bounty target, 0x008E06C5) — both got stat-tier and combat-style additions in 20260710;
  both reference the "SlasherBoss"-tagged unique weapons (Slasher Knife 0x00927375,
  Throwing Knife 0x00927376).
- Reward loot tables `SDOW_LL_Rewards_Activities_SlasherMaps` (0x008F2B68) and
  `SDOW_LL_Rewards_PublicEvents_SlasherMaps` (0x00904724) both gate the "Pint-Sized
  Phantoms' Map" drop (BOOK `SDOW_MQ02_SlasherMap`, 0x008F15E4).
- Verified: 2026-07-14 vs snapshots 20260702/20260710.

## Property-name errata (schema vs engine)

- `MinPowerPerShot` → **MaxPowerPerShot**, fixed in the schema 2026-07-14 (see Charge weapons
  above). Pre-fix analyses/data may still carry the old name — treat it as the same field.
- The same errata also applied to the raw WEAP `Data` (DNAM) struct field itself — xEdit's
  Pascal source still emits `Min Power Per Shot` verbatim, so full-record decodes (`esm get`
  on a WEAP FormID) kept showing the stale name even after the OMOD property-list fix above.
  Patched via a `record_patches` override in `schema/fo76.overrides.json` (WEAP → `Data` →
  `Min Power Per Shot`), fixed 2026-07-14. Same field, same semantics as the property-list
  entry — both now read `Max Power Per Shot`.

## Known schema gaps (unmapped fields seen in real diffs)

- HAZD: an unknown flag bit (cleared on 6 hazard clouds in 20260710) — gameplay meaning
  unmapped. Not schema-fixable: xEdit's own Pascal source (`wbDefinitionsFO76.pas`) names
  only bits 0–6 of `Data / Flags`, and bit 6 is itself `"Unknown 6"` — there is no
  authoritative name to adopt, upstream or otherwise. Renaming the bit also wouldn't change
  patch-notes triage: `is_numeric_change_entry` routes on the raw hex `Data / Flags / value`
  string, not the bit label, so a flag-only HAZD change stays `ambiguous` either way (see
  `test_hazd_flag_bit_only_change_is_not_auto_deep` in `tools/tests/test_triage_bundles.py`).
- QUST `QTFS`: **closed 2026-07-15.** Was an undecoded 2-byte blob; the "no limit" → 50 flip
  on the repeatable-quest conversion below reads cleanly as a little-endian u16, so it's now
  mapped via a `record_patches` override in `schema/fo76.overrides.json` (allowlisted in
  `tools/extractor/parity-exceptions.json` since xEdit's Pascal still calls it a byte array).
  Decodes as **`QTFS (Repeat Limit?)`** — the `?` flags that "repeat limit" is inferred from
  this one data point, not confirmed from engine source. `0xffff` (65535) reads as "no limit".
  This also fixed a triage gap: a QTFS-only change is now a real numeric delta on a major
  record type (QUST) and auto-DEEPs instead of falling to ambiguous.
