#!/usr/bin/env python3
"""
check_coverage.py — DEEP-tier coverage gate for the FO76 patch-notes
pipeline: every DEEP bundle is accounted for by id, never by hope.

    python3 pn/check_coverage.py <out_dir> [--summary]

Inputs: `work/triage.json` (the DEEP id list), `work/deep-slice.json`
(each bundle's anchor and members), every `drafts/deep[.partN].report.json`
(`bundles_covered: [id, ...]` and `deferred: [{"form_ids", ...}]`) with its
paired draft `.md`, and -- with `--summary` -- `patch-summary.md` plus the
optional `work/cuts.json` (`{"cuts": [{"bundle_id", "reason"}]}`).

Rules, per DEEP bundle id:
  * exactly one report lists it in `bundles_covered`, and that report's
    draft text mentions the bundle's anchor (its name, EditorID, or FormID
    -- FormIDs live in Evidence lines per the style guide);
  * or one report defers it (a `deferred[]` entry naming one of its
    FormIDs) AND another report covers it as above;
  * with `--summary`: the anchor also appears in `patch-summary.md`, unless
    `work/cuts.json` lists the id with a non-empty reason.
Two reports covering one id is an error (duplicate stories).

Writes `<out_dir>/work/coverage.json`; exits 1 on any violation. With no
reports on disk every DEEP id is a violation -- the expected state before
the deep pass has run.

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import layout  # noqa: E402

_FORMID_RE = re.compile(r"^0x[0-9A-Fa-f]{1,8}$")


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def _load(path: Path) -> Any:
    with path.open(encoding="utf-8") as f:
        return json.load(f)


def canon_formid(value: Any) -> str | None:
    if isinstance(value, str) and _FORMID_RE.match(value.strip()):
        return f"0x{int(value.strip(), 16):08X}"
    return None


def anchor_terms(bundle: dict) -> list[tuple[str, str]]:
    """[(kind, text)] a draft may use to name this bundle: the anchor's
    display name, EditorID, and FormID."""
    anchor = bundle.get("anchor") or {}
    terms: list[tuple[str, str]] = []
    if anchor.get("name"):
        terms.append(("name", anchor["name"]))
    if anchor.get("editor_id"):
        terms.append(("editor_id", anchor["editor_id"]))
    if anchor.get("form_id"):
        terms.append(("form_id", anchor["form_id"]))
    return terms


def mentions(text: str, terms: list[tuple[str, str]]) -> str | None:
    """The first anchor term found in `text` (names and FormIDs
    case-insensitively, EditorIDs exactly), or None."""
    lowered = text.lower()
    for kind, term in terms:
        if kind == "editor_id":
            if term in text:
                return term
        elif term.lower() in lowered:
            return term
    return None


def load_reports(out_dir: Path) -> list[dict]:
    reports = []
    for path in layout.drafts_deep_reports(out_dir):
        data = _load(path)
        draft_path = layout.draft_md_for_report(path)
        covered = data.get("bundles_covered") if isinstance(data, dict) else None
        deferred = data.get("deferred") if isinstance(data, dict) else None
        reports.append(
            {
                "name": path.name,
                "covered": [c for c in (covered or []) if isinstance(c, str)],
                "deferred": [d for d in (deferred or []) if isinstance(d, dict)],
                "text": draft_path.read_text(encoding="utf-8") if draft_path.is_file() else "",
                "has_covered_list": isinstance(covered, list),
            }
        )
    return reports


def load_cuts(out_dir: Path) -> dict[str, str]:
    path = layout.work_cuts_json(out_dir)
    if not path.is_file():
        return {}
    data = _load(path)
    out: dict[str, str] = {}
    for cut in (data.get("cuts") or []) if isinstance(data, dict) else []:
        if isinstance(cut, dict) and isinstance(cut.get("bundle_id"), str):
            out[cut["bundle_id"]] = str(cut.get("reason") or "").strip()
    return out


def run_check(out_dir: Path, summary: bool = False) -> dict:
    triage = _load(layout.work_triage_json(out_dir))
    deep_ids: list[str] = sorted(triage.get("deep") or [])
    slice_payload = _load(layout.work_deep_slice_json(out_dir))
    bundles_by_id = {b["id"]: b for b in (slice_payload.get("bundles") or []) if isinstance(b, dict) and b.get("id")}
    bundle_by_fid: dict[str, str] = {}
    for bid, b in bundles_by_id.items():
        for m in b.get("members") or []:
            c = canon_formid((m or {}).get("form_id"))
            if c:
                bundle_by_fid.setdefault(c, bid)
        c = canon_formid((b.get("anchor") or {}).get("form_id"))
        if c:
            bundle_by_fid[c] = bid

    reports = load_reports(out_dir)
    covering: dict[str, list[dict]] = {bid: [] for bid in deep_ids}
    for r in reports:
        for bid in r["covered"]:
            if bid in covering:
                covering[bid].append(r)
    deferrals: dict[str, list[str]] = {bid: [] for bid in deep_ids}
    for r in reports:
        for d in r["deferred"]:
            for fid in d.get("form_ids") or []:
                bid = bundle_by_fid.get(canon_formid(fid) or "")
                if bid is None or bid not in deferrals:
                    continue
                if r["name"] not in deferrals[bid]:
                    deferrals[bid].append(r["name"])

    cuts = load_cuts(out_dir) if summary else {}
    summary_text = ""
    if summary:
        sp = layout.patch_summary_md(out_dir)
        summary_text = sp.read_text(encoding="utf-8") if sp.is_file() else ""

    violations: list[dict] = []
    covered_by: dict[str, str] = {}
    deferred_resolved: dict[str, dict] = {}
    cut: dict[str, str] = {}
    unknown_bundles: list[str] = []

    for bid in deep_ids:
        bundle = bundles_by_id.get(bid)
        if bundle is None:
            unknown_bundles.append(bid)
            violations.append({"bundle_id": bid, "kind": "not_in_slice", "detail": "DEEP id missing from deep-slice.json"})
            continue
        terms = anchor_terms(bundle)
        owners = covering[bid]
        if len(owners) > 1:
            violations.append({
                "bundle_id": bid, "kind": "double_covered",
                "detail": f"covered by {', '.join(o['name'] for o in owners)}; one story per draft",
            })
            continue
        if not owners:
            if deferrals[bid]:
                violations.append({
                    "bundle_id": bid, "kind": "deferred_uncovered",
                    "detail": f"deferred by {', '.join(deferrals[bid])} but no draft covers it",
                })
            else:
                violations.append({"bundle_id": bid, "kind": "uncovered", "detail": "no draft lists it in bundles_covered"})
            continue
        owner = owners[0]
        hit = mentions(owner["text"], terms)
        if hit is None:
            violations.append({
                "bundle_id": bid, "kind": "anchor_missing",
                "detail": f"{owner['name']} claims it but its draft never names {', '.join(t for _, t in terms) or '(no anchor terms)'}",
            })
            continue
        covered_by[bid] = owner["name"]
        if deferrals[bid]:
            deferred_resolved[bid] = {"deferred_by": deferrals[bid], "covered_by": owner["name"]}
        if summary:
            if mentions(summary_text, terms) is None:
                reason = cuts.get(bid, "")
                if bid in cuts and reason:
                    cut[bid] = reason
                else:
                    violations.append({
                        "bundle_id": bid, "kind": "missing_from_summary",
                        "detail": f"not in patch-summary.md and not cut with a reason in work/cuts.json ({', '.join(t for _, t in terms)})",
                    })

    for r in reports:
        if not r["has_covered_list"]:
            violations.append({"bundle_id": None, "kind": "report_without_bundles_covered", "detail": f"{r['name']} has no bundles_covered list"})

    payload = {
        "schema_version": 1,
        "summary_checked": summary,
        "deep_total": len(deep_ids),
        "reports": [r["name"] for r in reports],
        "covered": covered_by,
        "deferred_resolved": deferred_resolved,
        "cut": cut,
        "violations": violations,
        "ok": not violations,
    }
    layout.work_dir(out_dir).mkdir(parents=True, exist_ok=True)
    with layout.work_coverage_json(out_dir).open("w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2, ensure_ascii=False)
        f.write("\n")
    return payload


def print_summary(payload: dict, stream=sys.stderr):
    print(
        f"DEEP bundles: {payload['deep_total']}; covered {len(payload['covered'])}; "
        f"deferred+covered {len(payload['deferred_resolved'])}; cut {len(payload['cut'])}; "
        f"violations {len(payload['violations'])}",
        file=stream,
    )
    for v in payload["violations"]:
        print(f"  {v['kind']} {v['bundle_id'] or ''}: {v['detail']}", file=stream)


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="check_coverage.py",
        description="Assert every DEEP bundle is covered by exactly one draft (or deferred to one), "
                    "and -- with --summary -- reaches patch-summary.md or is cut with a reason.",
    )
    ap.add_argument("out_dir", type=Path, help="Pipeline output directory")
    ap.add_argument("--summary", action="store_true", help="Also check patch-summary.md against work/cuts.json")
    return ap


def main(argv=None) -> int:
    args = build_arg_parser().parse_args(argv)
    payload = run_check(args.out_dir, summary=args.summary)
    print_summary(payload)
    eprint(f"wrote {layout.work_coverage_json(args.out_dir)}")
    return 0 if payload["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
