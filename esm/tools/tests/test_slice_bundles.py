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


def minimal_comprehensive_record(**overrides):
    record = {
        "form_id": "0x00000001",
        "record_type": "MISC",
        "editor_id": "Foo",
        "name": None,
        "description": None,
        "status": "changed",
        "prev_editor_id": None,
        "cut": None,
        "fields": None,
        "refs_out": [],
        "changes": [],
    }
    record.update(overrides)
    if "form_id" in overrides:
        record["form_id"] = overrides["form_id"]
    return record


def minimal_comprehensive_doc(records, **overrides):
    doc = {
        "schema_version": 1,
        "meta": {},
        "records": records,
        "common_changes": [],
        "ref_names": {},
    }
    doc.update(overrides)
    return doc


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
        if self._tmp is not None:
            self._tmp.cleanup()


# --------------------------------------------------------------------------
# _lints_index / lints_for_bundles
# --------------------------------------------------------------------------

class TestLints(unittest.TestCase):
    def setUp(self):
        self.data = load_fixture("bundles_small.json")

    def test_lints_for_bundles_filters_by_lint_ids(self):
        lints_by_id = sb._lints_index(self.data["lints"])
        # Build a minimal bundles structure to test lints_for_bundles
        bundles_subset = [b for b in self.data["bundles"] if b["id"] == "B0001"]
        lints = sb.lints_for_bundles(bundles_subset, lints_by_id)
        self.assertEqual([lint["id"] for lint in lints], ["L0001"])

    def test_lints_for_bundles_matches_via_bundle_id_even_if_not_in_lint_ids(self):
        lints_by_id = {"L9": {"id": "L9", "bundle_id": "B0001", "rule": "x"}}
        bundle = {"id": "B0001", "lint_ids": []}
        result = sb.lints_for_bundles([bundle], lints_by_id)
        self.assertEqual([lint["id"] for lint in result], ["L9"])


# --------------------------------------------------------------------------
# extract_records / run_extract (Mode 2)
# --------------------------------------------------------------------------

class TestExtract(unittest.TestCase):
    @staticmethod
    def _record(**overrides):
        entry = {
            "form_id": "0x00123456",
            "record_type": "WEAP",
            "editor_id": "EnclavePlasmaGun",
            "name": None,
            "description": None,
            "status": "changed",
            "prev_editor_id": None,
            "cut": None,
            "fields": None,
            "refs_out": [],
            "changes": [],
        }
        entry.update(overrides)
        return entry

    def setUp(self):
        self.comprehensive = {
            "schema_version": 1,
            "meta": {},
            "records": {
                "0x00123456": self._record(
                    form_id="0x00123456",
                    refs_out=[{"formid": "0x00ABCDEF", "path": "refs"}],
                ),
                "0x00ABCDEF": self._record(
                    form_id="0x00ABCDEF",
                    record_type="KYWD",
                    editor_id="SomeKeyword",
                ),
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
            import contextlib
            import io

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
    def test_main_extract_requires_formids(self):
        with TempOutDir() as out_dir:
            code = sb.main(["--extract", str(out_dir)])
            self.assertEqual(code, 1)

    def test_main_extract_mode(self):
        comp = minimal_comprehensive_doc({
            "0x00000001": minimal_comprehensive_record(form_id="0x00000001", editor_id="Foo"),
        })
        with TempOutDir(comprehensive_data=comp) as out_dir:
            import contextlib
            import io

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = sb.main(["--extract", str(out_dir), "0x00000001"])
            self.assertEqual(code, 0)
            printed = json.loads(buf.getvalue())
            self.assertEqual(printed["records"]["0x00000001"]["editor_id"], "Foo")


class TestSubprocessSmoke(unittest.TestCase):
    def test_script_runs_as_subprocess_in_extract_mode(self):
        comp = minimal_comprehensive_doc({
            "0x00000001": minimal_comprehensive_record(form_id="0x00000001", editor_id="Foo"),
        })
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
