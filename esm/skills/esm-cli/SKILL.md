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

- Subcommands: `daemon, info, get, list, search, refs, tree, diff, coverage,
  chase, walk, skill`.
- ESM path comes from the global `--esm <PATH>` flag (long only; works before
  or after the subcommand) with `FO76_ESM_PATH` env fallback — a plain process
  env var, there is no `.env` parser. The path may be the `.esm` file or its
  containing data folder. Exceptions: `diff <FILE_A> <FILE_B>` (two
  positionals), `daemon` (no path; resolves at spawn), and `skill` (no path —
  it only reads its own embedded doc).
- **Always pass `-p` for one-shot calls** — it auto-spawns/uses a warm daemon
  and exits after printing. Without `-p` the CLI drops into a REPL. `--local`
  runs cold in-process (seconds per open — never use it for bulk work).
  `--mmap-index` is a FormID-only lite mode: no EditorID lookups at all.
- After rebuilding the binary or changing loose files next to the dump, run
  `esm daemon stop` — the daemon caches both the old code and the loose-file
  view.

## Fetching records

- Selectors are `0x...` formids or EditorIDs. Bare tokens that *look* numeric
  auto-resolve FormID-first with EditorID fallback; scripts should still pass
  explicit `--formid`/`--edid` where the flag exists to skip the ambiguity.
- **Bulk get**: `esm -p get <sel1> <sel2> … --json` — one target returns the
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

- `esm -p refs --formid <0x...> [--depth N] [--type SIG] [--paths] --json`.
- `--depth` 1–6: direct referrers at 1; raise it to reach a target through an
  intermediary (e.g. a quest alias).
- `--type <SIG>` filters to ONE 4-char referrer type server-side (not a
  comma list) and composes correctly with `--limit`/`--depth`.
- `--paths` annotates each row with the JSON field path(s) from referrer to
  target (e.g. `Effects[2].Conditions[0].Parameter 1`). It decodes every
  emitted row, so it's opt-in.
- The default `--limit 100` truncates popular targets. The "output capped"
  note goes to **stderr** — stdout stays valid JSON under `--json`. Pass
  `--limit 0` when you need everything.

## Mechanics digests: `walk` and `chase`

- `esm -p walk <selector> [--refs] [--depth N] [--json]` — one compact digest
  instead of a chain of raw `get` dumps. Follows the standard chains
  (SPEL/ENCH/ALCH `Effects[]` → MGEF → "Perk to Apply" → PERK / "Equip
  Ability" → SPEL; PERK Ability → SPEL; OMOD → ENCH properties), resolves
  AVs/GLOBs/keywords to editor ids, prints curve points, and falls back to a
  search when the selector doesn't resolve. Walking a KYWD or AVIF
  reverse-chases its SPEL/PERK consumers (`refs --type … --paths`) instead of
  dumping the mostly-empty record. `--depth` caps chain-following (default 2;
  use 3 for OMOD → granted-perk). `--refs` appends grouped reverse references
  (see obtainability below).
- `esm -p chase <selector> [--depth N] [--ref-limit N] [--json]` — the
  mechanism taxonomy. An OMOD selector classifies each `Data.Properties[]`
  row: bare number (nothing to chase), directly-attached ENCH/SPEL, PERK
  grant (property 116), or KYWD/AVIF hook (reverse `refs --type SPEL/PERK
  --paths` to find the gated `Effects[N]` via `WornHasKeyword(...)`). A
  PERK/SPEL/ALCH/ENCH selector walks its own `Effects[]` directly. Both walks
  auto-follow one extra hop when a `Base Effect` MGEF carries "Perk to Apply"
  (→ PERK) or "Equip Ability" (→ SPEL).
- Reach for `chase` to decode an OMOD's mechanism or resolve an
  ENCH → MGEF → PERK proc chain in one call; use `walk` for everything else
  (MGEF archetypes, curves, GLOBs, conditions, PERK entry points, WEAP stats,
  obtainability review).

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

## Obtainability verdicts (`walk --refs`)

- Player-facing referrer types: COBJ, GMRW, LGDI, QUST, CONT, MISC, FLST.
  LVLI counts only through player-facing chains (NPC-loadout-only lists
  don't); referrers with NONPLAYABLE in the editor id are flagged.
- **No reverse refs at all is normal** for script/VMAD quest rewards,
  vendor/gold-bullion grants, and account-side (ATX) items — absence of refs
  is not evidence of junk.
- **The record graph cannot distinguish shipped from unshipped content** —
  cut or unreleased items can look perfectly obtainable on-record. Confirm
  release status externally before treating an unfamiliar record as real.

## Curve tables

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
record (e.g. `esm -p get GaussRifle`) and grep the actual field names before
trusting fixtures, extractor code, or prior notes.
