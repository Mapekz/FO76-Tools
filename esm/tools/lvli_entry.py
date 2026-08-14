#!/usr/bin/env python3
"""
lvli_entry.py — the single owner of "read one leveled-list (LVLI) entry",
consumed by `patchnotes_lib.py`, `run_lints.py`, and `lvli_audit.py`. Before
this module existed, all three carried independent copies that had already
drifted on three axes:

  - **Unwrap**: `patchnotes_lib.py`/`run_lints.py` both used the safe
    pass-through `e.get("Leveled List Entry", e)` — an already-unwrapped
    entry (no wrapper key at all) passes through intact. `lvli_audit.py`
    instead did `raw.get("Leveled List Entry") or {}`, which silently turns
    an unwrapped entry, or one whose wrapper value is itself falsy, into
    `{}` — dropping it from the audit entirely rather than merely
    mis-defaulting it. `unwrap_entry` below is the safe pass-through; this
    was a genuine bug in `lvli_audit.py`, not a style difference.
  - **Reference key**: `patchnotes_lib.py`/`run_lints.py` both fell back to
    `"Item"` when `"Reference"` was absent; `lvli_audit.py` read only
    `"Reference"`. The `Reference`/`Item` fallback is kept — two known key
    names for the same concept, not an omission worth narrowing.
  - **Quantity default**: `patchnotes_lib.py` defaulted a missing
    `Quantity`/`Count` to `1`; `run_lints.py` defaulted to `None`. `None` is
    the canonical behavior here — never fabricate a value absent from the
    data. A caller that needs a display value handles `None` explicitly
    (e.g. `patchnotes_lib.fmt_num` already renders it as `"?"`).

Deferred, not fixed here: `lvli_audit.py`'s `resolve_min_level` also
GLOB-resolves a `"Minimum Level Global"` field when present, falling back to
a static `"Minimum Level"` otherwise; `patchnotes_lib.py`/`run_lints.py`'s
level-reading logic (`ue.get("Minimum Level", ue.get("Level"))`) does not
attempt any GLOB resolution at all. Unifying that is deliberately out of
scope for this module: it would require verifying whether GLOB values are
even available at `patchnotes_lib.py`'s/`run_lints.py`'s point in the
pipeline — they operate over an already-decoded `diff.json`/
`comprehensive.json`, not a live ESM `Database`, so a GLOB reference may not
carry a resolved value at all by the time it reaches them. That
investigation hasn't happened yet; this is a known gap for a future pass.

Python 3, stdlib only.
"""

from __future__ import annotations

from typing import Any


def unwrap_entry(e: Any) -> dict:
    """The inner dict actually holding an LVLI entry's comparable fields.
    Some shapes wrap the entry in a named container (`{"Leveled List
    Entry": {...}}`); others don't. An already-unwrapped dict passes
    through intact — this is the safe pass-through, not the
    silent-drop-to-`{}` behavior a caller might be tempted to write with
    `e.get("Leveled List Entry") or {}` (that turns an unwrapped entry, or
    one whose wrapper value is itself falsy, into `{}`, silently discarding
    it instead of reading it). Also guards against a malformed `"Leveled
    List Entry"` value that isn't itself a dict (e.g. a stray scalar in a
    corrupt/unexpected decode shape) — `run_lints.py`'s prior copy of this
    logic had this guard and `patchnotes_lib.py`'s didn't; every caller
    gets it now rather than only the one that happened to add it."""
    if not isinstance(e, dict):
        return {}
    inner = e.get("Leveled List Entry", e)
    return inner if isinstance(inner, dict) else {}


def entry_reference(ue: dict) -> Any:
    """The referenced item from an unwrapped entry: `"Reference"`, falling
    back to `"Item"` — two known key names for the same concept across
    different decode shapes. Returns whatever shape the field carries (a
    bare FormID hex string, a resolved reference stub dict, or `None` if
    neither key is present)."""
    return ue.get("Reference") or ue.get("Item")


def entry_quantity(ue: dict) -> Any:
    """The entry's quantity: `"Quantity"`, falling back to `"Count"`,
    defaulting to `None` (never `1`) when both are absent — a missing
    quantity in the data must never be fabricated as an explicit value."""
    return ue.get("Quantity", ue.get("Count"))
