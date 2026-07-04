#!/usr/bin/env python3
"""
patchnotes_lib.py — shared library for the FO76 patch-notes generation pipeline.

Consumes the raw `esm diff --json` output (`DiffResult` in `src/diff.rs`):
`{"added": [RecordStub], "removed": [RecordStub],
  "changed": [{"stub": RecordStub, "field_changes": {...}, "prev_editor_id"?}],
  "ref_names": {"0x...": {"record_type", "editor_id"?, "name"?, "description"?}}}`.

This module normalizes that sparse, recursive diff into a flat, uniform list
of `ChangeEntry` dicts (see `extract_changes`) plus a handful of cross-cutting
helpers used by every downstream pipeline stage (mechanical rendering,
narrative summarization, lint checks): cut/deprecation classification,
common-change collapsing across sibling records, FormID reference
harvesting, and patch manifest read/write.

`field_changes` leaves come in two shapes for arrays:
  (a) NEW Rust shape — `{"_array_diff": {"strategy", "key_fields"?,
      "count_from", "count_to", "added": [...], "removed": [...],
      "changed": [{"key", "index_from", "index_to", "changes": {...}}]}}`.
  (b) LEGACY shape — a whole-array `{"from": [...], "to": [...]}` pair (the
      diff engine treats arrays as opaque unless/until it's upgraded to (a),
      and older diff JSON on disk will always be shape (b)).
Both normalize to the identical `array` sub-structure on a ChangeEntry — see
`extract_changes` and `smart_array_diff`.

Python 3, stdlib only.
"""

from __future__ import annotations

import json
import re
import struct
from collections import defaultdict
from pathlib import Path

# --------------------------------------------------------------------------
# Constants
# --------------------------------------------------------------------------

SCHEMA_VERSION = 1

# Record-type descriptions (used for section headers downstream).
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

# Record types never rendered by the pipeline (world-placement/positional,
# not meaningfully decoded).
EXCLUDED_TYPES = {"WRLD", "CELL"}

# Cut/deprecation EDID marker prefixes (all-caps or all-lowercase, typically
# _-delimited).
CUT_MARKERS = ["ZZZ", "CUT", "POST", "DEPRECATED", "DELETE"]

# Top-level field_changes keys that are purely positional/technical noise —
# always suppressed (kept in the ChangeEntry list, flagged, never rendered).
NOISE_TOP_KEYS = {"Object Bounds"}

# The full set of values `ChangeEntry["suppressed"]` may take (besides None).
SUPPRESSED_REASONS = {"redundant_count", "noise", "raw"}

# Minimum number of "changed" records of the same record_type sharing an
# identical (path, from, to) scalar delta before compute_common_changes()
# collapses them into one CommonChange bullet.
DEFAULT_COMMON_THRESHOLD = 5

_FORMID_RE = re.compile(r"^0x[0-9A-Fa-f]{8}$")


# --------------------------------------------------------------------------
# Small formatting helpers
# --------------------------------------------------------------------------


def fmt_num(v):
    """Compact number: drop trailing '.0', round floats to 2 dp."""
    if v is None:
        return "?"
    if isinstance(v, float):
        r = round(v, 2)
        return str(int(r)) if r == int(r) else str(r)
    return str(v)


def is_curve(v):
    """True if val is a decoded FormID reference with inlined curve points:
    `{"formid", "curve_path", "curve": [{x,y}, ...]}`."""
    return isinstance(v, dict) and isinstance(v.get("curve"), list)


def is_formid_str(v):
    """True if v is a bare FormID hex string as produced by FormId::display():
    exactly "0x" followed by 8 hex digits (case-insensitive)."""
    return isinstance(v, str) and bool(_FORMID_RE.match(v))


def _format_ref_info(fid, rtype, edid, label):
    if label and edid:
        return f'`{fid}` ({rtype}: `{edid}` *"{label}"*)'
    if edid:
        return f"`{fid}` ({rtype}: `{edid}`)"
    if label:
        return f'`{fid}` ({rtype}: *"{label}"*)'
    return f"`{fid}` ({rtype})"


def annotate_ref(value, ref_names=None):
    """
    Format a FormID-shaped value — a bare hex string, or a resolved reference
    dict `{"formid", "editor_id"?, "record_type"?, "name"?}` — as a readable
    reference: "`0xFFFFFFFF` (TYPE: `EditorID` "Name")". Falls back to the
    bare hex when nothing more is known (a "dangling" FormID with no
    ref_names entry). Prefers `name`, then `description` (from ref_names),
    when choosing the quoted label.
    """
    ref_names = ref_names or {}
    if isinstance(value, str):
        fid = value
        info = ref_names.get(fid)
        if info is None:
            return f"`{fid}`"
        rtype = info.get("record_type", "?")
        edid = info.get("editor_id")
        label = info.get("name") or info.get("description")
        return _format_ref_info(fid, rtype, edid, label)
    if isinstance(value, dict):
        fid = value.get("formid", "?")
        rtype = value.get("record_type", "?")
        edid = value.get("editor_id")
        label = value.get("name") or value.get("Name") or value.get("description")
        return _format_ref_info(fid, rtype, edid, label)
    return format_scalar(value, ref_names)


def format_scalar(v, ref_names=None):
    """Format an arbitrary decoded value for a display cell (no newlines).
    FormID-shaped values (hex strings or resolved stub/curve dicts) are
    annotated via `annotate_ref`. Never raises on unexpected shapes."""
    if v is None:
        return "*(null)*"
    if isinstance(v, bool):
        return f"`{str(v).lower()}`"
    if isinstance(v, (int, float)):
        return f"`{v}`"
    if isinstance(v, str):
        if is_formid_str(v):
            return annotate_ref(v, ref_names)
        s = v[:100] + ("…" if len(v) > 100 else "")
        return f"`{s}`"
    if isinstance(v, dict):
        if v.get("_unresolved") and "lstring_id" in v:
            return f"`[lstring {v['lstring_id']}]` *(unresolved)*"
        if v.get("_raw"):
            return "`[raw hex]`"
        flags = v.get("flags")
        if isinstance(flags, list):
            return f"`{', '.join(flags) or '(none)'}`"
        if is_curve(v):
            return annotate_ref(v.get("formid"), ref_names)
        if "formid" in v and ("editor_id" in v or "record_type" in v):
            return annotate_ref(v, ref_names)
        name = v.get("name") or v.get("Name")
        if name:
            return f"`{name}`"
        return f"`(struct: {', '.join(list(v.keys())[:4])})`"
    return f"`{repr(v)[:60]}`"


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


def annotate_cut(record):
    """
    Classify whether a diff record is cut/deprecated content. `record` is
    either a `changed` entry `{"stub": RecordStub, "field_changes": {...},
    "prev_editor_id"?: str}` or a bare RecordStub (as used for `added` /
    `removed` entries). Returns the `classify_cut()` dict, or None.
    """
    stub = record.get("stub", record)
    edid = stub.get("editor_id") or ""
    prev_edid = record.get("prev_editor_id")
    return classify_cut(edid, prev_edid=prev_edid)


# --------------------------------------------------------------------------
# VMAD raw-hex decoding
# --------------------------------------------------------------------------
#
# Some VMAD blocks fail structured decoding on both sides of a diff (e.g. a
# truncated subrecord) and fall back to the decoder's `{"_raw": true, "hex":
# "..."}` sentinel. When only the "hex" bytes differ between old/new, the
# Rust sparse json_diff descends into that wrapper and emits a leaf change
# scoped to "hex" — path "... / hex" containing "Virtual Machine Adapter".
# decode_vmad_props() is a best-effort, format-agnostic scanner (it does not
# replay the full VMAD grammar) over that raw byte blob: it looks for a
# u16-length-prefixed ASCII property name followed by a 1-byte type + 1-byte
# status + type-dependent value, which matches the on-disk property layout
# closely enough to recover simple int32/float/bool property changes.


def decode_vmad_props(hex_str):
    try:
        data = bytes.fromhex(hex_str)
    except ValueError:
        return {}

    result = {}
    i = 0
    while i < len(data) - 6:
        length = struct.unpack_from("<H", data, i)[0]
        if 2 <= length <= 80 and i + 2 + length + 3 <= len(data):
            try:
                name = data[i + 2 : i + 2 + length].decode("ascii")
            except UnicodeDecodeError:
                i += 1
                continue
            if name and (name[0].isalpha() or name[0] == "_") and all(
                c.isalnum() or c in "_:." for c in name
            ):
                after = i + 2 + length
                prop_type = data[after]
                value_start = after + 2
                if prop_type == 3 and value_start + 4 <= len(data):
                    result[name] = struct.unpack_from("<i", data, value_start)[0]
                elif prop_type == 4 and value_start + 4 <= len(data):
                    result[name] = round(struct.unpack_from("<f", data, value_start)[0], 4)
                elif prop_type == 5 and value_start + 1 <= len(data):
                    result[name] = bool(data[value_start])
                elif prop_type in (1, 2):
                    result[name] = "(object/string)"
        i += 1
    return result


def diff_vmad(old_hex, new_hex):
    """
    Decode two VMAD raw-hex blobs via decode_vmad_props() and diff their
    scanned script properties. Returns
    `{"added": {name: value}, "removed": {name: value},
      "changed": {name: {"from": .., "to": ..}}}`.
    """
    old = decode_vmad_props(old_hex)
    new = decode_vmad_props(new_hex)
    added, removed, changed = {}, {}, {}
    for k in old.keys() | new.keys():
        if k not in new:
            removed[k] = old[k]
        elif k not in old:
            added[k] = new[k]
        elif old[k] != new[k]:
            changed[k] = {"from": old[k], "to": new[k]}
    return {"added": added, "removed": removed, "changed": changed}


# --------------------------------------------------------------------------
# Generic keyed array-pairing engine (shared by the legacy semantic differs)
# --------------------------------------------------------------------------


def _fields_diff(pairs):
    """pairs: iterable of (field_name, old_val, new_val). Returns a sparse
    field_changes-shaped dict containing only the fields that actually
    differ, suitable for handing to extract_changes()."""
    fc = {}
    for name, ov, nv in pairs:
        if ov != nv:
            fc[name] = {"from": ov, "to": nv}
    return fc


def _key_tuple_display(k, key_fields, ref_names):
    if not key_fields:
        return format_scalar(k, ref_names)
    if isinstance(k, tuple) and len(key_fields) == len(k):
        return ", ".join(f"{kf}={format_scalar(v, ref_names)}" for kf, v in zip(key_fields, k))
    if len(key_fields) == 1:
        return f"{key_fields[0]}={format_scalar(k, ref_names)}"
    return format_scalar(k, ref_names)


def _key_dict_display(key, ref_names):
    if not isinstance(key, dict) or not key:
        return format_scalar(key, ref_names)
    return ", ".join(f"{k}={format_scalar(v, ref_names)}" for k, v in key.items())


def _keyed_array_diff(from_list, to_list, key_fields, key_fn, unwrap_fn, fields_fn, display_fn, ref_names):
    """
    Generic engine behind diff_components / diff_omod_properties /
    diff_lvli_entries / diff_effects / diff_objectives / diff_stages: groups
    both lists by key_fn(), pairs same-key entries positionally (Bethesda
    arrays occasionally carry duplicate keys), and returns the normalized
    `array` structure (added/removed/changed) shared with `_array_diff`.

    key_fn(raw_elem) -> hashable key
    unwrap_fn(raw_elem) -> inner dict actually holding the comparable fields
        (some shapes wrap entries in a named container, e.g.
        {"Leveled List Entry": {...}})
    fields_fn(old_inner, new_inner) -> list[(field_name, old_val, new_val)]
    display_fn(raw_elem, ref_names) -> str one-line summary
    """
    from_groups, to_groups = defaultdict(list), defaultdict(list)
    for e in from_list:
        if isinstance(e, dict):
            from_groups[key_fn(e)].append(e)
    for e in to_list:
        if isinstance(e, dict):
            to_groups[key_fn(e)].append(e)

    added, removed, changed = [], [], []
    all_keys = list(from_groups) + [k for k in to_groups if k not in from_groups]
    for k in all_keys:
        fg, tg = from_groups.get(k, []), to_groups.get(k, [])
        for i in range(max(len(fg), len(tg))):
            oe = fg[i] if i < len(fg) else None
            ne = tg[i] if i < len(tg) else None
            if oe is None:
                added.append(
                    {
                        "key_display": _key_tuple_display(k, key_fields, ref_names),
                        "display": display_fn(ne, ref_names),
                        "raw": ne,
                    }
                )
            elif ne is None:
                removed.append(
                    {
                        "key_display": _key_tuple_display(k, key_fields, ref_names),
                        "display": display_fn(oe, ref_names),
                        "raw": oe,
                    }
                )
            else:
                oi, ni = unwrap_fn(oe), unwrap_fn(ne)
                fc = _fields_diff(fields_fn(oi, ni))
                if fc:
                    changed.append(
                        {
                            "key_display": _key_tuple_display(k, key_fields, ref_names),
                            "changes": extract_changes(fc, ref_names),
                        }
                    )

    return {
        "strategy": "keyed",
        "key_fields": key_fields,
        "count_from": len(from_list),
        "count_to": len(to_list),
        "added": added,
        "removed": removed,
        "changed": changed,
    }


# ---- Components (COBJ Components / Repair / Scrap Received) --------------


def _comp_key(entry):
    c = entry.get("Component")
    if isinstance(c, str):
        return c
    if isinstance(c, dict):
        return c.get("formid", str(c))
    return str(c)


def _comp_qty(entry):
    q = entry.get("Quantity")
    return q if q is not None else entry.get("Count")


def _comp_display(entry, ref_names):
    return f"{format_scalar(entry.get('Component'), ref_names)} ×{fmt_num(_comp_qty(entry))}"


def diff_components(from_list, to_list, ref_names=None):
    """Per-component crafting-cost quantity diff, keyed by the referenced
    Component."""
    ref_names = ref_names or {}
    return _keyed_array_diff(
        from_list,
        to_list,
        ["Component"],
        _comp_key,
        lambda e: e,
        lambda o, n: [("Quantity", _comp_qty(o), _comp_qty(n))],
        _comp_display,
        ref_names,
    )


# ---- OMOD properties (Data / Properties) ----------------------------------


def _omod_key(p):
    ft = p.get("Function Type") or p.get("Type")
    ft_name = ft.get("name") if isinstance(ft, dict) else ft
    prop = p.get("Property") or p.get("Actor Value")
    prop_name = prop.get("name") if isinstance(prop, dict) else prop
    return (ft_name, prop_name)


def _omod_value1(p):
    v = p.get("Value 1")
    return v if v is not None else p.get("Value")


def _omod_display(p, ref_names):
    ft = p.get("Function Type") or p.get("Type")
    func = ft.get("name") if isinstance(ft, dict) else format_scalar(ft, ref_names)
    prop = p.get("Property") or p.get("Actor Value")
    stat = prop.get("name") if isinstance(prop, dict) else format_scalar(prop, ref_names)
    v1, v2 = _omod_value1(p), p.get("Value 2")
    if v2 in (None, 0, 0.0):
        val = format_scalar(v1, ref_names)
    else:
        val = f"{format_scalar(v1, ref_names)}, {format_scalar(v2, ref_names)}"
    return f"{func} {stat} {val}"


def diff_omod_properties(from_list, to_list, ref_names=None):
    """Per-property diff of an OMOD's Data / Properties[] array, keyed by
    (Function Type, Property) — the source of 'ADD NumProjectiles +2' /
    'MUL+ADD Speed 1.5 -> 2.0' style deltas."""
    ref_names = ref_names or {}
    return _keyed_array_diff(
        from_list,
        to_list,
        ["Function Type", "Property"],
        _omod_key,
        lambda e: e,
        lambda o, n: [
            ("Value 1", _omod_value1(o), _omod_value1(n)),
            ("Value 2", o.get("Value 2"), n.get("Value 2")),
        ],
        _omod_display,
        ref_names,
    )


# ---- Leveled list entries --------------------------------------------------


def _lvli_unwrap(e):
    return e.get("Leveled List Entry", e) if isinstance(e, dict) else {}


def _lvli_ref(ue):
    return ue.get("Reference") or ue.get("Item")


def _lvli_qty(ue):
    return ue.get("Quantity", ue.get("Count", 1))


def _lvli_key(e):
    ue = _lvli_unwrap(e)
    ref = _lvli_ref(ue)
    lvl = ue.get("Minimum Level", ue.get("Level"))
    fid = ref.get("formid") if isinstance(ref, dict) else ref
    return (fid, lvl)


def _lvli_display(e, ref_names):
    ue = _lvli_unwrap(e)
    ref = _lvli_ref(ue)
    lvl = ue.get("Minimum Level", ue.get("Level"))
    qty = _lvli_qty(ue)
    return f"{format_scalar(ref, ref_names)} (min lvl {fmt_num(lvl)}, ×{fmt_num(qty)})"


def diff_lvli_entries(from_list, to_list, ref_names=None):
    """Per-entry diff of a leveled list's entries — added/removed items and
    quantity changes, keyed by (referenced item, minimum level)."""
    ref_names = ref_names or {}
    return _keyed_array_diff(
        from_list,
        to_list,
        ["Reference", "Minimum Level"],
        _lvli_key,
        _lvli_unwrap,
        lambda o, n: [("Quantity", _lvli_qty(o), _lvli_qty(n))],
        _lvli_display,
        ref_names,
    )


# ---- Effects (MGEF/ENCH/SPEL Effects[]) ------------------------------------


def _effects_unwrap(e):
    return e.get("Effect", e) if isinstance(e, dict) else {}


def _effects_item(ue):
    return ue.get("Effect Item Data") or {}


def _effects_key(e):
    ue = _effects_unwrap(e)
    base = ue.get("Base Effect")
    return base.get("formid") if isinstance(base, dict) else base


def _effects_display(e, ref_names):
    ue = _effects_unwrap(e)
    item = _effects_item(ue)
    return (
        f"{format_scalar(ue.get('Base Effect'), ref_names)} "
        f"(mag {fmt_num(item.get('Magnitude'))}, dur {fmt_num(item.get('Duration'))})"
    )


def diff_effects(from_list, to_list, ref_names=None):
    """Per-effect diff — added/removed effects and magnitude/area/duration
    changes, keyed by the referenced Base Effect."""
    ref_names = ref_names or {}

    def fields(o, n):
        oi, ni = _effects_item(o), _effects_item(n)
        return [
            ("Magnitude", oi.get("Magnitude"), ni.get("Magnitude")),
            ("Area", oi.get("Area"), ni.get("Area")),
            ("Duration", oi.get("Duration"), ni.get("Duration")),
        ]

    return _keyed_array_diff(
        from_list, to_list, ["Base Effect"], _effects_key, _effects_unwrap, fields, _effects_display, ref_names
    )


# ---- Objectives (QUST Objectives[]) ----------------------------------------


def _objectives_unwrap(e):
    return e.get("Objective", e) if isinstance(e, dict) else {}


def _objectives_key(e):
    return _objectives_unwrap(e).get("Objective Index")


def _objectives_display(e, ref_names):
    ue = _objectives_unwrap(e)
    return f'[{fmt_num(ue.get("Objective Index"))}] "{ue.get("Display Text", "")}"'


def diff_objectives(from_list, to_list, ref_names=None):
    """Per-objective diff, keyed by Objective Index."""
    ref_names = ref_names or {}
    return _keyed_array_diff(
        from_list,
        to_list,
        ["Objective Index"],
        _objectives_key,
        _objectives_unwrap,
        lambda o, n: [("Display Text", o.get("Display Text"), n.get("Display Text"))],
        _objectives_display,
        ref_names,
    )


# ---- Stages (QUST Stages[]) -------------------------------------------------


def _stages_unwrap(e):
    return e.get("Stage", e) if isinstance(e, dict) else {}


def _stages_key(e):
    stage = _stages_unwrap(e)
    return (stage.get("INDX") or {}).get("Stage Index")


def _stage_log_notes(stage):
    return [
        entry.get("Log Entry", {}).get("Note", "")
        for entry in stage.get("Log Entries", [])
        if entry.get("Log Entry", {}).get("Note")
    ]


def _stages_display(e, ref_names):
    ue = _stages_unwrap(e)
    idx = (ue.get("INDX") or {}).get("Stage Index")
    notes = _stage_log_notes(ue)
    return f"Stage {fmt_num(idx)}: {'; '.join(notes) if notes else 'no log entries'}"


def diff_stages(from_list, to_list, ref_names=None):
    """Per-stage diff, keyed by Stage Index; compares the joined Log Entry
    notes as a single text field."""
    ref_names = ref_names or {}
    return _keyed_array_diff(
        from_list,
        to_list,
        ["Stage Index"],
        _stages_key,
        _stages_unwrap,
        lambda o, n: [("Log Entries", "; ".join(_stage_log_notes(o)), "; ".join(_stage_log_notes(n)))],
        _stages_display,
        ref_names,
    )


# ---- Scalar (non-dict) arrays: set-diff by value ---------------------------


def _normalize_scalar_array(from_list, to_list, ref_names):
    added_vals = [v for v in to_list if v not in from_list]
    removed_vals = [v for v in from_list if v not in to_list]
    return {
        "strategy": "set",
        "key_fields": None,
        "count_from": len(from_list),
        "count_to": len(to_list),
        "added": [
            {"key_display": format_scalar(v, ref_names), "display": format_scalar(v, ref_names), "raw": v}
            for v in added_vals
        ],
        "removed": [
            {"key_display": format_scalar(v, ref_names), "display": format_scalar(v, ref_names), "raw": v}
            for v in removed_vals
        ],
        "changed": [],
    }


# ---- Dispatcher: legacy whole-array {from,to} -> normalized array shape ----


def smart_array_diff(from_list, to_list, ref_names=None):
    """
    LEGACY-shape array normalizer. Given a whole-array `{"from": [...], "to":
    [...]}` pair (the pre-`_array_diff` Rust diff format — still emitted for
    any array the diff engine hasn't upgraded, or present in older diff JSON
    on disk), detect the array's semantic "shape" from a sample element and
    key/diff it the same way the new `_array_diff` engine would, returning
    the SAME normalized structure produced for a real `_array_diff` (see
    `extract_changes`). Falls back to a `count_from`/`count_to`-only result
    when the array holds scalars (routed to a set-diff) or an unrecognized
    struct shape with no stable per-element key.
    """
    ref_names = ref_names or {}
    count_from = len(from_list) if isinstance(from_list, list) else 0
    count_to = len(to_list) if isinstance(to_list, list) else 0

    def _empty():
        return {
            "strategy": "positional",
            "key_fields": None,
            "count_from": count_from,
            "count_to": count_to,
            "added": [],
            "removed": [],
            "changed": [],
        }

    if not isinstance(from_list, list) or not isinstance(to_list, list):
        return _empty()

    sample = next((x for x in (from_list + to_list) if isinstance(x, dict)), None)
    if sample is None:
        return _normalize_scalar_array(from_list, to_list, ref_names)

    detectors = [
        (lambda s: "Objective" in s, diff_objectives),
        (lambda s: "Stage" in s, diff_stages),
        (lambda s: "Function Type" in s and "Property" in s, diff_omod_properties),
        (lambda s: "Component" in s or "Quantity" in s, diff_components),
        (lambda s: "Leveled List Entry" in s, diff_lvli_entries),
        (lambda s: "Effect" in s, diff_effects),
    ]
    for pred, differ in detectors:
        if pred(sample):
            return differ(from_list, to_list, ref_names)

    return _empty()


# --------------------------------------------------------------------------
# _array_diff (new Rust shape) normalization
# --------------------------------------------------------------------------


def _struct_display(elem, ref_names):
    """Best-effort one-line summary of a dict array element (used for the
    generic `_array_diff` added/removed entries, which carry no shape-
    specific renderer the way the legacy differs above do)."""
    if "formid" in elem and ("editor_id" in elem or "record_type" in elem):
        return annotate_ref(elem, ref_names)
    if is_curve(elem):
        return annotate_ref(elem.get("formid"), ref_names)
    name = elem.get("name") or elem.get("Name")
    if name:
        return f"`{name}`"
    keys = list(elem.keys())[:4]
    parts = [f"{k}={format_scalar(elem[k], ref_names)}" for k in keys if not isinstance(elem[k], (dict, list))]
    if parts:
        return ", ".join(parts)
    return f"`(struct: {', '.join(keys)})`"


def _elem_display(elem, ref_names):
    if isinstance(elem, dict):
        return _struct_display(elem, ref_names)
    return format_scalar(elem, ref_names)


def _array_key_display(elem, key_fields, ref_names):
    if isinstance(elem, dict) and key_fields:
        parts = [f"{kf}={format_scalar(elem[kf], ref_names)}" for kf in key_fields if kf in elem]
        if parts:
            return ", ".join(parts)
    return _elem_display(elem, ref_names)


def _normalize_new_array_diff(ad, ref_names):
    key_fields = ad.get("key_fields")
    added = [
        {
            "key_display": _array_key_display(elem, key_fields, ref_names),
            "display": _elem_display(elem, ref_names),
            "raw": elem,
        }
        for elem in (ad.get("added") or [])
    ]
    removed = [
        {
            "key_display": _array_key_display(elem, key_fields, ref_names),
            "display": _elem_display(elem, ref_names),
            "raw": elem,
        }
        for elem in (ad.get("removed") or [])
    ]
    changed = []
    for c in ad.get("changed") or []:
        changed.append(
            {
                "key_display": _key_dict_display(c.get("key", {}), ref_names),
                "changes": extract_changes(c.get("changes", {}) or {}, ref_names),
            }
        )
    return {
        "strategy": ad.get("strategy", "positional"),
        "key_fields": key_fields,
        "count_from": ad.get("count_from"),
        "count_to": ad.get("count_to"),
        "added": added,
        "removed": removed,
        "changed": changed,
    }


# --------------------------------------------------------------------------
# ChangeEntry construction (kind detection)
# --------------------------------------------------------------------------


def _blank_entry(path, kind="scalar"):
    top_key = path.split(" / ", 1)[0]
    return {
        "path": path,
        "kind": kind,
        "from": None,
        "to": None,
        "from_display": None,
        "to_display": None,
        "suppressed": "noise" if top_key in NOISE_TOP_KEYS else None,
        "common_group": None,
        "array": None,
        "vmad": None,
    }


def _either_matches(fv, tv, pred):
    """True if both sides either match `pred` or are None, and at least one
    side actually matches — tolerates a field appearing/disappearing (one
    side None) while still classifying by the side that IS shaped."""
    fv_ok = fv is None or pred(fv)
    tv_ok = tv is None or pred(tv)
    return fv_ok and tv_ok and (pred(fv) or pred(tv))


def _looks_like_formid(v):
    if is_formid_str(v):
        return True
    if isinstance(v, dict):
        if is_curve(v):
            return True
        if "formid" in v and ("editor_id" in v or "record_type" in v):
            return True
    return False


def _is_formid_pair(fv, tv):
    return _either_matches(fv, tv, _looks_like_formid)


def _looks_like_enum(v):
    return isinstance(v, dict) and "value" in v and "name" in v and "flags" not in v


def _is_enum_pair(fv, tv):
    return _either_matches(fv, tv, _looks_like_enum)


def _looks_like_flags(v):
    return isinstance(v, dict) and isinstance(v.get("flags"), list)


def _is_flags_pair(fv, tv):
    return _either_matches(fv, tv, _looks_like_flags)


def _looks_unresolved(v):
    return isinstance(v, dict) and v.get("_unresolved") and "lstring_id" in v


def _is_unresolved_pair(fv, tv):
    return _either_matches(fv, tv, _looks_unresolved)


def _looks_like_raw_dict(v):
    return isinstance(v, dict) and bool(v.get("_raw"))


def _looks_like_raw_list(v):
    return isinstance(v, list) and len(v) > 0 and all(_looks_like_raw_dict(x) for x in v)


def _is_raw_pair(fv, tv):
    return _looks_like_raw_dict(fv) or _looks_like_raw_list(fv) or _looks_like_raw_dict(tv) or _looks_like_raw_list(tv)


def _raw_display(v):
    if isinstance(v, list):
        return f"`[raw hex ×{len(v)}]`" if v else "`[]`"
    if _looks_like_raw_dict(v):
        return "`[raw hex]`"
    return format_scalar(v)


def _looks_like_hex(s):
    return len(s) > 0 and len(s) % 2 == 0 and all(c in "0123456789abcdefABCDEF" for c in s)


def _is_vmad_hex_pair(path, fv, tv):
    return (
        "Virtual Machine Adapter" in path
        and "hex" in path.lower()
        and isinstance(fv, str)
        and isinstance(tv, str)
        and len(fv) > 40
        and len(tv) > 40
        and _looks_like_hex(fv)
        and _looks_like_hex(tv)
    )


def _enum_display(v):
    if isinstance(v, dict) and "name" in v:
        return f"`{v['name']}`"
    return format_scalar(v)


def _flags_names(v):
    if isinstance(v, dict) and isinstance(v.get("flags"), list):
        return list(v["flags"])
    return []


def _flags_display(fv, tv):
    f_names, t_names = _flags_names(fv), _flags_names(tv)
    added = [n for n in t_names if n not in f_names]
    removed = [n for n in f_names if n not in t_names]
    from_display = f"`{', '.join(f_names) or '(none)'}`"
    to_display = f"`{', '.join(t_names) or '(none)'}`"
    notes = []
    if added:
        notes.append(f"+{', '.join(added)}")
    if removed:
        notes.append(f"-{', '.join(removed)}")
    if notes:
        to_display += f" *({'; '.join(notes)})*"
    return from_display, to_display


def _make_array_diff_entry(path, ad, ref_names):
    entry = _blank_entry(path, "array")
    entry["to"] = ad
    entry["array"] = _normalize_new_array_diff(ad, ref_names)
    return entry


def _make_leaf_entry(path, fv, tv, ref_names):
    entry = _blank_entry(path)
    entry["from"], entry["to"] = fv, tv

    if _is_vmad_hex_pair(path, fv, tv):
        entry["kind"] = "vmad"
        entry["vmad"] = diff_vmad(fv, tv)
        entry["from_display"] = f"`[VMAD hex, {len(fv)} chars]`"
        entry["to_display"] = f"`[VMAD hex, {len(tv)} chars]`"
        return entry

    if _is_raw_pair(fv, tv):
        entry["kind"] = "raw"
        if entry["suppressed"] is None:
            entry["suppressed"] = "raw"
        entry["from_display"] = _raw_display(fv)
        entry["to_display"] = _raw_display(tv)
        return entry

    if isinstance(fv, list) or isinstance(tv, list):
        entry["kind"] = "array"
        entry["array"] = smart_array_diff(
            fv if isinstance(fv, list) else [],
            tv if isinstance(tv, list) else [],
            ref_names,
        )
        entry["from_display"] = f"`{len(fv)} items`" if isinstance(fv, list) else "`0 items`"
        entry["to_display"] = f"`{len(tv)} items`" if isinstance(tv, list) else "`0 items`"
        return entry

    if _is_enum_pair(fv, tv):
        entry["kind"] = "enum"
        entry["from_display"] = _enum_display(fv)
        entry["to_display"] = _enum_display(tv)
        return entry

    if _is_flags_pair(fv, tv):
        entry["kind"] = "flags"
        entry["from_display"], entry["to_display"] = _flags_display(fv, tv)
        return entry

    if _is_formid_pair(fv, tv):
        entry["kind"] = "formid"
        entry["from_display"] = format_scalar(fv, ref_names)
        entry["to_display"] = format_scalar(tv, ref_names)
        return entry

    if _is_unresolved_pair(fv, tv):
        entry["kind"] = "string"
        entry["from_display"] = format_scalar(fv, ref_names)
        entry["to_display"] = format_scalar(tv, ref_names)
        return entry

    entry["kind"] = "string" if isinstance(fv, str) or isinstance(tv, str) else "scalar"
    entry["from_display"] = format_scalar(fv, ref_names)
    entry["to_display"] = format_scalar(tv, ref_names)
    return entry


def _walk_changes(node, path, ref_names, out):
    if not isinstance(node, dict):
        return
    for key, val in node.items():
        cur_path = f"{path} / {key}" if path else key
        if not isinstance(val, dict):
            continue
        if "_array_diff" in val:
            out.append(_make_array_diff_entry(cur_path, val["_array_diff"], ref_names))
            continue
        if "from" in val and "to" in val:
            out.append(_make_leaf_entry(cur_path, val["from"], val["to"], ref_names))
            continue
        _walk_changes(val, cur_path, ref_names, out)


def extract_changes(field_changes, ref_names=None):
    """
    Walk a `field_changes` sparse diff tree (as produced by `esm diff --json`,
    or a nested sub-diff such as an `_array_diff.changed[].changes`) and
    return a flat list of ChangeEntry dicts:

        {"path": "Data / Damage", "kind": "scalar|string|enum|flags|formid|
                                            array|vmad|raw",
         "from": <raw json>, "to": <raw json>,
         "from_display": "`10`", "to_display": "`14`",
         "suppressed": None | "redundant_count" | "noise" | "raw",
         "common_group": None,
         "array": {...} | None,   # kind == "array"
         "vmad": {...} | None}    # kind == "vmad"

    Every leaf change becomes exactly one ChangeEntry — suppressed entries
    stay in the list (flagged), never dropped, so the result is exhaustive.
    """
    ref_names = ref_names or {}
    changes = []
    _walk_changes(field_changes or {}, "", ref_names, changes)
    return changes


# --------------------------------------------------------------------------
# Redundant-count suppression
# --------------------------------------------------------------------------


def _is_redundant_count_field(path, from_val, to_val, array_len_pairs):
    """True if this scalar entry is a '... Count' field whose (from, to)
    exactly mirrors an array-length change already reported elsewhere in the
    same record (Bethesda's format often stores an explicit count alongside
    the array it counts) — safe to drop as a duplicate of that array's
    delta."""
    last = path.split(" / ")[-1].lower().replace(" ", "")
    if "count" not in last:
        return False
    if isinstance(from_val, bool) or isinstance(to_val, bool):
        return False
    if not isinstance(from_val, (int, float)) or not isinstance(to_val, (int, float)):
        return False
    return (from_val, to_val) in array_len_pairs


def mark_redundant_counts(changes):
    """
    Given the flat ChangeEntry list for ONE record (as returned by
    extract_changes), find every array-kind entry's (count_from, count_to)
    and mark any scalar "... Count" entry whose (from, to) mirrors one of
    them as suppressed="redundant_count". Mutates `changes` in place;
    returns None.
    """
    array_len_pairs = set()
    for c in changes:
        if c["kind"] == "array" and c.get("array"):
            cf, ct = c["array"].get("count_from"), c["array"].get("count_to")
            if cf is not None and ct is not None:
                array_len_pairs.add((cf, ct))

    for c in changes:
        if c["suppressed"] is not None or c["kind"] != "scalar":
            continue
        if _is_redundant_count_field(c["path"], c["from"], c["to"], array_len_pairs):
            c["suppressed"] = "redundant_count"


# --------------------------------------------------------------------------
# Common-change collapsing
# --------------------------------------------------------------------------


def _hashable(v):
    try:
        return json.dumps(v, sort_keys=True)
    except TypeError:
        return repr(v)


_COMMON_KINDS = {"scalar", "string", "enum", "formid", "flags"}


def compute_common_changes(records, threshold=DEFAULT_COMMON_THRESHOLD):
    """
    `records`: dict[form_id_str, record] where each `record` is
        {"status": "added"|"removed"|"changed", "record_type": str,
         "changes": list[ChangeEntry], ...}
    (the "changes" list is what extract_changes() returns for that record's
    field_changes; only records with status "changed" are considered).

    Groups identical (record_type, path, from, to) scalar-ish deltas that
    recur across >= threshold records of the same type, collapsing them into
    one CommonChange dict and tagging each member ChangeEntry's
    "common_group" with the resulting id ("CC001", "CC002", ... assigned in
    a deterministic — record_type/path/value — sort order).

    Returns [{"id", "record_type", "path", "from", "to", "from_display",
              "to_display", "member_form_ids": [...]}], empty if none clear
    the threshold. Mutates the ChangeEntry dicts in `records` in place.
    """
    groups = defaultdict(list)
    for form_id, rec in records.items():
        if rec.get("status") != "changed":
            continue
        rtype = rec.get("record_type")
        for entry in rec.get("changes", []):
            if entry.get("suppressed") or entry.get("kind") not in _COMMON_KINDS:
                continue
            key = (rtype, entry["path"], _hashable(entry["from"]), _hashable(entry["to"]))
            groups[key].append((form_id, entry))

    common = []
    qualifying_keys = [key for key in sorted(groups.keys()) if len(groups[key]) >= threshold]
    for idx, key in enumerate(qualifying_keys, start=1):
        members = groups[key]
        rtype, path, _fk, _tk = key
        _, sample_entry = members[0]
        cc_id = f"CC{idx:03d}"
        common.append(
            {
                "id": cc_id,
                "record_type": rtype,
                "path": path,
                "from": sample_entry["from"],
                "to": sample_entry["to"],
                "from_display": sample_entry["from_display"],
                "to_display": sample_entry["to_display"],
                "member_form_ids": [fid for fid, _ in members],
            }
        )
        for _, entry in members:
            entry["common_group"] = cc_id
    return common


# --------------------------------------------------------------------------
# FormID reference harvesting
# --------------------------------------------------------------------------


def _is_formid_stub_dict(d):
    return isinstance(d, dict) and "formid" in d and (
        "editor_id" in d or "record_type" in d or "curve" in d or "curve_path" in d
    )


def _is_diff_leaf(d):
    return isinstance(d, dict) and "from" in d and "to" in d


def _emit_ref(fid, path, seen, out):
    key = (fid, path)
    if key not in seen:
        seen.add(key)
        out.append({"formid": fid, "path": path})


def _walk_refs(value, path, seen, out):
    if isinstance(value, str):
        if is_formid_str(value):
            _emit_ref(value, path, seen, out)
        return
    if isinstance(value, list):
        for item in value:
            _walk_refs(item, path, seen, out)
        return
    if not isinstance(value, dict):
        return

    if _is_formid_stub_dict(value):
        fid = value.get("formid")
        if isinstance(fid, str):
            _emit_ref(fid, path, seen, out)
        return
    if value.get("_unresolved") or value.get("_raw"):
        return
    if "_array_diff" in value:
        ad = value["_array_diff"]
        for item in (ad.get("added") or []) + (ad.get("removed") or []):
            _walk_refs(item, path, seen, out)
        for ch in ad.get("changed") or []:
            _walk_refs(ch.get("changes", {}), path, seen, out)
        return
    if _is_diff_leaf(value):
        _walk_refs(value.get("from"), path, seen, out)
        _walk_refs(value.get("to"), path, seen, out)
        return
    # Already-extracted ChangeEntry dict.
    if {"path", "kind", "from", "to"} <= value.keys():
        entry_path = value.get("path") or path
        _walk_refs(value.get("from"), entry_path, seen, out)
        _walk_refs(value.get("to"), entry_path, seen, out)
        arr = value.get("array")
        if arr:
            for item in (arr.get("added") or []) + (arr.get("removed") or []):
                _walk_refs(item.get("raw"), entry_path, seen, out)
            for ch in arr.get("changed") or []:
                for nested in ch.get("changes") or []:
                    _walk_refs(nested, entry_path, seen, out)
        return

    for k, v in value.items():
        child_path = f"{path} / {k}" if path else k
        _walk_refs(v, child_path, seen, out)


def collect_refs_out(fields_or_changes):
    """
    Recursively harvest every FormID-shaped reference from `fields_or_changes`
    — a raw decoded record's `fields` tree (added/removed RecordStub), a
    `field_changes` sparse-diff tree (changed record), or an already-built
    list[ChangeEntry] — together with its " / "-joined path. Returns a
    deduped list of `{"formid": "0x...", "path": "..."}` dicts, in
    first-seen order.
    """
    seen = set()
    out = []
    _walk_refs(fields_or_changes, "", seen, out)
    return out


# --------------------------------------------------------------------------
# Manifest helpers
# --------------------------------------------------------------------------


def load_manifest(out_dir):
    """Load `<out_dir>/manifest.json`, or None if it doesn't exist yet."""
    path = Path(out_dir) / "manifest.json"
    if not path.exists():
        return None
    with path.open(encoding="utf-8") as f:
        return json.load(f)


def write_manifest(out_dir, manifest):
    """Write `manifest` to `<out_dir>/manifest.json` (pretty-printed),
    creating `out_dir` if needed."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / "manifest.json"
    with path.open("w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")


def new_manifest(patch_date, old_token, new_token, new_esm_size, new_esm_mtime, pipeline_version, counts=None):
    """
    Build a fresh manifest dict for the mechanical stage to write:
        {"schema_version": 1, "patch_date": ..., "inputs": {...},
         "counts": {...},
         "stages": {"mechanical": {"completed_at": None, "files": {}},
                    "narrative": {"completed_at": None,
                                  "max_chunk_chars": 2000, "categories": []}}}
    """
    return {
        "schema_version": SCHEMA_VERSION,
        "patch_date": patch_date,
        "inputs": {
            "old_token": old_token,
            "new_token": new_token,
            "new_esm_size": new_esm_size,
            "new_esm_mtime": new_esm_mtime,
            "pipeline_version": pipeline_version,
        },
        "counts": counts or {},
        "stages": {
            "mechanical": {
                "completed_at": None,
                "files": {},
            },
            "narrative": {
                "completed_at": None,
                "max_chunk_chars": 2000,
                "categories": [],
            },
        },
    }
