#!/usr/bin/env python3
"""Tests for tools/patchnotes_lib.py."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import patchnotes_lib as pl  # noqa: E402

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def load_fixture(name):
    with open(FIXTURES_DIR / name, encoding="utf-8") as f:
        return json.load(f)


def find_changed(diff_data, form_id):
    for c in diff_data["changed"]:
        if c["stub"]["form_id"] == form_id:
            return c
    raise KeyError(form_id)


def find_entry(entries, path):
    for e in entries:
        if e["path"] == path:
            return e
    raise KeyError(path)


# ---------------------------------------------------------------------------
# Cut / deprecation classification
# ---------------------------------------------------------------------------


class TestClassifyCut(unittest.TestCase):
    def test_zzz_prefix_high_confidence(self):
        ci = pl.classify_cut("zzz_OldPerk", prev_edid="OldPerk")
        self.assertEqual(ci["kind"], "newly_deprecated")
        self.assertEqual(ci["marker"], "ZZZ")
        self.assertEqual(ci["confidence"], "high")

    def test_still_cut_when_prev_also_marked(self):
        ci = pl.classify_cut("zzz_OldPerk02", prev_edid="zzz_OldPerk01")
        self.assertEqual(ci["kind"], "still_cut")

    def test_added_cut_when_no_prev(self):
        ci = pl.classify_cut("CUT_PlaceholderThing")
        self.assertEqual(ci["kind"], "added_cut")
        self.assertEqual(ci["marker"], "CUT")

    def test_poster_false_positive_guarded(self):
        # "Poster01" must NOT be misdetected as a POST_ marker.
        self.assertIsNone(pl.classify_cut("Poster01"))
        self.assertIsNone(pl.classify_cut("PosterFrame"))

    def test_post_prefix_still_detected_with_delimiter(self):
        ci = pl.classify_cut("POST_FutureQuest")
        self.assertIsNotNone(ci)
        self.assertEqual(ci["marker"], "POST")

    def test_post_bare_prefix_before_camelcase(self):
        # Bare (non-underscore-delimited) POST prefix is deliberately kept at
        # "low" confidence -- more conservative than other markers' "medium"
        # -- since POST is the marker most prone to false positives.
        ci = pl.classify_cut("POSTUpdateItem")
        self.assertIsNotNone(ci)
        self.assertEqual(ci["marker"], "POST")
        self.assertEqual(ci["confidence"], "low")

    def test_suffix_marker_low_confidence(self):
        ci = pl.classify_cut("SomeQuest_DELETE")
        self.assertIsNotNone(ci)
        self.assertEqual(ci["marker"], "DELETE")
        self.assertEqual(ci["confidence"], "low")

    def test_clean_edid_not_cut(self):
        self.assertIsNone(pl.classify_cut("AssaultRifle"))

    def test_annotate_cut_on_changed_record(self):
        diff_data = load_fixture("diff_small.json")
        rec = find_changed(diff_data, "0x01005001")
        ci = pl.annotate_cut(rec)
        self.assertIsNotNone(ci)
        self.assertEqual(ci["kind"], "newly_deprecated")
        self.assertEqual(ci["marker"], "ZZZ")

    def test_annotate_cut_on_bare_stub(self):
        stub = {"editor_id": "CUT_Thing", "form_id": "0x01"}
        ci = pl.annotate_cut(stub)
        self.assertEqual(ci["kind"], "added_cut")

    def test_annotate_cut_none_when_clean(self):
        stub = {"editor_id": "AssaultRifle", "form_id": "0x01"}
        self.assertIsNone(pl.annotate_cut(stub))


# ---------------------------------------------------------------------------
# VMAD hex decode / diff
# ---------------------------------------------------------------------------


class TestVmad(unittest.TestCase):
    def test_decode_vmad_props_roundtrip(self):
        diff_data = load_fixture("diff_small.json")
        rec = find_changed(diff_data, "0x01004001")
        hex_pair = rec["field_changes"]["Virtual Machine Adapter"]["hex"]
        old = pl.decode_vmad_props(hex_pair["from"])
        new = pl.decode_vmad_props(hex_pair["to"])
        self.assertEqual(old, {"Count": 3, "Flag": False})
        self.assertEqual(new, {"Count": 5, "Flag": True})

    def test_diff_vmad_shape(self):
        diff_data = load_fixture("diff_small.json")
        rec = find_changed(diff_data, "0x01004001")
        hex_pair = rec["field_changes"]["Virtual Machine Adapter"]["hex"]
        result = pl.diff_vmad(hex_pair["from"], hex_pair["to"])
        self.assertEqual(result["added"], {})
        self.assertEqual(result["removed"], {})
        self.assertEqual(result["changed"], {"Count": {"from": 3, "to": 5}, "Flag": {"from": False, "to": True}})

    def test_decode_vmad_props_invalid_hex_returns_empty(self):
        self.assertEqual(pl.decode_vmad_props("not hex!!"), {})

    def test_extract_changes_detects_vmad_kind(self):
        diff_data = load_fixture("diff_small.json")
        rec = find_changed(diff_data, "0x01004001")
        entries = pl.extract_changes(rec["field_changes"], diff_data["ref_names"])
        entry = find_entry(entries, "Virtual Machine Adapter / hex")
        self.assertEqual(entry["kind"], "vmad")
        self.assertEqual(entry["vmad"]["changed"]["Count"], {"from": 3, "to": 5})
        self.assertEqual(entry["vmad"]["changed"]["Flag"], {"from": False, "to": True})


# ---------------------------------------------------------------------------
# extract_changes: one kind per record type in the fixture
# ---------------------------------------------------------------------------


class TestExtractChangesKinds(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.diff_data = load_fixture("diff_small.json")
        cls.ref_names = cls.diff_data["ref_names"]

    def _entries(self, form_id):
        rec = find_changed(self.diff_data, form_id)
        return pl.extract_changes(rec["field_changes"], self.ref_names)

    def test_scalar_kind(self):
        entries = self._entries("0x01001001")
        e = find_entry(entries, "Data / Damage")
        self.assertEqual(e["kind"], "scalar")
        self.assertEqual(e["from"], 10)
        self.assertEqual(e["to"], 14)
        self.assertEqual(e["from_display"], "`10`")
        self.assertEqual(e["to_display"], "`14`")
        self.assertIsNone(e["suppressed"])

    def test_string_kind(self):
        entries = self._entries("0x01001001")
        e = find_entry(entries, "Model / Model Path")
        self.assertEqual(e["kind"], "string")
        self.assertIn("rifle_old.nif", e["from_display"])
        self.assertIn("rifle_new.nif", e["to_display"])

    def test_enum_kind(self):
        entries = self._entries("0x01001001")
        e = find_entry(entries, "Data / Firing Type")
        self.assertEqual(e["kind"], "enum")
        self.assertEqual(e["from_display"], "`Auto`")
        self.assertEqual(e["to_display"], "`Burst`")

    def test_flags_kind(self):
        entries = self._entries("0x01001001")
        e = find_entry(entries, "Data / Weapon Flags")
        self.assertEqual(e["kind"], "flags")
        self.assertIn("Automatic", e["from_display"])
        self.assertIn("Non-Playable", e["to_display"])
        self.assertIn("+Non-Playable", e["to_display"])

    def test_formid_kind_resolved(self):
        entries = self._entries("0x01001001")
        e = find_entry(entries, "Data / Ammo")
        self.assertEqual(e["kind"], "formid")
        self.assertIn("Ammo556Old", e["from_display"])
        self.assertIn("Ammo762New", e["to_display"])

    def test_formid_kind_dangling(self):
        entries = self._entries("0x01001001")
        e = find_entry(entries, "Data / Scope Attachment")
        self.assertEqual(e["kind"], "formid")
        self.assertEqual(e["from"], None)
        self.assertEqual(e["to"], "0x00099999")
        # Dangling: absent from ref_names, so display is just the bare hex.
        self.assertEqual(e["to_display"], "`0x00099999`")

    def test_noise_suppression_object_bounds(self):
        entries = self._entries("0x01001001")
        for path in ("Object Bounds / X1", "Object Bounds / Y1"):
            e = find_entry(entries, path)
            self.assertEqual(e["suppressed"], "noise")

    def test_array_kind_new_shape(self):
        entries = self._entries("0x01002001")
        e = find_entry(entries, "Data / Properties")
        self.assertEqual(e["kind"], "array")
        self.assertIsNotNone(e["array"])
        self.assertEqual(e["array"]["strategy"], "keyed")

    def test_array_kind_legacy_shape(self):
        entries = self._entries("0x01003001")
        e = find_entry(entries, "Entries")
        self.assertEqual(e["kind"], "array")
        self.assertEqual(e["array"]["count_from"], 3)
        self.assertEqual(e["array"]["count_to"], 4)

    def test_vmad_kind(self):
        entries = self._entries("0x01004001")
        e = find_entry(entries, "Virtual Machine Adapter / hex")
        self.assertEqual(e["kind"], "vmad")

    def test_raw_kind_suppressed(self):
        entries = pl.extract_changes(
            {"Unknown Blob": {"from": {"_raw": True, "hex": "aa"}, "to": {"_raw": True, "hex": "bb"}}},
            {},
        )
        e = find_entry(entries, "Unknown Blob")
        self.assertEqual(e["kind"], "raw")
        self.assertEqual(e["suppressed"], "raw")

    def test_exhaustive_nothing_dropped(self):
        # Every entry (including suppressed ones) must remain in the list.
        entries = self._entries("0x01001001")
        paths = {e["path"] for e in entries}
        self.assertIn("Object Bounds / X1", paths)
        suppressed_paths = {e["path"] for e in entries if e["suppressed"]}
        self.assertTrue(suppressed_paths)


# ---------------------------------------------------------------------------
# _array_diff normalization (new shape) — added/removed/changed incl. nested
# ---------------------------------------------------------------------------


class TestArrayDiffNewShape(unittest.TestCase):
    def setUp(self):
        self.diff_data = load_fixture("diff_small.json")
        rec = find_changed(self.diff_data, "0x01002001")
        entries = pl.extract_changes(rec["field_changes"], self.diff_data["ref_names"])
        self.array = find_entry(entries, "Data / Properties")["array"]

    def test_strategy_and_key_fields_preserved(self):
        self.assertEqual(self.array["strategy"], "keyed")
        self.assertEqual(self.array["key_fields"], ["Function Type", "Property"])
        self.assertEqual(self.array["count_from"], 3)
        self.assertEqual(self.array["count_to"], 3)

    def test_added_entry(self):
        self.assertEqual(len(self.array["added"]), 1)
        added = self.array["added"][0]
        self.assertIn("key_display", added)
        self.assertIn("display", added)
        self.assertEqual(added["raw"]["Property"], "NumProjectiles")

    def test_removed_entry(self):
        self.assertEqual(len(self.array["removed"]), 1)
        removed = self.array["removed"][0]
        self.assertEqual(removed["raw"]["Property"], "Speed")

    def test_changed_entry_has_nested_change_entry_list(self):
        # Two "changed" pairs are fixtured: [0] uses plain-scalar key values
        # ("MUL"/"Damage" strings, the pre-Fix-A shape still legitimately
        # emitted when the underlying decoded field is just a string) and
        # [1] uses the enum-object ({"value", "name"}) shape Fix A now
        # preserves on `changed[].key` instead of collapsing to a bare int —
        # see TestArrayDiffEnumObjectKeyShape below for [1]'s key_display.
        self.assertEqual(len(self.array["changed"]), 2)
        changed = self.array["changed"][0]
        self.assertIn("key_display", changed)
        nested = changed["changes"]
        self.assertEqual(len(nested), 1)
        self.assertEqual(nested[0]["path"], "Value 1")
        self.assertEqual(nested[0]["kind"], "scalar")
        self.assertEqual(nested[0]["from"], 1.1)
        self.assertEqual(nested[0]["to"], 1.25)


# ---------------------------------------------------------------------------
# _array_diff normalization — enum-object ({value,name}) key shape.
#
# Fix A (src/diff.rs) now emits the ORIGINAL B-side value for a keyed
# array's `changed[].key` fields instead of the collapsed canonical scalar
# used only for pairing — e.g. `{"value": 1, "name": "MUL+ADD"}` rather than
# bare `1`. changed[0] on the 0x01002001 fixture record above covers the
# (still valid) plain-scalar-key shape; changed[1] covers this enum-object
# shape so both are exercised end to end through extract_changes().
# ---------------------------------------------------------------------------


class TestArrayDiffEnumObjectKeyShape(unittest.TestCase):
    def setUp(self):
        self.diff_data = load_fixture("diff_small.json")
        rec = find_changed(self.diff_data, "0x01002001")
        entries = pl.extract_changes(rec["field_changes"], self.diff_data["ref_names"])
        array = find_entry(entries, "Data / Properties")["array"]
        self.changed = array["changed"][1]

    def test_changed_entry_key_display_renders_enum_names(self):
        # format_scalar() renders an enum {value,name} dict as its name.
        self.assertIn("MUL+ADD", self.changed["key_display"])
        self.assertIn("AimModelMaxConeDegrees", self.changed["key_display"])
        self.assertNotIn("34", self.changed["key_display"])

    def test_nested_value_change_still_reported(self):
        value_entry = find_entry(self.changed["changes"], "Value 1")
        self.assertEqual(value_entry["from"], 1.1)
        self.assertEqual(value_entry["to"], 1.5)


# ---------------------------------------------------------------------------
# _struct_display / _key_dict_display formatting
# ---------------------------------------------------------------------------


class TestStructDisplay(unittest.TestCase):
    def test_dict_values_render_via_format_scalar_not_dropped(self):
        # Previously dict-valued fields were silently dropped from the
        # comprehension; an enum {value,name} dict must now render as its
        # name, and a resolved FormID stub must render as an annotated ref.
        ref_names = {"0x00000099": {"record_type": "WEAP", "editor_id": "SomeGun"}}
        elem = {
            "Function Type": {"value": 1, "name": "MUL+ADD"},
            "Property": {"value": 34, "name": "AimModelMaxConeDegrees"},
            "Value 1": 1.5,
        }
        out = pl._struct_display(elem, ref_names)
        self.assertIn("Function Type=`MUL+ADD`", out)
        self.assertIn("Property=`AimModelMaxConeDegrees`", out)
        self.assertIn("Value 1=`1.5`", out)

    def test_list_and_null_values_skipped(self):
        elem = {"Keywords": ["0x01", "0x02"], "Note": None, "Value 1": 2}
        out = pl._struct_display(elem, {})
        self.assertNotIn("Keywords", out)
        self.assertNotIn("Note", out)
        self.assertIn("Value 1=`2`", out)

    def test_cap_extended_to_six_renderable_fields(self):
        elem = {f"F{i}": i for i in range(8)}
        out = pl._struct_display(elem, {})
        for i in range(6):
            self.assertIn(f"F{i}=`{i}`", out)
        self.assertNotIn("F6=", out)
        self.assertNotIn("F7=", out)

    def test_lists_and_nulls_do_not_count_against_the_cap(self):
        # A field cap of "first 6 renderable fields" means skipped
        # (list/null) fields must not consume a slot — a property row with
        # interleaved list/null fields should still show 6 real values.
        elem = {
            "A": 1, "SkipList": [1, 2], "B": 2, "SkipNull": None,
            "C": 3, "D": 4, "E": 5, "F": 6,
        }
        out = pl._struct_display(elem, {})
        for name, val in (("A", 1), ("B", 2), ("C", 3), ("D", 4), ("E", 5), ("F", 6)):
            self.assertIn(f"{name}=`{val}`", out)

    def test_falls_back_to_formid_stub_annotation(self):
        elem = {"formid": "0x00000001", "record_type": "WEAP", "editor_id": "Foo"}
        out = pl._struct_display(elem, {})
        self.assertIn("WEAP", out)
        self.assertIn("Foo", out)


class TestKeyDictDisplay(unittest.TestCase):
    def test_enum_object_values_render_as_names(self):
        key = {
            "Function Type": {"value": 1, "name": "MUL+ADD"},
            "Property": {"value": 34, "name": "AimModelMaxConeDegrees"},
        }
        out = pl._key_dict_display(key, {})
        self.assertEqual(out, "Function Type=`MUL+ADD`, Property=`AimModelMaxConeDegrees`")

    def test_plain_scalar_key_with_ref_names_annotation(self):
        # LVLI-style key: a bare FormID hex value annotated via ref_names.
        ref_names = {"0x00AA0004": {"record_type": "WEAP", "editor_id": "SomeWeapon"}}
        key = {"Reference": "0x00AA0004", "Minimum Level": 5}
        out = pl._key_dict_display(key, ref_names)
        self.assertIn("Reference=`0x00AA0004` (WEAP: `SomeWeapon`)", out)
        self.assertIn("Minimum Level=`5`", out)

    def test_empty_or_non_dict_key_falls_back_to_format_scalar(self):
        self.assertEqual(pl._key_dict_display({}, {}), pl.format_scalar({}, {}))
        self.assertEqual(pl._key_dict_display(5, {}), pl.format_scalar(5, {}))


# ---------------------------------------------------------------------------
# Legacy whole-array fallback normalization
# ---------------------------------------------------------------------------


class TestLegacyArrayFallback(unittest.TestCase):
    def setUp(self):
        self.diff_data = load_fixture("diff_small.json")
        rec = find_changed(self.diff_data, "0x01003001")
        entries = pl.extract_changes(rec["field_changes"], self.diff_data["ref_names"])
        self.array = find_entry(entries, "Entries")["array"]

    def test_detects_lvli_shape_as_keyed(self):
        self.assertEqual(self.array["strategy"], "keyed")
        self.assertEqual(self.array["key_fields"], ["Reference", "Minimum Level"])

    def test_one_added_one_changed_none_removed(self):
        self.assertEqual(len(self.array["added"]), 1)
        self.assertEqual(len(self.array["removed"]), 0)
        self.assertEqual(len(self.array["changed"]), 1)

    def test_added_entry_is_0xaa0004(self):
        added = self.array["added"][0]
        self.assertEqual(added["raw"]["Leveled List Entry"]["Reference"], "0x00AA0004")

    def test_changed_entry_quantity_delta(self):
        changed = self.array["changed"][0]
        nested = changed["changes"]
        qty_entry = find_entry(nested, "Quantity")
        self.assertEqual(qty_entry["from"], 1)
        self.assertEqual(qty_entry["to"], 3)

    def test_diff_lvli_entries_direct_call_matches(self):
        rec = find_changed(self.diff_data, "0x01003001")
        fc = rec["field_changes"]["Entries"]
        direct = pl.diff_lvli_entries(fc["from"], fc["to"], self.diff_data["ref_names"])
        self.assertEqual(direct, self.array)

    def test_smart_array_diff_unkeyable_scalar_list_falls_back_to_counts(self):
        result = pl.smart_array_diff([1, 2, 3], [1, 2], {})
        self.assertEqual(result["strategy"], "set")
        self.assertEqual(result["count_from"], 3)
        self.assertEqual(result["count_to"], 2)

    def test_smart_array_diff_unrecognized_struct_shape_counts_only(self):
        from_list = [{"Foo": 1}, {"Foo": 2}]
        to_list = [{"Foo": 1}]
        result = pl.smart_array_diff(from_list, to_list, {})
        self.assertEqual(result["strategy"], "positional")
        self.assertIsNone(result["key_fields"])
        self.assertEqual(result["count_from"], 2)
        self.assertEqual(result["count_to"], 1)
        self.assertEqual(result["added"], [])
        self.assertEqual(result["removed"], [])


# ---------------------------------------------------------------------------
# Redundant-count marking
# ---------------------------------------------------------------------------


class TestRedundantCounts(unittest.TestCase):
    def test_lvli_count_marked_redundant(self):
        diff_data = load_fixture("diff_small.json")
        rec = find_changed(diff_data, "0x01003001")
        entries = pl.extract_changes(rec["field_changes"], diff_data["ref_names"])
        pl.mark_redundant_counts(entries)
        count_entry = find_entry(entries, "Leveled List Entry Count")
        self.assertEqual(count_entry["suppressed"], "redundant_count")

    def test_non_matching_count_not_suppressed(self):
        entries = pl.extract_changes(
            {
                "Some Array": {"from": [1, 2], "to": [1, 2, 3]},
                "Unrelated Count": {"from": 7, "to": 9},
            },
            {},
        )
        pl.mark_redundant_counts(entries)
        e = find_entry(entries, "Unrelated Count")
        self.assertIsNone(e["suppressed"])

    def test_is_redundant_count_field_helper(self):
        self.assertTrue(pl._is_redundant_count_field("Foo / Bar Count", 3, 4, {(3, 4)}))
        self.assertFalse(pl._is_redundant_count_field("Foo / Bar Count", 3, 5, {(3, 4)}))
        self.assertFalse(pl._is_redundant_count_field("Foo / Bar", 3, 4, {(3, 4)}))
        self.assertFalse(pl._is_redundant_count_field("Foo / Bar Count", True, False, {(True, False)}))


# ---------------------------------------------------------------------------
# Common-change grouping
# ---------------------------------------------------------------------------


class TestCommonChanges(unittest.TestCase):
    def _build_records(self, diff_data):
        records = {}
        for c in diff_data["changed"]:
            fid = c["stub"]["form_id"]
            entries = pl.extract_changes(c["field_changes"], diff_data["ref_names"])
            pl.mark_redundant_counts(entries)
            records[fid] = {
                "status": "changed",
                "record_type": c["stub"]["record_type"],
                "changes": entries,
            }
        return records

    def test_six_npc_records_collapse_into_one_common_change(self):
        diff_data = load_fixture("diff_small.json")
        records = self._build_records(diff_data)
        common = pl.compute_common_changes(records, threshold=5)
        self.assertEqual(len(common), 1)
        cc = common[0]
        self.assertEqual(cc["record_type"], "NPC_")
        self.assertEqual(cc["path"], "Stats / Confidence")
        self.assertEqual(cc["from"], 2)
        self.assertEqual(cc["to"], 3)
        self.assertEqual(len(cc["member_form_ids"]), 6)
        self.assertEqual(cc["id"], "CC001")

    def test_aggression_deltas_do_not_collapse(self):
        diff_data = load_fixture("diff_small.json")
        records = self._build_records(diff_data)
        pl.compute_common_changes(records, threshold=5)
        npc1 = records["0x01006001"]
        agg_entry = find_entry(npc1["changes"], "Stats / Aggression")
        self.assertIsNone(agg_entry["common_group"])

    def test_common_group_tag_set_on_members(self):
        diff_data = load_fixture("diff_small.json")
        records = self._build_records(diff_data)
        pl.compute_common_changes(records, threshold=5)
        for i in range(1, 7):
            fid = f"0x0100600{i}"
            conf_entry = find_entry(records[fid]["changes"], "Stats / Confidence")
            self.assertEqual(conf_entry["common_group"], "CC001")

    def test_below_threshold_no_grouping(self):
        diff_data = load_fixture("diff_small.json")
        records = self._build_records(diff_data)
        common = pl.compute_common_changes(records, threshold=7)
        self.assertEqual(common, [])

    def test_ignores_non_changed_status(self):
        records = {
            "0x01": {"status": "added", "record_type": "WEAP", "changes": []},
        }
        self.assertEqual(pl.compute_common_changes(records, threshold=1), [])


# ---------------------------------------------------------------------------
# annotate_ref
# ---------------------------------------------------------------------------


class TestAnnotateRef(unittest.TestCase):
    def test_with_name_and_edid(self):
        ref_names = {"0x00000001": {"record_type": "AMMO", "editor_id": "Ammo1", "name": "Round"}}
        out = pl.annotate_ref("0x00000001", ref_names)
        self.assertIn("0x00000001", out)
        self.assertIn("AMMO", out)
        self.assertIn("Ammo1", out)
        self.assertIn("Round", out)

    def test_with_edid_only(self):
        ref_names = {"0x00000002": {"record_type": "KYWD", "editor_id": "SomeKeyword"}}
        out = pl.annotate_ref("0x00000002", ref_names)
        self.assertIn("SomeKeyword", out)
        self.assertNotIn('"', out)  # no quoted name/description

    def test_with_description_only_fallback(self):
        ref_names = {"0x00000003": {"record_type": "MISC", "description": "A rare thing."}}
        out = pl.annotate_ref("0x00000003", ref_names)
        self.assertIn("A rare thing.", out)

    def test_dangling_no_ref_names_entry(self):
        out = pl.annotate_ref("0x000000FF", {})
        self.assertEqual(out, "`0x000000FF`")

    def test_stub_dict_form(self):
        stub = {"formid": "0x00000004", "record_type": "WEAP", "editor_id": "Foo", "name": "Foo Gun"}
        out = pl.annotate_ref(stub, {})
        self.assertIn("Foo Gun", out)
        self.assertIn("WEAP", out)

    def test_fixture_dangling_formid_in_context(self):
        diff_data = load_fixture("diff_small.json")
        out = pl.annotate_ref("0x00099999", diff_data["ref_names"])
        self.assertEqual(out, "`0x00099999`")


# ---------------------------------------------------------------------------
# collect_refs_out
# ---------------------------------------------------------------------------


class TestCollectRefsOut(unittest.TestCase):
    def test_harvests_from_added_record_fields(self):
        diff_data = load_fixture("diff_small.json")
        added_weap = next(r for r in diff_data["added"] if r["form_id"] == "0x0100A001")
        refs = pl.collect_refs_out(added_weap["fields"])
        formids = {r["formid"] for r in refs}
        self.assertIn("0x00050001", formids)
        self.assertIn("0x00050002", formids)
        self.assertIn("0x00050099", formids)  # dangling, still harvested
        keyword_refs = [r for r in refs if r["formid"] == "0x00050001"]
        self.assertEqual(keyword_refs[0]["path"], "Keywords")

    def test_harvests_from_field_changes_tree(self):
        diff_data = load_fixture("diff_small.json")
        rec = find_changed(diff_data, "0x01001001")
        refs = pl.collect_refs_out(rec["field_changes"])
        formids = {r["formid"] for r in refs}
        self.assertIn("0x00123456", formids)
        self.assertIn("0x00654321", formids)
        self.assertIn("0x00099999", formids)

    def test_dedupes_by_formid_and_path(self):
        tree = {"A": "0x00000001", "B": {"C": "0x00000001"}}
        refs = pl.collect_refs_out(tree)
        self.assertEqual(len(refs), 2)  # same formid, different paths -> kept
        tree2 = {"A": {"nested": "0x00000001"}}
        refs2 = pl.collect_refs_out(tree2)
        refs2b = pl.collect_refs_out(tree2)
        self.assertEqual(refs2, refs2b)

    def test_harvests_from_change_entry_list(self):
        diff_data = load_fixture("diff_small.json")
        rec = find_changed(diff_data, "0x01001001")
        entries = pl.extract_changes(rec["field_changes"], diff_data["ref_names"])
        refs = pl.collect_refs_out(entries)
        formids = {r["formid"] for r in refs}
        self.assertIn("0x00123456", formids)
        self.assertIn("0x00099999", formids)

    def test_no_refs_in_plain_scalar_tree(self):
        self.assertEqual(pl.collect_refs_out({"Foo": 1, "Bar": "hello"}), [])


# ---------------------------------------------------------------------------
# Manifest round-trip
# ---------------------------------------------------------------------------


class TestManifest(unittest.TestCase):
    def test_new_manifest_shape(self):
        m = pl.new_manifest(
            patch_date="2026-07-03",
            old_token="20260626",
            new_token="20260703",
            new_esm_size=123456,
            new_esm_mtime=1234567890.0,
            pipeline_version="1.0.0",
        )
        self.assertEqual(m["schema_version"], 1)
        self.assertEqual(m["patch_date"], "2026-07-03")
        self.assertEqual(m["inputs"]["old_token"], "20260626")
        self.assertEqual(m["stages"]["mechanical"]["completed_at"], None)
        self.assertEqual(m["stages"]["narrative"]["max_chunk_chars"], 2000)

    def test_write_then_load_roundtrip(self):
        m = pl.new_manifest("2026-07-03", "a", "b", 1, 2.0, "1.0.0", counts={"added": 3})
        with tempfile.TemporaryDirectory() as tmp:
            pl.write_manifest(tmp, m)
            loaded = pl.load_manifest(tmp)
            self.assertEqual(loaded, m)

    def test_load_manifest_missing_returns_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertIsNone(pl.load_manifest(tmp))

    def test_write_manifest_creates_out_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            nested = Path(tmp) / "nested" / "dir"
            m = pl.new_manifest("2026-07-03", "a", "b", 1, 2.0, "1.0.0")
            pl.write_manifest(nested, m)
            self.assertTrue((nested / "manifest.json").exists())


if __name__ == "__main__":
    unittest.main()
