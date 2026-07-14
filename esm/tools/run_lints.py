#!/usr/bin/env python3
"""
run_lints.py — Tool 3 of the FO76 patch-notes pipeline: automated lint checks
over the mechanical diff output.

Reads `<out_dir>/comprehensive.json` (full per-record detail, keyed by
FormID — see `patchnotes_lib.py` for the `ChangeEntry` shape each record's
`changes` list is made of) and `<out_dir>/bundles.json` (per-bundle
groupings), runs a fixed registry of rule functions against them (optionally
consulting a live/fake `esm` daemon client for reference-graph checks), and
writes `<out_dir>/lints.json`. It also rewrites `bundles.json` in place so
each bundle's `lint_ids`/`bug_watch` reflect this run's results.

Each rule is a function `rule_name(ctx) -> Iterable[dict]` returning partial
lint dicts (no `id`/`bundle_id` yet — those are assigned centrally after
every rule has run, so numbering is deterministic regardless of how any one
rule iterates). `ctx` is a plain dict (see `build_context`) bundling the
parsed `comprehensive.json`/`bundles.json` data, the daemon client, the ESM
paths, and a `settings` dict for rule tunables.

Rules must never raise: unexpected/missing shapes are skipped quietly (never
treated as a lint), and client errors (offline daemon, `--local`-only setup,
malformed fixture) cause that one check to be skipped rather than aborting
the whole run. When a check's result is genuinely ambiguous the rule should
prefer NOT emitting a lint — the narrative stage re-verifies every lint
against the live daemon before it reaches player-facing notes, so a false
negative here just gets caught later, but a false positive can waste a
writer's verification budget.

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import re
import sys
from collections import Counter, defaultdict
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import esm_gateway  # noqa: E402
import patchnotes_lib as pl  # noqa: E402

# --------------------------------------------------------------------------
# Tunables / defaults
# --------------------------------------------------------------------------

#: Default fnmatch patterns for "unique item" placeholder keywords (rule
#: `orphaned_unique`). Overridable via a `--categories` config file's
#: `unique_keyword_patterns` (or `settings.unique_keyword_patterns`) list.
DEFAULT_UNIQUE_KEYWORD_PATTERNS = ["if_tmp_*"]

#: Record types that count as "this item/perk/keyword can actually reach a
#: player" for the reverse-reference walks in `orphaned_unique` /
#: `unreferenced_perk_rank`.
LIVE_DROP_RECORD_TYPES = {"NPC_", "CONT", "QUST", "COBJ", "LVLI"}

#: Record types eligible for the "stale description still quotes the old
#: number" check (`stats_changed_desc_same`).
STATS_DESC_RECORD_TYPES = {"WEAP", "ARMO", "ALCH", "OMOD", "PERK"}

#: Reverse-reference walk depth used by `orphaned_unique`.
ORPHANED_UNIQUE_DEPTH = 4

#: Hard cap on `dangling_ref` lints in one run.
DANGLING_REF_CAP = 50

_FORMID_RE = re.compile(r"^0x[0-9A-Fa-f]{8}$")
_FORMID_IN_TEXT_RE = re.compile(r"0x[0-9A-Fa-f]{8}")
_DESC_PATH_RE = re.compile(r"description|\bdesc\b", re.IGNORECASE)
_NUMBER_IN_TEXT_RE = re.compile(r"-?\d+(?:\.\d+)?")


# --------------------------------------------------------------------------
# Small shared helpers
# --------------------------------------------------------------------------


def _is_null_formid(cand):
    try:
        return int(cand, 16) == 0
    except (TypeError, ValueError):
        return False


def _is_real_number(v):
    """True for an int/float that isn't secretly a bool (`isinstance(True,
    int)` is True in Python, and a bool is never a meaningful 'stat')."""
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def _as_float(v):
    if not _is_real_number(v):
        return None
    return float(v)


def _looks_like_chance_100(v):
    """ChanceNone accepted shapes: bare int/float 100, or an enum-ish dict
    `{"value": 100, "name": ...}`."""
    if _is_real_number(v):
        return float(v) == 100.0
    if isinstance(v, dict):
        return _looks_like_chance_100(v.get("value"))
    return False


def _lvli_unwrap_entry(e):
    """Mirror patchnotes_lib._lvli_unwrap: some shapes wrap the entry's
    fields in a `{"Leveled List Entry": {...}}` container, others don't."""
    if not isinstance(e, dict):
        return {}
    inner = e.get("Leveled List Entry", e)
    return inner if isinstance(inner, dict) else {}


def _lvli_entry_qty(ue):
    return ue.get("Quantity", ue.get("Count"))


def _lvli_entry_ref(ue):
    return ue.get("Reference") or ue.get("Item")


def _lvli_entry_level(ue):
    return ue.get("Minimum Level", ue.get("Level"))


def _ref_formid(ref):
    """Extract a bare FormID hex string from a leveled-list entry's
    Reference/Item value, whether it's already a bare string or a resolved
    stub dict."""
    if isinstance(ref, str) and pl.is_formid_str(ref):
        return ref
    if isinstance(ref, dict):
        fid = ref.get("formid")
        if isinstance(fid, str) and pl.is_formid_str(fid):
            return fid
    return None


def _entry_is_blocked(ue):
    """(blocked: bool, reason: 'quantity_zero'|'chance_none_100'|None) for a
    leveled-list entry's unwrapped inner dict."""
    qty = _lvli_entry_qty(ue)
    if _is_real_number(qty) and float(qty) == 0.0:
        return True, "quantity_zero"
    if "ChanceNone" in ue and _looks_like_chance_100(ue.get("ChanceNone")):
        return True, "chance_none_100"
    return False, None


def _blocked_reason_text(reason, *, new=False):
    if reason == "quantity_zero":
        return "Quantity is now 0." if new else "Quantity is 0."
    if reason == "chance_none_100":
        return "ChanceNone is now 100 (never drops)." if new else "ChanceNone is 100 (never drops)."
    return "blocked."


def _extract_formid_from_text(s):
    if not isinstance(s, str):
        return None
    m = _FORMID_IN_TEXT_RE.search(s)
    return m.group(0) if m else None


def _walk_for_formids(value, out=None):
    """Recursively collect every FormID-shaped value (bare `0x........`
    string, or a resolved stub/curve dict carrying a `formid` key) found
    anywhere inside `value`."""
    if out is None:
        out = []
    if isinstance(value, str):
        if pl.is_formid_str(value):
            out.append(value)
    elif isinstance(value, list):
        for v in value:
            _walk_for_formids(v, out)
    elif isinstance(value, dict):
        fid = value.get("formid")
        if isinstance(fid, str) and pl.is_formid_str(fid):
            out.append(fid)
        else:
            for v in value.values():
                _walk_for_formids(v, out)
    return out


def _collect_to_side_refs(changes):
    """Harvest every to-side FormID reference from an already-flattened
    `changes` (ChangeEntry list) — deliberately ignores from-side values,
    since `dangling_ref` only cares about references that are newly
    introduced or still present after the patch, not ones disappearing."""
    out = []
    for ce in changes or []:
        if not isinstance(ce, dict):
            continue
        if ce.get("kind") == "array":
            arr = ce.get("array") or {}
            for item in arr.get("added") or []:
                _walk_for_formids(item.get("raw") if isinstance(item, dict) else None, out)
            for item in arr.get("changed") or []:
                if isinstance(item, dict):
                    out.extend(_collect_to_side_refs(item.get("changes")))
        else:
            _walk_for_formids(ce.get("to"), out)
    return out


def _is_description_path(path):
    if not path:
        return False
    return bool(_DESC_PATH_RE.search(path))


def _is_full_ish_string_change(ce):
    """A prose-description-shaped string change: both sides are strings and
    at least one side is long enough to be prose, not a short label."""
    if ce.get("kind") not in ("scalar", "string"):
        return False
    fv, tv = ce.get("from"), ce.get("to")
    return isinstance(fv, str) and isinstance(tv, str) and (len(fv) > 20 or len(tv) > 20)


def _numbers_in_text(s):
    if not isinstance(s, str):
        return []
    out = []
    for m in _NUMBER_IN_TEXT_RE.finditer(s):
        try:
            out.append(float(m.group(0)))
        except ValueError:
            continue
    return out


def _numeric_match(text_numbers, value):
    fv = _as_float(value)
    if fv is None:
        return False
    return any(abs(n - fv) < 1e-9 for n in text_numbers)


def _find_bundle_for(fid, bundles):
    """First bundle where `fid` is the anchor, else first where it's any
    member. Returns None if `fid` appears in no bundle."""
    fallback = None
    for b in bundles:
        anchor = b.get("anchor") or {}
        if anchor.get("form_id") == fid:
            return b
        if fallback is None:
            for m in b.get("members") or []:
                if m.get("form_id") == fid:
                    fallback = b
                    break
    return fallback


def _edid_for(fid, records, ref_names):
    if not fid:
        return None
    rec = records.get(fid)
    if rec and rec.get("editor_id"):
        return rec["editor_id"]
    info = ref_names.get(fid)
    if info and info.get("editor_id"):
        return info["editor_id"]
    return None


def _record_type_for(fid, records, ref_names):
    if not fid:
        return None
    rec = records.get(fid)
    if rec and rec.get("record_type"):
        return rec["record_type"]
    info = ref_names.get(fid)
    if info:
        return info.get("record_type")
    return None


def _matches_any(edid, patterns):
    if not edid:
        return False
    return any(fnmatch.fnmatchcase(edid, pat) for pat in patterns)


def _unique_keyword_patterns(ctx):
    patterns = (ctx.get("settings") or {}).get("unique_keyword_patterns")
    return patterns if patterns else DEFAULT_UNIQUE_KEYWORD_PATTERNS


def _bundle_matching_keyword(bundle, records, ref_names, patterns):
    """First KYWD member/edge-endpoint of `bundle` whose EditorID matches one
    of `patterns`. Returns its FormID, or None."""
    for m in bundle.get("members") or []:
        if m.get("record_type") != "KYWD":
            continue
        fid = m.get("form_id")
        edid = m.get("editor_id") or _edid_for(fid, records, ref_names)
        if _matches_any(edid, patterns):
            return fid
    for e in bundle.get("edges") or []:
        for key in ("from", "to"):
            fid = e.get(key)
            if not fid:
                continue
            if _record_type_for(fid, records, ref_names) != "KYWD":
                continue
            edid = _edid_for(fid, records, ref_names)
            if _matches_any(edid, patterns):
                return fid
    return None


def _has_live_referencer(client, esm, fid, *, depth):
    """True/False if the reverse-reference walk succeeded and did/didn't
    surface a "can actually reach a player" record type; None if the walk
    itself failed (client error) — callers should treat None as "unknown,
    don't emit"."""
    try:
        result = client.refs(esm, fid, depth=depth)
    except Exception:
        return None
    rows = result.get("rows") if isinstance(result, dict) else None
    if not isinstance(rows, list):
        return None
    return any(row.get("record_type") in LIVE_DROP_RECORD_TYPES for row in rows)


# --------------------------------------------------------------------------
# Rule 1: lvli_blocked_entry (error)
# --------------------------------------------------------------------------


def rule_lvli_blocked_entry(ctx):
    records = ctx["records"]
    ref_names = ctx["ref_names"]
    lints = []

    for fid, rec in sorted(records.items()):
        if not isinstance(rec, dict) or rec.get("record_type") != "LVLI":
            continue
        status = rec.get("status")
        if status not in ("added", "changed"):
            continue
        edid = rec.get("editor_id") or fid

        try:
            if status == "added":
                entries = ((rec.get("fields") or {}).get("Entries")) or []
                valid_entries = [e for e in entries if isinstance(e, dict)]
                if not valid_entries:
                    continue

                blocked = []
                for e in valid_entries:
                    ue = _lvli_unwrap_entry(e)
                    is_blocked, reason = _entry_is_blocked(ue)
                    if is_blocked:
                        blocked.append((ue, reason))

                for ue, reason in blocked:
                    ref = _lvli_entry_ref(ue)
                    label = pl.annotate_ref(ref, ref_names)
                    level = _lvli_entry_level(ue)
                    lints.append(
                        {
                            "rule": "lvli_blocked_entry",
                            "severity": "error",
                            "form_id": fid,
                            "message": (
                                f"Leveled list `{edid}` has a blocked entry for {label} "
                                f"(min lvl {pl.fmt_num(level)}): {_blocked_reason_text(reason)}"
                            ),
                            "data": {
                                "status": "added",
                                "reason": reason,
                                "item": _ref_formid(ref),
                                "quantity": _lvli_entry_qty(ue),
                                "minimum_level": level,
                            },
                        }
                    )

                if blocked and len(blocked) == len(valid_entries):
                    lints.append(
                        {
                            "rule": "lvli_blocked_entry",
                            "severity": "error",
                            "form_id": fid,
                            "message": (
                                f"Leveled list `{edid}` has all {len(valid_entries)} entries "
                                "blocked (Quantity 0 or ChanceNone 100) -- this list can never "
                                "produce a drop."
                            ),
                            "data": {"all_blocked": True, "entry_count": len(valid_entries)},
                        }
                    )

            else:  # status == "changed"
                for ce in rec.get("changes") or []:
                    if not isinstance(ce, dict) or ce.get("kind") != "array":
                        continue
                    arr = ce.get("array") or {}

                    for item in arr.get("added") or []:
                        if not isinstance(item, dict):
                            continue
                        ue = _lvli_unwrap_entry(item.get("raw"))
                        is_blocked, reason = _entry_is_blocked(ue)
                        if not is_blocked:
                            continue
                        ref = _lvli_entry_ref(ue)
                        label = pl.annotate_ref(ref, ref_names) if ref is not None else item.get(
                            "key_display", "?"
                        )
                        level = _lvli_entry_level(ue)
                        lints.append(
                            {
                                "rule": "lvli_blocked_entry",
                                "severity": "error",
                                "form_id": fid,
                                "message": (
                                    f"Leveled list `{edid}` gained a new blocked entry for "
                                    f"{label} (min lvl {pl.fmt_num(level)}): "
                                    f"{_blocked_reason_text(reason)}"
                                ),
                                "data": {
                                    "status": "changed",
                                    "reason": reason,
                                    "item": _ref_formid(ref),
                                    "quantity": _lvli_entry_qty(ue),
                                    "minimum_level": level,
                                },
                            }
                        )

                    for item in arr.get("changed") or []:
                        if not isinstance(item, dict):
                            continue
                        is_blocked, reason = False, None
                        for nested in item.get("changes") or []:
                            last = (nested.get("path") or "").rsplit(" / ", 1)[-1].lower()
                            to_val = nested.get("to")
                            if last in ("quantity", "count") and _is_real_number(to_val) and float(to_val) == 0.0:
                                is_blocked, reason = True, "quantity_zero"
                                break
                            if last == "chancenone" and _looks_like_chance_100(to_val):
                                is_blocked, reason = True, "chance_none_100"
                                break
                        if not is_blocked:
                            continue
                        key_display = item.get("key_display")
                        ref_fid = _extract_formid_from_text(key_display)
                        label = pl.annotate_ref(ref_fid, ref_names) if ref_fid else (key_display or "?")
                        lints.append(
                            {
                                "rule": "lvli_blocked_entry",
                                "severity": "error",
                                "form_id": fid,
                                "message": (
                                    f"Leveled list `{edid}` entry {label} became blocked this "
                                    f"patch: {_blocked_reason_text(reason, new=True)}"
                                ),
                                "data": {
                                    "status": "changed",
                                    "reason": reason,
                                    "item": ref_fid,
                                    "key_display": key_display,
                                },
                            }
                        )
        except Exception:
            continue

    return lints


# --------------------------------------------------------------------------
# Rule 2: dangling_ref (error)
# --------------------------------------------------------------------------


def rule_dangling_ref(ctx):
    records = ctx["records"]
    ref_names = ctx["ref_names"]
    client = ctx["client"]
    new_esm = ctx.get("new_esm")
    old_esm = ctx.get("old_esm")

    exists_cache = {}

    def _exists(esm, cand):
        if esm is None:
            # Can't verify -- prefer NOT emitting over a false positive.
            return True
        key = (esm, cand)
        if key not in exists_cache:
            try:
                exists_cache[key] = bool(client.exists(esm, cand))
            except Exception:
                exists_cache[key] = True
        return exists_cache[key]

    lints = []
    seen_pairs = set()
    capped = False

    for fid, rec in sorted(records.items()):
        if not isinstance(rec, dict):
            continue
        if capped:
            break

        candidates = []
        for r in rec.get("refs_out") or []:
            if isinstance(r, dict):
                f = r.get("formid")
                if f:
                    candidates.append(f)
        candidates.extend(_collect_to_side_refs(rec.get("changes")))

        for cand in candidates:
            if not isinstance(cand, str) or not pl.is_formid_str(cand):
                continue
            if _is_null_formid(cand):
                continue
            if cand in ref_names:
                continue
            key = (fid, cand)
            if key in seen_pairs:
                continue
            seen_pairs.add(key)

            if len(lints) >= DANGLING_REF_CAP:
                capped = True
                break

            if _exists(new_esm, cand) or _exists(old_esm, cand):
                continue

            edid = rec.get("editor_id") or fid
            lints.append(
                {
                    "rule": "dangling_ref",
                    "severity": "error",
                    "form_id": fid,
                    "message": (
                        f"`{edid}` references FormID `{cand}`, which does not resolve in "
                        "either snapshot (dangling reference)."
                    ),
                    "data": {"dangling_formid": cand},
                }
            )

    if capped:
        ctx.setdefault("_notes", []).append(
            f"dangling_ref: hit the {DANGLING_REF_CAP}-lint cap; additional dangling "
            "references may exist but were not reported"
        )

    return lints


# --------------------------------------------------------------------------
# Rule 3: orphaned_unique (warn)
# --------------------------------------------------------------------------


def rule_orphaned_unique(ctx):
    records = ctx["records"]
    ref_names = ctx["ref_names"]
    client = ctx["client"]
    new_esm = ctx.get("new_esm")
    patterns = _unique_keyword_patterns(ctx)
    bundles = ctx.get("bundles") or []

    lints = []
    for fid, rec in sorted(records.items()):
        if not isinstance(rec, dict):
            continue
        if rec.get("status") not in ("added", "changed"):
            continue
        rtype = rec.get("record_type")

        try:
            if rtype == "KYWD":
                edid = rec.get("editor_id")
                if not _matches_any(edid, patterns):
                    continue
                has_live = _has_live_referencer(client, new_esm, fid, depth=ORPHANED_UNIQUE_DEPTH)
                if has_live is None or has_live:
                    continue
                lints.append(
                    {
                        "rule": "orphaned_unique",
                        "severity": "warn",
                        "form_id": fid,
                        "message": (
                            f"Keyword `{edid}` matches a unique-item marker pattern but has no "
                            "live referencer (NPC_/CONT/QUST/COBJ/LVLI) within "
                            f"{ORPHANED_UNIQUE_DEPTH} hops -- likely orphaned/unobtainable."
                        ),
                        "data": {"editor_id": edid, "checked_depth": ORPHANED_UNIQUE_DEPTH},
                    }
                )

            elif rtype in ("WEAP", "ARMO"):
                bundle = _find_bundle_for(fid, bundles)
                if bundle is None:
                    continue
                kywd_fid = _bundle_matching_keyword(bundle, records, ref_names, patterns)
                if kywd_fid is None:
                    continue
                has_live = _has_live_referencer(client, new_esm, fid, depth=ORPHANED_UNIQUE_DEPTH)
                if has_live is None or has_live:
                    continue
                edid = rec.get("editor_id") or fid
                lints.append(
                    {
                        "rule": "orphaned_unique",
                        "severity": "warn",
                        "form_id": fid,
                        "message": (
                            f"`{edid}` carries a unique-item keyword but has no "
                            "LVLI/NPC_/CONT/QUST/COBJ referencer within "
                            f"{ORPHANED_UNIQUE_DEPTH} hops -- it can never drop."
                        ),
                        "data": {
                            "editor_id": edid,
                            "keyword_formid": kywd_fid,
                            "checked_depth": ORPHANED_UNIQUE_DEPTH,
                        },
                    }
                )
        except Exception:
            continue

    return lints


# --------------------------------------------------------------------------
# Rule 4: unreferenced_perk_rank (warn)
# --------------------------------------------------------------------------


def rule_unreferenced_perk_rank(ctx):
    records = ctx["records"]
    client = ctx["client"]
    new_esm = ctx.get("new_esm")

    lints = []
    for fid, rec in sorted(records.items()):
        if not isinstance(rec, dict) or rec.get("record_type") != "PERK":
            continue
        if rec.get("status") not in ("added", "changed"):
            continue
        if rec.get("cut"):
            continue

        try:
            result = client.refs(new_esm, fid, depth=1)
        except Exception:
            continue
        rows = result.get("rows") if isinstance(result, dict) else None
        if not isinstance(rows, list):
            continue
        if any(row.get("record_type") == "PCRD" for row in rows):
            continue

        edid = rec.get("editor_id") or fid
        lints.append(
            {
                "rule": "unreferenced_perk_rank",
                "severity": "warn",
                "form_id": fid,
                "message": (
                    f"Perk rank `{edid}` has no PCRD referencing it -- not actually grantable "
                    "to any player."
                ),
                "data": {"editor_id": edid, "checked_depth": 1},
            }
        )

    return lints


# --------------------------------------------------------------------------
# Rule 5: desc_changed_stats_same (info)
# --------------------------------------------------------------------------


def rule_desc_changed_stats_same(ctx):
    records = ctx["records"]
    lints = []

    for fid, rec in sorted(records.items()):
        if not isinstance(rec, dict) or rec.get("status") != "changed":
            continue

        try:
            desc_entries = []
            numeric_entries = []
            for ce in rec.get("changes") or []:
                if not isinstance(ce, dict):
                    continue
                path = ce.get("path", "")
                if _is_description_path(path) or _is_full_ish_string_change(ce):
                    desc_entries.append(ce)
                if ce.get("suppressed") == "noise":
                    continue
                if ce.get("kind") == "scalar":
                    if _is_real_number(ce.get("from")) or _is_real_number(ce.get("to")):
                        numeric_entries.append(ce)

            if not desc_entries or numeric_entries:
                continue

            edid = rec.get("editor_id") or fid
            lints.append(
                {
                    "rule": "desc_changed_stats_same",
                    "severity": "info",
                    "form_id": fid,
                    "message": (
                        f"`{edid}`'s description changed but no numeric stat changed alongside "
                        "it -- text-only change; review for typo vs stealth mechanic claim."
                    ),
                    "data": {"paths": [ce.get("path") for ce in desc_entries]},
                }
            )
        except Exception:
            continue

    return lints


# --------------------------------------------------------------------------
# Rule 6: stats_changed_desc_same (info)
# --------------------------------------------------------------------------


def rule_stats_changed_desc_same(ctx):
    records = ctx["records"]
    lints = []

    for fid, rec in sorted(records.items()):
        if not isinstance(rec, dict) or rec.get("record_type") not in STATS_DESC_RECORD_TYPES:
            continue
        if rec.get("status") != "changed":
            continue

        try:
            desc = rec.get("description")
            if not isinstance(desc, str) or not desc.strip():
                continue
            text_numbers = _numbers_in_text(desc)
            if not text_numbers:
                continue

            changes = rec.get("changes") or []
            # If the description itself changed this patch, this isn't a
            # "stale description" case -- that's desc_changed_stats_same's
            # territory (or just a legitimate joint update).
            if any(_is_description_path(ce.get("path", "")) for ce in changes if isinstance(ce, dict)):
                continue

            for ce in changes:
                if not isinstance(ce, dict) or ce.get("kind") != "scalar":
                    continue
                if ce.get("suppressed") == "noise":
                    continue
                fv = ce.get("from")
                if not _is_real_number(fv):
                    continue
                if not _numeric_match(text_numbers, fv):
                    continue
                edid = rec.get("editor_id") or fid
                lints.append(
                    {
                        "rule": "stats_changed_desc_same",
                        "severity": "info",
                        "form_id": fid,
                        "message": (
                            f"`{edid}`'s description still quotes {pl.fmt_num(fv)}, matching the "
                            f"pre-patch value of {ce.get('path')} -- possible stale description."
                        ),
                        "data": {"matched_number": fv, "path": ce.get("path")},
                    }
                )
        except Exception:
            continue

    return lints


# --------------------------------------------------------------------------
# Rule 7: cut_newly_deprecated (info)
# --------------------------------------------------------------------------


def rule_cut_newly_deprecated(ctx):
    records = ctx["records"]
    lints = []

    for fid, rec in sorted(records.items()):
        if not isinstance(rec, dict):
            continue
        cut = rec.get("cut")
        if not isinstance(cut, dict) or cut.get("kind") != "newly_deprecated":
            continue

        edid = rec.get("editor_id") or fid
        lints.append(
            {
                "rule": "cut_newly_deprecated",
                "severity": "info",
                "form_id": fid,
                "message": (
                    f"`{edid}` was renamed into a cut/deprecated marker this patch "
                    f"(marker: {cut.get('marker')})."
                ),
                "data": dict(cut),
            }
        )

    return lints


# --------------------------------------------------------------------------
# Registry
# --------------------------------------------------------------------------

RULES = {
    "lvli_blocked_entry": rule_lvli_blocked_entry,
    "dangling_ref": rule_dangling_ref,
    "orphaned_unique": rule_orphaned_unique,
    "unreferenced_perk_rank": rule_unreferenced_perk_rank,
    "desc_changed_stats_same": rule_desc_changed_stats_same,
    "stats_changed_desc_same": rule_stats_changed_desc_same,
    "cut_newly_deprecated": rule_cut_newly_deprecated,
}

#: Execution order when `--rules` isn't given. Doesn't affect final lint
#: numbering (that's sorted by `(rule, form_id)` after the fact) but does
#: determine `meta.rules_run`'s order and stderr summary order.
RULE_ORDER = [
    "lvli_blocked_entry",
    "dangling_ref",
    "orphaned_unique",
    "unreferenced_perk_rank",
    "desc_changed_stats_same",
    "stats_changed_desc_same",
    "cut_newly_deprecated",
]


# --------------------------------------------------------------------------
# Context / orchestration
# --------------------------------------------------------------------------


def build_context(comprehensive, bundles, client, new_esm=None, old_esm=None, settings=None):
    """Build the `ctx` dict every rule function receives."""
    return {
        "records": (comprehensive or {}).get("records", {}) or {},
        "ref_names": (comprehensive or {}).get("ref_names", {}) or {},
        "bundles": (bundles or {}).get("bundles", []) or [],
        "client": client,
        "new_esm": new_esm,
        "old_esm": old_esm,
        "settings": settings or {},
        "_notes": [],
    }


def _now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def assign_bundle_id(form_id, bundles):
    """Primary bundle for a lint's `form_id`: prefer an anchor match, else
    the first bundle listing it as any member. None if it appears nowhere."""
    b = _find_bundle_for(form_id, bundles)
    return b.get("id") if b else None


def inject_into_bundles(lints, bundles_data):
    """Return a copy of `bundles_data` with its top-level `lints` replaced by
    `lints`, and each bundle's `lint_ids`/`bug_watch` recomputed from
    scratch: `lint_ids` = ids of lints whose `form_id` is one of that
    bundle's members (anchor included); `bug_watch` = True iff any of those
    lints has severity error or warn. Bundles with no matching lint end up
    with `lint_ids: []`, `bug_watch: False` -- everything else about each
    bundle dict is left untouched."""
    bundles_data = dict(bundles_data or {})
    bundles_list = list(bundles_data.get("bundles") or [])

    member_to_bundle_ids = defaultdict(list)
    for b in bundles_list:
        fids = set()
        anchor = b.get("anchor") or {}
        if anchor.get("form_id"):
            fids.add(anchor["form_id"])
        for m in b.get("members") or []:
            if isinstance(m, dict) and m.get("form_id"):
                fids.add(m["form_id"])
        for fid in fids:
            member_to_bundle_ids[fid].append(b.get("id"))

    lint_ids_by_bundle = defaultdict(list)
    severities_by_bundle = defaultdict(set)
    for lint in lints:
        for bid in member_to_bundle_ids.get(lint.get("form_id"), []):
            lint_ids_by_bundle[bid].append(lint["id"])
            severities_by_bundle[bid].add(lint.get("severity"))

    new_bundles = []
    for b in bundles_list:
        nb = dict(b)
        bid = nb.get("id")
        nb["lint_ids"] = lint_ids_by_bundle.get(bid, [])
        nb["bug_watch"] = any(s in ("error", "warn") for s in severities_by_bundle.get(bid, ()))
        new_bundles.append(nb)

    bundles_data["bundles"] = new_bundles
    bundles_data["lints"] = lints
    return bundles_data


def run_lints(comp, bundles, client, new_esm=None, old_esm=None, settings=None, rules=None):
    """Run the requested rules (default: all of `RULE_ORDER`) over `comp` /
    `bundles`, and return `(lints_payload, updated_bundles)`:

    - `lints_payload`: the full `lints.json` document —
      `{"schema_version", "meta": {"generated_at", "rules_run", "counts",
      "notes"?}, "lints": [...]}`.
    - `updated_bundles`: `bundles` with `lints`/`lint_ids`/`bug_watch`
      refreshed (see `inject_into_bundles`).

    Never raises: an individual rule that throws is caught, skipped, and
    noted in `meta.notes` rather than aborting the whole run.
    """
    rule_names = list(rules) if rules else list(RULE_ORDER)
    ctx = build_context(comp, bundles, client, new_esm, old_esm, settings)

    all_lints = []
    for name in rule_names:
        fn = RULES.get(name)
        if fn is None:
            ctx["_notes"].append(f"unknown rule '{name}' -- skipped")
            continue
        try:
            results = list(fn(ctx) or [])
        except Exception as exc:  # pragma: no cover - defensive, rules shouldn't raise
            ctx["_notes"].append(f"{name}: rule raised {exc!r} and was skipped")
            results = []
        for r in results:
            r.setdefault("rule", name)
        all_lints.extend(results)

    all_lints.sort(key=lambda l: (l.get("rule", ""), l.get("form_id", "")))
    for i, lint in enumerate(all_lints, start=1):
        lint["id"] = f"L{i:04d}"
        lint["bundle_id"] = assign_bundle_id(lint.get("form_id"), ctx["bundles"])

    counts = {"error": 0, "warn": 0, "info": 0}
    for lint in all_lints:
        sev = lint.get("severity")
        if sev in counts:
            counts[sev] += 1

    meta = {
        "generated_at": _now_iso(),
        "rules_run": rule_names,
        "counts": counts,
    }
    if ctx["_notes"]:
        meta["notes"] = ctx["_notes"]

    lints_payload = {
        "schema_version": pl.SCHEMA_VERSION,
        "meta": meta,
        "lints": all_lints,
    }

    updated_bundles = inject_into_bundles(all_lints, bundles)

    return lints_payload, updated_bundles


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def load_settings(categories_path):
    """Load rule tunables from a `--categories` config file. Accepts either
    `{"settings": {...}}` or a flat dict directly containing tunable keys
    (e.g. `unique_keyword_patterns`). Missing/unreadable/malformed file ->
    empty settings (rules fall back to their defaults)."""
    if not categories_path:
        return {}
    try:
        with open(categories_path, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        return {}
    if not isinstance(data, dict):
        return {}
    settings = data.get("settings")
    return settings if isinstance(settings, dict) else data


def print_summary(lints_payload, rule_names, stream=sys.stderr):
    lints = lints_payload.get("lints", [])
    counts_by_rule = Counter(l.get("rule") for l in lints)
    header = f"{'rule':<28}{'count':>8}"
    print(header, file=stream)
    print("-" * len(header), file=stream)
    for name in rule_names:
        print(f"{name:<28}{counts_by_rule.get(name, 0):>8}", file=stream)
    print("-" * len(header), file=stream)
    print(f"{'total':<28}{sum(counts_by_rule.values()):>8}", file=stream)

    counts = lints_payload.get("meta", {}).get("counts", {})
    print(
        f"by severity: error={counts.get('error', 0)} warn={counts.get('warn', 0)} "
        f"info={counts.get('info', 0)}",
        file=stream,
    )
    for note in lints_payload.get("meta", {}).get("notes", []):
        print(f"note: {note}", file=stream)


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="run_lints.py",
        description="Run automated lint checks over the patch-notes pipeline's "
        "comprehensive.json + bundles.json, writing lints.json and updating "
        "bundles.json in place.",
    )
    ap.add_argument("out_dir", help="Pipeline output directory (contains comprehensive.json, bundles.json).")
    ap.add_argument("--new-esm", help="Path to the new-snapshot ESM (required unless --offline).")
    ap.add_argument("--old-esm", help="Path to the old-snapshot ESM (optional; improves dangling_ref).")
    ap.add_argument(
        "--esm-bin", default="target/release/esm", help="Path to the esm CLI binary (used to spawn/reuse the daemon)."
    )
    ap.add_argument("--categories", help="Config file supplying rule tunables (e.g. unique_keyword_patterns).")
    ap.add_argument("--offline", action="store_true", help="Use a fixture-backed FakeGateway instead of a real daemon.")
    ap.add_argument("--refs-fixture", help="Fixture JSON for --offline mode (see esm_gateway.FakeGateway).")
    ap.add_argument("--rules", help="Comma-separated subset of rules to run (default: all).")
    return ap


def main(argv=None):
    args = build_arg_parser().parse_args(argv)
    out_dir = Path(args.out_dir)

    try:
        with open(out_dir / "comprehensive.json", encoding="utf-8") as f:
            comp = json.load(f)
        with open(out_dir / "bundles.json", encoding="utf-8") as f:
            bundles = json.load(f)
    except (OSError, json.JSONDecodeError) as exc:
        print(f"error: failed to read pipeline output from {out_dir}: {exc}", file=sys.stderr)
        return 1

    settings = load_settings(args.categories)

    rule_names = None
    if args.rules:
        rule_names = [r.strip() for r in args.rules.split(",") if r.strip()]
        for r in rule_names:
            if r not in RULES:
                print(f"warning: unknown rule '{r}' in --rules", file=sys.stderr)

    client = None
    try:
        if args.offline:
            if not args.refs_fixture:
                print("error: --offline requires --refs-fixture", file=sys.stderr)
                return 1
            client = esm_gateway.FakeGateway(args.refs_fixture)
            new_esm = args.new_esm or "new.esm"
            old_esm = args.old_esm or "old.esm"
        else:
            if not args.new_esm:
                print("error: --new-esm is required unless --offline", file=sys.stderr)
                return 1
            client = esm_gateway.ensure_daemon(args.esm_bin, args.new_esm)
            new_esm = args.new_esm
            old_esm = args.old_esm

        lints_payload, updated_bundles = run_lints(
            comp, bundles, client, new_esm, old_esm, settings, rules=rule_names
        )
    finally:
        if client is not None:
            client.close()

    with open(out_dir / "lints.json", "w", encoding="utf-8") as f:
        json.dump(lints_payload, f, indent=2)
        f.write("\n")
    with open(out_dir / "bundles.json", "w", encoding="utf-8") as f:
        json.dump(updated_bundles, f, indent=2)
        f.write("\n")

    print_summary(lints_payload, rule_names or RULE_ORDER)
    return 0


if __name__ == "__main__":
    sys.exit(main())
