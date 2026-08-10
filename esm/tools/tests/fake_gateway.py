#!/usr/bin/env python3
"""
`FakeGateway` -- an in-memory, fixture-backed stand-in for
`esm_gateway.EsmGateway`, used by tests and by every pipeline stage's
`--offline` mode (see `make_patch_notes.py`/`build_bundles.py`/
`run_lints.py`).

This lives under `tools/tests/`, not `tools/`, because it is a test double,
not a wire client: it never talks to a real `esm` daemon, it replays a JSON
fixture (see `FakeGateway`'s own docstring below for the fixture shape).
`esm_gateway.py` (the real seam) is intentionally kept free of it -- see
that module's docstring for the "one seam" property this split preserves.

The `--offline` code paths in `make_patch_notes.py`/`build_bundles.py`/
`run_lints.py` import this module lazily (only inside their `if
args.offline:` branch) with a small `sys.path` shim, so production code
importing from a test module is confined to that one opt-in code path --
see those modules' own comments at the import site. This is a deliberate,
accepted tradeoff (production code depending on a test module) rather than
duplicating this ~250-line class in two places; `test_fake_gateway.py`'s
conformance test is what keeps this class honest against the real
`ipc::referenced_by_enriched` BFS it reimplements in Python.

Python 3, stdlib only.
"""

from __future__ import annotations

import json
import sys
from collections import deque
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence, Union

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from esm_gateway import (  # noqa: E402
    DaemonError,
    FormIdLike,
    _sel_display,
    _sel_for_edid,
    _sel_for_formid,
    _sel_for_input,
    _sel_kind,
    formid_to_hex,
    formid_to_int,
)
from wire_constants import DEFAULT_MAX_DEPTH  # noqa: E402


class FakeGateway:
    """In-memory stand-in for `EsmGateway`, backed by a JSON fixture.

    Exposes the same public surface (`op`, `refs`, `record`, `record_by_edid`,
    `bulk_get`, `list_type`, `search`, `file_info`, `exists`, `close`,
    context-manager) so tests can swap it in for `EsmGateway` without
    branching. No `diff()` -- nothing in `tools/` calls `.diff()` on an
    injected client (it's a `@staticmethod` invoked directly, see
    `EsmGateway.diff`), so there's nothing to fake.

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
    `refs.rs::referenced_by_walk` performs server-side: expanding one hop at
    a time up to the requested depth (`depth=0` = unbounded, no fixed hop
    cap, mirroring `RefList.effective_depth == None`), visiting each node at
    most once (cycle-safe), and recording the intermediate-node `path` and
    hop `depth` exactly as the real `RefRow`/`RefPathNode` structs do, plus
    the same `RefList` walk-stats fields (`requested_depth`,
    `effective_depth`, `depth_capped`, `frontier_remaining`,
    `per_depth_totals`, `shown_max_depth` -- see `refs.rs`'s `WalkStats` for
    each field's exact meaning). `test_fake_gateway.py`'s
    `FakeGatewayConformanceTests` is what keeps this in sync with the real
    engine when the Rust side's walk logic changes. Final ordering also
    matches: rows are sorted by ascending numeric FormID (not by depth or
    discovery order), matching `referenced_by_walk`'s default `RefSort::Formid`.
    `type_filter` narrows *emission* only (the walk still traverses through
    non-matching nodes), and `paths=True` passes each matching adjacency
    row's own `field_paths` entry straight through -- there's no real record
    body to decode a path from in a fixture, so it's fixture-authored data,
    not a computed one, unlike the real daemon's `Database.formid_reference_paths`.

    `list_type()`/the `list_type_records` op are derived purely from
    `self.records` (grouped by `record_type`, sorted by ascending numeric
    FormID) -- there is no separate "list index" in the fixture schema, the
    same way the real `Database::list_type_records` is just a filtered scan
    over the same underlying record set `Database::referenced_by` walks.
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
        if kind == "list_type_records":
            return self._list_type_records(
                op["sig"], offset=op.get("offset", 0), limit=op.get("limit", 0)
            )
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

    def list_type(self, esm: str, sig: str, *, offset: int = 0, limit: int = 0) -> list[dict]:
        """Fixture-backed counterpart to `EsmGateway.list_type`: every
        `self.records` entry whose `record_type` matches `sig`
        (case-insensitive), sorted by ascending numeric FormID."""
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

    def _list_type_records(self, sig: str, *, offset: int, limit: int) -> list[dict]:
        sig_upper = sig.upper()
        # Explicit dict[str, Any]: a bare `dict` literal gives every key the
        # SAME inferred value type (the union of every value in this one
        # literal, e.g. "form_id"'s str gets muddied with "offset"'s int and
        # meta.get(...)'s `Unknown | None`) -- annotating widens each value
        # to Any so `r["form_id"]` below is still known-str at the call site.
        rows: list[dict[str, Any]] = [
            {
                "form_id": fid,
                "record_type": meta.get("record_type"),
                "editor_id": meta.get("editor_id"),
                "name": meta.get("name"),
                "offset": 0,
            }
            for fid, meta in self.records.items()
            if (meta.get("record_type") or "").upper() == sig_upper
        ]
        rows.sort(key=lambda r: formid_to_int(r["form_id"]))
        sliced = rows[offset:]
        return sliced[:limit] if limit > 0 else sliced

    def _referenced_by(
        self,
        target: int,
        *,
        depth: int,
        limit: int,
        type_filter: str | None = None,
        include_paths: bool = False,
    ) -> dict:
        # Mirror refs.rs::referenced_by_walk's clamp exactly: `depth == 0`
        # requests an UNBOUNDED walk (max_depth = None, no fixed hop cap,
        # RefList.effective_depth = None) -- NOT "treated as depth 1", which
        # is what this used to do before the conformance test in
        # test_fake_gateway.py caught the drift. Any other value clamps to
        # `[1, DEFAULT_MAX_DEPTH]` as before.
        requested_depth = depth
        max_depth: int | None = None if depth == 0 else max(1, min(depth, DEFAULT_MAX_DEPTH))
        effective_depth = max_depth
        target_hex = formid_to_hex(target)
        type_filter_upper = type_filter.upper() if type_filter else None

        seen: set[int] = {target}
        # Queue entries: (node_to_expand, path_of_intermediate_hops_leading_to_it).
        queue: deque[tuple[int, list[dict]]] = deque([(target, [])])
        rows: list[dict] = []
        # Newly-discovered nodes at the depth cutoff that were not expanded
        # further -- mirrors refs.rs's `frontier_remaining` exactly: counted
        # for EVERY newly-discovered edge at the cutoff, regardless of
        # type_filter (the `if type_matches {...}` row-emission block and
        # the `if hop_depth < max_depth {...} else {frontier_remaining += 1}`
        # expansion block are independent in the Rust source).
        frontier_remaining = 0

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

                if max_depth is None or hop_depth < max_depth:
                    new_path = path_here + [
                        {
                            "form_id": fid_hex,
                            "record_type": record_type,
                            "editor_id": editor_id,
                        }
                    ]
                    queue.append((fid, new_path))
                else:
                    frontier_remaining += 1

        rows.sort(key=lambda r: formid_to_int(r["form_id"]))

        # per_depth_totals: row count per hop depth (index = depth), over the
        # emitted (type-filtered) rows, BEFORE --limit truncation -- mirrors
        # refs.rs computing this from `all_rows` prior to the `limit` slice.
        max_depth_seen = max((r["depth"] for r in rows), default=0)
        per_depth_totals = [0] * (max_depth_seen + 1)
        for r in rows:
            per_depth_totals[r["depth"]] += 1

        total = len(rows)
        capped = limit > 0 and total > limit
        limited = rows[:limit] if limit > 0 else rows
        shown_max_depth = max((r["depth"] for r in limited), default=0)

        return {
            "target": target_hex,
            "rows": limited,
            "total": total,
            "capped": capped,
            "requested_depth": requested_depth,
            "effective_depth": effective_depth,
            "depth_capped": frontier_remaining > 0,
            "frontier_remaining": frontier_remaining,
            "per_depth_totals": per_depth_totals,
            "shown_max_depth": shown_max_depth,
        }
