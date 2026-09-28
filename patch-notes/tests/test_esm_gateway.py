#!/usr/bin/env python3
"""Tests for pn/esm_gateway.py.

Covers:
  - The `esm batch` protocol, against a stub executable that logs each
    request line and answers from a scripted list (request shapes, ok/err
    envelopes, one process serving many requests, restart after exit).
  - `EsmGateway.diff`/`build_diff_cmd`/`find_esm_binary`, exercised against a
    tiny shell-script stand-in for the `esm` binary.

`FakeGateway` (the fixture-backed test double) lives in
`tests/fake_gateway.py`, with its own tests in
`tests/test_fake_gateway.py`.

Every test above uses only synthetic fixtures/stubs. `RealEsmIntegrationTests`
at the bottom is the one exception: it drives `EsmGateway` end-to-end against
the real `esm` binary, gated on `$FO76_ESM_PATH` exactly like `tests/diff.rs`'s
`RUST_TEST_ESM_A`/`RUST_TEST_ESM_B` gate the Rust side -- it skips silently
(via `setUpClass` raising `SkipTest`) when unset, so it is a no-op in
CI/sandboxes without game data.
"""

from __future__ import annotations

import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any, cast
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "pn"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import esm_gateway  # noqa: E402
from builders import TempDirTestCase, fake_esm_script  # noqa: E402
from esm_gateway import (  # noqa: E402
    EsmError,
    EsmGateway,
    formid_to_hex,
    formid_to_int,
)

# ─── Stub `esm batch` ─────────────────────────────────────────────────────────

# Logs every request line to $STUB_LOG and answers request N with entry N of
# the JSON list in $STUB_RESPONSES; past the end of that list it exits
# without answering, like an `esm` that crashed.
_STUB_BATCH = """#!/usr/bin/env python3
import json, os, sys
assert sys.argv[1:] == ["batch"], sys.argv
responses = json.load(open(os.environ["STUB_RESPONSES"]))
with open(os.environ["STUB_LOG"], "a") as log:
    for i, line in enumerate(sys.stdin):
        log.write(json.dumps({"pid": os.getpid(), "request": json.loads(line)}) + "\\n")
        log.flush()
        if i >= len(responses):
            sys.exit(3)
        sys.stdout.write(json.dumps(responses[i]) + "\\n")
        sys.stdout.flush()
"""


# ─── Wire-format tests ───────────────────────────────────────────────────────


class WireFormatTests(TempDirTestCase):
    def setUp(self):
        super().setUp()
        self.client: EsmGateway | None = None
        self.log = self.tmp / "requests.log"
        stub = self.tmp / "esm"
        stub.write_text(_STUB_BATCH)
        stub.chmod(stub.stat().st_mode | stat.S_IEXEC)
        self.stub = stub
        env = mock.patch.dict(
            os.environ,
            {"STUB_LOG": str(self.log), "STUB_RESPONSES": str(self.tmp / "responses.json")},
        )
        env.start()
        self.addCleanup(env.stop)

    def tearDown(self):
        if self.client is not None:
            self.client.close()
        super().tearDown()

    def _client(self, responses: list) -> EsmGateway:
        (self.tmp / "responses.json").write_text(json.dumps(responses))
        self.client = EsmGateway(self.stub)
        return self.client

    def _requests(self) -> list[dict]:
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def _bodies(self) -> list[dict]:
        return [entry["request"] for entry in self._requests()]

    def test_refs_request_shape(self):
        client = self._client([{"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}}])
        result = client.refs("/data/SeventySix.esm", 0x00100001, depth=2, limit=0)
        self.assertEqual(result["target"], "0x00100001")

        self.assertEqual(
            self._bodies()[0],
            {
                "esm": "/data/SeventySix.esm",
                "op": {
                    "op": "referenced_by",
                    "sel": {"kind": "form_id", "value": 0x00100001},
                    "limit": 0,
                    "depth": 2,
                },
            },
        )

    def test_refs_accepts_hex_string_formid(self):
        client = self._client([{"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}}])
        client.refs("/data/x.esm", "0x00100001", depth=1)
        body = self._bodies()[0]
        self.assertEqual(body["op"]["sel"], {"kind": "form_id", "value": 0x00100001})

    def test_refs_request_shape_omits_type_filter_and_paths_by_default(self):
        # Same assertion as test_refs_request_shape but named to make the
        # backward-compat guarantee explicit: callers that never pass
        # type_filter/paths get the exact pre-existing wire shape.
        client = self._client([{"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}}])
        client.refs("/data/x.esm", 0x00100001, depth=2, limit=0)
        body = self._bodies()[0]["op"]
        self.assertNotIn("type_filter", body)
        self.assertNotIn("paths", body)

    def test_refs_request_shape_with_type_filter_and_paths(self):
        client = self._client([{"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}}])
        client.refs("/data/x.esm", 0x00100001, depth=1, limit=25, type_filter="SPEL", paths=True)
        body = self._bodies()[0]["op"]
        self.assertEqual(
            body,
            {
                "op": "referenced_by",
                "sel": {"kind": "form_id", "value": 0x00100001},
                "limit": 25,
                "depth": 1,
                "type_filter": "SPEL",
                "paths": True,
            },
        )

    def test_bulk_get_request_shape_mixed_formid_and_edid(self):
        client = self._client([{"status": "ok", "data": []}])
        client.bulk_get("/data/x.esm", [0x463F, "0x00100010", "AssaultRifle"], resolve="stub")
        body = self._bodies()[0]["op"]
        self.assertEqual(
            body,
            {
                "op": "record_bulk",
                "sels": [
                    {"kind": "form_id", "value": 0x463F},
                    {"kind": "form_id", "value": 0x00100010},
                    {"kind": "edid", "value": "AssaultRifle"},
                ],
                "depth": "stub",
            },
        )

    def test_bulk_get_returns_entries_verbatim(self):
        entries = [
            {"sel": "0x0000463F", "header": {}, "editor_id": "Foo", "fields": {}},
            {"sel": "0xDEADBEEF", "error": "FormID 0xDEADBEEF not found"},
        ]
        client = self._client([{"status": "ok", "data": entries}])
        result = client.bulk_get("/data/x.esm", [0x463F, 0xDEADBEEF])
        self.assertEqual(result, entries)

    def test_record_request_shape(self):
        client = self._client([{"status": "ok", "data": {"header": {}, "editor_id": "WEAP_TestRifle", "fields": {}}}])
        client.record("/data/x.esm", 0x463F, resolve="full")
        body = self._bodies()[0]
        self.assertEqual(
            body["op"],
            {"op": "record", "sel": {"kind": "form_id", "value": 0x463F}, "depth": "full"},
        )

    def test_record_by_edid_request_shape(self):
        client = self._client([{"status": "ok", "data": {"header": {}, "editor_id": "Foo", "fields": {}}}])
        client.record_by_edid("/data/x.esm", "AssaultRifle")
        body = self._bodies()[0]
        self.assertEqual(
            body["op"],
            {"op": "record", "sel": {"kind": "edid", "value": "AssaultRifle"}, "depth": "stub"},
        )

    def test_search_request_shape(self):
        client = self._client([{"status": "ok", "data": []}])
        client.search("/data/x.esm", "*Rifle*", record_type="WEAP", limit=50, field="name")
        body = self._bodies()[0]
        self.assertEqual(
            body["op"],
            {"op": "search", "pattern": "*Rifle*", "types": ["WEAP"], "field": "name", "limit": 50},
        )

    def test_list_type_request_shape(self):
        client = self._client([{"status": "ok", "data": []}])
        client.list_type("/data/x.esm", "OMOD", offset=5, limit=10)
        body = self._bodies()[0]
        self.assertEqual(
            body["op"],
            {"op": "list_type_records", "sig": "OMOD", "offset": 5, "limit": 10},
        )

    def test_ok_envelope_returns_data(self):
        client = self._client([{"status": "ok", "data": {"hello": "world"}}])
        self.assertEqual(client.file_info("/data/x.esm"), {"hello": "world"})

    def test_err_envelope_raises_esm_error(self):
        client = self._client([{"status": "err", "error": "EditorID 'Nope' not found"}])
        with self.assertRaises(EsmError) as ctx:
            client.record_by_edid("/data/x.esm", "Nope")
        self.assertIn("Nope", str(ctx.exception))

    def test_process_exiting_without_an_answer_raises(self):
        client = self._client([])
        with self.assertRaises(EsmError) as ctx:
            client.file_info("/data/x.esm")
        self.assertIn("exited", str(ctx.exception))

    def test_exists_true_and_false(self):
        client = self._client(
            [
                {"status": "ok", "data": {"header": {}, "editor_id": "X", "fields": {}}},
                {"status": "err", "error": "FormID 0xDEADBEEF not found"},
            ]
        )
        self.assertTrue(client.exists("/data/x.esm", 0x1))
        self.assertFalse(client.exists("/data/x.esm", 0xDEADBEEF))

    def test_one_process_answers_every_request(self):
        client = self._client(
            [{"status": "ok", "data": {"a": 1}}, {"status": "ok", "data": {"b": 2}}]
        )
        self.assertEqual(client.file_info("/data/x.esm"), {"a": 1})
        self.assertEqual(client.file_info("/data/y.esm"), {"b": 2})
        requests = self._requests()
        self.assertEqual(len({entry["pid"] for entry in requests}), 1)
        self.assertEqual([entry["request"]["esm"] for entry in requests], ["/data/x.esm", "/data/y.esm"])

    def test_restarts_the_process_after_it_exits(self):
        client = self._client([{"status": "ok", "data": {"a": 1}}])
        client.file_info("/data/x.esm")
        with self.assertRaises(EsmError):
            client.file_info("/data/x.esm")
        self.assertEqual(client.file_info("/data/x.esm"), {"a": 1})
        self.assertEqual(len({entry["pid"] for entry in self._requests()}), 2)


# ─── FormID helper tests ─────────────────────────────────────────────────────


class FormIdHelperTests(unittest.TestCase):
    def test_formid_to_int_accepts_hex_string(self):
        self.assertEqual(formid_to_int("0x00463F"), 0x463F)
        self.assertEqual(formid_to_int("0X00463F"), 0x463F)

    def test_formid_to_int_accepts_int(self):
        self.assertEqual(formid_to_int(0x463F), 0x463F)

    def test_formid_to_int_reads_bare_all_digit_token_as_hex_first(self):
        # Mirrors src/formid.rs's parse_formid: a bare token is hex first,
        # even one that's also plain-decimal-looking. "00568635" is hex
        # 0x00568635, not decimal 568635.
        self.assertEqual(formid_to_int("00568635"), 0x00568635)
        self.assertEqual(formid_to_int("18000"), 0x18000)

    def test_formid_to_int_accepts_bare_hex_with_letters(self):
        # Previously raised: formid_to_int had no bare-hex branch at all.
        self.assertEqual(formid_to_int("463F"), 0x463F)
        self.assertEqual(formid_to_int("DEADBEEF"), 0xDEADBEEF)

    def test_formid_to_int_falls_through_to_decimal_past_8_digits(self):
        self.assertEqual(formid_to_int("123456789"), 123456789)

    def test_formid_to_hex_matches_rust_display_format(self):
        # src/formid.rs: `format!("0x{:08X}", self.0)` -- uppercase, 8 digits.
        self.assertEqual(formid_to_hex(0x463F), "0x0000463F")
        self.assertEqual(formid_to_hex(0x00ABCDEF), "0x00ABCDEF")
        self.assertEqual(formid_to_hex("0x00abcdef"), "0x00ABCDEF")


# ─── find_esm_binary tests ───────────────────────────────────────────────────


class FindEsmBinaryTests(unittest.TestCase):
    def test_explicit_non_executable_path_raises(self):
        with tempfile.TemporaryDirectory() as tmp:
            not_exec = Path(tmp) / "esm"
            not_exec.write_text("not executable")
            with self.assertRaises(EsmError):
                esm_gateway.find_esm_binary(str(not_exec))

    def test_explicit_executable_path_is_returned(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "esm"
            exe.write_text("#!/bin/sh\n")
            exe.chmod(exe.stat().st_mode | stat.S_IEXEC)
            self.assertEqual(esm_gateway.find_esm_binary(str(exe)), exe)

    def test_nothing_found_raises(self):
        with mock.patch.object(esm_gateway, "ESM_CRATE_DIR", Path("/nonexistent-esm-crate")):
            with mock.patch("shutil.which", return_value=None):
                with self.assertRaises(EsmError):
                    esm_gateway.find_esm_binary(None)


# ─── build_diff_cmd / EsmGateway.diff tests ─────────────────────────────────


class BuildDiffCmdTests(unittest.TestCase):
    def _cmd(self, **overrides: Any):
        kwargs: dict[str, Any] = dict(
            lang="en", strings_dir_a=None, strings_dir_b=None, record_type=None,
            bodies="full", keep_noise=False, exclude_type="LAND,NAVM",
        )
        kwargs.update(overrides)
        return esm_gateway.build_diff_cmd(
            Path("esm"), Path("a.esm"), Path("b.esm"), **cast(Any, kwargs)
        )

    # The flag-mapping cases (--exclude-type, --strings-dir, --bodies, --type,
    # --keep-noise, --pretty) live in test_orchestrator.TestBuildDiffCmd: it
    # calls the same function object via make_patch_notes' re-export of
    # esm_gateway.build_diff_cmd, and its strings-dir case asserts strictly
    # more than this one did.

    def test_runs_the_diff_subcommand(self):
        # diff() shells out to `esm diff` -- see EsmGateway.diff's docstring
        # for why it bypasses the `esm batch` child.
        cmd = self._cmd()
        self.assertEqual(cmd[1], "diff")


class EsmGatewayDiffTests(TempDirTestCase):
    """Exercises `EsmGateway.diff` directly against a tiny shell-script
    stand-in for the `esm` binary -- same technique as
    tests/test_orchestrator.py's `make_fake_esm`, at the transport
    layer this delegates to."""

    def _fake_esm(self, stdout_text: str, *, exit_code: int = 0) -> Path:
        # exit_code is always spelled out here (unlike make_fake_esm, which
        # lets the script inherit `cat`'s status) because these tests need a
        # binary that fails on demand.
        return fake_esm_script(self.tmp, stdout_text=stdout_text, exit_code=exit_code)

    def _diff(self, esm_bin):
        return esm_gateway.EsmGateway.diff(
            esm_bin, Path("a.esm"), Path("b.esm"),
            strings_dir_a=Path("/strings"), strings_dir_b=Path("/strings"),
            lang="en", record_type=None, bodies="full", keep_noise=False,
            exclude_type="LAND,NAVM",
        )

    def test_parses_json(self):
        fake_esm = self._fake_esm('{"added": [], "removed": [], "changed": []}')
        result = self._diff(fake_esm)
        self.assertEqual(result.data, {"added": [], "removed": [], "changed": []})
        self.assertEqual(result.raw_json, '{"added": [], "removed": [], "changed": []}')

    def test_trailing_garbage_after_json_is_rejected(self):
        # Previously tolerated via `raw_decode` as a workaround for a CLI bug
        # where a subcommand ran and then fell through into the REPL, which
        # wrote its `esm> ` prompt to stdout right after the JSON. That bug
        # was fixed at the CLI level (a subcommand always exits after
        # running) and the REPL has since been removed entirely, so this is
        # no longer an `esm` quirk to route around -- any bytes trailing the
        # JSON blob are a hard error now, same as invalid JSON outright.
        fake_esm = self._fake_esm('{"added": [], "removed": [], "changed": []}esm> ')
        with self.assertRaises(EsmError):
            self._diff(fake_esm)

    def test_cmd_reflects_argv_used(self):
        fake_esm = self._fake_esm("{}")
        result = self._diff(fake_esm)
        self.assertEqual(result.cmd[0], str(fake_esm))
        self.assertEqual(result.cmd[1], "diff")
        self.assertIn("--strings-dir", result.cmd)

    def test_nonzero_exit_raises_esm_error(self):
        fake_esm = self._fake_esm("irrelevant", exit_code=1)
        with self.assertRaises(EsmError) as ctx:
            self._diff(fake_esm)
        self.assertIn("exit code 1", str(ctx.exception))

    def test_invalid_json_raises_esm_error(self):
        fake_esm = self._fake_esm("not json at all")
        with self.assertRaises(EsmError):
            self._diff(fake_esm)


# ─── Real-ESM integration test (env-gated, silent no-op without game data) ──


class RealEsmIntegrationTests(unittest.TestCase):
    """End-to-end smoke test of `EsmGateway` against the real `esm` binary
    -- no fixtures, no stub.

    Gated on `$FO76_ESM_PATH` (an absolute path to a real `SeventySix.esm`,
    ), mirroring `tests/diff.rs`'s
    `RUST_TEST_ESM_A`/`RUST_TEST_ESM_B` silent-skip convention on the Rust
    side. Skips (not fails) in any environment without real game data --
    this must be a no-op in CI/sandboxes.

    Deliberately does not exercise `EsmGateway.diff` here: `diff` needs a
    *second* snapshot with strings resolvable by the Rust CLI's exact
    `<esm-stem>_<lang>.strings` match (see `cli.rs::resolve_localization_or_bail`),
    which not every `$FO76_DATA_DIR` snapshot layout satisfies (e.g. an
    undated `SeventySix.esm` next to date-stamped `SeventySix_<date>_en.strings`
    -- a real, pre-existing mismatch between `make_patch_notes.py`'s lenient
    glob-based `locate_strings_dirs` and the Rust CLI's strict stem match,
    unrelated to this refactor). `bulk_get`/`refs`/`record`/`file_info` need
    no strings and are exercised below.
    """

    esm_path: str
    esm_bin: Path
    gateway: EsmGateway

    @classmethod
    def setUpClass(cls):
        esm_path = os.environ.get("FO76_ESM_PATH")
        if not esm_path or not Path(esm_path).is_file():
            raise unittest.SkipTest(
                "FO76_ESM_PATH not set (or not a file) -- skipping real-ESM integration test"
            )
        cls.esm_path = esm_path
        try:
            cls.esm_bin = esm_gateway.find_esm_binary(None)
        except EsmError as exc:
            raise unittest.SkipTest(f"esm binary not found -- skipping: {exc}")
        cls.gateway = EsmGateway(cls.esm_bin)

    @classmethod
    def tearDownClass(cls):
        gateway = getattr(cls, "gateway", None)
        if gateway is not None:
            gateway.close()

    def test_file_info_returns_the_esm_path(self):
        info = self.gateway.file_info(self.esm_path)
        self.assertEqual(Path(info["path"]).resolve(), Path(self.esm_path).resolve())
        self.assertGreater(info["record_count"], 0)

    def test_record_lookup_by_formid(self):
        # Any real ESM has a TES4 header record at 0x00000000... use search
        # instead of a hardcoded FormID, since specific FormIDs are not
        # guaranteed stable across snapshots.
        results = self.gateway.search(self.esm_path, "*", record_type="OMOD", limit=1)
        self.assertTrue(results, "expected at least one OMOD record in the ESM")
        formid = results[0]["form_id"]
        rec = self.gateway.record(self.esm_path, formid, resolve="none")
        self.assertEqual(rec["header"]["form_id"], formid)

    def test_bulk_get_isolates_a_bad_selector_among_good_ones(self):
        good = self.gateway.search(self.esm_path, "*", record_type="OMOD", limit=2)
        self.assertGreaterEqual(len(good), 2, "expected at least two OMOD records in the ESM")
        targets = [good[0]["form_id"], good[1]["form_id"], "0xFFFFFFF0"]
        entries = self.gateway.bulk_get(self.esm_path, targets, resolve="stub")
        self.assertEqual(len(entries), 3)
        self.assertEqual(entries[0]["sel"], good[0]["form_id"])
        self.assertNotIn("error", entries[0])
        self.assertIsNotNone(entries[0]["fields"])
        self.assertEqual(entries[2]["sel"], "0xFFFFFFF0")
        self.assertIn("error", entries[2])

    def test_refs_with_type_filter_and_paths_matches_a_real_omod_keyword(self):
        # An OMOD's Data.Properties[] Value 1 forward-references a KYWD --
        # walk any OMOD's first Keywords-typed property back to find it, then
        # confirm the reverse walk (type_filter="OMOD", paths=True) rediscovers
        # this exact OMOD with a field_paths entry pointing back at that
        # property (see chase/chase.py's keyword_hook pattern, which this
        # capability was added for).
        omods = self.gateway.search(self.esm_path, "*", record_type="OMOD", limit=25)
        self.assertTrue(omods)
        for stub in omods:
            rec = self.gateway.record(self.esm_path, stub["form_id"], resolve="stub")
            props = ((rec.get("fields") or {}).get("Data") or {}).get("Properties") or []
            kywd_targets = [
                p["Value 1"]
                for p in props
                if isinstance(p.get("Value 1"), dict) and p["Value 1"].get("record_type") == "KYWD"
            ]
            if not kywd_targets:
                continue
            kywd_fid = kywd_targets[0]["formid"]
            result = self.gateway.refs(
                self.esm_path, kywd_fid, depth=1, limit=10, type_filter="OMOD", paths=True
            )
            rows = result["rows"]
            self.assertTrue(rows)
            matching = [r for r in rows if r["form_id"] == stub["form_id"]]
            self.assertTrue(matching, "the OMOD itself must show up as a type_filter=OMOD referencer of its own KYWD")
            self.assertTrue(matching[0].get("field_paths"), "paths=True must annotate the field path")
            return
        self.skipTest("no OMOD in this ESM has a Keywords-typed property to test with")

    def test_exists_true_and_false(self):
        omods = self.gateway.search(self.esm_path, "*", record_type="OMOD", limit=1)
        self.assertTrue(omods)
        self.assertTrue(self.gateway.exists(self.esm_path, omods[0]["form_id"]))
        self.assertFalse(self.gateway.exists(self.esm_path, "0xFFFFFFF0"))


if __name__ == "__main__":
    unittest.main()
