#!/usr/bin/env python3
"""
update_manifest.py — narrative-stage manifest updater for the FO76 patch-notes
pipeline.

The mechanical stage (`make_patch_notes.py`) writes `manifest.json` with an
empty `stages.narrative` section. The narrative stage (the `/patch-notes`
Claude skill — see `.claude/skills/patch-notes/SKILL.md`) writes one
`notes/<slug>.md` per category and, for each, a `discord/<slug>/chunk_*.md`
sequence. This script is the last step of that skill: it scans those two
directories and fills in `stages.narrative`, leaving everything else in the
manifest untouched.

Usage:
    python3 tools/update_manifest.py OUT_DIR [--max-chunk-chars 2000]

Category labels are resolved from `<OUT_DIR>/work/categories.json` (written
by `slice_bundles.py`, whose `categories[].{id,label,slug,post_order}` is the
authoritative id/label/order mapping) when that file is present; otherwise
the slug itself is titleized into a label and no `post_order` is available
(such categories sort after every known one, by slug).

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import patchnotes_lib as pl  # noqa: E402


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def _now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


_SLUG_WORD_RE = re.compile(r"[A-Za-z0-9]+")


def titleize_slug(slug: str) -> str:
    """Fallback label for a slug with no `work/categories.json` entry: split
    on non-alphanumeric runs and title-case each word (e.g. `camp_workshop`
    -> `Camp Workshop`, `ui-misc` -> `Ui Misc`)."""
    words = _SLUG_WORD_RE.findall(slug)
    return " ".join(w.capitalize() for w in words) if words else slug


def load_category_labels(out_dir: Path) -> dict:
    """`{slug: {"id", "label", "post_order"}}` sourced from
    `<out_dir>/work/categories.json` (slice_bundles.py's output) when
    present, else `{}`. Missing/unreadable/malformed file -> `{}` (never
    raises)."""
    path = out_dir / "work" / "categories.json"
    if not path.is_file():
        return {}
    try:
        with path.open(encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        return {}
    if not isinstance(data, dict):
        return {}

    out = {}
    for cat in data.get("categories") or []:
        if not isinstance(cat, dict):
            continue
        slug = cat.get("slug")
        if not slug:
            continue
        out[slug] = {
            "id": cat.get("id", slug),
            "label": cat.get("label") or slug,
            "post_order": cat.get("post_order"),
        }
    return out


def discover_notes(out_dir: Path) -> list[str]:
    """Sorted list of slugs with a `notes/<slug>.md` file."""
    notes_dir = out_dir / "notes"
    if not notes_dir.is_dir():
        return []
    return sorted(p.stem for p in notes_dir.glob("*.md") if p.is_file())


def discover_chunks(out_dir: Path, slug: str) -> list[str]:
    """`discord/<slug>/chunk_*.md` paths, relative to `out_dir`, in filename
    order (chunk files are zero-padded, e.g. `chunk_001.md`, so a plain name
    sort is already numeric order)."""
    chunk_dir = out_dir / "discord" / slug
    if not chunk_dir.is_dir():
        return []
    paths = sorted(p for p in chunk_dir.glob("chunk_*.md") if p.is_file())
    return [f"discord/{slug}/{p.name}" for p in paths]


def build_narrative_categories(out_dir: Path):
    """Return `(categories, warnings)` — `categories` is the fully-built
    `stages.narrative.categories` list (sorted by `post_order`, falling back
    to the slug for categories `work/categories.json` doesn't know about);
    `warnings` is a list of human-readable strings for categories that have
    notes but zero Discord chunks."""
    labels = load_category_labels(out_dir)
    slugs = discover_notes(out_dir)

    entries = []
    for slug in slugs:
        meta = labels.get(slug, {})
        chunks = discover_chunks(out_dir, slug)
        post_order = meta.get("post_order")
        entries.append({
            "id": meta.get("id", slug),
            "label": meta.get("label") or titleize_slug(slug),
            "notes_md": f"notes/{slug}.md",
            "discord_dir": f"discord/{slug}",
            "chunk_count": len(chunks),
            "chunks": chunks,
            "_slug": slug,
            "_sort_key": (post_order if post_order is not None else float("inf"), slug),
        })

    entries.sort(key=lambda e: e["_sort_key"])

    warnings = []
    for e in entries:
        if e["chunk_count"] == 0:
            warnings.append(
                f"warning: category '{e['_slug']}' has a notes file but zero Discord "
                "chunks (chunk_count=0)"
            )
        del e["_sort_key"]
        del e["_slug"]

    return entries, warnings


def print_summary(categories, stream=sys.stderr):
    header = f"{'category':<28}{'label':<28}{'chunks':>8}"
    print(header, file=stream)
    print("-" * len(header), file=stream)
    for cat in categories:
        print(f"{cat['id']:<28}{cat['label']:<28}{cat['chunk_count']:>8}", file=stream)


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="update_manifest.py",
        description="Fill in manifest.json's narrative stage from notes/ + discord/ output.",
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

    categories, warnings = build_narrative_categories(out_dir)

    manifest.setdefault("stages", {})
    manifest["stages"]["narrative"] = {
        "completed_at": _now_iso(),
        "max_chunk_chars": args.max_chunk_chars,
        "categories": categories,
    }

    pl.write_manifest(out_dir, manifest)

    print_summary(categories)
    for w in warnings:
        eprint(w)
    eprint(f"\nwrote {out_dir / 'manifest.json'} ({len(categories)} categories)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
