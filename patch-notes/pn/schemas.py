"""The pipeline's artifact shapes and their strict validators.

Mechanical artifacts that later stages parse (comprehensive.json and its
records' change entries, bundles.json, triage.json, the DEEP slice) are
validated on read and reject keys their shape doesn't have. Agent-written
artifacts -- the
deep writers' reports, the assessor's assessment, the orchestrator's cuts and
usage, the reviewer's review -- follow the skill's prompts; their validators
also reject unknown keys, so a misspelled or drifted field fails its gate
with the JSON path at fault instead of being skipped.

`PIPELINE_VERSION` versions all of them together: `manifest.json` records it
in `inputs.pipeline_version`, and a run is reused only at the same version.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Callable, Literal, NotRequired, TypedDict, TypeVar, cast

from pn import jsonio

#: Bumped whenever any artifact's shape changes.
PIPELINE_VERSION = 5

T = TypeVar("T")

# --------------------------------------------------------------------------
# Mechanical artifacts
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
    dangling_refs: list[str]
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
    title: str
    anchor: BundleAnchor
    members: list[Member]
    edges: list[Edge]
    id: str


class TierInfo(TypedDict):
    tier: TierName
    reason: str | None
    bucket: str | None


class RolloutShape(TypedDict):
    record_type: str | None
    paths: list[str]
    record_count: int
    example_form_ids: list[str]
    #: Changed records sharing this shape that were kept OUT of the rollout
    #: because at least one of their changes is a real numeric delta
    #: (`triage_bundles.is_numeric_change_entry`); they tier normally.
    numeric_excluded_count: int


ClaimStatus = Literal["ok", "mismatch", "unverifiable"]

#: One number (or existence) a deep writer asserted in its draft, in the
#: shape `check_claims.py` re-verifies. `record` is a FormID hex or an
#: EditorID; `path` uses `comprehensive.json`'s ChangeEntry `path` notation
#: (`" / "`-joined field names; an array row is addressed by its
#: `key_display`, written as `[<key_display>]`). Exactly one kind applies:
#:   changed:   `path` + `from` + `to`
#:   existence: `status` in {added, removed}
#:   value:     `path` + `value` + `side` in {old, new} -- a value that did
#:              not change, or one that lives on a referenced record.
#: Declared in the functional form (like `Edge`) because `from` is a keyword.
Claim = TypedDict(
    "Claim",
    {
        "record": str,
        "path": NotRequired[str],
        "status": NotRequired[Literal["added", "removed"]],
        "side": NotRequired[Literal["old", "new"]],
        "value": NotRequired[Any],
        "from": NotRequired[Any],
        "to": NotRequired[Any],
    },
)


class ClaimResult(TypedDict):
    claim: Claim
    status: ClaimStatus
    #: Where the verdict came from: the record's `changes[]`, a live `esm`
    #: lookup, or nowhere (unverifiable).
    source: Literal["changes", "esm", "none"]
    detail: str


# --------------------------------------------------------------------------
# Validation
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


def _reject_unknown_keys(mapping: dict[str, Any], allowed: set[str], path: str) -> None:
    unknown = sorted(set(mapping) - allowed)
    if unknown:
        raise KeyError(f"{path}: unknown key(s) {unknown!r}")


_RECORD_ENTRY_KEYS = set(RecordEntry.__annotations__)
_CHANGE_ENTRY_KEYS = {
    "path", "kind", "from", "to", "from_display", "to_display", "suppressed", "common_group", "array",
}
_CHANGE_KINDS = {"scalar", "string", "raw", "array", "enum", "flags", "formid"}
_ARRAY_KEYS = {"strategy", "reorder_only", "key_fields", "count_from", "count_to", "added", "removed", "changed"}
_ARRAY_ELEMENT_KEYS = {"key_display", "display", "raw"}


def _require_bool(value: object, path: str) -> bool:
    if not isinstance(value, bool):
        raise TypeError(f"{path}: expected bool, got {_validation_type_name(value)}")
    return value


def _require_optional_int(value: object, path: str) -> int | None:
    return None if value is None else _require_int(value, path)


def _validate_array_payload(value: object, path: str) -> None:
    """A ChangeEntry's normalized array edit: every key present and typed,
    each added/removed element displayed, each changed element's inner
    edits valid ChangeEntries."""
    array = _require_mapping(value, path)
    _require_keys(array, path, _ARRAY_KEYS)
    _require_str(array["strategy"], f"{path}.strategy")
    _require_bool(array["reorder_only"], f"{path}.reorder_only")
    if array["key_fields"] is not None:
        _str_list(array["key_fields"], f"{path}.key_fields")
    _require_optional_int(array["count_from"], f"{path}.count_from")
    _require_optional_int(array["count_to"], f"{path}.count_to")
    for side in ("added", "removed"):
        for i, elem in enumerate(_require_list(array[side], f"{path}.{side}")):
            elem_path = f"{path}.{side}[{i}]"
            elem = _require_mapping(elem, elem_path)
            _require_keys(elem, elem_path, _ARRAY_ELEMENT_KEYS)
            _require_str(elem["key_display"], f"{elem_path}.key_display")
            _require_str(elem["display"], f"{elem_path}.display")
    for i, changed in enumerate(_require_list(array["changed"], f"{path}.changed")):
        changed_path = f"{path}.changed[{i}]"
        changed = _require_mapping(changed, changed_path)
        _require_keys(changed, changed_path, {"key_display", "changes"})
        _require_str(changed["key_display"], f"{changed_path}.key_display")
        for j, inner in enumerate(_require_list(changed["changes"], f"{changed_path}.changes")):
            validate_change_entry(inner, path=f"{changed_path}.changes[{j}]")


def validate_change_entry(value: object, *, path: str = "change") -> dict[str, Any]:
    """One ChangeEntry (see `change_entries.py`), nested array edits
    included. An `array` kind carries its array edit; no other kind does."""
    entry = _require_mapping(value, path)
    _require_keys(entry, path, _CHANGE_ENTRY_KEYS)
    _require_str(entry["path"], f"{path}.path")
    kind = _require_literal_str(entry["kind"], f"{path}.kind", _CHANGE_KINDS)
    _require_optional_str(entry["suppressed"], f"{path}.suppressed")
    _require_optional_str(entry["from_display"], f"{path}.from_display")
    _require_optional_str(entry["to_display"], f"{path}.to_display")
    if kind == "array":
        _validate_array_payload(entry["array"], f"{path}.array")
    elif entry["array"] is not None:
        raise ValueError(f"{path}.array: only an array change carries one, not a {kind!r} change")
    return entry


def validate_record_entry(value: object, *, path: str = "record") -> RecordEntry:
    rec = _require_mapping(value, path)
    _reject_unknown_keys(rec, _RECORD_ENTRY_KEYS, path)
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
        "dangling_refs": _require_list(rec.get("dangling_refs", []), f"{path}.dangling_refs"),
        "changes": _require_list(_require_key(rec, "changes", path), f"{path}.changes"),
    }
    for i, change in enumerate(entry["changes"]):
        validate_change_entry(change, path=f"{path}.changes[{i}]")
    return entry


_MEMBER_KEYS = set(Member.__annotations__)


def validate_member(value: object, *, path: str = "member") -> Member:
    member = _require_mapping(value, path)
    _require_keys(member, path, {"form_id", "status", "role"}, _MEMBER_KEYS)
    validated: Member = {
        "form_id": _require_str(member["form_id"], f"{path}.form_id"),
        "record_type": _require_optional_str(member.get("record_type"), f"{path}.record_type"),
        "editor_id": _require_optional_str(member.get("editor_id"), f"{path}.editor_id"),
        "name": _require_optional_str(member.get("name"), f"{path}.name"),
        "status": _require_str(member["status"], f"{path}.status"),
        "role": _require_member_role(member["role"], f"{path}.role"),
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
    return {
        "title": _require_str(_require_key(bundle, "title", path), f"{path}.title"),
        "anchor": validate_bundle_anchor(_require_key(bundle, "anchor", path), path=f"{path}.anchor"),
        "members": members,
        "edges": edges,
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


TIERS: tuple[TierName, ...] = ("rollout", "deep", "brief", "drop", "ambiguous")
_TRIAGE_KEYS = {*TIERS, "stats", "reasons", "rollout_shapes", "inputs"}
_ROLLOUT_SHAPE_KEYS = set(RolloutShape.__annotations__)


def validate_triage(value: object, *, label: str = "triage.json") -> dict[str, Any]:
    """`work/triage.json`: every tier's bundle ids (each bundle in one tier),
    the reasons, stats, rollout shapes and the digest of its inputs. A missing or misspelled tier is
    an error, never an empty tier."""
    root = _require_mapping(value, label)
    _reject_unknown_keys(root, _TRIAGE_KEYS, label)
    seen: dict[str, str] = {}
    for tier in TIERS:
        for i, bid in enumerate(_require_list(_require_key(root, tier, label), f"{label}.{tier}")):
            bid = _require_bundle_id(bid, f"{label}.{tier}[{i}]")
            if bid in seen:
                raise ValueError(f"{label}: {bid} is in both {seen[bid]} and {tier}")
            seen[bid] = tier
    reasons = _require_mapping(_require_key(root, "reasons", label), f"{label}.reasons")
    for bid, reason in reasons.items():
        _require_str(reason, f"{label}.reasons[{bid!r}]")
    _require_mapping(_require_key(root, "stats", label), f"{label}.stats")
    for i, shape in enumerate(_require_list(_require_key(root, "rollout_shapes", label), f"{label}.rollout_shapes")):
        _validate_rollout_shape(shape, f"{label}.rollout_shapes[{i}]")
    _require_str(_require_key(root, "inputs", label), f"{label}.inputs")
    return root


def _validate_rollout_shape(value: object, path: str) -> None:
    shape = _require_mapping(value, path)
    _require_keys(shape, path, _ROLLOUT_SHAPE_KEYS)
    _require_optional_str(shape["record_type"], f"{path}.record_type")
    _str_list(shape["paths"], f"{path}.paths")
    _require_int(shape["record_count"], f"{path}.record_count")
    _str_list(shape["example_form_ids"], f"{path}.example_form_ids")
    _require_int(shape["numeric_excluded_count"], f"{path}.numeric_excluded_count")


_LINT_KEYS = {"rule", "severity", "form_id", "message", "data", "id", "bundle_id"}
_LINT_ID_RE = re.compile(r"^L\d{4,}$")


def validate_lint(value: object, *, path: str = "lint") -> dict[str, Any]:
    """One `run_lints.py` finding, with its central `id` and `bundle_id`."""
    lint = _require_mapping(value, path)
    _require_keys(lint, path, _LINT_KEYS)
    _require_str(lint["rule"], f"{path}.rule")
    _require_literal_str(lint["severity"], f"{path}.severity", {"error", "warn", "info"})
    _require_optional_str(lint["form_id"], f"{path}.form_id")
    _require_str(lint["message"], f"{path}.message")
    _require_mapping(lint["data"], f"{path}.data")
    lint_id = _require_str(lint["id"], f"{path}.id")
    if not _LINT_ID_RE.match(lint_id):
        raise ValueError(f"{path}.id: expected a lint id like 'L0001', got {lint_id!r}")
    if lint["bundle_id"] is not None:
        _require_bundle_id(lint["bundle_id"], f"{path}.bundle_id")
    return lint


def validate_lints_payload(value: object, *, label: str = "lints.json") -> dict[str, Any]:
    root = _require_mapping(value, label)
    _require_keys(root, label, {"meta", "lints"})
    meta = _require_mapping(root["meta"], f"{label}.meta")
    _require_keys(meta, f"{label}.meta", {"generated_at", "rules_run", "counts"}, {"notes"})
    for i, lint in enumerate(_require_list(root["lints"], f"{label}.lints")):
        validate_lint(lint, path=f"{label}.lints[{i}]")
    return root


def validate_diff_payload(value: object, *, label: str = "diff.json") -> dict[str, Any]:
    """`esm diff --json` output: its record lists, each record addressed by
    FormID. Field contents are the diff engine's contract, not checked here."""
    root = _require_mapping(value, label)
    for side in ("added", "removed"):
        for i, stub in enumerate(_require_list(_require_key(root, side, label), f"{label}.{side}")):
            stub_path = f"{label}.{side}[{i}]"
            _require_str(_require_key(_require_mapping(stub, stub_path), "form_id", stub_path), f"{stub_path}.form_id")
    for i, rec in enumerate(_require_list(_require_key(root, "changed", label), f"{label}.changed")):
        rec_path = f"{label}.changed[{i}]"
        rec = _require_mapping(rec, rec_path)
        stub = _require_mapping(_require_key(rec, "stub", rec_path), f"{rec_path}.stub")
        _require_str(_require_key(stub, "form_id", f"{rec_path}.stub"), f"{rec_path}.stub.form_id")
        _require_mapping(_require_key(rec, "field_changes", rec_path), f"{rec_path}.field_changes")
    for key in ("ref_names", "suppressed_counts"):
        if key in root:
            _require_mapping(root[key], f"{label}.{key}")
    return root


_DEEP_SLICE_BUNDLE_KEYS = {"id", "title", "anchor", "members", "edges", "bug_watch", "lint_ids"}


def validate_deep_slice(value: object, *, label: str = "deep-slice.json") -> dict[str, Any]:
    """`work/deep-slice.json` (or one of its parts): the DEEP bundles, as the
    writers see them, and their lints."""
    root = _require_mapping(value, label)
    _reject_unknown_keys(root, {"bundles", "lints"}, label)
    for i, bundle in enumerate(_require_list(_require_key(root, "bundles", label), f"{label}.bundles")):
        bundle_path = f"{label}.bundles[{i}]"
        bundle = _require_mapping(bundle, bundle_path)
        _require_keys(bundle, bundle_path, _DEEP_SLICE_BUNDLE_KEYS)
        validate_bundle(bundle, path=bundle_path)
        _require_bool(bundle["bug_watch"], f"{bundle_path}.bug_watch")
        _str_list(bundle["lint_ids"], f"{bundle_path}.lint_ids")
    for i, lint in enumerate(_require_list(_require_key(root, "lints", label), f"{label}.lints")):
        validate_lint(lint, path=f"{label}.lints[{i}]")
    return root


# --------------------------------------------------------------------------
# Agent-written artifacts
# --------------------------------------------------------------------------

_BUNDLE_ID_RE = re.compile(r"^B\d{4,}$")


class Deferral(TypedDict):
    form_ids: list[str]
    expected_owner: str
    note: str


class Unresolved(TypedDict):
    what: str
    tried: str


class KbProposal(TypedDict):
    kind: Literal["mechanic", "trap"]
    entry: str


class DraftReport(TypedDict):
    """`drafts/deep[.partN].report.json`, one per deep writer (and the
    orchestrator's own `deep.orchestrator.report.json`)."""

    bundles: NotRequired[int]
    bundles_covered: list[str]
    claims: list[Claim]
    lints_confirmed: list[str]
    lints_not_reproduced: list[str]
    unresolved: list[Unresolved]
    deferred: list[Deferral]
    kb_proposals: list[KbProposal]


class AssessedTier(TypedDict):
    tier: Literal["deep", "brief", "drop"]
    reason: str
    bucket: NotRequired[str]


class Assessment(TypedDict):
    """`work/assessment.json`: the assessor's tier for each ambiguous bundle."""

    tiers: dict[str, AssessedTier]


class Cut(TypedDict):
    bundle_id: str
    reason: str


class Cuts(TypedDict):
    """`work/cuts.json`: DEEP stories the summary leaves out, each with a reason."""

    cuts: list[Cut]


class RoleUsage(TypedDict):
    tokens: int


class Usage(TypedDict):
    """`work/usage.json`: token usage the client reported, per subagent role."""

    assessor: NotRequired[RoleUsage]
    writers: NotRequired[list[RoleUsage]]
    reviewer: NotRequired[RoleUsage]


class ReviewFinding(TypedDict):
    severity: Literal["high", "med", "low"]
    summary: str
    location: str
    evidence: str


class Review(TypedDict):
    """`work/review.json`: the cold reviewer's findings."""

    findings: list[ReviewFinding]
    checked: dict[str, int]


def _require_keys(
    mapping: dict[str, Any], path: str, required: set[str], optional: frozenset[str] | set[str] = frozenset()
) -> None:
    """Every required key present and no key outside `required | optional`."""
    for key in sorted(required - mapping.keys()):
        raise KeyError(f"{path}: missing required key {key!r}")
    unknown = sorted(mapping.keys() - required - optional)
    if unknown:
        raise KeyError(f"{path}: unknown key(s) {', '.join(map(repr, unknown))}")


def _require_nonempty_str(value: object, path: str) -> str:
    s = _require_str(value, path)
    if not s.strip():
        raise ValueError(f"{path}: must not be empty")
    return s


def _require_int(value: object, path: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{path}: expected int, got {_validation_type_name(value)}")
    return value


def _require_bundle_id(value: object, path: str) -> str:
    s = _require_str(value, path)
    if not _BUNDLE_ID_RE.match(s):
        raise ValueError(f"{path}: expected a bundle id like 'B0123', got {s!r}")
    return s


def _str_list(value: object, path: str, item: Callable[[object, str], str] = _require_str) -> list[str]:
    return [item(v, f"{path}[{i}]") for i, v in enumerate(_require_list(value, path))]


_CLAIM_KINDS = {
    "changed": ({"record", "path", "from", "to"}, set()),
    "existence": ({"record", "status"}, set()),
    "value": ({"record", "path", "value", "side"}, set()),
}


def claim_kind(claim: dict[str, Any]) -> str:
    """Which of the three claim kinds `claim` is, from its keys."""
    if "status" in claim:
        return "existence"
    if "side" in claim or "value" in claim:
        return "value"
    return "changed"


def validate_claim(value: object, *, path: str = "claim") -> Claim:
    """One claim of exactly one kind:
      changed:   `record`, `path`, `from`, `to`
      existence: `record`, `status` in {added, removed}
      value:     `record`, `path`, `value`, `side` in {old, new}
    """
    claim = _require_mapping(value, path)
    kind = claim_kind(claim)
    required, optional = _CLAIM_KINDS[kind]
    _require_keys(claim, f"{path} ({kind} claim)", required, optional)
    _require_nonempty_str(claim["record"], f"{path}.record")
    if "path" in claim:
        _require_nonempty_str(claim["path"], f"{path}.path")
    if kind == "existence":
        _require_literal_str(claim["status"], f"{path}.status", {"added", "removed"})
    if kind == "value":
        _require_literal_str(claim["side"], f"{path}.side", {"old", "new"})
    return cast(Claim, claim)


def validate_report(value: object, *, label: str = "report") -> DraftReport:
    report = _require_mapping(value, label)
    _require_keys(
        report,
        label,
        {"bundles_covered", "claims"},
        {"bundles", "lints_confirmed", "lints_not_reproduced", "unresolved", "deferred", "kb_proposals"},
    )
    if "bundles" in report:
        _require_int(report["bundles"], f"{label}.bundles")
    for i, d in enumerate(_require_list(report.get("deferred", []), f"{label}.deferred")):
        p = f"{label}.deferred[{i}]"
        _require_keys(_require_mapping(d, p), p, {"form_ids", "expected_owner", "note"})
        _str_list(d["form_ids"], f"{p}.form_ids")
        _require_str(d["expected_owner"], f"{p}.expected_owner")
        _require_str(d["note"], f"{p}.note")
    for i, u in enumerate(_require_list(report.get("unresolved", []), f"{label}.unresolved")):
        p = f"{label}.unresolved[{i}]"
        _require_keys(_require_mapping(u, p), p, {"what", "tried"})
        _require_str(u["what"], f"{p}.what")
        _require_str(u["tried"], f"{p}.tried")
    for i, k in enumerate(_require_list(report.get("kb_proposals", []), f"{label}.kb_proposals")):
        p = f"{label}.kb_proposals[{i}]"
        _require_keys(_require_mapping(k, p), p, {"kind", "entry"}, {"refines"})
        _require_literal_str(k["kind"], f"{p}.kind", {"mechanic", "trap"})
        _require_nonempty_str(k["entry"], f"{p}.entry")
    validated: DraftReport = {
        "bundles_covered": _str_list(report["bundles_covered"], f"{label}.bundles_covered", _require_bundle_id),
        "claims": [validate_claim(c, path=f"{label}.claims[{i}]") for i, c in enumerate(_require_list(report["claims"], f"{label}.claims"))],
        "unresolved": report.get("unresolved", []),
        "deferred": report.get("deferred", []),
        "kb_proposals": report.get("kb_proposals", []),
        "lints_confirmed": _str_list(report.get("lints_confirmed", []), f"{label}.lints_confirmed"),
        "lints_not_reproduced": _str_list(report.get("lints_not_reproduced", []), f"{label}.lints_not_reproduced"),
    }
    if "bundles" in report:
        validated["bundles"] = report["bundles"]
    return validated


def validate_assessment(value: object, *, label: str = "assessment.json") -> Assessment:
    root = _require_mapping(value, label)
    _require_keys(root, label, {"tiers"})
    tiers = _require_mapping(root["tiers"], f"{label}.tiers")
    for bid, info in tiers.items():
        p = f"{label}.tiers[{bid!r}]"
        _require_bundle_id(bid, p)
        _require_keys(_require_mapping(info, p), p, {"tier", "reason"}, {"bucket"})
        _require_literal_str(info["tier"], f"{p}.tier", {"deep", "brief", "drop"})
        _require_nonempty_str(info["reason"], f"{p}.reason")
        if "bucket" in info:
            _require_nonempty_str(info["bucket"], f"{p}.bucket")
    return cast(Assessment, root)


def validate_cuts(value: object, *, label: str = "cuts.json") -> Cuts:
    root = _require_mapping(value, label)
    _require_keys(root, label, {"cuts"})
    for i, cut in enumerate(_require_list(root["cuts"], f"{label}.cuts")):
        p = f"{label}.cuts[{i}]"
        _require_keys(_require_mapping(cut, p), p, {"bundle_id", "reason"})
        _require_bundle_id(cut["bundle_id"], f"{p}.bundle_id")
        _require_nonempty_str(cut["reason"], f"{p}.reason")
    return cast(Cuts, root)


def _validate_role_usage(value: object, path: str) -> RoleUsage:
    role = _require_mapping(value, path)
    _require_keys(role, path, {"tokens"})
    _require_int(role["tokens"], f"{path}.tokens")
    return cast(RoleUsage, role)


def validate_usage(value: object, *, label: str = "usage.json") -> Usage:
    root = _require_mapping(value, label)
    _require_keys(root, label, set(), {"assessor", "writers", "reviewer"})
    for role in ("assessor", "reviewer"):
        if role in root:
            _validate_role_usage(root[role], f"{label}.{role}")
    for i, w in enumerate(_require_list(root.get("writers", []), f"{label}.writers")):
        _validate_role_usage(w, f"{label}.writers[{i}]")
    return cast(Usage, root)


def validate_review(value: object, *, label: str = "review.json") -> Review:
    root = _require_mapping(value, label)
    _require_keys(root, label, {"findings", "checked"})
    for i, f in enumerate(_require_list(root["findings"], f"{label}.findings")):
        p = f"{label}.findings[{i}]"
        _require_keys(_require_mapping(f, p), p, {"severity", "summary", "location", "evidence"})
        _require_literal_str(f["severity"], f"{p}.severity", {"high", "med", "low"})
        for key in ("summary", "location", "evidence"):
            _require_str(f[key], f"{p}.{key}")
    for key, n in _require_mapping(root["checked"], f"{label}.checked").items():
        _require_int(n, f"{label}.checked.{key}")
    return cast(Review, root)


def load(path: str | Path, validate: Callable[..., T]) -> T:
    """Read the JSON at `path` and validate it, labelling errors with the path."""
    return validate(jsonio.read(path), label=str(path))
