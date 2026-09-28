#!/usr/bin/env python3
"""Tests for pn/check_coverage.py: DEEP ids must be covered by exactly
one draft (or deferred to one), anchors must appear in the covering draft,
and -- with --summary -- in patch-summary.md unless cut with a reason."""

from __future__ import annotations

import json
import unittest

from pn import check_coverage as cv
from pn import layout
from tests import builders
from tests.builders import TempDirTestCase


def bundle(bid, fid, edid, name):
    anchor = builders.member(form_id=fid, record_type="WEAP", editor_id=edid, name=name, role="anchor")
    return builders.bundle(
        id=bid, title=f"{name} (WEAP)",
        anchor=builders.anchor_of(anchor), members=[anchor],
    )


BUNDLES = [
    bundle("B0001", "0x00000001", "WeapA", "Weapon A"),
    bundle("B0002", "0x00000002", "WeapB", "Weapon B"),
    bundle("B0003", "0x00000003", "WeapC", "Weapon C"),
]


class TestCoverage(TempDirTestCase):
    def setUp(self):
        super().setUp()
        layout.work_dir(self.tmp).mkdir()
        layout.work_triage_json(self.tmp).write_text(json.dumps(builders.triage(deep=["B0001", "B0002", "B0003"])))
        deep_slice = [{**b, "bug_watch": False, "lint_ids": []} for b in BUNDLES]
        layout.work_deep_slice_json(self.tmp).write_text(json.dumps({"bundles": deep_slice, "lints": []}))
        layout.drafts_dir(self.tmp).mkdir()

    def report(self, name, covered, text, deferred=None):
        (layout.drafts_dir(self.tmp) / f"{name}.report.json").write_text(
            json.dumps({"bundles_covered": covered, "claims": [], "deferred": deferred or []})
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

    def test_a_malformed_triage_fails_instead_of_checking_nothing(self):
        for bad in (
            {k: v for k, v in builders.triage(deep=["B0001"]).items() if k != "deep"} | {"depe": ["B0001"]},
            builders.triage(deep=["B0001"], brief=["B0001"]),
        ):
            layout.work_triage_json(self.tmp).write_text(json.dumps(bad))
            payload = cv.run_check(self.tmp)
            self.assertFalse(payload["ok"])
            self.assertEqual(self.kinds(payload), ["invalid_triage"])

    def test_a_slice_bundle_outside_deep_is_a_violation(self):
        layout.work_triage_json(self.tmp).write_text(json.dumps(builders.triage(deep=["B0001", "B0002"])))
        self.report("deep", ["B0001", "B0002"], "Weapon A and Weapon B.")
        payload = cv.run_check(self.tmp)
        self.assertEqual(self.kinds(payload), ["slice_mismatch"])

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

    def test_malformed_report_is_a_violation(self):
        (layout.drafts_dir(self.tmp) / "deep.report.json").write_text(json.dumps({"bundles": 3}))
        (layout.drafts_dir(self.tmp) / "deep.md").write_text("Weapon A, Weapon B, Weapon C")
        payload = cv.run_check(self.tmp)
        self.assertIn("invalid_report", self.kinds(payload))

    def test_summary_requires_anchor_or_cut_with_reason(self):
        self.report("deep", ["B0001", "B0002", "B0003"], "Weapon A, Weapon B, Weapon C.")
        layout.patch_summary_md(self.tmp).write_text("# Patch\nWeapon A and Weapon B.\n")
        payload = cv.run_check(self.tmp, summary=True)
        self.assertEqual(self.kinds(payload), ["missing_from_summary"])
        layout.work_cuts_json(self.tmp).write_text(json.dumps({"cuts": [{"bundle_id": "B0003", "reason": ""}]}))
        # A cut with an empty reason is malformed, and the story stays missing.
        self.assertEqual(self.kinds(cv.run_check(self.tmp, summary=True)), ["invalid_cuts", "missing_from_summary"])
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
