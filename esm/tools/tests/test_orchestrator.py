#!/usr/bin/env python3
"""Tests for tools/make_patch_notes.py (mechanical-stage orchestrator) and
tools/update_manifest.py (narrative-stage manifest updater).

The orchestrator end-to-end tests never spawn a real `esm`/`esm-server`: the
diff step is satisfied by a tiny generated shell script that ignores its
arguments and `cat`s `tools/tests/fixtures/diff_small.json` followed by the
`--local` REPL's trailing `esm> ` prompt (exercising make_patch_notes.py's
`json.JSONDecoder().raw_decode`-based tolerance of that prompt), and the
bundles/lints stages run against `esm_daemon.FakeClient` backed by
`tools/tests/fixtures/refs_graph.json` (`--offline --refs-fixture`). No real
daemon or ESM is touched.
"""

from __future__ import annotations

import json
import shutil
import stat
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import make_patch_notes as mpn  # noqa: E402
import patchnotes_lib as pl  # noqa: E402
import update_manifest as um  # noqa: E402

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
DIFF_SMALL = FIXTURES_DIR / "diff_small.json"
REFS_GRAPH = FIXTURES_DIR / "refs_graph.json"


# ---------------------------------------------------------------------------
# Shared fixture builders
# ---------------------------------------------------------------------------

_FAKE_ESM_TEMPLATE = "#!/bin/sh\ncat \"{diff_json}\"\nprintf 'esm> '\n"


def make_fake_esm(tmp_dir: Path, diff_json: Path = DIFF_SMALL) -> Path:
    """A tiny shell-script stand-in for the `esm` binary: ignores every
    argument and cats a fixed diff.json fixture, followed by the `--local`
    REPL's trailing prompt string -- exercises make_patch_notes.py's
    raw_decode()-based tolerance of that prompt."""
    script = tmp_dir / "fake_esm.sh"
    script.write_text(_FAKE_ESM_TEMPLATE.format(diff_json=diff_json))
    mode = script.stat().st_mode
    script.chmod(mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    return script


def make_snapshot(tmp_dir: Path, token: str, lang: str = "en") -> Path:
    """A dummy `<tmp_dir>/<token>/SeventySix_<token>.esm` plus a sibling
    `strings/` dir holding a matching `*_en.strings` stub -- satisfies
    make_patch_notes.locate_strings_dirs' per-side auto-detect (see
    has_any_strings(): `*{tok}*_{lang}.strings`)."""
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
        # itself (see CLAUDE.local.md's $FO76_DATA_DIR/<snapshot>/SeventySix.esm).
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
# Unit: esm-diff command construction (--exclude-type default/disable, etc.)
# ---------------------------------------------------------------------------


class TestBuildDiffCmd(unittest.TestCase):
    def _cmd(self, **overrides):
        kwargs = dict(
            lang="en",
            strings_dir_a=None,
            strings_dir_b=None,
            record_type=None,
            bodies="full",
            keep_noise=False,
            exclude_type="LAND,NAVM",
        )
        kwargs.update(overrides)
        return mpn.build_diff_cmd(Path("esm"), Path("a.esm"), Path("b.esm"), **kwargs)

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

    def test_shared_strings_dir_uses_single_flag(self):
        d = Path("/strings")
        cmd = self._cmd(strings_dir_a=d, strings_dir_b=d)
        self.assertIn("--strings-dir", cmd)
        self.assertNotIn("--strings-dir-a", cmd)
        self.assertNotIn("--strings-dir-b", cmd)

    def test_differing_strings_dirs_use_per_side_flags(self):
        cmd = self._cmd(strings_dir_a=Path("/a"), strings_dir_b=Path("/b"))
        self.assertIn("--strings-dir-a", cmd)
        self.assertIn("--strings-dir-b", cmd)
        self.assertNotIn("--strings-dir", cmd)

    def test_argparse_default_exclude_type(self):
        args = mpn.build_arg_parser().parse_args(["a.esm", "b.esm"])
        self.assertEqual(args.exclude_type, mpn.DEFAULT_EXCLUDE_TYPE)
        self.assertEqual(args.bodies, "full")

    def test_argparse_exclude_type_disable(self):
        args = mpn.build_arg_parser().parse_args(["a.esm", "b.esm", "--exclude-type", ""])
        self.assertEqual(args.exclude_type, "")


# ---------------------------------------------------------------------------
# End-to-end: make_patch_notes.main() against the fake esm binary + FakeClient
# ---------------------------------------------------------------------------


class TestOrchestratorEndToEnd(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp_dir = Path(self._tmp.name)
        self.fake_esm = make_fake_esm(self.tmp_dir)
        self.old_esm = make_snapshot(self.tmp_dir, "20260626")
        self.new_esm = make_snapshot(self.tmp_dir, "20260703")

    def tearDown(self):
        self._tmp.cleanup()

    def _run(self, out_dir, extra_args=()):
        return mpn.main([
            str(self.old_esm), str(self.new_esm),
            "--esm-bin", str(self.fake_esm),
            "--offline", "--refs-fixture", str(REFS_GRAPH),
            "--out-dir", str(out_dir),
            *extra_args,
        ])

    def test_full_run_writes_all_files(self):
        out_dir = self.tmp_dir / "out"
        self.assertEqual(self._run(out_dir), 0)
        for fname in (
            "diff.json", "comprehensive.json", "comprehensive.md",
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
        self.assertEqual(inputs["pipeline_version"], pl.SCHEMA_VERSION)

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
        self.assertIn("uncategorized", counts)
        self.assertEqual(set(counts["lints"].keys()), {"error", "warn", "info"})

    def test_manifest_stages_shape(self):
        out_dir = self.tmp_dir / "out"
        self._run(out_dir)
        manifest = json.loads((out_dir / "manifest.json").read_text())
        mech = manifest["stages"]["mechanical"]
        self.assertIsNotNone(mech["completed_at"])
        self.assertEqual(
            set(mech["files"].values()),
            {"diff.json", "comprehensive.json", "comprehensive.md", "bundles.json", "lints.json"},
        )
        narrative = manifest["stages"]["narrative"]
        self.assertIsNone(narrative["completed_at"])
        self.assertEqual(narrative["categories"], [])
        self.assertEqual(narrative["max_chunk_chars"], 2000)

    def test_default_out_dir_used_when_not_given(self):
        rc = mpn.main([
            str(self.old_esm), str(self.new_esm),
            "--esm-bin", str(self.fake_esm),
            "--offline", "--refs-fixture", str(REFS_GRAPH),
        ])
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

    def test_offline_without_fixture_is_input_validation_error(self):
        out_dir = self.tmp_dir / "out_bad"
        with self.assertRaises(SystemExit) as cm:
            mpn.main([
                str(self.old_esm), str(self.new_esm),
                "--esm-bin", str(self.fake_esm),
                "--offline",
                "--out-dir", str(out_dir),
            ])
        self.assertEqual(cm.exception.code, 1)

    def test_missing_strings_dir_is_input_validation_error(self):
        # A fresh, strings-less snapshot pair -> locate_strings_dirs must
        # fail loud with exit code 1 (never silently diff without strings).
        bare_dir = self.tmp_dir / "bare"
        bare_dir.mkdir()
        old_bare = bare_dir / "old_20260101.esm"
        old_bare.write_bytes(b"X")
        new_bare = bare_dir / "new_20260102.esm"
        new_bare.write_bytes(b"X")
        with self.assertRaises(SystemExit) as cm:
            mpn.main([
                str(old_bare), str(new_bare),
                "--esm-bin", str(self.fake_esm),
                "--offline", "--refs-fixture", str(REFS_GRAPH),
                "--out-dir", str(self.tmp_dir / "out_bare"),
            ])
        self.assertEqual(cm.exception.code, 1)


# ---------------------------------------------------------------------------
# update_manifest.py
# ---------------------------------------------------------------------------


class TestUpdateManifest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.out_dir = Path(self._tmp.name)
        manifest = pl.new_manifest(
            patch_date="2026-07-03",
            old_token="20260626",
            new_token="20260703",
            new_esm_size=123,
            new_esm_mtime=456,
            pipeline_version=pl.SCHEMA_VERSION,
            counts={"added": 1, "changed": 2, "removed": 0},
        )
        manifest["stages"]["mechanical"]["completed_at"] = "2026-07-03T00:00:00Z"
        manifest["stages"]["mechanical"]["files"] = {"diff": "diff.json"}
        pl.write_manifest(self.out_dir, manifest)

    def tearDown(self):
        self._tmp.cleanup()

    def _write_notes(self, slugs):
        notes_dir = self.out_dir / "notes"
        notes_dir.mkdir(exist_ok=True)
        for slug in slugs:
            (notes_dir / f"{slug}.md").write_text(f"# {slug}\n")

    def _write_chunks(self, slug, n):
        chunk_dir = self.out_dir / "discord" / slug
        chunk_dir.mkdir(parents=True, exist_ok=True)
        for i in range(1, n + 1):
            (chunk_dir / f"chunk_{i:03d}.md").write_text(f"chunk {i}")

    def _write_categories_json(self, entries):
        work_dir = self.out_dir / "work"
        work_dir.mkdir(exist_ok=True)
        (work_dir / "categories.json").write_text(
            json.dumps({"schema_version": 1, "categories": entries})
        )

    def test_missing_manifest_errors(self):
        empty = Path(tempfile.mkdtemp())
        try:
            self.assertEqual(um.main([str(empty)]), 1)
        finally:
            shutil.rmtree(empty)

    def test_no_notes_yields_empty_categories(self):
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        manifest = json.loads((self.out_dir / "manifest.json").read_text())
        self.assertEqual(manifest["stages"]["narrative"]["categories"], [])

    def test_categories_sorted_by_post_order_with_labels(self):
        self._write_categories_json([
            {"id": "perks", "label": "Perks & Legendary Perks", "slug": "perks", "post_order": 1},
            {"id": "weapons_combat", "label": "Weapons & Combat Balance", "slug": "weapons_combat", "post_order": 0},
        ])
        self._write_notes(["perks", "weapons_combat"])
        self._write_chunks("perks", 1)
        self._write_chunks("weapons_combat", 2)

        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)

        manifest = json.loads((self.out_dir / "manifest.json").read_text())
        narrative = manifest["stages"]["narrative"]
        self.assertIsNotNone(narrative["completed_at"])
        self.assertEqual(narrative["max_chunk_chars"], 2000)

        cats = narrative["categories"]
        self.assertEqual([c["id"] for c in cats], ["weapons_combat", "perks"])
        self.assertEqual(cats[0]["label"], "Weapons & Combat Balance")
        self.assertEqual(cats[0]["notes_md"], "notes/weapons_combat.md")
        self.assertEqual(cats[0]["discord_dir"], "discord/weapons_combat")
        self.assertEqual(cats[0]["chunk_count"], 2)
        self.assertEqual(
            cats[0]["chunks"],
            ["discord/weapons_combat/chunk_001.md", "discord/weapons_combat/chunk_002.md"],
        )

    def test_titleized_label_when_no_categories_json(self):
        self._write_notes(["camp_workshop"])
        self._write_chunks("camp_workshop", 1)
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        cats = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]["categories"]
        self.assertEqual(cats[0]["label"], "Camp Workshop")

    def test_fallback_sort_by_slug_when_no_post_order(self):
        self._write_notes(["zzz_cat", "aaa_cat"])
        self._write_chunks("zzz_cat", 1)
        self._write_chunks("aaa_cat", 1)
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        cats = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]["categories"]
        self.assertEqual([c["id"] for c in cats], ["aaa_cat", "zzz_cat"])

    def test_known_post_order_sorts_before_unknown_fallback(self):
        self._write_categories_json([
            {"id": "zzz_known", "label": "ZZZ Known", "slug": "zzz_known", "post_order": 5},
        ])
        self._write_notes(["zzz_known", "aaa_unknown"])
        self._write_chunks("zzz_known", 1)
        self._write_chunks("aaa_unknown", 1)
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        cats = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]["categories"]
        # Known post_order (5) sorts before the unknown fallback (inf), even
        # though "aaa_unknown" would win a plain alphabetic sort.
        self.assertEqual([c["id"] for c in cats], ["zzz_known", "aaa_unknown"])

    def test_notes_without_chunks_warned_and_zero_count(self):
        self._write_notes(["orphan"])
        rc = um.main([str(self.out_dir)])
        self.assertEqual(rc, 0)
        cats = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]["categories"]
        self.assertEqual(cats[0]["chunk_count"], 0)
        self.assertEqual(cats[0]["chunks"], [])

    def test_paths_relative_to_out_dir(self):
        self._write_notes(["armor"])
        self._write_chunks("armor", 1)
        um.main([str(self.out_dir)])
        cats = json.loads((self.out_dir / "manifest.json").read_text())["stages"]["narrative"]["categories"]
        for c in cats:
            self.assertFalse(c["notes_md"].startswith("/"))
            self.assertFalse(c["discord_dir"].startswith("/"))
            for chunk in c["chunks"]:
                self.assertFalse(chunk.startswith("/"))

    def test_idempotent_rerun(self):
        self._write_categories_json([{"id": "armor", "label": "Armor", "slug": "armor", "post_order": 0}])
        self._write_notes(["armor"])
        self._write_chunks("armor", 1)

        um.main([str(self.out_dir)])
        first = json.loads((self.out_dir / "manifest.json").read_text())
        um.main([str(self.out_dir)])
        second = json.loads((self.out_dir / "manifest.json").read_text())

        self.assertEqual(
            first["stages"]["narrative"]["categories"],
            second["stages"]["narrative"]["categories"],
        )
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


if __name__ == "__main__":
    unittest.main()
