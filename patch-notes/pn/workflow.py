"""The /patch-notes skill's mechanical steps, one verb each.

    prepare [OLD] [NEW]    snapshots -> pipeline (or reuse) -> cache -> triage -> slices
    merge-assessment OUT   fold the assessor's tiers in and re-slice
    gate OUT [--summary]   claims + coverage over the drafts (and the summary)
    publish OUT            validate the review, chunk for Discord, record the manifest

Each prints a JSON summary on stdout for the orchestrator (progress goes to
stderr). Absolute paths appear only there, never in files under the output
directory.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

from pn import (
    check_claims,
    check_coverage,
    discord_chunker,
    esmcli,
    fetch_official_notes,
    jsonio,
    layout,
    schemas,
    triage_bundles,
    update_manifest,
)
from pn import make_patch_notes as mpn
from pn import patchnotes_lib as pl

#: Above this many DEEP bundles the slice is split across two writers.
SPLIT_DEEP_ABOVE = 20

#: More DEEP bundles than this usually means a tier rule misfired.
DEEP_SANITY_LIMIT = 40

#: Record types that dropping wholesale probably loses a story.
DROP_WATCH_TYPES = ("WEAP", "PERK", "OMOD")

_SNAPSHOT_RE = re.compile(r"^\d{8}$")


def eprint(*args: Any) -> None:
    print(*args, file=sys.stderr)


def json_verb(fn):
    """A verb whose stages' output goes to stderr, and whose returned
    `(exit_code, payload)` payload is the only thing on stdout, as JSON."""

    @functools.wraps(fn)
    def verb(argv: list[str] | None = None, **kwargs: Any) -> int:
        stdout = sys.stdout
        with contextlib.redirect_stdout(sys.stderr):
            rc, payload = fn(argv, **kwargs)
        if payload is not None:
            print(json.dumps(payload, indent=2), file=stdout)
        return rc

    return verb


# --------------------------------------------------------------------------
# Snapshot resolution
# --------------------------------------------------------------------------


def data_dir() -> Path | None:
    value = os.environ.get("FO76_DATA_DIR")
    return Path(value) if value else None


def snapshot_tokens(root: Path) -> list[str]:
    """The dated snapshot directories under `root`, oldest first (`notes/`
    and anything else not named YYYYMMDD is skipped)."""
    return sorted(p.name for p in root.iterdir() if p.is_dir() and _SNAPSHOT_RE.match(p.name))


def resolve_snapshots(args: list[str], root: Path | None) -> tuple[Path, Path]:
    """The OLD and NEW snapshot directories from 0-2 arguments: none means
    the newest two snapshots, one names NEW (OLD is the snapshot before it),
    two name both. An argument is an absolute directory or a snapshot token
    under `$FO76_DATA_DIR`."""

    def under_root(token: str) -> Path:
        if token.startswith("/"):
            return Path(token)
        if root is None:
            raise ValueError("FO76_DATA_DIR is not set; pass absolute snapshot directories")
        return root / token

    if len(args) > 2:
        raise ValueError("at most two snapshots (OLD NEW)")
    if len(args) == 2:
        return under_root(args[0]), under_root(args[1])
    if root is None:
        raise ValueError("FO76_DATA_DIR is not set; pass OLD and NEW snapshot directories")
    tokens = snapshot_tokens(root)
    if not args:
        if len(tokens) < 2:
            raise ValueError(f"need two snapshots under {root}, found {len(tokens)}")
        return root / tokens[-2], root / tokens[-1]
    new = args[0]
    if new not in tokens:
        raise ValueError(f"no snapshot {new!r} under {root}")
    index = tokens.index(new)
    if index == 0:
        raise ValueError(f"{new} is the oldest snapshot; nothing to diff it against")
    return root / tokens[index - 1], root / new


def default_out_dir(old_esm: Path, new_esm: Path, root: Path | None) -> Path:
    name = f"{mpn.esm_token(old_esm)}_to_{mpn.esm_token(new_esm)}"
    if root is None:
        return mpn.default_out_dir(old_esm, new_esm)
    return root / "notes" / name


#: The mechanical artifacts `prepare` goes on to read, by manifest key.
REQUIRED_ARTIFACTS = {
    "diff": layout.diff_json,
    "comprehensive_json": layout.comprehensive_json,
    "bundles": layout.bundles_json,
    "lints": layout.lints_json,
}


def reuse_problem(out_dir: Path, old_esm: Path, new_esm: Path, options: dict) -> str | None:
    """Why `out_dir` doesn't hold this pair's finished mechanical output
    (both snapshots as they are now, the same output options, the current
    pipeline version, every artifact present and well-formed); `None` when
    it does."""
    manifest = pl.load_manifest(out_dir)
    if not manifest:
        return "no manifest"
    inputs = manifest.get("inputs") or {}
    for side, esm in (("old", old_esm), ("new", new_esm)):
        stat = esm.stat()
        if (
            inputs.get(f"{side}_token") != mpn.esm_token(esm)
            or inputs.get(f"{side}_esm_size") != stat.st_size
            or inputs.get(f"{side}_esm_mtime") != int(stat.st_mtime)
        ):
            return f"the {side} snapshot differs from the run's"
    if inputs.get("pipeline_version") != schemas.PIPELINE_VERSION:
        return "made by another pipeline version"
    if inputs.get("options") != options:
        return "made with other options (types, bodies, noise, sources)"
    mechanical = (manifest.get("stages") or {}).get("mechanical") or {}
    files = mechanical.get("files") or {}
    if not mechanical.get("completed_at"):
        return "the mechanical stage didn't finish"
    for key, path_of in REQUIRED_ARTIFACTS.items():
        if key not in files or not path_of(out_dir).is_file():
            return f"{path_of(out_dir).name} is missing"
    try:
        schemas.load(layout.diff_json(out_dir), schemas.validate_diff_payload)
        schemas.load(layout.comprehensive_json(out_dir), schemas.validate_comprehensive_payload)
        schemas.load(layout.bundles_json(out_dir), schemas.validate_bundles_payload)
        schemas.load(layout.lints_json(out_dir), schemas.validate_lints_payload)
    except (OSError, ValueError, TypeError, KeyError) as exc:
        return f"an artifact is malformed: {exc}"
    return None


def triage_problem(out_dir: Path) -> str | None:
    """Why the five `work/` triage files aren't a usable triage of
    `bundles.json` (all present and well-formed, every bundle in exactly one
    tier); `None` when they are."""
    if not layout.work_triage_json(out_dir).is_file():
        return "no triage yet"
    for path_of in (layout.work_ambiguous_json, layout.work_brief_lines_md, layout.work_rollouts_md):
        if not path_of(out_dir).is_file():
            return f"{path_of(out_dir).name} is missing"
    try:
        triage = schemas.load(layout.work_triage_json(out_dir), schemas.validate_triage)
        deep_slice = schemas.load(layout.work_deep_slice_json(out_dir), schemas.validate_deep_slice)
        ambiguous = jsonio.read(layout.work_ambiguous_json(out_dir))
        if not isinstance(ambiguous, dict) or not isinstance(ambiguous.get("bundles"), list):
            raise ValueError("ambiguous.json: expected {\"bundles\": [...]}")
        bundles = jsonio.read(layout.bundles_json(out_dir))["bundles"]
    except (OSError, ValueError, TypeError, KeyError) as exc:
        return f"malformed: {exc}"
    tiered = {bid for tier in schemas.TIERS for bid in triage[tier]}
    if tiered != {b["id"] for b in bundles}:
        return "its tiers don't cover exactly the bundles"
    if {b["id"] for b in deep_slice["bundles"]} != set(triage["deep"]):
        return "the DEEP slice doesn't match the DEEP tier"
    return None


def assessor_verdicts(out_dir: Path) -> dict[str, tuple[str | None, str]]:
    """The kept `work/triage.json`'s assessor verdicts, `{bundle_id: (tier,
    reason)}`, read leniently: it may be the file that failed validation."""
    try:
        triage = jsonio.read(layout.work_triage_json(out_dir))
        reasons = triage["reasons"]
    except (OSError, ValueError, TypeError, KeyError):
        return {}
    if not isinstance(reasons, dict):
        return {}
    tier_of = {
        bid: tier
        for tier in schemas.TIERS
        if isinstance(triage.get(tier), list)
        for bid in triage[tier]
        if isinstance(bid, str)
    }
    return {
        bid: (tier_of.get(bid), reason)
        for bid, reason in reasons.items()
        if isinstance(reason, str) and reason.startswith("assessor:")
    }


# --------------------------------------------------------------------------
# Triage follow-ups
# --------------------------------------------------------------------------


def split_deep_slice(out_dir: Path) -> list[Path]:
    """The slice file(s) writers get: `deep-slice.json` itself for up to
    SPLIT_DEEP_ABOVE bundles, else two contiguous halves in bundle-id
    (anchor FormID) order. Related bundles can land in different halves;
    the writers resolve those through deferrals. Stale part files are
    removed."""
    for stale in layout.work_dir(out_dir).glob("deep-slice.part*.json"):
        stale.unlink()
    full = layout.work_deep_slice_json(out_dir)
    payload = jsonio.read(full)
    bundles = payload["bundles"]
    if len(bundles) <= SPLIT_DEEP_ABOVE:
        return [full]
    half = (len(bundles) + 1) // 2
    parts = []
    for part, chunk in enumerate((bundles[:half], bundles[half:]), start=1):
        path = layout.work_deep_slice_part_json(out_dir, part)
        lints = triage_bundles.lints_for_bundles(chunk, triage_bundles.lints_index(payload["lints"]))
        jsonio.write(path, {"bundles": chunk, "lints": lints})
        parts.append(path)
    return parts


def tier_warnings(out_dir: Path) -> list[str]:
    """Signs a tier rule misfired, for the orchestrator to inspect in
    `work/triage.json`'s reasons before dispatching writers."""
    triage = jsonio.read(layout.work_triage_json(out_dir))
    warnings = []
    if len(triage["deep"]) > DEEP_SANITY_LIMIT:
        warnings.append(f"{len(triage['deep'])} DEEP bundles (over {DEEP_SANITY_LIMIT}): check the deep reasons")
    bundles = {b["id"]: b for b in jsonio.read(layout.bundles_json(out_dir))["bundles"]}
    dropped = {
        m["record_type"]
        for bid in triage["drop"]
        if triage["reasons"].get(bid) != "drop:reorder_only"
        for m in bundles.get(bid, {}).get("members", [])
        if m["role"] != "context"
    }
    for sig in DROP_WATCH_TYPES:
        if sig in dropped:
            warnings.append(f"DROP holds {sig} records: check the drop reasons")
    return warnings


def triage_summary(out_dir: Path) -> dict:
    triage = jsonio.read(layout.work_triage_json(out_dir))
    return {tier: len(triage[tier]) for tier in ("rollout", "deep", "brief", "drop", "ambiguous")}


def build_cache(esm_bin: Path, esm: Path) -> None:
    """Build every cache section for `esm` up front, so no writer's first
    query waits on it."""
    subprocess.run([str(esm_bin), "--esm", str(esm), "cache", "build"], check=True, stdout=subprocess.DEVNULL)


def coverage_gate(esm_bin: Path, esm: Path) -> tuple[bool, dict | None, str]:
    """`esm coverage --gate` over `esm`: whether every record decodes with
    no gap (raw fallbacks, malformed or trailing bytes, unmapped subrecords,
    unknown records), the marker totals, and the gate's error text."""
    proc = subprocess.run(
        [str(esm_bin), "--esm", str(esm), "coverage", "--gate", "--json"],
        capture_output=True,
        text=True,
    )
    try:
        totals = json.loads(proc.stdout).get("totals")
    except (ValueError, AttributeError):
        totals = None
    return proc.returncode == 0, totals, proc.stderr.strip()


# --------------------------------------------------------------------------
# Verbs
# --------------------------------------------------------------------------


@json_verb
def prepare(argv: list[str] | None = None, *, client=None) -> tuple[int, dict | None]:
    ap = argparse.ArgumentParser(
        prog="pn prepare",
        description="Resolve the snapshots, run or reuse the mechanical stage, build the new "
        "snapshot's cache, triage, and slice the DEEP tier for the writers.",
    )
    ap.add_argument("snapshots", nargs="*", help="[OLD] NEW: tokens under $FO76_DATA_DIR or absolute dirs")
    ap.add_argument("--out-dir", type=Path, default=None)
    ap.add_argument("--force-pipeline", action="store_true", help="Re-run the mechanical stage")
    ap.add_argument(
        "--retriage",
        action="store_true",
        help="Re-run rule triage on reused output (drops a merged assessment)",
    )
    ap.add_argument("--official-notes", default=None, metavar="URL_OR_FILE")
    ap.add_argument(
        "--exclude-type",
        default=mpn.DEFAULT_EXCLUDE_TYPE,
        metavar="LIST",
        help=f"Record types the diff omits (default: {mpn.DEFAULT_EXCLUDE_TYPE})",
    )
    ap.add_argument("--esm-bin", default=None)
    args = ap.parse_args(argv)

    root = data_dir()
    try:
        old_dir, new_dir = resolve_snapshots(args.snapshots, root)
        esm_bin = esmcli.find_esm_binary(args.esm_bin)
    except (ValueError, esmcli.EsmError) as exc:
        eprint(f"error: {exc}")
        return 1, None
    old_esm = mpn.resolve_esm(old_dir, "Old")
    new_esm = mpn.resolve_esm(new_dir, "New")
    out_dir = args.out_dir or default_out_dir(old_esm, new_esm, root)

    exclude_type = args.exclude_type.strip()
    run_args = [str(old_dir), str(new_dir), "--out-dir", str(out_dir), "--esm-bin", str(esm_bin)]
    run_args += ["--exclude-type", exclude_type]
    options = mpn.output_options(mpn.build_arg_parser().parse_args(run_args))
    problem = "--force-pipeline" if args.force_pipeline else reuse_problem(out_dir, old_esm, new_esm, options)
    reused = problem is None
    if not reused:
        eprint(f"running the mechanical stage ({problem})")
        if args.force_pipeline:
            run_args.append("--force-pipeline")
        rc = mpn.main(run_args, client=client)
        if rc:
            return rc, None

    notes = "none"
    if args.official_notes:
        source = Path(args.official_notes)
        target = layout.work_official_notes_txt(out_dir)
        if source.suffix == ".txt" and source.is_file():
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
            notes = "copied"
        else:
            rc = fetch_official_notes.main([args.official_notes, str(target)])
            notes = {0: "fetched", 3: "client_rendered"}.get(rc, "failed")

    eprint("building the new snapshot's cache")
    try:
        build_cache(esm_bin, new_esm)
    except subprocess.CalledProcessError as exc:
        eprint(f"error: esm cache build failed ({exc.returncode})")
        return 1, None
    # A decode gap in the new snapshot would reach the writers as missing
    # or wrong data (see skill/kb/pipeline-gotchas.md): stop here instead.
    eprint("checking the new snapshot's decode coverage")
    covered, coverage, coverage_error = coverage_gate(esm_bin, new_esm)
    if not covered:
        eprint(f"error: esm coverage --gate failed: {coverage_error or 'no detail'}")
        return 1, None
    # Reused output keeps its triage, including a merged assessment, unless
    # asked; the pipeline having run, or no usable triage, means triaging now.
    # Repairing a kept triage re-applies its merged assessment.
    warnings: list[str] = []
    stale_triage = triage_problem(out_dir) if reused else None
    if stale_triage and stale_triage != "no triage yet":
        eprint(f"re-triaging: the kept triage is unusable ({stale_triage})")
    retriaged = not reused or args.retriage or stale_triage is not None
    kept_verdicts = assessor_verdicts(out_dir) if stale_triage and not args.retriage else {}
    remerge = bool(kept_verdicts)
    if retriaged and remerge:
        try:
            triage_bundles.run_merge_assessment(out_dir, layout.work_assessment_json(out_dir))
            eprint("re-merged work/assessment.json into the repaired triage")
        except (OSError, ValueError, TypeError, KeyError) as exc:
            remerge = False
            warnings.append(
                f"the kept triage's merged assessment couldn't be re-applied ({exc}): "
                "re-run the assessor and merge-assessment"
            )
        else:
            restored = assessor_verdicts(out_dir)
            # A kept tier the lenient read couldn't place matches any tier.
            lost = sorted(
                bid
                for bid, (tier, reason) in kept_verdicts.items()
                if bid not in restored or restored[bid][1] != reason or tier not in (None, restored[bid][0])
            )
            if lost:
                warnings.append(
                    f"work/assessment.json doesn't restore the kept triage's verdicts for {', '.join(lost)}: "
                    "re-run the assessor and merge-assessment"
                )
    if retriaged and not remerge:
        triage_bundles.run_triage(out_dir)
    slices = split_deep_slice(out_dir)

    return 0, (
        {
            "out_dir": str(out_dir),
            "old": {"token": mpn.esm_token(old_esm), "esm": str(old_esm)},
            "new": {"token": mpn.esm_token(new_esm), "esm": str(new_esm)},
            "reused": reused,
            "retriaged": retriaged,
            "coverage": coverage,
            "official_notes": notes,
            "tiers": triage_summary(out_dir),
            "ambiguous_json": str(layout.work_ambiguous_json(out_dir)),
            "deep_slices": [str(p) for p in slices],
            "warnings": [*warnings, *tier_warnings(out_dir)],
        }
    )


@json_verb
def merge_assessment(argv: list[str] | None = None) -> tuple[int, dict | None]:
    ap = argparse.ArgumentParser(
        prog="pn merge-assessment",
        description="Fold work/assessment.json's tiers into triage and re-slice the DEEP tier.",
    )
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--assessment", type=Path, default=None, help="Default: <out_dir>/work/assessment.json")
    args = ap.parse_args(argv)
    assessment = args.assessment or layout.work_assessment_json(args.out_dir)
    try:
        result = triage_bundles.run_merge_assessment(args.out_dir, assessment)
    except (OSError, ValueError, TypeError, KeyError) as exc:
        eprint(f"error: {exc}")
        return 1, None
    slices = split_deep_slice(args.out_dir)
    return 0, (
        {
            "resolved_by_assessor": result["triage"]["stats"].get("resolved_by_assessor", 0),
            "tiers": triage_summary(args.out_dir),
            "deep_slices": [str(p) for p in slices],
            "warnings": tier_warnings(args.out_dir),
        }
    )


@json_verb
def gate(argv: list[str] | None = None, *, client=None) -> tuple[int, dict | None]:
    ap = argparse.ArgumentParser(
        prog="pn gate",
        description="Re-derive every drafted claim and check every DEEP bundle is covered "
        "(and, with --summary, reaches the summary or work/cuts.json). Exit 1 on any failure.",
    )
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--summary", action="store_true", help="Also check patch-summary.md coverage")
    ap.add_argument("--old-esm", required=True, help="The run's OLD ESM (prepare prints it)")
    ap.add_argument("--new-esm", required=True, help="The run's NEW ESM (prepare prints it)")
    ap.add_argument("--esm-bin", default=None)
    args = ap.parse_args(argv)

    claims_args = [str(args.out_dir), "--old-esm", args.old_esm, "--new-esm", args.new_esm]
    if args.esm_bin:
        claims_args += ["--esm-bin", args.esm_bin]
    try:
        claims_rc = check_claims.main(claims_args, client=client)
        coverage_rc = check_coverage.main([str(args.out_dir), *(["--summary"] if args.summary else [])])
        claims = jsonio.read(layout.work_claims_check_json(args.out_dir))
        coverage = jsonio.read(layout.work_coverage_json(args.out_dir))
    except (OSError, ValueError, TypeError, KeyError) as exc:
        eprint(f"error: {exc}")
        return 1, None
    ok = claims_rc == 0 and coverage_rc == 0
    return (0 if ok else 1), (
        {
            "ok": ok,
            "live_lookups": claims["live_lookups"],
            "claims": {
                "checked": claims["checked"],
                "mismatch": claims["mismatch_count"],
                "unverifiable": claims["unverifiable_count"],
                "invalid_reports": claims["invalid_count"],
            },
            "coverage_violations": coverage["violations"],
        }
    )


@json_verb
def publish(argv: list[str] | None = None) -> tuple[int, dict | None]:
    ap = argparse.ArgumentParser(
        prog="pn publish",
        description="Validate work/review.json, chunk patch-summary.md for Discord, and record "
        "the narrative stage in manifest.json.",
    )
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--allow-oversize", action="store_true", help="Accept a hard-truncated chunk")
    ap.add_argument(
        "--no-review",
        action="store_true",
        help="Publish without work/review.json (no independent reviewer was available)",
    )
    args = ap.parse_args(argv)
    out_dir = args.out_dir

    review = layout.work_review_json(out_dir)
    findings: dict[str, int] = {}
    if not review.is_file() and not args.no_review:
        eprint(f"error: no {review.name}; run the cold review, or pass --no-review when none is possible")
        return 1, None
    if review.is_file():
        try:
            for finding in schemas.load(review, schemas.validate_review)["findings"]:
                findings[finding["severity"]] = findings.get(finding["severity"], 0) + 1
        except (OSError, ValueError, TypeError, KeyError) as exc:
            eprint(f"error: {exc}")
            return 1, None

    chunk_args = [str(layout.patch_summary_md(out_dir)), str(layout.discord_dir(out_dir))]
    rc = discord_chunker.main(chunk_args + (["--allow-oversize"] if args.allow_oversize else []))
    if rc:
        return rc, None
    rc = update_manifest.main([str(out_dir)])
    if rc:
        return rc, None
    narrative = (pl.load_manifest(out_dir) or {})["stages"]["narrative"]
    return 0, (
        {
            "reviewed": review.is_file(),
            "review_findings": findings,
            "chunks": narrative["chunk_count"],
            "usage": narrative.get("usage"),
            "patch_summary": str(layout.patch_summary_md(out_dir)),
            "discord_dir": str(layout.discord_dir(out_dir)),
        }
    )
