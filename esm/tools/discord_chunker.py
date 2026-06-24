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


def convert_to_discord_md(text):
    """
    Convert standard GFM markdown to Discord-compatible markdown.
    Returns the converted string.
    """
    lines = text.split('\n')
    result = []
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
                result.append(f'**━━━━━━  {content}  ━━━━━━**')
                result.append('')
            elif level == 2:
                result.append('')
                result.append(f'**── {content} ──**')
                result.append('')
            elif level == 3:
                result.append(f'**{content}**')
            else:
                result.append(f'**{content}**')
            continue

        result.append(line)

    if in_table:
        result.extend(format_table_as_discord(table_lines))

    return '\n'.join(result)


# ---------------------------------------------------------------------------
# Chunking (code-block-aware split)
# ---------------------------------------------------------------------------

def split_into_chunks(text, max_chars=MAX_CHARS):
    """
    Split markdown text into chunks ≤ max_chars each.
    Prefers blank-line splits, but will split mid-code-block at line boundaries
    by closing and reopening the fence — so no content is truncated.
    """
    lines = text.split('\n')
    chunks = []
    current = []     # lines in the current chunk
    current_len = 0
    in_code_block = False
    code_fence = '```'

    def flush(reopen_fence=False):
        nonlocal current, current_len
        chunk = '\n'.join(current).strip()
        if chunk:
            chunks.append(chunk)
        if reopen_fence:
            current = [code_fence]
            current_len = len(code_fence) + 1
        else:
            current = []
            current_len = 0

    for line in lines:
        stripped = line.strip()
        if stripped.startswith('```'):
            in_code_block = not in_code_block

        line_len = len(line) + 1

        if current_len + line_len > max_chars and current:
            if in_code_block:
                # We're inside a code block — close it, flush, reopen
                current.append(code_fence)
                flush(reopen_fence=True)
            else:
                # Look for last blank line in recent history
                split_at = None
                for j in range(len(current) - 1, max(0, len(current) - 30), -1):
                    if current[j].strip() == '':
                        split_at = j
                        break
                if split_at is not None:
                    kept = current[split_at + 1:]
                    current = current[:split_at]
                    flush()
                    current = kept
                    current_len = sum(len(l) + 1 for l in current)
                else:
                    flush()

        current.append(line)
        current_len += line_len

    flush()
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
    discord_text = convert_to_discord_md(text)
    print(f"Converted to Discord markdown: {len(discord_text):,} chars", file=sys.stderr)

    os.makedirs(output_dir, exist_ok=True)

    chunks = split_into_chunks(discord_text)
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
