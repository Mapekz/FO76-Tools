#!/usr/bin/env python3
"""Extract xEdit's hardcoded-engine-form pseudo-plugin → schema/hardcoded_fo76.json.

Fallout76.esm only contains records with FormIDs the game's data files define.
A handful of low FormIDs (roughly < 0x800) are instead hardcoded into the game
executable itself — e.g. AVIF `KillStreak` at 0x399 — and never appear as a
record in the ESM. xEdit ships these as a pseudo-plugin at
``Core/Hardcoded/Fallout76.esp`` inside the TES5Edit checkout, purely so it has
something to resolve those FormIDs against.

This script shells out to the ``esm`` CLI ``--local`` (the same reader as
``src/reader.rs``, including the XXXX oversized-subrecord rule) for ``tree``
and ``get`` -- a cold one-shot open is the right call for this tiny
pseudo-plugin, no daemon worth keeping warm. ``list`` goes through
``esm_gateway.EsmGateway.list_type`` (a warm-daemon round-trip) instead of
its own subprocess call, per that module's "one seam" property -- see its
docstring. Emits a small lookup table of ``{formid, type, editor_id}``
entries, checked in at ``schema/hardcoded_fo76.json`` since the TES5Edit
checkout is not always present (same rationale as ``schema/fo76.json``).
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR.parent))

import esm_gateway  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
TES5 = ROOT.parent / "TES5Edit"
HARDCODED_ESP = TES5 / "Core" / "Hardcoded" / "Fallout76.esp"
OUT = ROOT / "schema" / "hardcoded_fo76.json"


def read_zstring(data: bytes) -> str | None:
    """Read a NUL-terminated inline string, stripping an optional `<ID=...>` prefix.

    Mirrors `inline_string_from_subrecords` in `src/reader.rs`.
    """
    if not data:
        return None
    nul_end = data.find(b"\x00")
    if nul_end < 0:
        nul_end = len(data)
    if nul_end == 0:
        return None
    text = data[:nul_end].decode("utf-8", errors="replace")
    if text.startswith("<ID="):
        close = text.find(">")
        if close >= 0:
            remainder = text[close + 1 :].lstrip()
            return remainder or None
    return text


def run_esm(esm_bin: str, esp_path: Path, *args: str) -> Any:
    """Run ``esm --esm <esp> --local <args...>`` and parse stdout as JSON."""
    cmd = [esm_bin, "--esm", str(esp_path), "--local", *args]
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            check=True,
        )
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or exc.stdout or "").strip() or f"exit {exc.returncode}"
        raise RuntimeError(f"command failed ({detail}): {' '.join(cmd)}") from exc
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(
            f"unparseable JSON from {' '.join(cmd)}: {exc}"
        ) from exc


def extract_full(record: dict[str, Any]) -> str | None:
    """Pull a display name from a decoded ``get`` record, if present.

    Schema-mapped AVIF rows expose ``fields.Name``; unknown types (e.g. EYES)
    keep FULL under ``fields._unmapped.FULL[].hex`` as raw NUL-terminated bytes.
    """
    fields = record.get("fields") or {}
    name = fields.get("Name")
    if isinstance(name, str) and name:
        return name
    unmapped = fields.get("_unmapped") or {}
    full_entries = unmapped.get("FULL") or []
    if isinstance(full_entries, list) and full_entries:
        hex_str = full_entries[0].get("hex")
        if isinstance(hex_str, str) and hex_str:
            return read_zstring(bytes.fromhex(hex_str))
    return None


def extract(esp_path: Path, esm_bin: str, client: esm_gateway.EsmGateway) -> list[dict]:
    # ``tree`` always emits JSON (no ``--json`` flag on this subcommand).
    tree = run_esm(esm_bin, esp_path, "tree", "--limit", "0")
    if not isinstance(tree, list):
        raise RuntimeError(f"expected tree JSON array, got {type(tree).__name__}")

    rows: list[dict] = []
    for group in tree:
        label = group.get("label") or {}
        sig = label.get("sig")
        if not isinstance(sig, str) or not sig:
            continue
        # `Op::ListTypeRecords` -- the same op `esm list --type SIG --json`
        # sends -- via the warm-daemon gateway instead of its own subprocess.
        rows.extend(client.list_type(str(esp_path), sig, limit=0))

    if not rows:
        return []

    formids = [row["form_id"] for row in rows]
    # Two or more targets → JSON array tagged with ``sel``; batch all at once.
    records = run_esm(esm_bin, esp_path, "get", *formids, "--json")
    if not isinstance(records, list):
        raise RuntimeError(
            f"expected get JSON array for {len(formids)} targets, "
            f"got {type(records).__name__}"
        )
    by_formid = {rec["header"]["form_id"]: rec for rec in records}

    out: list[dict] = []
    for row in rows:
        formid = row["form_id"]
        editor_id = row.get("editor_id")
        # xEdit authored a few EDIDs in the pseudo-plugin as display-style
        # labels ("Kill Streak", "Projectiles Fired"); real EditorIDs never
        # contain spaces, so normalize them away.
        if editor_id is not None:
            editor_id = editor_id.replace(" ", "")
        entry: dict = {
            "formid": formid,
            "type": row["record_type"],
            "editor_id": editor_id,
        }
        rec = by_formid.get(formid)
        if rec is not None:
            full = extract_full(rec)
            if full:
                entry["full"] = full
        out.append(entry)

    out.sort(key=lambda e: e["formid"])
    return out


def main() -> None:
    if not HARDCODED_ESP.exists():
        print(f"Missing {HARDCODED_ESP}", file=sys.stderr)
        sys.exit(1)
    esm_bin = shutil.which("esm")
    if esm_bin is None:
        print(
            "Missing esm binary on PATH (build with `cargo build --release`)",
            file=sys.stderr,
        )
        sys.exit(1)
    try:
        client = esm_gateway.ensure_daemon(esm_bin, HARDCODED_ESP)
    except esm_gateway.DaemonError as exc:
        print(f"failed to reach the esm daemon for `list`: {exc}", file=sys.stderr)
        sys.exit(1)
    try:
        entries = extract(HARDCODED_ESP, esm_bin, client)
    except RuntimeError as exc:
        print(str(exc), file=sys.stderr)
        sys.exit(1)
    finally:
        client.close()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({len(entries)} entries)", file=sys.stderr)


if __name__ == "__main__":
    main()
