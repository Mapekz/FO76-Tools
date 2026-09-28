#!/usr/bin/env python3
"""
patchnotes_lib.py — helpers several pipeline stages share:

  - reference formatting (`annotate_ref`, `format_scalar`, `fmt_num`,
    `is_ref`, `is_curve`), used by rendering, lints and triage;
  - `attach_lints`, the bundles.json + lints.json join;
  - `UNIQUE_KEYWORD_PATTERNS`, read by bundling and the lints;
  - manifest.json read/write (`load_manifest`, `write_manifest`,
    `new_manifest`) and `esm_is_localized`.

Artifact shapes and their validators live in `schemas.py`.

Python 3, stdlib only.
"""

from __future__ import annotations

from collections import defaultdict
from pathlib import Path

from pn import jsonio, layout

# --------------------------------------------------------------------------
# Pipeline wire shapes (comprehensive.json / bundles.json / triage / lints)
# --------------------------------------------------------------------------

#: fnmatch patterns for the placeholder keywords that mark a "unique" item
#: (bundle context ranking and the `orphaned_unique` lint).
UNIQUE_KEYWORD_PATTERNS = ["if_tmp_*"]


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


def is_ref(v, ref_names):
    """True if `v` is a FormID string the diff reported as a typed reference:
    a `ref_names` key (see `with_dangling_refs` for the unresolvable ones)."""
    return isinstance(v, str) and v in (ref_names or {})


def with_dangling_refs(ref_names, dangling):
    """`ref_names` plus a `{"dangling": True}` entry for each FormID in
    `dangling` that has no name entry, so every typed reference -- resolved
    or not -- is a key."""
    out = dict(ref_names or {})
    for fid in dangling:
        out.setdefault(fid, {"dangling": True})
    return out


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
        if info.get("dangling"):
            return f"`{fid}` *(dangling)*"
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
        if is_ref(v, ref_names):
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
        return f"`(struct: {', '.join(str(k) for k in list(v.keys())[:4])})`"
    return f"`{repr(v)[:60]}`"


# --------------------------------------------------------------------------
# Runtime validation (JSON process seams)
# --------------------------------------------------------------------------


def attach_lints(bundles, lints):
    """Copies of `bundles`, each with `lint_ids` (the lints whose `form_id`
    is one of its members, anchor included) and `bug_watch` (any of those
    lints is an error or warn). bundles.json and lints.json are separate
    artifacts; stages that need both join them here."""
    bundle_ids_by_member = defaultdict(list)
    for b in bundles:
        fids = {m["form_id"] for m in b.get("members") or [] if isinstance(m, dict) and m.get("form_id")}
        if (b.get("anchor") or {}).get("form_id"):
            fids.add(b["anchor"]["form_id"])
        for fid in fids:
            bundle_ids_by_member[fid].append(b.get("id"))

    lint_ids = defaultdict(list)
    severities = defaultdict(set)
    for lint in lints:
        for bid in bundle_ids_by_member.get(lint.get("form_id"), ()):
            lint_ids[bid].append(lint["id"])
            severities[bid].add(lint.get("severity"))

    return [
        {
            **b,
            "lint_ids": lint_ids.get(b.get("id"), []),
            "bug_watch": any(s in ("error", "warn") for s in severities.get(b.get("id"), ())),
        }
        for b in bundles
    ]


# --------------------------------------------------------------------------
# Manifest helpers
# --------------------------------------------------------------------------


def load_manifest(out_dir):
    """Load `<out_dir>/manifest.json`, or None if it doesn't exist yet."""
    path = layout.manifest_json(out_dir)
    if not path.exists():
        return None
    return jsonio.read(path)


def write_manifest(out_dir, manifest):
    """Write `manifest` to `<out_dir>/manifest.json` (pretty-printed),
    creating `out_dir` if needed."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    path = layout.manifest_json(out_dir)
    jsonio.write(path, manifest)


def esm_is_localized(esm_path) -> bool | None:
    """The TES4 header's Localized flag (0x80), or None when the file doesn't
    start with a TES4 record. PTS builds keep text inline until content
    freezes, so the flag flips every few months."""
    with Path(esm_path).open("rb") as f:
        head = f.read(12)
    if len(head) < 12 or head[:4] != b"TES4":
        return None
    return bool(int.from_bytes(head[8:12], "little") & 0x80)


def new_manifest(patch_date, old_token, new_token, new_esm_size, new_esm_mtime, pipeline_version, counts=None,
                 localized=None, exclude_type="", old_esm_size=None, old_esm_mtime=None):
    """
    Build a fresh manifest dict for the mechanical stage to write:
        {"patch_date": ..., "inputs": {...}, "counts": {...},
         "stages": {"mechanical": {"completed_at": None, "files": {}},
                    "narrative": {"completed_at": None,
                                  "patch_summary_md": None,
                                  "discord_dir": "discord", "chunk_count": 0,
                                  "chunks": [], "max_chunk_chars": 2000,
                                  "triage": None, "usage": None}}}

    `stages.narrative` is a placeholder in the shape
    `update_manifest.build_narrative_stage` fills in once the narrative
    stage runs.

    `localized` is `{"old": ..., "new": ...}`, each side's `esm_is_localized`.
    """
    return {
        "patch_date": patch_date,
        "inputs": {
            "old_token": old_token,
            "new_token": new_token,
            "old_esm_size": old_esm_size,
            "old_esm_mtime": old_esm_mtime,
            "new_esm_size": new_esm_size,
            "new_esm_mtime": new_esm_mtime,
            "pipeline_version": pipeline_version,
            "localized": localized or {"old": None, "new": None},
            "exclude_type": exclude_type,
        },
        "counts": counts or {},
        "stages": {
            "mechanical": {
                "completed_at": None,
                "files": {},
            },
            "narrative": {
                "completed_at": None,
                "patch_summary_md": None,
                "discord_dir": layout.DISCORD_DIRNAME,
                "chunk_count": 0,
                "chunks": [],
                "max_chunk_chars": 2000,
                "triage": None,
                "usage": None,
            },
        },
    }
