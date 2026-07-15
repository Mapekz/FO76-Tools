#!/usr/bin/env python3
"""Tests for tools/triage_bundles.py.

Covers rule matching for each tier (deep/brief/drop/ambiguous), first-match-
wins ordering (both within one rule list and across the deep > brief > drop
> ambiguous priority), ambiguous-digest truncation, the merge-assessment
round-trip, brief-line templating, and rerun determinism. Small synthetic
bundle/comprehensive dicts throughout -- no game data. A handful of tests
also exercise the real shipped tools/patch_notes_tiers.json to confirm it
behaves as documented (mirroring test_build_bundles.py's use of the real
patch_notes_categories.json).
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import triage_bundles as tb  # noqa: E402

SCRIPT_PATH = Path(__file__).resolve().parents[1] / "triage_bundles.py"
REAL_TIERS_PATH = Path(__file__).resolve().parents[1] / "patch_notes_tiers.json"


def load_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


# --------------------------------------------------------------------------
# Fixture builders
# --------------------------------------------------------------------------


def make_member(fid, record_type, editor_id=None, name=None, status="changed", role="anchor"):
    return {
        "form_id": fid, "record_type": record_type, "editor_id": editor_id,
        "name": name, "status": status, "role": role,
    }


def make_bundle(bid, members, category="uncategorized", title=None):
    """members[0] is the anchor (role forced to "anchor"); the rest keep
    their own role (default "satellite" via make_member)."""
    members = [dict(m) for m in members]
    members[0]["role"] = "anchor"
    anchor = members[0]
    return {
        "id": bid,
        "category": category,
        "category_label": category,
        "category_rule": None,
        "title": title or f"{anchor.get('name') or anchor.get('editor_id')} ({anchor['record_type']})",
        "anchor": {k: anchor[k] for k in ("form_id", "record_type", "editor_id", "name", "status")},
        "members": members,
        "edges": [],
        "bug_watch": False,
        "lint_ids": [],
    }


def make_change(path, from_=None, to=None, kind="scalar", suppressed=None, array=None, vmad=None):
    return {
        "path": path, "kind": kind, "from": from_, "to": to,
        "from_display": None, "to_display": None,
        "suppressed": suppressed, "common_group": None,
        "array": array, "vmad": vmad,
    }


def make_record(fid, record_type, editor_id=None, name=None, status="changed",
                 changes=None, cut=None, prev_editor_id=None):
    return {
        "form_id": fid, "record_type": record_type, "editor_id": editor_id, "name": name,
        "description": None, "status": status, "prev_editor_id": prev_editor_id, "cut": cut,
        "fields": None, "refs_out": [], "changes": changes or [],
    }


# A tiny, self-contained config exercising every rule field this module
# supports -- used by the isolated matcher-level tests (separate from the
# handful of tests that load the real shipped patch_notes_tiers.json).
MINI_CONFIG = {
    "field_path_drop_patterns": ["*model*", "*master*references*"],
    "narrative_signal_patterns": ["*name*", "*description*"],
    "deep_rules": [
        {
            "id": "custom_edid",
            "member_scope": "member", "member_match": "any",
            "record_type": ["OMOD"], "edid": ["mod_Custom_*", "*_mod_Custom_*"],
        },
        {
            "id": "post_prefix",
            "member_scope": "member", "member_match": "any",
            "edid": ["POST_*"],
        },
        {
            "id": "added_major",
            "member_scope": "member", "member_match": "any",
            "record_type": ["WEAP", "QUST"], "status": ["added"],
        },
        {
            "id": "substantive_major",
            "member_scope": "member", "member_match": "any",
            "record_type": ["WEAP", "PERK"], "status": ["changed"],
            "require_numeric_change": True,
        },
    ],
    "brief_rules": [
        {
            "id": "all_added", "bucket": "Added",
            "member_scope": "member", "member_match": "all", "status": ["added"],
        },
        {
            "id": "all_removed", "bucket": "Removed",
            "member_scope": "member", "member_match": "all", "status": ["removed"],
        },
        {
            "id": "renamed_or_cut_only", "bucket": "Renamed / Cut",
            "member_scope": "member", "member_match": "all", "status": ["changed"],
            "require_cut_or_renamed": True, "require_no_substantive_change": True,
        },
    ],
    "drop_rules": [
        {
            "id": "refr_only",
            "member_scope": "member", "member_match": "all", "record_type": ["REFR"],
        },
        {
            "id": "cosmetic_only",
            "member_scope": "member", "member_match": "all",
            "record_type": ["SNDR"], "status": ["changed"],
            "require_no_narrative_change": True,
        },
        {
            "id": "field_path_drop",
            "member_scope": "member", "member_match": "all", "status": ["changed"],
            "require_all_changes_drop": True,
        },
    ],
}


# --------------------------------------------------------------------------
# Numeric-change detector (is_numeric_change_entry / _contains_numeric_delta)
# --------------------------------------------------------------------------


class TestNumericValue(unittest.TestCase):
    def test_int_and_float_are_numeric(self):
        self.assertEqual(tb._numeric_value(5), 5.0)
        self.assertEqual(tb._numeric_value(5.5), 5.5)

    def test_bool_is_not_numeric(self):
        # bool is technically an int subclass in Python -- must be excluded
        # explicitly (a flag toggling True/False is not a "numeric stat").
        self.assertIsNone(tb._numeric_value(True))
        self.assertIsNone(tb._numeric_value(False))

    def test_plain_decimal_string_is_numeric(self):
        self.assertEqual(tb._numeric_value("5"), 5.0)
        self.assertEqual(tb._numeric_value("-3.5"), -3.5)

    def test_hex_string_is_not_numeric(self):
        # Covers both FormIDs and flags bitmasks, which share this shape.
        self.assertIsNone(tb._numeric_value("0x50"))
        self.assertIsNone(tb._numeric_value("0x0000ABCD"))

    def test_arbitrary_text_is_not_numeric(self):
        self.assertIsNone(tb._numeric_value("Old flavor text."))
        self.assertIsNone(tb._numeric_value(None))
        self.assertIsNone(tb._numeric_value({"not": "a number"}))


class TestScalarPairIsNumeric(unittest.TestCase):
    def test_differing_numbers(self):
        self.assertTrue(tb._scalar_pair_is_numeric(1, 2))
        self.assertTrue(tb._scalar_pair_is_numeric("1.0", "2.0"))

    def test_equal_numbers_are_not_a_delta(self):
        self.assertFalse(tb._scalar_pair_is_numeric(5, 5))

    def test_one_side_non_numeric_fails(self):
        self.assertFalse(tb._scalar_pair_is_numeric(5, "not a number"))
        self.assertFalse(tb._scalar_pair_is_numeric("0x50", "0x10"))


class TestIsCurveLike(unittest.TestCase):
    def test_inlined_curve_points(self):
        self.assertTrue(tb._is_curve_like({"formid": "0x01", "curve": [{"x": 1, "y": 2}]}))

    def test_bare_curv_stub_with_no_inlined_points(self):
        self.assertTrue(tb._is_curve_like({"formid": "0x01", "editor_id": "CT_Foo", "record_type": "CURV"}))

    def test_non_curve_stub_is_not_curve_like(self):
        self.assertFalse(tb._is_curve_like({"formid": "0x01", "editor_id": "SomeKeyword", "record_type": "KYWD"}))

    def test_bare_string_or_none_is_not_curve_like(self):
        self.assertFalse(tb._is_curve_like("0x0000ABCD"))
        self.assertFalse(tb._is_curve_like(None))


class TestIsNumericChangeEntry(unittest.TestCase):
    def test_scalar_numeric_delta(self):
        self.assertTrue(tb.is_numeric_change_entry(make_change("Data / Damage", 10, 14)))

    def test_scalar_text_delta_is_not_numeric(self):
        self.assertFalse(tb.is_numeric_change_entry(make_change("Description", "old", "new", kind="string")))

    def test_flags_hex_string_delta_is_not_numeric(self):
        self.assertFalse(
            tb.is_numeric_change_entry(make_change("Data / Flags / value", "0x50", "0x10", kind="string"))
        )

    def test_formid_curve_reference_change_is_numeric(self):
        # Real-data shape (NPC_/EXPL curve-table tier swaps): kind=="formid"
        # but either side is a curve-shaped dict.
        entry = make_change(
            "Curve Table / formid",
            {"formid": "0x01", "curve": [{"x": 1, "y": 2}]},
            {"formid": "0x02", "curve": [{"x": 1, "y": 9}]},
            kind="formid",
        )
        self.assertTrue(tb.is_numeric_change_entry(entry))

    def test_formid_bare_curv_stub_reference_change_is_numeric(self):
        # Same, but this pipeline run had no --startup-ba2/--curves-dir, so
        # only a bare CURV-type stub is available (no inlined points) --
        # matches the real "Curve Table / formid" ChangeEntry shape found in
        # actual comprehensive.json output (bare "0x........" hex strings at
        # the entry's own from/to level).
        entry = make_change(
            "Curve Table / formid", "0x0076E9FE", "0x0076EA06", kind="formid",
        )
        # A bare hex-string pair (no resolved stub at all) is NOT
        # detectable as curve-like on its own -- this is the documented gap
        # this test pins down (see the sibling "Curve Table / curve" test
        # below, which is what actually carries the numeric signal in that
        # real-data shape).
        self.assertFalse(tb.is_numeric_change_entry(entry))

    def test_formid_ordinary_reference_change_is_not_numeric(self):
        entry = make_change("Keywords", "0x00123456", "0x00654321", kind="formid")
        self.assertFalse(tb.is_numeric_change_entry(entry))

    def test_array_entry_with_numeric_changed_row_is_numeric(self):
        # Normalized shape: array.changed[].changes is a proper ChangeEntry
        # list (as patchnotes_lib.extract_changes() produces).
        arr = {
            "strategy": "keyed", "count_from": 1, "count_to": 1,
            "added": [], "removed": [],
            "changed": [{"key_display": "x", "changes": [make_change("Value 1", 1.0, 1.25)]}],
        }
        entry = make_change("Data / Properties", None, arr, kind="array", array=arr)
        self.assertTrue(tb.is_numeric_change_entry(entry))

    def test_array_entry_with_raw_unnormalized_changed_row_is_still_numeric(self):
        # Real-data shape found for curve-table point arrays: some array
        # diffs carry a RAW sparse-diff leaf ({"y": {"from":..,"to":..}})
        # in a changed row's "changes" field instead of a properly
        # extract_changes()-processed ChangeEntry list. The generic
        # from/to-pair recursion must catch this shape too, not just the
        # normalized one.
        arr = {
            "strategy": "positional", "count_from": 50, "count_to": 50,
            "added": [], "removed": [],
            "changed": [{"key": {"index": 0}, "index_from": 0, "index_to": 0, "changes": {"y": {"from": 87.0, "to": 346.0}}}],
        }
        entry = make_change("Curve Table / curve", None, arr, kind="array", array=arr)
        self.assertTrue(tb.is_numeric_change_entry(entry))

    def test_array_entry_with_only_text_or_flag_rows_is_not_numeric(self):
        arr = {
            "strategy": "set", "count_from": 2, "count_to": 1,
            "added": [], "removed": [{"key_display": "`Unknown 6`", "display": "`Unknown 6`", "raw": "Unknown 6"}],
            "changed": [],
        }
        entry = make_change("Data / Flags / flags", None, arr, kind="array", array=arr)
        self.assertFalse(tb.is_numeric_change_entry(entry))

    def test_array_entry_added_row_with_numeric_raw_leaf_is_numeric(self):
        # Regression (real OMOD Property row-swap data, see
        # test_omod_property_row_swap_added_removed_numeric_is_deep in
        # TestRealConfig): a brand-new added row has no "before" state to
        # diff against, so a bare Quantity=5 in its already-decoded raw
        # content (a recipe Component gaining a Quantity, per the
        # coordinator's explicit example) is itself the numeric signal --
        # counted via _raw_element_has_numeric_signal, which is safe to be
        # permissive here (see its docstring) since "raw" never contains
        # array-diff bookkeeping.
        arr = {
            "strategy": "keyed", "count_from": 0, "count_to": 1,
            "added": [{"key_display": "x", "display": "x", "raw": {"Component": "0x01", "Quantity": 5}}],
            "removed": [], "changed": [],
        }
        entry = make_change("Components", None, arr, kind="array", array=arr)
        self.assertTrue(tb.is_numeric_change_entry(entry))

    def test_array_entry_added_row_with_curve_reference_is_numeric(self):
        arr = {
            "strategy": "keyed", "count_from": 0, "count_to": 1,
            "added": [
                {"key_display": "x", "display": "x", "raw": {"Curve": {"formid": "0x01", "curve": [{"x": 1, "y": 2}]}}}
            ],
            "removed": [], "changed": [],
        }
        entry = make_change("Effects", None, arr, kind="array", array=arr)
        self.assertTrue(tb.is_numeric_change_entry(entry))

    def test_array_index_metadata_never_counts_as_numeric(self):
        # "index_from"/"index_to"/"count_from"/"count_to" are never
        # literally named "from"/"to", so they must never be mistaken for a
        # numeric delta even though they're plain differing integers.
        arr = {
            "strategy": "positional", "count_from": 3, "count_to": 5,
            "added": [], "removed": [], "changed": [],
        }
        entry = make_change("Some List", None, arr, kind="array", array=arr)
        self.assertFalse(tb.is_numeric_change_entry(entry))

    def test_non_dict_entry_is_not_numeric(self):
        self.assertFalse(tb.is_numeric_change_entry(None))
        self.assertFalse(tb.is_numeric_change_entry("not a dict"))


# --------------------------------------------------------------------------
# DEEP rule matching
# --------------------------------------------------------------------------


class TestDeepRules(unittest.TestCase):
    def test_omod_custom_edid(self):
        bundle = make_bundle("B0001", [make_member("0x01", "OMOD", "mod_Custom_Foo")])
        tier, reason, bucket = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason, bucket), ("deep", "deep:custom_edid", None))

    def test_omod_legendary_style_edid_via_infix_pattern(self):
        bundle = make_bundle("B0001", [make_member("0x01", "OMOD", "Weapon_mod_Custom_Thing")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual(tier, "deep")
        self.assertEqual(reason, "deep:custom_edid")

    def test_plain_omod_edid_does_not_match(self):
        bundle = make_bundle("B0001", [make_member("0x01", "OMOD", "mod_Ordinary_Foo")])
        tier, _reason, _bucket = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertNotEqual(tier, "deep")

    def test_post_prefix_any_type(self):
        bundle = make_bundle("B0001", [make_member("0x01", "MISC", "POST_SomeFeature")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason), ("deep", "deep:post_prefix"))

    def test_added_major_type(self):
        bundle = make_bundle("B0001", [make_member("0x01", "WEAP", "NewGun", status="added")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason), ("deep", "deep:added_major"))

    def test_added_non_major_type_does_not_trigger_added_major(self):
        bundle = make_bundle("B0001", [make_member("0x01", "ARTO", "NewArt", status="added")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertNotEqual(reason, "deep:added_major")

    def test_substantive_change_major_type(self):
        records = {
            "0x01": make_record(
                "0x01", "PERK", "SomePerk", changes=[make_change("Data / Rank", 1, 2)]
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "SomePerk")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_major"))

    def test_major_type_with_only_a_flags_hex_string_change_does_not_trigger(self):
        # Regression: a flags bitmask is rendered as a "0x.."-prefixed hex
        # STRING (e.g. a HAZD's Data/Flags/value going "0x50" -> "0x10"),
        # syntactically hex digits but not a numeric stat -- must not
        # auto-DEEP a "major type" bundle whose only change is one
        # unmapped flag bit.
        records = {
            "0x01": make_record(
                "0x01", "PERK", "SomePerk",
                changes=[make_change("Data / Flags / value", "0x50", "0x10", kind="string")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "SomePerk")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertNotEqual(tier, "deep")
        self.assertNotEqual(reason, "deep:substantive_major")

    def test_major_type_with_only_a_text_change_does_not_trigger(self):
        # Regression: a BOOK's holotape text edit (or, here, a PERK's plain
        # Description edit) is a real, non-suppressed, non-rename change --
        # but it isn't NUMERIC, so it must be left for the assessor rather
        # than auto-promoted.
        records = {
            "0x01": make_record(
                "0x01", "PERK", "SomePerk",
                changes=[make_change("Description", "Old flavor text.", "New flavor text.", kind="string")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "SomePerk")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertNotEqual(tier, "deep")
        self.assertNotEqual(reason, "deep:substantive_major")

    def test_major_type_with_zero_changes_does_not_trigger_substantive_rule(self):
        records = {"0x01": make_record("0x01", "PERK", "SomePerk", changes=[])}
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "SomePerk")])
        tier, _reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertNotEqual(tier, "deep")

    def test_major_type_with_only_suppressed_changes_does_not_trigger(self):
        records = {
            "0x01": make_record(
                "0x01", "PERK", "SomePerk",
                changes=[make_change("Object Bounds", 1, 2, suppressed="noise")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "SomePerk")])
        tier, _reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertNotEqual(tier, "deep")

    def test_editor_id_only_change_does_not_trigger_substantive_rule(self):
        # Regression: render_comprehensive.py emits an "Editor ID" leaf
        # ChangeEntry for every rename, in ADDITION to prev_editor_id/cut --
        # a bundle whose only change is that rename must not look
        # "substantive" (it would wrongly promote a plain cut-vaulting
        # rename like Foo -> zzzFoo to a deep writeup instead of routing it
        # to brief_rules/renamed_or_cut_only).
        records = {
            "0x01": make_record(
                "0x01", "PERK", "zzzSomePerk", prev_editor_id="SomePerk",
                changes=[make_change("Editor ID", "SomePerk", "zzzSomePerk")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "zzzSomePerk")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertNotEqual(tier, "deep")
        self.assertNotEqual(reason, "deep:substantive_major")


# --------------------------------------------------------------------------
# BRIEF rule matching
# --------------------------------------------------------------------------


class TestBriefRules(unittest.TestCase):
    def test_all_added_bucket(self):
        bundle = make_bundle("B0001", [make_member("0x01", "ARTO", "NewArt", status="added")])
        tier, reason, bucket = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason, bucket), ("brief", "brief:all_added", "Added"))

    def test_mixed_added_and_changed_does_not_match_all_added(self):
        bundle = make_bundle("B0001", [
            make_member("0x01", "ARTO", "NewArt", status="added"),
            make_member("0x02", "ARTO", "OtherArt", status="changed", role="satellite"),
        ])
        _tier, reason, _bucket = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertNotEqual(reason, "brief:all_added")

    def test_all_removed_bucket(self):
        bundle = make_bundle("B0001", [make_member("0x01", "SNDR", "OldSound", status="removed")])
        tier, reason, bucket = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason, bucket), ("brief", "brief:all_removed", "Removed"))

    def test_renamed_or_cut_only(self):
        records = {
            "0x01": make_record(
                "0x01", "PERK", "zzz_OldPerk", changes=[],
                cut={"marker": "ZZZ", "confidence": "high", "kind": "newly_deprecated"},
                prev_editor_id="OldPerk",
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "zzz_OldPerk")])
        tier, reason, bucket = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason, bucket), ("brief", "brief:renamed_or_cut_only", "Renamed / Cut"))

    def test_renamed_only_with_editor_id_change_entry_is_brief_not_deep(self):
        # The "Editor ID" ChangeEntry render_comprehensive.py emits for every
        # rename must not itself count as "substantive" -- a plain rename
        # (no cut marker, just prev_editor_id) with nothing else touched
        # routes to brief_rules/renamed_or_cut_only.
        records = {
            "0x01": make_record(
                "0x01", "PERK", "NewName", prev_editor_id="OldName",
                changes=[make_change("Editor ID", "OldName", "NewName")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "NewName")])
        tier, reason, bucket = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason, bucket), ("brief", "brief:renamed_or_cut_only", "Renamed / Cut"))

    def test_renamed_but_also_has_substantive_change_is_not_brief(self):
        # A rename PLUS a real stat delta belongs in a deep writeup, not a
        # one-liner -- require_no_substantive_change must disqualify it.
        records = {
            "0x01": make_record(
                "0x01", "PERK", "NewName", changes=[make_change("Data / Rank", 1, 2)],
                prev_editor_id="OldName",
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "NewName")])
        tier, reason, _bucket = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_major"))


# --------------------------------------------------------------------------
# DROP rule matching
# --------------------------------------------------------------------------


class TestDropRules(unittest.TestCase):
    def test_refr_only_placement(self):
        bundle = make_bundle("B0001", [make_member("0x01", "REFR", "SomeRef", status="changed")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason), ("drop", "drop:refr_only"))

    def test_refr_mixed_with_other_type_does_not_match_refr_only(self):
        bundle = make_bundle("B0001", [
            make_member("0x01", "REFR", "SomeRef", status="changed"),
            make_member("0x02", "MISC", "SomeItem", status="changed", role="satellite"),
        ])
        records = {
            "0x02": make_record("0x02", "MISC", "SomeItem", changes=[make_change("Model / Model Filename", "a", "b")]),
        }
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        # Falls through to the generic field_path_drop rule instead (both
        # members are "changed" and their only change matches *model*).
        self.assertEqual((tier, reason), ("drop", "drop:field_path_drop"))

    def test_cosmetic_type_only_no_narrative_change(self):
        records = {
            "0x01": make_record("0x01", "SNDR", "SomeSound", changes=[make_change("Data / Volume", 1.0, 0.8)])
        }
        bundle = make_bundle("B0001", [make_member("0x01", "SNDR", "SomeSound")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason), ("drop", "drop:cosmetic_only"))

    def test_cosmetic_type_disqualified_by_narrative_signal(self):
        records = {
            "0x01": make_record("0x01", "SNDR", "SomeSound", changes=[make_change("Full Name", "Old", "New")])
        }
        bundle = make_bundle("B0001", [make_member("0x01", "SNDR", "SomeSound")])
        tier, _reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertNotEqual(tier, "drop")

    def test_field_path_drop_patterns_generic(self):
        records = {
            "0x01": make_record("0x01", "STAT", "SomeStatic", changes=[make_change("Model / Model Filename", "a", "b")])
        }
        bundle = make_bundle("B0001", [make_member("0x01", "STAT", "SomeStatic")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason), ("drop", "drop:field_path_drop"))

    def test_lctn_master_references_array_drops(self):
        records = {
            "0x01": make_record(
                "0x01", "LCTN", "SomeLocation",
                changes=[make_change("Master Persist Location References", None, {"count_from": 1, "count_to": 1}, kind="array")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "LCTN", "SomeLocation")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason), ("drop", "drop:field_path_drop"))

    def test_field_path_drop_requires_at_least_one_change_not_vacuous(self):
        # Zero changes at all must NOT match require_all_changes_drop (that
        # would silently drop a pure-rename/empty-diff bundle instead of
        # routing it to brief_rules/renamed_or_cut_only or ambiguous).
        records = {"0x01": make_record("0x01", "STAT", "SomeStatic", changes=[])}
        bundle = make_bundle("B0001", [make_member("0x01", "STAT", "SomeStatic")])
        tier, _reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual(tier, "ambiguous")

    def test_one_non_matching_change_blocks_field_path_drop(self):
        records = {
            "0x01": make_record(
                "0x01", "STAT", "SomeStatic",
                changes=[make_change("Model / Model Filename", "a", "b"), make_change("Data / Custom Field", 1, 2)],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "STAT", "SomeStatic")])
        tier, _reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual(tier, "ambiguous")


# --------------------------------------------------------------------------
# Ambiguous fallback
# --------------------------------------------------------------------------


class TestAmbiguousFallback(unittest.TestCase):
    def test_unmatched_bundle_is_ambiguous(self):
        records = {
            "0x01": make_record("0x01", "MISC", "SomeItem", changes=[make_change("Data / Custom Field", 1, 2)])
        }
        bundle = make_bundle("B0001", [make_member("0x01", "MISC", "SomeItem")])
        tier, reason, bucket = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason, bucket), ("ambiguous", None, None))


# --------------------------------------------------------------------------
# First-match-wins ordering
# --------------------------------------------------------------------------


class TestFirstMatchWins(unittest.TestCase):
    def test_first_matching_deep_rule_wins_within_deep_rules(self):
        # Matches BOTH custom_edid (via the "*_mod_Custom_*" infix pattern)
        # and post_prefix (POST_* prefix) -- custom_edid is listed first.
        bundle = make_bundle("B0001", [make_member("0x01", "OMOD", "POST_mod_Custom_Foo")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason), ("deep", "deep:custom_edid"))

    def test_deep_outranks_drop_when_both_would_match(self):
        # A WEAP with only one change, on a "model"-matching path, but with
        # an actual NUMERIC delta (e.g. a model scale tweak): drop_rules/
        # field_path_drop would match in isolation (single changed member,
        # one drop-pattern-matching change) but deep_rules/substantive_major
        # must win because DEEP is evaluated first and this change clears
        # the numeric bar (unlike a pure model-FILENAME swap, which
        # wouldn't -- see test_major_type_with_only_a_text_change_does_not_
        # trigger for that case).
        records = {
            "0x01": make_record("0x01", "WEAP", "SomeGun", changes=[make_change("Model Scale", 1.0, 2.0)])
        }
        bundle = make_bundle("B0001", [make_member("0x01", "WEAP", "SomeGun")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_major"))

    def test_pure_model_filename_swap_on_major_type_no_longer_auto_deeps(self):
        # Regression: this exact scenario used to demonstrate "deep
        # outranks drop" before require_numeric_change tightened the rule --
        # a bare filename string swap on a "major type" is not itself a
        # numeric stat delta, so it now correctly falls through to
        # drop_rules/field_path_drop instead.
        records = {
            "0x01": make_record("0x01", "WEAP", "SomeGun", changes=[make_change("Model / Model Filename", "a", "b")])
        }
        bundle = make_bundle("B0001", [make_member("0x01", "WEAP", "SomeGun")])
        tier, reason, _ = tb.assign_tier(bundle, records, MINI_CONFIG)
        self.assertEqual((tier, reason), ("drop", "drop:field_path_drop"))

    def test_deep_outranks_brief_when_both_would_match(self):
        # An added WEAP would also satisfy brief_rules/all_added (all
        # non-context members added) -- deep_rules/added_major must win.
        bundle = make_bundle("B0001", [make_member("0x01", "WEAP", "NewGun", status="added")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason), ("deep", "deep:added_major"))

    def test_drop_outranks_brief_when_both_would_match(self):
        # Regression: an all-REFR bundle with status "removed" would also
        # satisfy brief_rules/all_removed (existence-is-the-story) -- but
        # drop_rules/refr_only must win, since REFR placement churn is
        # world positioning, never gameplay content, regardless of status.
        bundle = make_bundle("B0001", [make_member("0x01", "REFR", "SomeRef", status="removed")])
        tier, reason, _ = tb.assign_tier(bundle, {}, MINI_CONFIG)
        self.assertEqual((tier, reason), ("drop", "drop:refr_only"))


# --------------------------------------------------------------------------
# compute_bundle_tiers / build_triage_payload
# --------------------------------------------------------------------------


class TestBuildTriagePayload(unittest.TestCase):
    def setUp(self):
        self.bundles = [
            make_bundle("B0001", [make_member("0x01", "OMOD", "mod_Custom_Foo")]),
            make_bundle("B0002", [make_member("0x02", "ARTO", "NewArt", status="added")]),
            make_bundle("B0003", [make_member("0x03", "REFR", "SomeRef")]),
            make_bundle("B0004", [make_member("0x04", "MISC", "Mystery")]),
        ]
        self.records = {
            "0x04": make_record("0x04", "MISC", "Mystery", changes=[make_change("Data / Custom", 1, 2)]),
        }

    def test_stats_and_bucketing(self):
        tiers = tb.compute_bundle_tiers(self.bundles, self.records, MINI_CONFIG)
        payload = tb.build_triage_payload(self.bundles, tiers)
        self.assertEqual(payload["deep"], ["B0001"])
        self.assertEqual(payload["brief"], ["B0002"])
        self.assertEqual(payload["drop"], ["B0003"])
        self.assertEqual(payload["ambiguous"], ["B0004"])
        self.assertEqual(
            payload["stats"],
            {"total_bundles": 4, "deep": 1, "brief": 1, "drop": 1, "ambiguous": 1},
        )

    def test_reasons_only_include_bundles_with_a_reason(self):
        tiers = tb.compute_bundle_tiers(self.bundles, self.records, MINI_CONFIG)
        payload = tb.build_triage_payload(self.bundles, tiers)
        self.assertEqual(
            payload["reasons"],
            {"B0001": "deep:custom_edid", "B0002": "brief:all_added", "B0003": "drop:refr_only"},
        )
        self.assertNotIn("B0004", payload["reasons"])

    def test_lists_are_sorted(self):
        bundles = [make_bundle(f"B{i:04d}", [make_member(f"0x{i:02X}", "REFR")]) for i in (3, 1, 2)]
        tiers = tb.compute_bundle_tiers(bundles, {}, MINI_CONFIG)
        payload = tb.build_triage_payload(bundles, tiers)
        self.assertEqual(payload["drop"], ["B0001", "B0002", "B0003"])

    def test_extra_stats_merged(self):
        tiers = tb.compute_bundle_tiers(self.bundles, self.records, MINI_CONFIG)
        payload = tb.build_triage_payload(self.bundles, tiers, extra_stats={"resolved_by_assessor": 2})
        self.assertEqual(payload["stats"]["resolved_by_assessor"], 2)


# --------------------------------------------------------------------------
# deep-slice.json shape
# --------------------------------------------------------------------------


class TestDeepSlicePayload(unittest.TestCase):
    def test_bundle_dicts_stripped_to_writer_contract_keys(self):
        bundle = make_bundle("B0001", [make_member("0x01", "OMOD", "mod_Custom_Foo")])
        payload = tb.build_deep_slice_payload([bundle], {})
        self.assertEqual(
            set(payload["bundles"][0].keys()),
            {"id", "title", "anchor", "members", "edges", "bug_watch", "lint_ids"},
        )
        self.assertNotIn("category", payload["bundles"][0])
        self.assertNotIn("category_label", payload["bundles"][0])
        self.assertNotIn("category_rule", payload["bundles"][0])

    def test_top_level_shape(self):
        payload = tb.build_deep_slice_payload([], {})
        self.assertEqual(set(payload.keys()), {"schema_version", "bundles", "lints"})

    def test_relevant_lints_included(self):
        bundle = make_bundle("B0001", [make_member("0x01", "OMOD", "mod_Custom_Foo")])
        bundle["lint_ids"] = ["L0001"]
        lints_by_id = {
            "L0001": {"id": "L0001", "bundle_id": "B0001", "rule": "x", "severity": "info", "message": "m"},
            "L0002": {"id": "L0002", "bundle_id": "B0002", "rule": "y", "severity": "info", "message": "m"},
        }
        payload = tb.build_deep_slice_payload([bundle], lints_by_id)
        self.assertEqual([l["id"] for l in payload["lints"]], ["L0001"])


# --------------------------------------------------------------------------
# Ambiguous digest + truncation
# --------------------------------------------------------------------------


class TestAmbiguousDigest(unittest.TestCase):
    def test_digest_shape(self):
        records = {
            "0x01": make_record("0x01", "MISC", "Mystery", changes=[make_change("Data / Custom", 1, 2)])
        }
        bundle = make_bundle("B0001", [make_member("0x01", "MISC", "Mystery", name="Mystery Item")])
        digest = tb.build_ambiguous_digest(bundle, records, 100_000, 200)
        self.assertEqual(digest["id"], "B0001")
        self.assertEqual(digest["anchor"]["record_type"], "MISC")
        self.assertEqual(digest["anchor"]["name"], "Mystery Item")
        self.assertEqual(len(digest["members"]), 1)
        # No from_display/to_display supplied in this fixture -> falls back
        # to summarize_change's raw-scalar rendering (no backticks).
        self.assertIn("Data / Custom: 1 -> 2", digest["members"][0]["changes"][0])

    def test_context_members_excluded_from_digest(self):
        bundle = make_bundle("B0001", [
            make_member("0x01", "MISC", "Mystery"),
            make_member("0x02", "KYWD", "SomeKeyword", role="context", status="unchanged"),
        ])
        digest = tb.build_ambiguous_digest(bundle, {}, 100_000, 200)
        self.assertEqual([m["form_id"] for m in digest["members"]], ["0x01"])

    def test_suppressed_changes_excluded(self):
        records = {
            "0x01": make_record(
                "0x01", "MISC", "Mystery",
                changes=[make_change("Object Bounds", 1, 2, suppressed="noise"), make_change("Data / Real", 1, 2)],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "MISC", "Mystery")])
        digest = tb.build_ambiguous_digest(bundle, records, 100_000, 200)
        self.assertEqual(len(digest["members"][0]["changes"]), 1)
        self.assertIn("Data / Real", digest["members"][0]["changes"][0])

    def test_change_summary_truncated_to_max_chars(self):
        ce = make_change("Some Very Long Path Name", "x" * 300, "y" * 300)
        summary = tb.summarize_change(ce, 50)
        self.assertLessEqual(len(summary), 50)
        self.assertTrue(summary.endswith("…"))

    def test_bundle_digest_capped_and_marked_truncated(self):
        many_changes = [make_change(f"Field {i}", "x" * 50, "y" * 50) for i in range(50)]
        records = {"0x01": make_record("0x01", "MISC", "Mystery", changes=many_changes)}
        bundle = make_bundle("B0001", [make_member("0x01", "MISC", "Mystery")])
        digest = tb.build_ambiguous_digest(bundle, records, 400, 200)
        self.assertTrue(digest.get("truncated"))
        self.assertLessEqual(tb._digest_size(digest), 400)
        self.assertLess(len(digest["members"][0]["changes"]), 50)

    def test_small_digest_is_not_marked_truncated(self):
        records = {"0x01": make_record("0x01", "MISC", "Mystery", changes=[make_change("Data / X", 1, 2)])}
        bundle = make_bundle("B0001", [make_member("0x01", "MISC", "Mystery")])
        digest = tb.build_ambiguous_digest(bundle, records, 100_000, 200)
        self.assertNotIn("truncated", digest)


# --------------------------------------------------------------------------
# brief-lines.md templating
# --------------------------------------------------------------------------


class TestBriefLines(unittest.TestCase):
    def test_grouped_under_headings_in_bucket_order(self):
        bundles = {
            "B0001": make_bundle("B0001", [make_member("0x01", "ARTO", "NewArt", name="New Art", status="added")]),
            "B0002": make_bundle("B0002", [make_member("0x02", "SNDR", "OldSound", name="Old Sound", status="removed")]),
        }
        tiers = {
            "B0001": {"tier": "brief", "reason": "brief:all_added", "bucket": "Added"},
            "B0002": {"tier": "brief", "reason": "brief:all_removed", "bucket": "Removed"},
        }
        md = tb.render_brief_lines(["B0001", "B0002"], bundles, tiers, {})
        lines = md.splitlines()
        self.assertEqual(lines[0], "### Added")
        self.assertIn("- **New Art** (ARTO): added", lines)
        self.assertIn("### Removed", lines)
        self.assertIn("- **Old Sound** (SNDR): removed", lines)
        # Added section must appear before Removed (BUCKET_ORDER).
        self.assertLess(lines.index("### Added"), lines.index("### Removed"))

    def test_renamed_cut_line_newly_deprecated(self):
        bundle = make_bundle("B0001", [make_member("0x01", "PERK", "zzz_OldPerk", name="Old Perk")])
        records = {
            "0x01": make_record(
                "0x01", "PERK", "zzz_OldPerk", name="Old Perk",
                cut={"marker": "ZZZ", "confidence": "high", "kind": "newly_deprecated"},
                prev_editor_id="OldPerk",
            )
        }
        tiers = {"B0001": {"tier": "brief", "reason": "brief:renamed_or_cut_only", "bucket": "Renamed / Cut"}}
        md = tb.render_brief_lines(["B0001"], {"B0001": bundle}, tiers, records)
        self.assertIn("renamed", md)
        self.assertIn("OldPerk", md)
        self.assertIn("vaulted", md.lower())

    def test_empty_brief_set_yields_empty_string(self):
        self.assertEqual(tb.render_brief_lines([], {}, {}, {}), "")

    def test_bundle_ids_rendered_in_sorted_order_within_a_bucket(self):
        bundles = {
            "B0002": make_bundle("B0002", [make_member("0x02", "ARTO", "B", name="B", status="added")]),
            "B0001": make_bundle("B0001", [make_member("0x01", "ARTO", "A", name="A", status="added")]),
        }
        tiers = {
            "B0001": {"tier": "brief", "reason": "brief:all_added", "bucket": "Added"},
            "B0002": {"tier": "brief", "reason": "brief:all_added", "bucket": "Added"},
        }
        md = tb.render_brief_lines(["B0002", "B0001"], bundles, tiers, {})
        self.assertLess(md.index("**A**"), md.index("**B**"))


# --------------------------------------------------------------------------
# merge_assessment (unit)
# --------------------------------------------------------------------------


class TestMergeAssessment(unittest.TestCase):
    def test_resolves_ambiguous_bundles(self):
        tiers = {
            "B0001": {"tier": "ambiguous", "reason": None, "bucket": None},
            "B0002": {"tier": "deep", "reason": "deep:x", "bucket": None},
        }
        assessment = {"tiers": {"B0001": {"tier": "drop", "reason": "pure bookkeeping"}}}
        resolved = tb.merge_assessment(tiers, assessment)
        self.assertEqual(resolved, 1)
        self.assertEqual(tiers["B0001"]["tier"], "drop")
        self.assertEqual(tiers["B0001"]["reason"], "assessor:pure bookkeeping")

    def test_does_not_touch_already_resolved_bundles(self):
        tiers = {"B0001": {"tier": "deep", "reason": "deep:x", "bucket": None}}
        assessment = {"tiers": {"B0001": {"tier": "drop", "reason": "should be ignored"}}}
        tb.merge_assessment(tiers, assessment)
        self.assertEqual(tiers["B0001"]["tier"], "deep")
        self.assertEqual(tiers["B0001"]["reason"], "deep:x")

    def test_unmentioned_ambiguous_bundle_stays_ambiguous(self):
        tiers = {"B0001": {"tier": "ambiguous", "reason": None, "bucket": None}}
        resolved = tb.merge_assessment(tiers, {"tiers": {}})
        self.assertEqual(resolved, 0)
        self.assertEqual(tiers["B0001"]["tier"], "ambiguous")

    def test_invalid_tier_value_leaves_bundle_ambiguous(self):
        tiers = {"B0001": {"tier": "ambiguous", "reason": None, "bucket": None}}
        assessment = {"tiers": {"B0001": {"tier": "not_a_real_tier", "reason": "?"}}}
        resolved = tb.merge_assessment(tiers, assessment)
        self.assertEqual(resolved, 0)
        self.assertEqual(tiers["B0001"]["tier"], "ambiguous")

    def test_brief_resolution_defaults_bucket_to_other(self):
        tiers = {"B0001": {"tier": "ambiguous", "reason": None, "bucket": None}}
        assessment = {"tiers": {"B0001": {"tier": "brief", "reason": "existence is the story"}}}
        tb.merge_assessment(tiers, assessment)
        self.assertEqual(tiers["B0001"]["bucket"], "Other")

    def test_missing_reason_still_records_assessor_prefix(self):
        tiers = {"B0001": {"tier": "ambiguous", "reason": None, "bucket": None}}
        assessment = {"tiers": {"B0001": {"tier": "drop"}}}
        tb.merge_assessment(tiers, assessment)
        self.assertTrue(tiers["B0001"]["reason"].startswith("assessor:"))


# --------------------------------------------------------------------------
# File-based round trip: run_triage / run_merge_assessment / determinism
# --------------------------------------------------------------------------


class TempOutDir:
    """A temp dir laid out like a pipeline output dir: bundles.json +
    comprehensive.json (mirrors test_slice_bundles.py's helper)."""

    def __init__(self, bundles_data, comprehensive_data):
        self.bundles_data = bundles_data
        self.comprehensive_data = comprehensive_data
        self._tmp = tempfile.TemporaryDirectory()  # non-Optional: __exit__ always has one to clean up

    def __enter__(self):
        out_dir = Path(self._tmp.name)
        (out_dir / "bundles.json").write_text(json.dumps(self.bundles_data), encoding="utf-8")
        (out_dir / "comprehensive.json").write_text(json.dumps(self.comprehensive_data), encoding="utf-8")
        return out_dir

    def __exit__(self, *exc):
        self._tmp.cleanup()


def _sample_pipeline_output():
    bundles = [
        make_bundle("B0001", [make_member("0x01", "OMOD", "mod_Custom_Foo")]),
        make_bundle("B0002", [make_member("0x02", "ARTO", "NewArt", status="added")]),
        make_bundle("B0003", [make_member("0x03", "REFR", "SomeRef")]),
        make_bundle("B0004", [make_member("0x04", "MISC", "Mystery")]),
    ]
    bundles_data = {"schema_version": 1, "meta": {}, "bundles": bundles, "lints": []}
    records = {
        "0x04": make_record("0x04", "MISC", "Mystery", changes=[make_change("Data / Custom", 1, 2)]),
    }
    comprehensive_data = {"schema_version": 1, "meta": {}, "records": records, "common_changes": [], "ref_names": {}}
    return bundles_data, comprehensive_data


def _write_mini_tiers_config(tmp_path):
    path = tmp_path / "tiers.json"
    path.write_text(json.dumps(MINI_CONFIG), encoding="utf-8")
    return path


class TestRunTriage(unittest.TestCase):
    def test_writes_all_four_files(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            tiers_path = _write_mini_tiers_config(out_dir)
            result = tb.run_triage(out_dir, tiers_path)

            self.assertEqual(result["triage"]["deep"], ["B0001"])
            self.assertTrue((out_dir / "work" / "triage.json").is_file())
            self.assertTrue((out_dir / "work" / "deep-slice.json").is_file())
            self.assertTrue((out_dir / "work" / "ambiguous.json").is_file())
            self.assertTrue((out_dir / "work" / "brief-lines.md").is_file())

            on_disk_triage = load_json(out_dir / "work" / "triage.json")
            self.assertEqual(on_disk_triage, result["triage"])

    def test_missing_bundles_json_raises(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(FileNotFoundError):
                tb.run_triage(tmp, REAL_TIERS_PATH)

    def test_determinism_across_reruns(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            tiers_path = _write_mini_tiers_config(out_dir)
            tb.run_triage(out_dir, tiers_path)
            first = {
                p.name: p.read_bytes()
                for p in (out_dir / "work").glob("*")
                if p.name in ("triage.json", "deep-slice.json", "ambiguous.json", "brief-lines.md")
            }
            tb.run_triage(out_dir, tiers_path)
            second = {
                p.name: p.read_bytes()
                for p in (out_dir / "work").glob("*")
                if p.name in ("triage.json", "deep-slice.json", "ambiguous.json", "brief-lines.md")
            }
            self.assertEqual(first, second)


class TestRunMergeAssessment(unittest.TestCase):
    def test_round_trip_resolves_ambiguous_and_updates_all_files(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            tiers_path = _write_mini_tiers_config(out_dir)
            first = tb.run_triage(out_dir, tiers_path)
            self.assertEqual(first["triage"]["ambiguous"], ["B0004"])

            assessment_path = out_dir / "work" / "assessment.json"
            assessment_path.write_text(
                json.dumps({"tiers": {"B0004": {"tier": "brief", "reason": "cosmetic mystery"}}}),
                encoding="utf-8",
            )

            second = tb.run_merge_assessment(out_dir, assessment_path, tiers_path)
            self.assertEqual(second["triage"]["ambiguous"], [])
            self.assertIn("B0004", second["triage"]["brief"])
            self.assertEqual(second["triage"]["reasons"]["B0004"], "assessor:cosmetic mystery")
            self.assertEqual(second["triage"]["stats"]["resolved_by_assessor"], 1)

            on_disk = load_json(out_dir / "work" / "triage.json")
            self.assertEqual(on_disk, second["triage"])

            brief_md = (out_dir / "work" / "brief-lines.md").read_text(encoding="utf-8")
            self.assertIn("Mystery", brief_md)

    def test_still_ambiguous_bundles_remain_after_merge(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            tiers_path = _write_mini_tiers_config(out_dir)
            tb.run_triage(out_dir, tiers_path)
            assessment_path = out_dir / "work" / "assessment.json"
            assessment_path.write_text(json.dumps({"tiers": {}}), encoding="utf-8")
            result = tb.run_merge_assessment(out_dir, assessment_path, tiers_path)
            self.assertEqual(result["triage"]["ambiguous"], ["B0004"])
            self.assertEqual(result["triage"]["stats"]["resolved_by_assessor"], 0)


# --------------------------------------------------------------------------
# Real shipped config (patch_notes_tiers.json) -- representative rules
# --------------------------------------------------------------------------


class TestRealConfig(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.config = load_json(REAL_TIERS_PATH)

    def test_config_has_expected_top_level_shape(self):
        for key in ("field_path_drop_patterns", "narrative_signal_patterns",
                    "deep_rules", "brief_rules", "drop_rules"):
            self.assertIn(key, self.config)
        for rule_list in ("deep_rules", "brief_rules", "drop_rules"):
            ids = [r["id"] for r in self.config[rule_list]]
            self.assertEqual(len(ids), len(set(ids)), f"duplicate rule id in {rule_list}")

    def test_omod_custom_edid_is_deep(self):
        bundle = make_bundle("B0001", [make_member("0x01", "OMOD", "mod_Custom_MechanicsBestFriend")])
        tier, reason, _ = tb.assign_tier(bundle, {}, self.config)
        self.assertEqual(tier, "deep")
        self.assertEqual(reason, "deep:omod_custom_or_legendary_edid")

    def test_refr_only_bundle_drops(self):
        bundle = make_bundle("B0001", [make_member("0x01", "REFR", "SomePlacedRef")])
        tier, reason, _ = tb.assign_tier(bundle, {}, self.config)
        self.assertEqual((tier, reason), ("drop", "drop:refr_only_placement"))

    def test_lctn_master_references_only_change_drops(self):
        records = {
            "0x01": make_record(
                "0x01", "LCTN", "SomeLocation",
                changes=[
                    make_change(
                        "Master Persist Location References", None,
                        {"count_from": 13, "count_to": 13}, kind="array",
                    )
                ],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "LCTN", "SomeLocation")])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertEqual((tier, reason), ("drop", "drop:field_path_drop_patterns"))

    def test_added_weap_is_deep(self):
        bundle = make_bundle("B0001", [make_member("0x01", "WEAP", "NewGun", status="added")])
        tier, reason, _ = tb.assign_tier(bundle, {}, self.config)
        self.assertEqual((tier, reason), ("deep", "deep:added_major_record_type"))

    def test_added_cosmetic_type_is_brief(self):
        bundle = make_bundle("B0001", [make_member("0x01", "ARTO", "NewArt", status="added")])
        tier, reason, bucket = tb.assign_tier(bundle, {}, self.config)
        self.assertEqual((tier, reason, bucket), ("brief", "brief:all_added", "Added"))

    def test_hazd_flag_bit_only_change_is_not_auto_deep(self):
        # Coordinator-flagged regression: a HAZD bundle whose only change is
        # one unmapped flag bit (real shape: Data/Flags/value "0x50" ->
        # "0x10", a hex-string bitmask, not a number) must fall to
        # ambiguous, not auto-DEEP, under the shipped config.
        records = {
            "0x01": make_record(
                "0x01", "HAZD", "SomeHazard",
                changes=[make_change("Data / Flags / value", "0x50", "0x10", kind="string")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "HAZD", "SomeHazard")])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertEqual(tier, "ambiguous")
        self.assertIsNone(reason)

    def test_hazd_with_a_real_numeric_change_is_still_deep(self):
        # Contrast case: a HAZD with an actual numeric delta (e.g. its
        # damage-per-second radius) must still auto-DEEP.
        records = {
            "0x01": make_record(
                "0x01", "HAZD", "SomeHazard",
                changes=[make_change("Data / Radius", 100.0, 150.0)],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "HAZD", "SomeHazard")])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_change_major_record_type"))

    def test_qust_qtfs_numeric_change_is_deep(self):
        # Schema-gap follow-up: QTFS used to decode as an opaque hex blob
        # (never numeric, so a QTFS-only change fell to ambiguous like the
        # HAZD flag-bit case above). Now that it's mapped to a u16 ("QTFS
        # (Repeat Limit?)"), a real value change (e.g. 65535 "no limit" ->
        # 50, the shape seen on SDOW_SQ01_Graves_Repeatable) is a genuine
        # numeric delta on a major record type and must auto-DEEP.
        records = {
            "0x01": make_record(
                "0x01", "QUST", "SomeRepeatableQuest",
                changes=[make_change("QTFS (Repeat Limit?)", 65535, 50)],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "QUST", "SomeRepeatableQuest")])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_change_major_record_type"))

    def test_book_holotape_text_only_change_is_not_auto_deep(self):
        # Coordinator-flagged regression: BOOK isn't even in the major-type
        # list, so a holotape text edit was already excluded at the type
        # filter -- this pins that down explicitly so a future edit to the
        # record_type list can't silently regress it.
        records = {
            "0x01": make_record(
                "0x01", "BOOK", "SomeHolotape",
                changes=[make_change("Description", "Old holotape text.", "New holotape text.", kind="string")],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "BOOK", "SomeHolotape")])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertNotEqual(tier, "deep")

    def test_npc_curve_table_tier_swap_is_deep(self):
        # Real-data shape (SDOW_LvlSlasherFanBossPowerArmorHeavyAuto): an
        # NPC_'s Health curve-table reference swapping from Tier23 to
        # Tier31 -- the "Curve Table / formid" leaf alone is a bare hex
        # string pair (not curve-shaped), but the sibling "Curve Table /
        # curve" entry carries the actual y-value deltas. Must remain DEEP.
        # Every array-kind ChangeEntry below sets BOTH "to" and "array" to
        # the same normalized structure, matching how the real pipeline
        # always populates "array" (see _make_leaf_entry/_make_array_diff_
        # entry in patchnotes_lib.py) -- is_numeric_change_entry reads
        # "array" specifically, never "to", for array-kind dispatch.
        inner_curve_array = {
            "strategy": "positional", "count_from": 50, "count_to": 50,
            "added": [], "removed": [],
            "changed": [
                {
                    "key": {"index": 0}, "index_from": 0, "index_to": 0,
                    "changes": {"y": {"from": 87.0, "to": 346.0}},
                }
            ],
        }
        inner_curve_entry = make_change(
            "Curve Table / curve", None, inner_curve_array, kind="array", array=inner_curve_array,
        )
        outer_properties_array = {
            "strategy": "keyed", "count_from": 1, "count_to": 1, "added": [], "removed": [],
            "changed": [
                {
                    "key_display": "Actor Value=Health",
                    "changes": [
                        inner_curve_entry,
                        make_change("Curve Table / curve_path", "Tier23.json", "Tier31.json", kind="string"),
                        make_change("Curve Table / formid", "0x0076E9FE", "0x0076EA06", kind="formid"),
                    ],
                }
            ],
        }
        properties_entry = make_change(
            "Properties", None, outer_properties_array, kind="array", array=outer_properties_array,
        )
        records = {"0x01": make_record("0x01", "NPC_", "SomeBoss", changes=[properties_entry])}
        bundle = make_bundle("B0001", [make_member("0x01", "NPC_", "SomeBoss")])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_change_major_record_type"))

    def test_omod_property_row_swap_added_removed_numeric_is_deep(self):
        # Real-data shape (SDOW_mod_Legendary_Weapon4_Severing): an OMOD's
        # Properties array trades one Function Type/Property row for
        # another entirely (Enchantments grant -> direct ActorValues
        # modifier) -- a real mechanic rework. There's no "before" state to
        # diff against for a wholesale row swap, so the numeric signal
        # (Value 2: 1 removed, Value 2: 50.0 added) lives in the
        # added/removed rows' raw decoded content, not a from/to pair.
        properties_array = {
            "strategy": "keyed", "key_fields": ["Function Type", "Property"],
            "count_from": 1, "count_to": 1,
            "added": [
                {
                    "key_display": "Function Type=ADD, Property=ActorValues",
                    "display": "...",
                    "raw": {
                        "Value Type": {"value": 6, "name": "FormID,Float"},
                        "Function Type": {"value": 2, "name": "ADD"},
                        "Property": {"value": 94, "name": "ActorValues"},
                        "Value 1": "0x00837DFC", "Value 2": 50.0, "Curve Table": None,
                    },
                }
            ],
            "removed": [
                {
                    "key_display": "Function Type=ADD, Property=Enchantments",
                    "display": "...",
                    "raw": {
                        "Value Type": {"value": 4, "name": "FormID,Int"},
                        "Function Type": {"value": 2, "name": "ADD"},
                        "Property": {"value": 65, "name": "Enchantments"},
                        "Value 1": "0x008E0681", "Value 2": 1, "Curve Table": None,
                    },
                }
            ],
            "changed": [],
        }
        entry = make_change("Data / Properties", None, properties_array, kind="array", array=properties_array)
        records = {
            "0xAA": make_record("0xAA", "PERK", "SomeAssociatedPerk", changes=[]),
            "0x01": make_record("0x01", "OMOD", "SDOW_mod_Legendary_Weapon4_Severing", changes=[entry]),
        }
        # Anchor on PERK (a major type, but no numeric change of its own)
        # with the OMOD as a satellite -- exercises member_scope="member",
        # member_match="any" picking up the OMOD's own numeric signal even
        # though OMOD itself isn't in the major-type list.
        bundle = make_bundle("B0001", [
            make_member("0xAA", "PERK", "SomeAssociatedPerk"),
            make_member("0x01", "OMOD", "SDOW_mod_Legendary_Weapon4_Severing", role="satellite"),
        ])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_change_major_record_type"))

    def test_expl_curve_table_reference_nulled_is_deep(self):
        # Real-data shape (RD01_ExplosionAutoGrenadeLauncher_ResolveBreaker):
        # an EXPL's Damage Curve Table reference going from an inlined
        # curve-shaped dict to null. The inlined "curve" key alone must be
        # enough to qualify -- no need for a sibling "curve"-array entry.
        records = {
            "0x01": make_record(
                "0x01", "EXPL", "SomeExplosion",
                changes=[
                    make_change(
                        "Data / Damage Curve Table",
                        {"formid": "0x0080F20C", "curve_path": "Damage_Universal_Tier28.json",
                         "curve": [{"x": 1.0, "y": 42.0}]},
                        None,
                        kind="formid",
                    )
                ],
            )
        }
        bundle = make_bundle("B0001", [make_member("0x01", "EXPL", "SomeExplosion")])
        tier, reason, _ = tb.assign_tier(bundle, records, self.config)
        self.assertEqual((tier, reason), ("deep", "deep:substantive_change_major_record_type"))


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


class TestCli(unittest.TestCase):
    def test_missing_out_dir_files_is_hard_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            code = tb.main([tmp, "--tiers", str(REAL_TIERS_PATH)])
            self.assertEqual(code, 1)

    def test_missing_tiers_config_is_hard_error(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            code = tb.main([str(out_dir), "--tiers", "/nonexistent/tiers.json"])
            self.assertEqual(code, 1)

    def test_main_success_writes_files(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            tiers_path = _write_mini_tiers_config(out_dir)
            code = tb.main([str(out_dir), "--tiers", str(tiers_path)])
            self.assertEqual(code, 0)
            self.assertTrue((out_dir / "work" / "triage.json").is_file())

    def test_subprocess_smoke_test_with_real_config(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            result = subprocess.run(
                [sys.executable, str(SCRIPT_PATH), str(out_dir)],
                capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertTrue((out_dir / "work" / "triage.json").is_file())
            self.assertIn("deep", result.stderr)

    def test_merge_assessment_cli_flag(self):
        bundles_data, comprehensive_data = _sample_pipeline_output()
        with TempOutDir(bundles_data, comprehensive_data) as out_dir:
            tiers_path = _write_mini_tiers_config(out_dir)
            tb.main([str(out_dir), "--tiers", str(tiers_path)])
            assessment_path = out_dir / "work" / "assessment.json"
            assessment_path.write_text(
                json.dumps({"tiers": {"B0004": {"tier": "drop", "reason": "no story"}}}), encoding="utf-8"
            )
            code = tb.main([
                str(out_dir), "--tiers", str(tiers_path),
                "--merge-assessment", str(assessment_path),
            ])
            self.assertEqual(code, 0)
            triage = load_json(out_dir / "work" / "triage.json")
            self.assertIn("B0004", triage["drop"])
            self.assertEqual(triage["reasons"]["B0004"], "assessor:no story")


if __name__ == "__main__":
    unittest.main()
