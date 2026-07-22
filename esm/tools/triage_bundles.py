#!/usr/bin/env python3
"""
triage_bundles.py — mechanical-triage stage for the FO76 patch-notes pipeline.

Reads `<out_dir>/bundles.json` + `<out_dir>/comprehensive.json` and a rule
config (`tools/patch_notes_tiers.json` by default) and assigns each bundle a
tier -- `rollout` (one bulk data-shape change across many records), `deep` (a
real writeup), `brief` (a templated one-liner -- existence is the story),
`drop` (bookkeeping churn, never surfaced), or `ambiguous` (punted to a
runtime assessor agent) -- then writes five files under
`<out_dir>/work/`:

    triage.json       Tier assignment + per-bundle reasons + summary stats.
    deep-slice.json   DEEP bundles in the same {"bundles": [...], "lints":
                      [...]} shape the old per-category slices used, so
                      writer agents work unchanged (see
                      ../.claude/skills/patch-notes/deep-writer-prompt.md).
    ambiguous.json    A compact per-bundle field-diff digest for every
                      `ambiguous` bundle, small enough to paste into one
                      assessor-agent prompt.
    brief-lines.md    Templated one-liners for `brief` bundles, grouped
                      under `### Added` / `### Removed` / `### Renamed / Cut`
                      / `### Other`.
    rollouts.md       One compact aggregate row per recurring bulk change
                      shape; never one line per affected bundle.

Rules are ordered lists of small declarative dicts (see
tools/patch_notes_tiers.json), evaluated top-down, first-match-wins, within
a fixed priority: rollout > deep_rules > drop_rules > brief_rules >
(ambiguous fallback). Rollout is data-driven: every changed, non-context
member must have a change shape recurring at least `settings.
rollout_min_records` times. For every non-rollout bundle, the existing rule
priority is unchanged: a bundle that would satisfy both a deep_rules and a
drop_rules condition is DEEP, never DROP; and a bundle satisfying both
drop_rules and brief_rules (e.g. an all-REFR bundle that happens to be
all-"removed" status, which would otherwise look like brief_rules/
all_removed) is DROP, never BRIEF -- drop_rules and deep_rules are the two
"always" tiers (deliberately unconditional once matched); brief_rules is
softer bucketing for whatever's left.

    python3 tools/triage_bundles.py <out_dir>
        Tier every bundle in <out_dir>/bundles.json, writing the five files
        above.

    python3 tools/triage_bundles.py <out_dir> --merge-assessment ASSESSMENT.json
        Re-tiers using an assessor's `{"tiers": {bundle_id: {"tier":
        "deep"|"brief"|"drop", "reason": "..."}}}` output to resolve every
        bundle rule-tiering left `ambiguous`, then re-emits all five files
        (ambiguous bundles the assessor didn't resolve, or resolved with an
        unrecognized tier, remain `ambiguous`). Assessor-sourced reasons are
        recorded in triage.json's `reasons` prefixed `assessor:`.

Deterministic output: bundle-id lists are sorted, and JSON keys are built by
iterating those sorted lists, so two runs over the same inputs produce
byte-identical output.

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import slice_bundles as sb  # noqa: E402

DEFAULT_TIERS_PATH = SCRIPT_DIR / "patch_notes_tiers.json"

#: Fallback defaults for tiers-config `settings`, mirrored from the task spec
#: (~1200 chars per bundle digest, ~200 chars per summarized change).
DEFAULT_AMBIGUOUS_DIGEST_MAX_CHARS = 1200
DEFAULT_AMBIGUOUS_CHANGE_TRUNCATE_CHARS = 200
DEFAULT_ROLLOUT_MIN_RECORDS = 20

#: brief-lines.md section order; a rule's `bucket` picks which one it renders
#: under (falls back to "Other" if a rule/assessor override omits it).
BUCKET_ORDER = ["Added", "Removed", "Renamed / Cut", "Other"]
BUCKET_HEADINGS = {
    "Added": "### Added",
    "Removed": "### Removed",
    "Renamed / Cut": "### Renamed / Cut",
    "Other": "### Other",
}


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


# --------------------------------------------------------------------------
# Loading
# --------------------------------------------------------------------------


def load_json(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def load_bundles(out_dir):
    return load_json(Path(out_dir) / "bundles.json")


def load_comprehensive(out_dir):
    return load_json(Path(out_dir) / "comprehensive.json")


def load_tiers_config(path):
    return load_json(path)


# --------------------------------------------------------------------------
# Member / bundle helpers
# --------------------------------------------------------------------------


def _fnmatch_any(value, patterns):
    if not value or not patterns:
        return False
    v = value.lower()
    return any(fnmatch.fnmatch(v, p.lower()) for p in patterns)


def _non_context_members(bundle):
    return [m for m in bundle.get("members") or [] if m.get("role") != "context"]


def record_change_shape(record):
    """Return (record_type, sorted distinct top-level unsuppressed paths)."""
    paths = set()
    for entry in record.get("changes") or []:
        if not isinstance(entry, dict) or entry.get("suppressed"):
            continue
        path = entry.get("path") or ""
        paths.add(path.split(" / ", 1)[0])
    return record.get("record_type"), tuple(sorted(paths))


def compute_rollout_shapes(records, threshold):
    """Return deterministic metadata for changed-record shapes at threshold."""
    form_ids_by_shape = defaultdict(list)
    for form_id in sorted(records):
        record = records[form_id]
        if record.get("status") == "changed":
            form_ids_by_shape[record_change_shape(record)].append(form_id)

    rollout_shapes = []
    for (record_type, paths), form_ids in form_ids_by_shape.items():
        if len(form_ids) < threshold:
            continue
        rollout_shapes.append(
            {
                "record_type": record_type,
                "paths": list(paths),
                "record_count": len(form_ids),
                "example_form_ids": form_ids[:3],
            }
        )
    rollout_shapes.sort(
        key=lambda item: (
            -item["record_count"],
            item["record_type"] or "",
            tuple(item["paths"]),
        )
    )
    return rollout_shapes


def _scope_members(scope, bundle):
    if scope == "anchor":
        anchor_fid = (bundle.get("anchor") or {}).get("form_id")
        return [m for m in bundle.get("members") or [] if m.get("form_id") == anchor_fid]
    if scope == "any":
        return list(bundle.get("members") or [])
    return _non_context_members(bundle)  # "member" (default)


def _member_cut_or_renamed(member, records):
    rec = records.get(member.get("form_id")) or {}
    return bool(rec.get("cut")) or bool(rec.get("prev_editor_id"))


def _member_field_match(rule, member):
    record_types = rule.get("record_type")
    if record_types is not None and member.get("record_type") not in record_types:
        return False
    edids = rule.get("edid")
    if edids is not None and not _fnmatch_any(member.get("editor_id"), edids):
        return False
    statuses = rule.get("status")
    if statuses is not None and member.get("status") not in statuses:
        return False
    return True


def _member_matches(rule, member, records):
    if not _member_field_match(rule, member):
        return False
    if rule.get("require_cut_or_renamed") and not _member_cut_or_renamed(member, records):
        return False
    return True


def _members_condition_holds(rule, bundle, records):
    scope = rule.get("member_scope", "member")
    match_mode = rule.get("member_match", "any")
    members = _scope_members(scope, bundle)
    if not members:
        return False
    if match_mode == "all":
        return all(_member_matches(rule, m, records) for m in members)
    return any(_member_matches(rule, m, records) for m in members)


#: --------------------------------------------------------------------------
#: Numeric-change detection (the load-bearing predicate behind
#: deep_rules/substantive_change_major_record_type's require_numeric_change)
#: --------------------------------------------------------------------------
#:
#: A "major record type" bundle should only auto-DEEP when it contains an
#: actual numeric stat delta -- not just any non-rename field change. Real
#: counter-examples that must NOT auto-DEEP (verified against actual patch
#: data): a HAZD bundle whose only change is one unmapped flag bit
#: (rendered as a "0x.."-prefixed hex-STRING bitmask, e.g. "0x50" -> "0x10"
#: -- syntactically hex digits, but not a numeric stat), or a BOOK's
#: holotape text edit (a plain string delta). Real positives that MUST
#: still auto-DEEP (also verified against real data): a WEAP's
#: Damage/Capacity/Speed scalars; a changed curve-table reference (an NPC_'s
#: stat-tier swap, or an EXPL's Damage Curve Table going from a Tier28 curve
#: to nothing) -- which resolves to a plain "0x........" FormID string at
#: the ChangeEntry's own from/to level (kind=="formid") but whose sibling
#: "<field> / curve" nested entry carries the real y-value deltas, or whose
#: resolved stub simply names a CURV (Float Curve) record_type with no
#: inlined points when this run had no --startup-ba2/--curves-dir to
#: evaluate against; and an OMOD's Properties array swapping one Function
#: Type/Property row for another entirely (e.g. trading an Enchantments
#: grant for a direct ActorValues modifier) -- a real mechanic rework where
#: the meaningful numbers (Value 1/Value 2) live in brand-new added/removed
#: rows, not a from/to pair, since there's no "before" state for a wholesale
#: swap to diff against.

_NUMERIC_STRING_RE = re.compile(r"^-?\d+(\.\d+)?$")


def _is_real_number(v):
    """int/float but not bool (bool is technically an int subclass, and a
    boolean flag is never a "numeric stat")."""
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def _numeric_value(v):
    """`float(v)` if `v` is numeric-ish (a real number, or a plain decimal
    numeric string), else None. Deliberately does NOT accept a
    "0x..."-prefixed string: that shape covers both FormID references and
    flags bitmasks (see render_comprehensive.py's `_strip_flags_values`),
    neither of which is a "numeric stat value" even though it's built from
    hex digits -- and `_NUMERIC_STRING_RE` (decimal digits only) already
    rejects it without needing a separate hex-shape check."""
    if _is_real_number(v):
        return float(v)
    if isinstance(v, str) and _NUMERIC_STRING_RE.match(v.strip()):
        try:
            return float(v)
        except ValueError:
            return None
    return None


def _is_curve_like(value):
    """True if `value` is a resolved FormID reference to a curve table --
    either inlined (`{"formid", "curve": [{"x","y"}, ...]}`, per
    patchnotes_lib.is_curve) or a bare resolved stub whose `record_type` is
    CURV (Float Curve) with no inlined points (e.g. this pipeline run had no
    --startup-ba2/--curves-dir to evaluate curve tables against). A changed
    curve-table reference is always a numeric-table swap, even though the
    ChangeEntry itself classifies as kind=="formid" -- the same as any other
    FormID reference (a Keyword, an Actor Value) would."""
    if not isinstance(value, dict):
        return False
    if isinstance(value.get("curve"), list):
        return True
    return value.get("record_type") == "CURV"


def _scalar_pair_is_numeric(fv, tv):
    nf, nt = _numeric_value(fv), _numeric_value(tv)
    return nf is not None and nt is not None and nf != nt


def _contains_numeric_delta(value):
    """Recursively scan `value` -- a ChangeEntry, a raw (not yet
    extract_changes()-processed) sparse-diff dict, or any nested piece of
    one -- for either (a) a "from"/"to"-shaped leaf pair whose two numeric
    values actually differ, or (b) a curve-table reference (see
    _is_curve_like). SAFE to call on an entire ChangeEntry or array-diff
    substructure: it has no "any bare number counts" fallback, so
    array-diff bookkeeping that's never literally keyed "from"/"to" (e.g.
    "count_from"/"count_to", a row's "index_from"/"index_to") can never
    false-positive here -- contrast _raw_element_has_numeric_signal, the
    deliberately more permissive scan reserved for already-decoded
    added/removed array elements (see that function's docstring)."""
    if _is_curve_like(value):
        return True
    if isinstance(value, dict):
        if "from" in value and "to" in value and _scalar_pair_is_numeric(value.get("from"), value.get("to")):
            return True
        return any(_contains_numeric_delta(v) for v in value.values())
    if isinstance(value, list):
        return any(_contains_numeric_delta(v) for v in value)
    return False


def _raw_element_has_numeric_signal(value):
    """More permissive than _contains_numeric_delta: recursively scan an
    already-DECODED raw element -- an array row's "raw" value (from an
    added/removed entry only; this is never called on array-diff
    bookkeeping) or any nested piece of one -- for a bare numeric leaf
    (int/float, not bool) or a curve-table reference. A brand-new (or
    just-removed) element has no "before" state to diff against, so its
    mere numeric content is itself the signal -- e.g. an OMOD Property's
    Value 1/Value 2, an Effect's Magnitude, a Component's Quantity (per the
    coordinator's explicit example). Safe to be permissive here because the
    only inputs ever routed through this function are genuine decoded
    field values, never count/index/strategy bookkeeping."""
    if _is_curve_like(value):
        return True
    if isinstance(value, dict):
        return any(_raw_element_has_numeric_signal(v) for v in value.values())
    if isinstance(value, list):
        return any(_raw_element_has_numeric_signal(v) for v in value)
    return _numeric_value(value) is not None


def _array_entry_is_numeric(array_diff):
    """True if an array ChangeEntry's normalized `array` sub-structure
    carries a numeric delta anywhere in its added/removed/changed rows.
    Deliberately reads ONLY those three keys -- never the sibling
    strategy/key_fields/count_from/count_to bookkeeping -- so array-diff
    metadata can never be mistaken for a numeric leaf."""
    if not isinstance(array_diff, dict):
        return False
    added_removed = (array_diff.get("added") or []) + (array_diff.get("removed") or [])
    for row in added_removed:
        if isinstance(row, dict) and _raw_element_has_numeric_signal(row.get("raw")):
            return True
    for row in array_diff.get("changed") or []:
        if not isinstance(row, dict):
            continue
        changes = row.get("changes")
        if isinstance(changes, list):
            if any(is_numeric_change_entry(nested) for nested in changes if isinstance(nested, dict)):
                return True
        elif isinstance(changes, dict):
            # Defensive: some array diffs (e.g. inlined curve-table point
            # arrays, per real-data inspection) carry a RAW, not yet
            # extract_changes()-processed, sparse-diff dict here (e.g.
            # {"y": {"from":.., "to":..}}) instead of a ChangeEntry list.
            if _contains_numeric_delta(changes):
                return True
    return False


def is_numeric_change_entry(entry):
    """True if this ChangeEntry represents an actual numeric stat delta: a
    scalar/string/enum leaf whose from/to are both numbers (or plain
    decimal numeric strings) and differ; a changed curve-table reference
    (kind=="formid" but either side is curve-shaped, see _is_curve_like);
    or, for an array-kind entry, any added/removed/changed row carrying a
    numeric delta (see _array_entry_is_numeric). This is the load-bearing
    predicate behind deep_rules/substantive_change_major_record_type's
    require_numeric_change: whether a "major record type" bundle's field
    change is worth an unconditional deep writeup, or should be left for
    the assessor to judge (e.g. a HAZD bundle whose only change is one
    unmapped flag bit, or a BOOK's holotape text edit)."""
    if not isinstance(entry, dict):
        return False
    if _is_curve_like(entry.get("from")) or _is_curve_like(entry.get("to")):
        return True
    if _scalar_pair_is_numeric(entry.get("from"), entry.get("to")):
        return True
    if entry.get("kind") == "array":
        return _array_entry_is_numeric(entry.get("array"))
    return False


#: ChangeEntry `path` values that are pure EditorID-rename bookkeeping, not
#: "content" -- render_comprehensive.py emits an "Editor ID" leaf ChangeEntry
#: alongside (not instead of) the record's own `prev_editor_id`/`cut`
#: classification, so a bundle whose ONLY change is a rename would otherwise
#: look "substantive" to require_nonrename_change / require_all_changes_drop
#: / require_no_substantive_change and get misrouted (e.g. a cut-vaulting
#: rename like `Foo` -> `zzzFoo` would wrongly qualify as a deep-tier
#: "substantive change" instead of brief_rules/renamed_or_cut_only). Rename
#: detection belongs to require_cut_or_renamed (which reads `prev_editor_id`/
#: `cut` directly), not to the change-entry-based "is there real content
#: here" checks below.
_RENAME_ONLY_PATHS = {"editor id"}


def _bundle_change_entries(bundle, records):
    """Flat list of (form_id, ChangeEntry) for every non-suppressed,
    non-rename-only ChangeEntry belonging to a non-context member of
    `bundle`, pulled from comprehensive.json's per-record `changes` list.
    Context members carry no diff of their own (they're unchanged
    background, see build_bundles.py's attach_context) so they're never a
    source of "substantive change"."""
    out = []
    for m in _non_context_members(bundle):
        fid = m.get("form_id")
        rec = records.get(fid) or {}
        for ce in rec.get("changes") or []:
            if not isinstance(ce, dict) or ce.get("suppressed"):
                continue
            if (ce.get("path") or "").strip().lower() in _RENAME_ONLY_PATHS:
                continue
            out.append((fid, ce))
    return out


def _bundle_conditions_hold(rule, bundle, records, drop_patterns, narrative_patterns):
    entries = None  # computed lazily, at most once per rule evaluation

    def _entries():
        nonlocal entries
        if entries is None:
            entries = _bundle_change_entries(bundle, records)
        return entries

    if rule.get("require_nonrename_change") and not _entries():
        return False
    if rule.get("require_numeric_change") and not any(is_numeric_change_entry(ce) for _fid, ce in _entries()):
        return False
    if rule.get("require_no_substantive_change") and _entries():
        return False
    if rule.get("require_all_changes_drop"):
        es = _entries()
        if not es or not all(_fnmatch_any(ce.get("path"), drop_patterns) for _fid, ce in es):
            return False
    if rule.get("require_no_narrative_change"):
        if any(_fnmatch_any(ce.get("path"), narrative_patterns) for _fid, ce in _entries()):
            return False
    return True


def rule_matches(rule, bundle, records, drop_patterns, narrative_patterns):
    if not _members_condition_holds(rule, bundle, records):
        return False
    return _bundle_conditions_hold(rule, bundle, records, drop_patterns, narrative_patterns)


# --------------------------------------------------------------------------
# Tier assignment
# --------------------------------------------------------------------------


def assign_tier(bundle, records, config, bulk_shapes=None):
    """Return (tier, reason, bucket) for one bundle: `tier` is one of
    "rollout"/"deep"/"brief"/"drop"/"ambiguous"; `reason` is
    "rollout:<type>/<paths>", "<tier>:<rule id>", or None (ambiguous);
    `bucket` is the brief_rules bucket or None (non-brief). Priority:
    rollout > deep_rules > drop_rules > brief_rules -- see module docstring.

    `bulk_shapes` is precomputed by compute_bundle_tiers. It defaults empty
    so matcher-level callers can exercise only the declarative rule tiers.
    """
    bulk_shapes = bulk_shapes or set()
    members = _non_context_members(bundle)
    member_records = []
    if members:
        for member in members:
            form_id = member.get("form_id")
            if form_id not in records or records[form_id].get("status") != "changed":
                break
            record = records[form_id]
            if record_change_shape(record) not in bulk_shapes:
                break
            member_records.append(record)
        else:
            record_type, paths = record_change_shape(member_records[0])
            return "rollout", f"rollout:{record_type}/{'+'.join(paths)}", None

    drop_patterns = config.get("field_path_drop_patterns") or []
    narrative_patterns = config.get("narrative_signal_patterns") or []

    for rule in config.get("deep_rules") or []:
        if rule_matches(rule, bundle, records, drop_patterns, narrative_patterns):
            return "deep", f"deep:{rule['id']}", None

    for rule in config.get("drop_rules") or []:
        if rule_matches(rule, bundle, records, drop_patterns, narrative_patterns):
            return "drop", f"drop:{rule['id']}", None

    for rule in config.get("brief_rules") or []:
        if rule_matches(rule, bundle, records, drop_patterns, narrative_patterns):
            return "brief", f"brief:{rule['id']}", rule.get("bucket") or "Other"

    return "ambiguous", None, None


class TierAssignments(dict):
    """Bundle tier mapping carrying its once-computed rollout metadata."""

    def __init__(self, rollout_shapes):
        super().__init__()
        self.rollout_shapes = rollout_shapes


def compute_bundle_tiers(bundles, records, config):
    """`{bundle_id: {"tier", "reason", "bucket"}}` for every bundle."""
    settings = config.get("settings") or {}
    threshold = settings.get("rollout_min_records", DEFAULT_ROLLOUT_MIN_RECORDS)
    rollout_shapes = compute_rollout_shapes(records, threshold)
    bulk_shapes = {
        (item["record_type"], tuple(item["paths"]))
        for item in rollout_shapes
    }
    result = TierAssignments(rollout_shapes)
    for b in bundles:
        tier, reason, bucket = assign_tier(b, records, config, bulk_shapes)
        result[b["id"]] = {"tier": tier, "reason": reason, "bucket": bucket}
    return result


# --------------------------------------------------------------------------
# triage.json
# --------------------------------------------------------------------------


def build_triage_payload(bundles, tiers_by_id, extra_stats=None):
    buckets_by_tier = {"rollout": [], "deep": [], "brief": [], "drop": [], "ambiguous": []}
    for b in bundles:  # bundles.json order is already deterministic (B0001, B0002, ...)
        bid = b["id"]
        buckets_by_tier[tiers_by_id[bid]["tier"]].append(bid)

    rollout = sorted(buckets_by_tier["rollout"])
    deep = sorted(buckets_by_tier["deep"])
    brief = sorted(buckets_by_tier["brief"])
    drop = sorted(buckets_by_tier["drop"])
    ambiguous = sorted(buckets_by_tier["ambiguous"])

    reasons = {}
    for bid in sorted(tiers_by_id):
        r = tiers_by_id[bid].get("reason")
        if r:
            reasons[bid] = r

    stats = {
        "total_bundles": len(bundles),
        "rollout": len(rollout),
        "deep": len(deep),
        "brief": len(brief),
        "drop": len(drop),
        "ambiguous": len(ambiguous),
    }
    if extra_stats:
        stats.update(extra_stats)

    return {
        "schema_version": 1,
        "rollout": rollout,
        "deep": deep,
        "brief": brief,
        "drop": drop,
        "ambiguous": ambiguous,
        "stats": stats,
        "reasons": reasons,
        "rollout_shapes": list(getattr(tiers_by_id, "rollout_shapes", [])),
    }


# --------------------------------------------------------------------------
# deep-slice.json
# --------------------------------------------------------------------------

#: The exact per-bundle key set the writer contract documents (see
#: ../.claude/skills/patch-notes/deep-writer-prompt.md) -- deliberately drops
#: category/category_label/category_rule, which are retired concepts for the
#: DEEP tier (writers no longer work one category at a time).
_DEEP_SLICE_BUNDLE_KEYS = ("id", "title", "anchor", "members", "edges", "bug_watch", "lint_ids")


def _strip_bundle_for_deep_slice(bundle):
    return {k: bundle.get(k) for k in _DEEP_SLICE_BUNDLE_KEYS}


def build_deep_slice_payload(deep_bundles, lints_by_id):
    lints = sb.lints_for_bundles(deep_bundles, lints_by_id)
    return {
        "schema_version": 1,
        "bundles": [_strip_bundle_for_deep_slice(b) for b in deep_bundles],
        "lints": lints,
    }


# --------------------------------------------------------------------------
# ambiguous.json
# --------------------------------------------------------------------------


def _short_scalar(v):
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float, str)):
        return str(v)
    return "…"


def _truncate(s, max_chars):
    if max_chars is not None and max_chars > 0 and len(s) > max_chars:
        return s[: max(0, max_chars - 1)] + "…"
    return s


def summarize_change(ce, max_chars):
    """One-line summary of a ChangeEntry, truncated to ~max_chars."""
    path = ce.get("path") or "?"
    kind = ce.get("kind")
    if kind == "array":
        arr = ce.get("array") or {}
        cf, ct = arr.get("count_from"), arr.get("count_to")
        added_n = len(arr.get("added") or [])
        removed_n = len(arr.get("removed") or [])
        changed_n = len(arr.get("changed") or [])
        summary = f"{path}: {cf}->{ct} items (+{added_n} -{removed_n} ~{changed_n})"
    elif kind == "vmad":
        vmad = ce.get("vmad") or {}
        summary = (
            f"{path}: VMAD props +{len(vmad.get('added') or {})} "
            f"-{len(vmad.get('removed') or {})} ~{len(vmad.get('changed') or {})}"
        )
    else:
        fd = ce.get("from_display") or _short_scalar(ce.get("from"))
        td = ce.get("to_display") or _short_scalar(ce.get("to"))
        summary = f"{path}: {fd} -> {td}"
    return _truncate(summary, max_chars)


def _digest_size(digest):
    return len(json.dumps(digest, ensure_ascii=False))


def _cap_digest(digest, max_chars):
    if max_chars is None or max_chars <= 0 or _digest_size(digest) <= max_chars:
        return digest
    digest = dict(digest)
    digest["members"] = [dict(m) for m in digest.get("members") or []]
    digest["truncated"] = True
    # Trim from the end: drop trailing changes, then trailing whole members,
    # until the serialized digest fits (or there's nothing left to trim).
    while digest["members"] and _digest_size(digest) > max_chars:
        last = digest["members"][-1]
        if last.get("changes"):
            last["changes"] = last["changes"][:-1]
        else:
            digest["members"].pop()
    return digest


def build_ambiguous_digest(bundle, records, max_bundle_chars, max_change_chars):
    anchor = bundle.get("anchor") or {}
    members_digest = []
    for m in _non_context_members(bundle):
        fid = m.get("form_id")
        rec = records.get(fid) or {}
        changes = [
            summarize_change(ce, max_change_chars)
            for ce in (rec.get("changes") or [])
            if isinstance(ce, dict) and not ce.get("suppressed")
        ]
        members_digest.append(
            {
                "form_id": fid,
                "record_type": m.get("record_type"),
                "editor_id": m.get("editor_id"),
                "name": m.get("name"),
                "status": m.get("status"),
                "changes": changes,
            }
        )

    digest = {
        "id": bundle.get("id"),
        "title": bundle.get("title"),
        "category": bundle.get("category"),
        "anchor": {
            "record_type": anchor.get("record_type"),
            "editor_id": anchor.get("editor_id"),
            "name": anchor.get("name"),
        },
        "members": members_digest,
    }
    return _cap_digest(digest, max_bundle_chars)


def build_ambiguous_payload(ambiguous_bundles, records, max_bundle_chars, max_change_chars):
    return {
        "schema_version": 1,
        "bundles": [
            build_ambiguous_digest(b, records, max_bundle_chars, max_change_chars)
            for b in ambiguous_bundles
        ],
    }


# --------------------------------------------------------------------------
# brief-lines.md
# --------------------------------------------------------------------------


def _display_name(entity):
    return (entity or {}).get("name") or (entity or {}).get("editor_id") or (entity or {}).get("form_id") or "?"


def _added_line(bundle):
    a = bundle.get("anchor") or {}
    return f"- **{_display_name(a)}** ({a.get('record_type', '?')}): added"


def _removed_line(bundle):
    a = bundle.get("anchor") or {}
    return f"- **{_display_name(a)}** ({a.get('record_type', '?')}): removed"


def _renamed_cut_line(bundle, records):
    a = bundle.get("anchor") or {}
    name = _display_name(a)
    rec = records.get(a.get("form_id")) or {}
    prev = rec.get("prev_editor_id")
    cut = rec.get("cut") or {}
    marker = cut.get("marker")
    kind = cut.get("kind")
    if kind == "newly_deprecated" and prev:
        return (
            f"- **{name}**: renamed `{prev}` -> `{a.get('editor_id')}` this patch "
            f"({marker}-marked; vaulted/cut)"
        )
    if kind in ("still_cut", "added_cut") and marker:
        return f"- **{name}**: still {marker}-marked (cut content)"
    if prev:
        return f"- **{name}**: renamed from `{prev}`"
    return f"- **{name}** ({a.get('record_type', '?')}): cut/deprecated"


def _other_line(bundle):
    a = bundle.get("anchor") or {}
    return f"- **{_display_name(a)}** ({a.get('record_type', '?')}): {bundle.get('title') or 'changed'}"


def render_brief_lines(brief_ids, bundles_by_id, tiers_by_id, records):
    """Templated Markdown for the BRIEF tier, grouped under BUCKET_ORDER
    headings; empty string if there are no brief bundles at all."""
    buckets = defaultdict(list)
    for bid in sorted(brief_ids):
        bundle = bundles_by_id[bid]
        bucket = tiers_by_id[bid].get("bucket") or "Other"
        buckets[bucket].append(bundle)

    if not buckets:
        return ""

    lines = []
    for bucket in BUCKET_ORDER:
        items = buckets.get(bucket)
        if not items:
            continue
        lines.append(BUCKET_HEADINGS[bucket])
        lines.append("")
        for bundle in items:
            if bucket == "Added":
                lines.append(_added_line(bundle))
            elif bucket == "Removed":
                lines.append(_removed_line(bundle))
            elif bucket == "Renamed / Cut":
                lines.append(_renamed_cut_line(bundle, records))
            else:
                lines.append(_other_line(bundle))
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n"


# --------------------------------------------------------------------------
# rollouts.md
# --------------------------------------------------------------------------


def _markdown_cell(value):
    return str(value).replace("|", "\\|").replace("\n", " ")


def render_rollouts(rollout_shapes, rollout_ids, bundles_by_id, records):
    """Compact aggregate Markdown table for bulk rollout change shapes."""
    bundle_counts = defaultdict(int)
    for bundle_id in sorted(rollout_ids):
        shapes = {
            record_change_shape(records[member["form_id"]])
            for member in _non_context_members(bundles_by_id[bundle_id])
        }
        for shape in sorted(shapes, key=lambda item: (item[0] or "", item[1])):
            bundle_counts[shape] += 1

    lines = [
        "Each row is a single bulk data change affecting many records at once, "
        "and is normally worth at most one line in the patch notes.",
        "",
        "| Records | Bundles | Type | Fields |",
        "| ---: | ---: | --- | --- |",
    ]
    for item in sorted(
        rollout_shapes,
        key=lambda value: (
            -value["record_count"],
            value["record_type"] or "",
            tuple(value["paths"]),
        ),
    ):
        shape = item["record_type"], tuple(item["paths"])
        # An empty path set means every ChangeEntry on those records was
        # suppressed (raw hex blobs and the like) -- the record changed, but
        # nothing decoded into a nameable field. Say so rather than "(none)",
        # which reads like a bug.
        fields = ", ".join(item["paths"]) or "*(suppressed changes only)*"
        lines.append(
            f"| {item['record_count']} | {bundle_counts.get(shape, 0)} | "
            f"{_markdown_cell(item['record_type'] or '?')} | {_markdown_cell(fields)} |"
        )
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------
# Orchestration
# --------------------------------------------------------------------------


def assemble_outputs(bundles, records, lints_by_id, tiers_by_id, config, extra_stats=None):
    """Build all five outputs from a fully-resolved `tiers_by_id` map."""
    bundles_by_id = {b["id"]: b for b in bundles}

    triage_payload = build_triage_payload(bundles, tiers_by_id, extra_stats)

    deep_bundles = [bundles_by_id[bid] for bid in triage_payload["deep"]]
    deep_slice_payload = build_deep_slice_payload(deep_bundles, lints_by_id)

    settings = config.get("settings") or {}
    max_bundle_chars = settings.get("ambiguous_digest_max_chars", DEFAULT_AMBIGUOUS_DIGEST_MAX_CHARS)
    max_change_chars = settings.get(
        "ambiguous_change_truncate_chars", DEFAULT_AMBIGUOUS_CHANGE_TRUNCATE_CHARS
    )
    ambiguous_bundles = [bundles_by_id[bid] for bid in triage_payload["ambiguous"]]
    ambiguous_payload = build_ambiguous_payload(ambiguous_bundles, records, max_bundle_chars, max_change_chars)

    brief_lines_md = render_brief_lines(triage_payload["brief"], bundles_by_id, tiers_by_id, records)
    rollouts_md = render_rollouts(
        triage_payload["rollout_shapes"],
        triage_payload["rollout"],
        bundles_by_id,
        records,
    )

    return {
        "triage": triage_payload,
        "deep_slice": deep_slice_payload,
        "ambiguous": ambiguous_payload,
        "brief_lines_md": brief_lines_md,
        "rollouts_md": rollouts_md,
    }


def _write_json(path, payload):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2, ensure_ascii=False)
        f.write("\n")


def write_outputs(out_dir, result):
    work_dir = Path(out_dir) / "work"
    work_dir.mkdir(parents=True, exist_ok=True)
    _write_json(work_dir / "triage.json", result["triage"])
    _write_json(work_dir / "deep-slice.json", result["deep_slice"])
    _write_json(work_dir / "ambiguous.json", result["ambiguous"])
    (work_dir / "brief-lines.md").write_text(result["brief_lines_md"], encoding="utf-8")
    (work_dir / "rollouts.md").write_text(result["rollouts_md"], encoding="utf-8")


def run_triage(out_dir, tiers_path=DEFAULT_TIERS_PATH):
    """Fresh rule-only tiering. Returns the same dict `assemble_outputs`
    does; also writes the five `work/` files."""
    bundles_data = load_bundles(out_dir)
    comp_data = load_comprehensive(out_dir)
    config = load_tiers_config(tiers_path)

    bundles = bundles_data.get("bundles") or []
    records = comp_data.get("records") or {}
    lints_by_id = sb._lints_index(bundles_data.get("lints") or [])

    tiers_by_id = compute_bundle_tiers(bundles, records, config)
    result = assemble_outputs(bundles, records, lints_by_id, tiers_by_id, config)
    write_outputs(out_dir, result)
    return result


def merge_assessment(tiers_by_id, assessment):
    """Overlay an assessor's `{"tiers": {bundle_id: {"tier", "reason",
    "bucket"?}}}` onto `tiers_by_id` IN PLACE, resolving only bundles
    currently tiered "ambiguous". A bundle the assessor doesn't mention, or
    resolves with an unrecognized tier, is left ambiguous. Returns the
    number of bundles actually resolved this call."""
    assessor_tiers = (assessment or {}).get("tiers") or {}
    resolved = 0
    for bid, info in tiers_by_id.items():
        if info["tier"] != "ambiguous":
            continue
        override = assessor_tiers.get(bid)
        if not isinstance(override, dict):
            continue
        new_tier = override.get("tier")
        if new_tier not in ("deep", "brief", "drop"):
            eprint(f"warning: assessment.json has unrecognized tier for {bid}: {new_tier!r} -- left ambiguous")
            continue
        reason = override.get("reason")
        info["tier"] = new_tier
        info["reason"] = f"assessor:{reason}" if reason else "assessor:(no reason given)"
        if new_tier == "brief":
            info["bucket"] = override.get("bucket") or "Other"
        resolved += 1
    return resolved


def run_merge_assessment(out_dir, assessment_path, tiers_path=DEFAULT_TIERS_PATH):
    """Recompute rule-based tiers, overlay the assessor's resolution for the
    ambiguous set, and re-emit all five `work/` files."""
    bundles_data = load_bundles(out_dir)
    comp_data = load_comprehensive(out_dir)
    config = load_tiers_config(tiers_path)
    assessment = load_json(assessment_path)

    bundles = bundles_data.get("bundles") or []
    records = comp_data.get("records") or {}
    lints_by_id = sb._lints_index(bundles_data.get("lints") or [])

    tiers_by_id = compute_bundle_tiers(bundles, records, config)
    resolved = merge_assessment(tiers_by_id, assessment)

    result = assemble_outputs(
        bundles, records, lints_by_id, tiers_by_id, config,
        extra_stats={"resolved_by_assessor": resolved},
    )
    write_outputs(out_dir, result)
    return result


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def print_summary(triage_payload, stream=sys.stderr):
    stats = triage_payload.get("stats") or {}
    header = f"{'tier':<12}{'count':>8}"
    print(header, file=stream)
    print("-" * len(header), file=stream)
    for tier in ("rollout", "deep", "brief", "drop", "ambiguous"):
        print(f"{tier:<12}{stats.get(tier, 0):>8}", file=stream)
    print("-" * len(header), file=stream)
    print(f"{'total':<12}{stats.get('total_bundles', 0):>8}", file=stream)
    if "resolved_by_assessor" in stats:
        print(f"\nresolved by assessor this run: {stats['resolved_by_assessor']}", file=stream)


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="triage_bundles.py",
        description="Mechanical-triage stage: assign every bundles.json bundle a tier "
                     "(rollout/deep/brief/drop/ambiguous) via tools/patch_notes_tiers.json, and "
                     "write OUT/work/{triage.json,deep-slice.json,ambiguous.json,"
                     "brief-lines.md,rollouts.md}.",
    )
    ap.add_argument("out_dir", help="Pipeline output directory (must contain bundles.json + comprehensive.json).")
    ap.add_argument(
        "--tiers", default=str(DEFAULT_TIERS_PATH),
        help="Path to patch_notes_tiers.json (default: the copy next to this script).",
    )
    ap.add_argument(
        "--merge-assessment", default=None, metavar="ASSESSMENT_JSON",
        help='Merge an assessor\'s {"tiers": {bundle_id: {"tier","reason"}}} JSON file, '
             "resolving the ambiguous set, then re-emit all five work/ files.",
    )
    return ap


def main(argv=None):
    args = build_arg_parser().parse_args(argv)

    tiers_path = Path(args.tiers)
    if not tiers_path.is_file():
        eprint(f"error: tiers config not found: {tiers_path}")
        return 1

    try:
        if args.merge_assessment:
            result = run_merge_assessment(args.out_dir, args.merge_assessment, tiers_path)
        else:
            result = run_triage(args.out_dir, tiers_path)
    except FileNotFoundError as e:
        eprint(f"error: {e}")
        return 1
    except (OSError, json.JSONDecodeError) as e:
        eprint(f"error: failed to load pipeline output: {e}")
        return 1

    print_summary(result["triage"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
