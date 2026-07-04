#!/usr/bin/env python3
"""Tests for tools/esm_daemon.py.

Covers:
  - Wire-format correctness against a stdlib `http.server`-based stub daemon
    (request path, auth header, JSON body shape, ok/err envelope handling,
    keep-alive connection reuse, reconnect-after-close retry).
  - Discovery-path resolution (`runtime_dir` / `read_daemon_info`) with
    monkeypatched environment variables pointing at a temp dir.
  - `FakeClient`'s BFS reverse-reference walk against the checked-in fixture
    (depth-1 vs depth-3 expansion, cycle safety, path/depth fields, int vs
    hex FormID acceptance).

No real daemon is spawned and no real game data is touched.
"""

from __future__ import annotations

import http.server
import json
import os
import sys
import tempfile
import threading
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import esm_daemon  # noqa: E402
from esm_daemon import (  # noqa: E402
    DaemonClient,
    DaemonError,
    FakeClient,
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

    def log_message(self, fmt, *args):  # silence default stderr logging
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
            {"op_responses": list(op_responses), "requests_seen": [], "token": token, "close_after_n": None},
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
        self.client: DaemonClient | None = None

    def tearDown(self):
        if self.client is not None:
            self.client.close()
        if self.server is not None:
            self.server.stop()

    def _client(self, op_responses: list, token: str = "test-token-abc123") -> DaemonClient:
        self.server = _StubServer(op_responses, token=token)
        self.client = DaemonClient(self.server.port, token)
        return self.client

    def test_refs_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": {"target": "0x00100001", "rows": [], "total": 0, "capped": False}})])
        result = client.refs("/data/SeventySix.esm", 0x00100001, depth=2, limit=0)
        self.assertEqual(result["target"], "0x00100001")

        req = self.server.requests_seen[0]
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
        body = self.server.requests_seen[0]["body"]
        self.assertEqual(body["op"]["sel"], {"kind": "form_id", "value": 0x00100001})

    def test_record_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": {"header": {}, "editor_id": "WEAP_TestRifle", "fields": {}}})])
        client.record("/data/x.esm", 0x463F, resolve="full")
        body = self.server.requests_seen[0]["body"]
        self.assertEqual(
            body["op"],
            {"op": "record", "sel": {"kind": "form_id", "value": 0x463F}, "depth": "full"},
        )

    def test_record_by_edid_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": {"header": {}, "editor_id": "Foo", "fields": {}}})])
        client.record_by_edid("/data/x.esm", "AssaultRifle")
        body = self.server.requests_seen[0]["body"]
        self.assertEqual(
            body["op"],
            {"op": "record", "sel": {"kind": "edid", "value": "AssaultRifle"}, "depth": "stub"},
        )

    def test_search_request_shape(self):
        client = self._client([(200, {"status": "ok", "data": []})])
        client.search("/data/x.esm", "*Rifle*", record_type="WEAP", limit=50, field="name")
        body = self.server.requests_seen[0]["body"]
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
        self.assertEqual(len(self.server.requests_seen), 2)

    def test_reconnect_after_server_closes_connection(self):
        client = self._client(
            [
                (200, {"status": "ok", "data": {"a": 1}}),
                (200, {"status": "ok", "data": {"b": 2}}),
            ]
        )
        # Force the stub to close the TCP connection after the first response,
        # simulating an idle/stale keep-alive connection the daemon dropped.
        self.server.handler.close_after_n = 1

        client.file_info("/data/x.esm")
        result = client.file_info("/data/x.esm")  # must transparently reconnect
        self.assertEqual(result, {"b": 2})
        self.assertEqual(len(self.server.requests_seen), 2)


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


# ─── FakeClient BFS tests ─────────────────────────────────────────────────────


class FakeClientRefsTests(unittest.TestCase):
    def setUp(self):
        self.client = FakeClient(FIXTURE_PATH)
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


if __name__ == "__main__":
    unittest.main()
