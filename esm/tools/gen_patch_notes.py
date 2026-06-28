#!/usr/bin/env python3
"""
Generate a comprehensive patch-notes markdown from the ESM diff JSON.

Usage:
    python3 tools/gen_patch_notes.py <diff.json> [options]

Options:
    --old-label LABEL     Display label for the old ESM (default: derived from diff JSON)
    --new-label LABEL     Display label for the new ESM
    --patch-date DATE     Patch date string, e.g. 2026-06-26 (default: derived from new-label)
    --highlights-file F   Inject verbatim MD file as the Highlights section (skips auto-highlights)
    --open-a SECS         Timing: seconds to open+index ESM A
    --open-b SECS         Timing: seconds to open+index ESM B
    --diff SECS           Timing: seconds to compute diff
"""

import json
import sys
import re
import os
import argparse
import struct
import time as _time
from collections import defaultdict

# --------------------------------------------------------------------------
# Record-type descriptions
# --------------------------------------------------------------------------

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

# Feature prefix → (human-readable name, description)
FEATURE_MAP = {
    "HTO":      ("Hostile Takeover (Infestations)", "HTO_ records govern the Hostile Takeover / Infestation seasonal event."),
    "SDOW":     ("Slasher / SDOW Event", "SDOW_ records control the Slasher limited-time event."),
    "SCORE":    ("Scoreboard / S.C.O.R.E.", "SCORE_ records define scoreboard season rewards, challenges, and progression items."),
    "ATX":      ("Atomic Shop", "ATX_ records cover Atomic Shop cosmetic items."),
    "Fishing":  ("Fishing System", "Fishing_ records configure fish spawn rates, catch odds, and rewards."),
    "XPD":      ("Expeditions", "XPD_ records relate to Expeditions missions."),
    "LCP":      ("Living Colonial Park", "LCP_ records are linked to the Living Colonial Park event space."),
    "RD01":     ("Raid 01", "RD01_ records relate to an upcoming raid encounter."),
    "LLS":      ("Leveled List System", "LLS_ records control loot drop tables."),
    "Workshop": ("Workshop / C.A.M.P.", "Workshop_ records affect base-building placeables."),
    "CAMPPets": ("C.A.M.P. Pets", "CAMPPets_ records define pet companions available at C.A.M.P."),
    "WorldPets":("World Pets", "WorldPets_ records define pets that exist in the open world."),
    "E08B":     ("Event 08B", "E08B_ records relate to a specific public event."),
    "E09A":     ("Event 09A", "E09A_ records relate to a specific public event."),
    "E01F":     ("Event 01F", "E01F_ records relate to a specific public event."),
    "BonusPerKill": ("Bonus-Per-Kill Mechanics", "BonusPerKill_ records tune per-kill bonus reward scalars."),
    "Recipe":   ("Crafting Recipes", "Recipe_ records define item crafting requirements."),
    "Legendary":("Legendary System", "Legendary_ records control legendary item drop tables."),
}

# Record types to skip entirely (world-placement, positional, not decoded).
SKIP_DETAIL_TYPES = {"CELL", "WRLD"}

# Cut/deprecation EDID marker prefixes (all-caps or all-lowercase, typically _-delimited).
CUT_MARKERS = ["ZZZ", "CUT", "POST", "DEPRECATED", "DELETE"]

# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

def derive_labels_from_filenames(old_label, new_label):
    """Derive PATCH_DATE from new_label filename stem (e.g. SeventySix_20260626.esm → 2026-06-26)."""
    if not new_label:
        return old_label, new_label, "Unknown Date"
    stem = os.path.splitext(os.path.basename(new_label))[0]
    # Extract 8-digit date token, e.g. 20260626 → "2026-06-26"
    m = re.search(r'(\d{4})(\d{2})(\d{2})', stem)
    if m:
        patch_date = f"{m.group(1)}-{m.group(2)}-{m.group(3)}"
    else:
        patch_date = stem
    return old_label, new_label, patch_date


def edid_label(edid, form_id, name=None):
    """Human-readable label: name (if present), edid, form_id."""
    if name:
        if edid:
            return f"**{name}** `{edid}` `{form_id}`"
        return f"**{name}** `{form_id}`"
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


def format_scalar(v, ref_names=None):
    """Format a scalar value for a table cell (no newlines, capped length).
    If v looks like a FormID hex string and ref_names provides a resolution,
    annotate it with `(EditorID "Name")`.
    """
    if v is None:
        return "*(null)*"
    if isinstance(v, bool):
        return f"`{str(v).lower()}`"
    if isinstance(v, (int, float)):
        return f"`{v}`"
    if isinstance(v, str):
        # Unresolved LString ID — show the id rather than a raw dict
        if len(v) == 10 and v.startswith("0x") and all(c in "0123456789abcdefABCDEF" for c in v[2:]):
            if ref_names and v in ref_names:
                rn = ref_names[v]
                rtype = rn.get("record_type", "?")
                edid = rn.get("editor_id", "")
                name = rn.get("name", "")
                if name and edid:
                    return f"`{v}` ({rtype}: `{edid}` *\"{name}\"*)"
                if edid:
                    return f"`{v}` ({rtype}: `{edid}`)"
            return f"`{v}`"
        s = v[:100] + ("…" if len(v) > 100 else "")
        return f"`{s}`"
    if isinstance(v, dict):
        # Unresolved LString ID from ESM decoder
        if v.get("_unresolved") and "lstring_id" in v:
            lid = v["lstring_id"]
            return f"`[lstring {lid}]` *(unresolved)*"
        if v.get("_raw"):
            return "`[raw hex]`"
        flags = v.get("flags")
        if isinstance(flags, list):
            return f"`{', '.join(flags) or '(none)'}`"
        name = v.get("name") or v.get("Name")
        if name:
            return f"`{name}`"
        return f"`{str(v)[:60]}…`"
    return f"`{repr(v)[:60]}`"


def is_vmad_hex_change(path, fv, tv):
    return ("Virtual Machine Adapter" in path and "hex" in path
            and isinstance(fv, str) and isinstance(tv, str) and len(fv) > 40)


def is_raw_change(path, fv, tv):
    parts = path.split(" / ")
    if not parts[-1].startswith("_"):
        return False
    def is_raw_list(v):
        return isinstance(v, list) and v and isinstance(v[0], dict) and v[0].get("_raw")
    return is_raw_list(fv) and is_raw_list(tv)


# --------------------------------------------------------------------------
# Cut / deprecation detection
# --------------------------------------------------------------------------

def _marker_token(edid, markers):
    """
    Return (marker, confidence) if edid has a deprecation prefix, else None.
    Confidence: 'high' = MARKER_ or marker_ prefix exactly delimited;
                'medium' = bare prefix before CamelCase or digit;
                'low' = suffix or mid-word (only for non-POST markers).
    POST is only ever high/medium to avoid false positives (e.g. "Poster").
    """
    if not edid:
        return None
    for m in markers:
        # High: exactly MARKER_ or marker_ prefix
        if edid.startswith(m + "_") or edid.startswith(m.lower() + "_"):
            conf = "medium" if m == "POST" else "high"
            return (m, conf)
        # Medium: bare all-caps prefix before CamelCase or digit
        if edid.startswith(m) and len(edid) > len(m):
            ch = edid[len(m)]
            if ch.isupper() or ch.isdigit():
                conf = "low" if m == "POST" else "medium"
                return (m, conf)
        # Low: suffix (skip POST to avoid false positives)
        if m != "POST":
            if edid.endswith("_" + m) or edid.endswith("_" + m.lower()):
                return (m, "low")
    return None


def classify_cut(edid, prev_edid=None, markers=None):
    """
    Classify a record as cut/deprecated.
    Returns dict with keys 'marker', 'confidence', 'kind':
      kind = 'newly_deprecated'  (prev_edid was clean, edid is now marked)
           | 'still_cut'         (edid was already marked, changed this patch)
           | 'added_cut'         (new record already has a cut marker)
           | None
    Returns None if the record is not cut/deprecated.
    """
    if markers is None:
        markers = CUT_MARKERS
    tok_new = _marker_token(edid, markers) if edid else None
    tok_old = _marker_token(prev_edid, markers) if prev_edid else None
    if not tok_new:
        return None
    m, conf = tok_new
    if prev_edid is not None:
        if tok_old:
            return {"marker": m, "confidence": conf, "kind": "still_cut"}
        else:
            return {"marker": m, "confidence": conf, "kind": "newly_deprecated"}
    return {"marker": m, "confidence": conf, "kind": "added_cut"}


# --------------------------------------------------------------------------
# Numerical-delta scanning (for auto-highlights)
# --------------------------------------------------------------------------

def _collect_numeric_deltas(field_changes, stub, out, path=""):
    """Recursively collect (abs_delta, pct_delta, path, from_val, to_val, stub) tuples."""
    for key, val in field_changes.items():
        cur = f"{path} / {key}" if path else key
        if not isinstance(val, dict):
            continue
        if "from" in val and "to" in val:
            fv, tv = val["from"], val["to"]
            if isinstance(fv, (int, float)) and isinstance(tv, (int, float)):
                if fv != tv:
                    abs_d = abs(tv - fv)
                    pct_d = abs(tv - fv) / abs(fv) * 100 if fv != 0 else float("inf")
                    out.append((abs_d, pct_d, cur, fv, tv, stub))
        else:
            _collect_numeric_deltas(val, stub, out, cur)


def build_auto_highlights(diff_data, cut_info, max_per_section=8):
    """
    Build auto-generated highlights sections from the diff data.
    Returns a list of (category_name, [bullet_strings]) tuples.
    """
    sections = []

    # --- Cut / Deprecated Content ---
    newly_dep = cut_info.get("newly_deprecated", [])
    added_cut = cut_info.get("added_cut", [])
    removed_notable = []  # removed records with cut markers
    for r in diff_data.get("removed", []):
        tok = _marker_token(r.get("editor_id", ""), CUT_MARKERS)
        if tok:
            removed_notable.append(r)

    if newly_dep or added_cut or removed_notable:
        bullets = []
        for item in newly_dep[:max_per_section]:
            stub = item["stub"]
            info = item["cut_info"]
            prev = item["prev_editor_id"]
            edid = stub.get("editor_id", "")
            fid = stub["form_id"]
            rtype = stub["record_type"]
            bullets.append(
                f"**{rtype}** `{fid}`: EditorID renamed `{prev}` → `{edid}` "
                f"({info['marker']}, confidence: {info['confidence']}) — "
                f"*marked deprecated/cut this patch*"
            )
        for item in added_cut[:max_per_section]:
            edid = item.get("editor_id", "")
            fid = item["form_id"]
            rtype = item["record_type"]
            name = item.get("name", "")
            label = f'"{name}" ' if name else ""
            bullets.append(
                f"**{rtype}** `{fid}` {label}`{edid}` — *added as already-cut/placeholder*"
            )
        if removed_notable:
            bullets.append(
                f"{len(removed_notable)} previously-cut record(s) hard-removed: "
                + ", ".join(f"`{r.get('editor_id','?')}`" for r in removed_notable[:5])
                + ("…" if len(removed_notable) > 5 else "")
            )
        if bullets:
            sections.append(("⚰️ Cut / Deprecated Content", bullets))

    # --- Largest Numerical Deltas ---
    all_deltas = []
    for c in diff_data.get("changed", []):
        _collect_numeric_deltas(c.get("field_changes", {}), c["stub"], all_deltas)

    # Sort by abs_delta descending, then pct_delta descending
    all_deltas.sort(key=lambda x: (-x[0], -x[1]))
    # Deduplicate by (stub.form_id, path) to avoid showing same change multiple times
    seen = set()
    top_deltas = []
    for item in all_deltas:
        _abs_d, _pct_d, path, _fv, _tv, stub = item
        key = (stub["form_id"], path)
        if key not in seen:
            seen.add(key)
            top_deltas.append(item)
        if len(top_deltas) >= max_per_section * 2:
            break

    if top_deltas:
        bullets = []
        for _abs_d, pct_d, path, fv, tv, stub in top_deltas[:max_per_section]:
            edid = stub.get("editor_id", "?")
            rtype = stub.get("record_type", "?")
            fid = stub["form_id"]
            name = stub.get("name", "")
            label = f'"{name}" ' if name else ""
            sign = "+" if tv > fv else ""
            if pct_d == float("inf"):
                pct_str = " (was zero)"
            elif pct_d >= 1000:
                pct_str = f" ({sign}{pct_d:.0f}%)"
            else:
                pct_str = f" ({sign}{pct_d:.1f}%)"
            bullets.append(
                f"**{rtype}** `{edid}` {label}`{fid}` — `{path}`: `{fv}` → `{tv}`{pct_str}"
            )
        sections.append(("📊 Largest Numerical Changes", bullets))

    # --- New Content by Feature ---
    added = diff_data.get("added", [])
    removed = diff_data.get("removed", [])
    changed = diff_data.get("changed", [])

    # Group added by record type for a quick "new content" summary
    added_by_type = defaultdict(list)
    for r in added:
        added_by_type[r["record_type"]].append(r)

    if added:
        bullets = []
        for rtype in sorted(added_by_type, key=lambda t: -len(added_by_type[t])):
            cnt = len(added_by_type[rtype])
            desc = TYPE_DESC.get(rtype, rtype)
            examples = added_by_type[rtype][:3]
            eg_str = ", ".join(
                f"`{r.get('editor_id') or r['form_id']}`"
                + (f' *"{r["name"]}"*' if r.get("name") else "")
                for r in examples
            )
            more = f" *+{cnt - 3} more*" if cnt > 3 else ""
            bullets.append(f"**{cnt}×** {rtype} ({desc}): {eg_str}{more}")
            if len(bullets) >= max_per_section:
                break
        sections.append(("✅ New Records Summary", bullets))

    # Per-feature summary
    feature_bullets = []
    feature_seen = defaultdict(lambda: {"a": 0, "r": 0, "c": 0})
    for r in added:
        p = get_feature_prefix(r.get("editor_id"))
        if p:
            feature_seen[p]["a"] += 1
    for r in removed:
        p = get_feature_prefix(r.get("editor_id"))
        if p:
            feature_seen[p]["r"] += 1
    for c in changed:
        p = get_feature_prefix(c["stub"].get("editor_id"))
        if p:
            feature_seen[p]["c"] += 1
    for prefix, counts in sorted(feature_seen.items(), key=lambda kv: -(kv[1]["a"]+kv[1]["r"]+kv[1]["c"])):
        name, _ = FEATURE_MAP.get(prefix, (f"{prefix}_ records", ""))
        parts = []
        if counts["a"]:
            parts.append(f"+{counts['a']} added")
        if counts["r"]:
            parts.append(f"-{counts['r']} removed")
        if counts["c"]:
            parts.append(f"~{counts['c']} changed")
        feature_bullets.append(f"**{name}**: {', '.join(parts)}")
        if len(feature_bullets) >= max_per_section:
            break
    if feature_bullets:
        sections.append(("🗺️ Feature Activity", feature_bullets))

    return sections


# --------------------------------------------------------------------------
# Semantic array diffing
# --------------------------------------------------------------------------

def diff_objectives(from_list, to_list):
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
            added_n   = [n for n in new_notes if n not in old_notes]
            removed_n = [n for n in old_notes if n not in new_notes]
            if added_n:
                lines.append(f"  - Stage `{idx}` new log entries: {', '.join(f'*\"{n}\"*' for n in added_n)}")
            if removed_n:
                lines.append(f"  - Stage `{idx}` removed log entries: {', '.join(f'*\"{n}\"*' for n in removed_n)}")

    return lines


def smart_array_diff(from_list, to_list):
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

    delta = len(to_list) - len(from_list)
    if delta:
        return [f"  - Count: {len(from_list)} → {len(to_list)}"]
    return None


# --------------------------------------------------------------------------
# VMAD hex decoding
# --------------------------------------------------------------------------

def decode_vmad_props(hex_str):
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
            if name and (name[0].isalpha() or name[0] == '_') and all(
                    c.isalnum() or c in '_:.' for c in name):
                after = i + 2 + length
                prop_type = data[after]
                value_start = after + 2
                if prop_type == 3 and value_start + 4 <= len(data):
                    result[name] = struct.unpack_from('<i', data, value_start)[0]
                elif prop_type == 4 and value_start + 4 <= len(data):
                    result[name] = round(struct.unpack_from('<f', data, value_start)[0], 4)
                elif prop_type == 5 and value_start + 1 <= len(data):
                    result[name] = bool(data[value_start])
                elif prop_type in (1, 2):
                    result[name] = "(object/string)"
        i += 1
    return result


def diff_vmad(old_hex, new_hex):
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

def _walk_fc(fc, path, scalar_rows, array_sections, vmad_sections, raw_fields, ref_names=None):
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
                fs = format_scalar(fv, ref_names)
                ts = format_scalar(tv, ref_names)
                if fs != ts:
                    scalar_rows.append((cur_path, fs, ts))
        else:
            _walk_fc(val, cur_path, scalar_rows, array_sections, vmad_sections, raw_fields, ref_names)


def render_changed_record(stub, field_changes, compact=False, ref_names=None, cut_info=None):
    """
    Return a list of markdown lines for one changed record.
    compact=True: synthesis only (for Feature Analysis overview).
    compact=False: synthesis + scalar table + array sections (for Detailed section).
    """
    edid    = stub.get("editor_id") or "*(no edid)*"
    form_id = stub["form_id"]
    name    = stub.get("name")

    scalar_rows    = []
    array_sections = []
    vmad_sections  = []
    raw_fields     = []

    _walk_fc(field_changes, "", scalar_rows, array_sections, vmad_sections, raw_fields, ref_names)

    out = []
    # Header line: show name if present
    if name:
        out.append(f"**`{edid}`** `{form_id}` — *{name}*")
    else:
        out.append(f"**`{edid}`** `{form_id}`")
    out.append("")

    # Cut/deprecation notice
    if cut_info:
        kind = cut_info.get("kind")
        marker = cut_info.get("marker", "?")
        conf = cut_info.get("confidence", "?")
        if kind == "newly_deprecated":
            out.append(f"> ⚰️ **Newly deprecated this patch** (marker: `{marker}`, confidence: {conf})")
            out.append("")
        elif kind == "still_cut":
            out.append(f"> ⚰️ *Still marked deprecated/cut* (marker: `{marker}`)")
            out.append("")

    # Semantic sections (always shown)
    obj_fc = field_changes.get("Objectives")
    if isinstance(obj_fc, dict) and "from" in obj_fc and "to" in obj_fc:
        lines = diff_objectives(obj_fc["from"], obj_fc["to"])
        if lines:
            out.append("**Objectives:**")
            out.extend(lines)
            out.append("")

    stage_fc = field_changes.get("Stages")
    if isinstance(stage_fc, dict) and "from" in stage_fc and "to" in stage_fc:
        lines = diff_stages(stage_fc["from"], stage_fc["to"])
        if lines:
            out.append("**Stages:**")
            out.extend(lines)
            out.append("")

    for _, old_hex, new_hex in vmad_sections:
        lines = diff_vmad(old_hex, new_hex)
        if lines:
            out.append("**Script Properties (VMAD):**")
            out.extend(lines)
            out.append("")

    if compact:
        if not any(out[2:]):
            total = len(scalar_rows) + len(array_sections) + len(vmad_sections)
            out.append(f"  *{total} field(s) changed — see Detailed section below.*")
            out.append("")
        return out

    # Full detail
    ALREADY_HANDLED = {"Objectives", "Stages"}
    for fname, diff_lines in array_sections:
        if fname in ALREADY_HANDLED:
            continue
        out.append(f"**{fname}:**")
        out.extend(diff_lines)
        out.append("")

    scalar_rows = [(p, f, t) for p, f, t in scalar_rows
                   if p not in ("Objectives", "Stages")]
    if scalar_rows:
        out.append("| Field | From | To |")
        out.append("|-------|------|----|")
        for path, fs, ts in scalar_rows[:30]:
            out.append(f"| {path} | {fs.replace('|', chr(92)+'|')} | {ts.replace('|', chr(92)+'|')} |")
        if len(scalar_rows) > 30:
            out.append(f"| *…* | *{len(scalar_rows) - 30} more fields* | |")
        out.append("")

    if raw_fields:
        out.append(f"*Raw/unmapped subrecords changed: "
                   f"{', '.join(raw_fields[:5])}{'…' if len(raw_fields) > 5 else ''}*")
        out.append("")

    if len(out) <= 2:
        out.append("*(no decoded field changes)*")
        out.append("")

    return out


# --------------------------------------------------------------------------
# Added/removed record table rendering
# --------------------------------------------------------------------------

def render_added_table(records):
    """Render added records as a markdown table, showing name when available."""
    has_names = any(r.get("name") for r in records)
    has_desc = any(r.get("description") for r in records)

    out = []
    if has_names or has_desc:
        cols = "| FormID | EditorID | Name |"
        sep  = "|--------|----------|------|"
        out.append(cols)
        out.append(sep)
        for r in records:
            edid = r.get("editor_id") or "*(no edid)*"
            name = r.get("name") or r.get("description") or ""
            out.append(f"| `{r['form_id']}` | `{edid}` | {name} |")
    else:
        out.append("| FormID | EditorID |")
        out.append("|--------|----------|")
        for r in records:
            edid = r.get("editor_id") or "*(no edid)*"
            out.append(f"| `{r['form_id']}` | `{edid}` |")
    return out


# --------------------------------------------------------------------------
# Cut detection pass over all diff data
# --------------------------------------------------------------------------

def compute_cut_info(diff_data):
    """
    Scan the full diff for cut/deprecated records.
    Returns dict:
      newly_deprecated: list of {stub, cut_info, prev_editor_id}
      added_cut: list of RecordStub
      still_cut_changed: list of {stub, cut_info}
    """
    newly_dep = []
    added_cut_list = []
    still_cut = []

    for c in diff_data.get("changed", []):
        stub = c["stub"]
        edid = stub.get("editor_id", "")
        prev = c.get("prev_editor_id")
        ci = classify_cut(edid, prev_edid=prev)
        if ci:
            entry = {"stub": stub, "cut_info": ci, "prev_editor_id": prev}
            if ci["kind"] == "newly_deprecated":
                newly_dep.append(entry)
            else:  # still_cut
                still_cut.append(entry)

    for r in diff_data.get("added", []):
        edid = r.get("editor_id", "")
        ci = classify_cut(edid)
        if ci:
            added_cut_list.append(r)

    return {
        "newly_deprecated": newly_dep,
        "added_cut": added_cut_list,
        "still_cut_changed": still_cut,
    }


# --------------------------------------------------------------------------
# Main generator
# --------------------------------------------------------------------------

def generate_markdown(diff_data, old_label, new_label, patch_date,
                      timing=None, highlights_text=None):
    out = []

    added   = diff_data["added"]
    removed = diff_data["removed"]
    changed = diff_data["changed"]
    ref_names = diff_data.get("ref_names", {})

    # Build per-type lookup structures
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
        list(added_by_type) + list(removed_by_type) + list(changed_by_type)
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

    # Cut/deprecation pass
    cut_info = compute_cut_info(diff_data)

    # Cut info lookup by form_id for quick access during rendering
    cut_by_fid = {}
    for item in cut_info["newly_deprecated"] + cut_info["still_cut_changed"]:
        cut_by_fid[item["stub"]["form_id"]] = item["cut_info"]

    # ---- Header -----------------------------------------------------------
    out.append(f"# Fallout 76 ESM Patch Notes — {patch_date}")
    out.append("")
    out.append(f"> **Comparing:** `{old_label}` → `{new_label}`  ")
    out.append(f"> Generated from binary ESM diff (esm).")
    out.append("")

    if timing:
        out.append("## Parse & Diff Timing")
        out.append("")
        out.append("| Step | Time |")
        out.append("|------|------|")
        out.append(f"| Open + index `{old_label}` | {timing.get('open_a', 0):.2f}s |")
        out.append(f"| Open + index `{new_label}` | {timing.get('open_b', 0):.2f}s |")
        out.append(f"| Diff computation ({len(added)} added, {len(removed)} removed, {len(changed)} changed) | {timing.get('diff', 0):.2f}s |")
        out.append("| Interpretation + markdown generation | {INTERPRET_TIME} |")
        out.append(f"| **Total (parse + diff + interpret)** | {{TOTAL_TIME}} |")
        out.append("")

    # ---- Highlights -------------------------------------------------------
    out.append("## ⚡ Highlights & Notable Changes")
    out.append("")

    if highlights_text:
        # User/LLM-authored highlights injected verbatim
        out.append(highlights_text.strip())
        out.append("")
    else:
        # Auto-generated highlights from the diff data
        out.append("*Auto-generated from diff data — see detailed sections below for full field values.*")
        out.append("")
        auto_sections = build_auto_highlights(diff_data, cut_info)
        for section_name, bullets in auto_sections:
            out.append(f"### {section_name}")
            out.append("")
            for b in bullets:
                out.append(f"- {b}")
            out.append("")

    # ---- Cut / Deprecated section -----------------------------------------
    nd = cut_info["newly_deprecated"]
    ac = cut_info["added_cut"]
    sc = cut_info["still_cut_changed"]

    if nd or ac or sc:
        out.append("## ⚰️ Cut / Deprecated Content")
        out.append("")
        out.append("Records whose EditorID carries a deprecation/cut marker "
                   "(`ZZZ`, `CUT`, `POST`, `DEPRECATED`, `DELETE`).")
        out.append("")

        if nd:
            out.append("### Newly Deprecated This Patch")
            out.append("")
            out.append("*These records had their EditorID renamed to include a cut/deprecation "
                       "marker this patch — highest signal for content being retired.*")
            out.append("")
            for item in nd:
                stub = item["stub"]
                ci   = item["cut_info"]
                prev = item["prev_editor_id"]
                edid = stub.get("editor_id", "")
                fid  = stub["form_id"]
                rtype = stub["record_type"]
                name = stub.get("name", "")
                label = f'*"{name}"* ' if name else ""
                out.append(
                    f"- **{rtype}** `{fid}` {label}"
                    f"EditorID: `{prev}` → `{edid}` "
                    f"*(marker: `{ci['marker']}`, confidence: {ci['confidence']})*"
                )
            out.append("")

        if ac:
            out.append("### Added Already-Cut / Placeholder")
            out.append("")
            out.append("*New records whose EditorID already has a cut marker — likely placeholders.*")
            out.append("")
            for r in ac:
                edid  = r.get("editor_id", "")
                fid   = r["form_id"]
                rtype = r["record_type"]
                name  = r.get("name", "")
                label = f'*"{name}"* ' if name else ""
                out.append(f"- **{rtype}** `{fid}` {label}`{edid}`")
            out.append("")

        if sc:
            out.append("### Still-Cut, Changed This Patch")
            out.append("")
            out.append("*Records that were already marked cut/deprecated and received changes.*")
            out.append("")
            for item in sc:
                stub  = item["stub"]
                ci    = item["cut_info"]
                edid  = stub.get("editor_id", "")
                fid   = stub["form_id"]
                rtype = stub["record_type"]
                out.append(f"- **{rtype}** `{fid}` `{edid}` *(marker: `{ci['marker']}`)*")
            out.append("")

    # ---- Summary table ----------------------------------------------------
    out.append("## Summary")
    out.append("")
    out.append("| Metric | Count |")
    out.append("|--------|-------|")
    out.append(f"| ✅ Added records | **{len(added)}** |")
    out.append(f"| ❌ Removed records | **{len(removed)}** |")
    out.append(f"| 🔄 Changed records | **{len(changed)}** |")
    out.append(f"| **Total touched** | **{len(added)+len(removed)+len(changed)}** |")
    if nd or ac or sc:
        out.append(f"| ⚰️ Newly deprecated | **{len(nd)}** |")
        out.append(f"| ⚰️ Added already-cut | **{len(ac)}** |")
    out.append("")

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
    out.append("Changes grouped by EditorID prefix (which typically identifies "
               "the game system or seasonal event they belong to).")
    out.append("")

    sorted_features = sorted(
        feature_records,
        key=lambda p: -(len(feature_records[p]["added"]) +
                        len(feature_records[p]["removed"]) +
                        len(feature_records[p]["changed"])),
    )

    for prefix in sorted_features:
        frec = feature_records[prefix]
        name_str, desc = FEATURE_MAP.get(prefix, (f"{prefix}_ Records", ""))
        total = len(frec["added"]) + len(frec["removed"]) + len(frec["changed"])
        if total == 0:
            continue

        out.append(f"### {name_str}")
        out.append("")
        if desc:
            out.append(f"*{desc}*")
            out.append("")
        out.append("| Category | Count |")
        out.append("|----------|-------|")
        if frec["added"]:
            out.append(f"| Added | {len(frec['added'])} |")
        if frec["removed"]:
            out.append(f"| Removed | {len(frec['removed'])} |")
        if frec["changed"]:
            out.append(f"| Changed | {len(frec['changed'])} |")
        out.append("")

        if frec["added"]:
            by_type = defaultdict(list)
            for r in frec["added"]:
                by_type[r["record_type"]].append(r)
            out.append("**Newly added records:**")
            out.append("")
            for t in sorted(by_type):
                out.append(f"- **{t}** ({TYPE_DESC.get(t, '')}):")
                for r in by_type[t]:
                    lbl = edid_label(r.get("editor_id"), r["form_id"], r.get("name"))
                    out.append(f"  - {lbl}")
            out.append("")

        if frec["removed"]:
            out.append("**Removed records:**")
            out.append("")
            for r in frec["removed"]:
                lbl = edid_label(r.get("editor_id"), r["form_id"], r.get("name"))
                out.append(f"- `{r['record_type']}` {lbl}")
            out.append("")

        if frec["changed"]:
            out.append("**Changed records:**")
            out.append("")
            for c in frec["changed"]:
                stub = c["stub"]
                rtype = stub["record_type"]
                fc = c.get("field_changes", {})
                ci = cut_by_fid.get(stub["form_id"])
                lines = render_changed_record(stub, fc, compact=True,
                                              ref_names=ref_names, cut_info=ci)
                lines[0] = f"**[{rtype}]** {lines[0]}"
                out.extend(lines)
            out.append("")

    # ---- Detailed Changes by Record Type ----------------------------------
    out.append("---")
    out.append("")
    out.append("## Detailed Changes by Record Type")
    out.append("")
    out.append("> CELL and WRLD records are excluded (world geometry, not decoded by this parser).")
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

        if a_list:
            out.append(f"#### Added ({len(a_list)})")
            out.append("")
            out.extend(render_added_table(a_list))
            out.append("")

        if r_list:
            out.append(f"#### Removed ({len(r_list)})")
            out.append("")
            out.extend(render_added_table(r_list))
            out.append("")

        if c_list:
            out.append(f"#### Changed ({len(c_list)})")
            out.append("")
            for c in c_list:
                stub = c["stub"]
                fc = c.get("field_changes", {})
                ci = cut_by_fid.get(stub["form_id"])
                out.extend(render_changed_record(stub, fc, compact=False,
                                                 ref_names=ref_names, cut_info=ci))

        out.append("")

    # ---- Footer -----------------------------------------------------------
    out.append("---")
    out.append("")
    out.append(f"*Generated by [esm](https://github.com/) from binary ESM diff.*  ")
    out.append(f"*{old_label} vs {new_label}*  ")
    out.append(f"*Total: {len(added)} added · {len(removed)} removed · {len(changed)} changed*")
    out.append("")

    return "\n".join(out)


# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------

if __name__ == "__main__":
    ap = argparse.ArgumentParser(
        description="Generate patch notes markdown from ESM diff JSON."
    )
    ap.add_argument("diff_json", nargs="?", default="/tmp/fo76_diff.json",
                    help="Path to the diff JSON file (default: /tmp/fo76_diff.json)")
    ap.add_argument("--old-label", default=None,
                    help="Display label for the old ESM")
    ap.add_argument("--new-label", default=None,
                    help="Display label for the new ESM")
    ap.add_argument("--patch-date", default=None,
                    help="Patch date string (default: derived from --new-label)")
    ap.add_argument("--highlights-file", default=None, metavar="FILE",
                    help="Inject a markdown file verbatim as the Highlights section "
                         "(skips auto-highlights)")
    ap.add_argument("--open-a", type=float, default=None, metavar="SECS")
    ap.add_argument("--open-b", type=float, default=None, metavar="SECS")
    ap.add_argument("--diff",   type=float, default=None, metavar="SECS")
    args = ap.parse_args()

    print(f"Loading {args.diff_json}…", file=sys.stderr)
    with open(args.diff_json) as f:
        data = json.load(f)
    print(
        f"Loaded: {len(data['added'])} added, {len(data['removed'])} removed, "
        f"{len(data['changed'])} changed",
        file=sys.stderr,
    )
    print(f"  ref_names: {len(data.get('ref_names', {}))} resolved FormID references",
          file=sys.stderr)

    # Derive labels
    old_label = args.old_label or "old.esm"
    new_label = args.new_label or "new.esm"
    if args.patch_date:
        patch_date = args.patch_date
    else:
        _, _, patch_date = derive_labels_from_filenames(old_label, new_label)

    highlights_text = None
    if args.highlights_file:
        with open(args.highlights_file) as f:
            highlights_text = f.read()

    timing = None
    if args.open_a is not None or args.open_b is not None or args.diff is not None:
        timing = {
            "open_a": args.open_a or 0.0,
            "open_b": args.open_b or 0.0,
            "diff":   args.diff   or 0.0,
        }

    t_start = _time.time()
    md = generate_markdown(data, old_label, new_label, patch_date,
                           timing=timing, highlights_text=highlights_text)
    t_interpret = _time.time() - t_start

    if timing:
        timing["interpret"] = t_interpret
        total = sum(timing.values())
        md = md.replace("{INTERPRET_TIME}", f"{t_interpret:.2f}s")
        md = md.replace("{TOTAL_TIME}",     f"{total:.2f}s")

    print(f"Generated {len(md):,} chars of markdown in {t_interpret:.2f}s", file=sys.stderr)
    print(md)
