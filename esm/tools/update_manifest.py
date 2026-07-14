#!/usr/bin/env python3
"""
update_manifest.py — narrative-stage manifest updater for the FO76 patch-notes
pipeline (tiered edition).

The mechanical stage (`make_patch_notes.py` + `triage_bundles.py`) writes
`manifest.json` with an empty `stages.narrative` section. The narrative
stage (the `/patch-notes` Claude skill — see
`../.claude/skills/patch-notes/SKILL.md`) assembles ONE `patch-summary.md` and
chunks it into `discord/chunk_*.md`. This script is the last step of that
skill: it records those two outputs, plus the final `work/triage.json` tier
counts, into `stages.narrative`, leaving everything else in the manifest
untouched.

This is schema_version 2 of `stages.narrative` (the pipeline's older
per-category shape -- `categories: [{id, label, notes_md, discord_dir,
chunk_count, chunks}, ...]`, one `notes/<slug>.md` + `discord/<slug>/` per
category -- is retired along with the category-slicing narrative flow; see
`triage_bundles.py` and `deep-writer-prompt.md`). This version instead
records a single `patch_summary_md` path, a flat `discord/` chunk list, and
the triage tier counts, keyed under `stages.narrative.schema_version: 2` so
any downstream consumer can tell the two shapes apart.

Usage:
    python3 tools/update_manifest.py OUT_DIR [--max-chunk-chars 2000]

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import patchnotes_lib as pl  # noqa: E402

#: stages.narrative's own schema version (independent of the pipeline-wide
#: pl.SCHEMA_VERSION, which covers diff/comprehensive/bundles/lints shapes
#: this script doesn't touch).
NARRATIVE_SCHEMA_VERSION = 2

PATCH_SUMMARY_FILENAME = "patch-summary.md"
DISCORD_DIRNAME = "discord"


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def _now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def discover_patch_summary(out_dir: Path) -> str | None:
    """Relative path to `<out_dir>/patch-summary.md`, or None if it doesn't
    exist yet (never raises)."""
    path = out_dir / PATCH_SUMMARY_FILENAME
    return PATCH_SUMMARY_FILENAME if path.is_file() else None


def discover_discord_chunks(out_dir: Path) -> list[str]:
    """Sorted `discord/chunk_*.md` paths, relative to out_dir -- a single
    flat directory now (one merged patch-summary.md, not one per category),
    filenames are zero-padded (chunk_001.md, ...) so a plain name sort is
    already numeric order."""
    chunk_dir = out_dir / DISCORD_DIRNAME
    if not chunk_dir.is_dir():
        return []
    paths = sorted(p for p in chunk_dir.glob("chunk_*.md") if p.is_file())
    return [f"{DISCORD_DIRNAME}/{p.name}" for p in paths]


def load_triage_stats(out_dir: Path) -> dict | None:
    """`{"deep","brief","drop","ambiguous","total_bundles",
    "resolved_by_assessor"}` sourced from `<out_dir>/work/triage.json`
    (triage_bundles.py's output), or None if that file doesn't exist / is
    unreadable / malformed (never raises)."""
    path = out_dir / "work" / "triage.json"
    if not path.is_file():
        return None
    try:
        with path.open(encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict):
        return None

    stats = data.get("stats") or {}
    deep = stats.get("deep", len(data.get("deep") or []))
    brief = stats.get("brief", len(data.get("brief") or []))
    drop = stats.get("drop", len(data.get("drop") or []))
    ambiguous = stats.get("ambiguous", len(data.get("ambiguous") or []))
    return {
        "deep": deep,
        "brief": brief,
        "drop": drop,
        "ambiguous": ambiguous,
        "total_bundles": stats.get("total_bundles", deep + brief + drop + ambiguous),
        "resolved_by_assessor": stats.get("resolved_by_assessor", 0),
    }


def build_narrative_stage(out_dir: Path, max_chunk_chars: int) -> dict:
    """Build the full `stages.narrative` payload (schema_version 2)."""
    chunks = discover_discord_chunks(out_dir)
    return {
        "schema_version": NARRATIVE_SCHEMA_VERSION,
        "completed_at": _now_iso(),
        "patch_summary_md": discover_patch_summary(out_dir),
        "discord_dir": DISCORD_DIRNAME,
        "chunk_count": len(chunks),
        "chunks": chunks,
        "max_chunk_chars": max_chunk_chars,
        "triage": load_triage_stats(out_dir),
    }


def print_summary(narrative: dict, stream=sys.stderr):
    print(f"patch_summary_md: {narrative.get('patch_summary_md') or '(missing)'}", file=stream)
    print(f"discord chunks:   {narrative.get('chunk_count', 0)}", file=stream)
    triage = narrative.get("triage")
    if triage:
        print(
            f"triage tiers:     deep={triage.get('deep', 0)} brief={triage.get('brief', 0)} "
            f"drop={triage.get('drop', 0)} ambiguous={triage.get('ambiguous', 0)}",
            file=stream,
        )
        if triage.get("resolved_by_assessor"):
            print(f"resolved by assessor: {triage['resolved_by_assessor']}", file=stream)
    else:
        print("triage tiers:     (no work/triage.json found)", file=stream)


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="update_manifest.py",
        description="Fill in manifest.json's narrative stage from patch-summary.md + discord/ + work/triage.json.",
    )
    ap.add_argument("out_dir", help="Pipeline output directory (must already contain manifest.json).")
    ap.add_argument(
        "--max-chunk-chars", type=int, default=2000,
        help="Recorded verbatim in stages.narrative.max_chunk_chars (default: 2000).",
    )
    return ap


def main(argv=None):
    args = build_arg_parser().parse_args(argv)
    out_dir = Path(args.out_dir)

    manifest = pl.load_manifest(out_dir)
    if manifest is None:
        eprint(f"error: no manifest.json found in {out_dir}")
        return 1

    narrative = build_narrative_stage(out_dir, args.max_chunk_chars)

    manifest.setdefault("stages", {})
    manifest["stages"]["narrative"] = narrative

    pl.write_manifest(out_dir, manifest)

    print_summary(narrative)
    if narrative["patch_summary_md"] is None:
        eprint(f"warning: no {PATCH_SUMMARY_FILENAME} found in {out_dir}")
    if narrative["chunk_count"] == 0:
        eprint(f"warning: zero Discord chunks found under {out_dir / DISCORD_DIRNAME}")
    eprint(f"\nwrote {out_dir / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
