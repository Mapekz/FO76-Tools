#!/usr/bin/env python3
"""
Generate a comprehensive patch-notes markdown from the ESM diff JSON.
Usage: python3 tools/gen_patch_notes.py /tmp/fo76_diff.json > patch_notes.md
"""

import json
import sys
from collections import defaultdict

# --------------------------------------------------------------------------
# Configuration
# --------------------------------------------------------------------------

OLD_LABEL = "SeventySix_20260612.esm"
NEW_LABEL = "SeventySix_20260619.esm"
PATCH_DATE = "2026-06-19"

# Record types to skip entirely (world-placement, positional, not parsed)
SKIP_DETAIL_TYPES = {"CELL", "WRLD"}

# Record types where raw-hex unmapped changes dominate — only show summary count.
# REFR and ACHR are now schema-covered so they show full field diffs.
RAW_HEAVY_TYPES: set[str] = set()

# Short descriptions for record types
TYPE_DESC = {
    "ACHR": "Actor (placed NPC instance)",
    "ACTI": "Activator",
    "ADDN": "Addon Node",
    "ALCH": "Ingestible / Food / Chem",
    "AMMO": "Ammunition",
    "ARMO": "Armor / Apparel",
    "ARTO": "Art Object",
    "AVIF": "Actor Value Info",
    "AVTR": "Avatar (Scoreboard unlocks)",
    "BOOK": "Book / Holotape / Note",
    "CHAL": "Challenge",
    "CNDF": "Condition Form",
    "COBJ": "Constructible Object (Recipe)",
    "CONT": "Container",
    "CURV": "Float Curve",
    "DFOB": "Default Object",
    "DIAL": "Dialogue Topic",
    "EFSH": "Effect Shader",
    "EMOT": "Emotion",
    "ENCH": "Object Effect (Enchantment)",
    "ENTM": "Entry Type Menu",
    "EXPL": "Explosion",
    "FISH": "Fish",
    "FLST": "Form List",
    "FURN": "Furniture",
    "GLOB": "Global Variable",
    "IDLE": "Idle Animation",
    "INFO": "Dialogue Response",
    "INNR": "Instance Naming Rules",
    "KYWD": "Keyword",
    "LAYR": "Layer",
    "LCRT": "Location Reference Type",
    "LCTN": "Location",
    "LIGH": "Light",
    "LVLI": "Leveled Item List",
    "LVLN": "Leveled NPC List",
    "LVLP": "Leveled Perk List",
    "MDSP": "Material Swap",
    "MESG": "Message",
    "MGEF": "Magic Effect",
    "MISC": "Misc Item",
    "MSTT": "Movable Static",
    "MSWP": "Material Swap",
    "MUSC": "Music Type",
    "MUST": "Music Track",
    "NOTE": "Note",
    "NPC_": "Non-Player Character",
    "OMOD": "Object Modification (Mod Slot)",
    "PACK": "Package (AI)",
    "PERK": "Perk / Ability",
    "PLYT": "Playlist",
    "PMFT": "Phone Message",
    "PROJ": "Projectile",
    "QUST": "Quest",
    "RACE": "Race",
    "REFR": "Placed Object Reference (world)",
    "SCEN": "Scene",
    "SNDR": "Sound Descriptor",
    "SPEL": "Spell / Ability",
    "STAT": "Static Object",
    "TRNS": "Transform",
    "WAVE": "Water Wave",
    "WEAP": "Weapon",
    "WTHR": "Weather",
}

# Feature prefix → human-readable name + description
FEATURE_MAP = {
    "HTO":    ("Hostile Takeover (Infestations)", "HTO_ records govern the Hostile Takeover / Infestation seasonal event."),
    "SDOW":   ("Slasher Down on the West (SDOW Event)", "SDOW_ records control the Slasher limited-time event, including NPC spawns, AI packages, loot, and world references."),
    "SCORE":  ("Scoreboard / S.C.O.R.E.", "SCORE_ records define scoreboard season rewards, challenges, and progression items."),
    "ATX":    ("Atomic Shop", "ATX_ records cover Atomic Shop cosmetic items (armor, apparel, skins, camp items)."),
    "Fishing":("Fishing System", "Fishing_ records configure fish spawn rates, catch odds, and rewards."),
    "XPD":    ("Expeditions (Atlantic City & Beyond)", "XPD_ records relate to Expeditions missions."),
    "LCP":    ("Living Colonial Park", "LCP_ records are linked to the Living Colonial Park event space."),
    "RD01":   ("Raid 01", "RD01_ records relate to an upcoming raid encounter."),
    "LLS":    ("Leveled List System (LLS)", "LLS_ records control loot drop tables."),
    "Workshop":("Workshop / C.A.M.P.", "Workshop_ records affect base-building placeables."),
    "CAMPPets":("C.A.M.P. Pets", "CAMPPets_ records define pet companions available at C.A.M.P."),
    "WorldPets":("World Pets", "WorldPets_ records define pets that exist in the open world."),
    "E08B":   ("Event 08B", "E08B_ records relate to a specific public event."),
    "E09A":   ("Event 09A", "E09A_ records relate to a specific public event."),
    "E01F":   ("Event 01F", "E01F_ records relate to a specific public event."),
    "BonusPerKill":("Bonus-Per-Kill Mechanics", "BonusPerKill_ records tune per-kill bonus reward scalars."),
    "Recipe": ("Crafting Recipes", "Recipe_ records define item crafting requirements."),
    "Legendary":("Legendary System", "Legendary_ records control legendary item drop tables and effect assignments."),
}

# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

import struct
import argparse

def edid_label(edid, form_id):
    if edid:
        return f"`{edid}` ({form_id})"
    return f"({form_id})"

def get_feature_prefix(edid):
    if not edid:
        return None
    for prefix in FEATURE_MAP:
        if edid.startswith(prefix + "_") or edid == prefix:
            return prefix
    return None

def format_scalar(v):
    """Format a scalar value for a table cell (no newlines, capped length)."""
    if v is None:
        return "*(null)*"
    if isinstance(v, bool):
        return f"`{str(v).lower()}`"
    if isinstance(v, (int, float)):
        return f"`{v}`"
    if isinstance(v, str):
        s = v[:100] + ("…" if len(v) > 100 else "")
        return f"`{s}`"
    if isinstance(v, dict):
        if v.get("_raw"):
            return f"`[raw hex]`"
        flags = v.get("flags")
        if isinstance(flags, list):
            return f"`{', '.join(flags) or '(none)'}`"
        name = v.get("name") or v.get("Name")
        if name:
            return f"`{name}`"
        return f"`{str(v)[:60]}…`"
    return f"`{repr(v)[:60]}`"

def is_vmad_hex_change(path, fv, tv):
    """True if this path is the VMAD hex blob."""
    return ("Virtual Machine Adapter" in path and "hex" in path
            and isinstance(fv, str) and isinstance(tv, str) and len(fv) > 40)

def is_raw_change(path, fv, tv):
    """True if this is a _-prefixed raw-hex-only change."""
    parts = path.split(" / ")
    if not parts[-1].startswith("_"):
        return False
    def is_raw_list(v):
        return isinstance(v, list) and v and isinstance(v[0], dict) and v[0].get("_raw")
    return is_raw_list(fv) and is_raw_list(tv)

# --------------------------------------------------------------------------
# Semantic array diffing
# --------------------------------------------------------------------------

def diff_objectives(from_list, to_list):
    """Diff two Objectives arrays by Objective Index, surfacing Display Text changes."""
    def index_by(items):
        result = {}
        for item in items:
            obj = item.get("Objective", {})
            idx = obj.get("Objective Index")
            if idx is not None:
                result[idx] = obj
        return result

    old = index_by(from_list)
    new = index_by(to_list)
    lines = []
    for idx in sorted(set(old) | set(new)):
        o, n = old.get(idx), new.get(idx)
        if o is None and n is not None:
            text = n.get("Display Text", "")
            lines.append(f"  - Added objective `[{idx}]`: *\"{text}\"*")
        elif n is None and o is not None:
            text = o.get("Display Text", "")
            lines.append(f"  - Removed objective `[{idx}]`: *\"{text}\"*")
        elif o is not None and n is not None:
            ot, nt = o.get("Display Text", ""), n.get("Display Text", "")
            if ot != nt:
                lines.append(f"  - Objective `[{idx}]` renamed: *\"{ot}\"* → *\"{nt}\"*")
    return lines

def diff_stages(from_list, to_list):
    """Diff two Stages arrays by Stage Index, surfacing added stages and new log entry Notes."""
    def index_by(items):
        result = {}
        for item in items:
            stage = item.get("Stage", {})
            idx = (stage.get("INDX") or {}).get("Stage Index")
            if idx is not None:
                result[idx] = stage
        return result

    def log_notes(stage):
        return [e.get("Log Entry", {}).get("Note", "")
                for e in stage.get("Log Entries", [])
                if e.get("Log Entry", {}).get("Note")]

    old = index_by(from_list)
    new = index_by(to_list)
    lines = []

    for idx in sorted(set(new) - set(old)):
        notes = log_notes(new[idx])
        note_str = ", ".join(f'*"{n}"*' for n in notes) if notes else "no log entries"
        lines.append(f"  - Added Stage `{idx}`: {note_str}")

    for idx in sorted(set(old) - set(new)):
        lines.append(f"  - Removed Stage `{idx}`")

    for idx in sorted(set(old) & set(new)):
        old_notes, new_notes = log_notes(old[idx]), log_notes(new[idx])
        if old_notes != new_notes:
            added   = [n for n in new_notes if n not in old_notes]
            removed = [n for n in old_notes if n not in new_notes]
            if added:
                lines.append(f"  - Stage `{idx}` new log entries: {', '.join(f'*\"{n}\"*' for n in added)}")
            if removed:
                lines.append(f"  - Stage `{idx}` removed log entries: {', '.join(f'*\"{n}\"*' for n in removed)}")

    return lines

def smart_array_diff(from_list, to_list):
    """
    Return human-readable bullet lines for an array change, or None if no useful diff.
    Dispatches to semantic diffing for known list types; falls back to count delta.
    """
    if not isinstance(from_list, list) or not isinstance(to_list, list):
        return None

    sample = next((x for x in (from_list + to_list) if isinstance(x, dict)), None)
    if sample is None:
        delta = len(to_list) - len(from_list)
        return [f"  - Count: {len(from_list)} → {len(to_list)} items"] if delta else None

    if "Objective" in sample:
        return diff_objectives(from_list, to_list) or None

    if "Stage" in sample:
        return diff_stages(from_list, to_list) or None

    if "Leveled List Entry" in sample:
        delta = len(to_list) - len(from_list)
        if delta == 0:
            return ["  - Leveled list entries reordered or changed"]
        sign = "+" if delta > 0 else ""
        return [f"  - {sign}{delta} entries ({len(from_list)} → {len(to_list)} total)"]

    if "Effect" in sample:
        delta = len(to_list) - len(from_list)
        if delta > 0:
            return [f"  - +{delta} effect(s) added ({len(from_list)} → {len(to_list)} total)"]
        if delta < 0:
            return [f"  - {delta} effect(s) removed ({len(from_list)} → {len(to_list)} total)"]
        return ["  - Effect entries changed (same count)"]

    if "Combination" in sample or "mod" in str(sample).lower():
        delta = len(to_list) - len(from_list)
        if delta:
            sign = "+" if delta > 0 else ""
            return [f"  - {sign}{delta} combinations ({len(from_list)} → {len(to_list)} total)"]
        return ["  - Combinations reordered or changed"]

    if "Target" in sample:
        delta = len(to_list) - len(from_list)
        if delta:
            return [f"  - {'+' if delta > 0 else ''}{delta} targets ({len(from_list)} → {len(to_list)})"]
        return None

    # Generic fallback
    delta = len(to_list) - len(from_list)
    if delta:
        return [f"  - Count: {len(from_list)} → {len(to_list)}"]
    return None

# --------------------------------------------------------------------------
# VMAD hex decoding
# --------------------------------------------------------------------------

def decode_vmad_props(hex_str):
    """
    Best-effort scan of a VMAD hex blob for property names and their scalar values.
    VMAD property types: 1=Object, 2=String, 3=Int32, 4=Float, 5=Bool.
    Returns dict of {prop_name: value} for Int32/Float/Bool properties found.
    """
    try:
        data = bytes.fromhex(hex_str)
    except ValueError:
        return {}

    result = {}
    i = 0
    while i < len(data) - 6:
        length = struct.unpack_from('<H', data, i)[0]
        if 2 <= length <= 80 and i + 2 + length + 3 <= len(data):
            try:
                name = data[i + 2:i + 2 + length].decode('ascii')
            except UnicodeDecodeError:
                i += 1
                continue
            # Name must look like an identifier
            if name and (name[0].isalpha() or name[0] == '_') and all(
                    c.isalnum() or c in '_:.' for c in name):
                after = i + 2 + length
                prop_type = data[after]
                # status byte follows, then value
                value_start = after + 2
                if prop_type == 3 and value_start + 4 <= len(data):   # Int32
                    result[name] = struct.unpack_from('<i', data, value_start)[0]
                elif prop_type == 4 and value_start + 4 <= len(data): # Float
                    result[name] = round(struct.unpack_from('<f', data, value_start)[0], 4)
                elif prop_type == 5 and value_start + 1 <= len(data): # Bool
                    result[name] = bool(data[value_start])
                elif prop_type in (1, 2):                              # Object/String — record name only
                    result[name] = "(object/string)"
        i += 1
    return result

def diff_vmad(old_hex, new_hex):
    """Return bullet lines summarising VMAD property additions/removals/changes."""
    old = decode_vmad_props(old_hex)
    new = decode_vmad_props(new_hex)
    lines = []
    all_keys = sorted(set(old) | set(new),
                      key=lambda k: (k not in new, k not in old, k))
    for k in all_keys:
        ov, nv = old.get(k), new.get(k)
        if ov is None:
            val_str = f" = `{nv}`" if not isinstance(nv, str) else ""
            lines.append(f"  - `{k}`{val_str} *(added)*")
        elif nv is None:
            lines.append(f"  - `{k}` *(removed)*")
        elif ov != nv:
            lines.append(f"  - `{k}`: `{ov}` → `{nv}`")
    return lines

# --------------------------------------------------------------------------
# Per-record change rendering
# --------------------------------------------------------------------------

def _walk_fc(fc, path, scalar_rows, array_sections, vmad_sections, raw_fields):
    """
    Recursively walk field_changes, routing each leaf to the right bucket.
    """
    for key, val in fc.items():
        cur_path = f"{path} / {key}" if path else key

        if not isinstance(val, dict):
            continue

        if "from" in val and "to" in val:
            fv, tv = val["from"], val["to"]

            if is_vmad_hex_change(cur_path, fv, tv):
                vmad_sections.append((key, fv, tv))
            elif is_raw_change(cur_path, fv, tv):
                raw_fields.append(cur_path)
            elif isinstance(fv, list) or isinstance(tv, list):
                diff = smart_array_diff(
                    fv if isinstance(fv, list) else [],
                    tv if isinstance(tv, list) else [],
                )
                if diff:
                    array_sections.append((key, diff))
                else:
                    flen = len(fv) if isinstance(fv, list) else 0
                    tlen = len(tv) if isinstance(tv, list) else 0
                    if flen != tlen:
                        scalar_rows.append((cur_path,
                                            f"`{flen} items`",
                                            f"`{tlen} items`"))
            else:
                fs, ts = format_scalar(fv), format_scalar(tv)
                if fs != ts:
                    scalar_rows.append((cur_path, fs, ts))
        else:
            # Nested container — recurse
            _walk_fc(val, cur_path, scalar_rows, array_sections, vmad_sections, raw_fields)


def render_changed_record(stub, field_changes, compact=False):
    """
    Return a list of markdown lines for one changed record.
    compact=True: synthesis only (for Feature Analysis overview).
    compact=False: synthesis + scalar table + array sections (for Detailed section).
    """
    edid    = stub.get("editor_id") or "*(no edid)*"
    form_id = stub["form_id"]

    scalar_rows    = []   # (path, from_str, to_str)
    array_sections = []   # (field_name, [diff_lines])
    vmad_sections  = []   # (field_name, old_hex, new_hex)
    raw_fields     = []   # path strings

    _walk_fc(field_changes, "", scalar_rows, array_sections, vmad_sections, raw_fields)

    out = []
    out.append(f"**`{edid}`** `{form_id}`")
    out.append("")

    # --- Semantic sections (always shown) ---

    # Objectives
    obj_fc = field_changes.get("Objectives")
    if isinstance(obj_fc, dict) and "from" in obj_fc and "to" in obj_fc:
        lines = diff_objectives(obj_fc["from"], obj_fc["to"])
        if lines:
            out.append("**Objectives:**")
            out.extend(lines)
            out.append("")

    # Stages
    stage_fc = field_changes.get("Stages")
    if isinstance(stage_fc, dict) and "from" in stage_fc and "to" in stage_fc:
        lines = diff_stages(stage_fc["from"], stage_fc["to"])
        if lines:
            out.append("**Stages:**")
            out.extend(lines)
            out.append("")

    # VMAD
    for _, old_hex, new_hex in vmad_sections:
        lines = diff_vmad(old_hex, new_hex)
        if lines:
            out.append("**Script Properties (VMAD):**")
            out.extend(lines)
            out.append("")

    if compact:
        # In Feature Analysis: skip scalar table; only show synthesis above
        # If nothing was synthesised, fall back to showing count of changed fields
        if not any(out[2:]):  # [0]=bold edid, [1]=blank
            total = len(scalar_rows) + len(array_sections) + len(vmad_sections)
            out.append(f"  *{total} field(s) changed — see Detailed section below.*")
            out.append("")
        return out

    # --- Full detail (Detailed Changes section) ---

    # Array sections (non-Objectives/Stages — handled above)
    ALREADY_HANDLED = {"Objectives", "Stages"}
    for fname, diff_lines in array_sections:
        if fname in ALREADY_HANDLED:
            continue
        out.append(f"**{fname}:**")
        out.extend(diff_lines)
        out.append("")

    # Scalar table
    scalar_rows = [(p, f, t) for p, f, t in scalar_rows
                   if p not in ("Objectives", "Stages")]
    if scalar_rows:
        out.append("| Field | From | To |")
        out.append("|-------|------|----|")
        for path, fs, ts in scalar_rows[:30]:
            out.append(f"| {path} | {fs.replace('|', '\\|')} | {ts.replace('|', '\\|')} |")
        if len(scalar_rows) > 30:
            out.append(f"| *…* | *{len(scalar_rows) - 30} more fields* | |")
        out.append("")

    # Raw notice
    if raw_fields:
        out.append(f"*Raw/unmapped subrecords changed: "
                   f"{', '.join(raw_fields[:5])}{'…' if len(raw_fields) > 5 else ''}*")
        out.append("")

    if len(out) <= 2:   # nothing rendered beyond the header
        out.append("*(no decoded field changes)*")
        out.append("")

    return out

# --------------------------------------------------------------------------
# Main generator
# --------------------------------------------------------------------------

def generate_markdown(diff_data, timing=None):
    out = []

    added   = diff_data["added"]
    removed = diff_data["removed"]
    changed = diff_data["changed"]

    # ---- Build lookup structures ----------------------------------------
    # By record type
    added_by_type   = defaultdict(list)
    removed_by_type = defaultdict(list)
    changed_by_type = defaultdict(list)

    for r in added:
        added_by_type[r["record_type"]].append(r)
    for r in removed:
        removed_by_type[r["record_type"]].append(r)
    for c in changed:
        changed_by_type[c["stub"]["record_type"]].append(c)

    all_types = sorted(set(
        list(added_by_type.keys()) +
        list(removed_by_type.keys()) +
        list(changed_by_type.keys())
    ))
    meaningful_types = [t for t in all_types if t not in SKIP_DETAIL_TYPES]

    # Feature groupings
    feature_records = defaultdict(lambda: {"added": [], "removed": [], "changed": []})
    for r in added:
        p = get_feature_prefix(r.get("editor_id"))
        if p:
            feature_records[p]["added"].append(r)
    for r in removed:
        p = get_feature_prefix(r.get("editor_id"))
        if p:
            feature_records[p]["removed"].append(r)
    for c in changed:
        p = get_feature_prefix(c["stub"].get("editor_id"))
        if p:
            feature_records[p]["changed"].append(c)

    # ---- Header -----------------------------------------------------------
    out.append(f"# Fallout 76 ESM Patch Notes — {PATCH_DATE}")
    out.append("")
    out.append(f"> **Comparing:** `{OLD_LABEL}` → `{NEW_LABEL}`  ")
    out.append(f"> Generated from binary ESM diff (fo76-esm-parser).")
    out.append("")

    if timing:
        out.append("## Parse & Diff Timing")
        out.append("")
        out.append("| Step | Time |")
        out.append("|------|------|")
        out.append(f"| Open + index `{OLD_LABEL}` | {timing.get('open_a', '?'):.2f}s |")
        out.append(f"| Open + index `{NEW_LABEL}` | {timing.get('open_b', '?'):.2f}s |")
        out.append(f"| Diff computation ({len(added)} added, {len(removed)} removed, {len(changed)} changed) | {timing.get('diff', '?'):.2f}s |")
        out.append("| Interpretation + markdown generation | {INTERPRET_TIME} |")
        total = sum(timing.values())
        out.append(f"| **Total (parse + diff + interpret)** | {{TOTAL_TIME}} |")
        out.append("")

    # ---- Notable Changes (hand-curated from key findings) -----------------
    out.append("## ⚡ Highlights & Notable Changes")
    out.append("")
    out.append("Key changes identified from the diff — see the detailed sections below for full field values.")
    out.append("")

    highlights = [
        ("Balance — Weapons", [
            "**Railway Rifle** `0x000FE268`: Magazine capacity buffed **10 → 14**",
            "**Stormcutter** `0x0072DF0B`: 2 new Keywords added (enabling new OMOD slots); new `mod_melee_Stormcutter_Standard` OMOD added",
            "New **DeathTambo** melee mods: `mod_melee_DeathTambo_SpikesSmall` and `mod_melee_DeathTambo_Blades`",
            "New **RailwayRifle** mods: `mod_RailwayRifle_Receiver_Automatic_AntiScorchBeast` and `mod_RailwayRifle_Receiver_Splitter`",
            "6 new weapon **enchantments** added: GrandFinale, Kingfisher, WhackerSmacker, Longshot, PiratePunch, DeathTambo bleed",
        ]),
        ("Balance — Hostile Takeover (Infestations)", [
            "**Quest flow reworked (two-phase encounter)**: players must first *Clear the Infestation Mobs* before the boss spawns in — `MobKillPercentToSpawnBoss = 0.75` (75% kill threshold decoded from VMAD). New objective `[500] \"Defeat the Infestation Boss\"` appears only after the mob phase. New Stage 1 and log entry *\"Quest Mobs Killed\"* track the transition.",
            "**Ammo reward counts doubled+**: Boss `15 → 30`, Mob `5 → 10`, Support `1 → 5`",
            "**Fortify Bash (Boss)** doubled: `500 → 1000`",
            "**Fortify Health** feature enabled: `0 → 1` (HTO_LCP_HostileTakeOver_FortifyHealth_Toggle)",
            "**3-star Legendary drop 'no-drop' chance** reduced: Support `90% → 85%`, Mob `80% → 75%` (more legendaries)",
            "4 new **Fortify Damage** globals added (Boss/Mob/Support/Toggle)",
            "New **HTO corpse-highlight** effect shader and magic effect for Boss enemies",
            "New HTO explosion VFX (`HTO_crExplosionTeleportInVFX`) and SFX for enemy teleport-in",
        ]),
        ("Balance — Perks", [
            "**CrowdControl Perk** redesigned: `+1 PER per kill on streak` → `+5% Limb Damage per kill on streak`",
            "**LoveTap Perk** redesigned: `+1 CHA per kill on streak` → `+10% Bash Damage per kill on streak`",
            "New **SoleSurvivorPerk** + `AbSoleSurvivor` spell added (new lone-wolf perk)",
            "New **EldersMark** perk added",
            "New **custom_TickettoRevenge** perk added",
            "New **mod_custom_V63-BERTHA_Perk** added (associated with V63 weapon mod?)",
            "**Lone Wanderer** perk effects updated",
        ]),
        ("C.A.M.P. Pets", [
            "All CAMP pet NPCs (cats, dogs) received Keyword Count `10 → 11` — new keyword/feature added system-wide",
            "**WorldPets_DisabledBuffs** spell renamed to `Pet Buffs` — pet ability buffs may now be active in world",
            "Multiple pet NPC VMAD (script) changes indicate new workshop-linking mechanic",
        ]),
        ("Fishing System", [
            "New fish added: **Gold Axolotl** (`FISH` record + leveled lists: `Fishing_LLS_FishCollection_GoldAxolotls`, `Fishing_LLS_AxolotlFallback`)",
            "New global: `Fishing_Odds_GoldAxolotl_CatchRate` — catch rate is tunable server-side",
            "New player title prefix: `Fishing_PlayerTitles_Prefix_Gillded`",
        ]),
        ("Slasher Event (SDOW — 'Pint-Sized Slasher')", [
            "Entire event rebranded: **'Slasher' → 'Pint-Sized Slasher'** across all armor, plans, decorations, and props",
            "Slasher hat colors added: Blue, Red, Green, Orange Pint-Sized Slasher Hat variants",
            "New quest: `SDOW_Holotape03_RadioBroadcasts` (3rd holotape broadcast quest)",
            "New NPC AI package: `SDOW_Travel_Slasher_01_PatrolWeaponsOutNoRun`",
            "Slasher Grave Keeper NPC renamed to **Phantom Gravekeeper**; Ghost enemy type overhauled (race changed)",
            "New leveled reward list: `SDOW_LLS_Slasher_Rewards_LegendaryRewards`",
            "New actor value flags for clue/map tracking (`SDOW_MQ02_Graves_HasReadMap`, etc.)",
        ]),
        ("Atomic Shop (ATX) & Scoreboard (SCORE)", [
            "**~89 ATX records changed** — cosmetic shop inventory refresh (apparel, skins, camp items)",
            "New CAMP wall decoration: `ATX_WallDecor_Ceiling_SausageRack`",
            "New Floratron robot torso OMOD: `ATX_Bot_Floratron_Torso`",
            "**Lucky Dice emote** upgraded: animations count `1 → 6`",
            "**MadeInTheShade emote** renamed to **'Greasin' Up'**",
            "**Good Luck emote** renamed to **'Shrug'** (with new animation and icon)",
            "**Overgrown** player title added (was accidentally named 'Green')",
            "**~100 SCORE S26 records changed** — likely season 26 scoreboard refresh",
            "6 new S26 player icons: Bat Tamer, Bei, Castle, Night Person, Psychopath, T-51b",
        ]),
        ("Loot & Crafting", [
            "31 **Leveled Item Lists** updated — melee weapon mod pools for Toxic Valley, Savage Divide, Ashheap and others grew by 2 entries each (new DeathTambo/Stormcutter mods entering loot pool)",
            "**AntiScorchBeast ranged mod list** grew by 1 entry (new RailwayRifle anti-SB receiver)",
            "**Samuel vendor list** grew from 38→41 items",
            "**Workshop poster wall-decor loot list** grew from 105→108 entries",
            "10 new **COBJ recipes** added; 9 existing recipes modified",
        ]),
    ]

    for category, items in highlights:
        out.append(f"### {category}")
        out.append("")
        for item in items:
            out.append(f"- {item}")
        out.append("")

    # ---- Summary table ----------------------------------------------------
    out.append("## Summary")
    out.append("")
    out.append(f"| Metric | Count |")
    out.append(f"|--------|-------|")
    out.append(f"| ✅ Added records | **{len(added)}** |")
    out.append(f"| ❌ Removed records | **{len(removed)}** |")
    out.append(f"| 🔄 Changed records | **{len(changed)}** |")
    out.append(f"| **Total touched** | **{len(added)+len(removed)+len(changed)}** |")
    out.append("")

    # Per-type summary table
    out.append("### Changes by Record Type")
    out.append("")
    out.append("| Type | Description | ✅ Added | ❌ Removed | 🔄 Changed |")
    out.append("|------|-------------|----------|------------|------------|")
    for t in meaningful_types:
        a = len(added_by_type.get(t, []))
        r = len(removed_by_type.get(t, []))
        c = len(changed_by_type.get(t, []))
        desc = TYPE_DESC.get(t, "")
        out.append(f"| `{t}` | {desc} | {a if a else '—'} | {r if r else '—'} | {c if c else '—'} |")
    out.append("")

    # ---- Feature Analysis -------------------------------------------------
    out.append("---")
    out.append("")
    out.append("## Feature Analysis")
    out.append("")
    out.append("Changes are grouped below by their EditorID prefix, which typically")
    out.append("identifies which game system or seasonal event they belong to.")
    out.append("")

    sorted_features = sorted(feature_records.keys(),
                             key=lambda p: -(len(feature_records[p]["added"]) +
                                             len(feature_records[p]["removed"]) +
                                             len(feature_records[p]["changed"])))

    for prefix in sorted_features:
        frec = feature_records[prefix]
        name, desc = FEATURE_MAP.get(prefix, (f"{prefix}_ Records", ""))
        total = len(frec["added"]) + len(frec["removed"]) + len(frec["changed"])
        if total == 0:
            continue

        out.append(f"### {name}")
        out.append("")
        if desc:
            out.append(f"*{desc}*")
            out.append("")
        out.append(f"| Category | Count |")
        out.append(f"|----------|-------|")
        if frec["added"]:
            out.append(f"| Added | {len(frec['added'])} |")
        if frec["removed"]:
            out.append(f"| Removed | {len(frec['removed'])} |")
        if frec["changed"]:
            out.append(f"| Changed | {len(frec['changed'])} |")
        out.append("")

        # List added records in this feature
        if frec["added"]:
            by_type = defaultdict(list)
            for r in frec["added"]:
                by_type[r["record_type"]].append(r)
            out.append("**Newly added records:**")
            out.append("")
            for t in sorted(by_type):
                out.append(f"- **{t}** ({TYPE_DESC.get(t, '')}):")
                for r in by_type[t]:
                    lbl = edid_label(r.get("editor_id"), r["form_id"])
                    out.append(f"  - {lbl}")
            out.append("")

        # List removed records
        if frec["removed"]:
            out.append("**Removed records:**")
            out.append("")
            for r in frec["removed"]:
                lbl = edid_label(r.get("editor_id"), r["form_id"])
                out.append(f"- `{r['record_type']}` {lbl}")
            out.append("")

        # Summarize changed records
        if frec["changed"]:
            out.append("**Changed records:**")
            out.append("")
            for c in frec["changed"]:
                stub = c["stub"]
                rtype = stub["record_type"]
                field_changes = c.get("field_changes", {})
                lines = render_changed_record(stub, field_changes, compact=True)
                # Prefix record type
                lines[0] = f"**[{rtype}]** {lines[0]}"
                out.extend(lines)
            out.append("")

    # ---- Detailed Changes by Record Type ----------------------------------
    out.append("---")
    out.append("")
    out.append("## Detailed Changes by Record Type")
    out.append("")
    out.append("> CELL and WRLD records are excluded (world geometry, not decoded by this parser).")
    out.append("> REFR and ACHR records show summary only (positional/raw hex data).")
    out.append("")

    for rtype in meaningful_types:
        a_list = added_by_type.get(rtype, [])
        r_list = removed_by_type.get(rtype, [])
        c_list = changed_by_type.get(rtype, [])

        if not a_list and not r_list and not c_list:
            continue

        desc = TYPE_DESC.get(rtype, "")
        header_desc = f" — {desc}" if desc else ""
        out.append(f"### `{rtype}`{header_desc}")
        out.append("")

        # Raw-heavy types: just show summary
        if rtype in RAW_HEAVY_TYPES:
            out.append(f"*This record type contains primarily raw/unmapped positional or binary data.*")
            out.append("")
            if a_list:
                out.append(f"**{len(a_list)} added** references/instances:")
                for r in a_list[:20]:
                    lbl = edid_label(r.get("editor_id"), r["form_id"])
                    out.append(f"- {lbl}")
                if len(a_list) > 20:
                    out.append(f"- *…and {len(a_list)-20} more*")
                out.append("")
            if r_list:
                out.append(f"**{len(r_list)} removed** references/instances:")
                for r in r_list[:20]:
                    lbl = edid_label(r.get("editor_id"), r["form_id"])
                    out.append(f"- {lbl}")
                out.append("")
            if c_list:
                out.append(f"**{len(c_list)} changed** references — positional/raw-hex data shifted (world edits, not shown in detail).")
                out.append("")
            out.append("")
            continue

        # Added records
        if a_list:
            out.append(f"#### Added ({len(a_list)})")
            out.append("")
            out.append("| FormID | EditorID |")
            out.append("|--------|----------|")
            for r in a_list:
                edid = r.get("editor_id") or "*(no edid)*"
                out.append(f"| `{r['form_id']}` | `{edid}` |")
            out.append("")

        # Removed records
        if r_list:
            out.append(f"#### Removed ({len(r_list)})")
            out.append("")
            out.append("| FormID | EditorID |")
            out.append("|--------|----------|")
            for r in r_list:
                edid = r.get("editor_id") or "*(no edid)*"
                out.append(f"| `{r['form_id']}` | `{edid}` |")
            out.append("")

        # Changed records — show field-level diff
        if c_list:
            out.append(f"#### Changed ({len(c_list)})")
            out.append("")

            for c in c_list:
                out.extend(render_changed_record(c["stub"], c.get("field_changes", {}),
                                                 compact=False))

        out.append("")

    # ---- Footer -----------------------------------------------------------
    out.append("---")
    out.append("")
    out.append(f"*Generated by [fo76-esm-parser](https://github.com/) from binary ESM diff.*  ")
    out.append(f"*{OLD_LABEL} vs {NEW_LABEL}*  ")
    out.append(f"*Total: {len(added)} added · {len(removed)} removed · {len(changed)} changed*")
    out.append("")

    return "\n".join(out)

# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------

if __name__ == "__main__":
    import time as _time

    ap = argparse.ArgumentParser(description="Generate patch notes markdown from ESM diff JSON.")
    ap.add_argument("diff_json", nargs="?", default="/tmp/fo76_diff.json")
    ap.add_argument("--open-a", type=float, default=None, metavar="SECS",
                    help="Time to open+index ESM A (from fo76 diff timing)")
    ap.add_argument("--open-b", type=float, default=None, metavar="SECS",
                    help="Time to open+index ESM B (from fo76 diff timing)")
    ap.add_argument("--diff", type=float, default=None, metavar="SECS",
                    help="Time to compute diff (from fo76 diff timing)")
    args = ap.parse_args()

    print(f"Loading {args.diff_json}…", file=sys.stderr)
    with open(args.diff_json) as f:
        data = json.load(f)
    print(f"Loaded: {len(data['added'])} added, {len(data['removed'])} removed, {len(data['changed'])} changed",
          file=sys.stderr)

    timing = None
    if args.open_a is not None or args.open_b is not None or args.diff is not None:
        timing = {
            "open_a": args.open_a or 0.0,
            "open_b": args.open_b or 0.0,
            "diff":   args.diff   or 0.0,
        }

    t_start = _time.time()
    md = generate_markdown(data, timing=timing)
    t_interpret = _time.time() - t_start

    if timing:
        timing["interpret"] = t_interpret
        total = sum(timing.values())
        md = md.replace("{INTERPRET_TIME}", f"{t_interpret:.2f}s")
        md = md.replace("{TOTAL_TIME}", f"{total:.2f}s")

    print(f"Generated {len(md):,} chars of markdown in {t_interpret:.2f}s", file=sys.stderr)
    print(md)
