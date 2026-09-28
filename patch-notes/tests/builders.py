#!/usr/bin/env python3
"""Shared fixture builders and test-case bases for the pn/ test suite.

Not a test module -- `unittest discover` ignores it (no `test_` prefix); the
test files that use it add `tests/` to `sys.path` and `import builders`,
the same way they already `from fake_gateway import FakeGateway`.

The dict builders are **partial-override**: each returns a complete, valid
payload of its kind, with `overrides` merged over sensible defaults, so a test
that cares about one field states only that field and every other key still
matches the shape `patchnotes_lib`'s validators accept. The key sets here are
the canonical ones -- `record()` mirrors `patchnotes_lib.RecordEntry`,
`member()` `Member`, `anchor()` `BundleAnchor`, `bundle()` `Bundle`, and
`change()` comprehensive.json's ChangeEntry.

Per-suite wrappers (`test_run_lints.make_record`, `test_triage_bundles.
make_bundle`, ...) stay in their own files: they keep each suite's positional
signature and its pinned defaults visible next to the tests that depend on
them, and only the payload shape is shared from here.
"""

from __future__ import annotations

import json
import stat
import tempfile
import unittest
from pathlib import Path
from typing import Any

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


# --------------------------------------------------------------------------
# Fixture loading
# --------------------------------------------------------------------------


def load_fixture(name: str) -> Any:
    """Read `tests/fixtures/<name>` as JSON."""
    return load_json(FIXTURES_DIR / name)


def load_json(path: Path | str) -> Any:
    """Read an arbitrary JSON file (fixtures outside `fixtures/`, real shipped
    config such as `patch_notes_tiers.json`, pipeline output in a temp dir)."""
    with open(path, encoding="utf-8") as f:
        return json.load(f)


# --------------------------------------------------------------------------
# Partial-override payload builders
# --------------------------------------------------------------------------


def record(**overrides: Any) -> dict[str, Any]:
    """A comprehensive.json record entry (`patchnotes_lib.RecordEntry` shape)."""
    return {
        "form_id": "0x00000001",
        "record_type": "MISC",
        "editor_id": None,
        "name": None,
        "description": None,
        "status": "changed",
        "prev_editor_id": None,
        "cut": None,
        "fields": None,
        "refs_out": [],
        "dangling_refs": [],
        "changes": [],
        **overrides,
    }


def change(**overrides: Any) -> dict[str, Any]:
    """A comprehensive.json ChangeEntry.

    `from`/`to` are keywords in Python, so pass them through `overrides`
    (`change(**{"from": 20, "to": 25})`) or let a suite wrapper map its own
    `from_`/`to` parameters onto them.
    """
    return {
        "path": "Data / Value",
        "kind": "scalar",
        "from": None,
        "to": None,
        "from_display": None,
        "to_display": None,
        "suppressed": None,
        "common_group": None,
        "array": None,
        **overrides,
    }


def member(**overrides: Any) -> dict[str, Any]:
    """A bundles.json bundle member (`patchnotes_lib.Member` shape)."""
    return {
        "form_id": "0x00000001",
        "record_type": "MISC",
        "editor_id": None,
        "name": None,
        "status": "changed",
        "role": "anchor",
        **overrides,
    }


#: The `BundleAnchor` keys, in the order `anchor()` emits them.
ANCHOR_KEYS = ("form_id", "record_type", "editor_id", "name", "status")


def anchor(**overrides: Any) -> dict[str, Any]:
    """A bundles.json bundle anchor (`patchnotes_lib.BundleAnchor` shape)."""
    return {
        "form_id": "0x00000001",
        "record_type": "MISC",
        "editor_id": None,
        "name": None,
        "status": "changed",
        **overrides,
    }


def anchor_of(member_dict: dict[str, Any]) -> dict[str, Any]:
    """The anchor projection of a member dict: `ANCHOR_KEYS` only, dropping
    `role`. Raises KeyError if the member is missing one, which is the point --
    a bundle's anchor and its anchor member must agree."""
    return {k: member_dict[k] for k in ANCHOR_KEYS}


def bundle(**overrides: Any) -> dict[str, Any]:
    """A bundles.json bundle (`patchnotes_lib.Bundle` shape)."""
    return {
        "id": "B0001",
        "title": "Bundle",
        "anchor": anchor(),
        "members": [],
        "edges": [],
        **overrides,
    }


# --------------------------------------------------------------------------
# Temp-directory helpers
# --------------------------------------------------------------------------


class TempDirTestCase(unittest.TestCase):
    """A TestCase with its own empty temp dir as `self.tmp` (a Path), removed
    after each test.

    Cleanup goes through `addCleanup`, so a subclass needs no `tearDown`; a
    subclass that defines `setUp` must call `super().setUp()` before touching
    `self.tmp`.
    """

    tmp: Path

    def setUp(self) -> None:
        super().setUp()
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)


class TempOutDir:
    """Context manager yielding a temp dir laid out like a pipeline output dir:
    `bundles.json` and/or `comprehensive.json` pre-written, each skipped when
    its argument is None (some CLI tests want the file genuinely absent)."""

    def __init__(self, bundles_data: Any = None, comprehensive_data: Any = None) -> None:
        self.bundles_data = bundles_data
        self.comprehensive_data = comprehensive_data
        self._tmp: tempfile.TemporaryDirectory[str] | None = None

    def __enter__(self) -> Path:
        self._tmp = tempfile.TemporaryDirectory()
        out_dir = Path(self._tmp.name)
        if self.bundles_data is not None:
            (out_dir / "bundles.json").write_text(json.dumps(self.bundles_data), encoding="utf-8")
        if self.comprehensive_data is not None:
            (out_dir / "comprehensive.json").write_text(
                json.dumps(self.comprehensive_data), encoding="utf-8"
            )
        return out_dir

    def __exit__(self, *exc: object) -> None:
        if self._tmp is not None:
            self._tmp.cleanup()


def fake_esm_script(
    tmp_dir: Path,
    *,
    stdout_path: Path | None = None,
    stdout_text: str | None = None,
    exit_code: int | None = None,
    name: str = "fake_esm.sh",
) -> Path:
    """A tiny shell-script stand-in for the `esm` binary: it ignores every
    argument and `cat`s a fixed payload.

    Pass exactly one of `stdout_path` (an existing file, e.g. a diff.json
    fixture) or `stdout_text`, which is written to `<tmp_dir>/stdout.txt`
    first -- keeping the payload in a file instead of inlining it in the
    script sidesteps shell quoting entirely.

    `exit_code=None` leaves the script's status as `cat`'s own; pass an int to
    force it, which is how the transport tests simulate a failing binary.
    """
    if (stdout_path is None) == (stdout_text is None):
        raise TypeError("fake_esm_script takes exactly one of stdout_path/stdout_text")
    if stdout_text is not None:
        stdout_path = tmp_dir / "stdout.txt"
        stdout_path.write_text(stdout_text)
    body = f'#!/bin/sh\ncat "{stdout_path}"\n'
    if exit_code is not None:
        body += f"exit {exit_code}\n"
    script = tmp_dir / name
    script.write_text(body)
    script.chmod(script.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    return script
