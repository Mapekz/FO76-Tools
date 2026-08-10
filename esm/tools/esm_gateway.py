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
`list_type` (`Op::ListTypeRecords`, the `esm list --type SIG` op),
`refs(..., paths=True, type_filter=...)` (the `--paths`/`--type` refs
capabilities), `diff` (the two-ESM `esm --local diff` subprocess), and the one
canonical `find_esm_binary` (previously copy-pasted in `make_patch_notes.py`
and `build_bundles.py`) all live here now, so nothing else in `tools/` needs
to shell out to `esm` directly (`lvli_audit.py`/`extractor/hardcoded.py`
route their `esm list --type SIG` calls through `list_type` for exactly this
reason).

`FakeGateway`, the fixture-backed test double that used to live in this
file, has moved to `tools/tests/fake_gateway.py` -- it is a test double, not
a wire client, so it does not belong in the "one seam" module itself. See
that module's docstring for why `--offline` mode still reaches it from
production code (`make_patch_notes.py`/`build_bundles.py`/`run_lints.py`).

Wire format mirrors, exactly, the following Rust sources (re-verify there if
this file and the Rust side ever drift):

    src/backend.rs   -- daemon discovery file, health check, spawn/respawn
    src/ipc.rs        -- Op enum, Request/Response envelope, RefRow/RefList
    src/bin/server.rs -- /op, /health routes + bearer-token auth
    src/formid.rs     -- FormId Display format ("0x{:08X}", uppercase)
    src/bin/cli.rs    -- cmd_diff's --local/force-local rule (see `diff`'s
                         own docstring for why it stays subprocess-based)

The constants below (timeouts, `DEFAULT_MAX_DEPTH`) and the `Op`
discriminant strings / `diff` flag names used elsewhere in this file are no
longer hand-copied -- they're imported from `wire_constants.py`, a
"# GENERATED" module `tools/regen_wire_constants.py` (re)writes from the
Rust source of truth itself (`esm dump-wire-constants`). CI regenerates and
`git diff --exit-code`s it, so a Rust-side rename/value change that isn't
matched here fails the build instead of silently drifting -- see
`regen_wire_constants.py`'s own docstring.

Python 3, stdlib only -- no third-party dependencies.
"""

from __future__ import annotations

import http.client
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence, Union

sys.path.insert(0, str(Path(__file__).resolve().parent))

from wire_constants import (  # noqa: E402
    CONNECT_TIMEOUT_SECS,
    DAEMON_FILENAME,
    DIFF_FLAGS,
    HEALTH_POLL_INTERVAL_SECS,
    HEALTH_POLL_MAX_SECS,
    OP_NAMES,
    OP_TIMEOUT_SECS,
)

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


def _sel_kind(sel: Mapping[str, Any]) -> tuple[str, Any]:
    return sel["kind"], sel["value"]


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
    `esm --esm <esm_path> info` subprocess -- the Rust CLI itself performs
    the spawn-lock-coordinated spawn/stale-eviction dance (see
    `spawn_daemon_and_wait` in backend.rs) -- then poll the discovery file
    and `/health` until the (new) daemon is ready.
    """
    client = _connect_if_healthy_and_fresh()
    if client is not None:
        return client

    esm_bin = Path(esm_bin)
    esm_path = Path(esm_path)
    subprocess.run(
        [str(esm_bin), "--esm", str(esm_path), "info"],
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
        f"'{esm_bin} info {esm_path}'"
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

    # Defensive check against wire_constants.DIFF_FLAGS (generated from
    # DiffArgs, cli.rs): every long flag built above must be one `esm
    # --local diff` actually accepts today, so a typo'd or Rust-renamed
    # flag name fails immediately here instead of as an opaque clap error
    # from the subprocess. `--local`/`diff` aren't DiffArgs' own flags (the
    # former is a top-level Cli flag, the latter the subcommand name) so
    # they're allowed alongside DIFF_FLAGS. A heuristic, not a real argv
    # parser: assumes no flag VALUE (a path, a lang code) ever itself
    # starts with "--", true for every value this function ever builds.
    known = DIFF_FLAGS | {"--local"}
    for token in cmd[2:]:  # skip [esm_bin, "--local"]; "diff" is next but doesn't start with "--"
        if token.startswith("--") and token not in known:
            raise DaemonError(
                f"build_diff_cmd emitted {token!r}, not in wire_constants.DIFF_FLAGS -- "
                "regenerate tools/wire_constants.py or fix the flag name"
            )
    return cmd


class DiffResult:
    """Result of `EsmGateway.diff()`.

    `data`: the parsed `esm --local diff --json` output (a `DiffResult`-shaped
    dict on the Rust side -- see `src/diff.rs`; unrelated to this Python
    class despite the name collision, which mirrors the Rust type name for
    the reader's convenience).
    `raw_json`: the exact JSON text `esm` produced on stdout -- what callers
    write to `diff.json` verbatim, so the file matches byte-for-byte.
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
        `Response` envelope), or for an unparsable body. Also raises
        `DaemonError` client-side, before ever making the request, if
        `op["op"]` isn't one of `wire_constants.OP_NAMES` -- a typo'd or
        stale (renamed on the Rust side) op string would otherwise reach the
        server as an opaque unrecognized-op error.
        """
        kind = op.get("op")
        if kind not in OP_NAMES:
            raise DaemonError(f"unknown Op discriminant {kind!r} (not in wire_constants.OP_NAMES)")
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

    def list_type(self, esm: str, sig: str, *, offset: int = 0, limit: int = 0) -> list[dict]:
        """`Op::ListTypeRecords { sig, offset, limit }` -- the wire op behind
        `esm list --type SIG --limit N --json` (see cli.rs's `cmd_list`,
        which sends this exact op for the non-BA2-override case). Returns a
        list of `RecordRow`-shaped dicts: `{"form_id", "record_type",
        "editor_id", "name", "offset"}`. `limit=0` means unlimited, matching
        the CLI's own convention.

        This is the seam `lvli_audit.py`/`extractor/hardcoded.py` route
        their `esm list --type SIG` calls through instead of shelling out to
        the `esm` binary directly -- see this module's docstring's "one
        seam" claim.
        """
        return self.op(esm, {"op": "list_type_records", "sig": sig, "offset": offset, "limit": limit})

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
        `limit=0` means unlimited; `depth=0` requests an UNBOUNDED walk (no
        fixed hop cap), any other value clamps server-side to `[1,
        DEFAULT_MAX_DEPTH]`. Returns the `RefList` dict: `{target, rows,
        total, capped, requested_depth, effective_depth, depth_capped,
        frontier_remaining, per_depth_totals, shown_max_depth}` (see
        `RefList` in ipc.rs for each field's exact meaning; `effective_depth`
        is `None` when `requested_depth == 0`). `carrier_total`/`tag_total`
        are also part of the wire struct but only populated for
        entry-point/carrier-seeded walks, which this single-target method
        never produces -- they're omitted from a plain `refs()` response,
        same as the server's own `skip_serializing_if` omission.

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
        `esm diff A B`): `make_patch_notes.py`'s `locate_strings_dirs`
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

        `stdin=DEVNULL` is defensive hygiene for any subprocess call, not a
        workaround for anything `esm` does here -- there is no interactive
        fallback left in the CLI for a closed stdin to trip over.

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
            data = json.loads(raw_output)
        except json.JSONDecodeError as exc:
            raise DaemonError(
                f"esm diff produced invalid JSON: {exc}\n"
                f"First 500 chars: {raw_output[:500]}"
            ) from exc

        return DiffResult(data=data, raw_json=raw_output, cmd=cmd, stderr=result.stderr)
