"""Tests for pn/workflow.py: the skill's prepare / merge-assessment / gate /
publish verbs, run against a fake `esm` script and the fixture gateway."""

from __future__ import annotations

import contextlib
import io
import json
import os
import unittest
from pathlib import Path
from unittest import mock

from pn import jsonio, layout, schemas, workflow
from tests.builders import TempDirTestCase, fake_esm_script
from tests.fake_gateway import FakeGateway

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
DIFF_SMALL = FIXTURES_DIR / "diff_small.json"
REFS_GRAPH = FIXTURES_DIR / "refs_graph.json"


def make_data_dir(root: Path, *tokens: str) -> None:
    """`root/<token>/SeventySix.esm` for each token, plus the `notes/` dir the
    pipeline writes into (never a snapshot)."""
    for token in tokens:
        (root / token).mkdir(parents=True)
        (root / token / "SeventySix.esm").write_bytes(b"FAKE ESM")
    (root / "notes").mkdir()


def run_verb(verb, argv, **kwargs) -> tuple[int, dict | None]:
    """Run a workflow verb, returning its exit code and parsed JSON stdout."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
        rc = verb(argv, **kwargs)
    text = out.getvalue()
    return rc, (json.loads(text) if text.strip() else None)


class TestResolveSnapshots(TempDirTestCase):
    def setUp(self):
        super().setUp()
        make_data_dir(self.tmp, "20260903", "20260914", "20260918")

    def test_no_arguments_take_the_newest_two(self):
        old, new = workflow.resolve_snapshots([], self.tmp)
        self.assertEqual((old.name, new.name), ("20260914", "20260918"))

    def test_one_argument_names_new_and_its_predecessor(self):
        old, new = workflow.resolve_snapshots(["20260914"], self.tmp)
        self.assertEqual((old.name, new.name), ("20260903", "20260914"))

    def test_absolute_directories_need_no_data_dir(self):
        old, new = workflow.resolve_snapshots([str(self.tmp / "20260903"), str(self.tmp / "20260918")], None)
        self.assertEqual((old.name, new.name), ("20260903", "20260918"))

    def test_errors(self):
        for args, root in ((["20260903"], self.tmp), (["20990101"], self.tmp), ([], None), (["a", "b", "c"], self.tmp)):
            with self.subTest(args=args), self.assertRaises(ValueError):
                workflow.resolve_snapshots(args, root)


class TestSplitDeepSlice(TempDirTestCase):
    def write_slice(self, n):
        bundles = [{"id": f"B{i:04d}", "lint_ids": [f"L{i:04d}"]} for i in range(1, n + 1)]
        lints = [{"id": f"L{i:04d}", "bundle_id": f"B{i:04d}"} for i in range(1, n + 1)]
        jsonio.write(layout.work_deep_slice_json(self.tmp), {"bundles": bundles, "lints": lints})

    def test_small_tier_is_one_slice(self):
        self.write_slice(workflow.SPLIT_DEEP_ABOVE)
        self.assertEqual(workflow.split_deep_slice(self.tmp), [layout.work_deep_slice_json(self.tmp)])

    def test_large_tier_splits_in_contiguous_halves_with_their_lints(self):
        self.write_slice(25)
        part1, part2 = workflow.split_deep_slice(self.tmp)
        first, second = jsonio.read(part1), jsonio.read(part2)
        self.assertEqual([b["id"] for b in first["bundles"]], [f"B{i:04d}" for i in range(1, 14)])
        self.assertEqual(len(second["bundles"]), 12)
        self.assertEqual([lint["id"] for lint in second["lints"]], [f"L{i:04d}" for i in range(14, 26)])

    def test_resplitting_a_smaller_tier_removes_old_parts(self):
        self.write_slice(25)
        workflow.split_deep_slice(self.tmp)
        self.write_slice(5)
        workflow.split_deep_slice(self.tmp)
        self.assertEqual(list(layout.work_dir(self.tmp).glob("deep-slice.part*.json")), [])


class TestPrepareGatePublish(TempDirTestCase):
    def setUp(self):
        super().setUp()
        self.data = self.tmp / "data"
        make_data_dir(self.data, "20260626", "20260703")
        self.esm_bin = fake_esm_script(self.tmp, stdout_path=DIFF_SMALL)
        env = mock.patch.dict(os.environ, {"FO76_DATA_DIR": str(self.data)})
        env.start()
        self.addCleanup(env.stop)
        self.out = self.data / "notes" / "20260626_to_20260703"

    def prepare(self, *extra):
        return run_verb(
            workflow.prepare, ["--esm-bin", str(self.esm_bin), *extra], client=FakeGateway(REFS_GRAPH)
        )

    def test_prepare_runs_the_pipeline_then_reuses_it(self):
        rc, summary = self.prepare()
        self.assertEqual(rc, 0)
        assert summary is not None
        self.assertEqual(summary["out_dir"], str(self.out))
        self.assertFalse(summary["reused"])
        self.assertEqual(sum(summary["tiers"].values()), len(jsonio.read(layout.bundles_json(self.out))["bundles"]))
        self.assertEqual(summary["deep_slices"], [str(layout.work_deep_slice_json(self.out))])
        manifest = jsonio.read(layout.manifest_json(self.out))
        self.assertEqual(manifest["inputs"]["pipeline_version"], schemas.PIPELINE_VERSION)

        rc, summary = self.prepare()
        self.assertEqual(rc, 0)
        assert summary is not None
        self.assertTrue(summary["reused"])

    def test_output_from_an_older_pipeline_version_is_rebuilt(self):
        self.prepare()
        manifest = jsonio.read(layout.manifest_json(self.out))
        manifest["inputs"]["pipeline_version"] = schemas.PIPELINE_VERSION - 1
        jsonio.write(layout.manifest_json(self.out), manifest)
        _, summary = self.prepare()
        assert summary is not None
        self.assertFalse(summary["reused"])
        self.assertTrue(summary["retriaged"])

    def gate(self, *extra):
        esms = ["--old-esm", str(self.data / "20260626" / "SeventySix.esm")]
        esms += ["--new-esm", str(self.data / "20260703" / "SeventySix.esm")]
        return run_verb(workflow.gate, [str(self.out), *esms, *extra], client=FakeGateway(REFS_GRAPH))

    def test_gate_fails_without_drafts_and_publish_records_the_run(self):
        self.prepare()
        rc, summary = self.gate()
        self.assertEqual(rc, 1)
        assert summary is not None
        self.assertFalse(summary["ok"])
        self.assertTrue(summary["live_lookups"])

        layout.patch_summary_md(self.out).write_text("# FO76 Datamine\n\nOne story.\n", encoding="utf-8")
        rc, summary = run_verb(workflow.publish, [str(self.out), "--no-review"])
        self.assertEqual(rc, 0)
        assert summary is not None
        self.assertEqual(summary["chunks"], 1)
        self.assertFalse(summary["reviewed"])
        narrative = jsonio.read(layout.manifest_json(self.out))["stages"]["narrative"]
        self.assertIsNotNone(narrative["completed_at"])

    def test_gate_requires_the_run_esms(self):
        self.prepare()
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            workflow.gate([str(self.out)])

    def test_publish_needs_a_review_unless_told_otherwise(self):
        self.prepare()
        layout.patch_summary_md(self.out).write_text("# FO76 Datamine\n", encoding="utf-8")
        rc, _ = run_verb(workflow.publish, [str(self.out)])
        self.assertEqual(rc, 1)

    def test_republishing_a_shorter_summary_leaves_no_stale_chunks(self):
        self.prepare()
        long_summary = "# FO76 Datamine\n\n" + "\n\n".join(f"## Section {i}\n" + "word " * 250 for i in range(6))
        layout.patch_summary_md(self.out).write_text(long_summary, encoding="utf-8")
        rc, summary = run_verb(workflow.publish, [str(self.out), "--no-review"])
        assert summary is not None
        self.assertGreater(summary["chunks"], 1)
        layout.patch_summary_md(self.out).write_text("# FO76 Datamine\n\nShort.\n", encoding="utf-8")
        rc, summary = run_verb(workflow.publish, [str(self.out), "--no-review"])
        assert summary is not None
        self.assertEqual(summary["chunks"], 1)
        self.assertEqual(sorted(p.name for p in layout.discord_dir(self.out).iterdir()), ["chunk_001.md"])

    def test_reusing_output_keeps_a_merged_assessment(self):
        _, summary = self.prepare()
        assert summary is not None
        triage = jsonio.read(layout.work_triage_json(self.out))
        triage["reasons"]["marker"] = "kept"
        jsonio.write(layout.work_triage_json(self.out), triage)
        _, summary = self.prepare()
        assert summary is not None
        self.assertTrue(summary["reused"])
        self.assertFalse(summary["retriaged"])
        self.assertEqual(jsonio.read(layout.work_triage_json(self.out))["reasons"]["marker"], "kept")
        _, summary = self.prepare("--retriage")
        assert summary is not None
        self.assertTrue(summary["retriaged"])

    def test_publish_rejects_a_malformed_review(self):
        self.prepare()
        layout.patch_summary_md(self.out).write_text("# FO76 Datamine\n", encoding="utf-8")
        jsonio.write(layout.work_review_json(self.out), {"findings": [{"severity": "critical"}], "checked": {}})
        rc, _ = run_verb(workflow.publish, [str(self.out)])
        self.assertEqual(rc, 1)


if __name__ == "__main__":
    unittest.main()
