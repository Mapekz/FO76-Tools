#!/usr/bin/env python3
"""
lvli_audit.py — health sweep over every LVLI (Leveled Item) record in an
ESM, flagging leveled-list configurations where the selection algorithm
degenerates onto one fixed entry (or a near-fixed one) instead of the
apparently-intended pool/roll.

Built on the confirmed FO76 selection algorithm documented in
`skills/esm-cli/SKILL.md`'s "Drop-chance math" section: `Use All` rolls
every entry independently; no flag picks ONE entry uniformly at random then
applies its own chance-none; `Use First Object That Matches All Conditions`
walks entries in list order and takes the first whose Conditions pass (not
a 1/N pick).

Checks four independent patterns:

  A. use_first_starvation — under `Use First Match`, an entry with no
     Conditions, or a `GetRandomPercent` condition whose threshold is >=
     `--near-certain-threshold` (default 95), is always/near-always true —
     every entry listed after it becomes unreachable or effectively so.
     When such an entry's (GLOB-resolved) Minimum Level is also <= the next
     entry's, that's noted alongside the hit as corroborating evidence (the
     level gate can never "un-match" once passed, unlike state-consuming
     Conditions such as `HasLearnedRecipe` or `GetIsInRegion`) — but level
     ordering alone, on an entry with real Conditions, is NOT flagged: most
     ascending-level Use-First-Match lists gated on consumable/situational
     Conditions are legitimately order-dependent (e.g. "give the first
     recipe not yet learned"), so level is corroborating context only, never
     an independent trigger.

  B. bundle_name_uniform_pick — multi-entry lists with neither `Use All`
     nor `Use First Match` always resolve to ONE entry picked uniformly at
     random. That is normal for "pick one of N variants" lists, so this
     only fires when the EditorID/Name itself suggests a give-them-all
     bundle/reward-set (matches `BUNDLE_NAME_RE`), where a uniform
     single-pick may not be the intended behavior.

  C. level_tier_starvation — UNVERIFIED FOR FO76. Classic Creation Engine
     behavior (Skyrim/FO4) collapses entry selection to the single highest
     Minimum Level <= the player's level when `Calculate from all levels <=
     player's level` is unset, silently excluding every lower-Level entry.
     TES5Edit's FO76 schema (wbDefinitionsFO76.pas) defines the flag under
     the same name but does not document this selection-pool behavior for
     FO76's engine specifically, and a neighboring flag bit in the same
     source is annotated "Use special formula in skyrim" — i.e. this flag
     family is known to vary by game. Reported as a SUSPECTED pattern, not
     a confirmed bug — verify any hit against the live game before acting
     on it. Levels are read via `resolve_min_level` (GLOB-resolved where an
     entry carries `Minimum Level Global`, matching rule A).

  D. overlap_ladder — neither `Use All` nor `Use First Match` set, but 2+
     entries carry a one-sided range Condition (>=, >, <=, or < with no
     complementary bound in the same entry on the same Function/Parameters)
     — the shape that lets entries' eligibility overlap on a single roll
     instead of partitioning it, most visibly when a hand-authored rarity
     ladder (escalating `GetRandomPercent` thresholds, e.g. 90/70/guaranteed)
     was seemingly written assuming sequential priority that only `Use First
     Match` actually provides. Under the confirmed no-flag algorithm, the
     engine instead pools whichever entries currently pass and picks ONE
     uniformly at random, which dilutes the apparent tiers rather than
     partitioning them. `same_function` in the finding marks entries that
     all gate on the identical Condition function (the classic ladder look)
     vs. a mixed-function overlap (more often incidental). Odds are computed
     exactly — every eligible-pool subset enumerated — only when every gate
     in the record is `GetRandomPercent` with a resolved threshold and the
     list has <= `MAX_EXACT_ODDS_ENTRIES` entries; `GetLevel`/`GetValue`/
     `GetItemCount` gates are real Conditions but not probabilities, so no
     percentage is invented for them, and larger lists report raw
     conditions only (2^n subset enumeration stops being worth computing
     exactly beyond that size). Reported as a shape to review, not a
     confirmed bug — some hits are shared-threshold alternate pairs or
     level/need-gated variety pools that are almost certainly intentional.

Rules A, C, and D all resolve `Minimum Level Global` / `GetRandomPercent`
GLOB references (a per-entry or per-condition GLOB overriding the static
field, e.g. economy-tunable recipe unlock levels or event drop rates) via
one shared bulk GLOB lookup — reading the static/literal value alone would
understate the true effective threshold for any entry that uses one.

Usage:
    python3 tools/lvli_audit.py [--esm PATH] [--esm-bin PATH] [--out FILE]
                                 [--near-certain-threshold 95]

Python 3, stdlib only (uses esm_gateway.py from this directory).
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from itertools import combinations
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import esm_gateway  # noqa: E402

CALC_ALL_LEVELS_FLAG = "Calculate from all levels <= player's level"
USE_ALL_FLAG = "Use All"
USE_FIRST_MATCH_FLAG = "Use First Object That Matches All Conditions"

#: EditorID/Name substrings implying "hand out all of these", used to scope
#: rule B down from "every plain multi-entry list" (the most common LVLI
#: shape, and normal) to only the ones whose naming implies otherwise.
BUNDLE_NAME_RE = re.compile(r"(?i)(bundle|rewardsall|allrewards|collection|\bset\b|\bkit\b)")

DEFAULT_NEAR_CERTAIN_THRESHOLD = 95.0

#: Rule D — Operator families for detecting one-sided ("unbounded") range
#: Conditions vs. a bounded range (both a lower and upper Operator on the
#: same Function/Parameters within one entry) or an equality check (neither
#: is a range at all, so no overlap risk from that condition alone).
LOWER_OPS = {"Greater Than", "Greater Than Or Equal To"}
UPPER_OPS = {"Less Than", "Less Than Or Equal To"}
EQ_OPS = {"Equal To", "Not Equal To"}
RANGE_OPS = LOWER_OPS | UPPER_OPS

#: Rule D — above this many entries, exact pool-odds computation (2^n subset
#: enumeration) stops being worth it; such records are reported with raw
#: conditions only.
MAX_EXACT_ODDS_ENTRIES = 16


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def list_lvli_form_ids(client: esm_gateway.EsmGateway, esm_path: Path) -> list[str]:
    """Every LVLI FormID in the ESM, via `EsmGateway.list_type` (`Op::
    ListTypeRecords`, the same op `esm list --type LVLI --json` sends) --
    one warm round-trip through the daemon, no subprocess. Routing through
    the gateway (rather than shelling out to `esm list` directly, as this
    used to) is what lets `esm_gateway.py` claim to be the one seam
    everything in `tools/` reaches `esm` through -- see its module
    docstring."""
    return [row["form_id"] for row in client.list_type(str(esm_path), "LVLI", limit=0)]


def entry_conditions(entry: dict) -> list[dict]:
    """Flatten a `Leveled List Entry`'s `Conditions.Conditions[]` down to
    each condition's `Condition Data` dict (Function/Operator/Comparison
    Value/AND-OR), skipping malformed rows rather than raising."""
    conds = entry.get("Conditions")
    if not conds:
        return []
    out = []
    for c in conds.get("Conditions") or []:
        data = (c.get("Condition") or {}).get("Condition Data")
        if data:
            out.append(data)
    return out


def _param_key(param):
    """A hashable key for a condition's Parameter 1/2 under `--resolve stub`:
    the FormID when it's a resolved reference dict, else the raw value
    (usually `None`). Used to group an entry's conditions by what they
    actually gate on, not just which Function name they share."""
    if isinstance(param, dict):
        return param.get("formid")
    return param


def _resolve_glob_ref(ref: dict, glob_values: dict[str, float]) -> float | None:
    """A GLOB reference's current `Value`, looked up in `glob_values` by
    FormID, falling back to EditorID. `None` if neither resolves."""
    formid = ref.get("formid")
    if formid is not None and formid in glob_values:
        return glob_values[formid]
    edid = ref.get("editor_id")
    return glob_values.get(edid) if edid is not None else None


def resolve_comparison_value(cmp_value, glob_values: dict[str, float]) -> float | None:
    """A condition's literal float, or its GLOB's current `Value` (see
    `_resolve_glob_ref`) when the comparison is a GLOB reference. `None` if
    neither resolves."""
    if isinstance(cmp_value, dict):
        return _resolve_glob_ref(cmp_value, glob_values)
    if isinstance(cmp_value, (int, float, str)):
        try:
            return float(cmp_value)
        except (TypeError, ValueError):
            return None
    return None


def resolve_min_level(entry: dict, glob_values: dict[str, float]) -> float | None:
    """An entry's effective Minimum Level: its `Minimum Level Global` GLOB
    (see `_resolve_glob_ref`) when present, else its static `Minimum Level`
    field. `None` if a `Minimum Level Global` is present but unresolved —
    deliberately not falling back to the (likely stale/placeholder) static
    field in that case, since that would understate the true level."""
    lvlg = entry.get("Minimum Level Global")
    if isinstance(lvlg, dict):
        return _resolve_glob_ref(lvlg, glob_values)
    lv = entry.get("Minimum Level")
    return lv if isinstance(lv, (int, float)) else None


def is_near_certain(cond_list: list[dict], threshold: float, glob_values: dict[str, float]) -> bool | None:
    """Whether every condition in `cond_list` is a `GetRandomPercent`
    <=/< check whose threshold is >= `threshold` (i.e. the entry is
    always or near-always eligible). No conditions at all also counts as
    always-true. GLOB-referencing comparisons are resolved via
    `glob_values` (see `collect_glob_values`); returns `None` only when a
    GLOB reference couldn't be resolved (e.g. bulk_get error for that
    GLOB) — reported as "unknown, verify manually" rather than guessed at."""
    if not cond_list:
        return True
    saw_unresolved = False
    for c in cond_list:
        if c.get("Function") != "GetRandomPercent":
            return False
        if c.get("Operator") not in ("Less Than Or Equal To", "Less Than"):
            return False
        value = resolve_comparison_value(c.get("Comparison Value"), glob_values)
        if value is None:
            saw_unresolved = True
            continue
        if value < threshold:
            return False
    return None if saw_unresolved else True


def describe_condition(c: dict, glob_values: dict[str, float]) -> str:
    cmp_value = c.get("Comparison Value")
    if isinstance(cmp_value, dict):
        label = cmp_value.get("editor_id") or cmp_value.get("formid") or "?"
        resolved = resolve_comparison_value(cmp_value, glob_values)
        cmp_text = f"{label} (={resolved:g})" if resolved is not None else f"{label} (unresolved)"
    else:
        cmp_text = cmp_value
    return f"{c.get('Function', '?')} {c.get('Operator', '?')} {cmp_text}"


def entry_reference_text(entry: dict) -> str:
    ref = entry.get("Reference")
    if isinstance(ref, dict):
        return ref.get("editor_id") or ref.get("formid") or "?"
    return "?"


def find_unbounded_gate(entry: dict, glob_values: dict[str, float]) -> dict | None:
    """Rule D — the entry's Condition, if it boils down to a single one-sided
    range check (an Operator from `RANGE_OPS` with no complementary bound on
    the same Function/Parameters within this entry). Groups the entry's
    Conditions by (Function, Parameter 1, Parameter 2, Run On) first, so a
    genuinely bounded range (e.g. `GetLevel >= 10 AND GetLevel < 20`) isn't
    mistaken for one-sided. Returns `None` for an unconditioned entry
    (always eligible — no overlap risk by itself) or one whose conditions
    are already bounded/equality checks."""
    conds = entry_conditions(entry)
    if not conds:
        return None
    groups: dict[tuple, list[dict]] = {}
    for c in conds:
        key = (c.get("Function"), _param_key(c.get("Parameter 1")), _param_key(c.get("Parameter 2")), c.get("Run On"))
        groups.setdefault(key, []).append(c)
    for key, group in groups.items():
        ops = [c.get("Operator") for c in group]
        if any(o in EQ_OPS for o in ops):
            continue
        has_lower = any(o in LOWER_OPS for o in ops)
        has_upper = any(o in UPPER_OPS for o in ops)
        if has_lower and has_upper:
            continue  # bounded within this entry -> no overlap risk from this group
        if has_lower or has_upper:
            rep = next(c for c in group if c.get("Operator") in RANGE_OPS)
            return {
                "function": key[0],
                "operator": rep.get("Operator"),
                "comparison_value": rep.get("Comparison Value"),
                "resolved_value": resolve_comparison_value(rep.get("Comparison Value"), glob_values),
            }
    return None


def _random_percent_prob(operator: str, value: float) -> float:
    """A `GetRandomPercent` gate's true pass probability, given the engine's
    roll is uniform on [0, 100]."""
    if operator in LOWER_OPS:
        return max(0.0, min(1.0, (100.0 - value) / 100.0))
    return max(0.0, min(1.0, value / 100.0))


def gate_entry_probs(entries: list[dict], gates: list[dict | None]) -> list[float] | None:
    """Per-entry pass probability for one roll of the list, or `None` if it
    can't be computed honestly: only meaningful when every gated entry's
    gate is `GetRandomPercent` (the one Condition function that's genuinely
    a uniform 0-100 roll — `GetLevel`/`GetValue`/`GetItemCount` are real
    gates but not probabilities) with a GLOB-resolved threshold."""
    probs = []
    for gate in gates:
        if gate is None:
            probs.append(1.0)  # unconditioned -> always eligible
            continue
        if gate["function"] != "GetRandomPercent" or gate["resolved_value"] is None:
            return None
        probs.append(_random_percent_prob(gate["operator"], gate["resolved_value"]))
    return probs


def compute_pool_odds(probs: list[float]) -> list[float]:
    """Exact pool-then-uniform-pick odds per entry for a single roll under
    the confirmed no-flag algorithm: enumerate every subset of entries whose
    gates currently pass, weight by that subset's joint probability, split
    evenly among the subset's members. O(2^n) — caller caps `len(probs)` via
    `MAX_EXACT_ODDS_ENTRIES` before calling this."""
    n = len(probs)
    odds = [0.0] * n
    for subset_size in range(1, n + 1):
        for subset in combinations(range(n), subset_size):
            subset_set = set(subset)
            p = 1.0
            for i in range(n):
                p *= probs[i] if i in subset_set else (1 - probs[i])
            if p == 0:
                continue
            share = p / subset_size
            for i in subset:
                odds[i] += share
    return odds


def compute_naive_cascade_odds(probs: list[float]) -> list[float]:
    """What a human would read off the list assuming top-to-bottom
    `Use First Match`-style priority — the reading the round thresholds of a
    hand-authored ladder usually invite, for contrast against
    `compute_pool_odds` (what the engine actually does without that flag)."""
    odds = []
    remaining = 1.0
    for p in probs:
        odds.append(remaining * p)
        remaining *= (1 - p)
    return odds


def check_overlap_ladder(entries: list[dict], flags: set[str], glob_values: dict[str, float]) -> dict | None:
    if USE_ALL_FLAG in flags or USE_FIRST_MATCH_FLAG in flags:
        return None
    if len(entries) < 2:
        return None

    gates = [find_unbounded_gate(e, glob_values) for e in entries]
    gated_indices = [i for i, g in enumerate(gates) if g is not None]
    if len(gated_indices) < 2:
        return None

    same_function = len({g["function"] for g in gates if g is not None}) == 1

    odds = None
    if len(entries) <= MAX_EXACT_ODDS_ENTRIES:
        probs = gate_entry_probs(entries, gates)
        if probs is not None:
            odds = {"pool": compute_pool_odds(probs), "naive_cascade": compute_naive_cascade_odds(probs)}

    return {
        "same_function": same_function,
        "entries": [
            {
                "index": i,
                "reference": entry_reference_text(e),
                "gate": (
                    describe_condition(
                        {"Function": g["function"], "Operator": g["operator"], "Comparison Value": g["comparison_value"]},
                        glob_values,
                    )
                    if g is not None
                    else None
                ),
                "pool_odds": odds["pool"][i] if odds else None,
                "naive_cascade_odds": odds["naive_cascade"][i] if odds else None,
            }
            for i, (e, g) in enumerate(zip(entries, gates))
        ],
    }


def check_use_first_starvation(
    entries: list[dict], flags: set[str], threshold: float, glob_values: dict[str, float]
) -> dict | None:
    if USE_FIRST_MATCH_FLAG not in flags:
        return None
    hits = []
    for i, e in enumerate(entries[:-1]):
        conds = entry_conditions(e)
        verdict = is_near_certain(conds, threshold, glob_values)
        if verdict is False:
            continue

        # Level context is supporting evidence only, never an independent
        # trigger: most Use-First-Match lists with real Conditions (e.g.
        # HasLearnedRecipe, GetIsInRegion) are legitimately order-dependent
        # even when ascending in level -- those conditions are consumed or
        # differ per situation, unlike "always/near-always true".
        level_here = resolve_min_level(e, glob_values)
        level_next = resolve_min_level(entries[i + 1], glob_values)
        level_note = (
            f" (Minimum Level {level_here:g} <= next entry's {level_next:g})"
            if level_here is not None and level_next is not None and level_here <= level_next
            else ""
        )

        certainty = "always-true (no Conditions)" if not conds else (
            "near-certain" if verdict else "unknown (GLOB lookup failed, verify manually)"
        )

        hits.append(
            {
                "index": i,
                "reference": entry_reference_text(e),
                "certainty": certainty + level_note,
                "conditions": [describe_condition(c, glob_values) for c in conds],
                "starved_count": len(entries) - i - 1,
            }
        )
    return {"hits": hits} if hits else None


def check_bundle_name_uniform_pick(entries: list[dict], flags: set[str], name_text: str) -> dict | None:
    if USE_ALL_FLAG in flags or USE_FIRST_MATCH_FLAG in flags:
        return None
    if len(entries) < 3:
        return None
    if not BUNDLE_NAME_RE.search(name_text or ""):
        return None
    return {"entry_count": len(entries)}


def check_level_tier_starvation(entries: list[dict], flags: set[str], glob_values: dict[str, float]) -> dict | None:
    if CALC_ALL_LEVELS_FLAG in flags:
        return None
    seen_levels: set[float] = set()
    for e in entries:
        lv = resolve_min_level(e, glob_values)
        if lv is not None:
            seen_levels.add(lv)
    levels = sorted(seen_levels)
    if len(levels) < 2:
        return None
    return {"levels": levels}


def collect_glob_refs(records: list[dict]) -> set[str]:
    """FormIDs of every GLOB referenced either as a `GetRandomPercent`
    Comparison Value or as a `Minimum Level Global`, across every record's
    Leveled List Entries — collected up front so they can all be resolved
    in one bulk_get instead of per-entry lookups."""
    refs: set[str] = set()
    for rec in records:
        fields = rec.get("fields") or {}
        raw_entries = fields.get("Leveled List Entries") or []
        for raw in raw_entries:
            entry = raw.get("Leveled List Entry") or {}
            for c in entry_conditions(entry):
                if c.get("Function") != "GetRandomPercent":
                    continue
                cmp_value = c.get("Comparison Value")
                if isinstance(cmp_value, dict) and cmp_value.get("formid"):
                    refs.add(cmp_value["formid"])
            lvlg = entry.get("Minimum Level Global")
            if isinstance(lvlg, dict) and lvlg.get("formid"):
                refs.add(lvlg["formid"])
    return refs


def analyze_record(rec: dict, threshold: float, glob_values: dict[str, float]) -> dict | None:
    if rec.get("error"):
        return {"error": rec["error"], "sel": rec.get("sel")}

    fields = rec.get("fields") or {}
    flags = set((fields.get("Flags") or {}).get("flags") or [])
    raw_entries = fields.get("Leveled List Entries") or []
    entries = [e.get("Leveled List Entry") or {} for e in raw_entries]
    editor_id = rec.get("editor_id") or "?"
    name = fields.get("Override Name")
    name_text = f"{editor_id} {name or ''}"
    form_id = (rec.get("header") or {}).get("form_id") or rec.get("sel")

    findings = {}
    a = check_use_first_starvation(entries, flags, threshold, glob_values)
    if a:
        findings["A"] = a
    b = check_bundle_name_uniform_pick(entries, flags, name_text)
    if b:
        findings["B"] = b
    c = check_level_tier_starvation(entries, flags, glob_values)
    if c:
        findings["C"] = c
    d = check_overlap_ladder(entries, flags, glob_values)
    if d:
        findings["D"] = d
    if not findings:
        return None
    return {"form_id": form_id, "editor_id": editor_id, "name": name, "findings": findings}


def render_report(hits: list[dict], errors: list[dict], total: int, threshold: float) -> str:
    a_hits = [h for h in hits if "A" in h["findings"]]
    b_hits = [h for h in hits if "B" in h["findings"]]
    c_hits = [h for h in hits if "C" in h["findings"]]
    d_hits = [h for h in hits if "D" in h["findings"]]

    lines = []
    lines.append("# LVLI Leveled-List Health Sweep")
    lines.append("")
    lines.append(f"Scanned **{total}** LVLI records.")
    lines.append(
        f"- Rule A (Use-First-Match order-starvation): **{len(a_hits)}**\n"
        f"- Rule B (bundle-name uniform-pick): **{len(b_hits)}**\n"
        f"- Rule C (suspected level-tier starvation, UNVERIFIED): **{len(c_hits)}**\n"
        f"- Rule D (overlapping-gate reward ladder, no Use All/First): **{len(d_hits)}**"
    )
    if errors:
        lines.append(f"- Records that failed to fetch/decode: **{len(errors)}**")
    lines.append("")

    lines.append("## Rule A — Use-First-Match order-starvation (confirmed mechanism)")
    lines.append("")
    lines.append(
        "`Use First Object That Matches All Conditions` walks entries in list order and "
        "takes the first whose Conditions pass. An entry below is only reachable when every "
        f"entry above it fails its check — flagged here when an early entry has no Conditions "
        f"at all, or a `GetRandomPercent` threshold >= {threshold:g}."
    )
    lines.append("")
    if not a_hits:
        lines.append("_None found._")
    for h in a_hits:
        lines.append(f"### `{h['editor_id']}` ({h['form_id']})")
        for hit in h["findings"]["A"]["hits"]:
            lines.append(
                f"- entry {hit['index']} (`{hit['reference']}`) is {hit['certainty']}"
                f" — starves {hit['starved_count']} entr{'y' if hit['starved_count'] == 1 else 'ies'} below it"
            )
            if hit["conditions"]:
                lines.append(f"  - conditions: {'; '.join(hit['conditions'])}")
    lines.append("")

    lines.append("## Rule B — bundle-name uniform-pick (heuristic)")
    lines.append("")
    lines.append(
        "Multi-entry list with neither `Use All` nor `Use First Match` set — the engine picks "
        "ONE entry uniformly at random and applies its chance-none. Normal for \"pick one of N "
        "variants\" lists; flagged here only because the EditorID/Name suggests the list is "
        "meant to hand out a full set/bundle rather than one item."
    )
    lines.append("")
    if not b_hits:
        lines.append("_None found._")
    for h in b_hits:
        lines.append(f"- `{h['editor_id']}` ({h['form_id']}) — {h['findings']['B']['entry_count']} entries")
    lines.append("")

    lines.append("## Rule C — suspected level-tier starvation (UNVERIFIED for FO76)")
    lines.append("")
    lines.append(
        "**Caveat:** this rule assumes the classic Skyrim/FO4 Creation Engine behavior where, "
        "without `Calculate from all levels <= player's level`, entry selection collapses to "
        "the single highest Minimum Level <= the player's level, silently excluding lower-Level "
        "entries. TES5Edit's FO76 schema defines the flag under the same name but does not "
        "confirm this selection-pool behavior for FO76's engine, and a neighboring flag bit in "
        "the same source is annotated \"Use special formula in skyrim\" — this flag family is "
        "known to vary by game. Treat every hit below as a **lead to verify**, not a confirmed bug."
    )
    lines.append("")
    if not c_hits:
        lines.append("_None found._")
    for h in c_hits:
        levels = ", ".join(str(lv) for lv in h["findings"]["C"]["levels"])
        lines.append(f"- `{h['editor_id']}` ({h['form_id']}) — Minimum Levels: {levels}")
    lines.append("")

    lines.append("## Rule D — overlapping-gate reward ladder (no Use All / Use First)")
    lines.append("")
    lines.append(
        "Two or more entries carry a one-sided range Condition (>=, >, <=, or < with no "
        "complementary bound in the same entry) and neither `Use All` nor `Use First Match` is "
        "set. Under the confirmed no-flag algorithm the engine builds the pool of entries whose "
        "Conditions currently pass and picks ONE uniformly at random — since eligibility isn't "
        "mutually exclusive, entries can overlap on a given roll instead of partitioning it the "
        "way an ordered rarity ladder usually implies. Not a confirmed bug: some hits are "
        "shared-threshold alternate pairs, or level/need-gated variety pools where overlap is "
        "the intended mechanic — `same-function` marks the shape most likely to be a mistake "
        "(every gated entry uses the identical Condition function, the classic hand-authored "
        "tier-ladder look). Odds are computed exactly only when every gate is `GetRandomPercent` "
        "with a resolved threshold and the entry count is <= "
        f"{MAX_EXACT_ODDS_ENTRIES}."
    )
    lines.append("")
    if not d_hits:
        lines.append("_None found._")
    for h in d_hits:
        d = h["findings"]["D"]
        shape = "same-function ladder" if d["same_function"] else "mixed-function overlap"
        lines.append(f"### `{h['editor_id']}` ({h['form_id']}) — {shape}")
        for e in d["entries"]:
            gate_text = e["gate"] or "(unconditioned)"
            if e["pool_odds"] is not None:
                lines.append(
                    f"- entry {e['index']} (`{e['reference']}`) — {gate_text}"
                    f" — naive Use-First read {e['naive_cascade_odds'] * 100:.1f}%,"
                    f" actual pool odds {e['pool_odds'] * 100:.1f}%"
                )
            else:
                lines.append(f"- entry {e['index']} (`{e['reference']}`) — {gate_text}")
    lines.append("")

    if errors:
        lines.append("## Records that failed to fetch/decode")
        lines.append("")
        for e in errors:
            lines.append(f"- `{e.get('sel')}`: {e.get('error')}")
        lines.append("")

    return "\n".join(lines)


def build_arg_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        prog="lvli_audit.py",
        description="Sweep every LVLI record in an ESM for leveled-list selection bugs.",
    )
    ap.add_argument("--esm", default=os.environ.get("FO76_ESM_PATH"), help="Path to the ESM file (default: $FO76_ESM_PATH).")
    ap.add_argument("--esm-bin", help="Path to the esm CLI binary (default: workspace release build, else $PATH).")
    ap.add_argument("--out", help="Write the markdown report here instead of stdout.")
    ap.add_argument(
        "--near-certain-threshold",
        type=float,
        default=DEFAULT_NEAR_CERTAIN_THRESHOLD,
        help=f"GetRandomPercent threshold treated as near-certain-true for rule A (default: {DEFAULT_NEAR_CERTAIN_THRESHOLD:g}).",
    )
    return ap


def main(argv=None) -> int:
    args = build_arg_parser().parse_args(argv)

    if not args.esm:
        eprint("error: --esm is required (or set FO76_ESM_PATH)")
        return 1

    try:
        esm_bin = esm_gateway.find_esm_binary(args.esm_bin)
    except esm_gateway.DaemonError as exc:
        eprint(f"error: {exc}")
        return 1

    esm_path = Path(args.esm)

    client = esm_gateway.ensure_daemon(esm_bin, esm_path)
    try:
        form_ids = list_lvli_form_ids(client, esm_path)
        eprint(f"found {len(form_ids)} LVLI records; fetching...")

        records = client.bulk_get(str(esm_path), form_ids, resolve="stub")

        glob_refs = collect_glob_refs(records)
        glob_values: dict[str, float] = {}
        if glob_refs:
            eprint(f"resolving {len(glob_refs)} GLOB(s) referenced by GetRandomPercent conditions...")
            for glob_rec in client.bulk_get(str(esm_path), sorted(glob_refs), resolve="none"):
                value = (glob_rec.get("fields") or {}).get("Value")
                if not isinstance(value, (int, float)):
                    continue
                formid = (glob_rec.get("header") or {}).get("form_id")
                if formid:
                    glob_values[formid] = value
                edid = glob_rec.get("editor_id")
                if edid:
                    glob_values[edid] = value
    finally:
        client.close()

    hits = []
    errors = []
    for rec in records:
        result = analyze_record(rec, args.near_certain_threshold, glob_values)
        if result is None:
            continue
        if "error" in result:
            errors.append(result)
        else:
            hits.append(result)

    report = render_report(hits, errors, len(records), args.near_certain_threshold)

    if args.out:
        Path(args.out).write_text(report, encoding="utf-8")
        eprint(f"wrote {args.out}")
    else:
        print(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
