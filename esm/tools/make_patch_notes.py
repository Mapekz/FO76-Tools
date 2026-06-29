#!/usr/bin/env python3
"""
Orchestrator: ESM diff → patch notes → Discord chunks.

Usage:
    python3 tools/make_patch_notes.py OLD.esm NEW.esm [options]

Options:
    --strings-dir DIR     Shared strings directory for both ESMs. Auto-detected from
                          <esm_parent>/strings/ (or <esm_parent> itself) if omitted.
    --strings-dir-a DIR   Strings directory for ESM A only (overrides --strings-dir for A).
    --strings-dir-b DIR   Strings directory for ESM B only (overrides --strings-dir for B).
    --startup-ba2 PATH    Path to "SeventySix - Startup.ba2" (or env STARTUP_BA2).
                          Enables curve-table inlining and crafting-quantity evaluation.
                          Mutually exclusive with --curves-dir.
    --curves-dir DIR      Path to the misc/ directory extracted from a Startup BA2
                          (or env CURVES_DIR). Loose alternative to --startup-ba2;
                          curve JSON is read from <dir>/curvetables/json/.
                          Auto-detected from <new_esm_parent>/misc/ if omitted.
                          Mutually exclusive with --startup-ba2.
    --lang LANG           Localization language code (default: en)
    --out-dir DIR         Output directory. Default: discord_chunks_<date> next to NEW.esm.
    --highlights-file F   Inject this markdown file verbatim as the Highlights section
                          (skip auto-highlights). Useful for hand-authored / LLM analysis.
    --esm-bin PATH        Path to the esm binary (default: target/release/esm relative to
                          the esm/ workspace root, or whatever is on $PATH as 'esm').
    --keep-json PATH      Write the raw diff JSON to this path (default: /tmp/fo76_diff.json).
    --type TYPE           Only include records of this type in the report (passed to esm diff).
    -v, --verbose         Show full diff command + esm output.

Exit codes:
    0  Success
    1  Input validation error (missing files, missing strings, etc.)
    2  esm diff failed (non-zero exit)
    3  Markdown generation failed
    4  Discord chunking failed

Examples:
    # Each ESM in its own directory; strings auto-detected from <dir>/strings/.
    python3 tools/make_patch_notes.py \\
        /path/to/v1/SeventySix.esm /path/to/v2/SeventySix.esm

    # Shared strings directory (both ESMs in the same folder, or explicit path).
    python3 tools/make_patch_notes.py \\
        old/SeventySix.esm new/SeventySix.esm \\
        --strings-dir /path/to/strings

    # With Startup BA2 for curve-table detail:
    STARTUP_BA2="/path/to/SeventySix - Startup.ba2" \\
    python3 tools/make_patch_notes.py \\
        /path/to/v1/SeventySix.esm /path/to/v2/SeventySix.esm

    # Via justfile:
    just patch-notes /path/to/v1/SeventySix.esm /path/to/v2/SeventySix.esm
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import NoReturn

# Locate the esm/ workspace root (directory containing this script's parent).
SCRIPT_DIR = Path(__file__).resolve().parent
WORKSPACE_ROOT = SCRIPT_DIR.parent  # esm/

# --------------------------------------------------------------------------
# Helpers
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


def version_token(esm_path: Path) -> str:
    """Extract the 8-digit date token from an ESM stem (e.g. '20260619')."""
    stem = esm_path.stem
    m = re.search(r"\d{6,}", stem)
    return m.group(0) if m else stem


def find_esm_binary(explicit) -> Path:
    """Locate the esm binary, preferring the release build in the workspace."""
    if explicit:
        p = Path(explicit)
        if p.is_file() and os.access(p, os.X_OK):
            return p
        die(1, f"--esm-bin path not executable: {explicit}")

    # Try workspace release build first
    release = WORKSPACE_ROOT / "target" / "release" / "esm"
    if release.is_file() and os.access(release, os.X_OK):
        return release

    # Fallback: $PATH
    found = shutil.which("esm")
    if found:
        return Path(found)

    die(1,
        "Cannot find esm binary. Build it first:\n"
        "  cargo build --release\n"
        "Or pass --esm-bin /path/to/esm")


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
                # tok has no date digits (e.g. "SeventySix") — also accept plain name
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


def derive_patch_date(esm_path: Path) -> str:
    tok = version_token(esm_path)
    if len(tok) == 8:
        return f"{tok[:4]}-{tok[4:6]}-{tok[6:]}"
    return tok


def output_md_path(esm_new: Path, out_dir: Path | None) -> Path:
    date = derive_patch_date(esm_new)
    fname = f"patch_notes_{date}.md"
    if out_dir:
        return out_dir / fname
    return esm_new.resolve().parent / fname


def output_chunks_dir(esm_new: Path, explicit: str | None) -> Path:
    date = derive_patch_date(esm_new)
    if explicit:
        return Path(explicit)
    return esm_new.resolve().parent / f"discord_chunks_{date}"


# --------------------------------------------------------------------------
# Step 1: Run esm diff
# --------------------------------------------------------------------------

def run_esm_diff(
    esm_bin: Path,
    esm_a: Path,
    esm_b: Path,
    strings_dir_a: Path | None,
    strings_dir_b: Path | None,
    lang: str,
    json_out: Path,
    record_type: str | None,
    verbose: bool,
    startup_ba2: Path | None = None,
    curves_dir: Path | None = None,
) -> dict:
    cmd = [
        str(esm_bin),
        "--local",
        "diff",
        str(esm_a),
        str(esm_b),
        "--lang", lang,
        "--json",
        "--pretty",
    ]
    # Pass string dirs: shared if identical, per-side if different.
    if strings_dir_a and strings_dir_b:
        if strings_dir_a == strings_dir_b:
            cmd += ["--strings-dir", str(strings_dir_a)]
        else:
            cmd += ["--strings-dir-a", str(strings_dir_a),
                    "--strings-dir-b", str(strings_dir_b)]
    elif strings_dir_a:
        cmd += ["--strings-dir-a", str(strings_dir_a)]
    elif strings_dir_b:
        cmd += ["--strings-dir-b", str(strings_dir_b)]
    if record_type:
        cmd += ["--type", record_type]
    if startup_ba2:
        cmd += ["--startup-ba2", str(startup_ba2)]
    elif curves_dir:
        cmd += ["--curves-dir", str(curves_dir)]

    banner("Step 1: Running esm diff")
    eprint(f"  A:           {esm_a}")
    eprint(f"  B:           {esm_b}")
    if strings_dir_a == strings_dir_b:
        eprint(f"  strings-dir: {strings_dir_a}")
    else:
        eprint(f"  strings-dir-a: {strings_dir_a}")
        eprint(f"  strings-dir-b: {strings_dir_b}")
    eprint(f"  json output: {json_out}")
    if record_type:
        eprint(f"  --type filter: {record_type}")
    if verbose:
        eprint(f"\n  Command: {' '.join(str(c) for c in cmd)}")

    t_start = time.time()
    # Always capture stdout (JSON output); in verbose mode also echo stderr live.
    # stdin=DEVNULL: esm --local <cmd> enters a REPL after the subcommand — feeding
    # it EOF immediately avoids blocking and keeps stdout clean (just the JSON + "esm> ").
    result = subprocess.run(cmd, capture_output=True, text=True,
                            stdin=subprocess.DEVNULL)
    t_elapsed = time.time() - t_start

    if verbose and result.stderr:
        eprint(result.stderr)

    if result.returncode != 0:
        if result.stderr and not verbose:
            eprint(result.stderr)
        die(2,
            f"esm diff failed with exit code {result.returncode}.\n"
            "Check the error above. Common causes:\n"
            "  - Missing / wrong --strings-dir\n"
            "  - ESM not found or unreadable\n"
            "  - Binary needs rebuild: cargo build --release")

    raw_output = result.stdout

    # Parse using raw_decode so trailing REPL prompt ("esm> ") is ignored.
    # esm --local <cmd> enters the interactive REPL after the subcommand; its
    # "esm> " prompt goes to stdout, appearing after the JSON object.
    try:
        data, json_end = json.JSONDecoder().raw_decode(raw_output)
        raw_json = raw_output[:json_end]
    except json.JSONDecodeError as e:
        die(2, f"esm diff produced invalid JSON: {e}\nFirst 500 chars: {raw_output[:500]}")

    # Write JSON to disk
    json_out.parent.mkdir(parents=True, exist_ok=True)
    with open(json_out, "w") as f:
        f.write(raw_json)

    eprint(f"\n  ✓ Done in {t_elapsed:.1f}s")
    eprint(f"    added={len(data.get('added',[]))}, "
           f"removed={len(data.get('removed',[]))}, "
           f"changed={len(data.get('changed',[]))}, "
           f"ref_names={len(data.get('ref_names',{}))}")
    return data


# --------------------------------------------------------------------------
# Step 2: Generate Markdown
# --------------------------------------------------------------------------

def run_gen_patch_notes(
    diff_data: dict,
    esm_a: Path,
    esm_b: Path,
    md_out: Path,
    highlights_file: str | None,
    timing: dict | None,
) -> None:
    # Import from sibling tool (same directory as this script)
    sys.path.insert(0, str(SCRIPT_DIR))
    from gen_patch_notes import generate_markdown, derive_labels_from_filenames

    banner("Step 2: Generating Markdown")

    old_label = esm_a.name
    new_label = esm_b.name
    _, _, patch_date = derive_labels_from_filenames(old_label, new_label)
    eprint(f"  old_label:  {old_label}")
    eprint(f"  new_label:  {new_label}")
    eprint(f"  patch_date: {patch_date}")
    eprint(f"  output:     {md_out}")

    highlights_text = None
    if highlights_file:
        with open(highlights_file) as f:
            highlights_text = f.read()
        eprint(f"  highlights: {highlights_file} ({len(highlights_text)} chars, injected verbatim)")
    else:
        eprint("  highlights: auto-generated from diff data")

    t_start = time.time()
    md = generate_markdown(
        diff_data,
        old_label=old_label,
        new_label=new_label,
        patch_date=patch_date,
        timing=timing,
        highlights_text=highlights_text,
    )
    t_elapsed = time.time() - t_start

    if timing:
        timing["interpret"] = t_elapsed
        total = sum(timing.values())
        md = md.replace("{INTERPRET_TIME}", f"{t_elapsed:.2f}s")
        md = md.replace("{TOTAL_TIME}", f"{total:.2f}s")

    md_out.parent.mkdir(parents=True, exist_ok=True)
    with open(md_out, "w") as f:
        f.write(md)

    eprint(f"\n  ✓ Wrote {len(md):,} chars in {t_elapsed:.2f}s → {md_out}")


# --------------------------------------------------------------------------
# Step 3: Discord chunking
# --------------------------------------------------------------------------

def run_discord_chunker(md_path: Path, chunks_dir: Path) -> int:
    """Run the discord_chunker, return number of chunks written."""
    chunker = SCRIPT_DIR / "discord_chunker.py"
    if not chunker.is_file():
        die(4, f"discord_chunker.py not found at {chunker}")

    banner("Step 3: Splitting for Discord")
    eprint(f"  input:  {md_path}")
    eprint(f"  output: {chunks_dir}/")

    chunks_dir.mkdir(parents=True, exist_ok=True)
    cmd = [sys.executable, str(chunker), str(md_path), str(chunks_dir)]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        if result.stderr:
            eprint(result.stderr)
        die(4, f"discord_chunker.py failed with exit code {result.returncode}")

    chunk_files = sorted(chunks_dir.glob("chunk_*.md"))
    eprint(f"\n  ✓ {len(chunk_files)} chunk(s) written to {chunks_dir}/")
    if result.stdout.strip():
        eprint(f"  {result.stdout.strip()}")
    return len(chunk_files)


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(
        description="ESM diff → patch notes → Discord chunks. One command, no LLM.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    ap.add_argument("old_esm", type=Path, help="Path to the old ESM file")
    ap.add_argument("new_esm", type=Path, help="Path to the new ESM file")
    ap.add_argument("--strings-dir", default=None, metavar="DIR",
                    help="Shared strings directory for both ESMs (auto-detected if omitted)")
    ap.add_argument("--strings-dir-a", default=None, metavar="DIR",
                    help="Strings directory for ESM A only (overrides --strings-dir for A)")
    ap.add_argument("--strings-dir-b", default=None, metavar="DIR",
                    help="Strings directory for ESM B only (overrides --strings-dir for B)")
    ap.add_argument("--startup-ba2", default=os.environ.get("STARTUP_BA2"), metavar="PATH",
                    help='Path to "SeventySix - Startup.ba2" for curve-table inlining '
                         '(also $STARTUP_BA2). Mutually exclusive with --curves-dir.')
    ap.add_argument("--curves-dir", default=os.environ.get("CURVES_DIR"), metavar="DIR",
                    help='misc/ directory extracted from Startup BA2 for curve-table inlining '
                         '(also $CURVES_DIR). Auto-detected from <new_esm>/misc/ if omitted. '
                         'Mutually exclusive with --startup-ba2.')
    ap.add_argument("--lang", default="en", metavar="LANG",
                    help="Localization language code (default: en)")
    ap.add_argument("--out-dir", default=None, metavar="DIR",
                    help="Output directory for Discord chunks (default: discord_chunks_<date>/ "
                         "next to NEW.esm)")
    ap.add_argument("--highlights-file", default=None, metavar="FILE",
                    help="Inject this markdown verbatim as the Highlights section")
    ap.add_argument("--esm-bin", default=None, metavar="PATH",
                    help="Path to the esm binary (default: target/release/esm or $PATH)")
    ap.add_argument("--keep-json", default=None, metavar="PATH",
                    help="Write diff JSON to this path (default: /tmp/fo76_diff.json)")
    ap.add_argument("--type", default=None, dest="record_type", metavar="TYPE",
                    help="Only include records of this type (passed to esm diff)")
    ap.add_argument("-v", "--verbose", action="store_true",
                    help="Show full commands + esm output")
    args = ap.parse_args()

    t0 = time.time()

    # ---- Validate inputs --------------------------------------------------
    banner("Validating inputs")

    esm_a = args.old_esm.resolve()
    esm_b = args.new_esm.resolve()

    if not esm_a.is_file():
        die(1, f"Old ESM not found: {esm_a}")
    if not esm_b.is_file():
        die(1, f"New ESM not found: {esm_b}")

    eprint(f"  OLD: {esm_a}  ({esm_a.stat().st_size:,} bytes)")
    eprint(f"  NEW: {esm_b}  ({esm_b.stat().st_size:,} bytes)")

    if args.highlights_file and not Path(args.highlights_file).is_file():
        die(1, f"--highlights-file not found: {args.highlights_file}")

    esm_bin = find_esm_binary(args.esm_bin)
    eprint(f"  esm binary: {esm_bin}")

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

    json_path = Path(args.keep_json) if args.keep_json else Path("/tmp/fo76_diff.json")

    date = derive_patch_date(esm_b)
    md_out = output_md_path(esm_b, out_dir=None)  # always next to NEW.esm
    chunks_dir = output_chunks_dir(esm_b, args.out_dir)

    eprint(f"  patch date: {date}")
    eprint(f"  md output:  {md_out}")
    eprint(f"  chunks dir: {chunks_dir}/")

    # ---- Step 1: esm diff -------------------------------------------------
    t_diff_start = time.time()

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

    diff_data = run_esm_diff(
        esm_bin, esm_a, esm_b,
        strings_dir_a=strings_dir_a,
        strings_dir_b=strings_dir_b,
        lang=args.lang,
        json_out=json_path,
        record_type=args.record_type,
        verbose=args.verbose,
        startup_ba2=startup_ba2,
        curves_dir=curves_dir,
    )
    t_diff = time.time() - t_diff_start

    # Build timing dict for embedding in the report.
    # (We don't have per-side open times from the CLI — use total diff time.)
    timing = {
        "open_a": t_diff * 0.35,   # rough proportion for display
        "open_b": t_diff * 0.35,
        "diff":   t_diff * 0.30,
    }

    # ---- Step 2: Markdown -------------------------------------------------
    try:
        run_gen_patch_notes(
            diff_data, esm_a, esm_b,
            md_out=md_out,
            highlights_file=args.highlights_file,
            timing=timing,
        )
    except Exception as e:
        die(3, f"Markdown generation failed: {e}")

    # ---- Step 3: Discord chunks -------------------------------------------
    n_chunks = 0
    try:
        n_chunks = run_discord_chunker(md_out, chunks_dir)
    except Exception as e:
        die(4, f"Discord chunker failed: {e}")

    # ---- Summary ----------------------------------------------------------
    t_total = time.time() - t0
    banner("Done")
    eprint(f"\n  ✓ Diff JSON:      {json_path}")
    eprint(f"  ✓ Patch notes MD: {md_out}")
    eprint(f"  ✓ Discord chunks: {chunks_dir}/ ({n_chunks} files)")
    eprint(f"\n  Total time: {t_total:.1f}s")
    eprint(f"\nTo reproduce:")
    eprint(f"  python3 tools/make_patch_notes.py {esm_a} {esm_b} \\")
    if strings_dir_a == strings_dir_b:
        eprint(f"    --strings-dir {strings_dir_a}")
    else:
        eprint(f"    --strings-dir-a {strings_dir_a} \\")
        eprint(f"    --strings-dir-b {strings_dir_b}")
    eprint()


if __name__ == "__main__":
    main()
