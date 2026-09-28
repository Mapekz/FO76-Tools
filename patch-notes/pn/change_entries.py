#!/usr/bin/env python3
"""
change_entries.py — the single owner of patch-notes `ChangeEntry` construction,
consumed only by `render_comprehensive.py` (Tool 1 of the pipeline), in a
sibling module so that CLI entry point stays thin.

It owns cut/deprecation detection (`classify_cut`/`annotate_cut`),
`_array_diff` normalization (`_normalize_new_array_diff`, `presence_array_diff`,
and the `_looks_like_*`/`_is_*_pair` FormID/enum/flags/raw classifiers),
`ChangeEntry` construction (`extract_changes`, `_walk_changes`,
`_make_leaf_entry`, `_make_array_diff_entry`), redundant-count suppression
(`mark_redundant_counts`), common-change collapsing (`compute_common_changes`),
and FormID reference harvesting (`collect_refs_out`), plus the constants scoped
to them (`TYPE_DESC`, `EXCLUDED_TYPES`, `CUT_MARKERS`, `SUPPRESSED_REASONS`,
`DEFAULT_COMMON_THRESHOLD`).

It reaches into `patchnotes_lib` (`import patchnotes_lib as pl`) only for the
formatting helpers shared across the pipeline (`pl.format_scalar`,
`pl.annotate_ref`, `pl.is_curve`, `pl.is_formid_str`, `pl.fmt_num`);
`patchnotes_lib.py` never imports this module.

Input is the `esm diff --json` output (`DiffResult` in `src/diff/`): each
changed record's sparse `field_changes` map. Rust owns element identity
(ADR 0005), so every array edit arrives as an `_array_diff`; a `{"from",
"to"}` leaf holding a list means the array field appeared or disappeared.
`extract_changes` turns one record's `field_changes` into the flat
`ChangeEntry` list `render_comprehensive.py` walks.

Python 3, stdlib only.
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import patchnotes_lib as pl  # noqa: E402

# --------------------------------------------------------------------------
# Constants scoped to this engine
# --------------------------------------------------------------------------

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

# Top-level diff noise (GLOBAL_NOISE_FIELDS, PLACEMENT_NOISE_FIELDS,
# CELL_NOISE_FIELDS in esm/src/diff.rs) is stripped before this tooling
# runs.  --keep-noise surfaces those fields; this layer does not re-suppress.

# The full set of values `ChangeEntry["suppressed"]` may take (besides None).
# "noise" is retained for wire-schema compatibility (run_lints skips it;
# fixtures/historical JSON may still carry it) but extract_changes no longer
# assigns it.
SUPPRESSED_REASONS = {"redundant_count", "noise", "raw"}

# Minimum number of "changed" records of the same record_type sharing an
# identical (path, from, to) scalar delta before compute_common_changes()
# collapses them into one CommonChange bullet.
DEFAULT_COMMON_THRESHOLD = 5


# --------------------------------------------------------------------------
# Cut / deprecation detection
# --------------------------------------------------------------------------


def _marker_token(edid, markers):
    """
    Return (marker, confidence) if edid has a deprecation prefix, else None.
    Confidence: 'high' = MARKER_ or marker_ prefix exactly delimited;
                'medium' = bare prefix (either case: MARKER or marker) before
                CamelCase or digit -- e.g. "zzzLegendaryWeaponPerk" (lower-
                case "zzz" immediately followed by an uppercase letter, no
                underscore) matches exactly like "ZZZLegendaryWeaponPerk"
                would;
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
        # Medium: bare prefix (either case) before CamelCase or digit -- e.g.
        # "zzzLegendaryPerk" (lowercase "zzz" prefix, no underscore) is just
        # as much a deprecation marker as "ZZZLegendaryPerk" would be.
        for candidate in (m, m.lower()):
            if edid.startswith(candidate) and len(edid) > len(candidate):
                ch = edid[len(candidate)]
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


def _key_dict_display(key, ref_names):
    if not isinstance(key, dict) or not key:
        return pl.format_scalar(key, ref_names)
    return ", ".join(f"{k}={pl.format_scalar(v, ref_names)}" for k, v in key.items())


def presence_array_diff(from_list, to_list, ref_names=None):
    """Normalize a `{"from", "to"}` leaf that holds a list. Rust reports
    every array edit through `_array_diff` (ADR 0005) and uses this leaf
    shape only when the array field itself appears or disappears (one side
    is null), so each element on the present side is added or removed."""
    return _normalize_new_array_diff(
        {
            "strategy": "unkeyed",
            "count_from": len(from_list),
            "count_to": len(to_list),
            "removed": from_list,
            "added": to_list,
        },
        ref_names or {},
    )


# --------------------------------------------------------------------------
# _array_diff (new Rust shape) normalization
# --------------------------------------------------------------------------


_STRUCT_DISPLAY_MAX_FIELDS = 6
_STRUCT_DISPLAY_UNWRAP_MAX_DEPTH = 2


def _unwrap_element_wrapper(elem, max_depth=_STRUCT_DISPLAY_UNWRAP_MAX_DEPTH):
    """Unwrap the single-member "rstruct" wrapper rarray elements are often
    decoded into (e.g. a CTDA `Conditions[]` element,
    `{"Condition": {"Condition Data": {...}}}`) down to the first object
    carrying real fields, mirroring `diff.rs::unwrap_wrapper`. Stops at the
    first object with more than one key, a non-dict value, or after
    `max_depth` unwraps — whichever comes first. Elements that aren't
    wrapper-shaped (e.g. an already-flat OMOD property, or a resolved
    FormID stub with sibling `editor_id`/`record_type` keys) are returned
    unchanged, since `len(elem) == 1` never matches them."""
    depth = 0
    while depth < max_depth and isinstance(elem, dict) and len(elem) == 1:
        ((_, inner),) = elem.items()
        if not isinstance(inner, dict):
            break
        elem = inner
        depth += 1
    return elem


def _is_flat_renderable_dict(v):
    """True when `v` (a dict-valued member) already has a dedicated one-line
    rendering in `format_scalar` — a resolved FormID stub, a curve, a name,
    a flags list, or a raw/unresolved marker — rather than falling through
    to its `` `(struct: ...)` `` fallback. Mirrors `format_scalar`'s dict
    branch exactly, since it decides which members `_struct_display` below
    flattens instead of handing to `format_scalar` as-is."""
    if v.get("_unresolved") and "lstring_id" in v:
        return True
    if v.get("_raw"):
        return True
    if isinstance(v.get("flags"), list):
        return True
    if pl.is_curve(v):
        return True
    if "formid" in v and ("editor_id" in v or "record_type" in v):
        return True
    return bool(v.get("name") or v.get("Name"))


def _struct_display(elem, ref_names):
    """Best-effort one-line summary of a dict array element (used for the
    `_array_diff` added/removed entries). Dict-valued
    fields (e.g. an OMOD Property's `{"value": .., "name": ..}` enum, or a
    resolved FormID stub) render through `format_scalar` — which already
    knows how to turn those into a name/annotated reference — rather than
    being dropped; only list values and null/None are skipped, since those
    don't have a useful one-line rendering here.

    A dict member with no such dedicated rendering is flattened one level
    into its own `field=value` parts instead of being handed to
    `format_scalar` (which would print the unhelpful
    `` field=`(struct: ...)` ``): a two-key unkeyed element like
    `{"Condition Data": {"Operator": .., "Function": ..}, "Parameter #1":
    ..}` (CTDA condition rows nested inside a RACE `Attacks` entry are the
    motivating case) renders `Operator=.., Function=.., Parameter #1=..`
    directly rather than `` Condition Data=`(struct: Operator, ...)` ``.
    Flattening stops at one level — a struct nested inside the flattened
    member still falls back to `format_scalar`'s own `(struct: ...)`."""
    elem = _unwrap_element_wrapper(elem)
    if "formid" in elem and ("editor_id" in elem or "record_type" in elem):
        return pl.annotate_ref(elem, ref_names)
    if pl.is_curve(elem):
        return pl.annotate_ref(elem.get("formid"), ref_names)
    name = elem.get("name") or elem.get("Name")
    if name:
        return f"`{name}`"
    parts = []
    for k, v in elem.items():
        if v is None or isinstance(v, list):
            continue
        if isinstance(v, dict) and not _is_flat_renderable_dict(v):
            for sk, sv in v.items():
                if sv is None or isinstance(sv, list):
                    continue
                parts.append(f"{sk}={pl.format_scalar(sv, ref_names)}")
                if len(parts) >= _STRUCT_DISPLAY_MAX_FIELDS:
                    break
            if len(parts) >= _STRUCT_DISPLAY_MAX_FIELDS:
                break
            continue
        parts.append(f"{k}={pl.format_scalar(v, ref_names)}")
        if len(parts) >= _STRUCT_DISPLAY_MAX_FIELDS:
            break
    if parts:
        return ", ".join(parts)
    return f"`(struct: {', '.join(list(elem.keys())[:4])})`"


def _elem_display(elem, ref_names):
    if isinstance(elem, dict):
        return _struct_display(elem, ref_names)
    return pl.format_scalar(elem, ref_names)


def _array_key_display(elem, key_fields, ref_names):
    # Rust keys a wrapped element (`{"Leveled List Entry": {...}}`) by its
    # inner fields, so look the key fields up there.
    inner = _unwrap_element_wrapper(elem)
    if isinstance(inner, dict) and key_fields:
        parts = [f"{kf}={pl.format_scalar(inner[kf], ref_names)}" for kf in key_fields if kf in inner]
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
    return {
        "path": path,
        "kind": kind,
        "from": None,
        "to": None,
        "from_display": None,
        "to_display": None,
        "suppressed": None,
        "common_group": None,
        "array": None,
    }


def _either_matches(fv, tv, pred):
    """True if both sides either match `pred` or are None, and at least one
    side actually matches — tolerates a field appearing/disappearing (one
    side None) while still classifying by the side that IS shaped."""
    fv_ok = fv is None or pred(fv)
    tv_ok = tv is None or pred(tv)
    return fv_ok and tv_ok and (pred(fv) or pred(tv))


def _looks_like_formid(v, ref_names):
    if pl.is_ref(v, ref_names):
        return True
    if isinstance(v, dict):
        if pl.is_curve(v):
            return True
        if "formid" in v and ("editor_id" in v or "record_type" in v):
            return True
    return False


def _is_formid_pair(fv, tv, ref_names):
    return _either_matches(fv, tv, lambda v: _looks_like_formid(v, ref_names))


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
    return pl.format_scalar(v)


def _enum_display(v):
    if isinstance(v, dict) and "name" in v:
        return f"`{v['name']}`"
    return pl.format_scalar(v)


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

    if _is_raw_pair(fv, tv):
        entry["kind"] = "raw"
        if entry["suppressed"] is None:
            entry["suppressed"] = "raw"
        entry["from_display"] = _raw_display(fv)
        entry["to_display"] = _raw_display(tv)
        return entry

    if isinstance(fv, list) or isinstance(tv, list):
        entry["kind"] = "array"
        entry["array"] = presence_array_diff(
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

    if _is_formid_pair(fv, tv, ref_names):
        entry["kind"] = "formid"
        entry["from_display"] = pl.format_scalar(fv, ref_names)
        entry["to_display"] = pl.format_scalar(tv, ref_names)
        return entry

    if _is_unresolved_pair(fv, tv):
        entry["kind"] = "string"
        entry["from_display"] = pl.format_scalar(fv, ref_names)
        entry["to_display"] = pl.format_scalar(tv, ref_names)
        return entry

    entry["kind"] = "string" if isinstance(fv, str) or isinstance(tv, str) else "scalar"
    entry["from_display"] = pl.format_scalar(fv, ref_names)
    entry["to_display"] = pl.format_scalar(tv, ref_names)
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
                                            array|raw",
         "from": <raw json>, "to": <raw json>,
         "from_display": "`10`", "to_display": "`14`",
         "suppressed": None | "redundant_count" | "noise" | "raw",
         "common_group": None,
         "array": {...} | None}   # kind == "array"

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


def _walk_refs(value, path, refs, seen, out):
    if isinstance(value, str):
        if value in refs:
            _emit_ref(value, path, seen, out)
        return
    if isinstance(value, list):
        for item in value:
            _walk_refs(item, path, refs, seen, out)
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
            _walk_refs(item, path, refs, seen, out)
        for ch in ad.get("changed") or []:
            _walk_refs(ch.get("changes", {}), path, refs, seen, out)
        return
    if _is_diff_leaf(value):
        _walk_refs(value.get("from"), path, refs, seen, out)
        _walk_refs(value.get("to"), path, refs, seen, out)
        return
    # Already-extracted ChangeEntry dict.
    if {"path", "kind", "from", "to"} <= value.keys():
        entry_path = value.get("path") or path
        _walk_refs(value.get("from"), entry_path, refs, seen, out)
        _walk_refs(value.get("to"), entry_path, refs, seen, out)
        arr = value.get("array")
        if arr:
            for item in (arr.get("added") or []) + (arr.get("removed") or []):
                _walk_refs(item.get("raw"), entry_path, refs, seen, out)
            for ch in arr.get("changed") or []:
                for nested in ch.get("changes") or []:
                    _walk_refs(nested, entry_path, refs, seen, out)
        return

    for k, v in value.items():
        child_path = f"{path} / {k}" if path else k
        _walk_refs(v, child_path, refs, seen, out)


def collect_refs_out(fields_or_changes, refs):
    """
    Recursively harvest the FormID references in `fields_or_changes` — a raw
    decoded record's `fields` tree (added/removed RecordStub), a
    `field_changes` sparse-diff tree (changed record), or an already-built
    list[ChangeEntry] — together with each one's " / "-joined path. A string
    counts only if it is in `refs`, the record's typed references from
    `esm diff` (its `refs` list); resolved stub dicts always count. Returns a
    deduped list of `{"formid": "0x...", "path": "..."}` dicts, in
    first-seen order.
    """
    seen = set()
    out = []
    _walk_refs(fields_or_changes, "", set(refs), seen, out)
    return out
