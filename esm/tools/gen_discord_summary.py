#!/usr/bin/env python3
"""
Generate a condensed Discord-friendly patch summary from diff JSON.
Produces a ~50-100 line summary suitable for posting in a few Discord messages.
Usage: python3 tools/gen_discord_summary.py /tmp/fo76_diff.json > discord_summary.md
"""

import json
import sys
from collections import defaultdict

OLD_LABEL = "20260612"
NEW_LABEL = "20260619"

TYPE_DESC = {
    "ALCH": "Ingestibles/Chems", "AMMO": "Ammo", "ARMO": "Armor/Apparel",
    "AVTR": "Scoreboard Unlocks", "BOOK": "Books/Plans/Holotapes",
    "CHAL": "Challenges", "COBJ": "Recipes", "DIAL": "Dialogue Topics",
    "ENCH": "Enchantments", "ENTM": "Menu Entries", "EXPL": "Explosions",
    "FISH": "Fish", "GLOB": "Global Variables", "INFO": "Dialogue Responses",
    "KYWD": "Keywords", "LCTN": "Locations", "LIGH": "Lights",
    "LVLI": "Leveled Item Lists", "MGEF": "Magic Effects", "MISC": "Misc Items",
    "MSTT": "Movable Statics", "MSWP": "Material Swaps",
    "NOTE": "Notes", "NPC_": "NPCs", "OMOD": "Weapon/Armor Mods",
    "PACK": "AI Packages", "PERK": "Perks/Abilities", "PROJ": "Projectiles",
    "QUST": "Quests", "REFR": "World References", "SCEN": "Scenes",
    "SNDR": "Sound Descriptors", "SPEL": "Spells/Abilities",
    "STAT": "Static Objects", "WEAP": "Weapons",
}

HIGHLIGHTS = """## ⚡ FO76 ESM Diff: {old} → {new}
> **{added}** records added · **{removed}** removed · **{changed}** changed · **{total}** total

---
### 🔫 Weapons & Mods
- **Railway Rifle** mag capacity: `10 → 14`
- **Stormcutter** +2 new Keywords (enables new mod slots) + new `mod_melee_Stormcutter_Standard`
- New **DeathTambo** mods: Spikes Small, Blades
- New **RailwayRifle** mods: Anti-ScorchBeast Receiver, Splitter Receiver
- 7 new **Enchantments**: GrandFinale, Kingfisher, WhackerSmacker, Longshot, PiratePunch, DeathTambo Bleed, Red Terror Burning

---
### 🏚️ Hostile Takeover (Infestations)
- **Ammo rewards DOUBLED+**: Boss `15→30`, Mob `5→10`, Support `1→5`
- **Fortify Bash (Boss)**: `500 → 1000` (doubled)
- **Fortify Health** feature **enabled**: toggle `0 → 1`
- **3★ Legendary no-drop chance** decreased: Support `90%→85%`, Mob `80%→75%` (→ more legendaries)
- 4 new Fortify Damage globals (Boss/Mob/Support + Toggle)
- New boss corpse-highlight VFX/MGEF + teleport-in explosion effect

---
### 🎃 Slasher Event (SDOW) — "Pint-Sized Slasher" Rebrand
- All Slasher items renamed: **"Slasher X" → "Pint-Sized Slasher X"**
- New hat color variants: Blue, Red, Green, Orange Pint-Sized Slasher Hat
- "Grave Keeper" NPC → **Phantom Gravekeeper**; Ghost NPC overhauled
- New holotape quest: `SDOW_Holotape03_RadioBroadcasts`
- New legendary reward loot list for Slasher drops
- New NPC patrol AI package added

---
### ⭐ Perks Redesigned
| Perk | Old | New |
|------|-----|-----|
| CrowdControl | +1 PER per kill (streak) | +5% Limb Damage per kill (streak) |
| LoveTap | +1 CHA per kill (streak) | +10% Bash Damage per kill (streak) |

**New Perks Added:** SoleSurvivorPerk, EldersMark, custom_TickettoRevenge, mod_custom_V63-BERTHA_Perk
Also: Lone Wanderer perk effects updated

---
### 🐾 C.A.M.P. Pets
- All pet NPCs (cats, dogs, 70+ records): Keyword Count `10 → 11` — new system keyword
- **"Disabled Pet Buffs" spell renamed to "Pet Buffs"** — pet world buffs may now be active
- New workshop-linking script added to all pets (VMAD changes)

---
### 🎣 Fishing
- New rare fish: **Gold Axolotl** (leveled lists + catch-rate global)
- New player title prefix: `Gillded` (Fishing_PlayerTitles_Prefix_Gillded)

---
### 🎭 Emotes & Cosmetics
- **Lucky Dice emote**: animations `1 → 6`
- **MadeInTheShade** emote renamed to **"Greasin' Up"**
- **Good Luck** emote renamed to **"Shrug"** (new animation, new icon)
- **Green** player title corrected to **"Overgrown"**
- ATX Season 26 scoreboard: 6 new player icons (Bat Tamer, Bei, Castle, Night Person, Psychopath, T-51b)

---
### 📦 Loot & Crafting
- 31 leveled item lists updated (melee mod pools +2 entries each for new DeathTambo/Stormcutter mods)
- Samuel vendor inventory: `38 → 41` items
- Workshop poster wall-decor list: `105 → 108` entries
- 10 new craftable recipes (COBJ), 9 existing recipes modified
- Slasher legendary reward loot list added

---
### 🌐 World Changes (REFR)
- **170 new** placed object references (SDOW event world objects: graves, skulls, spawns)
- **7 removed** placed references (old SDOW markers replaced)
- 76 existing references repositioned/updated

---
### 📋 Full Record Type Summary
"""

def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/fo76_diff.json"
    with open(path) as f:
        data = json.load(f)

    added   = data["added"]
    removed = data["removed"]
    changed = data["changed"]

    added_by   = defaultdict(int)
    removed_by = defaultdict(int)
    changed_by = defaultdict(int)
    for r in added:   added_by[r["record_type"]] += 1
    for r in removed: removed_by[r["record_type"]] += 1
    for c in changed: changed_by[c["stub"]["record_type"]] += 1

    all_types = sorted(set(list(added_by) + list(removed_by) + list(changed_by)))
    # Skip CELL/WRLD
    all_types = [t for t in all_types if t not in ("CELL", "WRLD")]

    header = HIGHLIGHTS.format(
        old=OLD_LABEL, new=NEW_LABEL,
        added=len(added), removed=len(removed), changed=len(changed),
        total=len(added)+len(removed)+len(changed)
    )

    table_lines = []
    table_lines.append("| Type | Description | +Added | -Removed | ~Changed |")
    table_lines.append("|------|-------------|--------|----------|----------|")
    for t in all_types:
        a = added_by.get(t, 0)
        r = removed_by.get(t, 0)
        c = changed_by.get(t, 0)
        desc = TYPE_DESC.get(t, "")
        a_s = str(a) if a else "—"
        r_s = str(r) if r else "—"
        c_s = str(c) if c else "—"
        table_lines.append(f"| `{t}` | {desc} | {a_s} | {r_s} | {c_s} |")

    footer = f"\n---\n*Full detailed diff: `patch_notes_{NEW_LABEL}.md` ({len(added)+len(removed)+len(changed)} records)*"

    print(header + "\n".join(table_lines) + footer)

if __name__ == "__main__":
    main()
