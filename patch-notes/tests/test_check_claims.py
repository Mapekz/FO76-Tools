#!/usr/bin/env python3
"""Tests for pn/check_claims.py.

Synthetic comprehensive.json + deep-writer reports in a temp out-dir; the
live-esm fallback is exercised through a tiny in-memory gateway stub
(the same `bulk_get(esm, sels, resolve=)` surface as `EsmGateway`).
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

from pn import check_claims as cc
from pn import layout
from tests import builders
from tests.builders import TempDirTestCase


def change(path, from_, to, kind="scalar", **extra):
    # Unlike the other suites' change builders this one fills in the backtick
    # `from_display`/`to_display` renderings, because check_claims reads the
    # displayed value when matching a drafted number back to the diff.
    return builders.change(**{
        "path": path, "kind": kind, "from": from_, "to": to,
        "from_display": f"`{from_}`", "to_display": f"`{to}`",
        **extra,
    })


def record(fid, rtype, status="changed", editor_id=None, name=None, changes=None, fields=None, prev=None):
    return builders.record(
        form_id=fid, record_type=rtype, editor_id=editor_id, name=name,
        status=status, prev_editor_id=prev, fields=fields, changes=changes or [],
    )


EFFECTS_ARRAY = {
    "strategy": "keyed", "key_fields": ["Effect"], "count_from": 2, "count_to": 2,
    "added": [], "removed": [],
    "changed": [
        {"key_display": "Effect=0x00000010", "changes": [change("Magnitude", 10.0, 15.0)]},
    ],
}
KEYWORDS_ARRAY = {
    "strategy": "set", "key_fields": None, "count_from": 3, "count_to": 4,
    "added": [{"key_display": "0x00000030", "display": "0x00000030", "raw": "0x00000030"}],
    "removed": [], "changed": [],
}

RECORDS = {
    "0x00000001": record(
        "0x00000001", "WEAP", editor_id="WeapA", name="Weapon A",
        changes=[
            change("Data / Damage", 20, 25),
            change("Effects", [], [], kind="array", array=EFFECTS_ARRAY),
            change("Keywords", [], [], kind="array", array=KEYWORDS_ARRAY),
        ],
    ),
    "0x00000002": record(
        "0x00000002", "ARMO", status="added", editor_id="ArmoB", name="Armor B",
        fields={"Data": {"Value": 100, "Weight": 2.5}, "Keywords": ["0x00000020"]},
    ),
    "0x00000003": record(
        "0x00000003", "MISC", editor_id="NewName", prev="OldName", name="Misc C",
        changes=[change("Full Name", "a", "b", kind="string")],
    ),
}


class StubGateway:
    """`bulk_get` over a per-ESM fixture: {esm_path: {selector: fields}}."""

    def __init__(self, data):
        self.data = data
        self.calls = []

    def bulk_get(self, esm, sels, *, resolve="stub"):
        out = []
        for sel in sels:
            self.calls.append((esm, sel))
            fields = self.data.get(esm, {}).get(sel)
            if fields is None:
                out.append({"sel": sel, "error": f"not found: {sel}"})
            else:
                out.append({"sel": sel, "header": {"form_id": sel}, "editor_id": sel, "fields": fields})
        return out


def write_out_dir(tmp: Path, claims, draft="", report_name="deep.report.json"):
    tmp.joinpath("comprehensive.json").write_text(json.dumps({"records": RECORDS, "ref_names": {}}))
    drafts = layout.drafts_dir(tmp)
    drafts.mkdir(exist_ok=True)
    (drafts / report_name).write_text(json.dumps({"bundles": 1, "claims": claims}))
    (drafts / report_name.replace(".report.json", ".md")).write_text(draft)


class TestVerifyClaim(unittest.TestCase):
    def setUp(self):
        self.index = cc.RecordIndex(RECORDS)
        self.no_live = cc.LiveLookup(None, None, None)

    def verify(self, claim, live=None):
        return cc.verify_claim(claim, self.index, live or self.no_live)

    def test_scalar_change_ok(self):
        r = self.verify({"record": "0x00000001", "path": "Data / Damage", "from": 20, "to": 25})
        self.assertEqual((r["status"], r["source"]), ("ok", "changes"))

    def test_numeric_tolerance_and_string_numbers(self):
        r = self.verify({"record": "0x00000001", "path": "Data / Damage", "from": "20", "to": 25.0000001})
        self.assertEqual(r["status"], "ok")

    def test_wrong_value_is_mismatch(self):
        r = self.verify({"record": "0x00000001", "path": "Data / Damage", "from": 20, "to": 30})
        self.assertEqual(r["status"], "mismatch")

    def test_reversed_direction_is_mismatch_and_says_so(self):
        r = self.verify({"record": "0x00000001", "path": "Data / Damage", "from": 25, "to": 20})
        self.assertEqual(r["status"], "mismatch")
        self.assertIn("reversed", r["detail"])

    def test_editor_id_selector_and_array_row_by_key_display(self):
        r = self.verify({"record": "WeapA", "path": "Effects / [Effect=0x00000010] / Magnitude", "from": 10, "to": 15})
        self.assertEqual(r["status"], "ok")

    def test_array_row_key_is_whitespace_and_case_insensitive(self):
        r = self.verify({"record": "WeapA", "path": "Effects / [ effect=0X00000010 ] / Magnitude", "from": 10, "to": 15})
        self.assertEqual(r["status"], "ok")

    def test_array_entry_itself_compares_counts(self):
        ok = self.verify({"record": "0x00000001", "path": "Effects", "from": 2, "to": 2})
        bad = self.verify({"record": "0x00000001", "path": "Effects", "from": 1, "to": 2})
        self.assertEqual((ok["status"], bad["status"]), ("ok", "mismatch"))

    def test_unknown_row_without_esm_is_unverifiable(self):
        r = self.verify({"record": "0x00000001", "path": "Keywords / [0x00000099]", "from": 1, "to": 2})
        self.assertEqual(r["status"], "unverifiable")
        self.assertIn("no changed row", r["detail"])

    def test_existence_claims(self):
        self.assertEqual(self.verify({"record": "0x00000002", "status": "added"})["status"], "ok")
        self.assertEqual(self.verify({"record": "0x00000001", "status": "added"})["status"], "mismatch")
        self.assertEqual(self.verify({"record": "0x00000099", "status": "removed"})["status"], "unverifiable")

    def test_prior_value_on_added_record_is_mismatch(self):
        r = self.verify({"record": "0x00000002", "path": "Data / Value", "from": 50, "to": 100})
        self.assertEqual(r["status"], "mismatch")
        self.assertIn("no prior value", r["detail"])

    def test_value_claim_from_comprehensive_fields(self):
        ok = self.verify({"record": "ArmoB", "path": "Data / Weight", "value": 2.5})
        bad = self.verify({"record": "ArmoB", "path": "Data / Weight", "value": 3})
        self.assertEqual((ok["status"], ok["source"], bad["status"]), ("ok", "changes", "mismatch"))

    def test_value_claim_on_old_side_needs_esm(self):
        r = self.verify({"record": "NewName", "path": "Data / Value", "value": 7, "side": "old"})
        self.assertEqual((r["status"], r["source"]), ("unverifiable", "none"))

    def test_old_side_lookup_uses_previous_editor_id(self):
        gw = StubGateway({"old.esm": {"OldName": {"Data": {"Value": 7}}}, "new.esm": {"NewName": {"Data": {"Value": 9}}}})
        live = cc.LiveLookup(gw, "old.esm", "new.esm")
        r = self.verify({"record": "NewName", "path": "Data / Value", "value": 7, "side": "old"}, live)
        self.assertEqual((r["status"], r["source"]), ("ok", "esm"))
        self.assertIn(("old.esm", "OldName"), gw.calls)

    def test_changed_claim_not_in_changes_falls_back_to_both_sides(self):
        gw = StubGateway({"old.esm": {"OldName": {"Data": {"Value": 7}}}, "new.esm": {"NewName": {"Data": {"Value": 9}}}})
        live = cc.LiveLookup(gw, "old.esm", "new.esm")
        ok = self.verify({"record": "NewName", "path": "Data / Value", "from": 7, "to": 9}, live)
        self.assertEqual((ok["status"], ok["source"]), ("ok", "esm"))
        same = StubGateway({"old.esm": {"OldName": {"Data": {"Value": 9}}}, "new.esm": {"NewName": {"Data": {"Value": 9}}}})
        r = self.verify({"record": "NewName", "path": "Data / Value", "from": 7, "to": 9}, cc.LiveLookup(same, "old.esm", "new.esm"))
        self.assertEqual(r["status"], "mismatch")
        self.assertIn("did not change", r["detail"])

    def test_formid_values_match_stub_or_hex(self):
        self.assertTrue(cc.values_match("0x10", {"form_id": "0x00000010", "editor_id": "Foo"}))
        self.assertTrue(cc.values_match("Foo", {"form_id": "0x00000010", "editor_id": "Foo"}))
        self.assertFalse(cc.values_match("0x11", "0x00000010"))

    def test_malformed_claims_are_unverifiable(self):
        self.assertEqual(self.verify({"path": "x"})["status"], "unverifiable")
        self.assertEqual(self.verify({"record": "0x00000001", "path": "Data / Damage", "from": 20})["status"], "unverifiable")
        self.assertEqual(self.verify({"record": "0x00000001", "path": "Data / Damage"})["status"], "unverifiable")


class TestUnbackedNumbers(unittest.TestCase):
    def test_backed_numbers_are_skipped_including_percent_and_thousands(self):
        claims = [{"record": "x", "path": "p", "from": 1200, "to": 25}]
        draft = "Damage 1,200 → 25 (25%).\nEvidence: 0x00000001 has 77.\n`code 555`\nAlso 999 unbacked, 3 items."
        self.assertEqual(cc.find_unbacked_numbers(draft, claims), ["999"])

    def test_headings_and_evidence_lines_are_ignored(self):
        self.assertEqual(cc.find_unbacked_numbers("## Patch 20260912\nEvidence: 4242\n", []), [])


class TestRunCheck(TempDirTestCase):

    def test_all_ok_writes_payload_and_passes(self):
        write_out_dir(self.tmp, [{"record": "0x00000001", "path": "Data / Damage", "from": 20, "to": 25}],
                      draft="Damage goes 20 → 25. Then 999 appears.")
        payload = cc.run_check(self.tmp)
        self.assertTrue(payload["ok"])
        self.assertEqual(payload["reports"][0]["unbacked_numbers"], ["999"])
        self.assertTrue(layout.work_claims_check_json(self.tmp).is_file())
        self.assertEqual(cc.main([str(self.tmp), "--no-esm"]), 0)

    def test_mismatch_fails_the_gate(self):
        write_out_dir(self.tmp, [{"record": "0x00000001", "path": "Data / Damage", "from": 20, "to": 99}])
        self.assertFalse(cc.run_check(self.tmp)["ok"])
        self.assertEqual(cc.main([str(self.tmp), "--no-esm"]), 1)

    def test_unverifiable_fails_the_gate(self):
        write_out_dir(self.tmp, [{"record": "NewName", "path": "Data / Value", "value": 7, "side": "old"}])
        self.assertEqual(cc.main([str(self.tmp), "--no-esm"]), 1)

    def test_no_reports_is_not_ok(self):
        self.tmp.joinpath("comprehensive.json").write_text(json.dumps({"records": RECORDS, "ref_names": {}}))
        self.assertFalse(cc.run_check(self.tmp)["ok"])

    def test_part_reports_are_all_checked(self):
        write_out_dir(self.tmp, [{"record": "0x00000001", "path": "Data / Damage", "from": 20, "to": 25}], report_name="deep.part1.report.json")
        drafts = layout.drafts_dir(self.tmp)
        (drafts / "deep.part2.report.json").write_text(json.dumps({"claims": [{"record": "0x00000002", "status": "added"}]}))
        (drafts / "deep.part2.md").write_text("Armor B is new.")
        payload = cc.run_check(self.tmp)
        self.assertEqual([r["report"] for r in payload["reports"]], ["deep.part1.report.json", "deep.part2.report.json"])
        self.assertTrue(payload["ok"])


if __name__ == "__main__":
    unittest.main()
