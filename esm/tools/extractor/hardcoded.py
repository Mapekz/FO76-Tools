#!/usr/bin/env python3
"""Extract xEdit's hardcoded-engine-form pseudo-plugin → schema/hardcoded_fo76.json.

Fallout76.esm only contains records with FormIDs the game's data files define.
A handful of low FormIDs (roughly < 0x800) are instead hardcoded into the game
executable itself — e.g. AVIF `KillStreak` at 0x399 — and never appear as a
record in the ESM. xEdit ships these as a pseudo-plugin at
``Core/Hardcoded/Fallout76.esp`` inside the TES5Edit checkout, purely so it has
something to resolve those FormIDs against.

This script parses that plugin directly as raw ESP bytes (TES4 header, GRUP
groups, record headers, subrecord headers — the same on-disk format as
``src/format.rs``/``src/reader.rs``) and emits a small lookup table of
``{formid, type, editor_id}`` entries, checked in at ``schema/hardcoded_fo76.json``
since the TES5Edit checkout is not always present (same rationale as
``schema/fo76.json``).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TES5 = ROOT.parent / "TES5Edit"
HARDCODED_ESP = TES5 / "Core" / "Hardcoded" / "Fallout76.esp"
OUT = ROOT / "schema" / "hardcoded_fo76.json"

HEADER_SIZE = 24
SUBRECORD_HEADER_SIZE = 6
COMPRESSED_FLAG = 0x0004_0000


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


def parse_subrecords(data: bytes) -> list[tuple[str, bytes]]:
    """Parse a record's data payload into (signature, data) subrecords.

    Mirrors the XXXX oversized-subrecord rule in `parse_subrecords` (`src/reader.rs`):
    a 6-byte XXXX subrecord whose 4-byte payload carries the real size precedes an
    oversized subrecord whose own header `size` field is 0.
    """
    out: list[tuple[str, bytes]] = []
    pos = 0
    pending_size: int | None = None
    n = len(data)
    while pos + SUBRECORD_HEADER_SIZE <= n:
        sig = data[pos : pos + 4].decode("ascii", errors="replace")
        size = int.from_bytes(data[pos + 4 : pos + 6], "little")
        pos += SUBRECORD_HEADER_SIZE

        if sig == "XXXX" and size == 4 and pos + 4 <= n:
            pending_size = int.from_bytes(data[pos : pos + 4], "little")
            pos += 4
            continue

        if size == 0 and pending_size is not None:
            size = pending_size
        pending_size = None

        end = min(pos + size, n)
        out.append((sig, data[pos:end]))
        pos = end
    return out


def walk_records(data: bytes, pos: int, end: int, out: list[dict]) -> None:
    """Recursively walk a GRUP/record container, appending hardcoded-form entries."""
    while pos + HEADER_SIZE <= end:
        sig = data[pos : pos + 4]
        if sig == b"GRUP":
            group_size = int.from_bytes(data[pos + 4 : pos + 8], "little")
            group_type = int.from_bytes(data[pos + 12 : pos + 16], "little", signed=True)
            group_end = pos + group_size  # group_size includes the 24-byte header
            if group_end > end:
                raise ValueError(f"GRUP extends beyond container at offset {pos}")
            _ = group_type  # only top-level record groups (type 0) are expected here
            walk_records(data, pos + HEADER_SIZE, group_end, out)
            pos = group_end
            continue

        record_sig = sig.decode("ascii", errors="replace")
        data_size = int.from_bytes(data[pos + 4 : pos + 8], "little")
        flags = int.from_bytes(data[pos + 8 : pos + 12], "little")
        form_id = int.from_bytes(data[pos + 12 : pos + 16], "little")
        record_end = pos + HEADER_SIZE + data_size
        if record_end > end:
            raise ValueError(f"record extends beyond container at offset {pos}")

        if record_sig != "TES4":
            if flags & COMPRESSED_FLAG:
                raise ValueError(
                    f"compressed record {record_sig} {form_id:#010x} — "
                    "hardcoded.py does not implement decompression (unexpected in this file)"
                )
            payload = data[pos + HEADER_SIZE : record_end]
            subs = parse_subrecords(payload)
            editor_id = None
            full = None
            for sub_sig, sub_data in subs:
                if sub_sig == "EDID" and editor_id is None:
                    editor_id = read_zstring(sub_data)
                elif sub_sig == "FULL" and full is None:
                    full = read_zstring(sub_data)
            # xEdit authored a few EDIDs in the pseudo-plugin as display-style
            # labels ("Kill Streak", "Projectiles Fired"); real EditorIDs never
            # contain spaces, so normalize them away.
            if editor_id is not None:
                editor_id = editor_id.replace(" ", "")
            entry = {
                "formid": f"0x{form_id:08X}",
                "type": record_sig,
                "editor_id": editor_id,
            }
            if full:
                entry["full"] = full
            out.append(entry)

        pos = record_end


def extract(esp_path: Path) -> list[dict]:
    data = esp_path.read_bytes()
    if data[0:4] != b"TES4":
        raise ValueError(f"expected TES4 record at start of {esp_path}")
    tes4_data_size = int.from_bytes(data[4:8], "little")
    start = HEADER_SIZE + tes4_data_size
    out: list[dict] = []
    walk_records(data, start, len(data), out)
    out.sort(key=lambda e: e["formid"])
    return out


def main() -> None:
    if not HARDCODED_ESP.exists():
        print(f"Missing {HARDCODED_ESP}", file=sys.stderr)
        sys.exit(1)
    entries = extract(HARDCODED_ESP)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({len(entries)} entries)", file=sys.stderr)


if __name__ == "__main__":
    main()
