#!/usr/bin/env python3
"""
Split a markdown file into Discord-safe chunks (≤1900 chars each).

Discord does NOT render: markdown tables, H1-H6 headers, horizontal rules.
Discord DOES render: **bold**, *italic*, `code`, ```code blocks```, > blockquotes, - bullets.

This tool converts GFM markdown to Discord-compatible markdown before chunking:
  - Tables        → monospace code-block tables (```...```)
  - # Headers     → **BOLD** with decorative separators
  - --- rules     → Unicode line
  - Everything else stays as-is (bold, italic, inline code, bullets work in Discord)

Chunking is section-aware: each `#`-`######` header starts a new section, and
whole sections are greedily packed into chunks so a Discord post is a
self-contained section (or run of small sections) rather than an arbitrary
blank-line-bounded slice. See split_into_chunks() for the packing rules.

Usage: python3 tools/discord_chunker.py <input.md> [output_dir]
"""

import sys
import os
import re

MAX_CHARS = 1900


# ---------------------------------------------------------------------------
# Markdown → Discord conversion
# ---------------------------------------------------------------------------

def strip_md_inline(text):
    """Remove inline markdown for use inside monospace code blocks."""
    text = re.sub(r'\*\*(.+?)\*\*', r'\1', text)
    text = re.sub(r'\*(.+?)\*',     r'\1', text)
    text = re.sub(r'`(.+?)`',       r'\1', text)
    text = re.sub(r'\[([^\]]+)\]\([^)]+\)', r'\1', text)
    return text


def format_table_as_discord(table_lines):
    """Convert a GFM table (list of raw lines) to a Discord monospace code block."""
    rows = []
    for line in table_lines:
        stripped = line.strip()
        if not stripped or stripped == '|':
            continue
        # Skip GFM separator rows (|---|---|)
        if re.match(r'^\|[\s\-:|]+\|$', stripped):
            continue
        cells = [strip_md_inline(c.strip()) for c in stripped.strip('|').split('|')]
        rows.append(cells)

    if not rows:
        return []

    col_count = max(len(r) for r in rows)
    for r in rows:
        while len(r) < col_count:
            r.append('')

    widths = [max((len(r[c]) for r in rows if c < len(r)), default=2) for c in range(col_count)]
    widths = [max(w, 2) for w in widths]
    sep = '  '.join('─' * w for w in widths)

    out_lines = []
    for i, row in enumerate(rows):
        padded = '  '.join(cell.ljust(widths[j]) for j, cell in enumerate(row) if j < col_count)
        out_lines.append(padded)
        if i == 0:
            out_lines.append(sep)

    # Wrap in a single code-fence block
    return ['```'] + out_lines + ['```']


def convert_to_discord_md_lines(text):
    """
    Convert standard GFM markdown to Discord-compatible markdown.

    Returns (lines, heading_indices): `lines` is the converted document split
    on '\\n'; `heading_indices` is a sorted list of indices into `lines`
    marking the single output line each source `#`-`######` header produced
    (the "**...**" decorated line -- not the blank lines H1/H2 wrap it in).
    Threading this through lets split_into_chunks() pack by section without
    re-sniffing bold lines after the fact.
    """
    lines = text.split('\n')
    result = []
    heading_indices = []
    in_table = False
    table_lines = []
    in_code_block = False

    for line in lines:
        if line.strip().startswith('```'):
            in_code_block = not in_code_block
            if in_table:
                result.extend(format_table_as_discord(table_lines))
                table_lines = []
                in_table = False
            result.append(line)
            continue

        if in_code_block:
            result.append(line)
            continue

        stripped = line.strip()

        if stripped.startswith('|'):
            if not in_table:
                in_table = True
                table_lines = []
            table_lines.append(line)
            continue
        else:
            if in_table:
                result.extend(format_table_as_discord(table_lines))
                table_lines = []
                in_table = False

        # --- horizontal rules → Unicode line
        if re.match(r'^[-*_]{3,}\s*$', stripped):
            result.append('─' * 36)
            continue

        # Convert markdown headers to bold
        m = re.match(r'^(#{1,6})\s+(.*)', line)
        if m:
            level = len(m.group(1))
            content = m.group(2)
            if level == 1:
                result.append('')
                heading_indices.append(len(result))
                result.append(f'**━━━━━━  {content}  ━━━━━━**')
                result.append('')
            elif level == 2:
                result.append('')
                heading_indices.append(len(result))
                result.append(f'**── {content} ──**')
                result.append('')
            else:
                heading_indices.append(len(result))
                result.append(f'**{content}**')
            continue

        result.append(line)

    if in_table:
        result.extend(format_table_as_discord(table_lines))

    return result, heading_indices


def convert_to_discord_md(text):
    """Thin string-returning wrapper around convert_to_discord_md_lines()."""
    lines, _heading_indices = convert_to_discord_md_lines(text)
    return '\n'.join(lines)


# ---------------------------------------------------------------------------
# Chunking (section-aware, code-block-aware split)
# ---------------------------------------------------------------------------

def _build_section_ranges(n, heading_set):
    """
    Partition [0, n) into (start, end) ranges, one per heading boundary, plus
    a leading preamble range if content precedes the first heading. Because
    every heading is a range boundary, a heading line can only ever be the
    *first* line of a range -- never buried inside one.
    """
    boundaries = sorted(i for i in heading_set if 0 <= i < n)
    starts = []
    if not boundaries or boundaries[0] != 0:
        starts.append(0)
    starts.extend(boundaries)

    ranges = []
    for i, s in enumerate(starts):
        e = starts[i + 1] if i + 1 < len(starts) else n
        if e > s:
            ranges.append((s, e))
    return ranges


def _is_heading_only_range(lines, heading_set, s, e):
    """True if every line in [s, e) is either a heading or blank -- i.e. the
    section has no body content of its own and must never be left dangling
    at the end of a chunk."""
    return all(i in heading_set or lines[i].strip() == '' for i in range(s, e))


def _merge_heading_only_ranges(lines, heading_set, ranges):
    """
    Fold each heading-only range forward into the range that follows it, so
    a lone heading (or a run of them, e.g. an H2 immediately followed by an
    H3 with nothing in between) is never packed as its own section -- it
    always travels with the next section that actually has body content.
    A trailing heading-only run with no following section is folded
    backward into the previous section instead (nothing left to attach it
    forward to).
    """
    merged = []
    pending_start = None

    for s, e in ranges:
        start = pending_start if pending_start is not None else s
        if _is_heading_only_range(lines, heading_set, start, e):
            pending_start = start
            continue
        merged.append((start, e))
        pending_start = None

    if pending_start is not None:
        if merged:
            prev_s, _prev_e = merged[-1]
            merged[-1] = (prev_s, ranges[-1][1])
        else:
            merged.append((pending_start, ranges[-1][1]))

    return merged


def _split_large_section(flagged_lines, max_chars):
    """
    Split one over-sized section into <= max_chars string chunks.

    `flagged_lines` is a list of (line, is_heading) pairs. Prefers blank-line
    split points and closes/reopens code fences when forced to split
    mid-block (the legacy whole-document splitter's local algorithm) -- but
    a candidate split point is rejected if the chunk it would produce ends
    (at its last non-blank line) on a heading line, and accumulation is
    deferred (letting the chunk grow past max_chars if truly necessary)
    while the chunk built so far ends on a heading, so a heading is never
    left childless.
    """
    chunks = []
    current = []  # list of (line, is_heading)
    current_len = 0
    in_code_block = False
    code_fence = '```'

    def flush(reopen_fence=False):
        nonlocal current, current_len
        chunk = '\n'.join(l for l, _h in current).strip()
        if chunk:
            chunks.append(chunk)
        if reopen_fence:
            current = [(code_fence, False)]
            current_len = len(code_fence) + 1
        else:
            current = []
            current_len = 0

    def ends_on_heading(prefix):
        for l, is_heading in reversed(prefix):
            if l.strip() == '':
                continue
            return is_heading
        return False  # all-blank (or empty) prefix -- nothing to protect

    for line, is_heading in flagged_lines:
        stripped = line.strip()
        if stripped.startswith('```'):
            in_code_block = not in_code_block

        line_len = len(line) + 1

        if current_len + line_len > max_chars and current and not ends_on_heading(current):
            if in_code_block:
                # We're inside a code block — close it, flush, reopen
                current.append((code_fence, False))
                flush(reopen_fence=True)
            else:
                split_at = None
                for j in range(len(current) - 1, max(0, len(current) - 30), -1):
                    if current[j][0].strip() == '' and not ends_on_heading(current[:j]):
                        split_at = j
                        break
                if split_at is not None:
                    kept = current[split_at + 1:]
                    current = current[:split_at]
                    flush()
                    current = kept
                    current_len = sum(len(l) + 1 for l, _h in current)
                else:
                    flush()

        current.append((line, is_heading))
        current_len += line_len

    flush()
    return chunks


def split_into_chunks(lines, heading_indices, max_chars=MAX_CHARS):
    """
    Pack converted Discord-markdown `lines` (as produced by
    convert_to_discord_md_lines()) into chunks <= max_chars each, aligned to
    section boundaries.

    - A section starts at each heading line (content before the first
      heading is a preamble section). Whole sections are greedily packed
      into a chunk; if the next section doesn't fit in the current
      non-empty chunk, the chunk is flushed and a new one started at that
      section's heading.
    - A single section larger than max_chars is split internally at blank
      lines (falling back to the legacy whole-document behavior, including
      the code-fence close/reopen trick), but never immediately after its
      heading.
    - Invariant: no chunk's last non-blank line is ever a heading line --
      heading-only sections (including a heading run like H2 immediately
      followed by H3) are folded into the next section before packing.
    """
    n = len(lines)
    heading_set = set(i for i in heading_indices if 0 <= i < n)
    is_heading = [i in heading_set for i in range(n)]

    ranges = _build_section_ranges(n, heading_set)
    ranges = _merge_heading_only_ranges(lines, heading_set, ranges)

    chunks = []
    current_lines = []
    current_len = 0

    def flush_current():
        nonlocal current_lines, current_len
        chunk = '\n'.join(current_lines).strip()
        if chunk:
            chunks.append(chunk)
        current_lines = []
        current_len = 0

    for s, e in ranges:
        section_lines = lines[s:e]
        section_len = sum(len(l) + 1 for l in section_lines)

        if section_len <= max_chars:
            if current_lines and current_len + section_len > max_chars:
                flush_current()
            current_lines.extend(section_lines)
            current_len += section_len
        else:
            if current_lines:
                flush_current()
            flagged = list(zip(section_lines, is_heading[s:e]))
            chunks.extend(_split_large_section(flagged, max_chars))

    flush_current()
    return [c for c in chunks if c.strip()]


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <input.md> [output_dir]")
        sys.exit(1)

    input_path = sys.argv[1]
    output_dir = sys.argv[2] if len(sys.argv) > 2 else "discord_chunks"

    with open(input_path) as f:
        text = f.read()

    print(f"Read {len(text):,} chars from {input_path}", file=sys.stderr)
    discord_lines, heading_indices = convert_to_discord_md_lines(text)
    discord_text = '\n'.join(discord_lines)
    print(f"Converted to Discord markdown: {len(discord_text):,} chars", file=sys.stderr)

    os.makedirs(output_dir, exist_ok=True)

    chunks = split_into_chunks(discord_lines, heading_indices)
    total = len(chunks)
    print(f"Split into {total} Discord chunks (≤{MAX_CHARS} chars each)")

    oversized = 0
    for i, chunk in enumerate(chunks, 1):
        path = os.path.join(output_dir, f"chunk_{i:03d}.md")
        header = f"*(Part {i}/{total})*\n\n"
        content = header + chunk
        # Hard safety net — should not trigger with the code-block split above
        if len(content) > 2000:
            content = content[:1970] + '\n*[truncated — chunk too large]*'
            oversized += 1
        with open(path, 'w') as f:
            f.write(content)

    sizes = [len(c) for c in chunks]
    print(f"Written to {output_dir}/")
    print(f"Largest chunk: {max(sizes)} chars")
    print(f"Average chunk: {sum(sizes) // len(sizes)} chars")
    print(f"Smallest chunk: {min(sizes)} chars")
    if oversized:
        print(f"WARNING: {oversized} chunks exceeded 2000 chars and were hard-truncated", file=sys.stderr)


if __name__ == "__main__":
    main()
