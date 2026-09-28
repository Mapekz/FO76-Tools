#!/usr/bin/env python3
"""
make_patch_notes.py — mechanical-stage orchestrator for the FO76 patch-notes
pipeline.

Wires together, in-process, the deterministic pipeline steps that turn a raw
`esm diff --json` into a reviewable, bundled, linted output directory:

    1. `esm diff` (subprocess)              -> diff.json
    2. render_comprehensive.py (library)    -> comprehensive.json
    3. build_bundles.py (library)           -> bundles.json
    4. run_lints.py (library)               -> lints.json
    5. patchnotes_lib.py (manifest helpers) -> manifest.json

This is the **mechanical** stage only — deterministic, no LLM involved. The
narrative stage (triage, deep writers, gates, Discord chunking,
`update_manifest.py`) is the `/patch-notes` skill; see
`../skill/SKILL.md`.

Usage:
    python3 -m pn run OLD.esm NEW.esm [options]

Options:
    Without source flags, `esm diff` discovers each side's strings and curve
    tables from that ESM's own snapshot folder. The flags below override that
    and pass straight through to `esm diff`, which validates them.

    --strings-dir DIR     Strings directory for both ESMs.
    --strings-dir-a DIR   Strings directory for ESM A only.
    --strings-dir-b DIR   Strings directory for ESM B only.
    --startup-ba2 PATH    Startup BA2 for curve tables, both ESMs (or env STARTUP_BA2).
    --curves-dir DIR      Loose misc/ directory for curve tables, both ESMs
                          (or env CURVES_DIR).
    --lang LANG           Localization language code (default: en)
    --out-dir DIR         Output directory. Default: patch_<OLDTOK>_to_<NEWTOK>/
                          next to NEW.esm.
    --esm-bin PATH        Path to the esm binary (default: the repo's target/release/esm,
                          else esm on $PATH).
    --type SIG            Only include records of this type (passed to esm diff).
    --bodies LEVEL        Detail level for decoded fields on added/removed record
                          stubs: none|stub|full (default: stub; `full` recursively
                          resolves every added record body and can OOM the diff).
    --force-pipeline      Overwrite an out-dir whose manifest already records a
                          completed narrative stage. Without it the run is refused,
                          so a stray sanity-check run can never clobber a finished
                          week's notes -- use a scratch --out-dir for those.
    --keep-noise          Keep noisy fields (placement transforms, CELL precombine
                          bookkeeping, Object Bounds) instead of suppressing them.
    --exclude-type LIST   Comma-delimited record-type signatures to omit entirely
                          (default: LAND,NAVM). Pass --exclude-type '' to disable.
    --refs-depth N        Override the bundles stage's base reverse-ref BFS depth.
    --skip-bundles        Skip bundles.json (and, necessarily, lints.json).
    --skip-lints          Skip lints.json (bundles.json is still built).
    -v, --verbose         Show full diff command + esm output.

Exit codes:
    0  Success
    1  Input validation error (missing files, missing strings, etc.)
    2  esm diff failed (non-zero exit, or produced unparsable JSON)
    3  A downstream tooling stage failed (comprehensive/bundles/lints)

Examples:
    # Two snapshot folders; each side's strings and curves are discovered there.
    python3 -m pn run /path/to/v1/ /path/to/v2/

    # One strings directory for both sides.
    python3 -m pn run /path/to/old/ /path/to/new/ \\
        --strings-dir /path/to/strings

    # With Startup BA2 for curve-table detail:
    STARTUP_BA2="/path/to/startup.ba2" \\
    python3 -m pn run /path/to/v1/ /path/to/v2/

    # Via justfile:
    just run /path/to/v1/ /path/to/v2/
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import NoReturn

from pn import build_bundles as bb
from pn import esmcli as eg

# Locate the esm/ workspace root (directory containing this script's parent).
# Sibling pipeline-tool modules live next to this script.
from pn import jsonio, layout, schemas
from pn import patchnotes_lib as pl
from pn import render_comprehensive as rc
from pn import run_lints as rl

build_diff_cmd = eg.build_diff_cmd

# Orchestrator default for --exclude-type: world-placement/positional records
# that are noisy and not meaningfully decoded (mirrors change_entries.EXCLUDED_TYPES'
# WRLD/CELL exclusion, but applied at the Rust diff level to shrink diff.json
# itself rather than filtering after the fact).
DEFAULT_EXCLUDE_TYPE = "LAND,NAVM"

# --------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def banner(msg):
    bar = "─" * min(len(msg) + 4, 72)
    eprint(f"\n{bar}")
    eprint(f"  {msg}")
    eprint(f"{bar}")


def die(code, msg) -> NoReturn:
    eprint(f"\n❌  {msg}")
    sys.exit(code)


def _now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def version_token(esm_path: Path) -> str:
    """Extract the 8-digit date token from an ESM stem, or return the stem itself."""
    stem = esm_path.stem
    m = re.search(r"\d{6,}", stem)
    return m.group(0) if m else stem


def esm_token(esm_path: Path) -> str:
    """Version token used for the default out-dir name and manifest
    `inputs.{old,new}_token` — `version_token()` of the ESM's own stem,
    unless the stem carries no run of >=4 digits, in which case fall back to
    the parent directory's name (this pipeline's snapshot layout dates the
    *parent directory*, not the file itself, e.g.
    `$FO76_DATA_DIR/20260703/SeventySix.esm` — mirrors
    render_comprehensive.py's `_label_for_esm`, which makes the same choice
    for display labels)."""
    stem = esm_path.stem
    if not re.search(r"\d{4,}", stem) and esm_path.parent.name:
        return version_token(esm_path.parent)
    return version_token(esm_path)


def derive_patch_date(esm_path: Path) -> str:
    tok = version_token(esm_path)
    if len(tok) == 8 and tok.isdigit():
        return f"{tok[:4]}-{tok[4:6]}-{tok[6:]}"
    return tok


def default_out_dir(esm_a: Path, esm_b: Path) -> Path:
    return esm_b.parent / f"patch_{esm_token(esm_a)}_to_{esm_token(esm_b)}"


def narrative_completed_at(out_dir: Path) -> str | None:
    """`stages.narrative.completed_at` from an existing `<out_dir>/manifest.json`,
    or None when there is no manifest, it is unreadable, or the narrative
    stage never completed. Never raises."""
    try:
        manifest = pl.load_manifest(out_dir)
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(manifest, dict):
        return None
    narrative = (manifest.get("stages") or {}).get("narrative") or {}
    value = narrative.get("completed_at") if isinstance(narrative, dict) else None
    return value if isinstance(value, str) and value else None


def resolve_esm(path: Path, label: str) -> Path:
    """Resolve *path* to a concrete `.esm` file, mirroring the Rust CLI behaviour.

    * **File input** — used directly after verifying it exists.
    * **Directory input** — scanned (non-recursively) for exactly one `*.esm`
      file (case-insensitive).  Zero or multiple `.esm` files are an error.
    """
    p = path.resolve()
    if p.is_dir():
        esms = sorted(c for c in p.iterdir()
                      if c.is_file() and c.suffix.lower() == ".esm")
        if len(esms) == 1:
            return esms[0]
        if not esms:
            die(1, f"no .esm file found in {p}")
        names = "\n  ".join(e.name for e in esms)
        die(1, f"multiple .esm files in {p}; pass the file path directly:\n  {names}")
    if not p.is_file():
        die(1, f"{label} ESM not found: {p}")
    return p


def source_args(args: argparse.Namespace) -> list[str]:
    """The explicit string and curve sources, as `esm diff` flags. Without
    them `esm diff` discovers each side's sources from its own folder."""
    out: list[str] = []
    curves_dir = args.curves_dir
    if args.startup_ba2 and curves_dir:
        eprint("  note: --startup-ba2 wins over --curves-dir (esm takes one curve source)")
        curves_dir = None
    for flag, value in (
        ("--strings-dir", args.strings_dir),
        ("--strings-dir-a", args.strings_dir_a),
        ("--strings-dir-b", args.strings_dir_b),
        ("--startup-ba2", args.startup_ba2),
        ("--curves-dir", curves_dir),
    ):
        if value:
            out += [flag, str(Path(value).resolve())]
    return out


def output_options(args: argparse.Namespace) -> dict:
    """The options that shape the mechanical stage's output, as its manifest
    records them: a run is reusable only for the same ones."""
    return {
        "record_type": args.record_type,
        "bodies": args.bodies,
        "keep_noise": args.keep_noise,
        "exclude_type": (args.exclude_type or "").strip(),
        "refs_depth": args.refs_depth,
        "lang": args.lang,
        "sources": source_args(args),
    }


# --------------------------------------------------------------------------
# Step 2: Run esm diff
# --------------------------------------------------------------------------

# `build_diff_cmd` lives in esmcli.py (re-exported above as
# `build_diff_cmd = eg.build_diff_cmd` so call sites/tests reach it as
# `mpn.build_diff_cmd`); this stage's transport is `esmcli.EsmGateway.diff`
# (see its docstring for why it is a subprocess, not an `Op::Diff` request).


def run_esm_diff(
    esm_bin: Path,
    esm_a: Path,
    esm_b: Path,
    *,
    sources: list[str],
    lang: str,
    json_out: Path,
    record_type: str | None,
    bodies: str,
    keep_noise: bool,
    exclude_type: str,
    verbose: bool,
) -> dict:
    """CLI-output wrapper (banner/progress/exit-code translation) around
    `EsmGateway.diff`, which does the actual subprocess/JSON-parsing work."""
    banner("Step 2: Running esm diff")
    eprint(f"  A:           {esm_a}")
    eprint(f"  B:           {esm_b}")
    eprint(f"  sources:     {' '.join(sources) or 'discovered per snapshot by esm'}")
    eprint(f"  bodies:      {bodies}")
    if keep_noise:
        eprint("  keep-noise:  true")
    if exclude_type:
        eprint(f"  exclude-type: {exclude_type}")
    eprint(f"  json output: {json_out}")
    if record_type:
        eprint(f"  --type filter: {record_type}")

    t_start = time.time()
    try:
        result = eg.EsmGateway.diff(
            esm_bin, esm_a, esm_b,
            sources=sources, lang=lang, record_type=record_type, bodies=bodies,
            keep_noise=keep_noise, exclude_type=exclude_type,
        )
    except eg.EsmError as exc:
        die(2,
            f"esm diff failed.\n{exc}\n"
            "Check the error above. Common causes:\n"
            "  - A snapshot folder without strings/ (pass --strings-dir-a/-b)\n"
            "  - ESM not found or unreadable\n"
            "  - Stale binary: rebuild with `just release` in esm/")
    t_elapsed = time.time() - t_start

    if verbose:
        eprint(f"\n  Command: {' '.join(result.cmd)}")
        if result.stderr:
            eprint(result.stderr)

    # Write JSON to disk -- result.raw_json is the exact text esm produced,
    # so this matches byte-for-byte.
    json_out.parent.mkdir(parents=True, exist_ok=True)
    with open(json_out, "w") as f:
        f.write(result.raw_json)

    data = result.data
    eprint(f"\n  ✓ Done in {t_elapsed:.1f}s")
    eprint(f"    added={len(data.get('added',[]))}, "
           f"removed={len(data.get('removed',[]))}, "
           f"changed={len(data.get('changed',[]))}, "
           f"ref_names={len(data.get('ref_names',{}))}")
    return data


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="pn run",
        description="ESM diff -> comprehensive.json -> bundles.json -> lints.json "
                     "-> manifest.json. The mechanical (deterministic) half of the "
                     "patch-notes pipeline; no LLM involved.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    ap.add_argument("old_esm", type=Path,
                    help="Old ESM file, or directory containing exactly one .esm")
    ap.add_argument("new_esm", type=Path,
                    help="New ESM file, or directory containing exactly one .esm")
    ap.add_argument("--strings-dir", default=None, metavar="DIR",
                    help="Strings directory for both ESMs (default: each side's own, found by esm)")
    ap.add_argument("--strings-dir-a", default=None, metavar="DIR",
                    help="Strings directory for ESM A only")
    ap.add_argument("--strings-dir-b", default=None, metavar="DIR",
                    help="Strings directory for ESM B only")
    ap.add_argument("--startup-ba2", default=os.environ.get("STARTUP_BA2"), metavar="PATH",
                    help="Startup BA2 for curve tables, both ESMs (also $STARTUP_BA2)")
    ap.add_argument("--curves-dir", default=os.environ.get("CURVES_DIR"), metavar="DIR",
                    help="Loose misc/ directory for curve tables, both ESMs (also $CURVES_DIR)")
    ap.add_argument("--lang", default="en", metavar="LANG",
                    help="Localization language code (default: en)")
    ap.add_argument("--out-dir", default=None, metavar="DIR",
                    help="Output directory (default: patch_<OLDTOK>_to_<NEWTOK>/ next to NEW.esm)")
    ap.add_argument("--esm-bin", default=None, metavar="PATH",
                    help="Path to the esm binary (default: target/release/esm or $PATH)")
    ap.add_argument("--type", default=None, dest="record_type", metavar="TYPE",
                    help="Only include records of this type (passed to esm diff)")
    ap.add_argument("--bodies", default="stub", choices=["none", "stub", "full"], metavar="LEVEL",
                    help="Detail level for decoded fields on added/removed record stubs "
                         "(default: stub; full can OOM the diff on deeply linked added records)")
    ap.add_argument("--force-pipeline", action="store_true",
                    help="Overwrite an out-dir whose manifest records a completed narrative "
                         "stage (refused otherwise; use a scratch --out-dir for test runs)")
    ap.add_argument("--keep-noise", action="store_true",
                    help="Keep noisy fields (placement transforms, CELL precombine "
                         "bookkeeping, Object Bounds) instead of suppressing them")
    ap.add_argument("--exclude-type", default=DEFAULT_EXCLUDE_TYPE, metavar="LIST",
                    help=f"Comma-delimited record-type signatures to omit entirely "
                         f"(default: {DEFAULT_EXCLUDE_TYPE}). Pass --exclude-type '' to disable.")
    ap.add_argument("--refs-depth", type=int, default=None, metavar="N",
                    help="Override the bundles stage's base reverse-ref BFS depth")
    ap.add_argument("--skip-bundles", action="store_true",
                    help="Skip bundles.json (and, necessarily, lints.json)")
    ap.add_argument("--skip-lints", action="store_true",
                    help="Skip lints.json (bundles.json is still built)")
    ap.add_argument("-v", "--verbose", action="store_true",
                    help="Show full commands + esm output")
    return ap


def main(argv=None, *, client=None):
    """`client` replaces the live `esmcli.EsmGateway` for the bundles and
    lints stages (tests pass a fixture-backed stand-in)."""
    args = build_arg_parser().parse_args(argv)

    t0 = time.time()
    files_written: dict[str, str] = {}

    # ---- Step 1: Validate inputs -------------------------------------------
    banner("Step 1: Validating inputs")

    esm_a = resolve_esm(args.old_esm, "Old")
    esm_b = resolve_esm(args.new_esm, "New")

    eprint(f"  OLD: {esm_a}  ({esm_a.stat().st_size:,} bytes)")
    eprint(f"  NEW: {esm_b}  ({esm_b.stat().st_size:,} bytes)")
    eprint(f"  patch date (hint): {derive_patch_date(esm_b)}")

    try:
        esm_bin = eg.find_esm_binary(args.esm_bin)
    except eg.EsmError as exc:
        die(1, str(exc))
    eprint(f"  esm binary: {esm_bin}")

    sources = source_args(args)
    if sources:
        eprint(f"  sources: {' '.join(sources)}")

    out_dir = Path(args.out_dir).resolve() if args.out_dir else default_out_dir(esm_a, esm_b)
    completed_at = narrative_completed_at(out_dir)
    if completed_at and not args.force_pipeline:
        die(1,
            f"{out_dir} already holds a finished narrative stage (completed {completed_at}).\n"
            f"Re-running the mechanical stage would clobber that run's diff/bundles/manifest.\n"
            f"Pass --force-pipeline to overwrite it on purpose, or use a scratch --out-dir.")
    out_dir.mkdir(parents=True, exist_ok=True)
    eprint(f"  out dir: {out_dir}")
    # This run replaces the directory's artifacts. Until it finishes, nothing
    # here may pass for a finished run: the manifest marks a complete
    # mechanical stage, and triage.json a triage of that stage's bundles.
    layout.manifest_json(out_dir).unlink(missing_ok=True)
    layout.work_triage_json(out_dir).unlink(missing_ok=True)

    old_token = esm_token(esm_a)
    new_token = esm_token(esm_b)

    exclude_type = (args.exclude_type or "").strip()

    overrides = {} if args.refs_depth is None else {"refs_depth": args.refs_depth}

    localized = {"old": pl.esm_is_localized(esm_a), "new": pl.esm_is_localized(esm_b)}
    if None not in localized.values() and localized["old"] != localized["new"]:
        eprint(f"  Localized flag flips: {localized['old']} -> {localized['new']}. Text decodes the same "
               f"either way; string-only whitespace rewrites are counted as localization_flip_text.")

    # ---- Step 2: esm diff ---------------------------------------------------
    diff_json_path = layout.diff_json(out_dir)
    diff_data = run_esm_diff(
        esm_bin, esm_a, esm_b,
        sources=sources,
        lang=args.lang,
        json_out=diff_json_path,
        record_type=args.record_type,
        bodies=args.bodies,
        keep_noise=args.keep_noise,
        exclude_type=exclude_type,
        verbose=args.verbose,
    )
    files_written["diff"] = diff_json_path.name

    # ---- Step 3: comprehensive.json ----------------------------------------
    banner("Step 3: Building comprehensive.json")
    t_start = time.time()
    try:
        old_label, new_label, patch_date = rc.derive_labels_and_date(
            str(diff_json_path), str(esm_a), str(esm_b), None, None, None,
        )
        comp = rc.build_comprehensive(
            diff_data,
            old_esm=str(esm_a), new_esm=str(esm_b),
            old_label=old_label, new_label=new_label, patch_date=patch_date,
        )
        comp_json_path = layout.comprehensive_json(out_dir)
        jsonio.write(comp_json_path, comp)
    except Exception as e:
        die(3, f"building comprehensive.json failed: {e}")
    files_written["comprehensive_json"] = comp_json_path.name

    counts = dict(comp["meta"]["counts"])
    eprint(f"\n  ✓ Done in {time.time() - t_start:.1f}s "
           f"({counts.get('added', 0)} added, {counts.get('changed', 0)} changed, "
           f"{counts.get('removed', 0)} removed)")

    # ---- Steps 4 + 5: bundles.json / lints.json ------------------------------
    bundles_result = None
    lints_payload = None
    owns_client = client is None
    try:
        if args.skip_bundles:
            eprint("\nSkipping bundles.json and lint checks (--skip-bundles)")
            # A previous run's files would otherwise be read as this run's.
            layout.bundles_json(out_dir).unlink(missing_ok=True)
            layout.lints_json(out_dir).unlink(missing_ok=True)
        else:
            banner("Step 4: Building bundles.json")
            t_start = time.time()
            try:
                if client is None:
                    client = eg.EsmGateway(esm_bin)
                bundles_result = bb.build_bundles(comp, client, str(esm_a), str(esm_b), overrides)
                bundles_json_path = layout.bundles_json(out_dir)
                jsonio.write(bundles_json_path, bundles_result)
            except Exception as e:
                die(3, f"building bundles.json failed: {e}")
            files_written["bundles"] = bundles_json_path.name
            bc = bundles_result["meta"]["counts"]
            eprint(f"\n  ✓ Done in {time.time() - t_start:.1f}s "
                   f"({bc['bundles']} bundles, {bc['singletons']} singletons)")

            if args.skip_lints:
                eprint("\nSkipping lint checks (--skip-lints)")
                layout.lints_json(out_dir).unlink(missing_ok=True)
            else:
                banner("Step 5: Running lint checks")
                t_start = time.time()
                try:
                    lints_payload = rl.run_lints(
                        comp, bundles_result, client,
                        new_esm=str(esm_b),
                    )
                    lints_json_path = layout.lints_json(out_dir)
                    jsonio.write(lints_json_path, lints_payload)
                except Exception as e:
                    die(3, f"running lint checks failed: {e}")
                files_written["lints"] = lints_json_path.name
                lc = lints_payload["meta"]["counts"]
                eprint(f"\n  ✓ Done in {time.time() - t_start:.1f}s "
                       f"(error={lc['error']} warn={lc['warn']} info={lc['info']})")
    finally:
        if owns_client and client is not None:
            client.close()

    # ---- Step 6: manifest.json -----------------------------------------------
    banner("Step 6: Writing manifest.json")

    manifest_counts = dict(counts)
    if bundles_result is not None:
        manifest_counts.update(bundles_result["meta"]["counts"])
    if lints_payload is not None:
        manifest_counts["lints"] = lints_payload["meta"]["counts"]

    manifest = pl.new_manifest(
        patch_date=patch_date,
        old_token=old_token,
        new_token=new_token,
        new_esm_size=esm_b.stat().st_size,
        new_esm_mtime=int(esm_b.stat().st_mtime),
        old_esm_size=esm_a.stat().st_size,
        old_esm_mtime=int(esm_a.stat().st_mtime),
        options=output_options(args),
        pipeline_version=schemas.PIPELINE_VERSION,
        counts=manifest_counts,
        localized=localized,
        exclude_type=exclude_type,
    )
    manifest["stages"]["mechanical"]["completed_at"] = _now_iso()
    manifest["stages"]["mechanical"]["files"] = dict(files_written)
    pl.write_manifest(out_dir, manifest)
    files_written["manifest"] = layout.manifest_json(out_dir).name
    eprint(f"  wrote {layout.manifest_json(out_dir)}")

    # ---- Step 7: summary -------------------------------------------------
    t_total = time.time() - t0
    banner("Done")
    for fname in files_written.values():
        eprint(f"  ✓ {out_dir / fname}")
    eprint(f"\n  Total time: {t_total:.1f}s")
    eprint(
        "\n  Narrative stage: the /patch-notes skill "
        "(pn prepare, then writers, pn gate, pn publish)."
    )
    eprint()
    return 0


if __name__ == "__main__":
    sys.exit(main())
