#!/usr/bin/env python3
"""
make_patch_notes.py — mechanical-stage orchestrator for the FO76 patch-notes
pipeline.

Wires together, in-process, the four deterministic pipeline tools that turn a
raw `esm diff --json` into a reviewable, bundled, linted output directory:

    1. `esm --local diff` (subprocess)      -> diff.json
    2. render_comprehensive.py (library)    -> comprehensive.json + .md
    3. build_bundles.py (library)           -> bundles.json
    4. run_lints.py (library)               -> lints.json + updated bundles.json
    5. patchnotes_lib.py (manifest helpers) -> manifest.json

This is the **mechanical** stage only — deterministic, no LLM involved. The
narrative stage (slicing, per-category writer subagents, Discord chunking,
`update_manifest.py`) is the `/patch-notes` Claude skill; see
`../.claude/skills/patch-notes/SKILL.md`.

Usage:
    python3 tools/make_patch_notes.py OLD.esm NEW.esm [options]

Options:
    --strings-dir DIR     Shared strings directory for both ESMs. Auto-detected from
                          <esm_parent>/strings/ (or <esm_parent> itself) if omitted.
    --strings-dir-a DIR   Strings directory for ESM A only (overrides --strings-dir for A).
    --strings-dir-b DIR   Strings directory for ESM B only (overrides --strings-dir for B).
    --startup-ba2 PATH    Path to a Startup BA2 (or env STARTUP_BA2).
                          Enables curve-table inlining and crafting-quantity evaluation.
                          Mutually exclusive with --curves-dir.
    --curves-dir DIR      Path to the misc/ directory extracted from a Startup BA2
                          (or env CURVES_DIR). Loose alternative to --startup-ba2;
                          curve JSON is read from <dir>/curvetables/json/.
                          Auto-detected from <new_esm_parent>/misc/ if omitted.
                          Mutually exclusive with --startup-ba2.
    --lang LANG           Localization language code (default: en)
    --out-dir DIR         Output directory. Default: patch_<OLDTOK>_to_<NEWTOK>/
                          next to NEW.esm.
    --esm-bin PATH        Path to the esm binary (default: target/release/esm relative to
                          the esm/ workspace root, or whatever is on $PATH as 'esm').
    --type SIG            Only include records of this type (passed to esm diff).
    --bodies LEVEL        Detail level for decoded fields on added/removed record
                          stubs: none|stub|full (default: full).
    --keep-noise          Keep noisy fields (placement transforms, CELL precombine
                          bookkeeping, Object Bounds) instead of suppressing them.
    --exclude-type LIST   Comma-delimited record-type signatures to omit entirely
                          (default: LAND,NAVM). Pass --exclude-type '' to disable.
    --categories FILE     Path to patch_notes_categories.json (default: the copy
                          next to this script).
    --refs-depth N        Override the categorization config's base reverse-ref
                          BFS depth.
    --skip-bundles        Skip bundles.json (and, necessarily, lints.json).
    --skip-lints          Skip lints.json (bundles.json is still built).
    --offline             Use esm_gateway.FakeGateway instead of a live warm daemon
                          for the bundles/lints stages (requires --refs-fixture).
    --refs-fixture F      FakeGateway fixture JSON (required with --offline).
    -v, --verbose         Show full diff command + esm output.

Exit codes:
    0  Success
    1  Input validation error (missing files, missing strings, etc.)
    2  esm diff failed (non-zero exit, or produced unparsable JSON)
    3  A downstream tooling stage failed (comprehensive/bundles/lints)

Examples:
    # Each ESM in its own directory; strings auto-detected from <dir>/strings/.
    python3 tools/make_patch_notes.py /path/to/v1/ /path/to/v2/

    # Shared strings directory (both ESMs in the same folder, or explicit path).
    python3 tools/make_patch_notes.py /path/to/old/ /path/to/new/ \\
        --strings-dir /path/to/strings

    # With Startup BA2 for curve-table detail:
    STARTUP_BA2="/path/to/startup.ba2" \\
    python3 tools/make_patch_notes.py /path/to/v1/ /path/to/v2/

    # Via justfile:
    just patch-notes /path/to/v1/ /path/to/v2/
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

# Locate the esm/ workspace root (directory containing this script's parent).
SCRIPT_DIR = Path(__file__).resolve().parent
WORKSPACE_ROOT = SCRIPT_DIR.parent  # esm/
DEFAULT_CATEGORIES_PATH = SCRIPT_DIR / "patch_notes_categories.json"

# Sibling pipeline-tool modules live next to this script.
sys.path.insert(0, str(SCRIPT_DIR))

import build_bundles as bb  # noqa: E402
import esm_gateway as eg  # noqa: E402
import patchnotes_lib as pl  # noqa: E402
import render_comprehensive as rc  # noqa: E402
import run_lints as rl  # noqa: E402
from esm_gateway import build_diff_cmd  # noqa: E402  # re-exported: TestBuildDiffCmd calls this via mpn.build_diff_cmd

# Orchestrator default for --exclude-type: world-placement/positional records
# that are noisy and not meaningfully decoded (mirrors patchnotes_lib.EXCLUDED_TYPES'
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


def locate_strings_dirs(
    esm_a: Path,
    esm_b: Path,
    explicit: str | None,
    explicit_a: str | None,
    explicit_b: str | None,
    lang: str,
) -> tuple[Path, Path]:
    """
    Return (strings_dir_a, strings_dir_b) — may be the same path for both sides.

    Strategy:
    - Explicit per-side flags (--strings-dir-a/b) take precedence.
    - --strings-dir applies to both sides as a shared dir.
    - Auto-detect: if both ESMs share a parent, look for a shared strings/ dir there.
      Otherwise, detect per-side from each ESM's own parent directory.
    """
    tok_a = version_token(esm_a)
    tok_b = version_token(esm_b)

    def has_any_strings(d: Path, tok: str) -> bool:
        """True if d contains at least one *_{lang}.{strings,dlstrings,ilstrings} for tok."""
        if not d.is_dir():
            return False
        for ext in ("strings", "dlstrings", "ilstrings"):
            # Match date-stamped names (*{tok}*_en.*) or plain names (*_en.*).
            if list(d.glob(f"*{tok}*_{lang}.{ext}")):
                return True
            if re.search(r"\d{6,}", tok) is None:
                # tok has no date digits (e.g. stem without version suffix) — also accept plain name
                if list(d.glob(f"*_{lang}.{ext}")):
                    return True
        return False

    # --- Explicit per-side overrides ---
    if explicit_a and explicit_b:
        da = Path(explicit_a).resolve()
        db = Path(explicit_b).resolve()
        if not da.is_dir():
            die(1, f"--strings-dir-a not a directory: {da}")
        if not db.is_dir():
            die(1, f"--strings-dir-b not a directory: {db}")
        return da, db

    # --- Shared explicit dir ---
    if explicit:
        d = Path(explicit).resolve()
        if not d.is_dir():
            die(1, f"--strings-dir not a directory: {d}")
        if has_any_strings(d, tok_a) and has_any_strings(d, tok_b):
            return d, d
        missing = []
        if not has_any_strings(d, tok_a):
            missing.append(f"ESM A ({esm_a.name})")
        if not has_any_strings(d, tok_b):
            missing.append(f"ESM B ({esm_b.name})")
        die(1, f"--strings-dir {d} missing string files for: {', '.join(missing)}")

    # --- Auto-detect: shared dir first (works when both ESMs are in the same parent) ---
    if explicit_a is None and explicit_b is None:
        shared_candidates: list[Path] = []
        seen: set[Path] = set()
        for esm in (esm_a, esm_b):
            for cand in [esm.parent / "strings", esm.parent]:
                r = cand.resolve()
                if r not in seen:
                    seen.add(r)
                    shared_candidates.append(r)
        for d in shared_candidates:
            if has_any_strings(d, tok_a) and has_any_strings(d, tok_b):
                return d, d

    # --- Auto-detect: per-side (each ESM in its own directory with a sibling strings/) ---
    def find_for_esm(esm: Path, tok: str) -> Path | None:
        for d in [esm.parent / "strings", esm.parent]:
            if has_any_strings(d.resolve(), tok):
                return d.resolve()
        return None

    eff_a = Path(explicit_a).resolve() if explicit_a else find_for_esm(esm_a, tok_a)
    eff_b = Path(explicit_b).resolve() if explicit_b else find_for_esm(esm_b, tok_b)

    if eff_a and eff_b:
        return eff_a, eff_b

    # --- Fail loudly ---
    missing_sides = []
    if not eff_a:
        missing_sides.append(f"ESM A ({esm_a.name})")
    if not eff_b:
        missing_sides.append(f"ESM B ({esm_b.name})")
    searched = [esm_a.parent / "strings", esm_a.parent, esm_b.parent / "strings", esm_b.parent]
    tried_str = "\n  ".join(str(d) for d in searched)
    die(1,
        f"Cannot find string files for: {', '.join(missing_sides)}\n"
        f"Searched (auto-detect):\n  {tried_str}\n\n"
        f"Supply --strings-dir (shared) or --strings-dir-a/--strings-dir-b (per side).\n"
        f"Refusing to diff without strings — output would be noise.")


# --------------------------------------------------------------------------
# Step 2: Run esm diff
# --------------------------------------------------------------------------

# `build_diff_cmd` now lives in esm_gateway.py (re-exported above via
# `from esm_gateway import build_diff_cmd` so existing call sites/tests
# keep working as `mpn.build_diff_cmd`) -- this stage's own transport is
# `esm_gateway.EsmGateway.diff` (see its docstring for why it stays a
# subprocess against `--local`, never the warm daemon's `/op Diff` route).


def run_esm_diff(
    esm_bin: Path,
    esm_a: Path,
    esm_b: Path,
    *,
    strings_dir_a: Path | None,
    strings_dir_b: Path | None,
    lang: str,
    json_out: Path,
    record_type: str | None,
    bodies: str,
    keep_noise: bool,
    exclude_type: str,
    verbose: bool,
    startup_ba2: Path | None = None,
    curves_dir: Path | None = None,
) -> dict:
    """CLI-output wrapper (banner/progress/exit-code translation) around
    `EsmGateway.diff`, which does the actual subprocess/JSON-parsing work."""
    banner("Step 2: Running esm diff")
    eprint(f"  A:           {esm_a}")
    eprint(f"  B:           {esm_b}")
    if strings_dir_a == strings_dir_b:
        eprint(f"  strings-dir: {strings_dir_a}")
    else:
        eprint(f"  strings-dir-a: {strings_dir_a}")
        eprint(f"  strings-dir-b: {strings_dir_b}")
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
            strings_dir_a=strings_dir_a, strings_dir_b=strings_dir_b,
            lang=lang, record_type=record_type, bodies=bodies,
            keep_noise=keep_noise, exclude_type=exclude_type,
            startup_ba2=startup_ba2, curves_dir=curves_dir,
        )
    except eg.DaemonError as exc:
        die(2,
            f"esm diff failed.\n{exc}\n"
            "Check the error above. Common causes:\n"
            "  - Missing / wrong --strings-dir\n"
            "  - ESM not found or unreadable\n"
            "  - Binary needs rebuild: cargo build --release --features server")
    t_elapsed = time.time() - t_start

    if verbose:
        eprint(f"\n  Command: {' '.join(result.cmd)}")
        if result.stderr:
            eprint(result.stderr)

    # Write JSON to disk -- result.raw_json is the exact text esm produced
    # (sans the trailing --local REPL prompt), so this matches byte-for-byte.
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
        description="ESM diff -> comprehensive.json/.md -> bundles.json -> lints.json "
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
                    help="Shared strings directory for both ESMs (auto-detected if omitted)")
    ap.add_argument("--strings-dir-a", default=None, metavar="DIR",
                    help="Strings directory for ESM A only (overrides --strings-dir for A)")
    ap.add_argument("--strings-dir-b", default=None, metavar="DIR",
                    help="Strings directory for ESM B only (overrides --strings-dir for B)")
    ap.add_argument("--startup-ba2", default=os.environ.get("STARTUP_BA2"), metavar="PATH",
                    help='Path to a Startup BA2 for curve-table inlining '
                         '(also $STARTUP_BA2). Mutually exclusive with --curves-dir.')
    ap.add_argument("--curves-dir", default=os.environ.get("CURVES_DIR"), metavar="DIR",
                    help='misc/ directory extracted from Startup BA2 for curve-table inlining '
                         '(also $CURVES_DIR). Auto-detected from <new_esm>/misc/ if omitted. '
                         'Mutually exclusive with --startup-ba2.')
    ap.add_argument("--lang", default="en", metavar="LANG",
                    help="Localization language code (default: en)")
    ap.add_argument("--out-dir", default=None, metavar="DIR",
                    help="Output directory (default: patch_<OLDTOK>_to_<NEWTOK>/ next to NEW.esm)")
    ap.add_argument("--esm-bin", default=None, metavar="PATH",
                    help="Path to the esm binary (default: target/release/esm or $PATH)")
    ap.add_argument("--type", default=None, dest="record_type", metavar="TYPE",
                    help="Only include records of this type (passed to esm diff)")
    ap.add_argument("--bodies", default="full", choices=["none", "stub", "full"], metavar="LEVEL",
                    help="Detail level for decoded fields on added/removed record stubs "
                         "(default: full)")
    ap.add_argument("--keep-noise", action="store_true",
                    help="Keep noisy fields (placement transforms, CELL precombine "
                         "bookkeeping, Object Bounds) instead of suppressing them")
    ap.add_argument("--exclude-type", default=DEFAULT_EXCLUDE_TYPE, metavar="LIST",
                    help=f"Comma-delimited record-type signatures to omit entirely "
                         f"(default: {DEFAULT_EXCLUDE_TYPE}). Pass --exclude-type '' to disable.")
    ap.add_argument("--categories", default=str(DEFAULT_CATEGORIES_PATH), metavar="FILE",
                    help="Path to patch_notes_categories.json (default: the copy next to "
                         "this script)")
    ap.add_argument("--refs-depth", type=int, default=None, metavar="N",
                    help="Override the categorization config's base reverse-ref BFS depth")
    ap.add_argument("--skip-bundles", action="store_true",
                    help="Skip bundles.json (and, necessarily, lints.json)")
    ap.add_argument("--skip-lints", action="store_true",
                    help="Skip lints.json (bundles.json is still built)")
    ap.add_argument("--offline", action="store_true",
                    help="Use esm_gateway.FakeGateway instead of a live warm daemon for the "
                         "bundles/lints stages (requires --refs-fixture)")
    ap.add_argument("--refs-fixture", default=None, metavar="F",
                    help="FakeGateway fixture JSON (required with --offline)")
    ap.add_argument("-v", "--verbose", action="store_true",
                    help="Show full commands + esm output")
    return ap


def main(argv=None):
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
    except eg.DaemonError as exc:
        die(1, str(exc))
    eprint(f"  esm binary: {esm_bin}")

    if args.offline and not args.refs_fixture:
        die(1, "--offline requires --refs-fixture")
    if args.refs_fixture and not Path(args.refs_fixture).is_file():
        die(1, f"--refs-fixture not found: {args.refs_fixture}")

    strings_dir_a, strings_dir_b = locate_strings_dirs(
        esm_a, esm_b,
        explicit=args.strings_dir,
        explicit_a=args.strings_dir_a,
        explicit_b=args.strings_dir_b,
        lang=args.lang,
    )
    if strings_dir_a == strings_dir_b:
        eprint(f"  strings-dir:   {strings_dir_a}  ✓")
    else:
        eprint(f"  strings-dir-a: {strings_dir_a}  ✓")
        eprint(f"  strings-dir-b: {strings_dir_b}  ✓")

    if args.startup_ba2 and args.curves_dir:
        die(1, "--startup-ba2 and --curves-dir are mutually exclusive")

    startup_ba2 = Path(args.startup_ba2).resolve() if args.startup_ba2 else None
    if startup_ba2:
        if not startup_ba2.is_file():
            die(1, f"--startup-ba2 not found: {startup_ba2}")
        eprint(f"  startup-ba2: {startup_ba2}  ✓")

    # Resolve curves_dir: explicit flag/env > auto-detect from <new_esm>/misc/.
    curves_dir: Path | None = None
    if not startup_ba2:
        if args.curves_dir:
            curves_dir = Path(args.curves_dir).resolve()
            if not curves_dir.is_dir():
                die(1, f"--curves-dir not found: {curves_dir}")
            if not (curves_dir / "curvetables" / "json").is_dir():
                die(1, f"--curves-dir missing curvetables/json/: {curves_dir}")
            eprint(f"  curves-dir: {curves_dir}  ✓")
        else:
            for candidate in [esm_b.parent / "misc", esm_a.parent / "misc"]:
                if (candidate / "curvetables" / "json").is_dir():
                    curves_dir = candidate.resolve()
                    eprint(f"  curves-dir: {curves_dir}  (auto-detected) ✓")
                    break

    out_dir = Path(args.out_dir).resolve() if args.out_dir else default_out_dir(esm_a, esm_b)
    out_dir.mkdir(parents=True, exist_ok=True)
    eprint(f"  out dir: {out_dir}")

    old_token = esm_token(esm_a)
    new_token = esm_token(esm_b)

    exclude_type = (args.exclude_type or "").strip()

    categories_path = Path(args.categories)
    if not categories_path.is_file():
        die(1, f"--categories not found: {categories_path}")
    try:
        with categories_path.open(encoding="utf-8") as f:
            config = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        die(1, f"failed to load --categories {categories_path}: {e}")
    settings = dict(config.get("settings") or {})
    if args.refs_depth is not None:
        settings["refs_depth"] = args.refs_depth
    config = {**config, "settings": settings}

    # ---- Step 2: esm diff ---------------------------------------------------
    diff_json_path = out_dir / "diff.json"
    diff_data = run_esm_diff(
        esm_bin, esm_a, esm_b,
        strings_dir_a=strings_dir_a,
        strings_dir_b=strings_dir_b,
        lang=args.lang,
        json_out=diff_json_path,
        record_type=args.record_type,
        bodies=args.bodies,
        keep_noise=args.keep_noise,
        exclude_type=exclude_type,
        verbose=args.verbose,
        startup_ba2=startup_ba2,
        curves_dir=curves_dir,
    )
    files_written["diff"] = "diff.json"

    # ---- Step 3: comprehensive.json / comprehensive.md ----------------------
    banner("Step 3: Building comprehensive.json / comprehensive.md")
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
        md = rc.render_markdown(comp)

        comp_json_path = out_dir / "comprehensive.json"
        comp_md_path = out_dir / "comprehensive.md"
        with comp_json_path.open("w", encoding="utf-8") as f:
            json.dump(comp, f, indent=2, ensure_ascii=False)
            f.write("\n")
        with comp_md_path.open("w", encoding="utf-8") as f:
            f.write(md)
    except Exception as e:
        die(3, f"building comprehensive.json/.md failed: {e}")
    files_written["comprehensive_json"] = "comprehensive.json"
    files_written["comprehensive_md"] = "comprehensive.md"

    counts = dict(comp["meta"]["counts"])
    eprint(f"\n  ✓ Done in {time.time() - t_start:.1f}s "
           f"({counts.get('added', 0)} added, {counts.get('changed', 0)} changed, "
           f"{counts.get('removed', 0)} removed)")

    # ---- Steps 4 + 5: bundles.json / lints.json ------------------------------
    bundles_result = None
    lints_payload = None
    client = None
    try:
        if args.skip_bundles:
            eprint("\nSkipping bundles.json and lint checks (--skip-bundles)")
        else:
            banner("Step 4: Building bundles.json")
            t_start = time.time()
            try:
                if args.offline:
                    client = eg.FakeGateway(args.refs_fixture)
                else:
                    client = eg.ensure_daemon(esm_bin, esm_b)
                bundles_result = bb.build_bundles(comp, client, str(esm_a), str(esm_b), config)
                bundles_json_path = out_dir / "bundles.json"
                with bundles_json_path.open("w", encoding="utf-8") as f:
                    json.dump(bundles_result, f, indent=2, ensure_ascii=False)
                    f.write("\n")
            except Exception as e:
                die(3, f"building bundles.json failed: {e}")
            files_written["bundles"] = "bundles.json"
            bc = bundles_result["meta"]["counts"]
            eprint(f"\n  ✓ Done in {time.time() - t_start:.1f}s "
                   f"({bc['bundles']} bundles, {bc['singletons']} singletons, "
                   f"{bc['uncategorized']} uncategorized)")

            if args.skip_lints:
                eprint("\nSkipping lint checks (--skip-lints)")
            else:
                banner("Step 5: Running lint checks")
                t_start = time.time()
                try:
                    lints_payload, updated_bundles = rl.run_lints(
                        comp, bundles_result, client,
                        new_esm=str(esm_b), old_esm=str(esm_a),
                        settings=config.get("settings"),
                    )
                    with (out_dir / "lints.json").open("w", encoding="utf-8") as f:
                        json.dump(lints_payload, f, indent=2)
                        f.write("\n")
                    with (out_dir / "bundles.json").open("w", encoding="utf-8") as f:
                        json.dump(updated_bundles, f, indent=2)
                        f.write("\n")
                except Exception as e:
                    die(3, f"running lint checks failed: {e}")
                files_written["lints"] = "lints.json"
                lc = lints_payload["meta"]["counts"]
                eprint(f"\n  ✓ Done in {time.time() - t_start:.1f}s "
                       f"(error={lc['error']} warn={lc['warn']} info={lc['info']})")
    finally:
        if client is not None:
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
        pipeline_version=pl.SCHEMA_VERSION,
        counts=manifest_counts,
    )
    manifest["stages"]["mechanical"]["completed_at"] = _now_iso()
    manifest["stages"]["mechanical"]["files"] = dict(files_written)
    pl.write_manifest(out_dir, manifest)
    files_written["manifest"] = "manifest.json"
    eprint(f"  wrote {out_dir / 'manifest.json'}")

    # ---- Step 7: summary -------------------------------------------------
    t_total = time.time() - t0
    banner("Done")
    for fname in files_written.values():
        eprint(f"  ✓ {out_dir / fname}")
    eprint(f"\n  Total time: {t_total:.1f}s")
    eprint(
        "\n  Narrative stage: run the /patch-notes skill "
        "(writes notes/, discord/, then update_manifest.py)."
    )
    eprint()
    return 0


if __name__ == "__main__":
    sys.exit(main())
