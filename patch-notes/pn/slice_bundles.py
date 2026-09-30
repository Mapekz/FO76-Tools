#!/usr/bin/env python3
"""
On-demand record extraction from comprehensive.json for the FO76 patch-notes pipeline.

The patch-notes pipeline writes comprehensive.json (large, full per-record detail keyed
by FormID). A writer subagent invokes this script to fetch per-record detail on demand:

    python3 -m pn extract <out_dir> <FORMID> [<FORMID> ...]
        Reads `<out_dir>/comprehensive.json` and prints a small JSON object
        with just the requested records (and any `ref_names` entries they
        reference) to stdout.

Python 3, standard library only.
"""
import argparse
import json
import sys

from pn import formids, jsonio, layout, schemas

# --------------------------------------------------------------------------
# Tunables
# --------------------------------------------------------------------------

# Above this comprehensive.json size, extraction still loads the whole file
# (plain json.load) but warns to stderr first, since this script has no
# streaming JSON parser available (stdlib only).
COMPREHENSIVE_WARN_BYTES = 200 * 1024 * 1024

# Cap on the number of ref_names entries an extraction returns.
MAX_REF_NAMES = 200

# --------------------------------------------------------------------------
# On-demand record extraction from comprehensive.json
# --------------------------------------------------------------------------

def build_formid_lookup(keyed_dict):
    """Map canonical-hex -> actual dict key, inspecting the dict's real
    keys at runtime rather than assuming a fixed case/zero-padding format."""
    return {formids.canonical(k) or k: k for k in keyed_dict}

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
    elif formids.canonical(value):
        out.add(formids.canonical(value))
    return out

def extract_records(comprehensive_data, requested):
    """
    Core of `pn extract`: given the parsed comprehensive.json dict and a list
    of requested FormID strings (hex, `0x` optional, any case), return
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
    for fid in requested:
        key = formids.display(fid) if formids.looks_like_formid(fid) else fid
        actual_key = records_lookup.get(key)
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

def run_extract(out_dir, requested):
    """
    Mode 2 entry point. Returns a process exit code (0 on success — even if
    some/all requested formids were missing — 1 on hard errors) and prints
    the resulting JSON object to stdout on success.
    """
    path = layout.comprehensive_json(out_dir)
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
        data = schemas.validate_comprehensive_payload(jsonio.read(path), label=str(path))
    except (OSError, ValueError, TypeError, KeyError) as e:
        print(f"error: failed to load {path}: {e}", file=sys.stderr)
        return 1

    result = extract_records(data, requested)
    print(json.dumps(result, ensure_ascii=False))
    return 0

# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="pn extract",
        description="Print record detail from comprehensive.json as JSON.",
    )
    ap.add_argument("out_dir", help="Pipeline output directory.")
    ap.add_argument("formids", nargs="+", help="FormIDs to extract.")
    return ap


def main(argv=None):
    args = build_arg_parser().parse_args(argv)
    return run_extract(args.out_dir, args.formids)
