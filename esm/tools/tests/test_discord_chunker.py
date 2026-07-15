#!/usr/bin/env python3
"""Tests for tools/discord_chunker.py.

Covers section-aware chunking: no chunk's last non-blank line may be a
heading (including a merged H2-immediately-followed-by-H3 run), sections
that don't fit start a fresh chunk at their own heading, an over-sized
single section still splits at blank lines without leaving its heading
childless, code-fence close/reopen still works when forced to split inside
a code block, and no source content is lost or a chunk left oversized.
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import discord_chunker as dc  # noqa: E402

MAX_CHARS = dc.MAX_CHARS


def convert_and_split(text, max_chars=MAX_CHARS):
    lines, heading_indices = dc.convert_to_discord_md_lines(text)
    return dc.split_into_chunks(lines, heading_indices, max_chars)


def heading_lines(chunk):
    """Every line in `chunk` that looks like the bold heading line the
    converter emits for a source header (used only to sanity-check fixtures,
    not as the mechanism under test -- the real signal is heading_indices)."""
    return [l for l in chunk.splitlines() if l.strip().startswith('**') and l.strip().endswith('**')]


def last_non_blank_line(chunk):
    for line in reversed(chunk.splitlines()):
        if line.strip():
            return line
    return ''


def assert_no_chunk_ends_on_heading(test, chunks, heading_texts):
    """heading_texts: the exact "**...**" decorated lines that must never be
    a chunk's last non-blank line."""
    for i, chunk in enumerate(chunks):
        tail = last_non_blank_line(chunk)
        test.assertNotIn(tail, heading_texts, f"chunk {i} ends on a heading line: {tail!r}")


# --------------------------------------------------------------------------
# convert_to_discord_md_lines: heading index bookkeeping
# --------------------------------------------------------------------------


class TestConvertToDiscordMdLines(unittest.TestCase):
    def test_h1_through_h6_each_produce_exactly_one_heading_line(self):
        doc = "\n".join(f"{'#' * level} Heading {level}" for level in range(1, 7))
        lines, heading_indices = dc.convert_to_discord_md_lines(doc)
        self.assertEqual(len(heading_indices), 6)
        for idx in heading_indices:
            self.assertTrue(lines[idx].strip().startswith('**'))
            self.assertTrue(lines[idx].strip().endswith('**'))

    def test_h1_and_h2_are_wrapped_in_blank_lines_heading_index_points_at_bold_line(self):
        lines, heading_indices = dc.convert_to_discord_md_lines("## Section\ncontent")
        self.assertEqual(len(heading_indices), 1)
        idx = heading_indices[0]
        self.assertEqual(lines[idx], '**── Section ──**')
        self.assertEqual(lines[idx - 1], '')
        self.assertEqual(lines[idx + 1], '')

    def test_h3_plus_has_no_surrounding_blank_lines(self):
        lines, heading_indices = dc.convert_to_discord_md_lines("### Sub\ncontent")
        idx = heading_indices[0]
        self.assertEqual(lines[idx], '**Sub**')
        self.assertEqual(lines[idx + 1], 'content')

    def test_headers_inside_code_blocks_are_not_flagged(self):
        doc = "```\n# not a heading\n```\n## Real Heading\nbody"
        lines, heading_indices = dc.convert_to_discord_md_lines(doc)
        self.assertEqual(len(heading_indices), 1)
        self.assertEqual(lines[heading_indices[0]], '**── Real Heading ──**')

    def test_convert_to_discord_md_wrapper_matches_lines_joined(self):
        doc = "## Section\nsome text\n\n### Sub\nmore text"
        lines, _heading_indices = dc.convert_to_discord_md_lines(doc)
        self.assertEqual(dc.convert_to_discord_md(doc), '\n'.join(lines))


# --------------------------------------------------------------------------
# Invariant: no chunk ends with a heading
# --------------------------------------------------------------------------


class TestNoChunkEndsWithHeading(unittest.TestCase):
    def test_forced_boundary_regression(self):
        # Sized so that, under the OLD blank-line-only splitter, the
        # accumulated chunk would overflow right after "## Section Two"'s
        # heading + its trailing blank line, and the nearest blank line
        # found by a naive backward scan is that very one -- flushing a
        # chunk that ends on the heading and starting the next chunk on an
        # orphaned body with no heading at all. Confirmed against a
        # reimplementation of the pre-refactor algorithm during development:
        # it produces a chunk ending in "**── Section Two ──**".
        doc = (
            "## Section One\n"
            + ("X" * 1700) + "\n"
            + "## Section Two\n"
            + ("Y" * 300) + "\n"
        )
        chunks = convert_and_split(doc)
        heads = ['**── Section One ──**', '**── Section Two ──**']
        assert_no_chunk_ends_on_heading(self, chunks, heads)
        # And, per the section-alignment rule, Section Two starts its own
        # fresh chunk rather than being glued mid-flow to Section One.
        self.assertEqual(len(chunks), 2)
        self.assertTrue(chunks[1].startswith('**── Section Two ──**'))
        self.assertIn('Y' * 300, chunks[1])
        self.assertIn('X' * 1700, chunks[0])

    def test_many_small_sections_never_end_a_chunk_on_a_heading(self):
        # A long run of small H2 sections, sized so several pack per chunk --
        # check the invariant broadly across all resulting chunk boundaries.
        sections = [f"## Section {i}\nBody text for section {i}. " * 3 for i in range(30)]
        doc = "\n".join(sections)
        chunks = convert_and_split(doc)
        heads = [f'**── Section {i} ──**' for i in range(30)]
        assert_no_chunk_ends_on_heading(self, chunks, heads)
        self.assertGreater(len(chunks), 1)


# --------------------------------------------------------------------------
# Section alignment
# --------------------------------------------------------------------------


class TestSectionAlignment(unittest.TestCase):
    def test_next_chunk_starts_with_the_section_heading_that_did_not_fit(self):
        doc = (
            "## Alpha\n"
            + ("A" * 1500) + "\n"
            + "## Beta\n"
            + ("B" * 1000) + "\n"
        )
        chunks = convert_and_split(doc)
        self.assertEqual(len(chunks), 2)
        self.assertTrue(chunks[0].startswith('**── Alpha ──**'))
        self.assertTrue(chunks[1].startswith('**── Beta ──**'))

    def test_small_adjacent_sections_are_merged_into_one_chunk(self):
        doc = "## One\nshort body one\n\n## Two\nshort body two\n\n## Three\nshort body three\n"
        chunks = convert_and_split(doc)
        self.assertEqual(len(chunks), 1)
        self.assertIn('**── One ──**', chunks[0])
        self.assertIn('**── Two ──**', chunks[0])
        self.assertIn('**── Three ──**', chunks[0])

    def test_preamble_before_first_heading_is_its_own_section(self):
        doc = "Some preamble text.\n\n## First Heading\nbody\n"
        chunks = convert_and_split(doc)
        self.assertEqual(len(chunks), 1)
        self.assertTrue(chunks[0].startswith('Some preamble text.'))


# --------------------------------------------------------------------------
# Oversized single section
# --------------------------------------------------------------------------


class TestOversizedSection(unittest.TestCase):
    def test_splits_at_blank_lines_without_orphaning_its_own_heading(self):
        paragraphs = [("Paragraph %d filler text. " % i) * 12 for i in range(20)]
        body = "\n\n".join(paragraphs)
        doc = "## BigSection\n" + body + "\n"
        chunks = convert_and_split(doc)

        self.assertGreater(len(chunks), 1, "fixture should be big enough to force a split")
        for c in chunks:
            self.assertLessEqual(len(c), MAX_CHARS)
        assert_no_chunk_ends_on_heading(self, chunks, ['**── BigSection ──**'])
        # The heading's own chunk must carry at least one real paragraph
        # with it, not stand alone.
        self.assertTrue(chunks[0].startswith('**── BigSection ──**'))
        first_chunk_body = chunks[0][len('**── BigSection ──**'):].strip()
        self.assertTrue(first_chunk_body, "heading must not be left childless in its chunk")
        # No paragraph text lost.
        joined = "\n".join(chunks)
        for p in paragraphs:
            self.assertIn(p.strip(), joined)


# --------------------------------------------------------------------------
# Consecutive heading run (H2 immediately followed by H3)
# --------------------------------------------------------------------------


class TestConsecutiveHeadingRun(unittest.TestCase):
    def test_h2_then_h3_with_no_body_between_never_ends_a_chunk(self):
        # Padding before the H2/H3 pair forces a chunk boundary right at (or
        # near) the heading run.
        doc = (
            "## Zero\n"
            + ("A" * 1700) + "\n"
            + "## Parent\n"
            + "### Child\n"
            + ("B" * 400) + "\n"
        )
        chunks = convert_and_split(doc)
        heads = ['**── Zero ──**', '**── Parent ──**', '**Child**']
        assert_no_chunk_ends_on_heading(self, chunks, heads)
        # Parent+Child must travel together (Parent alone has no body of its
        # own -- it is folded forward onto Child's section).
        parent_chunk = next(c for c in chunks if '**── Parent ──**' in c)
        self.assertIn('**Child**', parent_chunk)
        self.assertIn('B' * 400, parent_chunk)

    def test_oversized_heading_run_plus_body_still_avoids_orphaning_either_heading(self):
        paragraphs = [("Child body paragraph %d. " % i) * 15 for i in range(15)]
        body = "\n\n".join(paragraphs)
        doc = "## Parent\n### Child\n" + body + "\n"
        chunks = convert_and_split(doc)
        self.assertGreater(len(chunks), 1)
        for c in chunks:
            self.assertLessEqual(len(c), MAX_CHARS)
        assert_no_chunk_ends_on_heading(self, chunks, ['**── Parent ──**', '**Child**'])
        self.assertTrue(chunks[0].startswith('**── Parent ──**'))
        self.assertIn('**Child**', chunks[0])


# --------------------------------------------------------------------------
# Code-fence close/reopen
# --------------------------------------------------------------------------


class TestCodeFenceSplitting(unittest.TestCase):
    def test_splitting_inside_a_code_block_closes_and_reopens_the_fence(self):
        code_lines = [f"line {i:03d} of code padded out a fair bit xxxxxxxxxxxxx" for i in range(60)]
        doc = "## CodeSection\nintro text\n\n```\n" + "\n".join(code_lines) + "\n```\n"
        chunks = convert_and_split(doc)
        self.assertGreater(len(chunks), 1)
        for c in chunks:
            self.assertLessEqual(len(c), MAX_CHARS)
            # Every chunk touched by the code block has a balanced (even)
            # number of fence markers.
            self.assertEqual(c.count('```') % 2, 0)
        # No code line lost.
        joined = "\n".join(chunks)
        for line in code_lines:
            self.assertIn(line, joined)


# --------------------------------------------------------------------------
# General: size cap and no content lost
# --------------------------------------------------------------------------


class TestSizeCapAndContentPreservation(unittest.TestCase):
    def test_every_chunk_within_max_chars_and_no_line_lost_in_order(self):
        rng_bodies = [f"Line of content number {i} with some padding text here." for i in range(400)]
        doc_lines = []
        for i in range(0, len(rng_bodies), 20):
            doc_lines.append(f"## Section {i // 20}")
            doc_lines.extend(rng_bodies[i:i + 20])
            doc_lines.append("")
        doc = "\n".join(doc_lines)

        chunks = convert_and_split(doc)
        for c in chunks:
            self.assertLessEqual(len(c), MAX_CHARS)

        joined_chunk_lines = []
        for c in chunks:
            joined_chunk_lines.extend(c.splitlines())
        non_blank_chunk_lines = [l for l in joined_chunk_lines if l.strip()]

        source_non_blank = [l for l in rng_bodies]
        # All source body lines appear, in order, somewhere across the
        # concatenated chunks (headings interleave but relative order of the
        # body lines themselves is preserved).
        it = iter(non_blank_chunk_lines)
        for line in source_non_blank:
            for candidate in it:
                if candidate == line:
                    break
            else:
                self.fail(f"content lost or reordered: {line!r} not found in order")


if __name__ == "__main__":
    unittest.main()
