"""Tests for pn/formids.py."""

from __future__ import annotations

import unittest

from pn import formids


class TestFormids(unittest.TestCase):
    def test_display_matches_esm_rendering(self):
        self.assertEqual(formids.display(0x463F), "0x0000463F")
        self.assertEqual(formids.display("0x00463f"), "0x0000463F")

    def test_bare_tokens_read_as_hex_up_to_eight_digits(self):
        self.assertEqual(formids.to_int("00568635"), 0x00568635)
        self.assertEqual(formids.to_int("463F"), 0x463F)
        self.assertEqual(formids.to_int("4294967296"), 4294967296)

    def test_canonical_accepts_short_and_mixed_case_tokens(self):
        self.assertEqual(formids.canonical("0X463f"), "0x0000463F")
        self.assertIsNone(formids.canonical("463F"))
        self.assertIsNone(formids.canonical("Rifle"))

    def test_is_rendered_requires_the_full_form(self):
        self.assertTrue(formids.is_rendered("0x0000463F"))
        self.assertFalse(formids.is_rendered("0x463F"))

    def test_sort_key_tolerates_garbage(self):
        self.assertEqual(formids.sort_key("0x10"), 16)
        self.assertEqual(formids.sort_key("nope"), 0)


if __name__ == "__main__":
    unittest.main()
