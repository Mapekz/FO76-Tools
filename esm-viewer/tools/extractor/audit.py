#!/usr/bin/env python3
"""Static parity audit: Pascal (wbDefinitionsFO76.pas) ↔ schema/fo76.json.

Imports the extractor, re-runs it (pre-override) with _raw_itype annotations,
then walks both member trees in parallel and classifies divergences.

Usage:
    python3 tools/extractor/audit.py [--json] [--gate] [--record SIG] [--min-sev SEV]

Exit codes:
    0 — all findings either clean or allowlisted
    1 — any CRITICAL/HIGH un-allowlisted finding (when --gate is passed)
"""

from __future__ import annotations

import io
import json
import re
import sys
from pathlib import Path
from typing import Any

# ── Bootstrap: add extractor dir to path ──────────────────────────────────
_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(_DIR))

from extract import (  # noqa: E402
    COMMON_PAS,
    Extractor,
    FO76_PAS,
    INT_MAP,
    OUT,
    WHITELIST,
    find_matching_paren,
    read_text,
    sig_id,
    split_top_level,
)

EXCEPTIONS_FILE = _DIR / "parity-exceptions.json"

# ── Severity constants and ranking ────────────────────────────────────────
CRIT = "CRITICAL"
HIGH = "HIGH"
MED = "MEDIUM"
LOW = "LOW"
ALLOWED = "ALLOWED"

SEV_ORDER: dict[str, int] = {CRIT: 0, HIGH: 1, MED: 2, LOW: 3, ALLOWED: 99}

# Type alias for a finding dict.
Finding = dict[str, Any]


# ── AuditExtractor ─────────────────────────────────────────────────────────
class AuditExtractor(Extractor):
    """Extractor subclass that annotates integer members with _raw_itype.

    The raw token is the Pascal itXxx literal *before* INT_MAP normalisation,
    so the audit can distinguish:
      - A token in INT_MAP (width/signed known exactly) from
      - A token NOT in INT_MAP (silently defaulted to u32 — the OBTS class).
    """

    def _parse_integer(self, expr: str) -> dict:
        result = super()._parse_integer(expr)
        # Re-parse to capture the raw itype token before normalisation.
        try:
            lp = expr.index("(")
            rp = find_matching_paren(expr, lp)
            parts = split_top_level(expr[lp + 1 : rp])
            idx = 1 if (parts and sig_id(parts[0].strip())) else 0
            # parts layout: [sig?,] name, itype?, format_arg?
            raw_itype = parts[idx + 1].strip() if idx + 1 < len(parts) else ""
        except Exception:
            raw_itype = ""
        # result may be a union dict (for wbObjectModPropertyToStr); guard with isinstance
        if isinstance(result, dict):
            result["_raw_itype"] = raw_itype
        return result


# ── Member summariser ─────────────────────────────────────────────────────
def _summarize(m: dict) -> dict:
    """Extract key comparable attributes from a member for display."""
    kind = m.get("kind", "?")
    s: dict[str, Any] = {"kind": kind, "name": m.get("name", ""), "sig": m.get("sig")}
    if kind == "integer":
        s["width"] = m.get("width", "u32")
        s["signed"] = m.get("signed", False)
        raw = m.get("_raw_itype", "")
        if raw:
            s["_raw_itype"] = raw
    elif kind == "struct":
        s["n_fields"] = len(m.get("fields", []))
    elif kind == "rstruct":
        s["n_members"] = len(m.get("members", []))
    elif kind in ("array", "rarray"):
        elem = m.get("element") or {}
        s["element_kind"] = elem.get("kind", "?")
    elif kind == "union":
        s["n_variants"] = len(m.get("variants", []))
    elif kind == "raw_fallback":
        s["reason"] = m.get("reason", "")
    return s


# ── Tree walker ───────────────────────────────────────────────────────────
def _index_by_sig(members: list[dict]) -> dict[str, list[dict]]:
    by_sig: dict[str, list[dict]] = {}
    for m in members:
        s = m.get("sig")
        if s:
            by_sig.setdefault(s, []).append(m)
    return by_sig


def compare_member_trees(
    pascal_mems: list[dict],
    schema_mems: list[dict],
    path: str,
    record: str,
    findings: list[Finding],
    depth: int = 0,
) -> None:
    """Recursively compare two member lists and append findings."""
    if depth > 8:
        return  # prevent runaway on deep/recursive trees

    p_by_sig = _index_by_sig(pascal_mems)
    s_by_sig = _index_by_sig(schema_mems)

    # ── Sig-keyed comparison ───────────────────────────────────────────────
    all_sigs = sorted(set(p_by_sig) | set(s_by_sig))
    for sig_key in all_sigs:
        p_list = p_by_sig.get(sig_key, [])
        s_list = s_by_sig.get(sig_key, [])
        node_path = f"{path}/{sig_key}"

        if p_list and not s_list:
            for pm in p_list:
                findings.append({
                    "record": record,
                    "path": node_path,
                    "class_": "dropped",
                    "sev": HIGH,
                    "detail": (
                        f"member '{pm.get('name', sig_key)}' ({pm.get('kind', '?')}) "
                        f"present in Pascal, absent from schema"
                    ),
                    "pascal": _summarize(pm),
                    "schema": None,
                })
        elif p_list and s_list:
            # Both present — compare first member of each (covers most real cases).
            # If there are multiple members with the same sig (unusual), compare pairwise.
            for pm, sm in zip(p_list, s_list):
                _compare_single(pm, sm, node_path, record, findings, depth)
            # Extra pascal members (more pascal than schema for same sig)
            for pm in p_list[len(s_list):]:
                findings.append({
                    "record": record,
                    "path": node_path,
                    "class_": "dropped",
                    "sev": HIGH,
                    "detail": (
                        f"extra pascal member '{pm.get('name', '?')}' ({pm.get('kind', '?')}) "
                        f"with sig {sig_key} has no schema counterpart"
                    ),
                    "pascal": _summarize(pm),
                    "schema": None,
                })
        # else: only in schema (injected builtin / override addition) — not a parity problem

    # ── Sig-less member alignment (positional) ────────────────────────────
    p_sigless = [m for m in pascal_mems if not m.get("sig")]
    s_sigless = [m for m in schema_mems if not m.get("sig")]
    min_len = min(len(p_sigless), len(s_sigless))
    for i in range(min_len):
        item_path = f"{path}[{i}:{p_sigless[i].get('name', '?')}]"
        _compare_single(p_sigless[i], s_sigless[i], item_path, record, findings, depth)
    # Extra pascal sig-less members → dropped
    for i in range(min_len, len(p_sigless)):
        pm = p_sigless[i]
        findings.append({
            "record": record,
            "path": f"{path}[{i}:{pm.get('name', '?')}]",
            "class_": "dropped",
            "sev": HIGH,
            "detail": (
                f"positional member #{i} '{pm.get('name', '?')}' ({pm.get('kind', '?')}) "
                f"present in Pascal, absent from schema"
            ),
            "pascal": _summarize(pm),
            "schema": None,
        })


def _compare_single(
    pm: dict,
    sm: dict,
    path: str,
    record: str,
    findings: list[Finding],
    depth: int,
) -> None:
    """Compare a single pascal member against its schema counterpart."""
    pk = pm.get("kind", "?")
    sk = sm.get("kind", "?")

    # ── Schema uses raw_fallback ───────────────────────────────────────────
    if sk == "raw_fallback":
        findings.append({
            "record": record,
            "path": path,
            "class_": "fell-back-to-raw",
            "sev": MED,
            "detail": (
                f"schema uses raw_fallback (reason: '{sm.get('reason', '?')}'); "
                f"pascal kind={pk}"
            ),
            "pascal": _summarize(pm),
            "schema": _summarize(sm),
        })
        return

    # ── Kind mismatch ──────────────────────────────────────────────────────
    # Allowed equivalences (no byte-layout impact):
    # - empty ↔ unused: both consume 0 bytes
    # - string kinds: string ↔ lstring have different decode but same wire scan
    EQUIVALENT_KINDS = {
        frozenset({"empty", "unused"}),
    }

    if pk != sk:
        # Special case: integer vs integer → handled below
        if pk == "integer" and sk == "integer":
            pass  # fall through to integer comparison
        else:
            is_equiv = any(frozenset({pk, sk}) == eq for eq in EQUIVALENT_KINDS)
            if not is_equiv:
                findings.append({
                    "record": record,
                    "path": path,
                    "class_": "kind-mismatch",
                    "sev": CRIT,
                    "detail": f"pascal kind={pk}, schema kind={sk}",
                    "pascal": _summarize(pm),
                    "schema": _summarize(sm),
                })
            return

    if pk == "integer" and sk == "integer":
        _compare_integers(pm, sm, path, record, findings)
        return

    # ── Recursive struct comparison ────────────────────────────────────────
    if pk == sk and pk in ("struct", "rstruct"):
        field_key = "fields" if pk == "struct" else "members"
        p_children = pm.get(field_key, [])
        s_children = sm.get(field_key, [])
        if len(p_children) != len(s_children):
            findings.append({
                "record": record,
                "path": path,
                "class_": "count-mismatch",
                "sev": HIGH,
                "detail": (
                    f"{pk} child count mismatch: pascal={len(p_children)}, "
                    f"schema={len(s_children)}"
                ),
                "pascal": _summarize(pm),
                "schema": _summarize(sm),
            })
        compare_member_trees(p_children, s_children, path, record, findings, depth + 1)
        return

    # ── Union variant count ────────────────────────────────────────────────
    if pk == sk and pk == "union":
        p_vars = pm.get("variants", [])
        s_vars = sm.get("variants", [])
        if len(p_vars) != len(s_vars):
            findings.append({
                "record": record,
                "path": path,
                "class_": "count-mismatch",
                "sev": HIGH,
                "detail": (
                    f"union variant count: pascal={len(p_vars)}, "
                    f"schema={len(s_vars)}"
                ),
                "pascal": _summarize(pm),
                "schema": _summarize(sm),
            })
        # Recurse into matched variants positionally
        for i, (pv, sv) in enumerate(zip(p_vars, s_vars)):
            _compare_single(
                pv, sv, f"{path}[v{i}]", record, findings, depth + 1
            )
        return

    # ── Array/rarray element comparison ──────────────────────────────────
    if pk == sk and pk in ("array", "rarray"):
        pe = pm.get("element") or {}
        se = sm.get("element") or {}
        if pe and se:
            _compare_single(pe, se, f"{path}[elem]", record, findings, depth + 1)
        return


def _compare_integers(
    pm: dict,
    sm: dict,
    path: str,
    record: str,
    findings: list[Finding],
) -> None:
    """Compare two integer members: detect silent defaults and width mismatches."""
    pw: str = pm.get("width", "u32")
    sw: str = sm.get("width", "u32")
    ps: bool = pm.get("signed", False)
    ss: bool = sm.get("signed", False)
    raw_itype: str = pm.get("_raw_itype", "")

    # Normalise the raw token the same way extract.py does (case-insensitive it[su]NN).
    norm_itype = re.sub(
        r"^it([su])(\d+)$",
        lambda m: f"it{m.group(1).upper()}{m.group(2)}",
        raw_itype,
    ) if raw_itype else ""

    # Silent-default suspect: normalised token not in INT_MAP AND schema has u32/unsigned.
    # This is the OBTS class: extract.py called INT_MAP.get(tok, ("u32", False)) and
    # the fallback default fired, producing a u32 regardless of what Pascal says.
    if norm_itype and norm_itype not in INT_MAP and sw == "u32" and not ss:
        findings.append({
            "record": record,
            "path": path,
            "class_": "silent-default-suspect",
            "sev": CRIT,
            "detail": (
                f"raw Pascal itype '{raw_itype}' (normalised: '{norm_itype}') "
                f"∉ INT_MAP → INT_MAP.get() default fired → schema=(u32, unsigned); "
                f"likely wrong width/signedness"
            ),
            "pascal": _summarize(pm),
            "schema": _summarize(sm),
        })
        return

    # Normal byte-width or signedness mismatch.
    if (pw, ps) != (sw, ss):
        findings.append({
            "record": record,
            "path": path,
            "class_": "byte-width-mismatch",
            "sev": CRIT,
            "detail": (
                f"pascal={pw}/signed={ps} (itype='{raw_itype}'), "
                f"schema={sw}/signed={ss}"
            ),
            "pascal": _summarize(pm),
            "schema": _summarize(sm),
        })


# ── Exceptions allowlist ──────────────────────────────────────────────────
def _load_exceptions(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return json.loads(path.read_text(encoding="utf-8")).get("exceptions", [])


def _matches_exception(f: Finding, exc: dict) -> bool:
    """Check if a finding matches an exception entry."""
    # Record: exact match or "*" wildcard
    exc_record = exc.get("record", "*")
    if exc_record != "*" and exc_record != f.get("record"):
        return False
    # class_: exact match or "*"
    exc_class = exc.get("class_", "*")
    if exc_class != "*" and exc_class != f.get("class_"):
        return False
    # path: exact match, "*", or prefix wildcard (trailing "*")
    exc_path = exc.get("path", "*")
    if exc_path != "*":
        if exc_path.endswith("*"):
            if not f.get("path", "").startswith(exc_path[:-1]):
                return False
        elif exc_path != f.get("path"):
            return False
    return True


def _apply_exceptions(
    findings: list[Finding],
    exceptions: list[dict],
) -> tuple[list[Finding], int]:
    """Downgrade matching findings to ALLOWED. Returns (updated_findings, count)."""
    applied = 0
    result = []
    for f in findings:
        matched_exc = None
        for exc in exceptions:
            if _matches_exception(f, exc):
                matched_exc = exc
                break
        if matched_exc:
            f = {**f, "sev": ALLOWED, "exception_reason": matched_exc.get("reason", "")}
            applied += 1
        result.append(f)
    return result, applied


# ── Main audit runner ─────────────────────────────────────────────────────
def run_audit(
    record_filter: str | None = None,
) -> tuple[list[Finding], dict[str, Any]]:
    """Run the full static audit and return (findings, totals)."""
    if not FO76_PAS.exists():
        print(f"ERROR: Missing {FO76_PAS}", file=sys.stderr)
        sys.exit(1)

    # Build pre-override pascal schema via AuditExtractor.
    # Suppress the per-record progress lines (173 lines of noise during audit).
    ex = AuditExtractor(
        read_text(FO76_PAS),
        read_text(COMMON_PAS) if COMMON_PAS.exists() else "",
    )
    _captured = io.StringIO()
    _old_stderr = sys.stderr
    sys.stderr = _captured
    try:
        pascal_schema = ex.run()
    finally:
        sys.stderr = _old_stderr
    extractor_stderr = _captured.getvalue()

    # Surface any extractor warnings (defaulted tokens, failures, etc.)
    warning_lines = [
        ln for ln in extractor_stderr.splitlines()
        if "WARNING" in ln or "ERROR" in ln or "defaulted" in ln.lower() or "failed" in ln.lower()
    ]
    if warning_lines:
        print("Extractor warnings during audit run:", file=sys.stderr)
        for ln in warning_lines:
            print(f"  {ln}", file=sys.stderr)

    # Load shipped schema (post-override).
    if not OUT.exists():
        print(
            f"ERROR: Missing {OUT} — run 'python3 tools/extractor/extract.py' first",
            file=sys.stderr,
        )
        sys.exit(1)
    shipped: dict = json.loads(OUT.read_text(encoding="utf-8"))

    exceptions = _load_exceptions(EXCEPTIONS_FILE)

    all_findings: list[Finding] = []
    sigs = [record_filter] if record_filter else WHITELIST

    for sig in sigs:
        p_rec: dict | None = pascal_schema.get("records", {}).get(sig)
        s_rec: dict | None = shipped.get("records", {}).get(sig)

        if p_rec is None and s_rec is None:
            continue
        if p_rec is None:
            # Override-only record (e.g. PERK replaced wholesale) — informational only.
            continue
        if s_rec is None:
            # Pascal has a record but schema has nothing.
            all_findings.append({
                "record": sig,
                "path": sig,
                "class_": "dropped",
                "sev": HIGH,
                "detail": (
                    f"record '{sig}' extracted from Pascal "
                    f"({len(p_rec.get('members', []))} members) "
                    f"but absent from shipped schema"
                ),
                "pascal": {
                    "name": p_rec.get("name"),
                    "n_members": len(p_rec.get("members", [])),
                },
                "schema": None,
            })
            continue

        compare_member_trees(
            p_rec.get("members", []),
            s_rec.get("members", []),
            sig,
            sig,
            all_findings,
        )

    # ── Unrecognized-construct drops ──────────────────────────────────────
    # These members were silently dropped by parse_member's terminal return None.
    # They never appear in the pascal tree so the tree comparison above misses them.
    # Surface them here with record context from the per-record attribution map.
    for rec_sig, constructs in ex.report.unrecognized_by_record.items():
        if record_filter and rec_sig != record_filter:
            continue
        for construct_key, count in sorted(constructs.items(), key=lambda x: -x[1]):
            all_findings.append({
                "record": rec_sig,
                "path": f"{rec_sig}/?/{construct_key}",
                "class_": "dropped",
                "sev": HIGH,
                "detail": (
                    f"Pascal helper '{construct_key}' dropped {count}× "
                    f"(no extractor handler); members not in schema"
                ),
                "pascal": {"kind": "unrecognized", "construct": construct_key, "count": count},
                "schema": None,
            })

    all_findings, exc_count = _apply_exceptions(all_findings, exceptions)

    # ── Totals ─────────────────────────────────────────────────────────────
    active = [f for f in all_findings if f["sev"] != ALLOWED]
    totals: dict[str, Any] = {
        "total": len(all_findings),
        "exceptions_applied": exc_count,
        "active": len(active),
        "by_severity": {},
        "by_class": {},
    }
    for f in active:
        sev = f["sev"]
        cls = f["class_"]
        totals["by_severity"][sev] = totals["by_severity"].get(sev, 0) + 1
        totals["by_class"][cls] = totals["by_class"].get(cls, 0) + 1

    return all_findings, totals


# ── Output ────────────────────────────────────────────────────────────────
def _print_table(findings: list[Finding], totals: dict, min_sev_order: int = 3) -> None:
    """Print a human-readable sorted table of active findings."""
    active = [
        f for f in findings
        if f["sev"] != ALLOWED and SEV_ORDER.get(f["sev"], 99) <= min_sev_order
    ]
    active.sort(key=lambda f: (SEV_ORDER.get(f["sev"], 99), f["record"], f["path"]))

    if not active:
        print("✓  No active parity findings (all clean or allowlisted)")
        return

    W_SEV = 8
    W_CLS = 24
    W_REC = 6
    W_PTH = 42

    hdr = (
        f"{'SEV':<{W_SEV}}  {'CLASS':<{W_CLS}}  {'REC':<{W_REC}}  "
        f"{'PATH':<{W_PTH}}  DETAIL"
    )
    print(hdr)
    print("─" * len(hdr))
    for f in active:
        path = f.get("path", "")
        if len(path) > W_PTH:
            path = "…" + path[-(W_PTH - 1):]
        detail = f.get("detail", "")[:80]
        print(
            f"{f['sev']:<{W_SEV}}  {f['class_']:<{W_CLS}}  "
            f"{f['record']:<{W_REC}}  {path:<{W_PTH}}  {detail}"
        )

    print()
    print(
        f"Findings: {totals['total']} total  "
        f"{totals['active']} active  "
        f"{totals['exceptions_applied']} exceptions applied"
    )
    sev_str = "  ".join(f"{k}:{v}" for k, v in sorted(
        totals["by_severity"].items(), key=lambda x: SEV_ORDER.get(x[0], 99)
    ))
    cls_str = "  ".join(f"{k}:{v}" for k, v in sorted(
        totals["by_class"].items(), key=lambda x: -x[1]
    ))
    if sev_str:
        print(f"By severity: {sev_str}")
    if cls_str:
        print(f"By class:    {cls_str}")


# ── Entry point ───────────────────────────────────────────────────────────
def main() -> None:
    import argparse

    ap = argparse.ArgumentParser(
        description=(
            "Static parity audit: Pascal definitions ↔ schema/fo76.json.\n"
            "Classifies divergences by severity and applies an allowlist for "
            "intentional differences."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument(
        "--json", action="store_true",
        help="Emit JSON output instead of a human-readable table",
    )
    ap.add_argument(
        "--gate", action="store_true",
        help=(
            "Exit with code 1 if any CRITICAL or HIGH findings remain "
            "(use as a CI gate)"
        ),
    )
    ap.add_argument(
        "--record", metavar="SIG",
        help="Audit a single record type only (e.g. WEAP, NPC_)",
    )
    ap.add_argument(
        "--min-sev", default="LOW",
        choices=["CRITICAL", "HIGH", "MEDIUM", "LOW"],
        help="Minimum severity to display in table output (default: LOW)",
    )
    args = ap.parse_args()

    findings, totals = run_audit(record_filter=args.record)

    if args.json:
        by_record: dict[str, list] = {}
        for f in findings:
            by_record.setdefault(f["record"], []).append(f)
        print(json.dumps(
            {
                "by_record": by_record,
                "totals": totals,
                "exceptions_applied": totals["exceptions_applied"],
            },
            indent=2,
        ))
    else:
        _print_table(findings, totals, SEV_ORDER[args.min_sev])

    if args.gate:
        n_crit_high = (
            totals["by_severity"].get(CRIT, 0)
            + totals["by_severity"].get(HIGH, 0)
        )
        if n_crit_high > 0:
            print(
                f"\nGATE FAILED: {n_crit_high} CRITICAL/HIGH finding(s) "
                f"require resolution or allowlisting",
                file=sys.stderr,
            )
            sys.exit(1)


if __name__ == "__main__":
    main()
