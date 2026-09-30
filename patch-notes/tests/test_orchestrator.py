#!/usr/bin/env python3
"""Tests for pn/make_patch_notes.py (mechanical-stage orchestrator) and
pn/update_manifest.py (narrative-stage manifest updater).

The orchestrator end-to-end tests never spawn a real `esm`: the
diff step is satisfied by a tiny generated shell script that ignores its
arguments and `cat`s `tests/fixtures/diff_small.json` verbatim, and the
bundles/lints stages run against `esmcli.FakeGateway` backed by
`tests/fixtures/refs_graph.json` (an injected `FakeGateway`). No real
ESM is touched.

(`esmcli`'s own tests cover the stricter JSON-parsing contract this fake
binary complies with -- see `test_esmcli.py`'s
`test_trailing_garbage_after_json_is_rejected`.)
"""

from __future__ import annotations

import json
import os
import unittest
from pathlib import Path
from typing import Any, cast
from unittest import mock

from pn import make_patch_notes as mpn
from pn import patchnotes_lib as pl
from pn import schemas
from pn import update_manifest as um
from tests.builders import TempDirTestCase, fake_esm_script
from tests.fake_gateway import FakeGateway

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
DIFF_SMALL = FIXTURES_DIR / "diff_small.json"
REFS_GRAPH = FIXTURES_DIR / "refs_graph.json"


# ---------------------------------------------------------------------------
# Shared fixture builders
# ---------------------------------------------------------------------------


def make_fake_esm(tmp_dir: Path, diff_json: Path = DIFF_SMALL) -> Path:
    """A tiny shell-script stand-in for the `esm` binary: ignores every
    argument and cats a fixed diff.json fixture verbatim."""
    return fake_esm_script(tmp_dir, stdout_path=diff_json)


def make_snapshot(tmp_dir: Path, token: str, lang: str = "en") -> Path:
    """A dummy `<tmp_dir>/<token>/SeventySix_<token>.esm` plus a sibling
    `strings/` dir holding a matching `*_en.strings` stub, the layout
    `esm diff` discovers each side's sources from."""
    snap_dir = tmp_dir / token
    snap_dir.mkdir()
    esm_path = snap_dir / f"SeventySix_{token}.esm"
    esm_path.write_bytes(b"FAKE ESM BYTES")
    strings_dir = snap_dir / "strings"
    strings_dir.mkdir()
    (strings_dir / f"SeventySix_{token}_{lang}.strings").write_bytes(b"")
    return esm_path


# ---------------------------------------------------------------------------
# Unit: out-dir / token derivation
# ---------------------------------------------------------------------------


class TestEsmTokenAndOutDir(unittest.TestCase):
    def test_dated_stem(self):
        p = Path("/data/v1/SeventySix_20260626.esm")
        self.assertEqual(mpn.esm_token(p), "20260626")

    def test_dated_parent_dir_fallback(self):
        # Real snapshot layout: the parent dir carries the date, not the file
        # itself ($FO76_DATA_DIR/<snapshot>/SeventySix.esm).
        p = Path("/data/20260626/SeventySix.esm")
        self.assertEqual(mpn.esm_token(p), "20260626")

    def test_dated_stem_takes_precedence_over_dated_parent(self):
        p = Path("/data/20260101/SeventySix_20260626.esm")
        self.assertEqual(mpn.esm_token(p), "20260626")

    def test_no_digits_anywhere_falls_back_to_parent_name(self):
        p = Path("/data/release/SeventySix.esm")
        self.assertEqual(mpn.esm_token(p), "release")

    def test_default_out_dir_uses_both_tokens_next_to_new_esm(self):
        old = Path("/data/20260626/SeventySix.esm")
        new = Path("/data/20260703/SeventySix.esm")
        self.assertEqual(
            mpn.default_out_dir(old, new),
            Path("/data/20260703/patch_20260626_to_20260703"),
        )


# ---------------------------------------------------------------------------
# Unit: source flags pass through to esm diff
# ---------------------------------------------------------------------------


class TestSourceArgs(unittest.TestCase):
    def _args(self, *argv: str):
        # $STARTUP_BA2 / $CURVES_DIR feed the parser's defaults.
        with mock.patch.dict(os.environ, clear=True):
            return mpn.build_arg_parser().parse_args(["a.esm", "b.esm", *argv])

    def test_no_flags_leave_discovery_to_esm(self):
        self.assertEqual(mpn.source_args(self._args()), [])

    def test_startup_ba2_wins_over_curves_dir(self):
        args = self._args("--startup-ba2", "/startup.ba2", "--curves-dir", "/misc")
        self.assertEqual(mpn.source_args(args), ["--startup-ba2", "/startup.ba2"])

    def test_flags_pass_through_as_absolute_paths(self):
        args = self._args("--strings-dir-a", "old/strings", "--strings-dir-b", "/new/strings",
                          "--curves-dir", "/misc")
        self.assertEqual(
            mpn.source_args(args),
            ["--strings-dir-a", str(Path("old/strings").resolve()),
             "--strings-dir-b", "/new/strings", "--curves-dir", "/misc"],
        )

    def test_manifest_options_carry_a_digest_not_source_paths(self):
        args = self._args("--strings-dir", "/private-root/strings")
        sources = mpn.output_options(args)["sources"]
        self.assertNotIn("private-root", sources)
        self.assertRegex(sources, r"^[0-9a-f]{16}$")
        self.assertEqual(sources, mpn.output_options(args)["sources"])
        other = self._args("--strings-dir", "/other-root/strings")
        self.assertNotEqual(sources, mpn.output_options(other)["sources"])
        self.assertEqual(mpn.output_options(self._args())["sources"], "")


# ---------------------------------------------------------------------------
# Unit: esm-diff command construction (--exclude-type default/disable, etc.)
# ---------------------------------------------------------------------------


class TestBuildDiffCmd(unittest.TestCase):
    def _cmd(self, **overrides: Any):
        kwargs: dict[str, Any] = dict(
            lang="en",
            record_type=None,
            bodies="full",
            keep_noise=False,
            exclude_type="LAND,NAVM",
        )
        kwargs.update(overrides)
        return mpn.build_diff_cmd(
            Path("esm"), Path("a.esm"), Path("b.esm"), **cast(Any, kwargs)
        )

    def test_default_exclude_type_passed_through(self):
        cmd = self._cmd()
        self.assertIn("--exclude-type", cmd)
        self.assertEqual(cmd[cmd.index("--exclude-type") + 1], "LAND,NAVM")

    def test_empty_exclude_type_disables_flag(self):
        cmd = self._cmd(exclude_type="")
        self.assertNotIn("--exclude-type", cmd)

    def test_keep_noise_flag_only_passed_when_true(self):
        self.assertIn("--keep-noise", self._cmd(keep_noise=True))
        self.assertNotIn("--keep-noise", self._cmd(keep_noise=False))

    def test_bodies_passed_through(self):
        cmd = self._cmd(bodies="stub")
        self.assertEqual(cmd[cmd.index("--bodies") + 1], "stub")

    def test_type_filter_passed_through_only_when_given(self):
        cmd = self._cmd(record_type="WEAP")
        self.assertEqual(cmd[cmd.index("--type") + 1], "WEAP")
        self.assertNotIn("--type", self._cmd(record_type=None))

    def test_pretty_never_passed(self):
        # Spec: drop --pretty (smaller diff.json) -- never emitted regardless
        # of other flags.
        self.assertNotIn("--pretty", self._cmd())

    def test_source_flags_pass_through_verbatim(self):
        cmd = self._cmd(sources=["--strings-dir-a", "/a", "--curves-dir", "/misc"])
        self.assertEqual(cmd[cmd.index("--strings-dir-a"):][:4],
                         ["--strings-dir-a", "/a", "--curves-dir", "/misc"])
        self.assertNotIn("--strings-dir", self._cmd())

    def test_argparse_default_exclude_type(self):
        args = mpn.build_arg_parser().parse_args(["a.esm", "b.esm"])
        self.assertEqual(args.exclude_type, mpn.DEFAULT_EXCLUDE_TYPE)
        self.assertEqual(args.bodies, "stub")

    def test_argparse_exclude_type_disable(self):
        args = mpn.build_arg_parser().parse_args(["a.esm", "b.esm", "--exclude-type", ""])
        self.assertEqual(args.exclude_type, "")


# ---------------------------------------------------------------------------
# End-to-end: make_patch_notes.main() against the fake esm binary + FakeGateway
# ---------------------------------------------------------------------------


class TestOrchestratorEndToEnd(TempDirTestCase):
    def setUp(self):
        super().setUp()
        self.tmp_dir = self.tmp  # this suite reads it as a scratch dir, not an out-dir
        self.fake_esm = make_fake_esm(self.tmp_dir)
        self.old_esm = make_snapshot(self.tmp_dir, "20260626")
        self.new_esm = make_snapshot(self.tmp_dir, "20260703")

    def _run(self, out_dir, extra_args=()):
        return mpn.main([
            str(self.old_esm), str(self.new_esm),
            "--esm-bin", str(self.fake_esm),
            "--out-dir", str(out_dir),
            *extra_args,
        ], client=FakeGateway(REFS_GRAPH))

    def test_full_run_writes_all_files(self):
        out_dir = self.tmp_dir / "out"
        self.assertEqual(self._run(out_dir), 0)
        for fname in (
            "diff.json", "comprehensive.json",
            "bundles.json", "lints.json", "manifest.json",
        ):
            self.assertTrue((out_dir / fname).is_file(), f"missing {fname}")

    def test_diff_json_matches_fixture(self):
        out_dir = self.tmp_dir / "out"
        self._run(out_dir)
        diff = json.loads((out_dir / "diff.json").read_text())
        fixture = json.loads(DIFF_SMALL.read_text())
        self.assertEqual(diff, fixture)

    def test_manifest_inputs_match_new_esm(self):
        out_dir = self.tmp_dir / "out"
        self._run(out_dir)
        manifest = json.loads((out_dir / "manifest.json").read_text())
        inputs = manifest["inputs"]
        st = self.new_esm.stat()
        self.assertEqual(inputs["old_token"], "20260626")
        self.assertEqual(inputs["new_token"], "20260703")
        self.assertEqual(inputs["new_esm_size"], st.st_size)
        self.assertEqual(inputs["new_esm_mtime"], int(st.st_mtime))
        self.assertEqual(inputs["pipeline_version"], schemas.PIPELINE_VERSION)

    def test_manifest_counts_populated(self):
        out_dir = self.tmp_dir / "out"
        self._run(out_dir)
        manifest = json.loads((out_dir / "manifest.json").read_text())
        counts = manifest["counts"]
        self.assertEqual(counts["added"], 2)
        self.assertEqual(counts["removed"], 1)
        self.assertEqual(counts["changed"], 11)
        self.assertIn("bundles", counts)
        self.assertIn("singletons", counts)
        self.assertEqual(set(counts["lints"].keys()), {"error", "warn", "info"})

    def test_manifest_stages_shape(self):
        out_dir = self.tmp_dir / "out"
        self._run(out_dir)
        manifest = json.loads((out_dir / "manifest.json").read_text())
        mech = manifest["stages"]["mechanical"]
        self.assertIsNotNone(mech["completed_at"])
        self.assertEqual(
            set(mech["files"].values()),
            {"diff.json", "comprehensive.json", "bundles.json", "lints.json"},
        )
        narrative = manifest["stages"]["narrative"]
        self.assertIsNone(narrative["completed_at"])
        self.assertEqual(narrative["max_chunk_chars"], 2000)
        self.assertIn("usage", narrative)
        self.assertIsNone(narrative["usage"])

    def test_finished_narrative_is_not_overwritten_without_force(self):
        out_dir = self.tmp_dir / "out"
        self.assertEqual(self._run(out_dir), 0)
        manifest = json.loads((out_dir / "manifest.json").read_text())
        manifest["stages"]["narrative"]["completed_at"] = "2026-09-12T00:00:00Z"
        (out_dir / "manifest.json").write_text(json.dumps(manifest))
        with self.assertRaises(SystemExit) as cm:
            self._run(out_dir)
        self.assertEqual(cm.exception.code, 1)
        # The finished run is untouched.
        self.assertEqual(
            json.loads((out_dir / "manifest.json").read_text())["stages"]["narrative"]["completed_at"],
            "2026-09-12T00:00:00Z",
        )
        self.assertEqual(self._run(out_dir, ["--force-pipeline"]), 0)
        self.assertIsNone(json.loads((out_dir / "manifest.json").read_text())["stages"]["narrative"]["completed_at"])

    def test_default_out_dir_used_when_not_given(self):
        rc = mpn.main([
            str(self.old_esm), str(self.new_esm),
            "--esm-bin", str(self.fake_esm),
        ], client=FakeGateway(REFS_GRAPH))
        self.assertEqual(rc, 0)
        expected = self.new_esm.parent / "patch_20260626_to_20260703"
        self.assertTrue((expected / "manifest.json").is_file())

    def test_skip_bundles_skips_bundles_and_lints(self):
        out_dir = self.tmp_dir / "out_skip_bundles"
        rc = self._run(out_dir, ["--skip-bundles"])
        self.assertEqual(rc, 0)
        self.assertTrue((out_dir / "comprehensive.json").is_file())
        self.assertFalse((out_dir / "bundles.json").exists())
        self.assertFalse((out_dir / "lints.json").exists())

        manifest = json.loads((out_dir / "manifest.json").read_text())
        self.assertNotIn("bundles", manifest["stages"]["mechanical"]["files"])
        self.assertNotIn("lints", manifest["stages"]["mechanical"]["files"])
        self.assertNotIn("bundles", manifest["counts"])
        self.assertNotIn("lints", manifest["counts"])

    def test_skip_lints_still_builds_bundles(self):
        out_dir = self.tmp_dir / "out_skip_lints"
        rc = self._run(out_dir, ["--skip-lints"])
        self.assertEqual(rc, 0)
        self.assertTrue((out_dir / "bundles.json").is_file())
        self.assertFalse((out_dir / "lints.json").exists())

        manifest = json.loads((out_dir / "manifest.json").read_text())
        self.assertIn("bundles", manifest["stages"]["mechanical"]["files"])
        self.assertNotIn("lints", manifest["stages"]["mechanical"]["files"])
        self.assertIn("bundles", manifest["counts"])
        self.assertNotIn("lints", manifest["counts"])


# ---------------------------------------------------------------------------
# update_manifest.py
# ---------------------------------------------------------------------------


class TestUpdateManifest(TempDirTestCase):
    """Covers update_manifest.py's narrative stage: a single
    patch-summary.md, a flat discord/ chunk list, and work/triage.json tier
    counts."""

    def setUp(self):
        super().setUp()
        self.out_dir = self.tmp  # every test here treats it as a pipeline out-dir
        manifest = pl.new_manifest(
            patch_date="2026-07-03",
            old_token="20260626",
            new_token="20260703",
            new_esm_size=123,
            new_esm_mtime=456,
            pipeline_version=schemas.PIPELINE_VERSION,
            counts={"added": 1, "changed": 2, "removed": 0},
        )
        manifest["stages"]["mechanical"]["completed_at"] = "2026-07-03T00:00:00Z"
        manifest["stages"]["mechanical"]["files"] = {"diff": "diff.json"}
        pl.write_manifest(self.out_dir, manifest)

    def _write_patch_summary(self, text="# Patch Summary\n"):
        (self.out_dir / "patch-summary.md").write_text(text)

    def _write_chunks(self, n):
        chunk_dir = self.out_dir / "discord"
        chunk_dir.mkdir(parents=True, exist_ok=True)
        for i in range(1, n + 1):
            (chunk_dir / f"chunk_{i:03d}.md").write_text(f"chunk {i}")

    def _write_triage_json(self, deep=0, brief=0, drop=0, ambiguous=0, resolved_by_assessor=0):
        work_dir = self.out_dir / "work"
        work_dir.mkdir(exist_ok=True)
        payload = {
            "deep": [f"B{i:04d}" for i in range(1, deep + 1)],
            "brief": [f"B{i:04d}" for i in range(deep + 1, deep + brief + 1)],
            "drop": [f"B{i:04d}" for i in range(deep + brief + 1, deep + brief + drop + 1)],
            "ambiguous": [
                f"B{i:04d}"
                for i in range(deep + brief + drop + 1, deep + brief + drop + ambiguous + 1)
            ],
            "stats": {
                "total_bundles": deep + brief + drop + ambiguous,
                "deep": deep, "brief": brief, "drop": drop, "ambiguous": ambiguous,
                "resolved_by_assessor": resolved_by_assessor,
            },
            "reasons": {},
        }
        (work_dir / "triage.json").write_text(json.dumps(payload))

    def test_missing_manifest_errors(self):
        # A sibling dir with no manifest.json -- self.out_dir has one by setUp.
        empty = self.tmp / "empty"
        empty.mkdir()
        self.assertEqual(um.main([str(empty)]), 1)

    def test_no_outputs_yields_null_summary_and_zero_chunks(self):
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertIsNone(narrative["patch_summary_md"])
        self.assertEqual(narrative["chunk_count"], 0)
        self.assertEqual(narrative["chunks"], [])
        self.assertIsNone(narrative["triage"])

    def test_usage_is_none_without_usage_json(self):
        self.assertEqual(um.main([str(self.out_dir)]), 0)
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertIsNone(narrative["usage"])

    def test_usage_json_is_folded_in_with_a_total(self):
        work_dir = self.out_dir / "work"
        work_dir.mkdir(exist_ok=True)
        (work_dir / "usage.json").write_text(json.dumps({
            "assessor": {"tokens": 1000},
            "writers": [{"tokens": 40000}, {"tokens": 35000}],
            "reviewer": {"tokens": 12000},
        }))
        self.assertEqual(um.main([str(self.out_dir)]), 0)
        usage = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]["usage"]
        self.assertEqual(usage["total_tokens"], 88000)
        self.assertEqual(usage["writers"][1]["tokens"], 35000)

    def test_malformed_usage_json_is_an_error(self):
        work_dir = self.out_dir / "work"
        work_dir.mkdir(exist_ok=True)
        (work_dir / "usage.json").write_text(json.dumps({"writers": [{"tokens": "lots"}]}))
        self.assertEqual(um.main([str(self.out_dir)]), 1)

    def test_patch_summary_and_chunks_discovered(self):
        self._write_patch_summary()
        self._write_chunks(3)
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)

        manifest = json.loads((self.out_dir / "manifest.json").read_text())
        narrative = manifest["stages"]["narrative"]
        self.assertIsNotNone(narrative["completed_at"])
        self.assertEqual(narrative["max_chunk_chars"], 2000)
        self.assertEqual(narrative["patch_summary_md"], "patch-summary.md")
        self.assertEqual(narrative["discord_dir"], "discord")
        self.assertEqual(narrative["chunk_count"], 3)
        self.assertEqual(
            narrative["chunks"],
            ["discord/chunk_001.md", "discord/chunk_002.md", "discord/chunk_003.md"],
        )

    def test_triage_stats_included(self):
        self._write_patch_summary()
        self._write_chunks(1)
        self._write_triage_json(deep=5, brief=10, drop=80, ambiguous=3)
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertEqual(
            narrative["triage"],
            {
                "deep": 5, "brief": 10, "drop": 80, "ambiguous": 3,
                "total_bundles": 98, "resolved_by_assessor": 0,
            },
        )

    def test_triage_stats_include_assessor_resolution_count(self):
        self._write_triage_json(deep=2, brief=1, drop=1, ambiguous=0, resolved_by_assessor=4)
        um.main([str(self.out_dir)])
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertEqual(narrative["triage"]["resolved_by_assessor"], 4)

    def test_missing_triage_json_yields_null_triage(self):
        self._write_patch_summary()
        um.main([str(self.out_dir)])
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertIsNone(narrative["triage"])

    def test_malformed_triage_json_yields_null_triage_not_a_crash(self):
        work_dir = self.out_dir / "work"
        work_dir.mkdir(exist_ok=True)
        (work_dir / "triage.json").write_text("{not valid json")
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertIsNone(narrative["triage"])

    def test_paths_relative_to_out_dir(self):
        self._write_patch_summary()
        self._write_chunks(1)
        um.main([str(self.out_dir)])
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertFalse(narrative["patch_summary_md"].startswith("/"))
        for chunk in narrative["chunks"]:
            self.assertFalse(chunk.startswith("/"))

    def test_idempotent_rerun(self):
        self._write_patch_summary()
        self._write_chunks(2)
        self._write_triage_json(deep=1, brief=1, drop=1, ambiguous=0)

        um.main([str(self.out_dir)])
        first = json.loads((self.out_dir / "manifest.json").read_text())
        um.main([str(self.out_dir)])
        second = json.loads((self.out_dir / "manifest.json").read_text())

        # completed_at legitimately ticks forward on every run; compare
        # everything else byte-for-byte.
        first_narrative = dict(first["stages"]["narrative"])
        second_narrative = dict(second["stages"]["narrative"])
        del first_narrative["completed_at"]
        del second_narrative["completed_at"]
        self.assertEqual(first_narrative, second_narrative)
        self.assertEqual(first["stages"]["mechanical"], second["stages"]["mechanical"])
        self.assertEqual(first["inputs"], second["inputs"])

    def test_leaves_other_manifest_sections_untouched(self):
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        manifest = json.loads((self.out_dir / "manifest.json").read_text())
        self.assertEqual(manifest["patch_date"], "2026-07-03")
        self.assertEqual(manifest["inputs"]["old_token"], "20260626")
        self.assertEqual(manifest["counts"], {"added": 1, "changed": 2, "removed": 0})
        self.assertEqual(manifest["stages"]["mechanical"]["completed_at"], "2026-07-03T00:00:00Z")

    def test_custom_max_chunk_chars(self):
        rc = um.main([str(self.out_dir), "--max-chunk-chars", "1500"])
        self.assertEqual(rc, 0)
        narrative = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]
        self.assertEqual(narrative["max_chunk_chars"], 1500)


if __name__ == "__main__":
    unittest.main()
