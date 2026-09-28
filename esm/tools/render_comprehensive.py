#!/usr/bin/env python3
"""
render_comprehensive.py — Tool 1 of the FO76 patch-notes pipeline.

Consumes the raw `esm diff --json` output and writes comprehensive.json into
an output directory: an exhaustive, machine-readable diff with every
added/removed/changed record keyed by FormID, every ChangeEntry (including
suppressed ones, flagged), common-change groups, and a verbatim `ref_names`
passthrough. Downstream pipeline stages (bundle slicing, narrative
summarization) consume it — its shape is a stable contract, see
build_comprehensive().

Usage:
    python3 tools/render_comprehensive.py DIFF_JSON --out-dir DIR \\
        [--old-esm PATH] [--new-esm PATH] \\
        [--old-label LABEL] [--new-label LABEL] [--patch-date YYYY-MM-DD] \\
        [--common-threshold N]

Library entry points (for in-process orchestration, no subprocess needed):
    build_comprehensive(diff, **meta_args) -> dict   # the comprehensive.json shape

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

import change_entries
import layout
import patchnotes_lib as pl

# --------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def _cap(s, limit=200):
    """Cap a display string at ~200 chars with an ellipsis. MD is meant to
    be exhaustive (no truncation of change *lists*) — this is only a guard
    against a single pathologically long display string blowing up a line."""
    if not isinstance(s, str):
        return s
    return s if len(s) <= limit else s[: limit - 1] + "…"


def _iso_now():
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def _version_token(label):
    """Extract the 8-digit-or-longer date token from a filename-shaped
    label's stem, or return the stem itself. Mirrors
    tools/make_patch_notes.py's version_token(), adapted to operate on a
    plain string rather than requiring a Path to an existing ESM."""
    if not label:
        return ""
    stem = Path(label).stem
    m = re.search(r"\d{6,}", stem)
    return m.group(0) if m else stem


def _derive_patch_date(label):
    """Mirrors tools/make_patch_notes.py's derive_patch_date(): an 8-digit
    token becomes YYYY-MM-DD, otherwise the token (or label) is returned
    as-is."""
    tok = _version_token(label)
    if len(tok) == 8 and tok.isdigit():
        return f"{tok[:4]}-{tok[4:6]}-{tok[6:]}"
    return tok


def _label_for_esm(esm_path):
    """
    Default display label for an ESM path: its own filename, unless the
    filename's stem carries no digits at all — this pipeline's snapshot
    layout dates the *parent directory*, not the file itself (e.g.
    `$FO76_DATA_DIR/20260703/SeventySix.esm`, see CLAUDE.local.md), so the
    sibling directory name is the far more useful/distinguishing label in
    that case (plain "SeventySix.esm" would be identical for both sides).
    """
    p = Path(esm_path)
    if not re.search(r"\d{4,}", p.stem) and p.parent.name:
        return p.parent.name
    return p.name


def derive_labels_and_date(diff_json_path, old_esm, new_esm, old_label, new_label, patch_date):
    """
    Fill in old_label/new_label/patch_date when not explicitly supplied:
      - old_label defaults via _label_for_esm(--old-esm), else "old".
      - new_label defaults via _label_for_esm(--new-esm), else "new".
      - patch_date is derived from an 8-digit YYYYMMDD token found in (in
        order) new_label, --new-esm's parent dir name, old_label, --old-esm's
        parent dir name, then the diff JSON's own filename — new-side
        candidates take priority since "patch date" means the NEW release's
        date; falls back to "Unknown Date" if none carry a recognizable date.
    """
    if not old_label:
        old_label = _label_for_esm(old_esm) if old_esm else "old"
    if not new_label:
        new_label = _label_for_esm(new_esm) if new_esm else "new"
    if not patch_date:
        # New-side candidates first (patch_date means the NEW release's
        # date) — new_esm's own parent dir is checked before old_label, so
        # a custom --new-label override can't cause an old_label that
        # happens to auto-resolve to a full date to win by accident.
        candidates = [new_label]
        if new_esm:
            candidates.append(Path(new_esm).parent.name)
        candidates.append(old_label)
        if old_esm:
            candidates.append(Path(old_esm).parent.name)
        if diff_json_path:
            candidates.append(Path(diff_json_path).name)
        for candidate in candidates:
            if not candidate:
                continue
            date = _derive_patch_date(candidate)
            if re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
                patch_date = date
                break
        if not patch_date:
            patch_date = "Unknown Date"
    return old_label, new_label, patch_date


def _strip_flags_values(value):
    """
    Return a deep copy of `value` with the bitmask `value` key of any
    flags-shaped dict (`{"value": "0x00000005", "flags": [...]}`) removed.

    A 32-bit flags bitmask is formatted as an 8-hex-digit `0x...` string —
    byte-for-byte identical in shape to a genuine FormID — which would
    otherwise cause change_entries.collect_refs_out()'s generic dict walk to
    mis-harvest it as a dangling FormID reference (it isn't; it's just a
    bitmask). This is the one documented decoded-value shape where that
    false positive reliably occurs, so it's worth pre-filtering before
    handing a tree to collect_refs_out().
    """
    if isinstance(value, dict):
        if isinstance(value.get("flags"), list):
            return {"flags": value["flags"]}
        return {k: _strip_flags_values(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_strip_flags_values(v) for v in value]
    return value


def _collect_refs_out(tree):
    """collect_refs_out(), guarded against the flags-bitmask false positive
    described in _strip_flags_values()."""
    return change_entries.collect_refs_out(_strip_flags_values(tree))


def _merge_refs(*ref_lists):
    seen = set()
    out = []
    for refs in ref_lists:
        for r in refs:
            key = (r["formid"], r["path"])
            if key not in seen:
                seen.add(key)
                out.append(r)
    return out


def _refs_out_for_changes(changes):
    """
    Harvest FormID references from only the to-side (new/current state) of
    a changed record's ChangeEntry list — the record's outgoing references
    *after* this patch, not what it used to reference. Delegates the actual
    FormID-shape detection to change_entries.collect_refs_out() (wrapping
    each value in a single-key dict so the path prefix survives), so this
    stays in sync with the library's own ref-walking rules.

    - scalar/string/enum/flags/formid/vmad-hex kinds: walk `entry["to"]`.
    - array kind: walk `added` elements (new) and, for `changed` elements,
      recurse into their nested `changes` (also to-side only). `removed`
      elements are old-state and intentionally excluded.
    - vmad kind: walk `added` prop values and `changed` props' `to` value;
      `removed` props are old-state and intentionally excluded.
    """
    collected = []
    for entry in changes:
        path = entry["path"]
        kind = entry.get("kind")
        if kind == "array" and entry.get("array"):
            arr = entry["array"]
            for a in arr.get("added") or []:
                collected.append(_collect_refs_out({path: a.get("raw")}))
            for c in arr.get("changed") or []:
                collected.append(_refs_out_for_changes(c.get("changes") or []))
        elif kind == "vmad" and entry.get("vmad"):
            # NOTE: sorted() here isn't just cosmetic — decode_vmad_props()
            # dicts are built from a `set` union upstream (change_entries
            # diff_vmad), whose iteration order is affected by Python's
            # per-process hash randomization. Sorting keeps refs_out order
            # (and, via _render_change_bullet, the MD prop listing) stable
            # across runs/processes.
            vmad = entry["vmad"]
            for name, v in sorted((vmad.get("added") or {}).items()):
                collected.append(_collect_refs_out({f"{path} / {name}": v}))
            for name, ch in sorted((vmad.get("changed") or {}).items()):
                collected.append(_collect_refs_out({f"{path} / {name}": ch.get("to")}))
        else:
            collected.append(_collect_refs_out({path: entry.get("to")}))
    return _merge_refs(*collected)


# --------------------------------------------------------------------------
# build_comprehensive: raw diff JSON -> comprehensive.json shape
# --------------------------------------------------------------------------


def _record_entry(form_id, record_type, editor_id, name, description, status,
                   prev_editor_id, cut, fields, refs_out, changes) -> pl.RecordEntry:
    return {
        "form_id": form_id,
        "record_type": record_type,
        "editor_id": editor_id,
        "name": name,
        "description": description,
        "status": status,
        "prev_editor_id": prev_editor_id,
        "cut": cut,
        "fields": fields,
        "refs_out": refs_out,
        "changes": changes,
    }


def build_comprehensive(
    diff,
    *,
    old_esm=None,
    new_esm=None,
    old_label=None,
    new_label=None,
    patch_date=None,
    common_threshold=change_entries.DEFAULT_COMMON_THRESHOLD,
    generated_at=None,
):
    """
    Normalize a raw `esm diff --json` dict into the comprehensive.json shape
    (see module docstring / CLAUDE-facing contract). Records whose
    `record_type` is in change_entries.EXCLUDED_TYPES (CELL/WRLD) are
    dropped from the returned `records` dict but tallied in
    `meta.counts_excluded`. Mutates nothing on `diff` itself.
    """
    ref_names = diff.get("ref_names") or {}
    records = {}
    counts = {"added": 0, "removed": 0, "changed": 0}
    counts_excluded = defaultdict(int)

    for stub in diff.get("added") or []:
        rtype = stub.get("record_type")
        if rtype in change_entries.EXCLUDED_TYPES:
            counts_excluded[rtype] += 1
            continue
        fid = stub.get("form_id")
        fields = stub.get("fields")
        records[fid] = _record_entry(
            fid, rtype, stub.get("editor_id"), stub.get("name"), stub.get("description"),
            "added", None, change_entries.annotate_cut(stub), fields,
            _collect_refs_out(fields) if fields is not None else [],
            [],
        )
        counts["added"] += 1

    for stub in diff.get("removed") or []:
        rtype = stub.get("record_type")
        if rtype in change_entries.EXCLUDED_TYPES:
            counts_excluded[rtype] += 1
            continue
        fid = stub.get("form_id")
        fields = stub.get("fields")
        records[fid] = _record_entry(
            fid, rtype, stub.get("editor_id"), stub.get("name"), stub.get("description"),
            "removed", None, change_entries.annotate_cut(stub), fields,
            _collect_refs_out(fields) if fields is not None else [],
            [],
        )
        counts["removed"] += 1

    for rec in diff.get("changed") or []:
        stub = rec.get("stub") or {}
        rtype = stub.get("record_type")
        if rtype in change_entries.EXCLUDED_TYPES:
            counts_excluded[rtype] += 1
            continue
        fid = stub.get("form_id")
        field_changes = rec.get("field_changes") or {}
        changes = change_entries.extract_changes(field_changes, ref_names)
        change_entries.mark_redundant_counts(changes)
        fields = stub.get("fields")
        refs_out = _refs_out_for_changes(changes)
        if fields is not None:
            refs_out = _merge_refs(refs_out, _collect_refs_out(fields))
        records[fid] = _record_entry(
            fid, rtype, stub.get("editor_id"), stub.get("name"), stub.get("description"),
            "changed", rec.get("prev_editor_id"), change_entries.annotate_cut(rec), fields,
            refs_out, changes,
        )
        counts["changed"] += 1

    # compute_common_changes reads only "status"/"record_type"/"changes" off
    # each record and mutates the ChangeEntry dicts in place (tagging
    # "common_group") — the same dicts held in `records`, so the JSON output
    # picks up the tag too.
    common_changes = change_entries.compute_common_changes(records, threshold=common_threshold)

    meta = {
        "old_esm": str(Path(old_esm).resolve()) if old_esm else "",
        "new_esm": str(Path(new_esm).resolve()) if new_esm else "",
        "old_label": old_label or "",
        "new_label": new_label or "",
        "patch_date": patch_date or "",
        "generated_at": generated_at or _iso_now(),
        "excluded_types": sorted(change_entries.EXCLUDED_TYPES),
        "counts_excluded": dict(counts_excluded),
        "suppressed_counts": diff.get("suppressed_counts") or {},
        "counts": counts,
    }

    return {
        "schema_version": pl.SCHEMA_VERSION,
        "meta": meta,
        "records": records,
        "common_changes": common_changes,
        "ref_names": ref_names,
    }


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="render_comprehensive.py",
        description="Render an `esm diff --json` file into comprehensive.json.",
    )
    ap.add_argument("diff_json", help="Path to the raw `esm diff --json` output file.")
    ap.add_argument("--out-dir", required=True, help="Directory to write comprehensive.json into.")
    ap.add_argument("--old-esm", help="Absolute (or resolvable) path to the OLD .esm, for meta.old_esm.")
    ap.add_argument("--new-esm", help="Absolute (or resolvable) path to the NEW .esm, for meta.new_esm.")
    ap.add_argument("--old-label", help="Display label for the old side (default: basename of --old-esm).")
    ap.add_argument("--new-label", help="Display label for the new side (default: basename of --new-esm).")
    ap.add_argument("--patch-date", help="Patch date YYYY-MM-DD (default: derived from filenames).")
    ap.add_argument(
        "--common-threshold", type=int, default=change_entries.DEFAULT_COMMON_THRESHOLD,
        help=f"Min number of changed records sharing a delta before it collapses into a "
             f"Common Change (default: {change_entries.DEFAULT_COMMON_THRESHOLD}).",
    )
    return ap


def main(argv=None):
    args = build_arg_parser().parse_args(argv)

    diff_path = Path(args.diff_json)
    try:
        with diff_path.open(encoding="utf-8") as f:
            diff = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        eprint(f"error: failed to load {diff_path}: {e}")
        return 1

    old_label, new_label, patch_date = derive_labels_and_date(
        str(diff_path), args.old_esm, args.new_esm, args.old_label, args.new_label, args.patch_date
    )

    comp = build_comprehensive(
        diff,
        old_esm=args.old_esm,
        new_esm=args.new_esm,
        old_label=old_label,
        new_label=new_label,
        patch_date=patch_date,
        common_threshold=args.common_threshold,
    )
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    json_path = layout.comprehensive_json(out_dir)

    with json_path.open("w", encoding="utf-8") as f:
        json.dump(comp, f, indent=2, ensure_ascii=False)
        f.write("\n")

    counts = comp["meta"]["counts"]
    eprint(
        f"wrote {json_path} "
        f"({counts['added']} added, {counts['changed']} changed, {counts['removed']} removed)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
