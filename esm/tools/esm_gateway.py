#!/usr/bin/env python3
"""
`EsmGateway` -- the one seam every `tools/*.py` pipeline stage uses to reach
the `esm` CLI/daemon (see ../CLAUDE.md, "Bulk / sweep workflow"). Talks the
same wire protocol the Rust CLI/N-API/MCP clients use so external tooling
(patch-notes generators, clustering scripts, ...) can reuse the resident
daemon instead of paying the ~280 MiB cold-index cost per call.

Historically this module (`esm_daemon.py`, `class DaemonClient`) only covered
single-record `get`/`refs`/`search`. It has since been promoted to a full
gateway: `bulk_get` (`Op::RecordBulk`, one round-trip for N selectors),
`refs(..., paths=True, type_filter=...)` (the `--paths`/`--type` refs
capabilities), `diff` (the two-ESM `esm --local diff` subprocess), and the one
canonical `find_esm_binary` (previously copy-pasted in `make_patch_notes.py`
and `build_bundles.py`) all live here now, so nothing else in `tools/` needs
to shell out to `esm` directly.

Wire format mirrors, exactly, the following Rust sources (re-verify there if
this file and the Rust side ever drift):

    src/backend.rs   -- daemon discovery file, health check, spawn/respawn
    src/ipc.rs        -- Op enum, Request/Response envelope, RefRow/RefList
    src/bin/server.rs -- /op, /health routes + bearer-token auth
    src/formid.rs     -- FormId Display format ("0x{:08X}", uppercase)
    src/bin/cli.rs    -- cmd_diff's --local/force-local rule (see `diff`'s
                         own docstring for why it stays subprocess-based)

Python 3, stdlib only -- no third-party dependencies.
"""

from __future__ import annotations

import http.client
import json
import os
import shutil
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


def _looks_like_formid(s: str) -> bool:
    """Mirror `looks_like_formid` in src/lib.rs exactly: a `0x`-prefixed hex
    value, or a bare run of only hex digits up to 8 chars (which also covers
    pure-decimal input), is a FormID; anything else is an EditorID."""
    s = s.strip()
    body = s[2:] if s[:2].lower() == "0x" else s
    return bool(body) and len(body) <= 8 and all(c in "0123456789abcdefABCDEF" for c in body)


def _sel_for_input(value: FormIdLike) -> dict:
    """Build a `RecordSel` wire value from one ambiguous token, auto-detecting
    FormID vs EditorID via `_looks_like_formid` -- mirrors `RecordSel::from_input`
    in src/ipc.rs. Used by `bulk_get`, whose selectors may be a mix of both
    (e.g. a caller's initial lookup token can be a FormID or an EditorID,
    while FormIDs discovered by a subsequent reverse-ref walk are always
    FormIDs)."""
    if isinstance(value, int):
        return _sel_for_formid(value)
    return _sel_for_formid(value) if _looks_like_formid(value) else _sel_for_edid(value)


def _sel_display(sel: Mapping[str, Any]) -> str:
    """Mirror `RecordSel::display()` in src/ipc.rs: a FormID hex string
    (`0x0000463F`) for a `form_id` selector, or the literal EditorID text for
    an `edid` selector."""
    kind, value = _sel_kind(sel)
    return formid_to_hex(value) if kind == "form_id" else value


# ─── esm binary discovery (mirrors make_patch_notes.py/build_bundles.py's ───
# ─── formerly-copy-pasted find_esm_binary; the one copy now lives here) ─────

#: esm/ workspace root -- this file lives at esm/tools/esm_gateway.py.
WORKSPACE_ROOT = Path(__file__).resolve().parent.parent


def find_esm_binary(explicit: str | Path | None = None) -> Path:
    """Locate the `esm` CLI binary: an explicit path, else the workspace
    release build (`WORKSPACE_ROOT/target/release/esm`), else whatever is on
    `$PATH` as `esm`.

    Raises `DaemonError` (never calls `sys.exit`/prints to stderr) -- this is
    a library function shared by every CLI entry point in `tools/`, each of
    which translates the error into its own exit-code convention (see
    `make_patch_notes.py::find_esm_binary`'s former `die(1, ...)` and
    `build_bundles.py::find_esm_binary`'s former `raise SystemExit(...)` --
    both now catch `DaemonError` instead and keep their own exit code).
    """
    if explicit:
        p = Path(explicit)
        if p.is_file() and os.access(p, os.X_OK):
            return p
        raise DaemonError(f"--esm-bin path not executable: {explicit}")

    release = WORKSPACE_ROOT / "target" / "release" / "esm"
    if release.is_file() and os.access(release, os.X_OK):
        return release

    found = shutil.which("esm")
    if found:
        return Path(found)

    raise DaemonError(
        "Cannot find esm binary. Build it first:\n"
        "  cargo build --release --features server\n"
        "Or pass --esm-bin /path/to/esm"
    )


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


def _connect_if_healthy_and_fresh() -> "EsmGateway | None":
    info = read_daemon_info()
    if info is None:
        return None
    if not health_check(info["port"], info["token"]):
        return None
    if not daemon_fresh(info):
        return None
    return EsmGateway(info["port"], info["token"])


def ensure_daemon(
    esm_bin: Path | str,
    esm_path: Path | str,
    *,
    timeout: float = HEALTH_POLL_MAX_SECS,
) -> "EsmGateway":
    """Return an `EsmGateway` for a healthy, up-to-date resident daemon,
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


# ─── diff() command construction (moved from make_patch_notes.py) ──────────


def build_diff_cmd(
    esm_bin: Path,
    esm_a: Path,
    esm_b: Path,
    *,
    lang: str,
    strings_dir_a: Path | None,
    strings_dir_b: Path | None,
    record_type: str | None,
    bodies: str,
    keep_noise: bool,
    exclude_type: str,
    startup_ba2: Path | None = None,
    curves_dir: Path | None = None,
) -> list[str]:
    """Build the `esm --local diff ...` argv list. Pure / side-effect-free so
    it can be unit-tested directly without spawning a subprocess (see
    `make_patch_notes.py`'s `TestBuildDiffCmd`, which calls this via
    `make_patch_notes.build_diff_cmd` -- re-exported there for that existing
    call site)."""
    cmd = [
        str(esm_bin), "--local", "diff", str(esm_a), str(esm_b),
        "--lang", lang, "--json", "--bodies", bodies,
    ]
    if keep_noise:
        cmd.append("--keep-noise")
    if exclude_type:
        cmd += ["--exclude-type", exclude_type]
    # Pass string dirs: shared if identical, per-side if different.
    if strings_dir_a and strings_dir_b:
        if strings_dir_a == strings_dir_b:
            cmd += ["--strings-dir", str(strings_dir_a)]
        else:
            cmd += ["--strings-dir-a", str(strings_dir_a),
                    "--strings-dir-b", str(strings_dir_b)]
    elif strings_dir_a:
        cmd += ["--strings-dir-a", str(strings_dir_a)]
    elif strings_dir_b:
        cmd += ["--strings-dir-b", str(strings_dir_b)]
    if record_type:
        cmd += ["--type", record_type]
    if startup_ba2:
        cmd += ["--startup-ba2", str(startup_ba2)]
    elif curves_dir:
        cmd += ["--curves-dir", str(curves_dir)]
    return cmd


class DiffResult:
    """Result of `EsmGateway.diff()`.

    `data`: the parsed `esm --local diff --json` output (a `DiffResult`-shaped
    dict on the Rust side -- see `src/diff.rs`; unrelated to this Python
    class despite the name collision, which mirrors the Rust type name for
    the reader's convenience).
    `raw_json`: the exact JSON text consumed by `raw_decode` (sans the
    trailing `--local` REPL prompt) -- what callers write to `diff.json`
    verbatim, so the file matches what `esm` produced byte-for-byte.
    `cmd`: the argv that was run (for verbose/debug echo).
    `stderr`: the subprocess's captured stderr (for verbose echo on success;
    failure already folds stderr into the raised `DaemonError` instead).
    """

    __slots__ = ("data", "raw_json", "cmd", "stderr")

    def __init__(self, *, data: dict, raw_json: str, cmd: list[str], stderr: str):
        self.data = data
        self.raw_json = raw_json
        self.cmd = cmd
        self.stderr = stderr


# ─── EsmGateway: real HTTP client + subprocess diff ─────────────────────────


class EsmGateway:
    """Persistent HTTP client for one resident `esm-server` daemon, plus the
    one `diff` entry point that stays subprocess-based (see `diff`'s own
    docstring).

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
        for _ in range(2):  # one reconnect-and-retry on a stale connection
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

    def __enter__(self) -> "EsmGateway":
        return self

    def __exit__(self, *_exc: object) -> None:
        del _exc
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

    def bulk_get(
        self, esm: str, sels: Iterable[FormIdLike], *, resolve: str = "stub"
    ) -> list[dict]:
        """`Op::RecordBulk { sels: Vec<RecordSel>, depth }` -- the bulk
        counterpart to `record`/`record_by_edid`: resolves every selector in
        one HTTP round-trip instead of N. Each element of `sels` may be a
        FormID (int or hex/decimal string) or an EditorID string; kind is
        auto-detected per-selector via `_looks_like_formid`, mirroring the
        Rust CLI's own `RecordSel::from_input` (see ipc.rs).

        Returns the raw list of `BulkRecordEntry` dicts, each shaped
        `{"sel": <selector display string>, "header"?, "editor_id"?,
        "fields"?, "error"?}` -- one bad selector produces an `error` entry
        for itself only, it never fails the whole call (see ipc.rs's
        `RecordBulk` docs). This lets a caller drop any single-vs-multi-target
        special case entirely: even a length-1 `sels` list gets the same
        per-selector error isolation a subprocess `esm get` with one bad
        target did not have.
        """
        wire_sels = [_sel_for_input(s) for s in sels]
        return self.op(esm, {"op": "record_bulk", "sels": wire_sels, "depth": resolve})

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
        self,
        esm: str,
        formid: FormIdLike,
        *,
        depth: int = 2,
        limit: int = 0,
        type_filter: str | None = None,
        paths: bool = False,
    ) -> dict:
        """`Op::ReferencedBy { sel: FormId, limit, depth, type_filter, paths }`.
        `limit=0` means unlimited; `depth` is clamped server-side to
        `[1, DEFAULT_MAX_DEPTH]`. Returns the `RefList` dict:
        `{target, rows, total, capped}`.

        `type_filter`, if given, must be a 4-character record-type signature
        (case-insensitive, e.g. `"OMOD"`) -- only referencing records of that
        type are emitted (the walk still traverses through non-matching
        nodes so a matching node further away stays reachable). `paths`, if
        true, annotates each emitted row with `field_paths`: the JSON field
        path(s) inside that row's decoded body referencing its predecessor in
        the hop chain -- opt-in because it requires a full decode per row.
        Both mirror `esm refs --type SIG --paths` (see ipc.rs's
        `Op::ReferencedBy` and cli.rs's `cmd_refs`).

        `type_filter`/`paths` are omitted from the wire request entirely
        when left at their defaults, keeping the request body byte-identical
        to the pre-existing wire shape for callers that never use them
        (`ipc.rs`'s `#[serde(default)]` on both fields makes this safe for
        older/newer clients either way).
        """
        op: dict[str, Any] = {
            "op": "referenced_by",
            "sel": _sel_for_formid(formid),
            "limit": limit,
            "depth": depth,
        }
        if type_filter is not None:
            op["type_filter"] = type_filter
        if paths:
            op["paths"] = paths
        return self.op(esm, op)

    def exists(self, esm: str, formid: FormIdLike) -> bool:
        """True iff `formid` resolves to a record, via a cheap `resolve=none` lookup."""
        try:
            self.record(esm, formid, resolve="none")
            return True
        except DaemonError:
            return False

    # ---- diff() : cold two-ESM subprocess, deliberately not the /op route ----

    @staticmethod
    def diff(
        esm_bin: Path,
        esm_a: Path,
        esm_b: Path,
        *,
        strings_dir_a: Path | None,
        strings_dir_b: Path | None,
        lang: str,
        record_type: str | None,
        bodies: str,
        keep_noise: bool,
        exclude_type: str,
        startup_ba2: Path | None = None,
        curves_dir: Path | None = None,
    ) -> "DiffResult":
        """Run `esm --local diff <A> <B> --json ...` as a one-shot subprocess
        and return a `DiffResult` (parsed JSON + the exact raw JSON text +
        the argv + captured stderr).

        A `@staticmethod`, not an instance method: unlike every other
        `EsmGateway` capability, `diff` does not go over this class's `/op`
        HTTP transport (`self.port`/`self.token` are unused), so it needs no
        connected instance -- callers can reach it as `EsmGateway.diff(...)`
        before any daemon has even been spawned (this is exactly how
        `make_patch_notes.py` uses it: the diff step runs before the
        bundles/lints stage ever calls `ensure_daemon`).

        **Why subprocess + `--local`, not the warm daemon's `/op Diff` route,
        even though that route works fine** (`Op::Diff` dispatches through a
        `Registry` two-key lookup and is exercised today by plain
        `esm -p diff A B`): `make_patch_notes.py`'s `locate_strings_dirs`
        always resolves and passes an explicit `--strings-dir`/
        `--strings-dir-a`/`--strings-dir-b` (it's a hard error to omit one,
        by design -- "Refusing to diff without strings"), and optionally
        `--startup-ba2`/`--curves-dir`. `cli.rs::cmd_diff`'s `force_local`
        check explicitly rejects every one of those flags when `daemon_mode`
        is set ("... are not supported in daemon mode for diff; use
        --local"). So for *this* pipeline's actual call pattern, `--local`
        isn't a leftover habit, it's the only mode the Rust CLI accepts --
        routing through `/op Diff` would require dropping per-side strings
        control and relying on the daemon's sibling-file auto-load instead,
        which is a real behavior change, not a plumbing one, and out of scope
        here (see esm/CLAUDE.md's "Bulk / sweep workflow" for how daemon
        auto-load works when no override flags are given).

        Tolerates `--local`'s trailing interactive-REPL prompt ("esm> ")
        after the JSON blob via `json.JSONDecoder().raw_decode`, same as
        the CLI's own `-p`/`--local` split relies on for any subprocess
        caller. `stdin=DEVNULL` avoids blocking on that REPL waiting for
        input.

        Raises `DaemonError` on a non-zero exit or unparsable JSON. Has no
        CLI-output side effects (no `eprint`/`die`/banners) -- callers that
        need process-exit semantics (see `make_patch_notes.py::run_esm_diff`)
        catch this and translate it themselves.
        """
        cmd = build_diff_cmd(
            esm_bin,
            esm_a,
            esm_b,
            lang=lang,
            strings_dir_a=strings_dir_a,
            strings_dir_b=strings_dir_b,
            record_type=record_type,
            bodies=bodies,
            keep_noise=keep_noise,
            exclude_type=exclude_type,
            startup_ba2=startup_ba2,
            curves_dir=curves_dir,
        )

        result = subprocess.run(
            cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL
        )

        if result.returncode != 0:
            raise DaemonError(
                f"esm diff failed with exit code {result.returncode}: "
                f"{result.stderr.strip() or '(no stderr)'}"
            )

        raw_output = result.stdout
        try:
            data, json_end = json.JSONDecoder().raw_decode(raw_output)
        except json.JSONDecodeError as exc:
            raise DaemonError(
                f"esm diff produced invalid JSON: {exc}\n"
                f"First 500 chars: {raw_output[:500]}"
            ) from exc

        return DiffResult(
            data=data, raw_json=raw_output[:json_end], cmd=cmd, stderr=result.stderr
        )


# ─── FakeGateway: fixture-backed stand-in, no daemon/ESM required ───────────


def _sel_kind(sel: Mapping[str, Any]) -> tuple[str, Any]:
    return sel["kind"], sel["value"]


class FakeGateway:
    """In-memory stand-in for `EsmGateway`, backed by a JSON fixture.

    Exposes the same public surface (`op`, `refs`, `record`, `record_by_edid`,
    `bulk_get`, `search`, `file_info`, `exists`, `close`, context-manager) so
    tests can swap it in for `EsmGateway` without branching. No `diff()` --
    nothing in `tools/` calls `.diff()` on an injected client (it's a
    `@staticmethod` invoked directly, see `EsmGateway.diff`), so there's
    nothing to fake.

    Fixture shape::

        {
          "records": {
            "0x00ABCDEF": {
              "record_type": "WEAP", "editor_id": "...", "name": "...",
              "fields": {...}                 # optional; needed only by
                                               # bulk_get() consumers that
                                               # inspect decoded fields
            },
            ...
          },
          "refs": {
            "0x00ABCDEF": [
              {"form_id": "0x...", "record_type": "...", "editor_id": "...",
               "name": ..., "depth": 1, "path": [],
               "field_paths": [...]}           # optional; only consulted
                                                # when refs(..., paths=True)
              ...
            ],
            ...
          }
        }

    `refs[X]` lists only the *direct* (depth-1) referencers of `X` -- exactly
    what `Database::referenced_by` returns for one node in the real backend.
    `FakeGateway.refs()` performs the same breadth-first walk that
    `ipc::referenced_by_enriched` performs server-side: expanding one hop at
    a time up to the requested depth, visiting each node at most once
    (cycle-safe), and recording the intermediate-node `path` and hop `depth`
    exactly as the real `RefRow`/`RefPathNode` structs do. Final ordering
    also matches: rows are sorted by ascending numeric FormID (not by depth
    or discovery order), matching `referenced_by_enriched`'s `sort_by_key`.
    `type_filter` narrows *emission* only (the walk still traverses through
    non-matching nodes), and `paths=True` passes each matching adjacency
    row's own `field_paths` entry straight through -- there's no real record
    body to decode a path from in a fixture, so it's fixture-authored data,
    not a computed one, unlike the real daemon's `Database.formid_reference_paths`.
    """

    def __init__(self, fixture: Union[dict, str, Path]):
        data: dict = (
            json.loads(Path(fixture).read_text())
            if isinstance(fixture, (str, Path))
            else fixture
        )
        self.records: dict[str, dict] = dict(data.get("records", {}))
        self.refs_adj: dict[str, list[dict]] = dict(data.get("refs", {}))

    # ---- generic op() for interface parity with EsmGateway ----

    def op(self, esm: str, op: Mapping[str, Any]) -> Any:
        kind = op.get("op")
        if kind == "referenced_by":
            fid = self._resolve_sel(op["sel"])
            return self._referenced_by(
                fid,
                depth=op.get("depth", 1),
                limit=op.get("limit", 0),
                type_filter=op.get("type_filter"),
                include_paths=op.get("paths", False),
            )
        if kind == "record":
            return self._record(op["sel"])
        if kind == "record_bulk":
            return self._bulk_record_entries(op.get("sels") or [])
        if kind == "file_info":
            return self.file_info(esm)
        if kind == "search":
            raise DaemonError("FakeGateway does not support op 'search' (no search index in fixture)")
        raise DaemonError(f"FakeGateway does not support op {kind!r}")

    def record(self, esm: str, formid: FormIdLike, *, resolve: str = "stub") -> dict:
        return self.op(esm, {"op": "record", "sel": _sel_for_formid(formid), "depth": resolve})

    def record_by_edid(self, esm: str, edid: str, *, resolve: str = "stub") -> dict:
        return self.op(esm, {"op": "record", "sel": _sel_for_edid(edid), "depth": resolve})

    def bulk_get(
        self, esm: str, sels: Iterable[FormIdLike], *, resolve: str = "stub"
    ) -> list[dict]:
        """Fixture-backed counterpart to `EsmGateway.bulk_get`: resolves each
        selector against `self.records`, isolating a lookup failure to its
        own `{"sel", "error"}` entry exactly like the real `Op::RecordBulk`
        dispatch does (see ipc.rs's `bulk_record_entry`)."""
        wire_sels = [_sel_for_input(s) for s in sels]
        return self.op(esm, {"op": "record_bulk", "sels": wire_sels, "depth": resolve})

    def refs(
        self,
        esm: str,
        formid: FormIdLike,
        *,
        depth: int = 2,
        limit: int = 0,
        type_filter: str | None = None,
        paths: bool = False,
    ) -> dict:
        return self.op(
            esm,
            {
                "op": "referenced_by",
                "sel": _sel_for_formid(formid),
                "depth": depth,
                "limit": limit,
                "type_filter": type_filter,
                "paths": paths,
            },
        )

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
        raise DaemonError(
            "FakeGateway does not support 'search' (fixture has no search index): "
            f"esm={esm!r} pattern={pattern!r} record_type={record_type!r} "
            f"types={types!r} field={field!r} limit={limit!r}"
        )

    def file_info(self, esm: str) -> dict:
        raise DaemonError(
            f"FakeGateway does not support 'file_info' (fixture has no header data): esm={esm!r}"
        )

    def exists(self, esm: str, formid: FormIdLike) -> bool:
        """True iff `formid` resolves to a record, via a cheap `resolve=none`
        lookup -- mirrors `EsmGateway.exists`."""
        try:
            self.record(esm, formid, resolve="none")
            return True
        except DaemonError:
            return False

    def close(self) -> None:
        pass

    def __enter__(self) -> "FakeGateway":
        return self

    def __exit__(self, *_exc: object) -> None:
        del _exc
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

    def _bulk_record_entries(self, wire_sels: Sequence[Mapping[str, Any]]) -> list[dict]:
        """Shared by `bulk_get()` and `op()`'s `record_bulk` dispatch --
        mirrors `bulk_record_entry` in ipc.rs: one bad selector becomes an
        isolated `error` entry, never aborting the whole batch."""
        entries = []
        for sel in wire_sels:
            display = _sel_display(sel)
            try:
                rec = self._record(sel)
            except DaemonError as exc:
                entries.append({"sel": display, "error": str(exc)})
                continue
            entries.append(
                {
                    "sel": display,
                    "header": rec.get("header"),
                    "editor_id": rec.get("editor_id"),
                    "fields": rec.get("fields"),
                }
            )
        return entries

    def _referenced_by(
        self,
        target: int,
        *,
        depth: int,
        limit: int,
        type_filter: str | None = None,
        include_paths: bool = False,
    ) -> dict:
        # Mirror ipc.rs::referenced_by_enriched's clamp: 0 (or any value < 1)
        # is treated as 1; values above DEFAULT_MAX_DEPTH are capped.
        max_depth = max(1, min(depth, DEFAULT_MAX_DEPTH))
        target_hex = formid_to_hex(target)
        type_filter_upper = type_filter.upper() if type_filter else None

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

                # `type_filter` narrows *emission* only -- the walk below still
                # expands through a non-matching node so a matching node
                # further away stays reachable (mirrors
                # ipc.rs::referenced_by_enriched's `type_matches` gate).
                type_matches = type_filter_upper is None or (
                    (record_type or "").upper() == type_filter_upper
                )
                if type_matches:
                    out_row: dict[str, Any] = {
                        "form_id": fid_hex,
                        "record_type": record_type,
                        "editor_id": editor_id,
                        "name": name,
                        "offset": row.get("offset", 0),
                        "depth": hop_depth,
                    }
                    # RefRow's `path` is `#[serde(skip_serializing_if =
                    # "Vec::is_empty")]` on the wire -- omit the key entirely
                    # at depth 1, same as the real daemon's JSON.
                    if path_here:
                        out_row["path"] = list(path_here)
                    if include_paths:
                        # Fixture-authored, not computed -- see class docstring.
                        out_row["field_paths"] = row.get("field_paths", [])
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
