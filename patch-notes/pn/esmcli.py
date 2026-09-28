#!/usr/bin/env python3
"""
`EsmGateway` -- the one seam every `pn/*.py` pipeline stage uses to reach
the `esm` CLI. It owns one `esm batch` child process and sends it JSON
`{"esm", "op"}` requests, one per line, reading one response envelope per line
back (see `src/bin/cli/batch.rs`). The child keeps each ESM's database open
for the gateway's lifetime, so a stage pays each ESM's open cost once, and it
exits when the gateway closes. The request/response shapes are the `Op`,
`Request` and `Response` types in `src/ops/mod.rs`.

A full gateway, not just single-record lookups: `bulk_get` (`Op::RecordBulk`,
one request for N selectors), `list_type` (`Op::ListTypeRecords`, the
`esm list --type SIG` op), `refs(..., paths=True, type_filter=...)` (the
`--paths`/`--type` refs capabilities), `diff` (the two-ESM `esm diff`
subprocess), and the one canonical `find_esm_binary` all live here, so
nothing else in `pn/` needs to shell out to `esm` directly
(`extractor/hardcoded.py` routes its `esm list --type SIG` calls through
`list_type` for exactly this reason).

`FakeGateway`, the fixture-backed test double, lives in
`tests/fake_gateway.py` -- it is a test double, not an `esm` client, so
it does not belong in the "one seam" module itself. See that module's
docstring for why `--offline` mode still reaches it from production code
(`make_patch_notes.py`/`build_bundles.py`/`run_lints.py`).

Python 3, stdlib only -- no third-party dependencies.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import IO, Any, Iterable, Mapping, Sequence

from pn import formids


class EsmError(Exception):
    """Raised for an `esm` error envelope, a failed or exited `esm` process,
    or a malformed reply. The message is `esm`'s own error string when one
    is available."""


def _sel_for_formid(formid: formids.FormIdLike) -> dict:
    """Build a `RecordSel::FormId` wire value: `{"kind":"form_id","value":<u32>}`."""
    return {"kind": "form_id", "value": formids.to_int(formid)}


def _sel_for_edid(edid: str) -> dict:
    """Build a `RecordSel::Edid` wire value: `{"kind":"edid","value":"..."}`."""
    return {"kind": "edid", "value": edid}


def _sel_for_input(value: formids.FormIdLike) -> dict:
    """Build a `RecordSel` wire value from one ambiguous token, auto-detecting
    FormID vs EditorID via `_looks_like_formid` -- mirrors `RecordSel::from_input`
    in src/ops/sel.rs. Used by `bulk_get`, whose selectors may be a mix of both
    (e.g. a caller's initial lookup token can be a FormID or an EditorID,
    while FormIDs discovered by a subsequent reverse-ref walk are always
    FormIDs)."""
    if isinstance(value, int):
        return _sel_for_formid(value)
    return _sel_for_formid(value) if formids.looks_like_formid(value) else _sel_for_edid(value)


def _sel_kind(sel: Mapping[str, Any]) -> tuple[str, Any]:
    return sel["kind"], sel["value"]


def _sel_display(sel: Mapping[str, Any]) -> str:
    """Mirror `RecordSel::display()` in src/ops/sel.rs: a FormID hex string
    (`0x0000463F`) for a `form_id` selector, or the literal EditorID text for
    an `edid` selector."""
    kind, value = _sel_kind(sel)
    return formids.display(value) if kind == "form_id" else value


# ─── esm binary discovery (the one find_esm_binary, shared by ─────────────
# ─── make_patch_notes.py/build_bundles.py) ─────────────────────────────────

#: The repo root -- this file lives at patch-notes/pn/esmcli.py.
REPO_ROOT = Path(__file__).resolve().parents[2]


def find_esm_binary(explicit: str | Path | None = None) -> Path:
    """Locate the `esm` CLI binary: an explicit path, else the workspace
    release build (`target/release/esm` at the repo root), else whatever is on
    `$PATH` as `esm`.

    Raises `EsmError` (never calls `sys.exit`/prints to stderr) -- this is
    a library function shared by every CLI entry point in `pn/`, each of
    which translates the error into its own exit-code convention (see
    `make_patch_notes.py::find_esm_binary`'s former `die(1, ...)` and
    `build_bundles.py::find_esm_binary`'s former `raise SystemExit(...)` --
    both now catch `EsmError` instead and keep their own exit code).
    """
    if explicit:
        p = Path(explicit)
        if p.is_file() and os.access(p, os.X_OK):
            return p
        raise EsmError(f"--esm-bin path not executable: {explicit}")

    release = REPO_ROOT / "target" / "release" / "esm"
    if release.is_file() and os.access(release, os.X_OK):
        return release

    found = shutil.which("esm")
    if found:
        return Path(found)

    raise EsmError(
        "Cannot find esm binary. Build it first:\n"
        "  cargo build --release\n"
        "Or pass --esm-bin /path/to/esm"
    )


# ─── diff() command construction (moved from make_patch_notes.py) ──────────


def build_diff_cmd(
    esm_bin: Path,
    esm_a: Path,
    esm_b: Path,
    *,
    lang: str,
    sources: Sequence[str] = (),
    record_type: str | None,
    bodies: str,
    keep_noise: bool,
    exclude_type: str,
) -> list[str]:
    """Build the `esm diff ...` argv list. Pure / side-effect-free so
    it can be unit-tested directly without spawning a subprocess (see
    `make_patch_notes.py`'s `TestBuildDiffCmd`, which calls this via
    `make_patch_notes.build_diff_cmd` -- re-exported there for that existing
    call site)."""
    cmd = [
        str(esm_bin), "diff", str(esm_a), str(esm_b),
        "--lang", lang, "--json", "--bodies", bodies,
    ]
    if keep_noise:
        cmd.append("--keep-noise")
    if exclude_type:
        cmd += ["--exclude-type", exclude_type]
    # Explicit string/curve source flags, verbatim; without them `esm diff`
    # discovers each side's sources from its own folder.
    cmd += list(sources)
    if record_type:
        cmd += ["--type", record_type]

    return cmd


class DiffResult:
    """Result of `EsmGateway.diff()`.

    `data`: the parsed `esm diff --json` output (a `DiffResult`-shaped
    dict on the Rust side -- see `src/diff.rs`; unrelated to this Python
    class despite the name collision, which mirrors the Rust type name for
    the reader's convenience).
    `raw_json`: the exact JSON text `esm` produced on stdout -- what callers
    write to `diff.json` verbatim, so the file matches byte-for-byte.
    `cmd`: the argv that was run (for verbose/debug echo).
    `stderr`: the subprocess's captured stderr (for verbose echo on success;
    failure already folds stderr into the raised `EsmError` instead).
    """

    __slots__ = ("data", "raw_json", "cmd", "stderr")

    def __init__(self, *, data: dict, raw_json: str, cmd: list[str], stderr: str):
        self.data = data
        self.raw_json = raw_json
        self.cmd = cmd
        self.stderr = stderr


# ─── EsmGateway: an `esm batch` child + subprocess diff ─────────────────────


class EsmGateway:
    """One `esm batch` child process, plus the `diff` entry point that runs
    its own subprocess (see `diff`'s docstring).

    The child starts on the first request and keeps every ESM it has opened
    warm until `close()` (or the end of a `with` block) closes its stdin.
    Not thread-safe: requests and responses share one pipe pair, so use one
    instance per thread.
    """

    def __init__(self, esm_bin: str | Path | None = None):
        self.esm_bin = find_esm_binary(esm_bin)
        self._proc: subprocess.Popen[str] | None = None

    # ---- low-level transport ----

    def _pipes(self) -> tuple[IO[str], IO[str]]:
        if self._proc is None or self._proc.poll() is not None:
            self._proc = subprocess.Popen(
                [str(self.esm_bin), "batch"],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                text=True,
                encoding="utf-8",
            )
        assert self._proc.stdin is not None and self._proc.stdout is not None
        return self._proc.stdin, self._proc.stdout

    def close(self) -> None:
        proc, self._proc = self._proc, None
        if proc is None:
            return
        if proc.stdin is not None:
            proc.stdin.close()
        try:
            proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()

    def __enter__(self) -> "EsmGateway":
        return self

    def __exit__(self, *_exc: object) -> None:
        del _exc
        self.close()

    # ---- op() : one request line, one response line ----

    def op(self, esm: str | Path, op: Mapping[str, Any]) -> Any:
        """Send `{"esm": esm, "op": op}` and return the `data` payload of an
        `{"status": "ok", ...}` response. Raises `EsmError` for an
        `{"status": "err", "error": ...}` response, for an `esm batch`
        process that exited, or for an unparsable reply.
        """
        stdin, stdout = self._pipes()
        try:
            stdin.write(json.dumps({"esm": str(esm), "op": op}) + "\n")
            stdin.flush()
            line = stdout.readline()
        except OSError as exc:
            raise EsmError(f"esm batch pipe failed: {exc}") from exc
        if not line:
            code = self._proc.wait() if self._proc is not None else None
            self._proc = None
            raise EsmError(f"esm batch exited (status {code}) before answering")
        try:
            parsed: Any = json.loads(line)
        except json.JSONDecodeError as exc:
            raise EsmError(f"invalid JSON from esm batch: {line[:500]!r}") from exc
        status = parsed.get("status") if isinstance(parsed, dict) else None
        if status == "ok":
            return parsed.get("data")
        if status == "err":
            raise EsmError(parsed.get("error", "unknown esm error"))
        raise EsmError(f"unrecognized response envelope: {parsed!r}")

    # ---- convenience wrappers over Op variants (ops/mod.rs::Op) ----

    def file_info(self, esm: str) -> dict:
        return self.op(esm, {"op": "file_info"})

    def record(self, esm: str, formid: formids.FormIdLike, *, resolve: str = "stub") -> dict:
        """`Op::Record { sel: FormId, depth }`. `resolve` is one of
        "none" | "stub" | "full" (`ResolveDepth` in src/decode/mod.rs, default "stub")."""
        return self.op(
            esm, {"op": "record", "sel": _sel_for_formid(formid), "depth": resolve}
        )

    def record_by_edid(self, esm: str, edid: str, *, resolve: str = "stub") -> dict:
        """`Op::Record { sel: Edid, depth }`."""
        return self.op(esm, {"op": "record", "sel": _sel_for_edid(edid), "depth": resolve})

    def bulk_get(
        self, esm: str, sels: Iterable[formids.FormIdLike], *, resolve: str = "stub"
    ) -> list[dict]:
        """`Op::RecordBulk { sels: Vec<RecordSel>, depth }` -- the bulk
        counterpart to `record`/`record_by_edid`: resolves every selector in
        one HTTP round-trip instead of N. Each element of `sels` may be a
        FormID (int or hex/decimal string) or an EditorID string; kind is
        auto-detected per-selector via `_looks_like_formid`, mirroring the
        Rust CLI's own `RecordSel::from_input` (see src/ops/sel.rs).

        Returns the raw list of `BulkRecordEntry` dicts, each shaped
        `{"sel": <selector display string>, "header"?, "editor_id"?,
        "fields"?, "error"?}` -- one bad selector produces an `error` entry
        for itself only, it never fails the whole call (see src/ops/records.rs's
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

        This is the seam `extractor/hardcoded.py` routes its
        `esm list --type SIG` calls through instead of shelling out to
        the `esm` binary directly -- see this module's docstring's "one
        seam" claim.
        """
        return self.op(esm, {"op": "list_type_records", "sig": sig, "offset": offset, "limit": limit})

    def refs(
        self,
        esm: str,
        formid: formids.FormIdLike,
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
        `RefList` in src/ops/refs.rs for each field's exact meaning; `effective_depth`
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
        Both mirror `esm refs --type SIG --paths` (see src/ops/refs.rs's
        `Op::ReferencedBy` and cli.rs's `cmd_refs`).

        `type_filter`/`paths` are omitted from the wire request entirely
        when left at their defaults, keeping the request body byte-identical
        to the pre-existing wire shape for callers that never use them
        (`ReferencedByArgs`'s `#[serde(default)]` on both fields makes this safe for
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

    def exists(self, esm: str, formid: formids.FormIdLike) -> bool:
        """True iff `formid` resolves to a record, via a cheap `resolve=none` lookup."""
        try:
            self.record(esm, formid, resolve="none")
            return True
        except EsmError:
            return False

    # ---- diff() : cold two-ESM subprocess, not the /op route ----

    @staticmethod
    def diff(
        esm_bin: Path,
        esm_a: Path,
        esm_b: Path,
        *,
        sources: Sequence[str] = (),
        lang: str,
        record_type: str | None,
        bodies: str,
        keep_noise: bool,
        exclude_type: str,
    ) -> "DiffResult":
        """Run `esm diff <A> <B> --json ...` as a one-shot subprocess and
        return a `DiffResult` (parsed JSON + the exact raw JSON text + the argv
        + captured stderr).

        A `@staticmethod` that bypasses the `esm batch` child: source
        overrides are `esm diff` flags rather than `Op::Diff` fields, and one diff runs once
        per pipeline run, so a subprocess costs nothing extra. Callers reach it
        as `EsmGateway.diff(...)` without constructing a gateway.

        Raises `EsmError` on a non-zero exit or unparsable JSON. Has no
        CLI-output side effects (no `eprint`/`die`/banners) -- callers that
        need process-exit semantics (see `make_patch_notes.py::run_esm_diff`)
        catch this and translate it themselves.
        """
        cmd = build_diff_cmd(
            esm_bin,
            esm_a,
            esm_b,
            lang=lang,
            sources=sources,
            record_type=record_type,
            bodies=bodies,
            keep_noise=keep_noise,
            exclude_type=exclude_type,
        )

        result = subprocess.run(
            cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL
        )

        if result.returncode != 0:
            raise EsmError(
                f"esm diff failed with exit code {result.returncode}: "
                f"{result.stderr.strip() or '(no stderr)'}"
            )

        raw_output = result.stdout
        try:
            data = json.loads(raw_output)
        except json.JSONDecodeError as exc:
            raise EsmError(
                f"esm diff produced invalid JSON: {exc}\n"
                f"First 500 chars: {raw_output[:500]}"
            ) from exc

        return DiffResult(data=data, raw_json=raw_output, cmd=cmd, stderr=result.stderr)
