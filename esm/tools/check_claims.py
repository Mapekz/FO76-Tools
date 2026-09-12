#!/usr/bin/env python3
"""
check_claims.py — machine verification of every number a deep writer
asserted, for the FO76 patch-notes pipeline.

Deep-writer subagents list each figure they state in the draft as a
structured claim in `drafts/deep[.partN].report.json` (`claims: [...]`, see
`patchnotes_lib.Claim`). This script re-derives every claim from the data:

    python3 tools/check_claims.py <out_dir> [--old-esm P --new-esm P]
                                            [--esm-bin P] [--no-daemon]

Claim kinds (exactly one per claim):
    changed    {"record", "path", "from", "to"}
    existence  {"record", "status": "added"|"removed"}
    value      {"record", "path", "value", "side": "old"|"new"}

`record` is a FormID (hex) or an EditorID (renames resolve through the
record's `prev_editor_id`). `path` is `comprehensive.json`'s ChangeEntry
notation: `" / "`-joined field names, with an array row addressed by its
`key_display` in brackets (`Effects / [Effect=0x0004B2E1] / Magnitude`); a
claim naming an array entry itself compares `from`/`to` against the row
counts.

Verification order per claim: the record's `changes[]` in
`comprehensive.json` (what the writer read via `slice_bundles.py --extract`)
first; a live `EsmGateway.bulk_get` against the OLD or NEW snapshot second
(only when both ESM paths were given and `--no-daemon` is absent); else
`unverifiable`.

Writes `<out_dir>/work/claims-check.json` and exits 1 iff any claim is a
`mismatch` or `unverifiable` -- an unverifiable number is a number the post
cannot stand behind. Numbers in the draft prose that no claim backs are
listed as warnings only (`unbacked_numbers`); the writer prompt requires a
claim for every stated figure, so a long list means the prompt drifted.

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import esm_gateway as eg  # noqa: E402
import layout  # noqa: E402
import patchnotes_lib as pl  # noqa: E402
from triage_bundles import _numeric_value  # noqa: E402

PATH_SEP = " / "
REL_TOL = 1e-6
_ROW_RE = re.compile(r"^\[(.*)\]$")
_FORMID_RE = re.compile(r"^0x[0-9A-Fa-f]{1,8}$")
_MIN_INTERESTING_INT = 10


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


# --------------------------------------------------------------------------
# Selectors / paths
# --------------------------------------------------------------------------


def canon_formid(value: Any) -> str | None:
    """`"0x00568635"` for anything FormID-shaped (int, `0x` hex string,
    resolved stub dict), else None."""
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return f"0x{value:08X}"
    if isinstance(value, dict):
        return canon_formid(value.get("form_id"))
    if isinstance(value, str) and _FORMID_RE.match(value.strip()):
        return f"0x{int(value.strip(), 16):08X}"
    return None


def split_path(path: str) -> list[str]:
    return [seg.strip() for seg in str(path).split(PATH_SEP) if seg.strip()]


def _norm_key(text: str) -> str:
    return re.sub(r"\s+", " ", str(text)).strip().lower()


class RecordIndex:
    """Lookup of `comprehensive.json` records by FormID, EditorID, or the
    pre-rename EditorID."""

    def __init__(self, records: dict[str, Any]):
        self.by_fid: dict[str, dict] = {}
        self.by_edid: dict[str, dict] = {}
        self.by_prev: dict[str, dict] = {}
        for fid, rec in records.items():
            c = canon_formid(fid) or canon_formid(rec.get("form_id"))
            if c:
                self.by_fid[c] = rec
            if rec.get("editor_id"):
                self.by_edid[rec["editor_id"]] = rec
            if rec.get("prev_editor_id"):
                self.by_prev[rec["prev_editor_id"]] = rec

    def get(self, selector: str) -> dict | None:
        c = canon_formid(selector)
        if c:
            return self.by_fid.get(c)
        return self.by_edid.get(selector) or self.by_prev.get(selector)


# --------------------------------------------------------------------------
# Value comparison
# --------------------------------------------------------------------------


def _strip_display(text: Any) -> str | None:
    if not isinstance(text, str):
        return None
    return text.strip().strip("`").strip()


def values_match(claimed: Any, actual: Any, display: Any = None) -> bool:
    """Tolerant equality between a writer's claimed value and the data's.
    Numbers compare within REL_TOL; FormID references match on the raw hex,
    the resolved stub's form_id/editor_id, or the rendered display; strings
    compare case-insensitively after trimming; lists pairwise; a claimed
    dict must be a subset of the actual one."""
    if isinstance(claimed, bool) or isinstance(actual, bool):
        return isinstance(claimed, bool) and isinstance(actual, bool) and claimed == actual
    nc, na = _numeric_value(claimed), _numeric_value(actual)
    if nc is not None and na is not None:
        return math.isclose(nc, na, rel_tol=REL_TOL, abs_tol=REL_TOL)
    cf, af = canon_formid(claimed), canon_formid(actual)
    if cf and af:
        return cf == af
    if isinstance(actual, dict):
        if isinstance(claimed, str):
            edid = actual.get("editor_id")
            if edid and claimed.strip() == edid:
                return True
        if isinstance(claimed, dict):
            return all(values_match(v, actual.get(k)) for k, v in claimed.items())
    if isinstance(claimed, list) and isinstance(actual, list):
        return len(claimed) == len(actual) and all(values_match(c, a) for c, a in zip(claimed, actual))
    if claimed is None or actual is None:
        return claimed is None and actual is None
    if isinstance(claimed, str) and isinstance(actual, str):
        return _norm_key(claimed) == _norm_key(actual)
    shown = _strip_display(display)
    if shown is not None and isinstance(claimed, (str, int, float)):
        return _norm_key(str(claimed)) == _norm_key(shown)
    return claimed == actual


def _short(value: Any, limit: int = 80) -> str:
    text = json.dumps(value, ensure_ascii=False, default=str)
    return text if len(text) <= limit else text[: limit - 1] + "…"


# --------------------------------------------------------------------------
# ChangeEntry walking (comprehensive.json)
# --------------------------------------------------------------------------


def _match_row(rows: list[dict], key: str, strategy: str | None) -> dict | None:
    """Find an array-diff row by its `key_display` (exact, then
    whitespace/case-normalized); a bare integer addresses the Nth row only
    for positional diffs."""
    for row in rows:
        if row.get("key_display") == key:
            return row
    nk = _norm_key(key)
    for row in rows:
        if _norm_key(row.get("key_display") or "") == nk:
            return row
    if key.isdigit() and strategy == "positional":
        n = int(key)
        if 0 <= n < len(rows):
            return rows[n]
    return None


def resolve_change(entries: list[dict], segs: list[str]) -> tuple[dict | None, str | None]:
    """Return (ChangeEntry, None) for the entry `segs` addresses within
    `entries`, descending through array rows; or (None, reason)."""
    candidates = sorted(
        (e for e in entries if isinstance(e, dict) and e.get("path")),
        key=lambda e: -len(split_path(e["path"])),
    )
    for entry in candidates:
        ep = split_path(entry["path"])
        if segs[: len(ep)] != ep:
            continue
        rest = segs[len(ep):]
        if not rest:
            return entry, None
        if entry.get("kind") != "array":
            continue
        m = _ROW_RE.match(rest[0])
        if not m:
            return None, f"'{rest[0]}' is not an array row address ([<key_display>]) under {entry['path']}"
        array = entry.get("array") or {}
        row = _match_row(array.get("changed") or [], m.group(1), array.get("strategy"))
        if row is None:
            known = [r.get("key_display") for r in (array.get("changed") or [])]
            return None, f"no changed row '{m.group(1)}' under {entry['path']}; rows: {_short(known)}"
        if array.get("strategy") in ("set", "unkeyed") and not rest[1:]:
            return None, f"{entry['path']} is a reordered/unkeyed array; cite its counts, not a row"
        if not rest[1:]:
            return None, f"row '{m.group(1)}' under {entry['path']} needs a field after it"
        return resolve_change(row.get("changes") or [], rest[1:])
    return None, "path not among this record's changes"


# --------------------------------------------------------------------------
# Live field walking (daemon)
# --------------------------------------------------------------------------


def walk_fields(fields: Any, segs: list[str]) -> tuple[bool, Any]:
    """Walk decoded record `fields` by path segments: dict keys (exact, then
    case-insensitive), `[N]` list index, or `[Field=value]` list-element
    key match. Returns (found, value)."""
    cur = fields
    for seg in segs:
        m = _ROW_RE.match(seg)
        if m:
            if not isinstance(cur, list):
                return False, None
            key = m.group(1)
            if key.isdigit():
                n = int(key)
                if n >= len(cur):
                    return False, None
                cur = cur[n]
                continue
            if "=" in key:
                field, _, want = key.partition("=")
                hit = None
                for elem in cur:
                    if isinstance(elem, dict) and field in elem and values_match(want.strip(), elem[field]):
                        hit = elem
                        break
                if hit is None:
                    return False, None
                cur = hit
                continue
            return False, None
        if not isinstance(cur, dict):
            return False, None
        if seg in cur:
            cur = cur[seg]
            continue
        lower = {_norm_key(k): k for k in cur}
        k = lower.get(_norm_key(seg))
        if k is None:
            return False, None
        cur = cur[k]
    return True, cur


class LiveLookup:
    """Cached `bulk_get` per (side, selector) against the OLD/NEW ESM."""

    def __init__(self, gateway, old_esm: str | None, new_esm: str | None):
        self.gateway = gateway
        self.esm = {"old": old_esm, "new": new_esm}
        self._cache: dict[tuple[str, str], dict | None] = {}

    def available(self, side: str) -> bool:
        return self.gateway is not None and bool(self.esm.get(side))

    def record(self, side: str, selector: str) -> dict | None:
        """`{"header", "editor_id", "fields"}` or None when absent/errored."""
        key = (side, selector)
        if key not in self._cache:
            entry = None
            if self.available(side):
                try:
                    got = self.gateway.bulk_get(self.esm[side], [selector], resolve="stub")
                except Exception as exc:  # daemon hiccup: treat as unavailable, never crash the gate
                    eprint(f"warning: bulk_get({side}, {selector}) failed: {exc}")
                    got = []
                first = got[0] if got else None
                if isinstance(first, dict) and not first.get("error") and first.get("fields") is not None:
                    entry = first
            self._cache[key] = entry
        return self._cache[key]


# --------------------------------------------------------------------------
# Claim verification
# --------------------------------------------------------------------------


def _result(claim: dict, status: str, source: str, detail: str) -> dict:
    return {"claim": claim, "status": status, "source": source, "detail": detail}


def _old_selector(claim: dict, rec: dict | None) -> str:
    """The selector to use against the OLD snapshot: a record renamed this
    patch is found under its previous EditorID there."""
    sel = claim["record"]
    if rec and not canon_formid(sel) and rec.get("prev_editor_id") and rec.get("editor_id") == sel:
        return rec["prev_editor_id"]
    return sel


def _live_value(live: LiveLookup, side: str, selector: str, segs: list[str]) -> tuple[str, Any]:
    """('found', value) | ('missing', None) | ('unavailable', None)."""
    if not live.available(side):
        return "unavailable", None
    entry = live.record(side, selector)
    if entry is None:
        return "missing", None
    found, value = walk_fields(entry.get("fields"), segs)
    return ("found", value) if found else ("missing", None)


def verify_claim(claim: dict, index: RecordIndex, live: LiveLookup) -> dict:
    if not isinstance(claim, dict) or not isinstance(claim.get("record"), str) or not claim["record"].strip():
        return _result(claim, "unverifiable", "none", "claim has no 'record' selector")
    rec = index.get(claim["record"].strip())

    if "status" in claim:
        want = claim["status"]
        if want not in ("added", "removed"):
            return _result(claim, "unverifiable", "none", f"status must be added|removed, got {want!r}")
        if rec is None:
            return _result(claim, "unverifiable", "none", "record not in comprehensive.json")
        if rec.get("status") == want:
            return _result(claim, "ok", "changes", f"record status is {want}")
        return _result(claim, "mismatch", "changes", f"record status is {rec.get('status')!r}, claim says {want!r}")

    if not isinstance(claim.get("path"), str) or not claim["path"].strip():
        return _result(claim, "unverifiable", "none", "claim needs a 'path' (or a 'status')")
    segs = split_path(claim["path"])

    if "from" in claim or "to" in claim:
        if "from" not in claim or "to" not in claim:
            return _result(claim, "unverifiable", "none", "a changed claim needs both 'from' and 'to'")
        if rec is not None:
            if rec.get("status") == "added" and claim.get("from") is not None:
                return _result(claim, "mismatch", "changes", "record is new this patch; it has no prior value")
            entry, why = resolve_change(rec.get("changes") or [], segs)
            if entry is not None:
                if entry.get("kind") == "array":
                    arr = entry.get("array") or {}
                    ok = values_match(claim["from"], arr.get("count_from")) and values_match(claim["to"], arr.get("count_to"))
                    detail = f"array counts {arr.get('count_from')} -> {arr.get('count_to')}"
                    return _result(claim, "ok" if ok else "mismatch", "changes", detail)
                ok_from = values_match(claim["from"], entry.get("from"), entry.get("from_display"))
                ok_to = values_match(claim["to"], entry.get("to"), entry.get("to_display"))
                detail = f"data: {_short(entry.get('from'))} -> {_short(entry.get('to'))}"
                if ok_from and ok_to:
                    return _result(claim, "ok", "changes", detail)
                if values_match(claim["to"], entry.get("from"), entry.get("from_display")) and values_match(
                    claim["from"], entry.get("to"), entry.get("to_display")
                ):
                    return _result(claim, "mismatch", "changes", f"from/to are reversed; {detail}")
                return _result(claim, "mismatch", "changes", detail)
            reason = why or "path not among this record's changes"
        else:
            reason = "record not in comprehensive.json"
        # Fallback: both sides live.
        old_state, old_val = _live_value(live, "old", _old_selector(claim, rec), segs)
        new_state, new_val = _live_value(live, "new", claim["record"].strip(), segs)
        if old_state == "unavailable" or new_state == "unavailable":
            return _result(claim, "unverifiable", "none", f"{reason}; no daemon to check live")
        if old_state == "missing" or new_state == "missing":
            return _result(claim, "unverifiable", "daemon", f"{reason}; live lookup found no such field/record")
        if values_match(old_val, new_val):
            return _result(claim, "mismatch", "daemon", f"{reason}; live value is {_short(new_val)} on BOTH sides (did not change)")
        ok = values_match(claim["from"], old_val) and values_match(claim["to"], new_val)
        return _result(claim, "ok" if ok else "mismatch", "daemon", f"live: {_short(old_val)} -> {_short(new_val)}")

    if "value" in claim:
        side = claim.get("side") or "new"
        if side not in ("old", "new"):
            return _result(claim, "unverifiable", "none", f"side must be old|new, got {side!r}")
        if side == "new" and rec is not None and rec.get("fields") is not None:
            found, value = walk_fields(rec.get("fields"), segs)
            if found:
                ok = values_match(claim["value"], value)
                return _result(claim, "ok" if ok else "mismatch", "changes", f"comprehensive fields: {_short(value)}")
        selector = _old_selector(claim, rec) if side == "old" else claim["record"].strip()
        state, value = _live_value(live, side, selector, segs)
        if state == "unavailable":
            return _result(claim, "unverifiable", "none", "value claim needs a live daemon (--old-esm/--new-esm)")
        if state == "missing":
            return _result(claim, "unverifiable", "daemon", "live lookup found no such field/record")
        ok = values_match(claim["value"], value)
        return _result(claim, "ok" if ok else "mismatch", "daemon", f"live ({side}): {_short(value)}")

    return _result(claim, "unverifiable", "none", "claim has neither status, from/to, nor value")


# --------------------------------------------------------------------------
# Unbacked numbers in prose (warning only)
# --------------------------------------------------------------------------

_FENCE_RE = re.compile(r"```.*?```", re.DOTALL)
_INLINE_CODE_RE = re.compile(r"`[^`\n]*`")
_NUMBER_RE = re.compile(r"(?<![\w.\-/])-?\d{1,3}(?:,\d{3})+(?:\.\d+)?%?|(?<![\w.\-/])-?\d+(?:\.\d+)?%?(?![\w\-/])")


def _number_variants(value: Any) -> set[str]:
    out: set[str] = set()
    n = _numeric_value(value)
    if n is None or isinstance(value, bool):
        return out
    if float(n).is_integer():
        i = int(n)
        out.update({str(i), f"{i:,}", f"{i}%", f"{i:,}%"})
    text = f"{n:g}"
    out.update({text, f"{text}%", str(value).strip() if isinstance(value, str) else repr(n)})
    for digits in (1, 2, 3):
        r = f"{n:.{digits}f}".rstrip("0").rstrip(".")
        out.update({r, f"{r}%"})
    return out


def backed_numbers(claims: list[dict]) -> set[str]:
    backed: set[str] = set()
    for claim in claims:
        if not isinstance(claim, dict):
            continue
        for key in ("from", "to", "value"):
            if key in claim:
                backed |= _number_variants(claim[key])
    return backed


def find_unbacked_numbers(draft: str, claims: list[dict]) -> list[str]:
    """Numbers stated in the draft prose (outside code, Evidence lines and
    headings) that no claim's from/to/value renders to. Heuristic, warning
    only: small integers (< 10) without a % sign are skipped as ordinary
    English counts."""
    backed = backed_numbers(claims)
    text = _FENCE_RE.sub(" ", draft)
    text = _INLINE_CODE_RE.sub(" ", text)
    found: list[str] = []
    seen: set[str] = set()
    for line in text.splitlines():
        s = line.strip()
        if not s or s.startswith("#") or s.lower().startswith(("evidence", "- evidence", "* evidence")):
            continue
        for m in _NUMBER_RE.finditer(line):
            token = m.group(0)
            core = token.rstrip("%")
            if not token.endswith("%"):
                try:
                    if abs(float(core.replace(",", ""))) < _MIN_INTERESTING_INT and float(core.replace(",", "")).is_integer():
                        continue
                except ValueError:
                    continue
            if token in backed or core in backed or core.replace(",", "") in backed:
                continue
            if token not in seen:
                seen.add(token)
                found.append(token)
    return found


# --------------------------------------------------------------------------
# Run
# --------------------------------------------------------------------------


def check_report(report_path: Path, index: RecordIndex, live: LiveLookup) -> dict:
    with report_path.open(encoding="utf-8") as f:
        report = json.load(f)
    claims = report.get("claims") if isinstance(report, dict) else None
    if not isinstance(claims, list):
        claims = []
    results = [verify_claim(c, index, live) for c in claims]
    draft_path = layout.draft_md_for_report(report_path)
    draft = draft_path.read_text(encoding="utf-8") if draft_path.is_file() else ""
    return {
        "report": report_path.name,
        "draft": draft_path.name if draft_path.is_file() else None,
        "checked": len(results),
        "ok": sum(1 for r in results if r["status"] == "ok"),
        "mismatches": [r for r in results if r["status"] == "mismatch"],
        "unverifiable": [r for r in results if r["status"] == "unverifiable"],
        "unbacked_numbers": find_unbacked_numbers(draft, claims) if draft else [],
        "no_claims": not claims,
    }


def run_check(out_dir: Path, gateway=None, old_esm: str | None = None, new_esm: str | None = None) -> dict:
    """Verify every report under `<out_dir>/drafts/`; write
    `work/claims-check.json`; return the payload (`ok` is the gate)."""
    comp_path = layout.comprehensive_json(out_dir)
    with comp_path.open(encoding="utf-8") as f:
        comp = pl.validate_comprehensive_payload(json.load(f))
    index = RecordIndex(comp.get("records") or {})
    live = LiveLookup(gateway, old_esm, new_esm)
    reports = [check_report(p, index, live) for p in layout.drafts_deep_reports(out_dir)]
    payload = {
        "schema_version": 1,
        "reports": reports,
        "checked": sum(r["checked"] for r in reports),
        "mismatch_count": sum(len(r["mismatches"]) for r in reports),
        "unverifiable_count": sum(len(r["unverifiable"]) for r in reports),
        "live_lookups": bool(gateway is not None and old_esm and new_esm),
    }
    payload["ok"] = bool(reports) and payload["mismatch_count"] == 0 and payload["unverifiable_count"] == 0
    layout.work_dir(out_dir).mkdir(parents=True, exist_ok=True)
    with layout.work_claims_check_json(out_dir).open("w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2, ensure_ascii=False)
        f.write("\n")
    return payload


def print_summary(payload: dict, stream=sys.stderr):
    for r in payload["reports"]:
        line = (
            f"{r['report']}: {r['ok']}/{r['checked']} ok, {len(r['mismatches'])} mismatch, "
            f"{len(r['unverifiable'])} unverifiable, {len(r['unbacked_numbers'])} unbacked number(s)"
        )
        if r["no_claims"]:
            line += "  [NO CLAIMS -- writer prompt drift]"
        print(line, file=stream)
        for res in r["mismatches"] + r["unverifiable"]:
            c = res["claim"]
            where = c.get("path") or c.get("status") if isinstance(c, dict) else "?"
            print(f"  {res['status'].upper()} {c.get('record') if isinstance(c, dict) else '?'} {where}: {res['detail']}", file=stream)
        if r["unbacked_numbers"]:
            print(f"  unbacked: {', '.join(r['unbacked_numbers'][:20])}", file=stream)
    if not payload["reports"]:
        print("no drafts/deep*.report.json found", file=stream)


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="check_claims.py",
        description="Re-verify every structured claim in drafts/deep*.report.json against "
                    "comprehensive.json (and, optionally, the live esm daemon).",
    )
    ap.add_argument("out_dir", type=Path, help="Pipeline output directory")
    ap.add_argument("--old-esm", default=None, help="OLD snapshot ESM (enables live lookups)")
    ap.add_argument("--new-esm", default=None, help="NEW snapshot ESM (enables live lookups)")
    ap.add_argument("--esm-bin", default=None, help="Path to the esm binary (default: auto)")
    ap.add_argument("--no-daemon", action="store_true", help="Never query the daemon; such claims are unverifiable")
    return ap


def main(argv=None) -> int:
    args = build_arg_parser().parse_args(argv)
    gateway = None
    if not args.no_daemon and args.old_esm and args.new_esm:
        try:
            gateway = eg.ensure_daemon(eg.find_esm_binary(args.esm_bin), args.new_esm)
        except eg.DaemonError as exc:
            eprint(f"warning: no daemon ({exc}); claims not in comprehensive.json will be unverifiable")
    try:
        payload = run_check(args.out_dir, gateway, args.old_esm, args.new_esm)
    finally:
        if gateway is not None and hasattr(gateway, "close"):
            gateway.close()
    print_summary(payload)
    eprint(f"wrote {layout.work_claims_check_json(args.out_dir)}")
    return 0 if payload["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
