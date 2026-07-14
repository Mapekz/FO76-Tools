# FO76 Mechanics Knowledge Base

Accumulated derivations of game mechanics, for patch-notes deep writers. Consult this
BEFORE chasing refs — chase only mechanics not covered here, and propose new entries (same
format) in your report when you derive one. Entries are point-in-time: each carries the
snapshot it was verified against. Treat entries older than a few months as hints — re-verify
the backing records with one live `get` before asserting their semantics in a draft.

## How unique-weapon effects are implemented (the chase pattern)

A `mod_Custom_*` / `*_mod_Custom_*` OMOD usually implements its mechanic in one of three ways:

1. **Direct property** — an ADD/SET on a weapon stat or actor value in the OMOD's own
   `Data/Properties`. Read the AVIF's name AND find its consumer (`refs` on the AVIF) before
   asserting what it does.
2. **Perk grant** — Property `116`/`Perks` ADD of a PERK. Item-granted perks have **no PCRD**;
   the `unreferenced_perk_rank` lint is a false positive for them. Verify the grant path via
   `refs` on the perk instead.
3. **Keyword hook** — the OMOD only ADDs a `CustomItemName_*` / `dn_*` KYWD; the real mechanic
   lives elsewhere in a SPEL/PERK effect conditioned on `WornHasKeyword(<that keyword>)`.
   Chase: `refs` on the keyword → fetch the referencing SPEL/PERK → find the effect whose
   Conditions test the keyword → read magnitude/curve/other conditions.

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
- Verified: 2026-07-13 vs snapshot 20260710.

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
- `STAT_DmgMultCryo` / `STAT_DmgMultFire` / `STAT_DmgMultPoison`: flat elemental damage
  bonus. Cryo/Fire are also fortified by the Science! INT perk family ranks named
  "Cryologist" (`ScienceMaster01`) and "Pyro-Technician" (`ScienceExpert01`); no live perk
  consumes the Poison variant as of 20260710.
- Since 20260710 these stats replace bespoke enchantment→perk script chains on several
  legendary mods (tech migration — same numbers, new plumbing). A blank OMOD Description
  alongside a STAT_* ADD usually means the tooltip auto-generates from the stat's own text.
- Verified: 2026-07-13 vs 20260710.

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
- Verified: 2026-07-13 vs 20260710.

## World Pets (NOT LIVE as of 20260710)

- Built-but-gated system: C.A.M.P. pet (Cat/Dog/Deathclaw/Radhog) follows you with
  commands and a hidden 1–200 "Pet Prowess Level" (AVIF `WorldPets_PetProwessLevel`-family).
- Pet Prowess perk ranks live on the `CAMPPets_Actor_*` NPC templates (item/NPC-granted, no
  PCRD — expected). Damage ×1→×8 and incoming damage ×1→×0.2 across level brackets
  1-49/50-99/100-149/150-199/200.
- Not live because: the `IsWorldPet` KYWD gating the follow package is applied to nothing;
  the World Pet faction has zero refs; a kill-switch spell ("Pet buffs are disabled")
  exists; the four command emotes are on the Atomic Shop hide list.
- Distinct from the older `PETS_`-prefixed adoptable-companion quest system (relationship
  unconfirmed).
- Verified: 2026-07-13 vs 20260710.

## Property-name errata (schema vs engine)

- `MinPowerPerShot` → **MaxPowerPerShot**, fixed in the schema 2026-07-14 (see Charge weapons
  above). Pre-fix analyses/data may still carry the old name — treat it as the same field.

## Known schema gaps (unmapped fields seen in real diffs)

- HAZD: an unknown flag bit (cleared on 6 hazard clouds in 20260710) — gameplay meaning
  unmapped.
- QUST `QTFS`: undecoded field (changed "no limit" → 50 on a repeatable-quest conversion in
  20260710) — likely repeat-limit/cooldown, unconfirmed.
