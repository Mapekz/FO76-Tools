#!/usr/bin/env python3
"""
Python HTTP client for the warm `esm` daemon (see ../CLAUDE.md, "Bulk / sweep
workflow"). Talks the same wire protocol the Rust CLI/N-API/MCP clients use so
external tooling (patch-notes generators, clustering scripts, ...) can reuse
the resident daemon instead of paying the ~280 MiB cold-index cost per call.

Wire format mirrors, exactly, the following Rust sources (re-verify there if
this file and the Rust side ever drift):

    src/backend.rs   -- daemon discovery file, health check, spawn/respawn
    src/ipc.rs        -- Op enum, Request/Response envelope, RefRow/RefList
    src/bin/server.rs -- /op, /health routes + bearer-token auth
    src/formid.rs     -- FormId Display format ("0x{:08X}", uppercase)

Python 3, stdlib only -- no third-party dependencies.
"""

from __future__ import annotations

import http.client
import json
import os
import subprocess
import tempfile
import time
from collections import deque
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence, Union

# ─── Constants (mirror backend.rs) ──────────────────────────────────────────

DAEMON_FILENAME = "esm-daemon.json"

#: Fast deadline for /health probes -- a live daemon responds instantly.
#: Mirrors backend.rs::CONNECT_TIMEOUT.
CONNECT_TIMEOUT_SECS = 2.0

#: Poll cadence while waiting for a freshly spawned daemon to come up.
#: Mirrors backend.rs::HEALTH_POLL_INTERVAL.
HEALTH_POLL_INTERVAL_SECS = 0.1

#: Max time to wait for a freshly spawned daemon. Mirrors
#: backend.rs::HEALTH_POLL_MAX.
HEALTH_POLL_MAX_SECS = 30.0

#: Deadline for a full /op round-trip. Mirrors backend.rs::op_timeout()'s
#: default (ESM_OP_TIMEOUT_SECS unset). The *first* op against a cold ESM can
#: trigger a full index build, hence the generous default.
OP_TIMEOUT_SECS = 300.0

#: Reverse-reference walk depth cap. Mirrors ipc.rs::DEFAULT_MAX_DEPTH.
DEFAULT_MAX_DEPTH = 6

FormIdLike = Union[int, str]


class DaemonError(Exception):
    """Raised for a daemon error envelope, a non-2xx HTTP response, or a
    malformed reply. The message is the daemon's own error string when one
    is available."""


# ─── FormID helpers (mirror src/formid.rs) ──────────────────────────────────


def formid_to_int(value: FormIdLike) -> int:
    """Accept an int or a "0x..."/decimal string and return the raw u32."""
    if isinstance(value, int):
        return value
    s = value.strip()
    if s.lower().startswith("0x"):
        return int(s, 16)
    return int(s)


def formid_to_hex(value: FormIdLike) -> str:
    """Match `FormId`'s `Display` impl in src/formid.rs exactly:

        pub fn display(self) -> String { format!("0x{:08X}", self.0) }

    i.e. "0x" + 8 uppercase hex digits (NOT lowercase -- verified against the
    Rust source, which uses `{:08X}`).
    """
    return f"0x{formid_to_int(value):08X}"


def _sel_for_formid(formid: FormIdLike) -> dict:
    """Build a `RecordSel::FormId` wire value: `{"kind":"form_id","value":<u32>}`."""
    return {"kind": "form_id", "value": formid_to_int(formid)}


def _sel_for_edid(edid: str) -> dict:
    """Build a `RecordSel::Edid` wire value: `{"kind":"edid","value":"..."}`."""
    return {"kind": "edid", "value": edid}


# ─── Daemon discovery (mirror backend.rs::runtime_dir / read_daemon_info) ───


def _absolute_env_path(name: str) -> Path | None:
    """Mirror `dirs_sys::is_absolute_path`: the env var must be set AND hold
    an absolute path, otherwise treat it as unset."""
    value = os.environ.get(name)
    if not value:
        return None
    p = Path(value)
    return p if p.is_absolute() else None


def runtime_dir() -> Path:
    """Mirror `backend.rs::runtime_dir()`:

        dirs::runtime_dir().or_else(dirs::cache_dir).unwrap_or_else(temp_dir)

    On Linux (`dirs` 5.x, src/lin.rs):
        runtime_dir() = $XDG_RUNTIME_DIR (absolute path only), else None
        cache_dir()   = $XDG_CACHE_HOME (absolute path only), else $HOME/.cache

    Final fallback is the OS temp directory (`std::env::temp_dir()`).
    """
    xdg_runtime = _absolute_env_path("XDG_RUNTIME_DIR")
    if xdg_runtime is not None:
        return xdg_runtime

    xdg_cache = _absolute_env_path("XDG_CACHE_HOME")
    if xdg_cache is not None:
        return xdg_cache

    home = os.environ.get("HOME")
    if home:
        return Path(home) / ".cache"

    return Path(tempfile.gettempdir())


def daemon_info_path() -> Path:
    return runtime_dir() / DAEMON_FILENAME


def read_daemon_info() -> dict | None:
    """Read and parse the discovery file written by the daemon on start.

    Returns None if the file is missing, unreadable, or not valid JSON --
    mirrors the `anyhow::Result` -> `.ok()` pattern the Rust callers use.

    A legacy discovery file (written before the exe-fingerprint fields
    existed) has no `exe_*`/`pid` keys at all; `#[serde(default)]` on the
    Rust side lets it still deserialize, so we fill in the same defaults
    here (empty exe_path => always treated as stale by `daemon_fresh`).
    """
    path = daemon_info_path()
    try:
        raw = path.read_text()
    except OSError:
        return None
    try:
        info = json.loads(raw)
    except json.JSONDecodeError:
        return None
    if not isinstance(info, dict) or "port" not in info or "token" not in info:
        return None
    info.setdefault("pid", 0)
    info.setdefault("exe_path", "")
    info.setdefault("exe_size", 0)
    info.setdefault("exe_mtime_secs", 0)
    info.setdefault("exe_mtime_nanos", 0)
    return info


def _exe_sig(path: Path) -> tuple[int, int, int]:
    """(size, mtime_secs, mtime_nanos) for `path`, mirroring
    `backend.rs::exe_sig()`'s use of `SystemTime::duration_since(UNIX_EPOCH)`."""
    st = path.stat()
    mtime_ns = st.st_mtime_ns
    return st.st_size, mtime_ns // 1_000_000_000, mtime_ns % 1_000_000_000


def daemon_fresh(info: Mapping[str, Any]) -> bool:
    """Mirror `backend.rs::daemon_fresh`: is the daemon still running the
    exact binary it was started with?

    This stats `info["exe_path"]` -- the path the *daemon itself* recorded
    for its own running executable (`esm-server`, a sibling of the `esm` CLI
    binary) at `DaemonInfo::current()` time -- and compares size + mtime
    against the fingerprint stored alongside it. It does NOT stat the `esm`
    CLI binary passed to `ensure_daemon`; that binary is a different file
    with its own (unrelated) mtime, so comparing against it directly would
    not reproduce the Rust self-heal behaviour.
    """
    exe_path = info.get("exe_path") or ""
    if not exe_path:
        return False
    try:
        size, secs, nanos = _exe_sig(Path(exe_path))
    except OSError:
        return False
    return (
        size == info.get("exe_size", 0)
        and secs == info.get("exe_mtime_secs", 0)
        and nanos == info.get("exe_mtime_nanos", 0)
    )


def health_check(port: int, token: str, timeout: float = CONNECT_TIMEOUT_SECS) -> bool:
    """GET /health with the bearer token; True only on HTTP 200."""
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
        try:
            conn.request("GET", "/health", headers={"Authorization": f"Bearer {token}"})
            resp = conn.getresponse()
            resp.read()
            return resp.status == 200
        finally:
            conn.close()
    except OSError:
        return False


def _connect_if_healthy_and_fresh() -> "DaemonClient | None":
    info = read_daemon_info()
    if info is None:
        return None
    if not health_check(info["port"], info["token"]):
        return None
    if not daemon_fresh(info):
        return None
    return DaemonClient(info["port"], info["token"])


def ensure_daemon(
    esm_bin: Path | str,
    esm_path: Path | str,
    *,
    timeout: float = HEALTH_POLL_MAX_SECS,
) -> "DaemonClient":
    """Return a `DaemonClient` for a healthy, up-to-date resident daemon,
    spawning (or respawning a stale) one if necessary.

    Mirrors `RemoteBackend::connect_or_spawn` in backend.rs: if a discovery
    file exists, points at a live daemon, AND that daemon is running the
    binary it started with (`daemon_fresh`), reuse it. Otherwise run one
    `esm -p info <esm_path>` subprocess -- the Rust CLI itself performs the
    spawn-lock-coordinated spawn/stale-eviction dance (see
    `spawn_daemon_and_wait` in backend.rs) -- then poll the discovery file
    and `/health` until the (new) daemon is ready.
    """
    client = _connect_if_healthy_and_fresh()
    if client is not None:
        return client

    esm_bin = Path(esm_bin)
    esm_path = Path(esm_path)
    subprocess.run(
        [str(esm_bin), "-p", "info", str(esm_path)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        client = _connect_if_healthy_and_fresh()
        if client is not None:
            return client
        time.sleep(HEALTH_POLL_INTERVAL_SECS)

    raise DaemonError(
        f"daemon did not become healthy within {timeout:.0f}s after running "
        f"'{esm_bin} -p info {esm_path}'"
    )


# ─── DaemonClient: real HTTP client ──────────────────────────────────────────


class DaemonClient:
    """Persistent HTTP client for one resident `esm-server` daemon.

    Keeps one keep-alive `http.client.HTTPConnection` open and reconnects
    (once) on a stale/closed connection. Not thread-safe -- use one instance
    per thread, as the underlying `http.client.HTTPConnection` isn't either.
    """

    def __init__(self, port: int, token: str, *, timeout: float = OP_TIMEOUT_SECS):
        self.port = port
        self.token = token
        self.timeout = timeout
        self._conn: http.client.HTTPConnection | None = None

    # ---- low-level transport ----

    def _connection(self) -> http.client.HTTPConnection:
        if self._conn is None:
            self._conn = http.client.HTTPConnection(
                "127.0.0.1", self.port, timeout=self.timeout
            )
        return self._conn

    def _reset_connection(self) -> None:
        if self._conn is not None:
            try:
                self._conn.close()
            except Exception:
                pass
        self._conn = None

    def _request(
        self, method: str, path: str, body: bytes | None = None
    ) -> tuple[int, bytes]:
        headers = {"Authorization": f"Bearer {self.token}"}
        if body is not None:
            headers["Content-Type"] = "application/json"
        last_exc: Exception | None = None
        for attempt in range(2):  # one reconnect-and-retry on a stale connection
            conn = self._connection()
            try:
                conn.request(method, path, body=body, headers=headers)
                resp = conn.getresponse()
                data = resp.read()
                return resp.status, data
            except (http.client.HTTPException, OSError) as exc:
                last_exc = exc
                self._reset_connection()
        assert last_exc is not None
        raise DaemonError(f"HTTP request to {path} failed after retry: {last_exc}")

    def close(self) -> None:
        self._reset_connection()

    def __enter__(self) -> "DaemonClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    # ---- op() : POST /op, envelope handling ----

    def op(self, esm: str, op: Mapping[str, Any]) -> Any:
        """POST `{"esm": esm, "op": op}` to /op and return the `data` payload
        of an `{"status":"ok", ...}` envelope.

        Raises `DaemonError` for an `{"status":"err","error":...}` envelope,
        for a non-2xx HTTP response (e.g. 401 from `check_auth`, whose body
        is the differently-shaped `{"error": "..."}` from `ApiError`, not the
        `Response` envelope), or for an unparsable body.
        """
        body = json.dumps({"esm": esm, "op": op}).encode("utf-8")
        status, data = self._request("POST", "/op", body)

        try:
            parsed: Any = json.loads(data.decode("utf-8")) if data else {}
        except (json.JSONDecodeError, UnicodeDecodeError) as exc:
            raise DaemonError(f"invalid JSON response (HTTP {status}): {data!r}") from exc

        if status != 200:
            message = parsed.get("error", parsed) if isinstance(parsed, dict) else parsed
            raise DaemonError(f"HTTP {status}: {message}")

        status_field = parsed.get("status") if isinstance(parsed, dict) else None
        if status_field == "ok":
            return parsed.get("data")
        if status_field == "err":
            raise DaemonError(parsed.get("error", "unknown daemon error"))
        raise DaemonError(f"unrecognized response envelope: {parsed!r}")

    # ---- convenience wrappers over Op variants (ipc.rs::Op) ----

    def file_info(self, esm: str) -> dict:
        return self.op(esm, {"op": "file_info"})

    def record(self, esm: str, formid: FormIdLike, *, resolve: str = "stub") -> dict:
        """`Op::Record { sel: FormId, depth }`. `resolve` is one of
        "none" | "stub" | "full" (ipc.rs `ResolveDepth`, default "stub")."""
        return self.op(
            esm, {"op": "record", "sel": _sel_for_formid(formid), "depth": resolve}
        )

    def record_by_edid(self, esm: str, edid: str, *, resolve: str = "stub") -> dict:
        """`Op::Record { sel: Edid, depth }`."""
        return self.op(esm, {"op": "record", "sel": _sel_for_edid(edid), "depth": resolve})

    def search(
        self,
        esm: str,
        pattern: str,
        *,
        record_type: str | None = None,
        types: Sequence[str] | None = None,
        field: str = "both",
        limit: int = 100,
    ) -> list:
        """`Op::Search { pattern, types, field, limit }`.

        `field` is one of "edid" | "name" | "both" (lib.rs `SearchField`).
        Pass either `record_type` (single 4-char signature) or `types` (a
        list); `record_type` is a convenience for the common single-type
        case and is folded into `types`.
        """
        type_list = list(types) if types else ([record_type] if record_type else [])
        return self.op(
            esm,
            {
                "op": "search",
                "pattern": pattern,
                "types": type_list,
                "field": field,
                "limit": limit,
            },
        )

    def refs(
        self, esm: str, formid: FormIdLike, *, depth: int = 2, limit: int = 0
    ) -> dict:
        """`Op::ReferencedBy { sel: FormId, limit, depth }`. `limit=0` means
        unlimited; `depth` is clamped server-side to `[1, DEFAULT_MAX_DEPTH]`.
        Returns the `RefList` dict: `{target, rows, total, capped}`.
        """
        return self.op(
            esm,
            {
                "op": "referenced_by",
                "sel": _sel_for_formid(formid),
                "limit": limit,
                "depth": depth,
            },
        )

    def exists(self, esm: str, formid: FormIdLike) -> bool:
        """True iff `formid` resolves to a record, via a cheap `resolve=none` lookup."""
        try:
            self.record(esm, formid, resolve="none")
            return True
        except DaemonError:
            return False


# ─── FakeClient: fixture-backed stand-in, no daemon/ESM required ────────────


def _sel_kind(sel: Mapping[str, Any]) -> tuple[str, Any]:
    return sel["kind"], sel["value"]


class FakeClient:
    """In-memory stand-in for `DaemonClient`, backed by a JSON fixture.

    Exposes the same public surface (`op`, `refs`, `record`, `record_by_edid`,
    `search`, `file_info`, `exists`, `close`, context-manager) so tests can
    swap it in for `DaemonClient` without branching.

    Fixture shape::

        {
          "records": {
            "0x00ABCDEF": {"record_type": "WEAP", "editor_id": "...", "name": "..."},
            ...
          },
          "refs": {
            "0x00ABCDEF": [
              {"form_id": "0x...", "record_type": "...", "editor_id": "...",
               "name": ..., "depth": 1, "path": []},
              ...
            ],
            ...
          }
        }

    `refs[X]` lists only the *direct* (depth-1) referencers of `X` -- exactly
    what `Database::referenced_by` returns for one node in the real backend.
    `FakeClient.refs()` performs the same breadth-first walk that
    `ipc::referenced_by_enriched` performs server-side: expanding one hop at
    a time up to the requested depth, visiting each node at most once
    (cycle-safe), and recording the intermediate-node `path` and hop `depth`
    exactly as the real `RefRow`/`RefPathNode` structs do. Final ordering
    also matches: rows are sorted by ascending numeric FormID (not by depth
    or discovery order), matching `referenced_by_enriched`'s `sort_by_key`.
    """

    def __init__(self, fixture: Union[dict, str, Path]):
        if isinstance(fixture, (str, Path)):
            fixture = json.loads(Path(fixture).read_text())
        self.records: dict[str, dict] = dict(fixture.get("records", {}))
        self.refs_adj: dict[str, list[dict]] = dict(fixture.get("refs", {}))

    # ---- generic op() for interface parity with DaemonClient ----

    def op(self, esm: str, op: Mapping[str, Any]) -> Any:
        kind = op.get("op")
        if kind == "referenced_by":
            fid = self._resolve_sel(op["sel"])
            return self._referenced_by(
                fid, depth=op.get("depth", 1), limit=op.get("limit", 0)
            )
        if kind == "record":
            return self._record(op["sel"])
        if kind == "file_info":
            return self.file_info(esm)
        if kind == "search":
            raise DaemonError("FakeClient does not support op 'search' (no search index in fixture)")
        raise DaemonError(f"FakeClient does not support op {kind!r}")

    def record(self, esm: str, formid: FormIdLike, *, resolve: str = "stub") -> dict:
        return self._record(_sel_for_formid(formid))

    def record_by_edid(self, esm: str, edid: str, *, resolve: str = "stub") -> dict:
        return self._record(_sel_for_edid(edid))

    def refs(
        self, esm: str, formid: FormIdLike, *, depth: int = 2, limit: int = 0
    ) -> dict:
        return self._referenced_by(formid_to_int(formid), depth=depth, limit=limit)

    def search(
        self,
        esm: str,
        pattern: str,
        *,
        record_type: str | None = None,
        types: Sequence[str] | None = None,
        field: str = "both",
        limit: int = 100,
    ) -> list:
        raise DaemonError("FakeClient does not support 'search' (fixture has no search index)")

    def file_info(self, esm: str) -> dict:
        raise DaemonError("FakeClient does not support 'file_info' (fixture has no header data)")

    def exists(self, esm: str, formid: FormIdLike) -> bool:
        return formid_to_hex(formid) in self.records

    def close(self) -> None:
        pass

    def __enter__(self) -> "FakeClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    # ---- internals ----

    def _resolve_sel(self, sel: Mapping[str, Any]) -> int:
        kind, value = _sel_kind(sel)
        if kind == "form_id":
            return formid_to_int(value)
        if kind == "edid":
            for key, meta in self.records.items():
                if meta.get("editor_id") == value:
                    return formid_to_int(key)
            raise DaemonError(f"EditorID '{value}' not found")
        raise DaemonError(f"unknown RecordSel kind {kind!r}")

    def _record(self, sel: Mapping[str, Any]) -> dict:
        fid = self._resolve_sel(sel)
        key = formid_to_hex(fid)
        rec = self.records.get(key)
        if rec is None:
            raise DaemonError(f"FormID {key} not found")
        return rec

    def _referenced_by(self, target: int, *, depth: int, limit: int) -> dict:
        # Mirror ipc.rs::referenced_by_enriched's clamp: 0 (or any value < 1)
        # is treated as 1; values above DEFAULT_MAX_DEPTH are capped.
        max_depth = max(1, min(depth, DEFAULT_MAX_DEPTH))
        target_hex = formid_to_hex(target)

        seen: set[int] = {target}
        # Queue entries: (node_to_expand, path_of_intermediate_hops_leading_to_it).
        queue: deque[tuple[int, list[dict]]] = deque([(target, [])])
        rows: list[dict] = []

        while queue:
            current, path_here = queue.popleft()
            current_hex = formid_to_hex(current)
            for row in self.refs_adj.get(current_hex, []):
                fid = formid_to_int(row["form_id"])
                if fid in seen:
                    continue  # already emitted via a shorter or equal-length path
                seen.add(fid)

                fid_hex = formid_to_hex(fid)
                meta = self.records.get(fid_hex, {})
                record_type = meta.get("record_type", row.get("record_type"))
                editor_id = meta.get("editor_id", row.get("editor_id"))
                name = meta.get("name", row.get("name"))
                hop_depth = len(path_here) + 1

                out_row: dict[str, Any] = {
                    "form_id": fid_hex,
                    "record_type": record_type,
                    "editor_id": editor_id,
                    "name": name,
                    "offset": row.get("offset", 0),
                    "depth": hop_depth,
                }
                # RefRow's `path` is `#[serde(skip_serializing_if = "Vec::is_empty")]`
                # on the wire -- omit the key entirely at depth 1, same as the
                # real daemon's JSON.
                if path_here:
                    out_row["path"] = list(path_here)
                rows.append(out_row)

                if hop_depth < max_depth:
                    new_path = path_here + [
                        {
                            "form_id": fid_hex,
                            "record_type": record_type,
                            "editor_id": editor_id,
                        }
                    ]
                    queue.append((fid, new_path))

        rows.sort(key=lambda r: formid_to_int(r["form_id"]))

        total = len(rows)
        capped = limit > 0 and total > limit
        limited = rows[:limit] if limit > 0 else rows

        return {
            "target": target_hex,
            "rows": limited,
            "total": total,
            "capped": capped,
        }
