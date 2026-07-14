#!/usr/bin/env python3
"""
chase.py — automates the "chase pattern" for unique-weapon OMOD effects
documented under "How unique-weapon effects are implemented (the chase
pattern)" in `../.claude/skills/patch-notes/mechanics-kb.md`. Read that
section first — this script is a mechanical implementation of the walk it
describes, nothing more.

A `mod_Custom_*` (or similarly named) OMOD implements its unique mechanic via
one or more `Data.Properties[]` rows. Each row is classified exactly the way
the KB describes:

    1. Direct property  — the property's `Value 1` is either a plain number
       (a bare stat tweak, nothing to chase further) or a FormID pointing at
       an AVIF (an actor value — chased by reverse `refs` to find who reads
       it, e.g. Bullet Storm/Kill Streak-style readers) or an ENCH/SPEL that
       is attached directly to the weapon (chased by a forward `get`, since
       the effect lives on that record, not behind a keyword gate).
    2. Perk grant        — `Value 1` is a PERK (property 116/"Perks"). Chased
       by a forward `get` on the granted PERK — its `Effects` ARE the
       mechanic.
    3. Keyword hook       — `Value 1` is a KYWD (property 31/"Keywords"). The
       keyword itself carries no behavior; chased by a reverse `refs --type
       SPEL,PERK --paths` walk to find the SPEL/PERK whose Conditions test
       `WornHasKeyword(<keyword>)`, then the exact `Effects[N]` entry gated by
       that condition (located via the `--paths` field path, not a full
       record dump).

Every hop goes through `esm_gateway.EsmGateway` (`bulk_get` for
`Op::RecordBulk`, `refs(..., paths=True, type_filter=...)` for
`Op::ReferencedBy`) against the warm daemon — the same transport
`build_bundles.py`/`run_lints.py` use, auto-spawning/reusing the daemon via
`ensure_daemon` — rather than shelling out to the `esm` CLI per hop. This is
still a prototype ahead of a native `esm chase` subcommand (see
`esm/todos.md` P5); only the transport is shared now, the walk/rendering
logic below is unchanged.

Usage:
    python3 tools/chase/chase.py <OMOD_FORMID_OR_EDID> [--esm PATH] [options]

    OMOD is a FormID (`0x0064D005`) or EditorID (`E08B_mod_Custom_ToneDeath`).
    --esm defaults to $FO76_ESM_PATH if set.

Options:
    --esm PATH        ESM file or data-folder path (default: $FO76_ESM_PATH).
    --esm-bin PATH     Path to the esm binary (default: target/release/esm
                       relative to the esm/ workspace root, or $PATH).
    --depth N          Reverse-ref walk depth for keyword/AVIF consumer
                       lookups (default: 1 — the KB's chase pattern is a
                       single hop; raise this only if a mechanic is gated
                       through an intermediary, e.g. a quest alias).
    --ref-limit N      Cap on refs rows fetched per record-type filter
                       before bulk-fetching consumers (default: 25).
    --json             Emit the evidence tree as JSON instead of text.

Output: a compact evidence tree — one entry per OMOD property, each carrying
just enough to explain *why* it's part of the chain (the property, its
target, and — for keyword/AVIF hops — the exact `Effects[N]` sub-object and
field path that gates on it, not the referencing record's full body).
Optimized for a /patch-notes deep-writer agent: minimal tokens, no full
record dumps.

Limitations (prototype scope — see esm/todos.md P5):
    - Property classification is heuristic (property name / target record
      type), mirroring the KB's own 3-way taxonomy. Properties that don't
      fit any of the three patterns are reported as "direct_property" with
      no further chase (just the raw values) rather than guessed at.
    - Reverse chase defaults to depth=1, matching the KB's description of the
      pattern as a single hop (keyword -> gating SPEL/PERK). Multi-hop gates
      (e.g. via an intermediary quest alias or FLST) need --depth raised
      explicitly; this script does not detect when that's needed.
    - Does not evaluate curve tables or the MUL+ADD formula from the KB
      ("effective = base x (1 + Value1) + Value2") — raw Value1/Value2/
      CurveTable are reported for a human/agent to interpret.
    - Only chases the OMOD-property patterns in the KB. Unique effects
      implemented outside an OMOD (bespoke quest scripts, native engine
      hooks with no ESM-side consumer) are out of scope and will simply show
      up as an empty evidence list — see "no downstream consumer found" in
      the text output.
    - One or two HTTP round-trips per hop (a `bulk_get` and/or a `refs` call);
      relies on the warm daemon for speed, same as the rest of `tools/`.

Python 3, stdlib only (plus the sibling `esm_gateway` module).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
TOOLS_DIR = SCRIPT_DIR.parent  # tools/chase/ -> tools/

sys.path.insert(0, str(TOOLS_DIR))

import esm_gateway as eg  # noqa: E402

# Record types whose Conditions are checked for a WornHasKeyword (or similar)
# gate on a keyword/AVIF this OMOD ADDs. Mirrors the KB's "SPEL/PERK effect
# conditioned on WornHasKeyword(...)" — these are the only two record types
# the chase pattern names as mechanic carriers.
CONSUMER_TYPES = ("SPEL", "PERK")

# Target record types whose own Effects/Description are pulled directly
# (forward `get`) because the OMOD property attaches them straight to the
# weapon rather than gating them behind a keyword condition.
FORWARD_FETCH_TYPES = ("PERK", "ENCH", "SPEL")

DEFAULT_DEPTH = 1
DEFAULT_REF_LIMIT = 25


class ChaseError(Exception):
    """Raised for an `EsmGateway` transport failure or malformed output."""


# ─── EsmGateway transport (bulk_get / refs) ──────────────────────────────────


def esm_get_bulk(gateway: eg.EsmGateway, esm_path: str, targets: list[str]) -> list[dict]:
    """Fetch one or more records with `--resolve stub` in a single bulk
    round-trip via `EsmGateway.bulk_get` (`Op::RecordBulk`).

    `Op::RecordBulk` isolates a bad selector to its own `{"sel", "error"}`
    entry regardless of how many selectors are requested (see ipc.rs's
    `bulk_record_entry`), so — unlike the old subprocess-based single-`get`
    CLI path, which failed the whole process on one bad selector — no
    single-vs-multi-target special case is needed here any more.
    """
    if not targets:
        return []
    try:
        return gateway.bulk_get(esm_path, targets, resolve="stub")
    except eg.DaemonError as exc:
        raise ChaseError(f"bulk get failed for {targets}: {exc}") from exc


def esm_refs(
    gateway: eg.EsmGateway,
    esm_path: str,
    target_formid: str,
    *,
    record_type: str,
    depth: int,
    limit: int,
) -> list[dict]:
    """`EsmGateway.refs(..., type_filter=record_type, paths=True)` -- the
    daemon-side equivalent of `esm refs <esm> <target> --type <record_type>
    --paths --depth --limit --json` (see cli.rs's `cmd_refs`, whose `-p`
    path sends the exact same `Op::ReferencedBy` fields).

    Returns the raw list of RefRow dicts (empty list if nothing matches —
    an empty result is not an error, see cli.rs's `print_refs`)."""
    try:
        result = gateway.refs(
            esm_path, target_formid, depth=depth, limit=limit,
            type_filter=record_type, paths=True,
        )
    except eg.DaemonError as exc:
        raise ChaseError(
            f"refs failed for {target_formid} (type={record_type}): {exc}"
        ) from exc
    return result.get("rows") or []


# ─── schema helpers ──────────────────────────────────────────────────────────


def _named(field: Any) -> Any:
    """Extract the human name from a `{"value":.., "name":..}` schema enum
    dict, or return the value unchanged if it isn't wrapped that way."""
    if isinstance(field, dict) and "name" in field:
        return field["name"]
    return field


def _is_formid_stub(value: Any) -> bool:
    return isinstance(value, dict) and "formid" in value


# `collect_formid_paths` (src/lib.rs) builds paths as dot-joined JSON object
# keys with array indices appended directly to the preceding key, e.g.
# "Effects[1].Effect.Conditions.Conditions[0].Condition.Condition Data.Parameter 1".
# Key names may contain spaces but never dots or brackets, so splitting on
# "." is safe.
_BRACKET_RE = re.compile(r"\[(\d+)\]")
_TOKEN_RE = re.compile(r"^([^\[]*)((?:\[\d+\])*)$")


def _first_array_container(path: str) -> str | None:
    """Return the path prefix up to and including the first `[N]` index,
    e.g. `"Effects[1].Effect.Conditions..."` -> `"Effects[1]"`. This isolates
    the one Effects entry a keyword/AVIF gates, instead of the whole record."""
    prefix: list[str] = []
    for part in path.split("."):
        prefix.append(part)
        if _BRACKET_RE.search(part):
            return ".".join(prefix)
    return None


def _walk_path(fields: dict, path: str) -> Any:
    """Descend into a decoded record's `fields` dict along a dot/`[N]` path."""
    cur: Any = fields
    for part in path.split("."):
        m = _TOKEN_RE.match(part)
        if not m:
            return None
        key, brackets = m.group(1), m.group(2)
        if key:
            if not isinstance(cur, dict) or key not in cur:
                return None
            cur = cur[key]
        for idx_str in _BRACKET_RE.findall(brackets):
            idx = int(idx_str)
            if not isinstance(cur, list) or idx >= len(cur):
                return None
            cur = cur[idx]
    return cur


def _slice_effect(fields: dict, path: str) -> Any:
    container = _first_array_container(path)
    if container is None:
        return None
    return _walk_path(fields, container)


def _extract_conditions(obj: Any, acc: list[str] | None = None) -> list[str]:
    """Recursively find every `Condition Data`-shaped dict (has both
    `Function` and `Operator`) inside `obj` and render it compactly.
    SPEL and PERK nest conditions differently (`Conditions.Conditions[]` vs
    `Perk Conditions[].Perk Condition.Conditions[]`) — this walks either."""
    if acc is None:
        acc = []
    if isinstance(obj, dict):
        if "Function" in obj and "Operator" in obj:
            fn = obj.get("Function")
            op = obj.get("Operator")
            val = obj.get("Comparison Value")
            param = obj.get("Parameter 1")
            if isinstance(param, dict):
                param_txt = param.get("editor_id") or param.get("formid")
                acc.append(f"{fn}({param_txt}) {op} {val}")
            elif param is not None:
                acc.append(f"{fn}({param}) {op} {val}")
            else:
                acc.append(f"{fn} {op} {val}")
        else:
            for v in obj.values():
                _extract_conditions(v, acc)
    elif isinstance(obj, list):
        for v in obj:
            _extract_conditions(v, acc)
    return acc


# Keys already surfaced explicitly by _summarize_effect — anything else on
# the effect dict is scanned generically (see the loop below) so fields like
# PERK's "Function Parameter 3 (Actor Value)" (e.g. the AVIF a Kill-Streak-
# style perk reads) aren't silently dropped just because they're not one of
# the handful of well-known keys.
_HANDLED_EFFECT_KEYS = {
    "Base Effect",
    "Entry Point",
    "Effect Item Data",
    "Float",
    "Conditions",
    "Perk Conditions",
    "Effect Header",
    "Effect Flags",
    "Cooldown Duration",
    "Effect End",
}


def _summarize_effect(effect_entry: Any) -> str:
    """Render one `Effects[]` element (`{"Effect": {...}}`, SPEL or PERK
    shape) as a single compact line: base effect / entry point, magnitude,
    duration, any other FormID reference on the effect (e.g. the actor value
    a perk's function operates on), and any gating conditions found anywhere
    inside it."""
    if not isinstance(effect_entry, dict):
        return json.dumps(effect_entry)[:200]
    inner = effect_entry.get("Effect", effect_entry)
    if not isinstance(inner, dict):
        return json.dumps(effect_entry)[:200]

    parts: list[str] = []
    base = inner.get("Base Effect")
    if isinstance(base, dict):
        parts.append(str(base.get("editor_id") or base.get("formid")))

    entry_point = inner.get("Entry Point")
    if isinstance(entry_point, dict):
        ep_name = _named(entry_point.get("Entry Point"))
        fn_name = _named(entry_point.get("Function"))
        if ep_name:
            parts.append(f"{ep_name}/{fn_name}" if fn_name else str(ep_name))

    item_data = inner.get("Effect Item Data")
    if isinstance(item_data, dict):
        if item_data.get("Magnitude") is not None:
            parts.append(f"Magnitude={item_data['Magnitude']}")
        if item_data.get("Duration"):
            parts.append(f"Duration={item_data['Duration']}")

    if "Float" in inner:
        parts.append(f"Float={inner['Float']}")

    # Generic pass: any other FormID-stub field on the effect itself (not
    # nested under Conditions, handled separately below) — e.g. a PERK's
    # "Function Parameter N (Actor Value)" pointing at an AVIF.
    for key, val in inner.items():
        if key in _HANDLED_EFFECT_KEYS:
            continue
        if isinstance(val, dict) and "formid" in val:
            parts.append(f"{key}={val.get('editor_id') or val.get('formid')}")

    text = "  ".join(parts)
    conditions = _extract_conditions(inner.get("Conditions") or inner.get("Perk Conditions"))
    if conditions:
        text += ("  " if text else "") + "Conditions: " + "; ".join(conditions)
    return text or json.dumps(inner)[:200]


# ─── the chase ───────────────────────────────────────────────────────────────


def _stub(row_or_target: dict) -> dict:
    return {
        "formid": row_or_target.get("formid") or row_or_target.get("form_id"),
        "editor_id": row_or_target.get("editor_id"),
        "record_type": row_or_target.get("record_type"),
    }


def _forward_evidence(target: dict, entries_by_sel: dict) -> dict:
    entry = entries_by_sel.get(target["formid"])
    if entry is None or "error" in entry:
        err = entry.get("error", "no response") if entry else "no response"
        return {"source": _stub(target), "via": None, "detail": {"note": f"fetch failed: {err}"}}
    fields = entry.get("fields") or {}
    detail: dict = {}
    if fields.get("Description"):
        detail["description"] = fields["Description"]
    effects = fields.get("Effects")
    if isinstance(effects, list) and effects:
        capped = effects[:12]
        detail["effects"] = capped
        if len(effects) > len(capped):
            detail["effects_truncated"] = len(effects) - len(capped)
    if not detail:
        detail["note"] = "no Description/Effects field on this record"
    return {"source": _stub(target), "via": None, "detail": detail}


def _reverse_chase(
    gateway: eg.EsmGateway, esm_path: str, target: dict, *, depth: int, limit: int
) -> list[dict]:
    """Reverse `refs --type SPEL` + `--type PERK` walk on `target` (a keyword
    or AVIF), then a single bulk `get` for every distinct consumer found,
    slicing out just the `Effects[N]` entry each `--paths` field path points
    at (see module docstring, pattern 1/3)."""
    rows: list[dict] = []
    for record_type in CONSUMER_TYPES:
        rows.extend(
            esm_refs(
                gateway, esm_path, target["formid"], record_type=record_type, depth=depth, limit=limit
            )
        )
    if not rows:
        return []

    ids = sorted({row["form_id"] for row in rows})
    fetched = esm_get_bulk(gateway, esm_path, ids)
    by_sel = {e.get("sel"): e for e in fetched}

    evidence = []
    for row in rows:
        entry = by_sel.get(row["form_id"], {})
        fields = entry.get("fields") or {}
        paths = row.get("field_paths") or [None]
        for path in paths:
            sliced = _slice_effect(fields, path) if path else None
            if sliced is not None:
                detail = {"effect": sliced}
            else:
                detail = {
                    "note": "reference confirmed but the exact effect could not be "
                    "isolated from the field path; inspect the full record"
                }
            item = {
                "source": _stub(row),
                "via": path,
                "detail": detail,
            }
            if row.get("depth", 1) > 1:
                item["hop_depth"] = row["depth"]
                item["path_chain"] = row.get("path")
            evidence.append(item)
    return evidence


def chase(
    gateway: eg.EsmGateway,
    esm_path: str,
    omod_selector: str,
    *,
    depth: int = DEFAULT_DEPTH,
    ref_limit: int = DEFAULT_REF_LIMIT,
) -> dict:
    """Run the full chase for one OMOD selector and return the evidence tree.

    `gateway` is anything implementing `EsmGateway`'s `bulk_get()`/`refs()`
    surface -- normally a live warm-daemon `EsmGateway` (see
    `esm_gateway.ensure_daemon`, wired up by `main()`), or a `FakeGateway`
    for tests (see `tests/test_chase.py`)."""
    entries = esm_get_bulk(gateway, esm_path, [omod_selector])
    entry = entries[0]
    if "error" in entry:
        raise ChaseError(f"failed to resolve {omod_selector!r}: {entry['error']}")

    fields = entry.get("fields") or {}
    record_type = fields.get("_record_type")
    if record_type != "Object Modification":
        header = entry.get("header") or {}
        got = record_type or header.get("signature") or "unknown"
        raise ChaseError(
            f"{omod_selector!r} resolves to a {got!r} record, not an OMOD "
            "(Object Modification) — chase only supports OMOD input"
        )

    header = entry.get("header") or {}
    omod_stub = {
        "formid": header.get("form_id"),
        "editor_id": entry.get("editor_id"),
        "name": fields.get("Name") or None,
        "description": fields.get("Description") or None,
    }

    properties = ((fields.get("Data") or {}).get("Properties")) or []

    hops: list[dict] = []
    forward_targets: list[tuple[int, dict]] = []  # (hop index, target stub)
    reverse_targets: list[tuple[int, dict]] = []

    for i, prop in enumerate(properties):
        prop_name = _named(prop.get("Property"))
        function = _named(prop.get("Function Type"))
        value1 = prop.get("Value 1")
        value2 = prop.get("Value 2")
        curve_table = prop.get("Curve Table")

        hop: dict = {
            "property_index": i,
            "property": prop_name,
            "function": function,
            "value1": value1,
            "value2": value2,
        }
        if curve_table:
            hop["curve_table"] = curve_table

        if not _is_formid_stub(value1):
            hop["kind"] = "direct_property"
            hop["evidence"] = []
            hops.append(hop)
            continue

        target = _stub(value1)
        hop["target"] = target
        rt = target["record_type"]

        if rt == "KYWD":
            hop["kind"] = "keyword_hook"
            reverse_targets.append((i, target))
        elif rt == "PERK":
            hop["kind"] = "perk_grant"
            forward_targets.append((i, target))
        elif rt in FORWARD_FETCH_TYPES:  # ENCH / SPEL attached directly (not via "Perks")
            hop["kind"] = "direct_property"
            forward_targets.append((i, target))
        elif rt == "AVIF":
            hop["kind"] = "direct_property"
            reverse_targets.append((i, target))
        else:
            hop["kind"] = "direct_property"
            hop["evidence"] = []

        hops.append(hop)

    # ---- forward fetch (perk_grant + direct ENCH/SPEL attachments): 1 bulk call ----
    if forward_targets:
        fetched = esm_get_bulk(gateway, esm_path, [t["formid"] for _, t in forward_targets])
        by_sel = {e.get("sel"): e for e in fetched}
        for i, target in forward_targets:
            hops[i]["evidence"] = [_forward_evidence(target, by_sel)]

    # ---- reverse chase (keyword_hook + AVIF consumer lookup) ----
    for i, target in reverse_targets:
        hops[i]["evidence"] = _reverse_chase(gateway, esm_path, target, depth=depth, limit=ref_limit)

    return {"omod": omod_stub, "hops": hops}


# ─── rendering ───────────────────────────────────────────────────────────────


def _fmt_stub(stub: dict) -> str:
    rt = stub.get("record_type") or "?"
    fid = stub.get("formid") or "?"
    edid = stub.get("editor_id") or ""
    return f"{rt} {fid} {edid}".rstrip()


def render_text(tree: dict) -> str:
    lines: list[str] = []
    omod = tree["omod"]
    header = f"OMOD {omod['formid']} {omod['editor_id']}"
    if omod.get("name"):
        header += f'  "{omod["name"]}"'
    lines.append(header)
    if omod.get("description"):
        lines.append(f'  Description: "{omod["description"]}"')

    if not tree["hops"]:
        lines.append("  (no Properties on this OMOD — nothing to chase)")
        return "\n".join(lines)

    dead_end_kinds = {"keyword_hook"}
    for hop in tree["hops"]:
        lines.append("")
        lines.append(f"  [{hop['property_index']}] {hop['property']} {hop['function']} ({hop['kind']})")
        if hop.get("target"):
            lines.append(f"      -> {_fmt_stub(hop['target'])}")
        else:
            lines.append(f"      value1={hop.get('value1')!r} value2={hop.get('value2')!r}")
        if hop.get("curve_table"):
            lines.append(f"      curve_table={hop['curve_table']}")

        evidence = hop.get("evidence") or []
        is_avif = hop.get("target", {}).get("record_type") == "AVIF"
        if not evidence:
            if hop["kind"] in dead_end_kinds or is_avif:
                lines.append(
                    "      (no SPEL/PERK condition references this target — dead end; "
                    "may be UI-only, native-engine-consumed, or a shared/common tag)"
                )
            continue

        for ev in evidence:
            via = f"  via {ev['via']}" if ev.get("via") else ""
            lines.append(f"      -> {_fmt_stub(ev['source'])}{via}")
            detail = ev.get("detail") or {}
            if detail.get("description"):
                lines.append(f'         Description: "{detail["description"]}"')
            if "effect" in detail:
                lines.append(f"         Effect: {_summarize_effect(detail['effect'])}")
            if "effects" in detail:
                for eff in detail["effects"]:
                    lines.append(f"         Effect: {_summarize_effect(eff)}")
                if detail.get("effects_truncated"):
                    lines.append(f"         ... +{detail['effects_truncated']} more effects (truncated)")
            if "note" in detail:
                lines.append(f"         Note: {detail['note']}")

    return "\n".join(lines)


# ─── CLI ─────────────────────────────────────────────────────────────────────


def build_arg_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        description="Automate the unique-weapon OMOD chase pattern (see "
        "../.claude/skills/patch-notes/mechanics-kb.md) over the esm CLI.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    ap.add_argument("omod", help="OMOD FormID (0x...) or EditorID to chase")
    ap.add_argument(
        "--esm",
        default=os.environ.get("FO76_ESM_PATH"),
        metavar="PATH",
        help="ESM file or data-folder path (default: $FO76_ESM_PATH)",
    )
    ap.add_argument("--esm-bin", default=None, metavar="PATH", help="Path to the esm binary")
    ap.add_argument(
        "--depth",
        type=int,
        default=DEFAULT_DEPTH,
        metavar="N",
        help=f"Reverse-ref walk depth for keyword/AVIF consumer lookups (default: {DEFAULT_DEPTH})",
    )
    ap.add_argument(
        "--ref-limit",
        type=int,
        default=DEFAULT_REF_LIMIT,
        metavar="N",
        help=f"Cap on refs rows per record-type filter (default: {DEFAULT_REF_LIMIT})",
    )
    ap.add_argument("--json", action="store_true", help="Emit the evidence tree as JSON")
    return ap


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)

    if not args.esm:
        print("error: --esm is required (or set $FO76_ESM_PATH)", file=sys.stderr)
        return 2

    try:
        esm_bin = eg.find_esm_binary(args.esm_bin)
        gateway = eg.ensure_daemon(esm_bin, args.esm)
    except eg.DaemonError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    try:
        tree = chase(gateway, args.esm, args.omod, depth=args.depth, ref_limit=args.ref_limit)
    except ChaseError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    finally:
        gateway.close()

    if args.json:
        print(json.dumps(tree, indent=2))
    else:
        print(render_text(tree))
    return 0


if __name__ == "__main__":
    sys.exit(main())
