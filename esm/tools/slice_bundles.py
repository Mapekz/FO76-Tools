#!/usr/bin/env python3
"""
Bundle-slicing helper for the FO76 patch-notes pipeline.

The patch-notes pipeline writes two files to an output directory:
`bundles.json` (small, per-bundle summaries grouped by category) and
`comprehensive.json` (large, full per-record detail keyed by FormID). A
Claude skill fans out one writer subagent per category; each writer must
receive ONLY its own category's slice of `bundles.json`, and fetch
per-record detail on demand rather than loading the whole (potentially huge)
`comprehensive.json` into its context.

This script has two modes:

    python3 tools/slice_bundles.py <out_dir>
        Reads `<out_dir>/bundles.json` and writes one slice per category
        (further split into byte/size-bounded parts if needed) plus a
        `work/categories.json` table of contents, under `<out_dir>/work/`.

    python3 tools/slice_bundles.py --extract <out_dir> <FORMID> [<FORMID> ...]
        Reads `<out_dir>/comprehensive.json` and prints a small JSON object
        with just the requested records (and any `ref_names` entries they
        reference) to stdout. Intended to be invoked as a short-lived
        subprocess by a writer subagent that needs one record's full detail.

Python 3, standard library only.
"""

import argparse
import json
import re
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import patchnotes_lib as pl  # noqa: E402

# --------------------------------------------------------------------------
# Tunables
# --------------------------------------------------------------------------

# Soft cap on a category slice's serialized size before it gets split into
# multiple `.partN.json` files. "Soft" because a single oversized bundle is
# still emitted whole (never split mid-bundle) even if that pushes a part
# over this cap.
MAX_SLICE_BYTES = 150_000

# Hard cap on bundle count per split part, independent of byte size.
MAX_BUNDLES_PER_PART = 25

# Above this comprehensive.json size, --extract still loads the whole file
# (plain json.load) but warns to stderr first, since this script has no
# streaming JSON parser available (stdlib only).
COMPREHENSIVE_WARN_BYTES = 200 * 1024 * 1024

# Cap on the number of ref_names entries returned by --extract.
MAX_REF_NAMES = 200


# --------------------------------------------------------------------------
# Mode 1: slicing bundles.json into per-category work/ files
# --------------------------------------------------------------------------

def load_bundles(out_dir):
    """Load and return the parsed `bundles.json` from out_dir."""
    path = Path(out_dir) / "bundles.json"
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def _lints_index(lints):
    """Map lint id -> lint dict, preserving insertion (declaration) order."""
    by_id = {}
    for lint in lints:
        lid = lint.get("id")
        if lid is not None:
            by_id[lid] = lint
    return by_id


def group_bundles_by_category(bundles):
    """
    Group bundle dicts by their `category`, preserving first-appearance
    order (the order categories are first seen scanning `bundles` top to
    bottom in bundles.json) — this becomes each category's `post_order`.

    Returns an ordered list of (category_id, category_label, [bundle, ...]).
    Only categories with at least one bundle can appear here (there is
    nothing upstream of this function that would produce an empty group),
    but callers should still not assume that invariant blindly.
    """
    order = []
    grouped = {}
    labels = {}
    for b in bundles:
        cat = b.get("category")
        if cat not in grouped:
            grouped[cat] = []
            order.append(cat)
            labels[cat] = b.get("category_label", cat)
        grouped[cat].append(b)
    return [(cat, labels[cat], grouped[cat]) for cat in order if grouped[cat]]


def lints_for_bundles(bundles_subset, lints_by_id):
    """
    Return the lints relevant to `bundles_subset`: any lint whose id is
    listed in one of these bundles' `lint_ids`, plus (defensively) any lint
    whose own `bundle_id` names one of these bundles even if that bundle's
    `lint_ids` omitted it. Order follows first reference; deduplicated.
    """
    bundle_ids = {b.get("id") for b in bundles_subset}
    result = []
    seen = set()

    for b in bundles_subset:
        for lid in b.get("lint_ids") or []:
            if lid in seen:
                continue
            lint = lints_by_id.get(lid)
            if lint is not None:
                result.append(lint)
                seen.add(lid)

    for lid, lint in lints_by_id.items():
        if lid in seen:
            continue
        if lint.get("bundle_id") in bundle_ids:
            result.append(lint)
            seen.add(lid)

    return result


def _json_bytes(obj):
    return len(json.dumps(obj, ensure_ascii=False).encode("utf-8"))


def build_slice_payload(category, label, bundles_subset, lints_subset):
    return {
        "schema_version": 1,
        "category": category,
        "category_label": label,
        "bundles": bundles_subset,
        "lints": lints_subset,
    }


def slice_category(category, label, bundles_for_cat, lints_by_id,
                    max_bytes=MAX_SLICE_BYTES, max_count=MAX_BUNDLES_PER_PART):
    """
    Decide whether `bundles_for_cat` fits in one slice or needs splitting.

    Returns a list of (bundles_subset, lints_subset) tuples: one entry if
    the whole category fits under `max_bytes` serialized, otherwise several
    — each with <= max_count bundles and (approximately) <= max_bytes,
    packed greedily in original order. A single bundle is never split
    across parts, even if it alone exceeds max_bytes.
    """
    full_lints = lints_for_bundles(bundles_for_cat, lints_by_id)
    full_payload = build_slice_payload(category, label, bundles_for_cat, full_lints)
    if _json_bytes(full_payload) <= max_bytes:
        return [(bundles_for_cat, full_lints)]

    envelope_bytes = _json_bytes(build_slice_payload(category, label, [], []))
    parts_bundles = []
    current = []
    current_bytes = envelope_bytes
    for b in bundles_for_cat:
        b_bytes = _json_bytes(b)
        if current and (len(current) >= max_count or current_bytes + b_bytes > max_bytes):
            parts_bundles.append(current)
            current = []
            current_bytes = envelope_bytes
        current.append(b)
        current_bytes += b_bytes
    if current:
        parts_bundles.append(current)

    return [(part, lints_for_bundles(part, lints_by_id)) for part in parts_bundles]


_SLUG_UNSAFE_RE = re.compile(r"[^A-Za-z0-9_-]+")


def slug_for_category(category_id):
    """Filesystem-safe slug for a category id (used verbatim unless it
    contains characters unsafe for a filename)."""
    return _SLUG_UNSAFE_RE.sub("_", str(category_id))


def owned_work_files(work_dir):
    """Files under work_dir this script owns and may freely wipe/recreate:
    all `bundles.*.json` slices plus `categories.json`. Everything else in
    work/ is left untouched."""
    if not work_dir.exists():
        return []
    files = sorted(work_dir.glob("bundles.*.json"))
    cat_path = work_dir / "categories.json"
    if cat_path.exists():
        files.append(cat_path)
    return files


def wipe_owned_work_files(work_dir):
    for p in owned_work_files(work_dir):
        p.unlink()


def print_summary(rows, stream=sys.stderr):
    """Print a (category, bundles, parts, KB) summary table."""
    header = f"{'category':<28} {'bundles':>8} {'parts':>6} {'KB':>10}"
    print(header, file=stream)
    print("-" * len(header), file=stream)
    for cat, n_bundles, n_parts, total_bytes in rows:
        print(f"{cat:<28} {n_bundles:>8} {n_parts:>6} {total_bytes / 1024:>10.1f}", file=stream)


def run_slice(out_dir):
    """
    Mode 1 entry point. Reads `<out_dir>/bundles.json`, writes
    `<out_dir>/work/categories.json` and one or more
    `<out_dir>/work/bundles.<slug>[.partN].json` per category.

    Returns the categories.json payload dict (also useful for tests).
    """
    out_dir = Path(out_dir)
    data = load_bundles(out_dir)
    bundles = data.get("bundles", [])
    lints_by_id = _lints_index(data.get("lints", []))

    work_dir = out_dir / "work"
    work_dir.mkdir(parents=True, exist_ok=True)
    wipe_owned_work_files(work_dir)

    grouped = group_bundles_by_category(bundles)

    categories = []
    summary_rows = []
    for post_order, (cat, label, cat_bundles) in enumerate(grouped):
        slug = slug_for_category(cat)
        parts = slice_category(cat, label, cat_bundles, lints_by_id)
        multi_part = len(parts) > 1

        slice_names = []
        total_bytes = 0
        for i, (part_bundles, part_lints) in enumerate(parts, start=1):
            fname = f"bundles.{slug}.part{i}.json" if multi_part else f"bundles.{slug}.json"
            payload = build_slice_payload(cat, label, part_bundles, part_lints)
            text = json.dumps(payload, ensure_ascii=False, indent=2)
            (work_dir / fname).write_text(text, encoding="utf-8")
            total_bytes += len(text.encode("utf-8"))
            slice_names.append(f"work/{fname}")

        bundle_ids = [b.get("id") for b in cat_bundles]
        bug_watch_count = sum(1 for b in cat_bundles if b.get("bug_watch"))

        categories.append({
            "id": cat,
            "label": label,
            "slug": slug,
            "slices": slice_names,
            "bundle_ids": bundle_ids,
            "bundle_count": len(cat_bundles),
            "bug_watch_count": bug_watch_count,
            "bytes": total_bytes,
            "post_order": post_order,
        })
        summary_rows.append((cat, len(cat_bundles), len(parts), total_bytes))

    categories_payload = {"schema_version": 1, "categories": categories}
    (work_dir / "categories.json").write_text(
        json.dumps(categories_payload, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    print_summary(summary_rows)
    return categories_payload


# --------------------------------------------------------------------------
# Mode 2: on-demand record extraction from comprehensive.json
# --------------------------------------------------------------------------

def _canonical_hex(s):
    """Strip an optional 0x/0X prefix and uppercase the remaining hex
    digits, for case-insensitive FormID matching."""
    s = s.strip()
    if s.lower().startswith("0x"):
        s = s[2:]
    return s.upper()


def _looks_like_formid(s):
    if not isinstance(s, str) or not s.lower().startswith("0x"):
        return False
    hexpart = s[2:]
    return 1 <= len(hexpart) <= 8 and all(c in "0123456789abcdefABCDEF" for c in hexpart)


def build_formid_lookup(keyed_dict):
    """Map canonical-hex -> actual dict key, inspecting the dict's real
    keys at runtime rather than assuming a fixed case/zero-padding format."""
    return {_canonical_hex(k): k for k in keyed_dict}


def _collect_formid_strings(value, out=None):
    """Recursively collect every 0x-hex-looking string found anywhere
    inside `value` (dict values, list items, or a bare string), normalized
    to canonical-hex form."""
    if out is None:
        out = set()
    if isinstance(value, dict):
        for v in value.values():
            _collect_formid_strings(v, out)
    elif isinstance(value, list):
        for v in value:
            _collect_formid_strings(v, out)
    elif _looks_like_formid(value):
        out.add(_canonical_hex(value))
    return out


def extract_records(comprehensive_data, formids):
    """
    Core of --extract: given the parsed comprehensive.json dict and a list
    of requested FormID strings (case-insensitive 0x-hex), return
    {"records": {fid: <entry or None>}, "ref_names": {...capped}}.

    Result `records` keys echo back the caller's original requested strings
    verbatim (so the caller can match its own input list even if case or
    zero-padding differs from the file's own key format).
    """
    records = comprehensive_data.get("records", {}) or {}
    ref_names_all = comprehensive_data.get("ref_names", {}) or {}
    records_lookup = build_formid_lookup(records)

    out_records = {}
    matched_keys = []
    for fid in formids:
        actual_key = records_lookup.get(_canonical_hex(fid))
        if actual_key is not None:
            out_records[fid] = records[actual_key]
            matched_keys.append(actual_key)
        else:
            out_records[fid] = None

    referenced_canons = set()
    for key in matched_keys:
        _collect_formid_strings(records[key], referenced_canons)

    ref_names_lookup = build_formid_lookup(ref_names_all)
    out_ref_names = {}
    for canon in referenced_canons:
        actual = ref_names_lookup.get(canon)
        if actual is not None and actual not in out_ref_names:
            out_ref_names[actual] = ref_names_all[actual]
            if len(out_ref_names) >= MAX_REF_NAMES:
                break

    return {"records": out_records, "ref_names": out_ref_names}


def run_extract(out_dir, formids):
    """
    Mode 2 entry point. Returns a process exit code (0 on success — even if
    some/all requested formids were missing — 1 on hard errors) and prints
    the resulting JSON object to stdout on success.
    """
    path = Path(out_dir) / "comprehensive.json"
    if not path.exists():
        print(f"error: {path} not found", file=sys.stderr)
        return 1

    try:
        size = path.stat().st_size
        if size > COMPREHENSIVE_WARN_BYTES:
            print(
                f"warning: {path} is {size / (1024 * 1024):.1f} MB (> "
                f"{COMPREHENSIVE_WARN_BYTES / (1024 * 1024):.0f} MB) — "
                "loading it fully into memory anyway",
                file=sys.stderr,
            )
        with open(path, "r", encoding="utf-8") as f:
            data = pl.validate_comprehensive_payload(json.load(f), label=str(path))
    except (OSError, json.JSONDecodeError) as e:
        print(f"error: failed to load {path}: {e}", file=sys.stderr)
        return 1

    result = extract_records(data, formids)
    print(json.dumps(result, ensure_ascii=False))
    return 0


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="slice_bundles.py",
        description="Slice bundles.json into per-category work/ files, or "
                     "extract on-demand record detail from comprehensive.json.",
    )
    ap.add_argument(
        "--extract", action="store_true",
        help="Extract mode: print per-FormID record detail (from "
             "comprehensive.json) as JSON to stdout instead of slicing.",
    )
    ap.add_argument("out_dir", help="Pipeline output directory.")
    ap.add_argument(
        "formids", nargs="*",
        help="FormIDs to extract (--extract mode only; ignored/rejected otherwise).",
    )
    return ap


def main(argv=None):
    args = build_arg_parser().parse_args(argv)

    if args.extract:
        if not args.formids:
            print("error: --extract requires at least one FORMID", file=sys.stderr)
            return 1
        return run_extract(args.out_dir, args.formids)

    if args.formids:
        print(
            "error: unexpected extra arguments in slice mode: "
            f"{' '.join(args.formids)}",
            file=sys.stderr,
        )
        return 1

    try:
        run_slice(args.out_dir)
    except FileNotFoundError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
