#!/usr/bin/env python3
"""Tests for pn/render_comprehensive.py."""

import json
import tempfile
import unittest
from pathlib import Path

from pn import change_entries, schemas
from pn import render_comprehensive as rc
from tests.builders import load_fixture, run_pn

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
GOLDEN_DIR = FIXTURES_DIR / "golden"


# ---------------------------------------------------------------------------
# Excluded-type dropping + counts_excluded
# ---------------------------------------------------------------------------


def _excluded_types_diff():
    return {
        "added": [
            {"form_id": "0x00000001", "editor_id": "TestCellAdded", "record_type": "CELL", "offset": 1},
            {
                "form_id": "0x00000002", "editor_id": "TestWeapAdded", "record_type": "WEAP", "offset": 2,
                "fields": {"Data": {"Damage": 5}},
            },
        ],
        "removed": [
            {"form_id": "0x00000003", "editor_id": "TestWrldRemoved", "record_type": "WRLD", "offset": 3},
        ],
        "changed": [
            {
                "stub": {"form_id": "0x00000004", "editor_id": "TestCellChanged", "record_type": "CELL", "offset": 4},
                "field_changes": {"Foo": {"from": 1, "to": 2}},
            },
            {
                "stub": {"form_id": "0x00000005", "editor_id": "TestWeapChanged", "record_type": "WEAP", "offset": 5},
                "field_changes": {"Data": {"Damage": {"from": 1, "to": 2}}},
            },
        ],
        "ref_names": {},
    }


class TestExcludedTypes(unittest.TestCase):
    def setUp(self):
        self.comp = rc.build_comprehensive(_excluded_types_diff(), generated_at="X")

    def test_excluded_types_dropped_from_records(self):
        self.assertNotIn("0x00000001", self.comp["records"])  # CELL added
        self.assertNotIn("0x00000003", self.comp["records"])  # WRLD removed
        self.assertNotIn("0x00000004", self.comp["records"])  # CELL changed
        self.assertIn("0x00000002", self.comp["records"])
        self.assertIn("0x00000005", self.comp["records"])

    def test_counts_excluded_tally(self):
        self.assertEqual(self.comp["meta"]["counts_excluded"], {"CELL": 2, "WRLD": 1})

    def test_counts_reflect_only_included_records(self):
        self.assertEqual(self.comp["meta"]["counts"], {"added": 1, "removed": 0, "changed": 1})

    def test_excluded_types_meta_field(self):
        self.assertEqual(self.comp["meta"]["excluded_types"], sorted(change_entries.EXCLUDED_TYPES))

    def test_no_excluded_types_means_empty_counts_excluded(self):
        comp = rc.build_comprehensive({"added": [], "removed": [], "changed": [], "ref_names": {}}, generated_at="X")
        self.assertEqual(comp["meta"]["counts_excluded"], {})


# ---------------------------------------------------------------------------
# Unkeyed arrays (CTDA `Conditions[]`: position is semantic AND/OR chaining,
# so it has no element_key_spec entry) arrive from esm/src/diff/array_diff.rs
# as an `unkeyed` `_array_diff` strategy with whole element lists under
# `removed`/`added`.
# This is the round-trip test for that contract; the Rust side is
# `array_diff_unkeyed_ctda_conditions_length_mismatch` in `esm/tests/diff.rs`.
# ---------------------------------------------------------------------------


def _unkeyed_conditions_diff():
    return {
        "added": [], "removed": [],
        "changed": [{
            "stub": {
                "form_id": "0x00200001", "editor_id": "TestRecipeCond",
                "record_type": "COBJ", "offset": 1,
            },
            "field_changes": {
                "Conditions": {
                    "_array_diff": {
                        "strategy": "unkeyed",
                        "count_from": 1,
                        "count_to": 2,
                        "removed": [
                            {"Condition": {"Condition Data": {
                                "Function": "HasEntitlement", "Operator": "Equal To",
                                "Comparison Value": 1.0,
                            }}},
                        ],
                        "added": [
                            {"Condition": {"Condition Data": {
                                "Function": "HasEntitlement", "Operator": "Equal To",
                                "Comparison Value": 1.0,
                            }}},
                            {"Condition": {"Condition Data": {
                                "Function": "HasLearnedRecipe", "Operator": "Equal To",
                                "Comparison Value": 1.0,
                            }}},
                        ],
                    }
                }
            },
        }],
        "ref_names": {},
    }


class TestUnkeyedArrayShape(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.diff = _unkeyed_conditions_diff()
        cls.comp = rc.build_comprehensive(cls.diff, generated_at="X")

    def test_change_entry_carries_normalized_array_shape(self):
        changes = self.comp["records"]["0x00200001"]["changes"]
        self.assertEqual(len(changes), 1)
        arr = changes[0]["array"]
        self.assertEqual(arr["strategy"], "unkeyed")
        self.assertEqual(len(arr["removed"]), 1)
        self.assertEqual(len(arr["added"]), 2)



# ---------------------------------------------------------------------------
# refs_out population per status
# ---------------------------------------------------------------------------


class TestRefsOutPopulation(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.diff = load_fixture("diff_small.json")
        cls.comp = rc.build_comprehensive(cls.diff, generated_at="X")
        cls.records = cls.comp["records"]

    def test_added_record_refs_from_fields(self):
        refs = self.records["0x0100A001"]["refs_out"]
        formids = {r["formid"] for r in refs}
        self.assertEqual(formids, {"0x00050001", "0x00050002", "0x00050099"})

    def test_removed_record_refs_from_fields(self):
        refs = self.records["0x0100B001"]["refs_out"]
        formids = {r["formid"] for r in refs}
        self.assertEqual(formids, {"0x00050003"})

    def test_changed_record_refs_are_to_side_only(self):
        refs = self.records["0x01001001"]["refs_out"]
        formids = {r["formid"] for r in refs}
        # to-side (new state) only: the new Ammo + new Scope Attachment.
        # The OLD Ammo value (0x00123456) must NOT appear.
        self.assertEqual(formids, {"0x00654321", "0x00099999"})

    def test_flags_bitmask_not_mistaken_for_formid_ref(self):
        # Regression test: "Weapon Flags" to-value is
        # {"value": "0x00000005", "flags": [...]}. The 8-hex-digit bitmask
        # must NOT be harvested as a dangling FormID reference.
        refs = self.records["0x01001001"]["refs_out"]
        formids = {r["formid"] for r in refs}
        self.assertNotIn("0x00000005", formids)

    def test_changed_array_added_element_ref_included(self):
        refs = self.records["0x01003001"]["refs_out"]
        formids = {r["formid"] for r in refs}
        self.assertIn("0x00AA0004", formids)

    def test_changed_array_omod_no_formid_refs(self):
        # OMOD Properties diff has no FormID-shaped values at all.
        refs = self.records["0x01002001"]["refs_out"]
        self.assertEqual(refs, [])

    def test_no_fields_no_refs(self):
        # A changed record with no "fields" on its stub and no FormID-shaped
        # changes at all yields an empty refs_out.
        refs = self.records["0x01006001"]["refs_out"]
        self.assertEqual(refs, [])


class TestBuildComprehensiveConformance(unittest.TestCase):
    def test_build_comprehensive_records_satisfy_wire_contract(self):
        diff = load_fixture("diff_small.json")
        comp = rc.build_comprehensive(diff, generated_at="X")
        schemas.validate_comprehensive_payload(comp)
        for fid, record in comp["records"].items():
            with self.subTest(form_id=fid):
                schemas.validate_record_entry(record, path=f"records[{fid!r}]")


# ---------------------------------------------------------------------------
# Records whose every change is covered elsewhere stay in the JSON
# ---------------------------------------------------------------------------


def _drop_vs_keep_diff():
    # Use raw (_unmapped) blobs for the "fully covered" cases — top-level diff
    # noise is stripped in esm/src/diff/noise.rs, not re-suppressed here.
    raw_only = {
        "from": {"_raw": True, "hex": "aa"},
        "to": {"_raw": True, "hex": "bb"},
    }
    changed = [
        {  # (a) dropped: no cut, no rename, only a suppressed raw change.
            "stub": {"form_id": "0x02000001", "editor_id": "PlainRecord", "record_type": "STAT", "offset": 1},
            "field_changes": {"Unknown Blob": raw_only},
        },
        {  # (b) kept: cut-marked, but otherwise no renderable changes.
            "stub": {"form_id": "0x02000002", "editor_id": "zzz_CutRecord", "record_type": "STAT", "offset": 2},
            "field_changes": {"Unknown Blob": raw_only},
        },
        {  # (c) kept: renamed this patch, but otherwise no renderable changes.
            "stub": {"form_id": "0x02000003", "editor_id": "RenamedRecord", "record_type": "STAT", "offset": 3},
            "field_changes": {"Unknown Blob": raw_only},
            "prev_editor_id": "OldRenamedRecord",
        },
        {  # (d) kept: has a genuinely renderable change alongside a suppressed raw blob.
            "stub": {"form_id": "0x02000004", "editor_id": "RenderableRecord", "record_type": "STAT", "offset": 4},
            "field_changes": {
                "Unknown Blob": raw_only,
                "Data": {"Value": {"from": 1, "to": 2}},
            },
        },
    ]
    return {"added": [], "removed": [], "changed": changed, "ref_names": {}}


class TestFullyCoveredRecordKept(unittest.TestCase):
    def test_fully_covered_record_stays_in_json(self):
        comp = rc.build_comprehensive(_drop_vs_keep_diff(), generated_at="X")
        self.assertIn("0x02000001", comp["records"])


# ---------------------------------------------------------------------------
# meta.old_esm / new_esm absolute-path resolution
# ---------------------------------------------------------------------------


class TestMetaCarriesNoPaths(unittest.TestCase):
    def test_esm_paths_stay_out_of_the_artifact(self):
        comp = rc.build_comprehensive(
            {"added": [], "removed": [], "changed": [], "ref_names": {}},
            old_esm="/fake/old/Game.esm", new_esm="/fake/new/Game.esm", generated_at="X",
        )
        self.assertNotIn("/fake/", json.dumps(comp))


# ---------------------------------------------------------------------------
# CLI: arg parsing + label/date derivation
# ---------------------------------------------------------------------------


class TestLabelDerivation(unittest.TestCase):
    def test_defaults_when_nothing_given(self):
        old, new, date = rc.derive_labels_and_date("diff.json", None, None, None, None, None)
        self.assertEqual(old, "old")
        self.assertEqual(new, "new")
        self.assertEqual(date, "Unknown Date")

    def test_labels_from_esm_basenames_when_filename_itself_is_dated(self):
        old, new, date = rc.derive_labels_and_date(
            "diff.json", "/data/old/Game_20260626.esm", "/data/new/Game_20260703.esm", None, None, None,
        )
        self.assertEqual(old, "Game_20260626.esm")
        self.assertEqual(new, "Game_20260703.esm")
        self.assertEqual(date, "2026-07-03")

    def test_patch_date_derived_from_new_label_date_token(self):
        _, _, date = rc.derive_labels_and_date("diff.json", None, None, "20260626", "Game_20260703", None)
        self.assertEqual(date, "2026-07-03")

    def test_patch_date_falls_back_to_old_label(self):
        _, _, date = rc.derive_labels_and_date("diff.json", None, None, "Game_20260626", "no-date-here", None)
        self.assertEqual(date, "2026-06-26")

    def test_patch_date_falls_back_to_diff_json_filename(self):
        _, _, date = rc.derive_labels_and_date("/tmp/diff_20260703.json", None, None, "old-label", "new-label", None)
        self.assertEqual(date, "2026-07-03")

    def test_patch_date_unknown_when_nothing_carries_a_date(self):
        _, _, date = rc.derive_labels_and_date("diff.json", None, None, "old-label", "new-label", None)
        self.assertEqual(date, "Unknown Date")

    def test_explicit_patch_date_wins_over_derivation(self):
        _, _, date = rc.derive_labels_and_date("diff.json", None, None, None, "Game_20260703", "2099-01-01")
        self.assertEqual(date, "2099-01-01")

    def test_explicit_labels_win_over_esm_basenames(self):
        old, new, _ = rc.derive_labels_and_date(
            "diff.json", "/data/old/Game.esm", "/data/new/Game.esm", "Old Label", "New Label", None,
        )
        self.assertEqual(old, "Old Label")
        self.assertEqual(new, "New Label")

    def test_labels_prefer_dated_parent_dir_when_filename_is_undated(self):
        # This pipeline's real snapshot layout: <root>/<date>/SeventySix.esm
        # — the filename itself never carries a date, so the sibling
        # directory name (otherwise identical "SeventySix.esm" on both
        # sides would be useless as a label) should be preferred.
        old, new, date = rc.derive_labels_and_date(
            "diff.json",
            "/data/20260626/SeventySix.esm", "/data/20260703/SeventySix.esm",
            None, None, None,
        )
        self.assertEqual(old, "20260626")
        self.assertEqual(new, "20260703")
        self.assertEqual(date, "2026-07-03")

    def test_patch_date_falls_back_to_esm_parent_dir_when_label_overridden(self):
        # A custom --new-label shouldn't prevent patch-date auto-derivation
        # from the actual snapshot directory name.
        _, _, date = rc.derive_labels_and_date(
            "diff.json",
            "/data/20260626/SeventySix.esm", "/data/20260703/SeventySix.esm",
            None, "Public Test Server Build", None,
        )
        self.assertEqual(date, "2026-07-03")


class TestCliArgParsing(unittest.TestCase):
    def test_common_threshold_default_and_type(self):
        args = rc.build_arg_parser().parse_args(["diff.json", "--out-dir", "out"])
        self.assertEqual(args.common_threshold, change_entries.DEFAULT_COMMON_THRESHOLD)
        self.assertIsInstance(args.common_threshold, int)

    def test_common_threshold_override(self):
        args = rc.build_arg_parser().parse_args(["diff.json", "--out-dir", "out", "--common-threshold", "3"])
        self.assertEqual(args.common_threshold, 3)

    def test_out_dir_required(self):
        with self.assertRaises(SystemExit):
            rc.build_arg_parser().parse_args(["diff.json"])

    def test_all_meta_flags_parsed(self):
        args = rc.build_arg_parser().parse_args([
            "diff.json", "--out-dir", "out",
            "--old-esm", "/a.esm", "--new-esm", "/b.esm",
            "--old-label", "A", "--new-label", "B", "--patch-date", "2026-01-01",
        ])
        self.assertEqual(args.old_esm, "/a.esm")
        self.assertEqual(args.new_esm, "/b.esm")
        self.assertEqual(args.old_label, "A")
        self.assertEqual(args.new_label, "B")
        self.assertEqual(args.patch_date, "2026-01-01")


class TestCliEndToEnd(unittest.TestCase):
    def test_main_writes_json_and_summary(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            diff_path = tmp / "diff.json"
            diff_path.write_text(json.dumps(load_fixture("diff_small.json")), encoding="utf-8")
            out_dir = tmp / "out"
            result = run_pn(
                "render", str(diff_path), "--out-dir", str(out_dir),
                "--old-label", "20260626", "--new-label", "20260703", "--patch-date", "2026-07-03",
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((out_dir / "comprehensive.json").exists())
            self.assertFalse((out_dir / "comprehensive.md").exists())
            self.assertIn("added", result.stderr)
            comp = json.loads((out_dir / "comprehensive.json").read_text(encoding="utf-8"))
            self.assertEqual(comp["meta"]["patch_date"], "2026-07-03")

    def test_main_derives_labels_and_date_when_omitted(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            diff_path = tmp / "diff_20260703.json"
            diff_path.write_text(json.dumps(load_fixture("diff_small.json")), encoding="utf-8")
            out_dir = tmp / "out"
            result = run_pn("render", str(diff_path), "--out-dir", str(out_dir))
            self.assertEqual(result.returncode, 0, result.stderr)
            comp = json.loads((out_dir / "comprehensive.json").read_text(encoding="utf-8"))
            self.assertEqual(comp["meta"]["old_label"], "old")
            self.assertEqual(comp["meta"]["new_label"], "new")
            self.assertEqual(comp["meta"]["patch_date"], "2026-07-03")  # derived from diff json's own filename

    def test_main_reports_error_on_missing_diff_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp) / "out"
            result = run_pn("render", str(Path(tmp) / "does_not_exist.json"), "--out-dir", str(out_dir))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("error", result.stderr)


# ---------------------------------------------------------------------------
# JSON schema keys present
# ---------------------------------------------------------------------------


class TestJsonSchemaKeys(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.comp = rc.build_comprehensive(load_fixture("diff_small.json"), generated_at="X")

    def test_top_level_keys(self):
        self.assertEqual(set(self.comp.keys()), {"meta", "records", "common_changes", "ref_names"})

    def test_meta_keys(self):
        self.assertEqual(
            set(self.comp["meta"].keys()),
            {
                "old_label", "new_label", "patch_date", "generated_at",
                "excluded_types", "counts_excluded", "suppressed_counts", "counts",
            },
        )

    def test_record_entry_keys(self):
        rec = self.comp["records"]["0x01001001"]
        self.assertEqual(
            set(rec.keys()),
            {
                "form_id", "record_type", "editor_id", "name", "description", "status",
                "prev_editor_id", "cut", "fields", "refs_out", "dangling_refs", "changes",
            },
        )

    def test_ref_names_carry_diff_names_plus_dangling_refs(self):
        diff = load_fixture("diff_small.json")
        expected = dict(diff["ref_names"])
        expected["0x00050099"] = {"dangling": True}
        expected["0x00099999"] = {"dangling": True}
        self.assertEqual(self.comp["ref_names"], expected)


# ---------------------------------------------------------------------------
# Golden fixture regression test
# ---------------------------------------------------------------------------


class TestGoldenComprehensive(unittest.TestCase):
    """
    comprehensive_small.json under tests/fixtures/golden/ were
    generated from diff_small.json with the fixed args used below
    (old_label="20260626", new_label="20260703", patch_date="2026-07-03",
    a pinned generated_at sentinel, and the default common_threshold=5),
    then manually reviewed (see the task's final report for what was
    checked) before being committed as the expected output.

    generated_at is pinned via build_comprehensive()'s optional override
    (rather than normalized post-hoc) so this test needs no placeholder
    substitution and is fully deterministic; likewise --old-esm/--new-esm
    are omitted (left "") so no machine-dependent absolute path appears in
    the golden either. Absolute-path resolution has its own dedicated unit
    test (TestMetaEsmPaths) instead.
    """

    GENERATED_AT = "2026-07-03T00:00:00Z"

    @classmethod
    def setUpClass(cls):
        diff = load_fixture("diff_small.json")
        cls.comp = rc.build_comprehensive(
            diff,
            old_label="20260626", new_label="20260703", patch_date="2026-07-03",
            generated_at=cls.GENERATED_AT,
        )

    def test_json_matches_golden(self):
        with open(GOLDEN_DIR / "comprehensive_small.json", encoding="utf-8") as f:
            golden = json.load(f)
        self.assertEqual(self.comp, golden)



if __name__ == "__main__":
    unittest.main()
