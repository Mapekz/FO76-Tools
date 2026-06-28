#!/usr/bin/env python3
"""
Orchestrator: ESM diff → patch notes → Discord chunks.

Usage:
    python3 tools/make_patch_notes.py OLD.esm NEW.esm [options]

Options:
    --strings-dir DIR     Directory containing loose .strings/.dlstrings/.ilstrings files.
                          Tried in this order:
                          1. --strings-dir (if given)
                          2. <esm_dir>/strings
                          3. <esm_dir>
                          The resolved directory must contain version-matched files
                          for BOTH ESMs (sevenysix_<date>_en.*). Fails loudly if absent.
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
    python3 tools/make_patch_notes.py \\
        SeventySix_20260619.esm SeventySix_20260626.esm \\
        --strings-dir strings

    # With hand-authored highlights:
    python3 tools/make_patch_notes.py \\
        SeventySix_20260619.esm SeventySix_20260626.esm \\
        --strings-dir strings \\
        --highlights-file /tmp/highlights_20260626.md

    # Via justfile:
    just patch-notes SeventySix_20260619.esm SeventySix_20260626.esm
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


def locate_strings_dir(esm_a: Path, esm_b: Path, explicit, lang: str) -> Path:
    """
    Return the directory that contains version-matched loose string files for
    BOTH esm_a and esm_b, or die loudly.
    """
    tok_a = version_token(esm_a)
    tok_b = version_token(esm_b)

    def has_strings(d: Path, tok: str) -> bool:
        """True if d has at least one of *.strings, *.dlstrings, *.ilstrings for tok."""
        if not d.is_dir():
            return False
        for ext in ("strings", "dlstrings", "ilstrings"):
            candidates = list(d.glob(f"*{tok}*_{lang}.{ext}"))
            if candidates:
                return True
        return False

    # Candidate directories in priority order
    candidates = []
    if explicit:
        candidates.append(Path(explicit))
    for esm in (esm_a, esm_b):
        esm_dir = esm.resolve().parent
        candidates.append(esm_dir / "strings")
        candidates.append(esm_dir)

    seen = set()
    for d in candidates:
        if d in seen:
            continue
        seen.add(d)
        if has_strings(d, tok_a) and has_strings(d, tok_b):
            return d.resolve()

    # Build a helpful error
    tried = "\n  ".join(str(d) for d in list(seen)[:6])
    die(1,
        f"No version-matched string files found for both ESMs.\n"
        f"Expected files matching *{tok_a}*_{lang}.{{strings,dlstrings,ilstrings}}\n"
        f"and *{tok_b}*_{lang}.{{strings,dlstrings,ilstrings}}\n"
        f"Searched:\n  {tried}\n\n"
        f"Supply --strings-dir pointing to the directory with both sets of loose files.\n"
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
    strings_dir: Path,
    lang: str,
    json_out: Path,
    record_type: str | None,
    verbose: bool,
) -> dict:
    cmd = [
        str(esm_bin),
        "--local",
        "diff",
        str(esm_a),
        str(esm_b),
        "--strings-dir", str(strings_dir),
        "--lang", lang,
        "--json",
        "--pretty",
    ]
    if record_type:
        cmd += ["--type", record_type]

    banner("Step 1: Running esm diff")
    eprint(f"  A:           {esm_a}")
    eprint(f"  B:           {esm_b}")
    eprint(f"  strings-dir: {strings_dir}")
    eprint(f"  json output: {json_out}")
    if record_type:
        eprint(f"  --type filter: {record_type}")
    if verbose:
        eprint(f"\n  Command: {' '.join(cmd)}")

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
                    help="Directory with loose .strings/.dlstrings/.ilstrings for both ESMs")
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

    strings_dir = locate_strings_dir(esm_a, esm_b, args.strings_dir, args.lang)
    eprint(f"  strings-dir: {strings_dir}  ✓ (both versions found)")

    json_path = Path(args.keep_json) if args.keep_json else Path("/tmp/fo76_diff.json")

    date = derive_patch_date(esm_b)
    md_out = output_md_path(esm_b, out_dir=None)  # always next to NEW.esm
    chunks_dir = output_chunks_dir(esm_b, args.out_dir)

    eprint(f"  patch date: {date}")
    eprint(f"  md output:  {md_out}")
    eprint(f"  chunks dir: {chunks_dir}/")

    # ---- Step 1: esm diff -------------------------------------------------
    t_diff_start = time.time()
    diff_data = run_esm_diff(
        esm_bin, esm_a, esm_b,
        strings_dir=strings_dir,
        lang=args.lang,
        json_out=json_path,
        record_type=args.record_type,
        verbose=args.verbose,
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
    eprint(f"  python3 tools/make_patch_notes.py {esm_a.name} {esm_b.name} \\")
    eprint(f"    --strings-dir {strings_dir}")
    eprint()


if __name__ == "__main__":
    main()
