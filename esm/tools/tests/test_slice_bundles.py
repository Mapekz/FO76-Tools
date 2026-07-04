#!/usr/bin/env python3
"""Tests for tools/slice_bundles.py."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import slice_bundles as sb  # noqa: E402

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
SCRIPT_PATH = Path(__file__).resolve().parents[1] / "slice_bundles.py"


def load_fixture(name):
    with open(FIXTURES_DIR / name, "r", encoding="utf-8") as f:
        return json.load(f)


class TempOutDir:
    """Context manager: a temp dir with bundles.json (and optionally
    comprehensive.json) already written, mirroring the pipeline's output
    directory layout."""

    def __init__(self, bundles_data=None, comprehensive_data=None):
        self.bundles_data = bundles_data
        self.comprehensive_data = comprehensive_data
        self._tmp = None

    def __enter__(self):
        self._tmp = tempfile.TemporaryDirectory()
        out_dir = Path(self._tmp.name)
        if self.bundles_data is not None:
            (out_dir / "bundles.json").write_text(
                json.dumps(self.bundles_data), encoding="utf-8"
            )
        if self.comprehensive_data is not None:
            (out_dir / "comprehensive.json").write_text(
                json.dumps(self.comprehensive_data), encoding="utf-8"
            )
        return out_dir

    def __exit__(self, *exc):
        self._tmp.cleanup()


def make_padded_bundle(bundle_id, category, category_label, pad_size):
    return {
        "id": bundle_id,
        "category": category,
        "category_label": category_label,
        "title": f"Bundle {bundle_id}",
        "anchor": {"form_id": "0x00000001", "record_type": "MISC"},
        "members": [
            {
                "form_id": "0x00000001",
                "record_type": "MISC",
                "editor_id": f"Item{bundle_id}",
                "name": "Item",
                "status": "changed",
                "role": "primary",
                "padding": "x" * pad_size,
            }
        ],
        "edges": [],
        "bug_watch": False,
        "lint_ids": [],
    }


# --------------------------------------------------------------------------
# group_bundles_by_category / lints_for_bundles
# --------------------------------------------------------------------------

class TestGrouping(unittest.TestCase):
    def setUp(self):
        self.data = load_fixture("bundles_small.json")

    def test_groups_preserve_first_appearance_order(self):
        grouped = sb.group_bundles_by_category(self.data["bundles"])
        cat_ids = [cat for cat, _label, _bundles in grouped]
        self.assertEqual(cat_ids, ["unique_weapons_gear", "perks"])

    def test_group_contents_and_label(self):
        grouped = sb.group_bundles_by_category(self.data["bundles"])
        by_id = {cat: (label, bundles) for cat, label, bundles in grouped}
        label, bundles = by_id["unique_weapons_gear"]
        self.assertEqual(label, "Unique Weapons & Gear")
        self.assertEqual([b["id"] for b in bundles], ["B0001", "B0002"])

        label2, bundles2 = by_id["perks"]
        self.assertEqual(label2, "Perks")
        self.assertEqual([b["id"] for b in bundles2], ["B0003"])

    def test_empty_bundle_list_yields_no_groups(self):
        self.assertEqual(sb.group_bundles_by_category([]), [])

    def test_lints_for_bundles_filters_by_lint_ids(self):
        lints_by_id = sb._lints_index(self.data["lints"])
        grouped = dict((cat, bundles) for cat, _label, bundles in
                       sb.group_bundles_by_category(self.data["bundles"]))
        uwg_lints = sb.lints_for_bundles(grouped["unique_weapons_gear"], lints_by_id)
        self.assertEqual([lint["id"] for lint in uwg_lints], ["L0001"])

        perk_lints = sb.lints_for_bundles(grouped["perks"], lints_by_id)
        self.assertEqual([lint["id"] for lint in perk_lints], ["L0002"])

    def test_lints_for_bundles_matches_via_bundle_id_even_if_not_in_lint_ids(self):
        lints_by_id = {"L9": {"id": "L9", "bundle_id": "B0001", "rule": "x"}}
        bundle = {"id": "B0001", "lint_ids": []}
        result = sb.lints_for_bundles([bundle], lints_by_id)
        self.assertEqual([l["id"] for l in result], ["L9"])


# --------------------------------------------------------------------------
# run_slice (Mode 1)
# --------------------------------------------------------------------------

class TestRunSlice(unittest.TestCase):
    def test_writes_categories_and_per_category_slices(self):
        data = load_fixture("bundles_small.json")
        with TempOutDir(bundles_data=data) as out_dir:
            categories_payload = sb.run_slice(out_dir)

            cat_file = out_dir / "work" / "categories.json"
            self.assertTrue(cat_file.exists())
            on_disk = json.loads(cat_file.read_text(encoding="utf-8"))
            self.assertEqual(on_disk, categories_payload)
            self.assertEqual(on_disk["schema_version"], 1)

            cats = {c["id"]: c for c in on_disk["categories"]}
            self.assertEqual(set(cats), {"unique_weapons_gear", "perks"})

            uwg = cats["unique_weapons_gear"]
            self.assertEqual(uwg["label"], "Unique Weapons & Gear")
            self.assertEqual(uwg["slug"], "unique_weapons_gear")
            self.assertEqual(uwg["bundle_ids"], ["B0001", "B0002"])
            self.assertEqual(uwg["bundle_count"], 2)
            self.assertEqual(uwg["bug_watch_count"], 1)
            self.assertEqual(uwg["post_order"], 0)
            self.assertEqual(uwg["slices"], ["work/bundles.unique_weapons_gear.json"])
            self.assertGreater(uwg["bytes"], 0)

            perks = cats["perks"]
            self.assertEqual(perks["bundle_ids"], ["B0003"])
            self.assertEqual(perks["bundle_count"], 1)
            self.assertEqual(perks["bug_watch_count"], 0)
            self.assertEqual(perks["post_order"], 1)
            self.assertEqual(perks["slices"], ["work/bundles.perks.json"])

            uwg_slice = json.loads(
                (out_dir / "work" / "bundles.unique_weapons_gear.json").read_text(encoding="utf-8")
            )
            self.assertEqual(uwg_slice["schema_version"], 1)
            self.assertEqual(uwg_slice["category"], "unique_weapons_gear")
            self.assertEqual(uwg_slice["category_label"], "Unique Weapons & Gear")
            self.assertEqual([b["id"] for b in uwg_slice["bundles"]], ["B0001", "B0002"])
            self.assertEqual([lint["id"] for lint in uwg_slice["lints"]], ["L0001"])

            perks_slice = json.loads(
                (out_dir / "work" / "bundles.perks.json").read_text(encoding="utf-8")
            )
            self.assertEqual([b["id"] for b in perks_slice["bundles"]], ["B0003"])
            self.assertEqual([lint["id"] for lint in perks_slice["lints"]], ["L0002"])

    def test_no_slice_file_leaks_other_categories_bundles(self):
        data = load_fixture("bundles_small.json")
        with TempOutDir(bundles_data=data) as out_dir:
            sb.run_slice(out_dir)
            perks_slice = json.loads(
                (out_dir / "work" / "bundles.perks.json").read_text(encoding="utf-8")
            )
            ids = [b["id"] for b in perks_slice["bundles"]]
            self.assertNotIn("B0001", ids)
            self.assertNotIn("B0002", ids)

    def test_empty_input_yields_no_categories(self):
        data = {"schema_version": 1, "meta": {}, "bundles": [], "lints": []}
        with TempOutDir(bundles_data=data) as out_dir:
            payload = sb.run_slice(out_dir)
            self.assertEqual(payload["categories"], [])
            # No stray bundles.*.json slice files should exist.
            self.assertEqual(list((out_dir / "work").glob("bundles.*.json")), [])

    def test_idempotent_rerun_replaces_stale_slices(self):
        # First run: category with enough padded bundles to force 2 parts.
        bundles = [make_padded_bundle(f"B{i:04d}", "big_cat", "Big Cat", 80_000)
                   for i in range(3)]
        data = {"schema_version": 1, "meta": {}, "bundles": bundles, "lints": []}
        with TempOutDir(bundles_data=data) as out_dir:
            first = sb.run_slice(out_dir)
            first_cat = next(c for c in first["categories"] if c["id"] == "big_cat")
            self.assertGreaterEqual(len(first_cat["slices"]), 2)
            part_files_after_first = sorted((out_dir / "work").glob("bundles.big_cat.*"))
            self.assertEqual(len(part_files_after_first), len(first_cat["slices"]))

            # Second run: shrink to a single small bundle -> should collapse
            # back to one slice file, with the old part files removed.
            (out_dir / "bundles.json").write_text(json.dumps({
                "schema_version": 1, "meta": {},
                "bundles": [make_padded_bundle("B0000", "big_cat", "Big Cat", 10)],
                "lints": [],
            }), encoding="utf-8")
            second = sb.run_slice(out_dir)
            second_cat = next(c for c in second["categories"] if c["id"] == "big_cat")
            self.assertEqual(second_cat["slices"], ["work/bundles.big_cat.json"])

            remaining = sorted(p.name for p in (out_dir / "work").glob("bundles.big_cat*"))
            self.assertEqual(remaining, ["bundles.big_cat.json"])

    def test_idempotent_rerun_leaves_unrelated_work_files_alone(self):
        data = load_fixture("bundles_small.json")
        with TempOutDir(bundles_data=data) as out_dir:
            work_dir = out_dir / "work"
            work_dir.mkdir(parents=True, exist_ok=True)
            (work_dir / "notes.md").write_text("keep me", encoding="utf-8")

            sb.run_slice(out_dir)
            sb.run_slice(out_dir)

            self.assertEqual((work_dir / "notes.md").read_text(encoding="utf-8"), "keep me")


# --------------------------------------------------------------------------
# Splitting behavior
# --------------------------------------------------------------------------

class TestSplitting(unittest.TestCase):
    def test_large_category_splits_into_bounded_parts(self):
        # ~30 bundles, each padded to ~20KB -> should force multiple parts,
        # respecting both the byte cap and the 25-bundle cap.
        bundles = [make_padded_bundle(f"B{i:04d}", "unique_weapons_gear",
                                       "Unique Weapons & Gear", 20_000)
                   for i in range(30)]
        lints_by_id = {}
        parts = sb.slice_category("unique_weapons_gear", "Unique Weapons & Gear",
                                   bundles, lints_by_id)

        self.assertGreater(len(parts), 1)

        all_ids_seen = []
        for part_bundles, _part_lints in parts:
            self.assertLessEqual(len(part_bundles), sb.MAX_BUNDLES_PER_PART)
            payload = sb.build_slice_payload("unique_weapons_gear", "Unique Weapons & Gear",
                                              part_bundles, [])
            size = sb._json_bytes(payload)
            # Each individual bundle here is far under the cap, so no part
            # should need to exceed it to hold a single oversized bundle.
            self.assertLessEqual(size, sb.MAX_SLICE_BYTES)
            all_ids_seen.extend(b["id"] for b in part_bundles)

        # No bundle split across parts, none duplicated, none dropped.
        self.assertEqual(all_ids_seen, [b["id"] for b in bundles])

    def test_small_category_is_not_split(self):
        bundles = [make_padded_bundle(f"B{i:04d}", "small_cat", "Small Cat", 100)
                   for i in range(5)]
        parts = sb.slice_category("small_cat", "Small Cat", bundles, {})
        self.assertEqual(len(parts), 1)
        self.assertEqual(len(parts[0][0]), 5)

    def test_oversized_single_bundle_is_not_split(self):
        huge_bundle = make_padded_bundle("B0001", "solo_cat", "Solo Cat", 200_000)
        parts = sb.slice_category("solo_cat", "Solo Cat", [huge_bundle], {})
        self.assertEqual(len(parts), 1)
        self.assertEqual(len(parts[0][0]), 1)
        self.assertIs(parts[0][0][0], huge_bundle)

    def test_categories_json_lists_parts_in_order(self):
        bundles = [make_padded_bundle(f"B{i:04d}", "unique_weapons_gear",
                                       "Unique Weapons & Gear", 20_000)
                   for i in range(30)]
        data = {"schema_version": 1, "meta": {}, "bundles": bundles, "lints": []}
        with TempOutDir(bundles_data=data) as out_dir:
            payload = sb.run_slice(out_dir)
            cat = payload["categories"][0]
            self.assertGreater(len(cat["slices"]), 1)
            expected = [f"work/bundles.unique_weapons_gear.part{i}.json"
                        for i in range(1, len(cat["slices"]) + 1)]
            self.assertEqual(cat["slices"], expected)
            for rel_path in cat["slices"]:
                self.assertTrue((out_dir / rel_path).exists())


# --------------------------------------------------------------------------
# extract_records / run_extract (Mode 2)
# --------------------------------------------------------------------------

class TestExtract(unittest.TestCase):
    def setUp(self):
        self.comprehensive = {
            "schema_version": 1,
            "meta": {},
            "records": {
                "0x00123456": {
                    "editor_id": "EnclavePlasmaGun",
                    "record_type": "WEAP",
                    "refs": {"formid": "0x00ABCDEF", "editor_id": "SomeKeyword"},
                },
                "0x00ABCDEF": {
                    "editor_id": "SomeKeyword",
                    "record_type": "KYWD",
                },
            },
            "common_changes": [],
            "ref_names": {
                "0x00ABCDEF": {"editor_id": "SomeKeyword", "record_type": "KYWD", "name": ""},
                "0x00FFFFFF": {"editor_id": "Unrelated", "record_type": "MISC", "name": ""},
            },
        }

    def test_existing_and_missing_formids(self):
        result = sb.extract_records(self.comprehensive, ["0x00123456", "0xDEADBEEF"])
        self.assertEqual(
            result["records"]["0x00123456"]["editor_id"], "EnclavePlasmaGun"
        )
        self.assertIsNone(result["records"]["0xDEADBEEF"])

    def test_case_insensitive_matching(self):
        result = sb.extract_records(self.comprehensive, ["0x00123456".lower()])
        # Result key echoes the caller's requested string verbatim.
        self.assertIn("0x00123456", result["records"])
        self.assertIsNotNone(result["records"]["0x00123456"])
        self.assertEqual(result["records"]["0x00123456"]["editor_id"], "EnclavePlasmaGun")

        result2 = sb.extract_records(self.comprehensive, ["0X00123456"])
        self.assertIsNotNone(result2["records"]["0X00123456"])

    def test_ref_names_subset_only_includes_referenced_formids(self):
        result = sb.extract_records(self.comprehensive, ["0x00123456"])
        self.assertIn("0x00ABCDEF", result["ref_names"])
        self.assertNotIn("0x00FFFFFF", result["ref_names"])

    def test_ref_names_capped_at_200(self):
        many_refs = {f"0x{i:08X}": {"formid": f"0x{i:08X}"} for i in range(300)}
        comp = {
            "records": {"0x00000001": {"nested": many_refs}},
            "ref_names": {f"0x{i:08X}": {"name": f"n{i}"} for i in range(300)},
        }
        result = sb.extract_records(comp, ["0x00000001"])
        self.assertLessEqual(len(result["ref_names"]), sb.MAX_REF_NAMES)

    def test_no_formids_given_to_extract_records_yields_empty(self):
        result = sb.extract_records(self.comprehensive, [])
        self.assertEqual(result["records"], {})
        self.assertEqual(result["ref_names"], {})

    def test_run_extract_success(self):
        with TempOutDir(comprehensive_data=self.comprehensive) as out_dir:
            import io
            import contextlib

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = sb.run_extract(out_dir, ["0x00123456", "0xDEADBEEF"])
            self.assertEqual(code, 0)
            printed = json.loads(buf.getvalue())
            self.assertIsNotNone(printed["records"]["0x00123456"])
            self.assertIsNone(printed["records"]["0xDEADBEEF"])

    def test_run_extract_missing_file_is_hard_error(self):
        with TempOutDir() as out_dir:
            code = sb.run_extract(out_dir, ["0x00123456"])
            self.assertEqual(code, 1)

    def test_run_extract_bad_json_is_hard_error(self):
        with TempOutDir() as out_dir:
            (Path(out_dir) / "comprehensive.json").write_text("{not valid json", encoding="utf-8")
            code = sb.run_extract(out_dir, ["0x00123456"])
            self.assertEqual(code, 1)


# --------------------------------------------------------------------------
# CLI (main())
# --------------------------------------------------------------------------

class TestCli(unittest.TestCase):
    def test_main_slice_mode(self):
        data = load_fixture("bundles_small.json")
        with TempOutDir(bundles_data=data) as out_dir:
            code = sb.main([str(out_dir)])
            self.assertEqual(code, 0)
            self.assertTrue((out_dir / "work" / "categories.json").exists())

    def test_main_extract_requires_formids(self):
        with TempOutDir() as out_dir:
            code = sb.main(["--extract", str(out_dir)])
            self.assertEqual(code, 1)

    def test_main_extract_mode(self):
        comp = {
            "records": {"0x00000001": {"editor_id": "Foo"}},
            "ref_names": {},
        }
        with TempOutDir(comprehensive_data=comp) as out_dir:
            import io
            import contextlib

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = sb.main(["--extract", str(out_dir), "0x00000001"])
            self.assertEqual(code, 0)
            printed = json.loads(buf.getvalue())
            self.assertEqual(printed["records"]["0x00000001"]["editor_id"], "Foo")

    def test_main_slice_mode_rejects_extra_positional_args(self):
        data = load_fixture("bundles_small.json")
        with TempOutDir(bundles_data=data) as out_dir:
            code = sb.main([str(out_dir), "unexpected"])
            self.assertEqual(code, 1)

    def test_main_slice_mode_missing_bundles_json(self):
        with TempOutDir() as out_dir:
            code = sb.main([str(out_dir)])
            self.assertEqual(code, 1)


class TestSubprocessSmoke(unittest.TestCase):
    def test_script_runs_as_subprocess_in_slice_mode(self):
        data = load_fixture("bundles_small.json")
        with TempOutDir(bundles_data=data) as out_dir:
            result = subprocess.run(
                [sys.executable, str(SCRIPT_PATH), str(out_dir)],
                capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertTrue((out_dir / "work" / "categories.json").exists())
            self.assertIn("bundles", result.stderr)

    def test_script_runs_as_subprocess_in_extract_mode(self):
        comp = {"records": {"0x00000001": {"editor_id": "Foo"}}, "ref_names": {}}
        with TempOutDir(comprehensive_data=comp) as out_dir:
            result = subprocess.run(
                [sys.executable, str(SCRIPT_PATH), "--extract", str(out_dir), "0x00000001"],
                capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            printed = json.loads(result.stdout)
            self.assertEqual(printed["records"]["0x00000001"]["editor_id"], "Foo")


if __name__ == "__main__":
    unittest.main()
