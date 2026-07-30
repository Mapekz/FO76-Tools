#!/usr/bin/env python3
"""
On-demand record extraction from comprehensive.json for the FO76 patch-notes pipeline.

The patch-notes pipeline writes comprehensive.json (large, full per-record detail keyed
by FormID). A writer subagent invokes this script to fetch per-record detail on demand:

    python3 tools/slice_bundles.py --extract <out_dir> <FORMID> [<FORMID> ...]
        Reads `<out_dir>/comprehensive.json` and prints a small JSON object
        with just the requested records (and any `ref_names` entries they
        reference) to stdout.

Python 3, standard library only.
"""
import argparse
import json
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import patchnotes_lib as pl  # noqa: E402

# --------------------------------------------------------------------------
# Tunables
# --------------------------------------------------------------------------

# Above this comprehensive.json size, --extract still loads the whole file
# (plain json.load) but warns to stderr first, since this script has no
# streaming JSON parser available (stdlib only).
COMPREHENSIVE_WARN_BYTES = 200 * 1024 * 1024

# Cap on the number of ref_names entries returned by --extract.
MAX_REF_NAMES = 200

# --------------------------------------------------------------------------
# Lint index utilities (used by triage_bundles.py)
# --------------------------------------------------------------------------

def _lints_index(lints):
    """Map lint id -> lint dict, preserving insertion (declaration) order."""
    by_id = {}
    for lint in lints:
        lid = lint.get("id")
        if lid is not None:
            by_id[lid] = lint
    return by_id

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
        description="Extract on-demand record detail from comprehensive.json.",
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

    print(
        "error: mode 1 (category slicing) is retired; use --extract instead",
        file=sys.stderr,
    )
    return 1

if __name__ == "__main__":
    sys.exit(main())
