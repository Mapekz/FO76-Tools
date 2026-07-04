#!/usr/bin/env python3
"""Tests for tools/run_lints.py.

Uses hand-built minimal comprehensive.json/bundles.json dicts (rather than
loading the full diff_small.json fixture for every case) plus
`esm_daemon.FakeClient` backed by the shared `refs_graph.json` fixture for
the rules that need a reverse-reference walk (orphaned_unique,
unreferenced_perk_rank). No real daemon or ESM is touched.
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import patchnotes_lib as pnl  # noqa: E402
import run_lints as rl  # noqa: E402
from esm_daemon import FakeClient  # noqa: E402

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
REFS_GRAPH_FIXTURE = FIXTURES_DIR / "refs_graph.json"


# ---------------------------------------------------------------------------
# Small builders for hand-rolled comprehensive.json / bundles.json data
# ---------------------------------------------------------------------------


def make_record(
    form_id,
    record_type,
    status,
    *,
    editor_id=None,
    name=None,
    description=None,
    cut=None,
    fields=None,
    refs_out=None,
    changes=None,
):
    return {
        "form_id": form_id,
        "record_type": record_type,
        "editor_id": editor_id,
        "name": name,
        "description": description,
        "status": status,
        "cut": cut,
        "fields": fields if fields is not None else {},
        "refs_out": refs_out if refs_out is not None else [],
        "changes": changes if changes is not None else [],
    }


def make_comp(records, ref_names=None):
    return {"records": {r["form_id"]: r for r in records}, "ref_names": ref_names or {}}


def make_bundle(bundle_id, category, anchor_fid, anchor_type, members=None, edges=None, **extra):
    b = {
        "id": bundle_id,
        "category": category,
        "category_label": category,
        "title": f"Bundle {bundle_id}",
        "anchor": {"form_id": anchor_fid, "record_type": anchor_type},
        "members": members
        if members is not None
        else [{"form_id": anchor_fid, "record_type": anchor_type, "role": "primary"}],
        "edges": edges or [],
        "bug_watch": False,
        "lint_ids": [],
    }
    b.update(extra)
    return b


def make_bundles(bundles=None):
    return {"schema_version": 1, "bundles": bundles or [], "lints": []}


def no_op_client():
    """A FakeClient over an empty fixture -- fine for rules that never touch
    the client (lvli_blocked_entry, desc_changed_stats_same,
    stats_changed_desc_same, cut_newly_deprecated) and for dangling_ref tests
    that build their own records/exists universe."""
    return FakeClient({"records": {}, "refs": {}})


def refs_graph_client():
    return FakeClient(REFS_GRAPH_FIXTURE)


class TestRunLintsBase(unittest.TestCase):
    def ctx_for(self, comp, bundles=None, client=None, new_esm="new.esm", old_esm="old.esm", settings=None):
        return rl.build_context(comp, bundles or make_bundles(), client or no_op_client(), new_esm, old_esm, settings)


# ---------------------------------------------------------------------------
# Rule 1: lvli_blocked_entry
# ---------------------------------------------------------------------------


class TestLvliBlockedEntry(TestRunLintsBase):
    def test_added_lvli_quantity_zero_is_blocked(self):
        rec = make_record(
            "0x01000001",
            "LVLI",
            "added",
            editor_id="LVLI_Test",
            fields={
                "Entries": [
                    {"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 0}},
                    {"Leveled List Entry": {"Reference": "0x00AA0002", "Minimum Level": 1, "Quantity": 1}},
                ]
            },
        )
        ref_names = {"0x00AA0001": {"record_type": "MISC", "editor_id": "Junk1", "name": "Scrap"}}
        ctx = self.ctx_for(make_comp([rec], ref_names))

        lints = rl.RULES["lvli_blocked_entry"](ctx)

        self.assertEqual(len(lints), 1)
        lint = lints[0]
        self.assertEqual(lint["rule"], "lvli_blocked_entry")
        self.assertEqual(lint["severity"], "error")
        self.assertEqual(lint["form_id"], "0x01000001")
        self.assertIn("Scrap", lint["message"])
        self.assertEqual(lint["data"]["reason"], "quantity_zero")
        self.assertNotIn("all_blocked", lint["data"])

    def test_added_lvli_no_blocked_entries_emits_nothing(self):
        rec = make_record(
            "0x01000002",
            "LVLI",
            "added",
            editor_id="LVLI_Clean",
            fields={
                "Entries": [
                    {"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 1}},
                ]
            },
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["lvli_blocked_entry"](ctx), [])

    def test_chance_none_100_accepts_int_float_and_enum_dict(self):
        # A second, always-unblocked entry keeps this a "some but not all
        # entries blocked" case, so only the per-entry lint fires (not also
        # the all-blocked variant) -- isolates the ChanceNone-100 detection.
        for chance_val in (100, 100.0, {"value": 100, "name": "Always"}):
            with self.subTest(chance_val=chance_val):
                rec = make_record(
                    "0x01000003",
                    "LVLI",
                    "added",
                    editor_id="LVLI_ChanceTest",
                    fields={
                        "Entries": [
                            {
                                "Leveled List Entry": {
                                    "Reference": "0x00AA0002",
                                    "Minimum Level": 1,
                                    "Quantity": 1,
                                    "ChanceNone": chance_val,
                                }
                            },
                            {"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 1}},
                        ]
                    },
                )
                ctx = self.ctx_for(make_comp([rec]))
                lints = rl.RULES["lvli_blocked_entry"](ctx)
                self.assertEqual(len(lints), 1)
                self.assertEqual(lints[0]["data"]["reason"], "chance_none_100")

    def test_chance_none_below_100_not_blocked(self):
        rec = make_record(
            "0x01000004",
            "LVLI",
            "added",
            editor_id="LVLI_ChanceOk",
            fields={
                "Entries": [
                    {
                        "Leveled List Entry": {
                            "Reference": "0x00AA0002",
                            "Minimum Level": 1,
                            "Quantity": 1,
                            "ChanceNone": {"value": 50, "name": "Half"},
                        }
                    }
                ]
            },
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["lvli_blocked_entry"](ctx), [])

    def test_added_lvli_all_entries_blocked_emits_extra_variant(self):
        rec = make_record(
            "0x01000005",
            "LVLI",
            "added",
            editor_id="LVLI_AllBlocked",
            fields={
                "Entries": [
                    {"Leveled List Entry": {"Reference": "0x00AA0005", "Minimum Level": 1, "Quantity": 0}},
                    {
                        "Leveled List Entry": {
                            "Reference": "0x00AA0006",
                            "Minimum Level": 1,
                            "Quantity": 1,
                            "ChanceNone": {"value": 100, "name": "Always"},
                        }
                    },
                ]
            },
        )
        ctx = self.ctx_for(make_comp([rec]))
        lints = rl.RULES["lvli_blocked_entry"](ctx)

        # 2 per-entry lints + 1 all-blocked summary lint.
        self.assertEqual(len(lints), 3)
        all_blocked = [l for l in lints if l["data"].get("all_blocked")]
        self.assertEqual(len(all_blocked), 1)
        self.assertEqual(all_blocked[0]["rule"], "lvli_blocked_entry")
        self.assertEqual(all_blocked[0]["severity"], "error")
        self.assertEqual(all_blocked[0]["data"]["entry_count"], 2)
        self.assertEqual(all_blocked[0]["form_id"], "0x01000005")

    def test_added_lvli_not_all_entries_blocked_no_extra_variant(self):
        rec = make_record(
            "0x01000006",
            "LVLI",
            "added",
            editor_id="LVLI_PartlyBlocked",
            fields={
                "Entries": [
                    {"Leveled List Entry": {"Reference": "0x00AA0005", "Minimum Level": 1, "Quantity": 0}},
                    {"Leveled List Entry": {"Reference": "0x00AA0006", "Minimum Level": 1, "Quantity": 1}},
                ]
            },
        )
        ctx = self.ctx_for(make_comp([rec]))
        lints = rl.RULES["lvli_blocked_entry"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertFalse(lints[0]["data"].get("all_blocked"))

    def _changed_lvli_record(self, form_id, from_list, to_list, ref_names=None):
        field_changes = {"Entries": {"from": from_list, "to": to_list}}
        changes = pnl.extract_changes(field_changes, ref_names or {})
        return make_record(form_id, "LVLI", "changed", editor_id="LVLI_Changed", changes=changes)

    def test_changed_lvli_new_added_entry_blocked(self):
        from_list = [{"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 1}}]
        to_list = [
            {"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 1}},
            {"Leveled List Entry": {"Reference": "0x00AA0002", "Minimum Level": 5, "Quantity": 0}},
        ]
        rec = self._changed_lvli_record("0x01000007", from_list, to_list)
        ctx = self.ctx_for(make_comp([rec]))
        lints = rl.RULES["lvli_blocked_entry"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["data"]["status"], "changed")
        self.assertEqual(lints[0]["data"]["reason"], "quantity_zero")
        self.assertEqual(lints[0]["data"]["item"], "0x00AA0002")

    def test_changed_lvli_existing_entry_newly_blocked(self):
        from_list = [{"Leveled List Entry": {"Reference": "0x00AA0003", "Minimum Level": 1, "Quantity": 2}}]
        to_list = [{"Leveled List Entry": {"Reference": "0x00AA0003", "Minimum Level": 1, "Quantity": 0}}]
        ref_names = {"0x00AA0003": {"record_type": "MISC", "editor_id": "JunkItem03", "name": "Scrap Plastic"}}
        rec = self._changed_lvli_record("0x01000008", from_list, to_list, ref_names)
        ctx = self.ctx_for(make_comp([rec], ref_names))
        lints = rl.RULES["lvli_blocked_entry"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["data"]["reason"], "quantity_zero")
        self.assertEqual(lints[0]["data"]["item"], "0x00AA0003")
        self.assertIn("Scrap Plastic", lints[0]["message"])

    def test_changed_lvli_no_regression_emits_nothing(self):
        # Quantity 1 -> 3, still nonzero: not blocked (mirrors diff_small.json's 0x01003001).
        from_list = [{"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 1}}]
        to_list = [{"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 3}}]
        rec = self._changed_lvli_record("0x01000009", from_list, to_list)
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["lvli_blocked_entry"](ctx), [])

    def test_non_lvli_and_wrong_status_records_ignored(self):
        weap = make_record("0x0100000A", "WEAP", "added", editor_id="Rifle", fields={"Entries": [{"Quantity": 0}]})
        lvli_removed = make_record(
            "0x0100000B",
            "LVLI",
            "removed",
            editor_id="LVLI_Removed",
            fields={"Entries": [{"Leveled List Entry": {"Reference": "0x00AA0001", "Quantity": 0}}]},
        )
        ctx = self.ctx_for(make_comp([weap, lvli_removed]))
        self.assertEqual(rl.RULES["lvli_blocked_entry"](ctx), [])


# ---------------------------------------------------------------------------
# Rule 2: dangling_ref
# ---------------------------------------------------------------------------


class TestDanglingRef(TestRunLintsBase):
    def test_dangling_formid_in_refs_out_is_flagged(self):
        rec = make_record(
            "0x02000001",
            "WEAP",
            "changed",
            editor_id="WEAP_Test",
            refs_out=[{"formid": "0x0BADF00D", "path": "Data / Ammo"}],
        )
        client = FakeClient({"records": {}, "refs": {}})  # 0x0BADF00D resolves nowhere
        ctx = self.ctx_for(make_comp([rec]), client=client)
        lints = rl.RULES["dangling_ref"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["form_id"], "0x02000001")
        self.assertEqual(lints[0]["data"]["dangling_formid"], "0x0BADF00D")

    def test_ref_in_ref_names_not_flagged(self):
        rec = make_record(
            "0x02000002",
            "WEAP",
            "changed",
            editor_id="WEAP_Test2",
            refs_out=[{"formid": "0x00AA0001", "path": "Data / Ammo"}],
        )
        ref_names = {"0x00AA0001": {"record_type": "AMMO", "editor_id": "Ammo1"}}
        client = FakeClient({"records": {}, "refs": {}})
        ctx = self.ctx_for(make_comp([rec], ref_names), client=client)
        self.assertEqual(rl.RULES["dangling_ref"](ctx), [])

    def test_ref_resolving_in_new_or_old_esm_not_flagged(self):
        rec = make_record(
            "0x02000003",
            "WEAP",
            "changed",
            editor_id="WEAP_Test3",
            refs_out=[{"formid": "0x00AA0009", "path": "Data / Ammo"}],
        )
        client = FakeClient({"records": {"0x00AA0009": {"record_type": "AMMO"}}, "refs": {}})
        ctx = self.ctx_for(make_comp([rec]), client=client)
        self.assertEqual(rl.RULES["dangling_ref"](ctx), [])

    def test_null_formid_skipped(self):
        rec = make_record(
            "0x02000004", "WEAP", "changed", editor_id="WEAP_Test4", refs_out=[{"formid": "0x00000000", "path": "x"}]
        )
        client = FakeClient({"records": {}, "refs": {}})
        ctx = self.ctx_for(make_comp([rec]), client=client)
        self.assertEqual(rl.RULES["dangling_ref"](ctx), [])

    def test_to_side_change_entry_harvested_not_from_side(self):
        rec = make_record(
            "0x02000005",
            "WEAP",
            "changed",
            editor_id="WEAP_Test5",
            changes=[
                {
                    "path": "Data / Ammo",
                    "kind": "formid",
                    "from": "0x00111111",  # stale from-side ref: must NOT be flagged
                    "to": "0x00222222",  # dangling to-side ref: must be flagged
                    "suppressed": None,
                }
            ],
        )
        client = FakeClient({"records": {}, "refs": {}})
        ctx = self.ctx_for(make_comp([rec]), client=client)
        lints = rl.RULES["dangling_ref"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["data"]["dangling_formid"], "0x00222222")

    def test_missing_esm_paths_never_flag(self):
        rec = make_record(
            "0x02000006",
            "WEAP",
            "changed",
            editor_id="WEAP_Test6",
            refs_out=[{"formid": "0x0BADF00D", "path": "x"}],
        )
        client = FakeClient({"records": {}, "refs": {}})
        ctx = self.ctx_for(make_comp([rec]), client=client, new_esm=None, old_esm=None)
        self.assertEqual(rl.RULES["dangling_ref"](ctx), [])

    def test_cap_at_50_lints_and_notes_it(self):
        records = []
        for i in range(60):
            fid = f"0x0300{i:04X}"
            dangling = f"0x0400{i:04X}"
            records.append(
                make_record(
                    fid, "WEAP", "changed", editor_id=f"WEAP_{i}", refs_out=[{"formid": dangling, "path": "x"}]
                )
            )
        client = FakeClient({"records": {}, "refs": {}})
        ctx = self.ctx_for(make_comp(records), client=client)
        lints = rl.RULES["dangling_ref"](ctx)
        self.assertEqual(len(lints), 50)
        self.assertTrue(any("cap" in note for note in ctx["_notes"]))


# ---------------------------------------------------------------------------
# Rule 3: orphaned_unique
# ---------------------------------------------------------------------------


class TestOrphanedUnique(TestRunLintsBase):
    KEYWORD_PATTERNS = ["if_tmp_*", "*Keyword"]

    def test_default_pattern_matches_orphaned_keyword(self):
        rec = make_record("0x00100080", "KYWD", "added", editor_id="OrphanKeyword")
        ctx = self.ctx_for(
            make_comp([rec]),
            client=refs_graph_client(),
            settings={"unique_keyword_patterns": self.KEYWORD_PATTERNS},
        )
        lints = rl.RULES["orphaned_unique"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["severity"], "warn")
        self.assertEqual(lints[0]["form_id"], "0x00100080")

    def test_hub_keyword_with_no_live_referencer_orphaned(self):
        rec = make_record("0x00100090", "KYWD", "added", editor_id="HubKeyword")
        ctx = self.ctx_for(
            make_comp([rec]),
            client=refs_graph_client(),
            settings={"unique_keyword_patterns": self.KEYWORD_PATTERNS},
        )
        lints = rl.RULES["orphaned_unique"](ctx)
        self.assertEqual(len(lints), 1)

    def test_keyword_connected_via_lvli_not_orphaned(self):
        # if_tmp_WeaponMod (0x00100050) -> WEAP_TestRifle (depth1) -> LVLI_TestList
        # (depth2): reaches a live drop type within depth 4, so it's NOT orphaned.
        rec = make_record("0x00100050", "KYWD", "added", editor_id="if_tmp_WeaponMod")
        ctx = self.ctx_for(
            make_comp([rec]), client=refs_graph_client(), settings={"unique_keyword_patterns": self.KEYWORD_PATTERNS}
        )
        self.assertEqual(rl.RULES["orphaned_unique"](ctx), [])

    def test_keyword_not_matching_pattern_ignored(self):
        rec = make_record("0x00100080", "KYWD", "added", editor_id="OrphanKeyword")
        ctx = self.ctx_for(make_comp([rec]), client=refs_graph_client())  # default patterns: if_tmp_* only
        self.assertEqual(rl.RULES["orphaned_unique"](ctx), [])

    def test_weap_bundle_with_orphaned_keyword_member_flagged(self):
        weap = make_record("0x00100091", "WEAP", "added", editor_id="WEAP_HubRef01")
        bundle = make_bundle(
            "B0001",
            "unique_weapons_gear",
            "0x00100091",
            "WEAP",
            members=[
                {"form_id": "0x00100091", "record_type": "WEAP", "editor_id": "WEAP_HubRef01", "role": "primary"},
                {"form_id": "0x00100090", "record_type": "KYWD", "editor_id": "HubKeyword", "role": "satellite"},
            ],
        )
        ctx = self.ctx_for(
            make_comp([weap]),
            bundles=make_bundles([bundle]),
            client=refs_graph_client(),
            settings={"unique_keyword_patterns": self.KEYWORD_PATTERNS},
        )
        lints = rl.RULES["orphaned_unique"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["form_id"], "0x00100091")
        self.assertEqual(lints[0]["data"]["keyword_formid"], "0x00100090")

    def test_weap_bundle_with_connected_keyword_member_not_flagged(self):
        weap = make_record("0x00100001", "WEAP", "changed", editor_id="WEAP_TestRifle")
        bundle = make_bundle(
            "B0002",
            "unique_weapons_gear",
            "0x00100001",
            "WEAP",
            members=[
                {"form_id": "0x00100001", "record_type": "WEAP", "editor_id": "WEAP_TestRifle", "role": "primary"},
                {"form_id": "0x00100050", "record_type": "KYWD", "editor_id": "if_tmp_WeaponMod", "role": "satellite"},
            ],
        )
        ctx = self.ctx_for(
            make_comp([weap]),
            bundles=make_bundles([bundle]),
            client=refs_graph_client(),
            settings={"unique_keyword_patterns": self.KEYWORD_PATTERNS},
        )
        self.assertEqual(rl.RULES["orphaned_unique"](ctx), [])

    def test_weap_with_no_bundle_skipped(self):
        weap = make_record("0x00100091", "WEAP", "added", editor_id="WEAP_HubRef01")
        ctx = self.ctx_for(
            make_comp([weap]), client=refs_graph_client(), settings={"unique_keyword_patterns": self.KEYWORD_PATTERNS}
        )
        self.assertEqual(rl.RULES["orphaned_unique"](ctx), [])


# ---------------------------------------------------------------------------
# Rule 4: unreferenced_perk_rank
# ---------------------------------------------------------------------------


class TestUnreferencedPerkRank(TestRunLintsBase):
    def test_perk_with_pcrd_not_flagged(self):
        rec = make_record("0x00100070", "PERK", "changed", editor_id="TestPerk01")
        ctx = self.ctx_for(make_comp([rec]), client=refs_graph_client())
        self.assertEqual(rl.RULES["unreferenced_perk_rank"](ctx), [])

    def test_perk_without_pcrd_flagged(self):
        rec = make_record("0x00100072", "PERK", "added", editor_id="TestPerk02_orphan")
        ctx = self.ctx_for(make_comp([rec]), client=refs_graph_client())
        lints = rl.RULES["unreferenced_perk_rank"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["severity"], "warn")
        self.assertEqual(lints[0]["form_id"], "0x00100072")

    def test_cut_marked_perk_skipped_even_without_pcrd(self):
        rec = make_record(
            "0x00100072",
            "PERK",
            "added",
            editor_id="zzz_TestPerk02_orphan",
            cut={"marker": "ZZZ", "confidence": "high", "kind": "added_cut"},
        )
        ctx = self.ctx_for(make_comp([rec]), client=refs_graph_client())
        self.assertEqual(rl.RULES["unreferenced_perk_rank"](ctx), [])

    def test_removed_perk_not_considered(self):
        rec = make_record("0x00100072", "PERK", "removed", editor_id="TestPerk02_orphan")
        ctx = self.ctx_for(make_comp([rec]), client=refs_graph_client())
        self.assertEqual(rl.RULES["unreferenced_perk_rank"](ctx), [])


# ---------------------------------------------------------------------------
# Rule 5: desc_changed_stats_same
# ---------------------------------------------------------------------------


class TestDescChangedStatsSame(TestRunLintsBase):
    def test_description_only_change_flagged(self):
        rec = make_record(
            "0x04000001",
            "WEAP",
            "changed",
            editor_id="WEAP_DescOnly",
            changes=[
                {
                    "path": "Description",
                    "kind": "string",
                    "from": "The old flavor text for this item.",
                    "to": "The new flavor text for this item, slightly reworded.",
                    "suppressed": None,
                }
            ],
        )
        ctx = self.ctx_for(make_comp([rec]))
        lints = rl.RULES["desc_changed_stats_same"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["severity"], "info")

    def test_description_change_with_numeric_change_not_flagged(self):
        rec = make_record(
            "0x04000002",
            "WEAP",
            "changed",
            editor_id="WEAP_DescAndStat",
            changes=[
                {
                    "path": "Description",
                    "kind": "string",
                    "from": "The old flavor text for this item.",
                    "to": "The new flavor text for this item, slightly reworded.",
                    "suppressed": None,
                },
                {"path": "Data / Damage", "kind": "scalar", "from": 10, "to": 14, "suppressed": None},
            ],
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["desc_changed_stats_same"](ctx), [])

    def test_object_bounds_noise_does_not_disqualify(self):
        rec = make_record(
            "0x04000003",
            "WEAP",
            "changed",
            editor_id="WEAP_DescPlusBounds",
            changes=[
                {
                    "path": "Description",
                    "kind": "string",
                    "from": "The old flavor text for this item.",
                    "to": "The new flavor text for this item, slightly reworded.",
                    "suppressed": None,
                },
                {"path": "Object Bounds / X1", "kind": "scalar", "from": -10, "to": -12, "suppressed": "noise"},
            ],
        )
        ctx = self.ctx_for(make_comp([rec]))
        lints = rl.RULES["desc_changed_stats_same"](ctx)
        self.assertEqual(len(lints), 1)

    def test_no_description_change_not_flagged(self):
        rec = make_record(
            "0x04000004",
            "WEAP",
            "changed",
            editor_id="WEAP_NoDesc",
            changes=[{"path": "Data / Damage", "kind": "scalar", "from": 10, "to": 14, "suppressed": None}],
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["desc_changed_stats_same"](ctx), [])


# ---------------------------------------------------------------------------
# Rule 6: stats_changed_desc_same
# ---------------------------------------------------------------------------


class TestStatsChangedDescSame(TestRunLintsBase):
    def test_stale_description_number_matched(self):
        rec = make_record(
            "0x05000001",
            "WEAP",
            "changed",
            editor_id="WEAP_Stale",
            description="Deals 25 damage in melee range.",
            changes=[{"path": "Data / Damage", "kind": "scalar", "from": 25, "to": 30, "suppressed": None}],
        )
        ctx = self.ctx_for(make_comp([rec]))
        lints = rl.RULES["stats_changed_desc_same"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["severity"], "info")
        self.assertEqual(lints[0]["data"]["matched_number"], 25)
        self.assertEqual(lints[0]["data"]["path"], "Data / Damage")

    def test_no_match_when_description_number_differs(self):
        rec = make_record(
            "0x05000002",
            "WEAP",
            "changed",
            editor_id="WEAP_NoMatch",
            description="Deals 10 damage in melee range.",
            changes=[{"path": "Data / Damage", "kind": "scalar", "from": 25, "to": 30, "suppressed": None}],
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["stats_changed_desc_same"](ctx), [])

    def test_skipped_when_description_itself_changed(self):
        rec = make_record(
            "0x05000003",
            "WEAP",
            "changed",
            editor_id="WEAP_DescChanged",
            description="Deals 30 damage in melee range.",
            changes=[
                {"path": "Data / Damage", "kind": "scalar", "from": 25, "to": 30, "suppressed": None},
                {
                    "path": "Description",
                    "kind": "string",
                    "from": "Deals 25 damage in melee range.",
                    "to": "Deals 30 damage in melee range.",
                    "suppressed": None,
                },
            ],
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["stats_changed_desc_same"](ctx), [])

    def test_non_stats_record_type_ignored(self):
        rec = make_record(
            "0x05000004",
            "MISC",
            "changed",
            editor_id="MISC_Stale",
            description="Worth 25 caps.",
            changes=[{"path": "Data / Value", "kind": "scalar", "from": 25, "to": 30, "suppressed": None}],
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["stats_changed_desc_same"](ctx), [])


# ---------------------------------------------------------------------------
# Rule 7: cut_newly_deprecated
# ---------------------------------------------------------------------------


class TestCutNewlyDeprecated(TestRunLintsBase):
    def test_newly_deprecated_passthrough(self):
        rec = make_record(
            "0x06000001",
            "PERK",
            "changed",
            editor_id="zzz_TestPerkRank03",
            cut={"marker": "ZZZ", "confidence": "high", "kind": "newly_deprecated"},
        )
        ctx = self.ctx_for(make_comp([rec]))
        lints = rl.RULES["cut_newly_deprecated"](ctx)
        self.assertEqual(len(lints), 1)
        self.assertEqual(lints[0]["severity"], "info")
        self.assertEqual(lints[0]["data"]["kind"], "newly_deprecated")

    def test_still_cut_not_passthrough(self):
        rec = make_record(
            "0x06000002",
            "PERK",
            "changed",
            editor_id="zzz_TestPerkRank04",
            cut={"marker": "ZZZ", "confidence": "high", "kind": "still_cut"},
        )
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["cut_newly_deprecated"](ctx), [])

    def test_no_cut_not_flagged(self):
        rec = make_record("0x06000003", "PERK", "changed", editor_id="TestPerkRankNormal", cut=None)
        ctx = self.ctx_for(make_comp([rec]))
        self.assertEqual(rl.RULES["cut_newly_deprecated"](ctx), [])


# ---------------------------------------------------------------------------
# Injection: bundles.json lint_ids / bug_watch
# ---------------------------------------------------------------------------


class TestInjection(unittest.TestCase):
    def test_lint_ids_and_bug_watch_set_only_on_matching_bundles(self):
        lvli_rec = make_record(
            "0x01000001",
            "LVLI",
            "added",
            editor_id="LVLI_Test",
            fields={"Entries": [{"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 0}}]},
        )
        comp = make_comp([lvli_rec])

        matching_bundle = make_bundle("B0001", "loot", "0x01000001", "LVLI")
        untouched_bundle = make_bundle(
            "B0002",
            "weapons",
            "0x09999999",
            "WEAP",
            lint_ids=["L9999"],
            bug_watch=True,
        )
        bundles = make_bundles([matching_bundle, untouched_bundle])

        lints_payload, updated = rl.run_lints(comp, bundles, no_op_client(), "new.esm", "old.esm", {})

        by_id = {b["id"]: b for b in updated["bundles"]}
        self.assertGreaterEqual(len(by_id["B0001"]["lint_ids"]), 1)
        self.assertTrue(by_id["B0001"]["bug_watch"])

        # B0002 has no matching lint this run: recomputed to empty/false, and
        # every OTHER field (category, anchor, ...) is left exactly as-is.
        self.assertEqual(by_id["B0002"]["lint_ids"], [])
        self.assertFalse(by_id["B0002"]["bug_watch"])
        self.assertEqual(by_id["B0002"]["category"], "weapons")
        self.assertEqual(by_id["B0002"]["anchor"], {"form_id": "0x09999999", "record_type": "WEAP"})

        self.assertEqual(updated["lints"], lints_payload["lints"])

    def test_lint_ids_populated_for_member_not_just_anchor(self):
        weap_rec = make_record("0x00100091", "WEAP", "added", editor_id="WEAP_HubRef01")
        comp = make_comp([weap_rec])
        bundle = make_bundle(
            "B0003",
            "unique_weapons_gear",
            "0x00100091",
            "WEAP",
            members=[
                {"form_id": "0x00100091", "record_type": "WEAP", "role": "primary"},
                {"form_id": "0x00100090", "record_type": "KYWD", "editor_id": "HubKeyword", "role": "satellite"},
            ],
        )
        bundles = make_bundles([bundle])
        client = refs_graph_client()
        settings = {"unique_keyword_patterns": ["*Keyword"]}

        lints_payload, updated = rl.run_lints(comp, bundles, client, "new.esm", "old.esm", settings)

        self.assertEqual(len(lints_payload["lints"]), 1)
        self.assertEqual(updated["bundles"][0]["lint_ids"], [lints_payload["lints"][0]["id"]])
        self.assertTrue(updated["bundles"][0]["bug_watch"])


# ---------------------------------------------------------------------------
# Determinism
# ---------------------------------------------------------------------------


class TestDeterminism(unittest.TestCase):
    def _mixed_comp(self):
        lvli_rec = make_record(
            "0x07000002",
            "LVLI",
            "added",
            editor_id="LVLI_Test",
            fields={"Entries": [{"Leveled List Entry": {"Reference": "0x00AA0001", "Minimum Level": 1, "Quantity": 0}}]},
        )
        dangling_rec = make_record(
            "0x07000001",
            "WEAP",
            "changed",
            editor_id="WEAP_Dangling",
            refs_out=[{"formid": "0x0BADF00D", "path": "Data / Ammo"}],
        )
        cut_rec = make_record(
            "0x07000003",
            "PERK",
            "changed",
            editor_id="zzz_Perk",
            cut={"marker": "ZZZ", "confidence": "high", "kind": "newly_deprecated"},
        )
        return make_comp([lvli_rec, dangling_rec, cut_rec])

    def test_ids_sorted_by_rule_then_form_id(self):
        comp = self._mixed_comp()
        bundles = make_bundles()
        client = no_op_client()
        lints_payload, _ = rl.run_lints(comp, bundles, client, "new.esm", "old.esm", {})

        lints = lints_payload["lints"]
        self.assertTrue(lints, "expected at least one lint from the mixed fixture")
        keys = [(l["rule"], l["form_id"]) for l in lints]
        self.assertEqual(keys, sorted(keys))
        # Ids assigned in that same sorted order, 1-indexed.
        for i, lint in enumerate(lints, start=1):
            self.assertEqual(lint["id"], f"L{i:04d}")

    def test_repeated_runs_produce_identical_output(self):
        comp = self._mixed_comp()
        bundles = make_bundles()
        run1, _ = rl.run_lints(comp, bundles, no_op_client(), "new.esm", "old.esm", {})
        run2, _ = rl.run_lints(comp, bundles, no_op_client(), "new.esm", "old.esm", {})
        ids1 = [(l["rule"], l["form_id"], l["id"]) for l in run1["lints"]]
        ids2 = [(l["rule"], l["form_id"], l["id"]) for l in run2["lints"]]
        self.assertEqual(ids1, ids2)


# ---------------------------------------------------------------------------
# End-to-end CLI
# ---------------------------------------------------------------------------


class TempOutDir:
    """Temp dir pre-populated with comprehensive.json + bundles.json,
    mirroring the pipeline's output-directory layout."""

    def __init__(self, comp, bundles):
        self.comp = comp
        self.bundles = bundles
        self._tmp = None

    def __enter__(self):
        self._tmp = tempfile.TemporaryDirectory()
        out_dir = Path(self._tmp.name)
        (out_dir / "comprehensive.json").write_text(json.dumps(self.comp), encoding="utf-8")
        (out_dir / "bundles.json").write_text(json.dumps(self.bundles), encoding="utf-8")
        return out_dir

    def __exit__(self, *exc):
        self._tmp.cleanup()


class TestEndToEndCli(unittest.TestCase):
    def test_full_run_writes_lints_and_updates_bundles(self):
        lvli_rec = make_record(
            "0x08000001",
            "LVLI",
            "added",
            editor_id="LVLI_AllBlocked",
            fields={
                "Entries": [
                    {"Leveled List Entry": {"Reference": "0x00AA0005", "Minimum Level": 1, "Quantity": 0}},
                    {
                        "Leveled List Entry": {
                            "Reference": "0x00AA0006",
                            "Minimum Level": 1,
                            "Quantity": 1,
                            "ChanceNone": {"value": 100, "name": "Always"},
                        }
                    },
                ]
            },
        )
        cut_rec = make_record(
            "0x08000002",
            "PERK",
            "changed",
            editor_id="zzz_Perk",
            cut={"marker": "ZZZ", "confidence": "high", "kind": "newly_deprecated"},
        )
        comp = make_comp([lvli_rec, cut_rec])
        bundle = make_bundle("B0001", "loot", "0x08000001", "LVLI")
        bundles = make_bundles([bundle])

        with TempOutDir(comp, bundles) as out_dir:
            rc = rl.main(
                [
                    str(out_dir),
                    "--offline",
                    "--refs-fixture",
                    str(REFS_GRAPH_FIXTURE),
                ]
            )
            self.assertEqual(rc, 0)

            lints_path = out_dir / "lints.json"
            bundles_path = out_dir / "bundles.json"
            self.assertTrue(lints_path.exists())
            self.assertTrue(bundles_path.exists())

            lints_payload = json.loads(lints_path.read_text(encoding="utf-8"))
            self.assertEqual(lints_payload["schema_version"], 1)
            self.assertIn("rules_run", lints_payload["meta"])
            self.assertEqual(set(lints_payload["meta"]["counts"].keys()), {"error", "warn", "info"})

            # 2 per-entry + 1 all_blocked (LVLI) + 1 cut_newly_deprecated == 4.
            self.assertEqual(len(lints_payload["lints"]), 4)

            updated_bundles = json.loads(bundles_path.read_text(encoding="utf-8"))
            b0001 = next(b for b in updated_bundles["bundles"] if b["id"] == "B0001")
            self.assertTrue(b0001["bug_watch"])
            self.assertGreaterEqual(len(b0001["lint_ids"]), 1)
            self.assertEqual(updated_bundles["lints"], lints_payload["lints"])

    def test_offline_requires_refs_fixture(self):
        comp = make_comp([])
        bundles = make_bundles()
        with TempOutDir(comp, bundles) as out_dir:
            rc = rl.main([str(out_dir), "--offline"])
            self.assertEqual(rc, 1)

    def test_rules_filter_runs_subset_only(self):
        cut_rec = make_record(
            "0x08000003",
            "PERK",
            "changed",
            editor_id="zzz_Perk2",
            cut={"marker": "ZZZ", "confidence": "high", "kind": "newly_deprecated"},
        )
        lvli_rec = make_record(
            "0x08000004",
            "LVLI",
            "added",
            editor_id="LVLI_AllBlocked2",
            fields={"Entries": [{"Leveled List Entry": {"Reference": "0x00AA0005", "Minimum Level": 1, "Quantity": 0}}]},
        )
        comp = make_comp([cut_rec, lvli_rec])
        bundles = make_bundles()

        with TempOutDir(comp, bundles) as out_dir:
            rc = rl.main(
                [
                    str(out_dir),
                    "--offline",
                    "--refs-fixture",
                    str(REFS_GRAPH_FIXTURE),
                    "--rules",
                    "cut_newly_deprecated",
                ]
            )
            self.assertEqual(rc, 0)
            lints_payload = json.loads((out_dir / "lints.json").read_text(encoding="utf-8"))
            self.assertEqual(lints_payload["meta"]["rules_run"], ["cut_newly_deprecated"])
            self.assertEqual(len(lints_payload["lints"]), 1)
            self.assertEqual(lints_payload["lints"][0]["rule"], "cut_newly_deprecated")


if __name__ == "__main__":
    unittest.main()
