#!/usr/bin/env python3
"""Tests for tools/check_coverage.py: DEEP ids must be covered by exactly
one draft (or deferred to one), anchors must appear in the covering draft,
and -- with --summary -- in patch-summary.md unless cut with a reason."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import check_coverage as cv  # noqa: E402
import layout  # noqa: E402


def bundle(bid, fid, edid, name):
    return {
        "id": bid, "title": f"{name} (WEAP)",
        "anchor": {"form_id": fid, "record_type": "WEAP", "editor_id": edid, "name": name, "status": "changed"},
        "members": [{"form_id": fid, "record_type": "WEAP", "editor_id": edid, "name": name, "status": "changed", "role": "anchor"}],
        "edges": [], "bug_watch": False, "lint_ids": [],
    }


BUNDLES = [
    bundle("B0001", "0x00000001", "WeapA", "Weapon A"),
    bundle("B0002", "0x00000002", "WeapB", "Weapon B"),
    bundle("B0003", "0x00000003", "WeapC", "Weapon C"),
]


class TestCoverage(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        layout.work_dir(self.tmp).mkdir()
        layout.work_triage_json(self.tmp).write_text(json.dumps({"deep": ["B0001", "B0002", "B0003"]}))
        layout.work_deep_slice_json(self.tmp).write_text(json.dumps({"bundles": BUNDLES, "lints": []}))
        layout.drafts_dir(self.tmp).mkdir()

    def tearDown(self):
        self._tmp.cleanup()

    def report(self, name, covered, text, deferred=None):
        (layout.drafts_dir(self.tmp) / f"{name}.report.json").write_text(
            json.dumps({"bundles_covered": covered, "deferred": deferred or []})
        )
        (layout.drafts_dir(self.tmp) / f"{name}.md").write_text(text)

    def kinds(self, payload):
        return sorted(v["kind"] for v in payload["violations"])

    def test_all_covered_passes(self):
        self.report("deep", ["B0001", "B0002", "B0003"], "Weapon A, `WeapB`, and Evidence: 0x00000003 all change.")
        payload = cv.run_check(self.tmp)
        self.assertTrue(payload["ok"], payload["violations"])
        self.assertEqual(payload["covered"], {"B0001": "deep.report.json", "B0002": "deep.report.json", "B0003": "deep.report.json"})
        self.assertTrue(layout.work_coverage_json(self.tmp).is_file())

    def test_no_reports_means_every_deep_id_uncovered(self):
        payload = cv.run_check(self.tmp)
        self.assertEqual(self.kinds(payload), ["uncovered", "uncovered", "uncovered"])
        self.assertEqual(cv.main([str(self.tmp)]), 1)

    def test_anchor_missing_from_draft_is_a_violation(self):
        self.report("deep", ["B0001", "B0002", "B0003"], "Weapon A and Weapon B changed. Nothing about the third.")
        payload = cv.run_check(self.tmp)
        self.assertEqual(self.kinds(payload), ["anchor_missing"])
        self.assertEqual(payload["violations"][0]["bundle_id"], "B0003")

    def test_deferred_and_covered_by_other_draft(self):
        self.report("deep.part1", ["B0001"], "Weapon A. (Weapon B: see part 2)",
                    deferred=[{"form_ids": ["0x00000002"], "expected_owner": "part2", "note": "n"}])
        self.report("deep.part2", ["B0002", "B0003"], "Weapon B and Weapon C.")
        payload = cv.run_check(self.tmp)
        self.assertTrue(payload["ok"], payload["violations"])
        self.assertEqual(payload["deferred_resolved"]["B0002"]["deferred_by"], ["deep.part1.report.json"])

    def test_deferred_to_nobody(self):
        self.report("deep.part1", ["B0001"], "Weapon A.", deferred=[{"form_ids": ["0x00000002"], "expected_owner": "part2", "note": "n"}])
        self.report("deep.part2", ["B0003"], "Weapon C.")
        payload = cv.run_check(self.tmp)
        self.assertEqual(self.kinds(payload), ["deferred_uncovered"])

    def test_double_covered(self):
        self.report("deep.part1", ["B0001", "B0002"], "Weapon A and Weapon B.")
        self.report("deep.part2", ["B0002", "B0003"], "Weapon B and Weapon C.")
        payload = cv.run_check(self.tmp)
        self.assertEqual(self.kinds(payload), ["double_covered"])

    def test_report_without_bundles_covered_list(self):
        (layout.drafts_dir(self.tmp) / "deep.report.json").write_text(json.dumps({"bundles": 3}))
        (layout.drafts_dir(self.tmp) / "deep.md").write_text("Weapon A, Weapon B, Weapon C")
        payload = cv.run_check(self.tmp)
        self.assertIn("report_without_bundles_covered", self.kinds(payload))

    def test_summary_requires_anchor_or_cut_with_reason(self):
        self.report("deep", ["B0001", "B0002", "B0003"], "Weapon A, Weapon B, Weapon C.")
        layout.patch_summary_md(self.tmp).write_text("# Patch\nWeapon A and Weapon B.\n")
        payload = cv.run_check(self.tmp, summary=True)
        self.assertEqual(self.kinds(payload), ["missing_from_summary"])
        layout.work_cuts_json(self.tmp).write_text(json.dumps({"cuts": [{"bundle_id": "B0003", "reason": ""}]}))
        self.assertEqual(self.kinds(cv.run_check(self.tmp, summary=True)), ["missing_from_summary"])
        layout.work_cuts_json(self.tmp).write_text(json.dumps({"cuts": [{"bundle_id": "B0003", "reason": "over budget"}]}))
        payload = cv.run_check(self.tmp, summary=True)
        self.assertTrue(payload["ok"], payload["violations"])
        self.assertEqual(payload["cut"], {"B0003": "over budget"})
        self.assertEqual(cv.main([str(self.tmp), "--summary"]), 0)

    def test_formid_match_is_case_insensitive_editor_id_exact(self):
        self.report("deep", ["B0001", "B0002", "B0003"], "Evidence: 0x00000001. weapb is not the editor id. Evidence: 0X00000003")
        payload = cv.run_check(self.tmp)
        self.assertEqual(self.kinds(payload), ["anchor_missing"])
        self.assertEqual(payload["violations"][0]["bundle_id"], "B0002")


if __name__ == "__main__":
    unittest.main()
