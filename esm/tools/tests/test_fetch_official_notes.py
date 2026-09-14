#!/usr/bin/env python3
"""Tests for tools/fetch_official_notes.py: cut at the first <hr>, strip
tags, exit codes for fetch failure / client-rendered shells; network is
mocked via urllib.request.urlopen."""

from __future__ import annotations

import sys
import unittest
import urllib.error
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import fetch_official_notes as fon  # noqa: E402
from builders import TempDirTestCase  # noqa: E402

PAGE = """<html><head><title>Inside the Vault</title><style>.x{}</style>
<script>window.__x = 1;</script></head><body>
<article><h1>PTS notes — week 2</h1><p>Weapon A damage is now 25.</p>
<ul><li>Item &amp; thing</li><li>Other</li></ul>
<HR class="sep"/>
<h1>PTS notes — week 1</h1><p>Old stuff that must not leak.</p></article>
</body></html>"""


class FakeResponse:
    """The slice of an `http.client.HTTPResponse` the script touches."""

    def __init__(self, body: bytes, charset="utf-8"):
        self._body = body
        self.headers = mock.Mock()
        self.headers.get_content_charset.return_value = charset

    def read(self) -> bytes:
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        return None


class TestExtraction(unittest.TestCase):
    def test_cut_at_first_hr_case_insensitive(self):
        head, found = fon.cut_at_first_hr(PAGE)
        self.assertTrue(found)
        self.assertIn("week 2", head)
        self.assertNotIn("week 1", head)

    def test_no_hr_keeps_whole_page(self):
        head, found = fon.cut_at_first_hr("<p>only</p>")
        self.assertFalse(found)
        self.assertEqual(head, "<p>only</p>")

    def test_html_to_text_strips_tags_scripts_and_entities(self):
        text, found = fon.extract_newest_section(PAGE)
        self.assertTrue(found)
        self.assertIn("Weapon A damage is now 25.", text)
        self.assertIn("Item & thing", text)
        self.assertNotIn("window.__x", text)
        self.assertNotIn(".x{}", text)
        self.assertNotIn("Old stuff", text)
        self.assertNotIn("<", text)


class TestMain(TempDirTestCase):
    def test_local_html_file(self):
        src = self.tmp / "page.html"
        src.write_text(PAGE)
        out = self.tmp / "work" / "official-notes.txt"
        self.assertEqual(fon.main([str(src), str(out), "--min-chars", "10"]), 0)
        self.assertIn("week 2", out.read_text())
        self.assertNotIn("week 1", out.read_text())

    def test_url_fetch_is_mocked_and_cut(self):
        out = self.tmp / "notes.txt"
        with mock.patch.object(fon.urllib.request, "urlopen", return_value=FakeResponse(PAGE.encode())) as m:
            self.assertEqual(fon.main(["https://example.test/itv", str(out), "--min-chars", "10"]), 0)
        req = m.call_args[0][0]
        self.assertIn("Mozilla", req.headers.get("User-agent", ""))
        self.assertIn("Weapon A", out.read_text())

    def test_fetch_error_exits_2(self):
        with mock.patch.object(fon.urllib.request, "urlopen", side_effect=urllib.error.URLError("down")):
            self.assertEqual(fon.main(["https://example.test/itv", str(self.tmp / "x.txt")]), 2)

    def test_missing_local_file_exits_1(self):
        self.assertEqual(fon.main([str(self.tmp / "nope.html"), str(self.tmp / "x.txt")]), 1)

    def test_client_rendered_shell_exits_3(self):
        shell = "<html><body><div id='app'></div><script>render()</script></body></html>"
        with mock.patch.object(fon.urllib.request, "urlopen", return_value=FakeResponse(shell.encode())):
            self.assertEqual(fon.main(["https://example.test/itv", str(self.tmp / "x.txt")]), 3)
        self.assertFalse((self.tmp / "x.txt").exists())


if __name__ == "__main__":
    unittest.main()
