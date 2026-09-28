#!/usr/bin/env python3
"""
fetch_official_notes.py — deterministic fetch of an official patch-notes
article for the FO76 patch-notes pipeline.

Bethesda's "Inside the Vault" PTS articles stack several weeks of notes on
one page, separated by horizontal rules; only the newest section (the text
BEFORE the first `<hr>`) is the comparison baseline for a snapshot pair.
An LLM-mediated fetch (WebFetch) summarizes the whole page and blends the
weeks together, so this script does the cut mechanically:

    python3 -m pn fetch-notes <URL-or-local-.html> <out.txt>

1. Download the page (browser-like User-Agent, 30 s timeout), or read a
   local HTML file.
2. Truncate the raw HTML at the first case-insensitive `<hr` tag.
3. Strip tags (skipping <script>/<style>), collapse whitespace, write text.

Exit codes:
    0  wrote the text
    1  usage / unreadable local file
    2  fetch failed (HTTP error, network error)
    3  extracted text is too short (< MIN_TEXT_CHARS): almost certainly a
       client-rendered page shell -- the skill falls back to a WebFetch
       with an explicit split-at-first-horizontal-rule instruction.

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import re
import sys
import urllib.error
import urllib.request
from html.parser import HTMLParser
from pathlib import Path

USER_AGENT = (
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/128.0 Safari/537.36"
)
TIMEOUT_SECS = 30
MIN_TEXT_CHARS = 200
_HR_RE = re.compile(r"<hr\b", re.IGNORECASE)
_BLOCK_TAGS = {
    "p", "div", "br", "li", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6",
    "tr", "td", "th", "table", "section", "article", "header", "footer",
    "blockquote", "pre",
}


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


class _TextExtractor(HTMLParser):
    """Collect visible text, one line per block-level element, skipping
    <script>/<style> bodies."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self._parts: list[str] = []
        self._skip_depth = 0

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style"):
            self._skip_depth += 1
        elif tag in _BLOCK_TAGS:
            self._parts.append("\n")

    def handle_endtag(self, tag):
        if tag in ("script", "style") and self._skip_depth:
            self._skip_depth -= 1
        elif tag in _BLOCK_TAGS:
            self._parts.append("\n")

    def handle_data(self, data):
        if not self._skip_depth:
            self._parts.append(data)

    def text(self) -> str:
        raw = "".join(self._parts)
        lines = [re.sub(r"[ \t\r\f\v]+", " ", ln).strip() for ln in raw.split("\n")]
        out: list[str] = []
        for ln in lines:
            if ln:
                out.append(ln)
            elif out and out[-1] != "":
                out.append("")
        return "\n".join(out).strip() + "\n"


def cut_at_first_hr(html: str) -> tuple[str, bool]:
    """Return (html before the first `<hr`, whether an `<hr` was found)."""
    m = _HR_RE.search(html)
    if m is None:
        return html, False
    return html[: m.start()], True


def html_to_text(html: str) -> str:
    parser = _TextExtractor()
    parser.feed(html)
    parser.close()
    return parser.text()


def fetch_html(source: str) -> str:
    """Download `source` if it looks like a URL, else read it as a local file.
    Raises `urllib.error.URLError`/`OSError` on failure."""
    if re.match(r"^https?://", source, re.IGNORECASE):
        req = urllib.request.Request(source, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=TIMEOUT_SECS) as resp:  # noqa: S310 -- caller-supplied URL by design
            charset = resp.headers.get_content_charset() or "utf-8"
            return resp.read().decode(charset, errors="replace")
    return Path(source).read_text(encoding="utf-8", errors="replace")


def extract_newest_section(html: str) -> tuple[str, bool]:
    """Text of the newest notes section: everything before the first
    horizontal rule, tags stripped. Second value: whether an `<hr` was
    found (False means the page had a single section or is not the
    expected shape -- the caller decides whether that is acceptable)."""
    head, found = cut_at_first_hr(html)
    return html_to_text(head), found


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="pn fetch-notes",
        description="Fetch an official patch-notes page, keep only the newest section "
                    "(before the first <hr>), strip tags, write plain text.",
    )
    ap.add_argument("source", help="Article URL, or a local .html file")
    ap.add_argument("out", type=Path, help="Output text file (parent dirs are created)")
    ap.add_argument(
        "--min-chars", type=int, default=MIN_TEXT_CHARS,
        help=f"Fail (exit 3) when the extracted text is shorter than this (default {MIN_TEXT_CHARS})",
    )
    return ap


def main(argv=None) -> int:
    args = build_arg_parser().parse_args(argv)
    try:
        html = fetch_html(args.source)
    except (urllib.error.URLError, OSError, ValueError) as exc:
        eprint(f"error: could not fetch {args.source}: {exc}")
        return 2 if re.match(r"^https?://", args.source, re.IGNORECASE) else 1

    text, found_hr = extract_newest_section(html)
    if len(text.strip()) < args.min_chars:
        eprint(
            f"error: extracted only {len(text.strip())} chars of text from {args.source} "
            f"(page is probably client-rendered); fall back to a WebFetch that keeps only "
            f"the section before the first horizontal rule"
        )
        return 3

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(text, encoding="utf-8")
    eprint(
        f"wrote {args.out} ({len(text):,} chars; "
        f"{'cut at first <hr>' if found_hr else 'no <hr> found, whole page kept'})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
