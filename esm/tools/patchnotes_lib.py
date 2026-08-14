#!/usr/bin/env python3
"""
patchnotes_lib.py — shared library for the FO76 patch-notes generation
pipeline: pipeline wire shapes and the handful of helpers genuinely read by
two or more downstream stages (mechanical rendering, bundle building/
slicing/triage, lint checks, manifest bookkeeping).

Owns:
  - The wire-shape TypedDicts shared across ≥2 consumers: `RecordEntry`
    (comprehensive.json), `Member`/`Edge`/`BundleAnchor`/`Bundle`
    (bundles.json, read by `build_bundles.py`), `TierInfo`/`RolloutShape`
    (`triage_bundles.py`), `RuleContext` (`run_lints.py`) — plus
    `RecordStatus`/`MemberRole`/`TierName`, the `Literal` aliases those
    TypedDicts share, and `LintFinding` (currently unreferenced, but scoped
    to the same lint-pipeline domain as `RuleContext`, not to the
    render_comprehensive-only engine below).
  - Small formatting helpers `run_lints.py` also imports directly:
    `annotate_ref`/`fmt_num`/`is_formid_str` (and the `format_scalar`/
    `is_curve` they recurse through).
  - JSON payload validation (`validate_record_entry`/`validate_member`/
    `validate_edge`/`validate_bundle_anchor`/`validate_bundle`/
    `validate_bundles_payload`/`validate_comprehensive_payload`), now read
    by four pipeline-stage entry points.
  - Patch manifest read/write (`load_manifest`/`write_manifest`/
    `new_manifest`).

Everything else that used to live here — cut/deprecation detection, VMAD
raw-hex decoding, the generic keyed-array pairing engine, `_array_diff`
normalization, `ChangeEntry` construction (`extract_changes`), redundant-
count suppression, common-change collapsing, and FormID reference
harvesting — was read by exactly one consumer, `render_comprehensive.py`,
and moved to the sibling `change_entries.py` module (which imports this
module as `pl` for the shared pieces above; this module does not import it
back).

Consumes the raw `esm diff --json` output (`DiffResult` in `src/diff.rs`):
`{"added": [RecordStub], "removed": [RecordStub],
  "changed": [{"stub": RecordStub, "field_changes": {...}, "prev_editor_id"?}],
  "ref_names": {"0x...": {"record_type", "editor_id"?, "name"?, "description"?}}}`.

`field_changes` leaves come in two shapes for arrays — see
`change_entries.py`'s module docstring for the (a)/(b) NEW-vs-LEGACY split;
both normalize to the identical `array` sub-structure on a ChangeEntry via
`change_entries.extract_changes`/`change_entries.smart_array_diff`.

Python 3, stdlib only.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any, Literal, NotRequired, TypedDict, cast

sys.path.insert(0, str(Path(__file__).resolve().parent))

import layout  # noqa: E402

# --------------------------------------------------------------------------
# Pipeline wire shapes (comprehensive.json / bundles.json / triage / lints)
# --------------------------------------------------------------------------

RecordStatus = Literal["added", "removed", "changed"]
MemberRole = Literal["anchor", "satellite", "context"]
TierName = Literal["rollout", "deep", "brief", "drop", "ambiguous"]


class RecordEntry(TypedDict):
    form_id: str
    record_type: str
    editor_id: str | None
    name: str | None
    description: str | None
    status: RecordStatus
    prev_editor_id: str | None
    cut: dict[str, Any] | None
    fields: Any
    refs_out: list[dict[str, str]]
    changes: list[dict[str, Any]]


class Member(TypedDict):
    form_id: str
    record_type: str | None
    editor_id: str | None
    name: str | None
    status: str
    role: MemberRole


Edge = TypedDict(
    "Edge",
    {
        "from": str,
        "to": str,
        "relation": str,
        "label": str,
        "via": list[str],
        "source": str,
    },
)


class BundleAnchor(TypedDict):
    form_id: str
    record_type: str | None
    editor_id: str | None
    name: str | None
    status: str


class Bundle(TypedDict):
    category: str
    category_label: str
    category_rule: str | None
    title: str
    anchor: BundleAnchor
    members: list[Member]
    edges: list[Edge]
    bug_watch: bool
    lint_ids: list[str]
    id: str


class TierInfo(TypedDict):
    tier: TierName
    reason: str | None
    bucket: str | None


class LintFinding(TypedDict):
    rule: str
    severity: str
    form_id: str
    message: str
    data: dict[str, Any]
    id: NotRequired[str]
    bundle_id: NotRequired[str]


class RolloutShape(TypedDict):
    record_type: str | None
    paths: list[str]
    record_count: int
    example_form_ids: list[str]


class RuleContext(TypedDict):
    """The `ctx` dict every `run_lints.py` rule function (`rule_name(ctx) ->
    Iterable[dict]`) receives -- built once per run by `run_lints.
    build_context` and threaded read-only through every rule except
    `_notes`, a mutable out-parameter accumulator: rules (and
    `_RuleRecordTally.append_note`) `.append()` a one-line summary there
    when they swallow a per-record/per-check error, and `run_lints.run_lints`
    folds it into `lints.json`'s `meta.notes` afterward.

    `client` is an `esm_gateway.EsmGateway | esm_gateway.FakeGateway`-shaped
    object (duck-typed as `Any` here -- this module is a dependency-free
    leaf imported by every pipeline stage, including ones with no reason to
    also import esm_gateway.py, so it does not import that sibling module
    just to spell this one field's type)."""

    records: dict[str, Any]
    ref_names: dict[str, Any]
    bundles: list[Bundle]
    client: Any
    new_esm: str | None
    old_esm: str | None
    settings: dict[str, Any]
    _notes: list[str]


# --------------------------------------------------------------------------
# Constants
# --------------------------------------------------------------------------

SCHEMA_VERSION = 1

#: `manifest.json`'s `stages.narrative` section's own schema version
#: (independent of SCHEMA_VERSION above, which covers diff/comprehensive/
#: bundles/lints shapes). Version 2 is the current "flat" shape written by
#: `update_manifest.py::build_narrative_stage` -- a single
#: `patch_summary_md` path, a flat `discord/` chunk list, and triage tier
#: counts. Version 1 (retired, no longer produced) was the pipeline's older
#: per-category shape: `categories: [{id, label, notes_md, discord_dir,
#: chunk_count, chunks}, ...]`, one `notes/<slug>.md` + `discord/<slug>/`
#: per category. `new_manifest` below seeds a fresh v2-shaped placeholder so
#: the mechanical stage never writes the retired v1 shape.
NARRATIVE_SCHEMA_VERSION = 2


_FORMID_RE = re.compile(r"^0x[0-9A-Fa-f]{8}$")


# --------------------------------------------------------------------------
# Small formatting helpers
# --------------------------------------------------------------------------


def fmt_num(v):
    """Compact number: drop trailing '.0', round floats to 2 dp."""
    if v is None:
        return "?"
    if isinstance(v, float):
        r = round(v, 2)
        return str(int(r)) if r == int(r) else str(r)
    return str(v)


def is_curve(v):
    """True if val is a decoded FormID reference with inlined curve points:
    `{"formid", "curve_path", "curve": [{x,y}, ...]}`."""
    return isinstance(v, dict) and isinstance(v.get("curve"), list)


def is_formid_str(v):
    """True if v is a bare FormID hex string as produced by FormId::display():
    exactly "0x" followed by 8 hex digits (case-insensitive)."""
    return isinstance(v, str) and bool(_FORMID_RE.match(v))


def _format_ref_info(fid, rtype, edid, label):
    if label and edid:
        return f'`{fid}` ({rtype}: `{edid}` *"{label}"*)'
    if edid:
        return f"`{fid}` ({rtype}: `{edid}`)"
    if label:
        return f'`{fid}` ({rtype}: *"{label}"*)'
    return f"`{fid}` ({rtype})"


def annotate_ref(value, ref_names=None):
    """
    Format a FormID-shaped value — a bare hex string, or a resolved reference
    dict `{"formid", "editor_id"?, "record_type"?, "name"?}` — as a readable
    reference: "`0xFFFFFFFF` (TYPE: `EditorID` "Name")". Falls back to the
    bare hex when nothing more is known (a "dangling" FormID with no
    ref_names entry). Prefers `name`, then `description` (from ref_names),
    when choosing the quoted label.
    """
    ref_names = ref_names or {}
    if isinstance(value, str):
        fid = value
        info = ref_names.get(fid)
        if info is None:
            return f"`{fid}`"
        rtype = info.get("record_type", "?")
        edid = info.get("editor_id")
        label = info.get("name") or info.get("description")
        return _format_ref_info(fid, rtype, edid, label)
    if isinstance(value, dict):
        fid = value.get("formid", "?")
        rtype = value.get("record_type", "?")
        edid = value.get("editor_id")
        label = value.get("name") or value.get("Name") or value.get("description")
        return _format_ref_info(fid, rtype, edid, label)
    return format_scalar(value, ref_names)


def format_scalar(v, ref_names=None):
    """Format an arbitrary decoded value for a display cell (no newlines).
    FormID-shaped values (hex strings or resolved stub/curve dicts) are
    annotated via `annotate_ref`. Never raises on unexpected shapes."""
    if v is None:
        return "*(null)*"
    if isinstance(v, bool):
        return f"`{str(v).lower()}`"
    if isinstance(v, (int, float)):
        return f"`{v}`"
    if isinstance(v, str):
        if is_formid_str(v):
            return annotate_ref(v, ref_names)
        s = v[:100] + ("…" if len(v) > 100 else "")
        return f"`{s}`"
    if isinstance(v, dict):
        if v.get("_unresolved") and "lstring_id" in v:
            return f"`[lstring {v['lstring_id']}]` *(unresolved)*"
        if v.get("_raw"):
            return "`[raw hex]`"
        flags = v.get("flags")
        if isinstance(flags, list):
            return f"`{', '.join(flags) or '(none)'}`"
        if is_curve(v):
            return annotate_ref(v.get("formid"), ref_names)
        if "formid" in v and ("editor_id" in v or "record_type" in v):
            return annotate_ref(v, ref_names)
        name = v.get("name") or v.get("Name")
        if name:
            return f"`{name}`"
        return f"`(struct: {', '.join(str(k) for k in list(v.keys())[:4])})`"
    return f"`{repr(v)[:60]}`"


# --------------------------------------------------------------------------
# Runtime validation (JSON process seams)
# --------------------------------------------------------------------------


def _validation_type_name(value: object) -> str:
    return type(value).__name__


def _require_mapping(value: object, path: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise TypeError(f"{path}: expected dict, got {_validation_type_name(value)}")
    return cast(dict[str, Any], value)


def _require_list(value: object, path: str) -> list[Any]:
    if not isinstance(value, list):
        raise TypeError(f"{path}: expected list, got {_validation_type_name(value)}")
    return cast(list[Any], value)


def _require_str(value: object, path: str) -> str:
    if not isinstance(value, str):
        raise TypeError(f"{path}: expected str, got {_validation_type_name(value)}")
    return value


def _require_optional_str(value: object, path: str) -> str | None:
    if value is None:
        return None
    return _require_str(value, path)


def _require_bool(value: object, path: str) -> bool:
    if not isinstance(value, bool):
        raise TypeError(f"{path}: expected bool, got {_validation_type_name(value)}")
    return value


def _require_key(mapping: dict[str, Any], key: str, path: str) -> Any:
    if key not in mapping:
        raise KeyError(f"{path}: missing required key {key!r}")
    return mapping[key]


def _require_literal_str(value: object, path: str, allowed: set[str]) -> str:
    s = _require_str(value, path)
    if s not in allowed:
        raise ValueError(f"{path}: expected one of {sorted(allowed)!r}, got {s!r}")
    return s


def _require_record_status(value: object, path: str) -> RecordStatus:
    s = _require_literal_str(value, path, {"added", "removed", "changed"})
    return cast(RecordStatus, s)


def _require_member_role(value: object, path: str) -> MemberRole:
    s = _require_literal_str(value, path, {"anchor", "satellite", "context"})
    return cast(MemberRole, s)


def validate_record_entry(value: object, *, path: str = "record") -> RecordEntry:
    rec = _require_mapping(value, path)
    entry: RecordEntry = {
        "form_id": _require_str(_require_key(rec, "form_id", path), f"{path}.form_id"),
        "record_type": _require_str(_require_key(rec, "record_type", path), f"{path}.record_type"),
        "editor_id": _require_optional_str(rec.get("editor_id"), f"{path}.editor_id"),
        "name": _require_optional_str(rec.get("name"), f"{path}.name"),
        "description": _require_optional_str(rec.get("description"), f"{path}.description"),
        "status": _require_record_status(_require_key(rec, "status", path), f"{path}.status"),
        "prev_editor_id": _require_optional_str(rec.get("prev_editor_id"), f"{path}.prev_editor_id"),
        "cut": rec.get("cut") if rec.get("cut") is None else _require_mapping(rec["cut"], f"{path}.cut"),
        "fields": _require_key(rec, "fields", path),
        "refs_out": _require_list(_require_key(rec, "refs_out", path), f"{path}.refs_out"),
        "changes": _require_list(_require_key(rec, "changes", path), f"{path}.changes"),
    }
    return entry


def validate_member(value: object, *, path: str = "member") -> Member:
    member = _require_mapping(value, path)
    validated: Member = {
        "form_id": _require_str(_require_key(member, "form_id", path), f"{path}.form_id"),
        "record_type": _require_optional_str(member.get("record_type"), f"{path}.record_type"),
        "editor_id": _require_optional_str(member.get("editor_id"), f"{path}.editor_id"),
        "name": _require_optional_str(member.get("name"), f"{path}.name"),
        "status": _require_str(_require_key(member, "status", path), f"{path}.status"),
        "role": _require_member_role(_require_key(member, "role", path), f"{path}.role"),
    }
    return validated


def validate_edge(value: object, *, path: str = "edge") -> Edge:
    edge = _require_mapping(value, path)
    via = _require_list(_require_key(edge, "via", path), f"{path}.via")
    for i, item in enumerate(via):
        _require_str(item, f"{path}.via[{i}]")
    return {
        "from": _require_str(_require_key(edge, "from", path), f"{path}.from"),
        "to": _require_str(_require_key(edge, "to", path), f"{path}.to"),
        "relation": _require_str(_require_key(edge, "relation", path), f"{path}.relation"),
        "label": _require_str(_require_key(edge, "label", path), f"{path}.label"),
        "via": via,
        "source": _require_str(_require_key(edge, "source", path), f"{path}.source"),
    }


def validate_bundle_anchor(value: object, *, path: str = "anchor") -> BundleAnchor:
    anchor = _require_mapping(value, path)
    return {
        "form_id": _require_str(_require_key(anchor, "form_id", path), f"{path}.form_id"),
        "record_type": _require_optional_str(anchor.get("record_type"), f"{path}.record_type"),
        "editor_id": _require_optional_str(anchor.get("editor_id"), f"{path}.editor_id"),
        "name": _require_optional_str(anchor.get("name"), f"{path}.name"),
        "status": _require_str(_require_key(anchor, "status", path), f"{path}.status"),
    }


def validate_bundle(value: object, *, path: str = "bundle") -> Bundle:
    bundle = _require_mapping(value, path)
    members_raw = _require_list(_require_key(bundle, "members", path), f"{path}.members")
    members = [validate_member(m, path=f"{path}.members[{i}]") for i, m in enumerate(members_raw)]
    edges_raw = _require_list(_require_key(bundle, "edges", path), f"{path}.edges")
    edges = [validate_edge(e, path=f"{path}.edges[{i}]") for i, e in enumerate(edges_raw)]
    lint_ids_raw = _require_list(_require_key(bundle, "lint_ids", path), f"{path}.lint_ids")
    for i, lid in enumerate(lint_ids_raw):
        _require_str(lid, f"{path}.lint_ids[{i}]")
    return {
        "category": _require_str(_require_key(bundle, "category", path), f"{path}.category"),
        "category_label": _require_str(_require_key(bundle, "category_label", path), f"{path}.category_label"),
        "category_rule": _require_optional_str(
            _require_key(bundle, "category_rule", path),
            f"{path}.category_rule",
        ),
        "title": _require_str(_require_key(bundle, "title", path), f"{path}.title"),
        "anchor": validate_bundle_anchor(_require_key(bundle, "anchor", path), path=f"{path}.anchor"),
        "members": members,
        "edges": edges,
        "bug_watch": _require_bool(_require_key(bundle, "bug_watch", path), f"{path}.bug_watch"),
        "lint_ids": lint_ids_raw,
        "id": _require_str(_require_key(bundle, "id", path), f"{path}.id"),
    }


def validate_bundles_payload(value: object, *, label: str = "bundles.json") -> dict[str, Any]:
    root = _require_mapping(value, label)
    bundles = _require_list(_require_key(root, "bundles", label), f"{label}.bundles")
    for i, item in enumerate(bundles):
        validate_bundle(item, path=f"{label}.bundles[{i}]")
    return root


def validate_comprehensive_payload(value: object, *, label: str = "comprehensive.json") -> dict[str, Any]:
    root = _require_mapping(value, label)
    records = _require_mapping(_require_key(root, "records", label), f"{label}.records")
    for fid, rec in records.items():
        validate_record_entry(rec, path=f"{label}.records[{fid!r}]")
    return root


# --------------------------------------------------------------------------
# Manifest helpers
# --------------------------------------------------------------------------


def load_manifest(out_dir):
    """Load `<out_dir>/manifest.json`, or None if it doesn't exist yet."""
    path = layout.manifest_json(out_dir)
    if not path.exists():
        return None
    with path.open(encoding="utf-8") as f:
        return json.load(f)


def write_manifest(out_dir, manifest):
    """Write `manifest` to `<out_dir>/manifest.json` (pretty-printed),
    creating `out_dir` if needed."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    path = layout.manifest_json(out_dir)
    with path.open("w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")


def new_manifest(patch_date, old_token, new_token, new_esm_size, new_esm_mtime, pipeline_version, counts=None):
    """
    Build a fresh manifest dict for the mechanical stage to write:
        {"schema_version": 1, "patch_date": ..., "inputs": {...},
         "counts": {...},
         "stages": {"mechanical": {"completed_at": None, "files": {}},
                    "narrative": {"schema_version": 2, "completed_at": None,
                                  "patch_summary_md": None,
                                  "discord_dir": "discord", "chunk_count": 0,
                                  "chunks": [], "max_chunk_chars": 2000,
                                  "triage": None}}}

    `stages.narrative`'s placeholder shape here matches the LIVE shape
    `update_manifest.py::build_narrative_stage` writes once the narrative
    stage actually runs (schema_version NARRATIVE_SCHEMA_VERSION == 2), not
    the retired per-category shape -- see NARRATIVE_SCHEMA_VERSION's
    docstring above.
    """
    return {
        "schema_version": SCHEMA_VERSION,
        "patch_date": patch_date,
        "inputs": {
            "old_token": old_token,
            "new_token": new_token,
            "new_esm_size": new_esm_size,
            "new_esm_mtime": new_esm_mtime,
            "pipeline_version": pipeline_version,
        },
        "counts": counts or {},
        "stages": {
            "mechanical": {
                "completed_at": None,
                "files": {},
            },
            "narrative": {
                "schema_version": NARRATIVE_SCHEMA_VERSION,
                "completed_at": None,
                "patch_summary_md": None,
                "discord_dir": layout.DISCORD_DIRNAME,
                "chunk_count": 0,
                "chunks": [],
                "max_chunk_chars": 2000,
                "triage": None,
            },
        },
    }
