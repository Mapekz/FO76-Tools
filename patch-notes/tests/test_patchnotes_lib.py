#!/usr/bin/env python3
"""Tests for pn/patchnotes_lib.py."""

import tempfile
import unittest
from pathlib import Path

from pn import patchnotes_lib as pl
from tests.builders import load_fixture

# ---------------------------------------------------------------------------
# annotate_ref
# ---------------------------------------------------------------------------


class TestAnnotateRef(unittest.TestCase):
    def test_with_name_and_edid(self):
        ref_names = {"0x00000001": {"record_type": "AMMO", "editor_id": "Ammo1", "name": "Round"}}
        out = pl.annotate_ref("0x00000001", ref_names)
        self.assertIn("0x00000001", out)
        self.assertIn("AMMO", out)
        self.assertIn("Ammo1", out)
        self.assertIn("Round", out)

    def test_with_edid_only(self):
        ref_names = {"0x00000002": {"record_type": "KYWD", "editor_id": "SomeKeyword"}}
        out = pl.annotate_ref("0x00000002", ref_names)
        self.assertIn("SomeKeyword", out)
        self.assertNotIn('"', out)  # no quoted name/description

    def test_with_description_only_fallback(self):
        ref_names = {"0x00000003": {"record_type": "MISC", "description": "A rare thing."}}
        out = pl.annotate_ref("0x00000003", ref_names)
        self.assertIn("A rare thing.", out)

    def test_dangling_no_ref_names_entry(self):
        out = pl.annotate_ref("0x000000FF", {})
        self.assertEqual(out, "`0x000000FF`")

    def test_stub_dict_form(self):
        stub = {"formid": "0x00000004", "record_type": "WEAP", "editor_id": "Foo", "name": "Foo Gun"}
        out = pl.annotate_ref(stub, {})
        self.assertIn("Foo Gun", out)
        self.assertIn("WEAP", out)

    def test_fixture_dangling_formid_in_context(self):
        diff_data = load_fixture("diff_small.json")
        out = pl.annotate_ref("0x00099999", diff_data["ref_names"])
        self.assertEqual(out, "`0x00099999`")


# ---------------------------------------------------------------------------
# Manifest round-trip
# ---------------------------------------------------------------------------


class TestManifest(unittest.TestCase):
    def test_new_manifest_shape(self):
        m = pl.new_manifest(
            patch_date="2026-07-03",
            old_token="20260626",
            new_token="20260703",
            new_esm_size=123456,
            new_esm_mtime=1234567890.0,
            pipeline_version="1.0.0",
        )
        self.assertEqual(m["schema_version"], 1)
        self.assertEqual(m["patch_date"], "2026-07-03")
        self.assertEqual(m["inputs"]["old_token"], "20260626")
        self.assertEqual(m["stages"]["mechanical"]["completed_at"], None)
        self.assertEqual(m["stages"]["narrative"]["max_chunk_chars"], 2000)
        # Seeded narrative shape must match the LIVE schema_version 3 shape
        # update_manifest.py writes (see NARRATIVE_SCHEMA_VERSION's
        # docstring) -- not the retired per-category shape ("categories": []).
        self.assertEqual(m["stages"]["narrative"]["schema_version"], 3)
        self.assertIsNone(m["stages"]["narrative"]["usage"])
        self.assertNotIn("categories", m["stages"]["narrative"])
        self.assertEqual(m["stages"]["narrative"]["discord_dir"], "discord")

    def test_write_then_load_roundtrip(self):
        m = pl.new_manifest("2026-07-03", "a", "b", 1, 2.0, "1.0.0", counts={"added": 3})
        with tempfile.TemporaryDirectory() as tmp:
            pl.write_manifest(tmp, m)
            loaded = pl.load_manifest(tmp)
            self.assertEqual(loaded, m)

    def test_esm_is_localized_reads_the_tes4_flag(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "x.esm"
            for flags, want in ((0x81, True), (0x01, False)):
                path.write_bytes(b"TES4" + b"\0" * 4 + flags.to_bytes(4, "little") + b"\0" * 12)
                self.assertIs(pl.esm_is_localized(path), want)
            path.write_bytes(b"FAKE ESM BYTES")
            self.assertIsNone(pl.esm_is_localized(path))

    def test_new_manifest_records_localized_flags(self):
        m = pl.new_manifest("2026-07-03", "a", "b", 1, 2.0, "1.0.0", localized={"old": True, "new": False})
        self.assertEqual(m["inputs"]["localized"], {"old": True, "new": False})
        m = pl.new_manifest("2026-07-03", "a", "b", 1, 2.0, "1.0.0")
        self.assertEqual(m["inputs"]["localized"], {"old": None, "new": None})

    def test_load_manifest_missing_returns_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertIsNone(pl.load_manifest(tmp))

    def test_write_manifest_creates_out_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            nested = Path(tmp) / "nested" / "dir"
            m = pl.new_manifest("2026-07-03", "a", "b", 1, 2.0, "1.0.0")
            pl.write_manifest(nested, m)
            self.assertTrue((nested / "manifest.json").exists())


class TestWireShapeValidation(unittest.TestCase):
    def test_validate_record_entry_rejects_missing_key(self):
        with self.assertRaises(KeyError) as ctx:
            pl.validate_record_entry({"form_id": "0x01", "record_type": "MISC"})
        self.assertIn("status", str(ctx.exception))

    def test_validate_bundle_rejects_bad_member_role(self):
        bundle = {
            "title": "t",
            "anchor": {
                "form_id": "0x01",
                "record_type": "MISC",
                "editor_id": "e",
                "name": None,
                "status": "changed",
            },
            "members": [{
                "form_id": "0x01",
                "record_type": "MISC",
                "editor_id": "e",
                "name": None,
                "status": "changed",
                "role": "not_a_role",
            }],
            "edges": [],
            "id": "B0001",
        }
        with self.assertRaises(ValueError) as ctx:
            pl.validate_bundle(bundle)
        self.assertIn("role", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
