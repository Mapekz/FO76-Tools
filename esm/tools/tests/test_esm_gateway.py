#!/usr/bin/env python3
"""Tests for tools/esm_gateway.py.

Covers:
  - Wire-format correctness against a stdlib `http.server`-based stub daemon
    (request path, auth header, JSON body shape, ok/err envelope handling,
    keep-alive connection reuse, reconnect-after-close retry).
  - Discovery-path resolution (`runtime_dir` / `read_daemon_info`) with
    monkeypatched environment variables pointing at a temp dir.
  - `FakeGateway`'s BFS reverse-reference walk against the checked-in fixture
    (depth-1 vs depth-3 expansion, cycle safety, path/depth fields, int vs
    hex FormID acceptance).
  - `EsmGateway.diff`/`build_diff_cmd`/`find_esm_binary`, exercised against a
    tiny shell-script stand-in for the `esm` binary.

Every test above uses only synthetic fixtures/stubs -- no real daemon or game
data. `RealEsmIntegrationTests` at the bottom is the one exception: it drives
`EsmGateway` end-to-end against the real `esm` binary and a live warm daemon,
gated on `$FO76_ESM_PATH` (see esm/CLAUDE.local.md) exactly like
`tests/diff.rs`'s `RUST_TEST_ESM_A`/`RUST_TEST_ESM_B` gate the Rust side --
it skips silently (via `setUpClass` raising `SkipTest`) when unset, so it is
a no-op in CI/sandboxes without game data.
"""

from __future__ import annotations

import http.server
import json
import os
import stat
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from typing import Any, cast
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import esm_gateway  # noqa: E402
from esm_gateway import (  # noqa: E402
    DaemonError,
    EsmGateway,
    FakeGateway,
    daemon_fresh,
    daemon_info_path,
    formid_to_hex,
    formid_to_int,
    read_daemon_info,
    runtime_dir,
)

FIXTURE_PATH = Path(__file__).resolve().parent / "fixtures" / "refs_graph.json"


# ─── Stub HTTP daemon ────────────────────────────────────────────────────────


class _StubHandler(http.server.BaseHTTPRequestHandler):
    """Minimal stand-in for esm-server's daemon router (build_daemon_router
    in src/bin/server.rs): /health (auth-gated 200), /op (auth-gated, echoes
    back a scripted response keyed by request body), and request-log capture
    for assertions."""

    protocol_version = "HTTP/1.1"  # keep-alive, so we can test connection reuse

    # Populated per-test via class attributes (see `_serve_with` below).
    token = "test-token-abc123"
    op_responses: list = []
    requests_seen: list = []
    close_after_n: int | None = None  # force-close connection after N requests

    def log_message(self, format: str, *args):  # silence default stderr logging
        pass

    def _check_auth(self) -> bool:
        return self.headers.get("Authorization") == f"Bearer {self.token}"

    def do_GET(self):
        if self.path == "/health":
            if not self._check_auth():
                self._send_json(401, {"error": "invalid or missing bearer token"})
                return
            self._send_json(200, {})
            return
        self._send_json(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/op":
            self._send_json(404, {"error": "not found"})
            return

        length = int(self.headers.get("Content-Length", 0))
        raw_body = self.rfile.read(length)

        type(self).requests_seen.append(
            {
                "path": self.path,
                "authorization": self.headers.get("Authorization"),
                "content_type": self.headers.get("Content-Type"),
                "body": json.loads(raw_body.decode("utf-8")) if raw_body else None,
            }
        )

        if not self._check_auth():
            self._send_json(401, {"error": "invalid or missing bearer token"})
            return

        idx = len(type(self).requests_seen) - 1
        responses = type(self).op_responses
        status, payload = responses[min(idx, len(responses) - 1)]

        force_close = (
            type(self).close_after_n is not None
            and len(type(self).requests_seen) == type(self).close_after_n
        )
        self._send_json(status, payload, force_close=force_close)

    def _send_json(self, status: int, payload: dict, force_close: bool = False) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        if force_close:
            self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        if force_close:
            self.close_connection = True


class _StubServer:
    """Runs `_StubHandler` on a background thread on 127.0.0.1:<ephemeral>."""

    def __init__(self, op_responses: list, token: str = "test-token-abc123"):
        handler = type(
            "ScopedHandler",
            (_StubHandler,),
            {"op_responses": list(op_responses), "requests_seen": [], "token": token},
        )
        self.handler = handler
        self.httpd = http.server.HTTPServer(("127.0.0.1", 0), handler)
        self.port = self.httpd.server_address[1]
        self.token = token
        self._thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self._thread.start()

    @property
    def requests_seen(self) -> list:
        return self.handler.requests_seen

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()
        self._thread.join(timeout=5)


# ─── Wire-format tests ───────────────────────────────────────────────────────


class WireFormatTests(unittest.TestCase):
    def setUp(self):
        self.server: _StubServer | None = None
        self.client: EsmGateway | None = None

    def tearDown(self):
        if self.client is not None:
            self.client.close()
        if self.server is not None:
            self.server.stop()

    def _client(self, op_responses: list, token: str = "test-token-abc123") -> EsmGateway:
        self.server = _StubServer(op_responses, token=token)
        self.client = EsmGateway(self.server.port, token)
        return self.client

    def _require_server(self) -> _StubServer:
        assert self.server is not None
        return self.server

    def test_refs_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}})])
        result = client.refs("/data/SeventySix.esm", 0x00100001, depth=2, limit=0)
        self.assertEqual(result["target"], "0x00100001")

        req = self._require_server().requests_seen[0]
        self.assertEqual(req["path"], "/op")
        self.assertEqual(req["authorization"], "Bearer test-token-abc123")
        self.assertEqual(req["content_type"], "application/json")
        self.assertEqual(
            req["body"],
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
        client = self._client([(200, {"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}})])
        client.refs("/data/x.esm", "0x00100001", depth=1)
        body = self._require_server().requests_seen[0]["body"]
        self.assertEqual(body["op"]["sel"], {"kind": "form_id", "value": 0x00100001})

    def test_refs_request_shape_omits_type_filter_and_paths_by_default(self):
        # Same assertion as test_refs_request_shape but named to make the
        # backward-compat guarantee explicit: callers that never pass
        # type_filter/paths get the exact pre-existing wire shape.
        client = self._client([(200, {"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}})])
        client.refs("/data/x.esm", 0x00100001, depth=2, limit=0)
        body = self._require_server().requests_seen[0]["body"]["op"]
        self.assertNotIn("type_filter", body)
        self.assertNotIn("paths", body)

    def test_refs_request_shape_with_type_filter_and_paths(self):
        client = self._client([(200, {"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}})])
        client.refs("/data/x.esm", 0x00100001, depth=1, limit=25, type_filter="SPEL", paths=True)
        body = self._require_server().requests_seen[0]["body"]["op"]
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
        client = self._client([(200, {"status": "ok", "data": []})])
        client.bulk_get("/data/x.esm", [0x463F, "0x00100010", "AssaultRifle"], resolve="stub")
        body = self._require_server().requests_seen[0]["body"]["op"]
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
        client = self._client([(200, {"status": "ok", "data": entries})])
        result = client.bulk_get("/data/x.esm", [0x463F, 0xDEADBEEF])
        self.assertEqual(result, entries)

    def test_record_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": {"header": {}, "editor_id": "WEAP_TestRifle", "fields": {}}})])
        client.record("/data/x.esm", 0x463F, resolve="full")
        body = self._require_server().requests_seen[0]["body"]
        self.assertEqual(
            body["op"],
            {"op": "record", "sel": {"kind": "form_id", "value": 0x463F}, "depth": "full"},
        )

    def test_record_by_edid_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": {"header": {}, "editor_id": "Foo", "fields": {}}})])
        client.record_by_edid("/data/x.esm", "AssaultRifle")
        body = self._require_server().requests_seen[0]["body"]
        self.assertEqual(
            body["op"],
            {"op": "record", "sel": {"kind": "edid", "value": "AssaultRifle"}, "depth": "stub"},
        )

    def test_search_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": []})])
        client.search("/data/x.esm", "*Rifle*", record_type="WEAP", limit=50, field="name")
        body = self._require_server().requests_seen[0]["body"]
        self.assertEqual(
            body["op"],
            {"op": "search", "pattern": "*Rifle*", "types": ["WEAP"], "field": "name", "limit": 50},
        )

    def test_ok_envelope_returns_data(self):
        client = self._client([(200, {"status": "ok", "data": {"hello": "world"}})])
        self.assertEqual(client.file_info("/data/x.esm"), {"hello": "world"})

    def test_err_envelope_raises_daemon_error(self):
        client = self._client([(200, {"status": "err", "error": "EditorID 'Nope' not found"})])
        with self.assertRaises(DaemonError) as ctx:
            client.record_by_edid("/data/x.esm", "Nope")
        self.assertIn("Nope", str(ctx.exception))

    def test_non_200_error_body_shape_raises(self):
        # Mirrors ApiError's body shape ({"error": ...}), distinct from the
        # {"status": "ok"/"err"} Response envelope -- e.g. a 401 from check_auth.
        client = self._client([(401, {"error": "invalid or missing bearer token"})], token="right-token")
        # Force a wrong token on the client side to trigger the stub's 401 path.
        client.token = "wrong-token"
        with self.assertRaises(DaemonError) as ctx:
            client.file_info("/data/x.esm")
        self.assertIn("401", str(ctx.exception))

    def test_exists_true_and_false(self):
        client = self._client(
            [
                (200, {"status": "ok", "data": {"header": {}, "editor_id": "X", "fields": {}}}),
                (200, {"status": "err", "error": "FormID 0xDEADBEEF not found"}),
            ]
        )
        self.assertTrue(client.exists("/data/x.esm", 0x1))
        self.assertFalse(client.exists("/data/x.esm", 0xDEADBEEF))

    def test_keep_alive_connection_reused(self):
        client = self._client(
            [
                (200, {"status": "ok", "data": {"a": 1}}),
                (200, {"status": "ok", "data": {"b": 2}}),
            ]
        )
        client.file_info("/data/x.esm")
        conn_after_first = client._conn
        client.file_info("/data/x.esm")
        self.assertIs(client._conn, conn_after_first, "connection object should be reused across calls")
        self.assertEqual(len(self._require_server().requests_seen), 2)

    def test_reconnect_after_server_closes_connection(self):
        client = self._client(
            [
                (200, {"status": "ok", "data": {"a": 1}}),
                (200, {"status": "ok", "data": {"b": 2}}),
            ]
        )
        # Force the stub to close the TCP connection after the first response,
        # simulating an idle/stale keep-alive connection the daemon dropped.
        self._require_server().handler.close_after_n = 1

        client.file_info("/data/x.esm")
        result = client.file_info("/data/x.esm")  # must transparently reconnect
        self.assertEqual(result, {"b": 2})
        self.assertEqual(len(self._require_server().requests_seen), 2)


# ─── Discovery-path tests ────────────────────────────────────────────────────


class DiscoveryPathTests(unittest.TestCase):
    def setUp(self):
        self._env_backup = dict(os.environ)
        self._tmp = tempfile.TemporaryDirectory()

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self._env_backup)
        self._tmp.cleanup()

    def _clear_xdg_env(self):
        for var in ("XDG_RUNTIME_DIR", "XDG_CACHE_HOME", "HOME"):
            os.environ.pop(var, None)

    def test_runtime_dir_prefers_xdg_runtime_dir(self):
        self._clear_xdg_env()
        runtime = Path(self._tmp.name) / "runtime"
        runtime.mkdir()
        os.environ["XDG_RUNTIME_DIR"] = str(runtime)
        os.environ["XDG_CACHE_HOME"] = str(Path(self._tmp.name) / "cache")
        self.assertEqual(runtime_dir(), runtime)

    def test_runtime_dir_falls_back_to_xdg_cache_home(self):
        self._clear_xdg_env()
        cache = Path(self._tmp.name) / "cache"
        os.environ["XDG_CACHE_HOME"] = str(cache)
        self.assertEqual(runtime_dir(), cache)

    def test_runtime_dir_falls_back_to_home_cache(self):
        self._clear_xdg_env()
        home = Path(self._tmp.name) / "home"
        home.mkdir()
        os.environ["HOME"] = str(home)
        self.assertEqual(runtime_dir(), home / ".cache")

    def test_runtime_dir_rejects_relative_xdg_runtime_dir(self):
        # dirs_sys::is_absolute_path treats a non-absolute value as unset.
        self._clear_xdg_env()
        os.environ["XDG_RUNTIME_DIR"] = "relative/path"
        home = Path(self._tmp.name) / "home"
        home.mkdir()
        os.environ["HOME"] = str(home)
        self.assertEqual(runtime_dir(), home / ".cache")

    def test_read_daemon_info_missing_file_returns_none(self):
        self._clear_xdg_env()
        os.environ["XDG_RUNTIME_DIR"] = self._tmp.name
        self.assertIsNone(read_daemon_info())

    def test_read_daemon_info_round_trip(self):
        self._clear_xdg_env()
        os.environ["XDG_RUNTIME_DIR"] = self._tmp.name
        info = {
            "port": 12345,
            "token": "abc",
            "pid": 999,
            "exe_path": "/usr/local/bin/esm-server",
            "exe_size": 42,
            "exe_mtime_secs": 100,
            "exe_mtime_nanos": 200,
        }
        daemon_info_path().write_text(json.dumps(info))
        self.assertEqual(read_daemon_info(), info)

    def test_read_daemon_info_legacy_file_gets_defaults(self):
        # Legacy discovery file written before exe_* fields existed.
        self._clear_xdg_env()
        os.environ["XDG_RUNTIME_DIR"] = self._tmp.name
        daemon_info_path().write_text(json.dumps({"port": 1, "token": "x", "pid": 2}))
        info = read_daemon_info()
        assert info is not None
        self.assertEqual(info["port"], 1)
        self.assertEqual(info["exe_path"], "")
        self.assertFalse(daemon_fresh(info))

    def test_daemon_fresh_true_when_binary_signature_matches(self):
        exe = Path(self._tmp.name) / "esm-server"
        exe.write_bytes(b"fake binary contents")
        st = exe.stat()
        info = {
            "exe_path": str(exe),
            "exe_size": st.st_size,
            "exe_mtime_secs": st.st_mtime_ns // 1_000_000_000,
            "exe_mtime_nanos": st.st_mtime_ns % 1_000_000_000,
        }
        self.assertTrue(daemon_fresh(info))

    def test_daemon_fresh_false_after_binary_changes(self):
        exe = Path(self._tmp.name) / "esm-server"
        exe.write_bytes(b"fake binary contents")
        st = exe.stat()
        info = {
            "exe_path": str(exe),
            "exe_size": st.st_size,
            "exe_mtime_secs": st.st_mtime_ns // 1_000_000_000,
            "exe_mtime_nanos": st.st_mtime_ns % 1_000_000_000,
        }
        exe.write_bytes(b"rebuilt, different size and mtime now")
        os.utime(exe, ns=(st.st_mtime_ns + 5_000_000_000, st.st_mtime_ns + 5_000_000_000))
        self.assertFalse(daemon_fresh(info))

    def test_daemon_fresh_false_for_empty_exe_path(self):
        self.assertFalse(daemon_fresh({"exe_path": ""}))

    def test_daemon_fresh_false_for_missing_exe(self):
        self.assertFalse(daemon_fresh({"exe_path": "/nonexistent/esm-server-nowhere"}))


# ─── FormID helper tests ─────────────────────────────────────────────────────


class FormIdHelperTests(unittest.TestCase):
    def test_formid_to_int_accepts_hex_string(self):
        self.assertEqual(formid_to_int("0x00463F"), 0x463F)
        self.assertEqual(formid_to_int("0X00463F"), 0x463F)

    def test_formid_to_int_accepts_int(self):
        self.assertEqual(formid_to_int(0x463F), 0x463F)

    def test_formid_to_hex_matches_rust_display_format(self):
        # src/formid.rs: `format!("0x{:08X}", self.0)` -- uppercase, 8 digits.
        self.assertEqual(formid_to_hex(0x463F), "0x0000463F")
        self.assertEqual(formid_to_hex(0x00ABCDEF), "0x00ABCDEF")
        self.assertEqual(formid_to_hex("0x00abcdef"), "0x00ABCDEF")


# ─── FakeGateway BFS tests ────────────────────────────────────────────────────


class FakeGatewayRefsTests(unittest.TestCase):
    def setUp(self):
        self.client = FakeGateway(FIXTURE_PATH)
        self.WEAP = 0x00100001
        self.OMOD1 = 0x00100010
        self.OMOD2 = 0x00100011
        self.LVLI = 0x00100020
        self.NPC = 0x00100030
        self.CONT = 0x00100031
        self.COBJ = 0x00100040
        self.QUST = 0x00100060

    def _form_ids(self, rows) -> set:
        return {r["form_id"] for r in rows}

    def test_depth_1_returns_only_direct_referencers(self):
        result = self.client.refs("esm", self.WEAP, depth=1)
        self.assertEqual(result["target"], "0x00100001")
        self.assertEqual(result["total"], 4)
        self.assertEqual(
            self._form_ids(result["rows"]),
            {formid_to_hex(self.OMOD1), formid_to_hex(self.OMOD2), formid_to_hex(self.LVLI), formid_to_hex(self.COBJ)},
        )
        for r in result["rows"]:
            self.assertEqual(r["depth"], 1)
            self.assertNotIn("path", r)  # skip_serializing_if empty, mirrored

    def test_depth_2_adds_lvli_referencers(self):
        result = self.client.refs("esm", self.WEAP, depth=2)
        ids = self._form_ids(result["rows"])
        self.assertIn(formid_to_hex(self.NPC), ids)
        self.assertIn(formid_to_hex(self.CONT), ids)
        self.assertEqual(result["total"], 6)  # 4 depth-1 + NPC_ + CONT

        npc_row = next(r for r in result["rows"] if r["form_id"] == formid_to_hex(self.NPC))
        self.assertEqual(npc_row["depth"], 2)
        self.assertEqual(npc_row["path"], [{"form_id": formid_to_hex(self.LVLI), "record_type": "LVLI", "editor_id": "LVLI_TestList"}])

    def test_depth_3_adds_qust_via_cont(self):
        result = self.client.refs("esm", self.WEAP, depth=3)
        ids = self._form_ids(result["rows"])
        self.assertIn(formid_to_hex(self.QUST), ids)
        self.assertEqual(result["total"], 7)  # 6 from depth 2 + QUST

        qust_row = next(r for r in result["rows"] if r["form_id"] == formid_to_hex(self.QUST))
        self.assertEqual(qust_row["depth"], 3)
        self.assertEqual(
            qust_row["path"],
            [
                {"form_id": formid_to_hex(self.LVLI), "record_type": "LVLI", "editor_id": "LVLI_TestList"},
                {"form_id": formid_to_hex(self.CONT), "record_type": "CONT", "editor_id": "CONT_TestContainer"},
            ],
        )

    def test_depth_beyond_graph_size_is_a_noop_plateau(self):
        result_3 = self.client.refs("esm", self.WEAP, depth=3)
        result_6 = self.client.refs("esm", self.WEAP, depth=6)
        self.assertEqual(result_3["total"], result_6["total"])

    def test_cycle_safety_target_not_reemitted(self):
        # The fixture deliberately makes LVLI reference the WEAP back (in
        # addition to the WEAP being referenced by LVLI), i.e. WEAP <-> LVLI.
        # The target must never appear in its own result set, and the walk
        # must terminate rather than looping forever.
        result = self.client.refs("esm", self.WEAP, depth=6)
        ids = self._form_ids(result["rows"])
        self.assertNotIn(formid_to_hex(self.WEAP), ids)
        # Every form_id appears at most once.
        self.assertEqual(len(result["rows"]), len(ids))

    def test_cycle_safety_via_kywd_reaches_weap_referencers_once(self):
        kywd = 0x00100050  # if_tmp_WeaponMod, referenced only by WEAP
        result = self.client.refs("esm", kywd, depth=6)
        ids = [r["form_id"] for r in result["rows"]]
        # No duplicates even though the graph loops back toward the KYWD's
        # own referencer chain via WEAP <-> LVLI.
        self.assertEqual(len(ids), len(set(ids)))
        self.assertIn(formid_to_hex(self.WEAP), ids)

    def test_accepts_int_and_hex_string_formid_interchangeably(self):
        by_int = self.client.refs("esm", self.WEAP, depth=1)
        by_hex = self.client.refs("esm", "0x00100001", depth=1)
        self.assertEqual(by_int, by_hex)

    def test_hub_keyword_has_more_than_8_referencers(self):
        result = self.client.refs("esm", 0x00100090, depth=1)
        self.assertGreater(result["total"], 8)

    def test_orphan_perk_and_kywd_have_no_referencers(self):
        self.assertEqual(self.client.refs("esm", 0x00100072, depth=6)["total"], 0)  # PERK2
        self.assertEqual(self.client.refs("esm", 0x00100080, depth=6)["total"], 0)  # OrphanKeyword

    def test_perk_with_pcrd_referencer(self):
        result = self.client.refs("esm", 0x00100070, depth=1)
        self.assertEqual(result["total"], 1)
        self.assertEqual(result["rows"][0]["record_type"], "PCRD")

    def test_limit_caps_and_sets_capped_flag(self):
        result = self.client.refs("esm", self.WEAP, depth=3, limit=2)
        self.assertEqual(len(result["rows"]), 2)
        self.assertTrue(result["capped"])
        self.assertEqual(result["total"], 7)

    def test_record_lookup_by_formid_and_edid(self):
        rec = self.client.record("esm", self.WEAP)
        self.assertEqual(rec["editor_id"], "WEAP_TestRifle")
        rec2 = self.client.record_by_edid("esm", "WEAP_TestRifle")
        self.assertEqual(rec, rec2)

    def test_record_not_found_raises(self):
        with self.assertRaises(DaemonError):
            self.client.record("esm", 0xFFFFFFFF)
        with self.assertRaises(DaemonError):
            self.client.record_by_edid("esm", "NoSuchEditorId")

    def test_exists(self):
        self.assertTrue(self.client.exists("esm", self.WEAP))
        self.assertFalse(self.client.exists("esm", 0xFFFFFFFF))

    def test_generic_op_matches_convenience_method(self):
        via_op = self.client.op(
            "esm",
            {"op": "referenced_by", "sel": {"kind": "form_id", "value": self.WEAP}, "limit": 0, "depth": 2},
        )
        via_method = self.client.refs("esm", self.WEAP, depth=2)
        self.assertEqual(via_op, via_method)

    def test_type_filter_narrows_emission_but_keeps_traversal(self):
        # OMOD1/OMOD2/LVLI/COBJ are the direct (depth-1) referencers, none of
        # them NPC_ -- type_filter must drop all four from the emitted rows
        # while still traversing through LVLI to reach the depth-2 NPC_.
        result = self.client.refs("esm", self.WEAP, depth=2, type_filter="NPC_")
        self.assertEqual(self._form_ids(result["rows"]), {formid_to_hex(self.NPC)})
        self.assertEqual(result["total"], 1)

    def test_type_filter_is_case_insensitive(self):
        result = self.client.refs("esm", self.WEAP, depth=1, type_filter="omod")
        self.assertEqual(
            self._form_ids(result["rows"]), {formid_to_hex(self.OMOD1), formid_to_hex(self.OMOD2)}
        )


# ─── FakeGateway paths= / bulk_get tests (inline fixture) ───────────────────


class FakeGatewayPathsAndBulkGetTests(unittest.TestCase):
    """Uses a small inline fixture (rather than the shared refs_graph.json)
    because it needs a `fields` payload on records and a `field_paths` entry
    on an adjacency row -- extensions to the fixture schema that the other
    shared-fixture tests above don't exercise."""

    def setUp(self):
        self.client = FakeGateway(
            {
                "records": {
                    "0x00000010": {"record_type": "KYWD", "editor_id": "if_tmp_Test"},
                    "0x00000020": {
                        "record_type": "SPEL",
                        "editor_id": "TestSpell",
                        "fields": {"Effects": [{"Effect": {"Magnitude": 5}}]},
                    },
                    "0x00000030": {
                        "record_type": "OMOD",
                        "editor_id": "mod_Custom_Test",
                        "fields": {"Data": {"Properties": []}},
                    },
                },
                "refs": {
                    "0x00000010": [
                        {
                            "form_id": "0x00000020",
                            "record_type": "SPEL",
                            "editor_id": "TestSpell",
                            "field_paths": ["Effects[0].Conditions.Conditions[0].Parameter 1"],
                        },
                    ],
                },
            }
        )

    def test_paths_true_passes_through_fixture_field_paths(self):
        result = self.client.refs("esm", 0x10, depth=1, paths=True)
        self.assertEqual(
            result["rows"][0]["field_paths"],
            ["Effects[0].Conditions.Conditions[0].Parameter 1"],
        )

    def test_paths_false_omits_field_paths_key(self):
        result = self.client.refs("esm", 0x10, depth=1)
        self.assertNotIn("field_paths", result["rows"][0])

    def test_bulk_get_isolates_errors_per_selector(self):
        entries = self.client.bulk_get("esm", [0x20, 0xFFFFFFFF, "mod_Custom_Test"])
        self.assertEqual(entries[0], {
            "sel": "0x00000020",
            "header": None,
            "editor_id": "TestSpell",
            "fields": {"Effects": [{"Effect": {"Magnitude": 5}}]},
        })
        self.assertEqual(entries[1]["sel"], "0xFFFFFFFF")
        self.assertIn("error", entries[1])
        # EditorID selectors display as the literal input text (mirrors
        # RecordSel::display() in ipc.rs), not the resolved FormID.
        self.assertEqual(entries[2]["sel"], "mod_Custom_Test")
        self.assertEqual(entries[2]["fields"], {"Data": {"Properties": []}})

    def test_bulk_get_via_generic_op_matches_convenience_method(self):
        via_op = self.client.op(
            "esm", {"op": "record_bulk", "sels": [{"kind": "form_id", "value": 0x20}]}
        )
        via_method = self.client.bulk_get("esm", [0x20])
        self.assertEqual(via_op, via_method)

    def test_bulk_get_empty_list_returns_empty_list(self):
        self.assertEqual(self.client.bulk_get("esm", []), [])


# ─── find_esm_binary tests ───────────────────────────────────────────────────


class FindEsmBinaryTests(unittest.TestCase):
    def test_explicit_non_executable_path_raises(self):
        with tempfile.TemporaryDirectory() as tmp:
            not_exec = Path(tmp) / "esm"
            not_exec.write_text("not executable")
            with self.assertRaises(DaemonError):
                esm_gateway.find_esm_binary(str(not_exec))

    def test_explicit_executable_path_is_returned(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "esm"
            exe.write_text("#!/bin/sh\n")
            exe.chmod(exe.stat().st_mode | stat.S_IEXEC)
            self.assertEqual(esm_gateway.find_esm_binary(str(exe)), exe)

    def test_nothing_found_raises(self):
        with mock.patch.object(esm_gateway, "WORKSPACE_ROOT", Path("/nonexistent-workspace-root")):
            with mock.patch("shutil.which", return_value=None):
                with self.assertRaises(DaemonError):
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

    def test_shared_strings_dir_uses_single_flag(self):
        d = Path("/strings")
        cmd = self._cmd(strings_dir_a=d, strings_dir_b=d)
        self.assertIn("--strings-dir", cmd)
        self.assertNotIn("--strings-dir-a", cmd)

    def test_empty_exclude_type_omits_flag(self):
        self.assertNotIn("--exclude-type", self._cmd(exclude_type=""))

    def test_always_uses_local_diff(self):
        # diff() only ever shells out to `--local diff` -- see EsmGateway.diff's
        # docstring for why the /op Diff HTTP route isn't used here.
        cmd = self._cmd()
        self.assertIn("--local", cmd)
        self.assertIn("diff", cmd)


class EsmGatewayDiffTests(unittest.TestCase):
    """Exercises `EsmGateway.diff` directly against a tiny shell-script
    stand-in for the `esm` binary -- same technique as
    tools/tests/test_orchestrator.py's `make_fake_esm`, at the transport
    layer this delegates to."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp_dir = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def _fake_esm(self, stdout_text: str, *, exit_code: int = 0) -> Path:
        # Write the desired stdout to its own file and `cat` it, rather than
        # inlining it into the shell script, to sidestep shell-quoting
        # entirely (mirrors test_orchestrator.py's make_fake_esm).
        payload = self.tmp_dir / "stdout.txt"
        payload.write_text(stdout_text)
        script = self.tmp_dir / "fake_esm.sh"
        script.write_text(f'#!/bin/sh\ncat "{payload}"\nexit {exit_code}\n')
        script.chmod(script.stat().st_mode | stat.S_IEXEC)
        return script

    def _diff(self, esm_bin):
        return esm_gateway.EsmGateway.diff(
            esm_bin, Path("a.esm"), Path("b.esm"),
            strings_dir_a=Path("/strings"), strings_dir_b=Path("/strings"),
            lang="en", record_type=None, bodies="full", keep_noise=False,
            exclude_type="LAND,NAVM",
        )

    def test_parses_json_and_strips_trailing_repl_prompt(self):
        fake_esm = self._fake_esm('{"added": [], "removed": [], "changed": []}esm> ')
        result = self._diff(fake_esm)
        self.assertEqual(result.data, {"added": [], "removed": [], "changed": []})
        self.assertEqual(result.raw_json, '{"added": [], "removed": [], "changed": []}')

    def test_cmd_reflects_argv_used(self):
        fake_esm = self._fake_esm("{}")
        result = self._diff(fake_esm)
        self.assertEqual(result.cmd[0], str(fake_esm))
        self.assertIn("--local", result.cmd)
        self.assertIn("--strings-dir", result.cmd)

    def test_nonzero_exit_raises_daemon_error(self):
        fake_esm = self._fake_esm("irrelevant", exit_code=1)
        with self.assertRaises(DaemonError) as ctx:
            self._diff(fake_esm)
        self.assertIn("exit code 1", str(ctx.exception))

    def test_invalid_json_raises_daemon_error(self):
        fake_esm = self._fake_esm("not json at all")
        with self.assertRaises(DaemonError):
            self._diff(fake_esm)


# ─── Real-ESM integration test (env-gated, silent no-op without game data) ──


class RealEsmIntegrationTests(unittest.TestCase):
    """End-to-end smoke test of `EsmGateway` against the real `esm` binary
    and a live warm daemon it spawns/reuses -- no fixtures, no stub server.

    Gated on `$FO76_ESM_PATH` (an absolute path to a real `SeventySix.esm`,
    per esm/CLAUDE.local.md), mirroring `tests/diff.rs`'s
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
        except DaemonError as exc:
            raise unittest.SkipTest(f"esm binary not found -- skipping: {exc}")
        cls.gateway = esm_gateway.ensure_daemon(cls.esm_bin, cls.esm_path)

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
