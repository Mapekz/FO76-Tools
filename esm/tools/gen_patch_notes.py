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

# Gameplay types that get fully-decoded spec sheets in the Detailed section.
# Cosmetic/world types keep the identity-only render_added_table.
SPEC_SHEET_TYPES = {
    "WEAP", "ARMO", "AMMO", "PROJ", "EXPL", "COBJ",
    "OMOD", "AVIF", "NPC_", "LVLI", "MGEF", "ENCH",
}

# NPC / character curve sampling window (HP, resist, damage scaling by level).
# Adjust these when the level cap rises — e.g. set MAX to 500.
NPC_CURVE_LEVEL_MIN  = 100
NPC_CURVE_LEVEL_MAX  = 200
NPC_CURVE_LEVEL_STEP = 25

# Player-gear level-band fallback when Object Template carries no Level Min/Max.
PLAYER_LEVEL_FALLBACK_MIN = 1
PLAYER_LEVEL_FALLBACK_MAX = 50

# Per-block display caps (keep spec sheets scannable and Discord-chunk-safe).
MAX_CURVE_ROWS        = 8
MAX_DMG_TYPES         = 8
MAX_MODS_PER_SLOT     = 6
MAX_LVLI_ROWS         = 12
MAX_PERKS             = 12
MAX_INVENTORY         = 12
MAX_SOURCES_PER_KIND  = 6

SOURCE_KIND_LABEL = {
    "leveled_list": "Leveled List",
    "container":    "Container",
    "recipe":       "Recipe",
    "quest":        "Quest",
    "npc_drop":     "NPC Drop",
    "vendor":       "Vendor",
    "world":        "World",
}

# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

def derive_labels_from_filenames(old_label, new_label):
    """Derive PATCH_DATE from new_label filename stem (e.g. Game_YYYYMMDD.esm → YYYY-MM-DD)."""
    if not new_label:
        return old_label, new_label, "Unknown Date"
    stem = os.path.splitext(os.path.basename(new_label))[0]
    # Extract 8-digit date token (YYYYMMDD) and reformat as "YYYY-MM-DD"
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


# --------------------------------------------------------------------------
# Curve helpers
# --------------------------------------------------------------------------

def curve_eval(points, x):
    """Linear interpolate y at x over [{'x': f, 'y': f}, ...].
    Mirrors Rust curves::eval: clamp to first.y below first.x,
    last.y above last.x, lerp between. Returns None for an empty curve."""
    if not points:
        return None
    pts = sorted(points, key=lambda p: p["x"])  # defensive sort
    if x <= pts[0]["x"]:
        return pts[0]["y"]
    if x >= pts[-1]["x"]:
        return pts[-1]["y"]
    for i in range(len(pts) - 1):
        x0, y0 = pts[i]["x"], pts[i]["y"]
        x1, y1 = pts[i + 1]["x"], pts[i + 1]["y"]
        if x0 <= x <= x1:
            if x1 == x0:
                return y0
            t = (x - x0) / (x1 - x0)
            return y0 + t * (y1 - y0)
    return pts[-1]["y"]


def fmt_num(v):
    """Compact number: drop trailing '.0', round floats to 2 dp."""
    if v is None:
        return "?"
    if isinstance(v, float):
        r = round(v, 2)
        return str(int(r)) if r == int(r) else str(r)
    return str(v)


def curve_table_for_levels(points, levels):
    """Evaluate curve at each level; collapse runs of equal rounded values.
    Returns [(label_str, value_str), ...] capped at MAX_CURVE_ROWS."""
    if not points or not levels:
        return []
    rows = []
    for lv in sorted(set(int(round(lv)) for lv in levels)):
        y = curve_eval(points, lv)
        rows.append((lv, fmt_num(y)))
    # Collapse consecutive identical values → "a–b: val"
    collapsed = []
    i = 0
    while i < len(rows):
        lv, val = rows[i]
        j = i + 1
        while j < len(rows) and rows[j][1] == val:
            j += 1
        if j - i == 1:
            collapsed.append((str(lv), val))
        else:
            collapsed.append((f"{lv}–{rows[j - 1][0]}", val))
        i = j
    if len(collapsed) > MAX_CURVE_ROWS:
        dropped = len(collapsed) - MAX_CURVE_ROWS + 1
        collapsed = collapsed[:MAX_CURVE_ROWS - 1] + [(f"…+{dropped} more", "")]
    return collapsed


def fmt_curve_inline(rows):
    """Join [(label, value), ...] as 'label: val · label: val · …'."""
    return " · ".join(f"{lb}: {vl}" for lb, vl in rows if vl)


def is_curve(val):
    """True if val is a decoded FormID reference with inlined curve points."""
    return isinstance(val, dict) and "curve" in val and isinstance(val.get("curve"), list)


def iter_curves(node, path=""):
    """Recursively yield (dotted_label, points_list) for every inlined curve."""
    if is_curve(node):
        yield path, node["curve"]
    elif isinstance(node, dict):
        for k, v in node.items():
            sub = f"{path}.{k}" if path else k
            yield from iter_curves(v, sub)
    elif isinstance(node, list):
        for i, item in enumerate(node):
            yield from iter_curves(item, f"{path}[{i}]")


def derive_eligible_levels(fields):
    """Return (min_level, max_level) from Object Template Level Min/Max.
    Falls back to PLAYER_LEVEL_FALLBACK_MIN/MAX if absent."""
    lo, hi = None, None
    tmpl = fields.get("Object Template")
    if isinstance(tmpl, list):
        for combo in tmpl:
            items = combo.get("Object Mod Template Item") or []
            if isinstance(items, dict):
                items = [items]
            for item in (items if isinstance(items, list) else []):
                lmin = item.get("Level Min")
                lmax = item.get("Level Max")
                if isinstance(lmin, (int, float)):
                    lo = lmin if lo is None else min(lo, lmin)
                if isinstance(lmax, (int, float)):
                    hi = lmax if hi is None else max(hi, lmax)
    lo = lo if lo is not None else PLAYER_LEVEL_FALLBACK_MIN
    hi = hi if hi is not None else PLAYER_LEVEL_FALLBACK_MAX
    return int(lo), int(hi)


def _player_curve_rows(points, fields):
    """Evaluate curve at player-eligible levels: band endpoints + in-band breakpoints."""
    lmin, lmax = derive_eligible_levels(fields)
    in_band = {int(round(p["x"])) for p in points if lmin <= p["x"] <= lmax}
    levels = sorted({lmin, lmax} | in_band)
    return curve_table_for_levels(points, levels)


def _npc_curve_levels():
    return list(range(NPC_CURVE_LEVEL_MIN, NPC_CURVE_LEVEL_MAX + 1, NPC_CURVE_LEVEL_STEP))


def _npc_curve_cells(points):
    """Evaluate curve at NPC stat window levels. Returns list of value strings."""
    return [fmt_num(curve_eval(points, lv)) for lv in _npc_curve_levels()]


def _ref(v, ref_names):
    """Format a FormID value (possibly a curve object) as a readable reference."""
    if is_curve(v):
        fid = v.get("formid", "?")
        return format_scalar(fid, ref_names)
    return format_scalar(v, ref_names)


# --------------------------------------------------------------------------
# Sources block renderer
# --------------------------------------------------------------------------

def render_sources_block(sources, ref_names=None, label="Sources"):
    """Group a sources list by kind and return markdown bullet lines."""
    if not sources:
        return []
    by_kind = defaultdict(list)
    for s in sources:
        by_kind[s.get("kind", "world")].append(s)
    lines = [f"*{label}:*"]
    order = ["leveled_list", "container", "recipe", "quest", "npc_drop", "vendor", "world"]
    for kind in order:
        group = by_kind.get(kind)
        if not group:
            continue
        lbl = SOURCE_KIND_LABEL.get(kind, kind)
        entries = group[:MAX_SOURCES_PER_KIND]
        extra = len(group) - len(entries)
        parts = []
        for s in entries:
            fid = s.get("form_id", "?")
            rt = s.get("record_type", "?")
            edid = s.get("editor_id") or ""
            sname = s.get("name") or ""
            if sname and edid:
                parts.append(f"`{fid}` ({rt}: `{edid}` *\"{sname}\"*)")
            elif edid:
                parts.append(f"`{fid}` ({rt}: `{edid}`)")
            else:
                parts.append(f"`{fid}` ({rt})")
        suffix = f" *+{extra} more*" if extra else ""
        lines.append(f"- **{lbl}** ({len(group)}): {', '.join(parts)}{suffix}")
    return lines


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


def diff_components(from_list, to_list, ref_names=None):
    """Render per-component crafting-cost quantity diffs."""
    def comp_key(entry):
        c = entry.get("Component")
        if isinstance(c, str):
            return c
        if isinstance(c, dict):
            return c.get("formid", str(c))
        return str(c)

    def qty(entry):
        q = entry.get("Quantity")
        if q is None:
            q = entry.get("Count")
        return q

    from_map = {comp_key(e): e for e in from_list if isinstance(e, dict)}
    to_map   = {comp_key(e): e for e in to_list   if isinstance(e, dict)}
    all_keys = list(from_map) + [k for k in to_map if k not in from_map]
    lines = []
    for key in all_keys:
        old_e = from_map.get(key)
        new_e = to_map.get(key)
        comp_ref = _ref(new_e.get("Component") if new_e else (old_e.get("Component") if old_e else key), ref_names)
        if old_e is None:
            lines.append(f"  - **+** {comp_ref}: ×{fmt_num(qty(new_e))}")
        elif new_e is None:
            lines.append(f"  - **−** {comp_ref}: was ×{fmt_num(qty(old_e))}")
        else:
            oq, nq = qty(old_e), qty(new_e)
            if oq != nq:
                lines.append(f"  - {comp_ref}: ×{fmt_num(oq)} → ×{fmt_num(nq)}")
    return lines or None


def smart_array_diff(from_list, to_list, ref_names=None):
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

    # Component structs (COBJ Components / Repair / Scrap Recieved).
    # Must come before the "Combination"/"mod" branch to avoid mis-matching.
    if "Component" in sample or "Quantity" in sample:
        return diff_components(from_list, to_list, ref_names) or None

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
                    ref_names,
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
# Spec-sheet renderers for added gameplay records
# --------------------------------------------------------------------------

def _spec_header(record):
    """Shared one-line header for every spec sheet."""
    edid    = record.get("editor_id") or "*(no edid)*"
    form_id = record.get("form_id", "?")
    name    = record.get("name")
    if name:
        return f"**`{edid}`** `{form_id}` — *{name}*"
    return f"**`{edid}`** `{form_id}`"


def render_weap_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    data = fields.get("Data") or fields.get("DNAM") or {}
    stats = []
    for key in ("Value", "Weight", "Weapon Type"):
        v = data.get(key)
        if v is not None:
            stats.append(f"{key} {format_scalar(v, ref_names)}")
    if stats:
        out.append("- **Stats:** " + " · ".join(stats))

    dmg_parts = []
    for key in ("Base Damage",):
        v = data.get(key)
        if v is not None:
            dmg_parts.append(f"Base Dmg {format_scalar(v, ref_names)}")
    for key in ("Action Point Cost", "Speed", "Attack Delay Seconds"):
        v = data.get(key)
        if v is not None:
            dmg_parts.append(f"{key} {format_scalar(v, ref_names)}")
    ammo = data.get("Ammo")
    if ammo:
        dmg_parts.append(f"Ammo {_ref(ammo, ref_names)}")
    cap = data.get("Capacity")
    if cap is not None:
        dmg_parts.append(f"Capacity {format_scalar(cap, ref_names)}")
    shots = data.get("Ammo used per shot")
    if shots is not None:
        dmg_parts.append(f"{shots}/shot")
    if dmg_parts:
        out.append("- **Weapon:** " + " · ".join(str(x) for x in dmg_parts))

    # Projectile override
    for struct_key in ("FNAM", "RGW2", "RGW3"):
        proj_struct = fields.get(struct_key) or {}
        if proj_struct:
            proj = proj_struct.get("Override Projectile")
            nproj = proj_struct.get("# Projectiles")
            fire  = proj_struct.get("Animation Fire Seconds")
            pparts = []
            if proj:
                pparts.append(f"Proj {_ref(proj, ref_names)}")
            if nproj:
                pparts.append(f"×{nproj}")
            if fire:
                pparts.append(f"fire {format_scalar(fire, ref_names)}s")
            if pparts:
                out.append("- **Projectile:** " + " · ".join(pparts))
            break

    # Crit
    crit = fields.get("Critical Data") or {}
    if crit:
        cparts = []
        cm = crit.get("Crit Damage Mult")
        ce = crit.get("Crit Effect")
        if cm is not None:
            cparts.append(f"×{fmt_num(cm)} dmg")
        if ce:
            cparts.append(f"effect {_ref(ce, ref_names)}")
        if cparts:
            out.append("- **Crit:** " + " · ".join(cparts))

    # Damage types (with curve eval)
    dmg_types = fields.get("Damage Types") or []
    if isinstance(dmg_types, list) and dmg_types:
        lmin, lmax = derive_eligible_levels(fields)
        out.append("")
        out.append("*Damage Types:*")
        out.append("| Type | Amount | Scaling (player lvl→amt) |")
        out.append("|------|--------|--------------------------|")
        for entry in dmg_types[:MAX_DMG_TYPES]:
            if not isinstance(entry, dict):
                continue
            dtype = _ref(entry.get("Type"), ref_names)
            amount = fmt_num(entry.get("Amount"))
            curv = entry.get("Curve Table")
            if is_curve(curv):
                rows = _player_curve_rows((curv or {}).get("curve") or [], fields)
                scaling = fmt_curve_inline(rows) or "flat"
            else:
                scaling = "flat"
            out.append(f"| {dtype} | {amount} | {scaling} |")

    # Mod slots from Object Template
    tmpl = fields.get("Object Template")
    if isinstance(tmpl, list) and tmpl:
        lmin, lmax = derive_eligible_levels(fields)
        # Count distinct attach-point indices and collect mods
        slots = defaultdict(list)
        for combo in tmpl:
            items = combo.get("Object Mod Template Item") or []
            if isinstance(items, dict):
                items = [items]
            for item in (items if isinstance(items, list) else []):
                includes = item.get("Includes") or []
                if isinstance(includes, dict):
                    includes = [includes]
                for inc in (includes if isinstance(includes, list) else []):
                    mod = inc.get("Mod")
                    idx = inc.get("Attach Point Index", "?")
                    if mod and _ref(mod, ref_names) not in [_ref(m, ref_names) for m in slots[idx]]:
                        slots[idx].append(mod)
        if slots:
            out.append("")
            out.append(f"*Mod slots:* {len(slots)} (eligible levels {lmin}–{lmax})")
            for idx, mods in sorted(slots.items(), key=lambda x: str(x[0])):
                shown = mods[:MAX_MODS_PER_SLOT]
                extra = len(mods) - len(shown)
                mod_strs = [_ref(m, ref_names) for m in shown]
                suffix = f" *+{extra} more*" if extra else ""
                out.append(f"- Slot [{idx}]: {', '.join(mod_strs)}{suffix}")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_armo_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    armor_data = fields.get("Armor Data") or fields.get("DATA") or {}
    stats = []
    for key in ("Value", "Weight", "Health"):
        v = armor_data.get(key)
        if v is not None:
            stats.append(f"{key} {format_scalar(v, ref_names)}")
    if stats:
        out.append("- **Stats:** " + " · ".join(stats))

    rating = fields.get("Rating Addon Data") or {}
    rparts = []
    for key in ("Armor Rating", "Stagger Rating"):
        v = rating.get(key)
        if v is not None:
            rparts.append(f"{key} {format_scalar(v, ref_names)}")
    if rparts:
        out.append("- **Ratings:** " + " · ".join(rparts))

    resists = fields.get("Resistances") or []
    if isinstance(resists, list) and resists:
        out.append("")
        out.append("*Resistances:*")
        out.append("| Type | Amount | Scaling (player lvl→amt) |")
        out.append("|------|--------|--------------------------|")
        for entry in resists[:MAX_DMG_TYPES]:
            if not isinstance(entry, dict):
                continue
            rtype = _ref(entry.get("Type"), ref_names)
            amount = fmt_num(entry.get("Amount"))
            curv = entry.get("Curve Table")
            if is_curve(curv):
                rows = _player_curve_rows((curv or {}).get("curve") or [], fields)
                scaling = fmt_curve_inline(rows) or "flat"
            else:
                scaling = "flat"
            out.append(f"| {rtype} | {amount} | {scaling} |")

    # Mod slots
    tmpl = fields.get("Object Template")
    if isinstance(tmpl, list) and tmpl:
        lmin, lmax = derive_eligible_levels(fields)
        slots = defaultdict(list)
        for combo in tmpl:
            items = combo.get("Object Mod Template Item") or []
            if isinstance(items, dict):
                items = [items]
            for item in (items if isinstance(items, list) else []):
                includes = item.get("Includes") or []
                if isinstance(includes, dict):
                    includes = [includes]
                for inc in (includes if isinstance(includes, list) else []):
                    mod = inc.get("Mod")
                    idx = inc.get("Attach Point Index", "?")
                    if mod:
                        slots[idx].append(mod)
        if slots:
            out.append("")
            out.append(f"*Mod slots:* {len(slots)} (eligible levels {lmin}–{lmax})")
            for idx, mods in sorted(slots.items(), key=lambda x: str(x[0])):
                shown = mods[:MAX_MODS_PER_SLOT]
                extra = len(mods) - len(shown)
                mod_strs = [_ref(m, ref_names) for m in shown]
                suffix = f" *+{extra} more*" if extra else ""
                out.append(f"- Slot [{idx}]: {', '.join(mod_strs)}{suffix}")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_ammo_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    dnam = fields.get("DNAM") or fields.get("Data") or {}
    parts = []
    for key in ("Damage", "Health"):
        v = dnam.get(key)
        if v is not None:
            parts.append(f"{key} {format_scalar(v, ref_names)}")
    proj = dnam.get("Projectile")
    if proj:
        parts.append(f"Proj {_ref(proj, ref_names)}")
    flags = dnam.get("Flags")
    if flags:
        parts.append(f"Flags {format_scalar(flags, ref_names)}")
    data = fields.get("Data") or {}
    for key in ("Value", "Weight"):
        v = data.get(key) if key not in dnam else None
        if v is not None:
            parts.append(f"{key} {format_scalar(v, ref_names)}")
    if parts:
        out.append("- " + " · ".join(parts))

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_proj_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    data = fields.get("Data") or fields.get("DNAM") or {}
    parts = []
    for key in ("Type", "Speed", "Gravity", "Range"):
        v = data.get(key)
        if v is not None:
            parts.append(f"{key} {format_scalar(v, ref_names)}")
    expl = data.get("Explosion")
    if expl:
        parts.append(f"Explosion {_ref(expl, ref_names)}")
    if parts:
        out.append("- " + " · ".join(parts))

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_expl_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    data = fields.get("Data") or fields.get("ENIT") or {}
    parts = []
    for key in ("Damage", "Force"):
        v = data.get(key)
        if v is not None:
            parts.append(f"{key} {format_scalar(v, ref_names)}")
    for key in ("Inner Radius", "Outer Radius"):
        v = data.get(key)
        if v is not None:
            parts.append(f"{key} {format_scalar(v, ref_names)}")
    sp = data.get("Spawn Projectile")
    if sp:
        parts.append(f"Spawn Proj {_ref(sp, ref_names)}")
    if parts:
        out.append("- " + " · ".join(parts))

    # Damage curve (breakpoints only — no parent Object Template for level band)
    dcurv = data.get("Damage Curve Table")
    if is_curve(dcurv):
        pts = (dcurv or {}).get("curve") or []
        breakpoints = sorted({int(round(p["x"])) for p in pts})
        rows = curve_table_for_levels(pts, breakpoints)
        if rows:
            out.append(f"- **Damage scaling:** {fmt_curve_inline(rows)}")

    dmg_types = fields.get("Damage Types") or []
    if isinstance(dmg_types, list) and dmg_types:
        out.append("")
        out.append("*Damage Types:*")
        out.append("| Type | Amount |")
        out.append("|------|--------|")
        for entry in dmg_types[:MAX_DMG_TYPES]:
            if not isinstance(entry, dict):
                continue
            out.append(f"| {_ref(entry.get('Type'), ref_names)} | {fmt_num(entry.get('Amount'))} |")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_cobj_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    created = fields.get("Created Object") or fields.get("CNAM")
    if created:
        out.append(f"- **Creates:** {_ref(created, ref_names)}")

    bench = fields.get("Workbench Keyword") or fields.get("BNAM")
    if bench:
        out.append(f"- **Workbench:** {_ref(bench, ref_names)}")

    learn_method = fields.get("Learn Method")
    if learn_method:
        lm_str = learn_method.get("name") if isinstance(learn_method, dict) else str(learn_method)
        out.append(f"- **Learn method:** {lm_str}")

    recipe_from = fields.get("Learn Recipe From") or fields.get("GNAM")
    if recipe_from:
        out.append(f"- **Learn recipe from:** {_ref(recipe_from, ref_names)}")

    build_group = fields.get("Build Group Name") or fields.get("NAM1")
    if build_group:
        out.append(f"- **Build group:** {format_scalar(build_group, ref_names)}")

    # Crafting cost
    components = fields.get("Components") or fields.get("FVPA") or []
    if isinstance(components, list) and components:
        out.append("")
        out.append("*Crafting cost:*")
        for entry in components:
            if not isinstance(entry, dict):
                continue
            comp = entry.get("Component")
            qty  = entry.get("Quantity")
            qs   = entry.get("Quantity Source", "count")
            if qty is None:
                qty = entry.get("Count")
            comp_ref = _ref(comp, ref_names)
            suspect = "" if qs == "curve" else " ⚠ *(no curve table — qty = raw Count, may be inaccurate)*"
            out.append(f"- {fmt_num(qty)} × {comp_ref}{suspect}")

    # Also show Repair and Scrap Recieved if present
    for arr_key in ("Repair", "Scrap Recieved"):
        arr = fields.get(arr_key) or []
        if isinstance(arr, list) and arr:
            out.append("")
            out.append(f"*{arr_key}:*")
            for entry in arr:
                if not isinstance(entry, dict):
                    continue
                comp = entry.get("Component")
                qty  = entry.get("Quantity") or entry.get("Count")
                out.append(f"- {fmt_num(qty)} × {_ref(comp, ref_names)}")

    # Recipe source: from sources that look like plans/notes
    recipe_sources = [s for s in sources if s.get("kind") in ("recipe", "vendor", "quest", "container")]
    other_sources  = [s for s in sources if s not in recipe_sources]
    if recipe_sources:
        out.append("")
        out.extend(render_sources_block(recipe_sources, ref_names, label="Recipe source"))
    if other_sources:
        out.extend(render_sources_block(other_sources, ref_names))
    elif not recipe_sources:
        out.extend(render_sources_block(sources, ref_names))

    return out


def render_omod_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    attach = fields.get("Attach Point") or fields.get("ANAM")
    if attach:
        out.append(f"- **Attach point:** {format_scalar(attach, ref_names)}")

    kws = fields.get("Target OMOD Keywords") or fields.get("MNAM") or []
    if kws:
        kw_strs = [format_scalar(k, ref_names) for k in (kws if isinstance(kws, list) else [kws])]
        out.append(f"- **Targets:** {', '.join(kw_strs[:6])}")

    data = fields.get("Data") or {}
    props = data.get("Properties") if isinstance(data, dict) else None
    if not props:
        props = fields.get("Properties") or fields.get("DATA") or []
    if isinstance(props, list) and props:
        out.append("")
        out.append("*Properties (effects):*")
        out.append("| Stat | Function | Value |")
        out.append("|------|----------|-------|")
        for p in props:
            if not isinstance(p, dict):
                continue
            stat = format_scalar(p.get("Property") or p.get("Actor Value"), ref_names)
            func = format_scalar(p.get("Function Type") or p.get("Type"), ref_names)
            v1   = p.get("Value 1") or p.get("Value")
            v2   = p.get("Value 2")
            val  = fmt_num(v1) if v2 is None else f"{fmt_num(v1)}, {fmt_num(v2)}"
            out.append(f"| {stat} | {func} | {val} |")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_avif_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    parts = []
    for key in ("Default Value", "Minimum Value", "Maximum Value"):
        v = fields.get(key)
        if v is not None:
            parts.append(f"{key.replace(' Value', '')} {format_scalar(v, ref_names)}")
    if parts:
        out.append("- **Values:** " + " · ".join(parts))

    desc = fields.get("Description") or fields.get("DESC")
    if desc:
        d = format_scalar(desc, ref_names)
        out.append(f"- *Description:* {d}")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_npc_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    # Level band
    acbs = fields.get("ACBS") or {}
    lv_parts = []
    lvl  = acbs.get("Level") or acbs.get("Level Mult")
    cmin = acbs.get("Calc min level")
    cmax = acbs.get("Calc max level")
    if cmin is not None and cmax is not None:
        lv_parts.append(f"Level {format_scalar(cmin, ref_names)}–{format_scalar(cmax, ref_names)}")
    elif lvl is not None:
        lv_parts.append(f"Level {format_scalar(lvl, ref_names)}")

    dnam = fields.get("DNAM") or {}
    hp = dnam.get("Calculated Health")
    ap = dnam.get("Calculated Action Points")
    if hp is not None:
        lv_parts.append(f"Health {format_scalar(hp, ref_names)}")
    if ap is not None:
        lv_parts.append(f"AP {format_scalar(ap, ref_names)}")
    if lv_parts:
        out.append("- **Stats:** " + " · ".join(lv_parts))

    znam = fields.get("Combat Style") or fields.get("ZNAM")
    if znam:
        out.append(f"- **Combat style:** {_ref(znam, ref_names)}")

    tplt = fields.get("Default Template") or fields.get("TPLT")
    if tplt:
        out.append(f"- **Default template:** {_ref(tplt, ref_names)}")

    # NPC scaling curves: collect all inlined curves and render the NPC window
    curve_found = {}
    for label, pts in iter_curves(fields):
        short = label.split(".")[-1].replace("[", "").replace("]", "")
        if short not in curve_found:
            curve_found[short] = pts
    if curve_found:
        levels = _npc_curve_levels()
        header_lvls = " | ".join(str(lv) for lv in levels)
        sep_lvls    = " | ".join("---" for _ in levels)
        out.append("")
        out.append(f"*Scaling (NPC level):*")
        out.append(f"| Stat | {header_lvls} |")
        out.append(f"|------|{sep_lvls}|")
        for stat, pts in list(curve_found.items())[:8]:
            cells = " | ".join(_npc_curve_cells(pts))
            out.append(f"| {stat} | {cells} |")

    perks = fields.get("Perks") or []
    if isinstance(perks, list) and perks:
        perk_strs = []
        for p in perks[:MAX_PERKS]:
            if not isinstance(p, dict):
                continue
            pf = p.get("Perk")
            rk = p.get("Rank")
            s = _ref(pf, ref_names)
            if rk is not None:
                s += f" r{rk}"
            perk_strs.append(s)
        extra = len(perks) - len(perk_strs)
        suffix = f" *+{extra} more*" if extra else ""
        out.append(f"*Perks ({len(perks)}):* {', '.join(perk_strs)}{suffix}")

    inv = fields.get("Items") or fields.get("CNTO") or []
    if isinstance(inv, list) and inv:
        shown = inv[:MAX_INVENTORY]
        extra = len(inv) - len(shown)
        inv_strs = []
        for item in shown:
            if not isinstance(item, dict):
                continue
            it = item.get("Item")
            cnt = item.get("Count", 1)
            inv_strs.append(f"{cnt}× {_ref(it, ref_names)}")
        suffix = f" *+{extra} more*" if extra else ""
        out.append(f"*Inventory ({len(inv)}):* {', '.join(inv_strs)}{suffix}")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_lvli_spec(record, ref_names=None):
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    entries = fields.get("Leveled List Entries") or fields.get("LLCT") or []
    chance_none = fields.get("Chance None") or fields.get("LVLD")

    header_parts = [f"Entries: {len(entries)}"]
    if chance_none is not None and chance_none != 0:
        header_parts.append(f"Chance none: {fmt_num(chance_none)}%")
    out.append("- " + " · ".join(header_parts))

    if isinstance(entries, list) and entries:
        out.append("")
        out.append("| Lvl | Item | Count |")
        out.append("|-----|------|-------|")
        shown = entries[:MAX_LVLI_ROWS]
        extra = len(entries) - len(shown)
        for e in shown:
            if not isinstance(e, dict):
                continue
            bd = e.get("Base Data") or e
            lv  = bd.get("Level", "?")
            it  = bd.get("Item") or bd.get("Reference")
            cnt = bd.get("Count", 1)
            out.append(f"| {lv} | {_ref(it, ref_names)} | {cnt} |")
        if extra:
            out.append(f"| … | *{extra} more entries* | |")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


def render_generic_spec(record, ref_names=None):
    """Fallback: key→value table for MGEF, ENCH, and any unrecognised gameplay type."""
    fields  = record.get("fields") or {}
    sources = record.get("sources") or []
    out     = [_spec_header(record), ""]

    if fields:
        out.append("| Field | Value |")
        out.append("|-------|-------|")
        for k, v in list(fields.items())[:20]:
            if k.startswith("_"):
                continue
            out.append(f"| {k} | {format_scalar(v, ref_names)} |")

    # Any curves found anywhere
    for label, pts in list(iter_curves(fields))[:3]:
        breakpoints = sorted({int(round(p["x"])) for p in pts})
        rows = curve_table_for_levels(pts, breakpoints)
        if rows:
            out.append(f"- **{label} (curve breakpoints):** {fmt_curve_inline(rows)}")

    out.append("")
    out.extend(render_sources_block(sources, ref_names))
    return out


SPEC_RENDERERS = {
    "WEAP": render_weap_spec,
    "ARMO": render_armo_spec,
    "AMMO": render_ammo_spec,
    "PROJ": render_proj_spec,
    "EXPL": render_expl_spec,
    "COBJ": render_cobj_spec,
    "OMOD": render_omod_spec,
    "AVIF": render_avif_spec,
    "NPC_": render_npc_spec,
    "LVLI": render_lvli_spec,
    "LVLN": render_lvli_spec,
    "LVLP": render_lvli_spec,
}


def render_added_detail(record, ref_names=None):
    """Render a single added gameplay record as a spec sheet."""
    rtype = record.get("record_type", "")
    renderer = SPEC_RENDERERS.get(rtype, render_generic_spec)
    # If no decoded fields were attached (e.g. decode failed), fall back to identity.
    if not record.get("fields"):
        edid    = record.get("editor_id") or "*(no edid)*"
        form_id = record.get("form_id", "?")
        name    = record.get("name")
        if name:
            return [f"**`{edid}`** `{form_id}` — *{name}*", ""]
        return [f"**`{edid}`** `{form_id}`", ""]
    return renderer(record, ref_names)


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
            if rtype in SPEC_SHEET_TYPES:
                for rec in a_list:
                    out.extend(render_added_detail(rec, ref_names))
                    out.append("")   # blank separator = chunker split point
            else:
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
