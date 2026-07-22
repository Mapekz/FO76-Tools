#!/usr/bin/env python3
"""Tests for tools/render_comprehensive.py."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import patchnotes_lib as pl  # noqa: E402
import render_comprehensive as rc  # noqa: E402

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
GOLDEN_DIR = FIXTURES_DIR / "golden"
SCRIPT = Path(__file__).resolve().parents[1] / "render_comprehensive.py"


def load_fixture(name):
    with open(FIXTURES_DIR / name, encoding="utf-8") as f:
        return json.load(f)


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
        self.assertEqual(self.comp["meta"]["excluded_types"], sorted(pl.EXCLUDED_TYPES))

    def test_no_excluded_types_means_empty_counts_excluded(self):
        comp = rc.build_comprehensive({"added": [], "removed": [], "changed": [], "ref_names": {}}, generated_at="X")
        self.assertEqual(comp["meta"]["counts_excluded"], {})


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
        pl.validate_comprehensive_payload(comp)
        for fid, record in comp["records"].items():
            with self.subTest(form_id=fid):
                pl.validate_record_entry(record, path=f"records[{fid!r}]")


# ---------------------------------------------------------------------------
# render_fields: nesting / depth-cap / list rendering
# ---------------------------------------------------------------------------


class TestRenderFields(unittest.TestCase):
    def test_flat_dict(self):
        lines = rc.render_fields({"Damage": 25, "Value": 150}, {})
        self.assertEqual(lines, ["- **Damage:** `25`", "- **Value:** `150`"])

    def test_nested_dict_indents(self):
        lines = rc.render_fields({"Data": {"Damage": 25}}, {})
        self.assertEqual(lines, ["- **Data:**", "  - **Damage:** `25`"])

    def test_list_of_scalars_indexed(self):
        lines = rc.render_fields(["0x00000001", "0x00000002"], {})
        self.assertEqual(lines[0], "- [0] `0x00000001`")
        self.assertEqual(lines[1], "- [1] `0x00000002`")

    def test_list_of_structs_recurse_with_index(self):
        lines = rc.render_fields([{"Reference": "0x00000001", "Quantity": 2}], {})
        self.assertEqual(lines[0], "- [0]")
        self.assertIn("  - **Reference:** `0x00000001`", lines)
        self.assertIn("  - **Quantity:** `2`", lines)

    def test_empty_dict_and_list(self):
        self.assertEqual(rc.render_fields({}, {}), ["- *(empty)*"])
        self.assertEqual(rc.render_fields([], {}), ["- *(empty list)*"])

    def test_empty_nested_dict_and_list_values(self):
        lines = rc.render_fields({"Sub": {}, "Items": []}, {})
        self.assertIn("- **Sub:** *(empty)*", lines)
        self.assertIn("- **Items:** *(empty list)*", lines)

    def test_depth_cap_terminates_pathological_nesting(self):
        # 10 levels deep; rendering must terminate with an ellipsis marker
        # rather than recursing without bound.
        deep = 1
        for _ in range(10):
            deep = {"a": deep}
        lines = rc.render_fields(deep, {})
        self.assertTrue(any("…" in line for line in lines))
        a_headers = [line for line in lines if line.strip() == "- **a:**"]
        self.assertLessEqual(len(a_headers), rc.MAX_RENDER_DEPTH + 1)

    def test_shallow_nesting_not_truncated(self):
        # Well within the cap: every level must render, no "…" marker.
        shallow = 1
        for _ in range(3):
            shallow = {"a": shallow}
        lines = rc.render_fields(shallow, {})
        self.assertFalse(any("…" in line for line in lines))

    def test_enum_flags_curve_unresolved_render_as_leaves_not_recursed(self):
        fields = {
            "Firing Type": {"value": 1, "name": "Burst"},
            "Weapon Flags": {"value": "0x00000001", "flags": ["Automatic"]},
            "Some Curve": {"formid": "0x00000009", "curve_path": "x", "curve": [{"x": 0, "y": 1}]},
            "Unresolved Text": {"_unresolved": True, "lstring_id": 42},
        }
        lines = rc.render_fields(fields, {})
        joined = "\n".join(lines)
        self.assertIn("**Firing Type:** `Burst`", joined)
        self.assertIn("Automatic", joined)
        self.assertIn("**Some Curve:** `0x00000009`", joined)
        self.assertIn("unresolved", joined)
        self.assertNotIn("curve_path", joined)  # never recursed into

    def test_formid_ref_resolved_via_ref_names(self):
        ref_names = {"0x00000001": {"record_type": "KYWD", "editor_id": "SomeKeyword"}}
        lines = rc.render_fields({"Keyword": "0x00000001"}, ref_names)
        self.assertIn("SomeKeyword", lines[0])


# ---------------------------------------------------------------------------
# MD drop-vs-keep rule for "fully covered" changed records
# ---------------------------------------------------------------------------


def _drop_vs_keep_diff():
    # Use raw (_unmapped) blobs for the "fully covered" cases — top-level diff
    # noise is stripped in esm/src/diff.rs, not re-suppressed here.
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


class TestMdDropVsKeepRule(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.comp = rc.build_comprehensive(_drop_vs_keep_diff(), generated_at="X")
        cls.md = rc.render_markdown(cls.comp)

    def test_fully_covered_record_dropped_from_md(self):
        self.assertNotIn("PlainRecord", self.md)

    def test_fully_covered_record_stays_in_json(self):
        self.assertIn("0x02000001", self.comp["records"])

    def test_fully_covered_note_counts_exactly_the_dropped_record(self):
        self.assertIn("*(+1 records fully covered by Common Changes or suppressed noise)*", self.md)

    def test_cut_record_with_no_renderable_changes_still_shown(self):
        self.assertIn("zzz_CutRecord", self.md)

    def test_renamed_record_with_no_renderable_changes_still_shown(self):
        self.assertIn("RenamedRecord", self.md)

    def test_covered_elsewhere_marker_line_present(self):
        self.assertIn("*(all changes covered by Common Changes / suppressed noise)*", self.md)

    def test_record_with_a_real_change_renders_that_change(self):
        self.assertIn("RenderableRecord", self.md)
        self.assertIn("Data / Value", self.md)


# ---------------------------------------------------------------------------
# _record_heading_line fallback logic
# ---------------------------------------------------------------------------


class TestRecordHeadingFallback(unittest.TestCase):
    def test_name_and_edid(self):
        rec = {"name": "Foo", "editor_id": "FooEdid", "form_id": "0x01", "cut": None, "prev_editor_id": None}
        self.assertEqual(rc._record_heading_line(rec), "**Foo** `FooEdid` `0x01`")

    def test_edid_only(self):
        rec = {"name": None, "editor_id": "FooEdid", "form_id": "0x01", "cut": None, "prev_editor_id": None}
        self.assertEqual(rc._record_heading_line(rec), "**FooEdid** `0x01`")

    def test_neither_name_nor_edid(self):
        rec = {"name": None, "editor_id": None, "form_id": "0x01", "cut": None, "prev_editor_id": None}
        self.assertEqual(rc._record_heading_line(rec), "`0x01`")

    def test_cut_annotation_appended(self):
        rec = {
            "name": None, "editor_id": "zzz_Foo", "form_id": "0x01", "prev_editor_id": None,
            "cut": {"marker": "ZZZ", "confidence": "high", "kind": "added_cut"},
        }
        self.assertIn("cut: ZZZ, high confidence", rc._record_heading_line(rec))


# ---------------------------------------------------------------------------
# Cut section bucketing + common-changes / VMAD rendering (structural checks)
# ---------------------------------------------------------------------------


class TestCutSectionAndRenderingStructure(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.comp = rc.build_comprehensive(load_fixture("diff_small.json"), generated_at="X")
        cls.md = rc.render_markdown(cls.comp)

    def test_newly_deprecated_section_present_with_rename(self):
        self.assertIn("### Newly Deprecated This Patch", self.md)
        self.assertIn("`TestPerkRank03` → `zzz_TestPerkRank03`", self.md)

    def test_empty_cut_subsections_omitted(self):
        self.assertNotIn("### Added Already-Cut", self.md)
        self.assertNotIn("### Still-Cut Changed", self.md)
        self.assertNotIn("### Removed Previously-Cut", self.md)

    def test_common_changes_block_present(self):
        self.assertIn("**Common Changes:**", self.md)
        self.assertIn("CC001", self.md)
        self.assertIn("Stats / Confidence", self.md)

    def test_common_change_lists_all_six_members(self):
        for i in range(1, 7):
            self.assertIn(f"TestNPC0{i}", self.md)

    def test_individual_npc_records_still_show_uncollapsed_aggression(self):
        self.assertIn("Stats / Aggression", self.md)

    def test_vmad_section_rendered(self):
        self.assertIn("Script Properties (VMAD)", self.md)
        self.assertIn("`Count`: `3` → `5`", self.md)
        self.assertIn("`Flag`: `false` → `true`", self.md)

    def test_array_diff_bullets_added_removed_changed(self):
        self.assertIn("**+**", self.md)
        self.assertIn("**−**", self.md)
        self.assertIn("**~**", self.md)

    def test_object_bounds_rendered_when_present(self):
        # diff_small.json includes Object Bounds (as --keep-noise would surface them).
        self.assertIn("Object Bounds / X1", self.md)


# ---------------------------------------------------------------------------
# meta.old_esm / new_esm absolute-path resolution
# ---------------------------------------------------------------------------


class TestMetaEsmPaths(unittest.TestCase):
    def test_paths_resolved_absolute(self):
        comp = rc.build_comprehensive(
            {"added": [], "removed": [], "changed": [], "ref_names": {}},
            old_esm="/fake/old/Game.esm", new_esm="/fake/new/Game.esm", generated_at="X",
        )
        self.assertEqual(comp["meta"]["old_esm"], str(Path("/fake/old/Game.esm").resolve()))
        self.assertEqual(comp["meta"]["new_esm"], str(Path("/fake/new/Game.esm").resolve()))

    def test_blank_when_not_given(self):
        comp = rc.build_comprehensive(
            {"added": [], "removed": [], "changed": [], "ref_names": {}}, generated_at="X"
        )
        self.assertEqual(comp["meta"]["old_esm"], "")
        self.assertEqual(comp["meta"]["new_esm"], "")


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
        self.assertEqual(args.common_threshold, pl.DEFAULT_COMMON_THRESHOLD)
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
    def test_main_writes_both_files_and_summary(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            diff_path = tmp / "diff.json"
            diff_path.write_text(json.dumps(load_fixture("diff_small.json")), encoding="utf-8")
            out_dir = tmp / "out"
            result = subprocess.run(
                [
                    sys.executable, str(SCRIPT), str(diff_path), "--out-dir", str(out_dir),
                    "--old-label", "20260626", "--new-label", "20260703", "--patch-date", "2026-07-03",
                ],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((out_dir / "comprehensive.json").exists())
            self.assertTrue((out_dir / "comprehensive.md").exists())
            self.assertIn("added", result.stderr)
            comp = json.loads((out_dir / "comprehensive.json").read_text(encoding="utf-8"))
            self.assertEqual(comp["schema_version"], pl.SCHEMA_VERSION)
            self.assertEqual(comp["meta"]["patch_date"], "2026-07-03")

    def test_main_derives_labels_and_date_when_omitted(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            diff_path = tmp / "diff_20260703.json"
            diff_path.write_text(json.dumps(load_fixture("diff_small.json")), encoding="utf-8")
            out_dir = tmp / "out"
            result = subprocess.run(
                [sys.executable, str(SCRIPT), str(diff_path), "--out-dir", str(out_dir)],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            comp = json.loads((out_dir / "comprehensive.json").read_text(encoding="utf-8"))
            self.assertEqual(comp["meta"]["old_label"], "old")
            self.assertEqual(comp["meta"]["new_label"], "new")
            self.assertEqual(comp["meta"]["patch_date"], "2026-07-03")  # derived from diff json's own filename

    def test_main_reports_error_on_missing_diff_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp) / "out"
            result = subprocess.run(
                [sys.executable, str(SCRIPT), str(Path(tmp) / "does_not_exist.json"), "--out-dir", str(out_dir)],
                capture_output=True, text=True,
            )
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
        self.assertEqual(set(self.comp.keys()), {"schema_version", "meta", "records", "common_changes", "ref_names"})

    def test_meta_keys(self):
        self.assertEqual(
            set(self.comp["meta"].keys()),
            {
                "old_esm", "new_esm", "old_label", "new_label", "patch_date", "generated_at",
                "excluded_types", "counts_excluded", "suppressed_counts", "counts",
            },
        )

    def test_record_entry_keys(self):
        rec = self.comp["records"]["0x01001001"]
        self.assertEqual(
            set(rec.keys()),
            {
                "form_id", "record_type", "editor_id", "name", "description", "status",
                "prev_editor_id", "cut", "fields", "refs_out", "changes",
            },
        )

    def test_schema_version_matches_library_constant(self):
        self.assertEqual(self.comp["schema_version"], pl.SCHEMA_VERSION)

    def test_ref_names_passthrough_verbatim(self):
        diff = load_fixture("diff_small.json")
        self.assertEqual(self.comp["ref_names"], diff["ref_names"])


# ---------------------------------------------------------------------------
# Golden fixture regression test
# ---------------------------------------------------------------------------


class TestGoldenComprehensive(unittest.TestCase):
    """
    comprehensive_small.json/.md under tools/tests/fixtures/golden/ were
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
        cls.md = rc.render_markdown(cls.comp)

    def test_json_matches_golden(self):
        with open(GOLDEN_DIR / "comprehensive_small.json", encoding="utf-8") as f:
            golden = json.load(f)
        self.assertEqual(self.comp, golden)

    def test_md_matches_golden(self):
        golden_md = (GOLDEN_DIR / "comprehensive_small.md").read_text(encoding="utf-8")
        self.assertEqual(self.md, golden_md)


if __name__ == "__main__":
    unittest.main()
