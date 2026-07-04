#!/usr/bin/env python3
"""Tests for tools/build_bundles.py.

Covers:
  - Edge relation/label mapping (the ordered rule table), including the
    direction-aware LVLI "contains"/"dropped via" phrasing and the
    COBJ path-substring rules (crafts/crafted_from/crafted_at) that can
    only fire on forward-discovered edges.
  - Universe / degree computation helpers, in isolation.
  - Union-find hub exemption, in isolation (direct U<->U edges, a non-hub
    context-node bridge, and a hub context-node that must NOT bridge).
  - Anchor selection tie-break order (priority, status weight, has-name,
    edge degree, lowest form_id).
  - Oversized-component splitting (BFS-nearest-anchor assignment, tie
    break toward the higher-priority anchor, under-cap/single-anchor
    no-ops).
  - Bundle merging (same-anchor, overlap-ratio fixpoint).
  - Context-member attachment (cap + preference order).
  - Categorization against the real patch_notes_categories.json (first-
    rule-match-wins, the "keyword" scope's `client.record()` lookup +
    caching + failure-as-no-match, the uncategorized fallback).
  - The full offline pipeline (FakeClient + refs_graph.json + a
    hand-written comprehensive_mini.json aligned to that fixture's node
    ids): the WEAP/OMOD/LVLI/KYWD cluster forming one bundle with a WEAP
    anchor, NPC_/CONT/QUST context members with the right edge labels, the
    degree-12 hub keyword NOT merging its 12 unrelated referrers, orphan
    singletons, the PCRD/PERK pair, old-vs-new ESM selection for
    removed-status records, deterministic IDs, and the bundles.json shape
    contract.
  - CLI behavior (argument validation, settings precedence, a subprocess
    smoke test).

No real daemon or ESM is touched -- everything runs against
esm_daemon.FakeClient and the checked-in fixtures.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import build_bundles as bb  # noqa: E402
import esm_daemon  # noqa: E402

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"
SCRIPT_PATH = Path(__file__).resolve().parents[1] / "build_bundles.py"
REFS_FIXTURE_PATH = FIXTURES_DIR / "refs_graph.json"
COMPREHENSIVE_MINI_PATH = FIXTURES_DIR / "comprehensive_mini.json"
CATEGORIES_PATH = Path(__file__).resolve().parents[1] / "patch_notes_categories.json"


def load_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def _edge(frm, to, source="forward"):
    return {"from": frm, "to": to, "relation": "references", "label": "references", "via": [], "source": source}


# ---------------------------------------------------------------------------
# Edge relation / label mapping
# ---------------------------------------------------------------------------


class TestEdgeRelationMapping(unittest.TestCase):
    def test_lvli_forward_is_contains(self):
        self.assertEqual(
            bb.edge_relation_and_label("LVLI", "MISC", "Entries", "forward"), ("contains", "contains")
        )

    def test_lvli_reverse_is_dropped_via(self):
        self.assertEqual(
            bb.edge_relation_and_label("LVLI", "MISC", None, "reverse"), ("contains", "dropped via")
        )

    def test_npc_carried_by(self):
        self.assertEqual(bb.edge_relation_and_label("NPC_", "LVLI", None, "reverse"), ("carried_by", "carried by"))

    def test_cont_found_in(self):
        self.assertEqual(bb.edge_relation_and_label("CONT", "LVLI", None, "reverse"), ("found_in", "found in"))

    def test_omod_mod_for_weap(self):
        self.assertEqual(bb.edge_relation_and_label("OMOD", "WEAP", None, "reverse"), ("mod_for", "mod for"))

    def test_omod_mod_for_armo(self):
        self.assertEqual(bb.edge_relation_and_label("OMOD", "ARMO", None, "reverse")[0], "mod_for")

    def test_omod_non_weapon_armor_dst_falls_back(self):
        self.assertEqual(bb.edge_relation_and_label("OMOD", "MISC", None, "reverse"), ("references", "references"))

    def test_weap_mod_for_omod_reverse(self):
        # The mirror-image direction: a WEAP/ARMO forward-references its
        # compatible OMODs (e.g. via its Object Template), so when the OMOD
        # is the one in the diff universe, `client.refs()` on the OMOD
        # surfaces the WEAP/ARMO as the referencer -- from=WEAP, to=OMOD.
        # Verified against a live ESM: refs(mod_Custom_SaltOfTheEarth)
        # returns DoubleBarrelShotgun (the OMOD's own data has no forward
        # pointer back to the weapon at all).
        self.assertEqual(bb.edge_relation_and_label("WEAP", "OMOD", None, "reverse"), ("mod_for", "mod for"))

    def test_armo_mod_for_omod_reverse(self):
        self.assertEqual(bb.edge_relation_and_label("ARMO", "OMOD", None, "reverse")[0], "mod_for")

    def test_weap_mod_for_omod_also_matches_forward(self):
        # If the WEAP itself were ever in U and forward-discovered the OMOD
        # via its own refs_out, the same relation/label must apply -- the
        # rule doesn't depend on path or discovery direction.
        self.assertEqual(
            bb.edge_relation_and_label("WEAP", "OMOD", "Object Template / Mod", "forward"),
            ("mod_for", "mod for"),
        )

    def test_omod_dst_non_weapon_armor_src_falls_back(self):
        self.assertEqual(bb.edge_relation_and_label("KYWD", "OMOD", None, "reverse"), ("references", "references"))

    def test_cobj_created_object_is_crafts(self):
        self.assertEqual(
            bb.edge_relation_and_label("COBJ", "WEAP", "Data / Created Object", "forward"), ("crafts", "crafts")
        )

    def test_cobj_components_is_crafted_from(self):
        self.assertEqual(
            bb.edge_relation_and_label("COBJ", "MISC", "Components / Component", "forward"),
            ("crafted_from", "crafted from"),
        )

    def test_cobj_workbench_keyword_is_crafted_at(self):
        self.assertEqual(
            bb.edge_relation_and_label("COBJ", "KYWD", "Workbench Keyword", "forward"),
            ("crafted_at", "crafted at"),
        )

    def test_cobj_reverse_edge_has_no_path_so_falls_back(self):
        # Reverse-discovered edges carry `via` (intermediate hops), not a
        # source-record field path, so the path-substring COBJ rules can
        # never fire for them.
        self.assertEqual(bb.edge_relation_and_label("COBJ", "WEAP", None, "reverse"), ("references", "references"))

    def test_ench_spel_mgef_grants_effect(self):
        for src in ("ENCH", "SPEL", "MGEF"):
            self.assertEqual(bb.edge_relation_and_label(src, "ALCH", None, "reverse")[0], "grants_effect")

    def test_qust_rewarded_by(self):
        self.assertEqual(bb.edge_relation_and_label("QUST", "CONT", None, "reverse"), ("rewarded_by", "rewarded by"))

    def test_pcrd_perk_card_for(self):
        self.assertEqual(bb.edge_relation_and_label("PCRD", "PERK", None, "reverse"), ("card_for", "card for"))

    def test_pcrd_non_perk_dst_falls_back(self):
        self.assertEqual(bb.edge_relation_and_label("PCRD", "WEAP", None, "reverse"), ("references", "references"))

    def test_unrelated_types_fallback(self):
        self.assertEqual(bb.edge_relation_and_label("MISC", "MISC", None, "forward"), ("references", "references"))


# ---------------------------------------------------------------------------
# Universe / degrees
# ---------------------------------------------------------------------------


class TestUniverse(unittest.TestCase):
    def test_excludes_wrld_and_cell(self):
        comp = {
            "records": {
                "0x01": {"record_type": "WEAP", "status": "changed"},
                "0x02": {"record_type": "WRLD", "status": "changed"},
                "0x03": {"record_type": "CELL", "status": "changed"},
            }
        }
        u = bb.build_universe(comp)
        self.assertEqual(set(u.keys()), {"0x01"})


class TestDegrees(unittest.TestCase):
    def test_full_degree_counts_distinct_neighbors(self):
        u = {"A": {}, "B": {}}
        edges = [_edge("A", "B"), _edge("A", "C"), _edge("D", "C")]
        full_degree, _ = bb.compute_degrees(u, edges)
        self.assertEqual(full_degree["A"], 2)

    def test_context_u_degree_counts_only_u_neighbors(self):
        u = {"A": {}, "B": {}}
        edges = [_edge("A", "B"), _edge("A", "C"), _edge("D", "C")]
        _, context_u_degree = bb.compute_degrees(u, edges)
        # C's neighbors are {A, D}; only A is in U.
        self.assertEqual(context_u_degree["C"], 1)


# ---------------------------------------------------------------------------
# Union-find hub exemption (isolated from the full pipeline)
# ---------------------------------------------------------------------------


class TestUnionFind(unittest.TestCase):
    def test_direct_uu_edge_unions(self):
        u = {"A": {}, "B": {}, "C": {}}
        edges = [_edge("A", "B")]
        full_degree, context_u_degree = bb.compute_degrees(u, edges)
        dsu = bb.union_find(u, edges, full_degree, context_u_degree, hub_degree=8)
        self.assertEqual(dsu.find("A"), dsu.find("B"))
        self.assertNotEqual(dsu.find("A"), dsu.find("C"))

    def test_non_hub_context_node_bridges_two_u_records(self):
        u = {"A": {}, "B": {}}
        edges = [_edge("A", "LINK"), _edge("B", "LINK")]
        full_degree, context_u_degree = bb.compute_degrees(u, edges)
        dsu = bb.union_find(u, edges, full_degree, context_u_degree, hub_degree=8)
        self.assertEqual(dsu.find("A"), dsu.find("B"))

    def test_hub_context_node_does_not_bridge(self):
        u = {"A": {}, "B": {}}
        edges = [_edge("A", "HUB"), _edge("B", "HUB")]
        full_degree, context_u_degree = bb.compute_degrees(u, edges)
        # HUB touches 2 distinct U records; hub_degree=1 makes it a hub.
        dsu = bb.union_find(u, edges, full_degree, context_u_degree, hub_degree=1)
        self.assertNotEqual(dsu.find("A"), dsu.find("B"))

    def test_hub_uu_edge_endpoint_does_not_union(self):
        u = {"A": {}, "HUBNODE": {}}
        edges = [_edge("A", "HUBNODE")]
        full_degree = {"A": 1, "HUBNODE": 50}  # HUBNODE itself is a hub
        dsu = bb.union_find(u, edges, full_degree, {}, hub_degree=8)
        self.assertNotEqual(dsu.find("A"), dsu.find("HUBNODE"))


# ---------------------------------------------------------------------------
# Anchor selection
# ---------------------------------------------------------------------------


class TestAnchorSelection(unittest.TestCase):
    def test_priority_rank_wins_over_status(self):
        u = {"Q": {"record_type": "QUST", "status": "removed"}, "W": {"record_type": "WEAP", "status": "added"}}
        self.assertEqual(bb.select_anchor({"Q", "W"}, u, {}), "Q")

    def test_status_weight_tiebreak_added_beats_changed(self):
        u = {"W1": {"record_type": "WEAP", "status": "changed"}, "W2": {"record_type": "WEAP", "status": "added"}}
        self.assertEqual(bb.select_anchor({"W1", "W2"}, u, {}), "W2")

    def test_has_name_tiebreak(self):
        u = {
            "W1": {"record_type": "WEAP", "status": "changed", "name": None},
            "W2": {"record_type": "WEAP", "status": "changed", "name": "Has A Name"},
        }
        self.assertEqual(bb.select_anchor({"W1", "W2"}, u, {}), "W2")

    def test_degree_tiebreak(self):
        u = {
            "W1": {"record_type": "WEAP", "status": "changed", "name": "N"},
            "W2": {"record_type": "WEAP", "status": "changed", "name": "N"},
        }
        self.assertEqual(bb.select_anchor({"W1", "W2"}, u, {"W1": 1, "W2": 5}), "W2")

    def test_form_id_tiebreak_lowest_wins(self):
        u = {
            "0x00000002": {"record_type": "WEAP", "status": "changed", "name": "N"},
            "0x00000001": {"record_type": "WEAP", "status": "changed", "name": "N"},
        }
        self.assertEqual(bb.select_anchor({"0x00000002", "0x00000001"}, u, {}), "0x00000001")


# ---------------------------------------------------------------------------
# Oversized-component splitting
# ---------------------------------------------------------------------------


class TestOversizedSplit(unittest.TestCase):
    def _synthesize_two_chains(self):
        """A 41-node component: WEAP1 with a 20-node chain, ARMO1 with a
        19-node chain (41 = 2 anchors + 20 + 19), joined nowhere else."""
        u = {
            "WEAP1": {"record_type": "WEAP", "status": "changed", "name": "A"},
            "ARMO1": {"record_type": "ARMO", "status": "changed", "name": "B"},
        }
        edges = []
        component = {"WEAP1", "ARMO1"}
        # BOOK is deliberately NOT in ANCHOR_PRIORITY, so these fillers never
        # themselves become split candidates -- only WEAP1/ARMO1 do.
        for i in range(20):
            fid = f"0x0050{i:04X}"
            u[fid] = {"record_type": "BOOK", "status": "changed"}
            component.add(fid)
            src = "WEAP1" if i == 0 else f"0x0050{i - 1:04X}"
            edges.append(_edge(fid, src))
        for i in range(19):
            fid = f"0x0060{i:04X}"
            u[fid] = {"record_type": "BOOK", "status": "changed"}
            component.add(fid)
            src = "ARMO1" if i == 0 else f"0x0060{i - 1:04X}"
            edges.append(_edge(fid, src))
        return component, u, edges

    def test_splits_into_two_groups_by_nearest_anchor(self):
        component, u, edges = self._synthesize_two_chains()
        self.assertEqual(len(component), 41)

        groups = bb.split_oversized(component, u, edges, max_members=40)
        self.assertEqual(len(groups), 2)

        by_anchor = {}
        for g in groups:
            anchors_in_g = [f for f in g if f in ("WEAP1", "ARMO1")]
            self.assertEqual(len(anchors_in_g), 1, "each split group should contain exactly one top anchor")
            by_anchor[anchors_in_g[0]] = g

        self.assertEqual(len(by_anchor["WEAP1"]), 21)
        self.assertEqual(len(by_anchor["ARMO1"]), 20)
        for i in range(20):
            self.assertIn(f"0x0050{i:04X}", by_anchor["WEAP1"])
        for i in range(19):
            self.assertIn(f"0x0060{i:04X}", by_anchor["ARMO1"])

    def test_under_cap_is_not_split(self):
        component, u, edges = self._synthesize_two_chains()
        small = set(list(component)[:10])
        self.assertEqual(bb.split_oversized(small, u, edges, max_members=40), [small])

    def test_single_anchor_type_is_not_split(self):
        u = {"WEAP1": {"record_type": "WEAP", "status": "changed"}}
        component = {"WEAP1"}
        edges = []
        for i in range(45):
            fid = f"0x0070{i:04X}"
            u[fid] = {"record_type": "BOOK", "status": "changed"}
            component.add(fid)
            edges.append(_edge(fid, "WEAP1"))
        groups = bb.split_oversized(component, u, edges, max_members=40)
        self.assertEqual(groups, [component])

    def test_tie_break_prefers_higher_priority_anchor(self):
        # MID is distance-1 from both QUST1 (rank 0) and ARMO1 (rank 3);
        # QUST1 must win the tie.
        u = {
            "QUST1": {"record_type": "QUST", "status": "changed"},
            "ARMO1": {"record_type": "ARMO", "status": "changed"},
            "MID": {"record_type": "BOOK", "status": "changed"},
        }
        edges = [_edge("MID", "QUST1"), _edge("MID", "ARMO1")]
        component = {"QUST1", "ARMO1", "MID"}
        for i in range(40):
            fid = f"0x0080{i:04X}"
            u[fid] = {"record_type": "BOOK", "status": "changed"}
            component.add(fid)
            edges.append(_edge(fid, "QUST1"))

        groups = bb.split_oversized(component, u, edges, max_members=40)
        qust_group = next(g for g in groups if "QUST1" in g)
        self.assertIn("MID", qust_group)


# ---------------------------------------------------------------------------
# Bundle merging
# ---------------------------------------------------------------------------


class TestMergeSameAnchor(unittest.TestCase):
    def test_merges_groups_sharing_the_same_selected_anchor(self):
        u = {
            "WEAP1": {"record_type": "WEAP", "status": "changed", "name": "W"},
            "SATA": {"record_type": "MISC", "status": "changed"},
            "SATB": {"record_type": "MISC", "status": "changed"},
        }
        groups = [{"WEAP1", "SATA"}, {"WEAP1", "SATB"}]
        result = bb.merge_same_anchor(groups, u, {})
        self.assertEqual(result, [{"WEAP1", "SATA", "SATB"}])


class TestMergeByOverlap(unittest.TestCase):
    def test_merges_high_overlap_groups(self):
        g1, g2 = {"A", "B", "C", "D", "E"}, {"A", "B", "C", "D", "F"}  # overlap 4/5 = 0.8
        result = bb.merge_by_overlap([g1, g2])
        self.assertEqual(result, [g1 | g2])

    def test_does_not_merge_low_overlap_groups(self):
        g1, g2 = {"A", "B", "C", "D", "E"}, {"A", "X", "Y", "Z", "W"}  # overlap 1/5 = 0.2
        result = bb.merge_by_overlap([g1, g2])
        self.assertEqual(len(result), 2)

    def test_transitive_merge_reaches_fixpoint(self):
        g1, g2, g3 = {"A", "B", "C"}, {"B", "C", "D"}, {"C", "D", "E"}
        result = bb.merge_by_overlap([g1, g2, g3])
        self.assertEqual(result, [{"A", "B", "C", "D", "E"}])


# ---------------------------------------------------------------------------
# Context-member attachment
# ---------------------------------------------------------------------------


class TestAttachContext(unittest.TestCase):
    def test_cap_and_preference_order(self):
        members = {"U1"}
        context_incidence = {}
        context_stubs = {}

        preferred_fids = []
        for i, rtype in enumerate(["NPC_", "CONT", "QUST", "COBJ"]):
            fid = f"0x0060{i:04X}"
            context_incidence[fid] = [("U1", _edge(fid, "U1", source="reverse"))]
            context_stubs[fid] = {"record_type": rtype, "editor_id": f"Pref{i}", "name": None}
            preferred_fids.append(fid)

        kw_fid = "0x00700000"
        context_incidence[kw_fid] = [("U1", _edge(kw_fid, "U1"))]
        context_stubs[kw_fid] = {"record_type": "KYWD", "editor_id": "if_tmp_Special", "name": None}

        other_fids = []
        for i in range(10):
            fid = f"0x0050{i:04X}"
            context_incidence[fid] = [("U1", _edge(fid, "U1"))]
            context_stubs[fid] = {"record_type": "MISC", "editor_id": f"Other{i}", "name": None}
            other_fids.append(fid)

        members_result, edges_result = bb.attach_context(members, context_incidence, context_stubs, 12, ["if_tmp_*"])

        self.assertEqual(len(members_result), 12)
        self.assertEqual(len(edges_result), 12)
        got_ids = [m["form_id"] for m in members_result]

        # The 4 preferred-type candidates and the if_tmp_* keyword all fit
        # comfortably within the cap and must all be present, ahead of the
        # 10 generic MISC candidates (of which only 12 - 5 = 7 fit).
        for fid in preferred_fids:
            self.assertIn(fid, got_ids)
        self.assertIn(kw_fid, got_ids)
        kept_others = [f for f in got_ids if f in other_fids]
        self.assertEqual(len(kept_others), 7)
        # Preference order: the 4 preferred types (in CONTEXT_PREFERRED_TYPES
        # order) come first, then the unique keyword, then the lowest-form-id
        # generic candidates.
        self.assertEqual(got_ids[:4], preferred_fids)
        self.assertEqual(got_ids[4], kw_fid)
        self.assertEqual(kept_others, sorted(other_fids, key=bb._int_fid)[:7])
        for m in members_result:
            self.assertEqual(m["status"], "unchanged")
            self.assertEqual(m["role"], "context")

    def test_no_candidates_returns_empty(self):
        members_result, edges_result = bb.attach_context({"U1"}, {}, {}, 12, ["if_tmp_*"])
        self.assertEqual(members_result, [])
        self.assertEqual(edges_result, [])

    def test_mod_for_target_survives_cap_over_crowding_cont_cobj(self):
        # Regression test for the real-world "Salt Of The Earth (OMOD)"
        # bundle: an OMOD anchor with 13 CONT/COBJ context candidates (more
        # than the cap of 12 by itself) plus the one WEAP it's a mod for.
        # Before the mod_for-aware top tier, the WEAP -- connected only via
        # a "references"-fallback-free "mod_for" edge -- ranked with
        # "everything else" and was crowded out entirely by the 13
        # preferred-type candidates.
        members = {"OMOD1"}
        context_incidence = {}
        context_stubs = {}

        weap_fid = "0x00090000"
        context_incidence[weap_fid] = [
            ("OMOD1", {"from": weap_fid, "to": "OMOD1", "relation": "mod_for", "label": "mod for", "via": [], "source": "reverse"})
        ]
        context_stubs[weap_fid] = {
            "record_type": "WEAP", "editor_id": "DoubleBarrelShotgun", "name": "Double-Barrel Shotgun",
        }

        crowd_fids = []
        for i in range(13):
            fid = f"0x00A0{i:04X}"
            rtype = "CONT" if i % 2 == 0 else "COBJ"
            context_incidence[fid] = [("OMOD1", _edge(fid, "OMOD1", source="reverse"))]
            context_stubs[fid] = {"record_type": rtype, "editor_id": f"Crowd{i}", "name": None}
            crowd_fids.append(fid)

        members_result, edges_result = bb.attach_context(members, context_incidence, context_stubs, 12, ["if_tmp_*"])

        got_ids = [m["form_id"] for m in members_result]
        self.assertEqual(len(members_result), 12)
        self.assertIn(weap_fid, got_ids, "the mod_for target must survive the context cap")
        self.assertEqual(got_ids[0], weap_fid, "the mod_for target must rank ahead of NPC_/CONT/QUST/COBJ")
        self.assertTrue(
            any(e["relation"] == "mod_for" and e["to"] == "OMOD1" for e in edges_result),
            "the WEAP's connecting edge must be the mod_for edge",
        )


# ---------------------------------------------------------------------------
# Categorization
# ---------------------------------------------------------------------------


class _NoKeywordsClient:
    """A client whose record() always resolves but reports no Keywords."""

    def record(self, esm, formid, *, resolve="stub"):
        return {"fields": {}}


class TestCategorization(unittest.TestCase):
    def setUp(self):
        self.config = load_json(CATEGORIES_PATH)
        self.categories = self.config["categories"]
        self.client = _NoKeywordsClient()

    @staticmethod
    def _member(form_id, record_type, editor_id=None, name=None, role="anchor", status="changed"):
        return {
            "form_id": form_id, "record_type": record_type, "editor_id": editor_id,
            "name": name, "status": status, "role": role,
        }

    def test_perk_anchor_matches_perks_rule_0(self):
        anchor = self._member("0x01", "PERK", "SomePerk")
        cat_id, _label, rule = bb.categorize_bundle(anchor, [anchor], self.categories, self.client, "esm", {})
        self.assertEqual(cat_id, "perks")
        self.assertEqual(rule, "perks/rule_0")

    def test_first_match_wins_unique_weapons_over_weapons_combat(self):
        anchor = self._member("0x01", "WEAP", "SomeRifle")
        kywd_member = self._member("0x02", "KYWD", "if_tmp_Something", role="satellite")
        cat_id, _label, rule = bb.categorize_bundle(
            anchor, [anchor, kywd_member], self.categories, self.client, "esm", {}
        )
        self.assertEqual(cat_id, "unique_weapons_gear")
        self.assertEqual(rule, "unique_weapons_gear/rule_0")

    def test_falls_through_to_weapons_combat_without_unique_keyword(self):
        anchor = self._member("0x01", "WEAP", "SomeRifle")
        cat_id, _label, _rule = bb.categorize_bundle(anchor, [anchor], self.categories, self.client, "esm", {})
        self.assertEqual(cat_id, "weapons_combat")

    def test_omod_anchor_with_mod_custom_edid_is_unique_weapons_gear(self):
        # Regression test: an OMOD anchor named per Bethesda's unique-item
        # mod convention (mod_Custom_*) must categorize as unique gear even
        # with no if_tmp_* keyword member at all (the real-world
        # "Salt Of The Earth (OMOD)" bundle had none -- the WEAP it modifies
        # was crowded out of the context cap, see attach_context tests).
        anchor = self._member("0x04", "OMOD", "mod_Custom_SaltOfTheEarth")
        cat_id, _label, rule = bb.categorize_bundle(anchor, [anchor], self.categories, self.client, "esm", {})
        self.assertEqual(cat_id, "unique_weapons_gear")
        self.assertEqual(rule, "unique_weapons_gear/rule_2")

    def test_plain_omod_anchor_still_falls_through_to_weapons_combat(self):
        # A plain (non mod_Custom_*) OMOD anchor, with no if_tmp_* keyword
        # anywhere, must still land in weapons_combat -- the new OMOD rule
        # must not over-match every OMOD.
        anchor = self._member("0x05", "OMOD", "mod_LegendaryEffect_Bloodied")
        cat_id, _label, rule = bb.categorize_bundle(anchor, [anchor], self.categories, self.client, "esm", {})
        self.assertEqual(cat_id, "weapons_combat")
        self.assertEqual(rule, "weapons_combat/rule_0")

    def test_member_scope_if_tmp_keyword_routes_to_unique_weapons_gear_with_other_anchor(self):
        # The member-scoped keyword rule catches bundles whose weapon
        # carries the unique keyword even when the anchor is something
        # else entirely (here MISC, which would otherwise fall to ui_misc).
        anchor = self._member("0x06", "MISC", "SomeJunkItem")
        kywd_member = self._member("0x07", "KYWD", "if_tmp_UniqueThing", role="satellite")
        cat_id, _label, _rule = bb.categorize_bundle(
            anchor, [anchor, kywd_member], self.categories, self.client, "esm", {}
        )
        self.assertEqual(cat_id, "unique_weapons_gear")

    def test_uncategorized_fallback_has_no_rule(self):
        anchor = self._member("0x02", "CONT", "SomeContainer")
        cat_id, _label, rule = bb.categorize_bundle(anchor, [anchor], self.categories, self.client, "esm", {})
        self.assertEqual(cat_id, "uncategorized")
        self.assertIsNone(rule)

    def test_keyword_scope_rule_via_client_record_stub(self):
        class _KeywordClient:
            def record(self, esm, formid, *, resolve="stub"):
                return {"fields": {"Keywords": [{"formid": "0x00999999", "editor_id": "if_tmp_SpecialGear"}]}}

        anchor = self._member("0x03", "WEAP", "PlainNamedRifle")
        cat_id, _label, rule = bb.categorize_bundle(
            anchor, [anchor], self.categories, _KeywordClient(), "esm", {}
        )
        self.assertEqual(cat_id, "unique_weapons_gear")
        self.assertEqual(rule, "unique_weapons_gear/rule_1")

    def test_keyword_lookup_is_cached_per_anchor(self):
        class _CountingClient:
            def __init__(self):
                self.calls = 0

            def record(self, esm, formid, *, resolve="stub"):
                self.calls += 1
                return {"fields": {"Keywords": [{"editor_id": "if_tmp_X"}]}}

        client = _CountingClient()
        cache = {}
        self.assertEqual(bb._anchor_keyword_edids(client, "esm", "0xAA", cache), ["if_tmp_X"])
        self.assertEqual(bb._anchor_keyword_edids(client, "esm", "0xAA", cache), ["if_tmp_X"])
        self.assertEqual(client.calls, 1)

    def test_keyword_lookup_failure_is_treated_as_no_match(self):
        class _FailingClient:
            def record(self, esm, formid, *, resolve="stub"):
                raise esm_daemon.DaemonError("not found")

        self.assertEqual(bb._anchor_keyword_edids(_FailingClient(), "esm", "0xBB", {}), [])


class TestSettingsPrecedence(unittest.TestCase):
    def test_resolve_settings_merges_config_over_defaults(self):
        settings = bb.resolve_settings({"settings": {"hub_degree": 99}})
        self.assertEqual(settings["hub_degree"], 99)
        self.assertEqual(settings["max_members"], bb.DEFAULT_SETTINGS["max_members"])

    def test_resolve_settings_empty_config_uses_defaults(self):
        self.assertEqual(bb.resolve_settings({}), bb.DEFAULT_SETTINGS)


# ---------------------------------------------------------------------------
# Regression: "Salt Of The Earth (OMOD)" bundle -- an OMOD anchor whose only
# forward-facing "true" bundle-mate (the WEAP it's a mod for) was crowded
# out of the context cap by CONT/COBJ candidates, and mis-categorized as
# weapons_combat instead of unique_weapons_gear. Runs the real, current
# patch_notes_categories.json end to end through build_bundles(), but with
# an inline FakeClient fixture (not the shared refs_graph.json /
# comprehensive_mini.json, which other test modules also depend on).
# ---------------------------------------------------------------------------


class TestModForOmodBundleRegression(unittest.TestCase):
    OMOD_FID = "0x008F0DCD"
    WEAP_FID = "0x00092217"

    @classmethod
    def setUpClass(cls):
        cls.config = load_json(CATEGORIES_PATH)

        records = {
            cls.OMOD_FID: {
                "record_type": "OMOD", "editor_id": "mod_Custom_SaltOfTheEarth", "name": "Salt Of The Earth",
            },
            cls.WEAP_FID: {
                "record_type": "WEAP", "editor_id": "DoubleBarrelShotgun", "name": "Double-Barrel Shotgun",
            },
        }
        # refs(OMOD) rows: the WEAP (a genuine forward reference from the
        # WEAP's own Object Template, verified against a live ESM) plus 13
        # CONT/COBJ candidates -- more than the context_cap of 12 -- so the
        # WEAP would be crowded out entirely without the mod_for top tier.
        refs_rows = [{"form_id": cls.WEAP_FID, "record_type": "WEAP", "editor_id": "DoubleBarrelShotgun"}]
        for i in range(13):
            fid = f"0x00A0{i:04X}"
            rtype = "CONT" if i % 2 == 0 else "COBJ"
            records[fid] = {"record_type": rtype, "editor_id": f"Crowd{i}", "name": None}
            refs_rows.append({"form_id": fid, "record_type": rtype, "editor_id": f"Crowd{i}"})

        refs_fixture = {"records": records, "refs": {cls.OMOD_FID: refs_rows}}

        comp = {
            "meta": {"patch_date": "2026-07-04"},
            "records": {
                cls.OMOD_FID: {
                    "form_id": cls.OMOD_FID,
                    "record_type": "OMOD",
                    "editor_id": "mod_Custom_SaltOfTheEarth",
                    "name": "Salt Of The Earth",
                    "status": "added",
                    "refs_out": [],
                    "changes": [],
                },
            },
            "ref_names": {},
        }

        client = esm_daemon.FakeClient(refs_fixture)
        cls.result = bb.build_bundles(comp, client, "OLD.esm", "NEW.esm", cls.config)

    def test_single_bundle_anchored_on_the_omod(self):
        self.assertEqual(len(self.result["bundles"]), 1)
        bundle = self.result["bundles"][0]
        self.assertEqual(bundle["anchor"]["form_id"], self.OMOD_FID)
        self.assertEqual(bundle["anchor"]["record_type"], "OMOD")

    def test_categorized_as_unique_weapons_gear_not_weapons_combat(self):
        bundle = self.result["bundles"][0]
        self.assertEqual(bundle["category"], "unique_weapons_gear")
        self.assertEqual(bundle["category_rule"], "unique_weapons_gear/rule_2")

    def test_weap_survives_the_context_cap_ahead_of_cont_cobj_crowd(self):
        bundle = self.result["bundles"][0]
        context_members = [m for m in bundle["members"] if m["role"] == "context"]
        self.assertEqual(len(context_members), 12)  # default context_cap
        context_ids = [m["form_id"] for m in context_members]
        self.assertIn(self.WEAP_FID, context_ids, "the WEAP the OMOD mods must survive the cap")
        self.assertEqual(context_ids[0], self.WEAP_FID, "mod_for target must rank ahead of CONT/COBJ")

        weap_edges = [e for e in bundle["edges"] if e["from"] == self.WEAP_FID and e["to"] == self.OMOD_FID]
        self.assertTrue(any(e["relation"] == "mod_for" and e["label"] == "mod for" for e in weap_edges))


# ---------------------------------------------------------------------------
# Full offline pipeline (FakeClient + refs_graph.json + comprehensive_mini.json)
# ---------------------------------------------------------------------------


class TestFullPipelineOffline(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.comp = load_json(COMPREHENSIVE_MINI_PATH)
        cls.config = load_json(CATEGORIES_PATH)
        cls.client = esm_daemon.FakeClient(REFS_FIXTURE_PATH)
        cls.result = bb.build_bundles(cls.comp, cls.client, "OLD.esm", "NEW.esm", cls.config)

    def _bundle_containing(self, form_id):
        for b in self.result["bundles"]:
            if any(m["form_id"] == form_id for m in b["members"]):
                return b
        raise AssertionError(f"no bundle contains {form_id}")

    def test_weap_omod_lvli_keyword_cluster_is_one_bundle_with_weap_anchor(self):
        b = self._bundle_containing("0x00100001")
        self.assertEqual(b["anchor"]["form_id"], "0x00100001")
        self.assertEqual(b["anchor"]["record_type"], "WEAP")
        non_context = {m["form_id"] for m in b["members"] if m["role"] != "context"}
        self.assertEqual(
            non_context,
            {"0x00100001", "0x00100010", "0x00100011", "0x00100020", "0x00100040", "0x00100050"},
        )
        self.assertEqual(b["category"], "unique_weapons_gear")
        self.assertEqual(b["category_rule"], "unique_weapons_gear/rule_0")

    def test_npc_and_cont_are_context_members_with_expected_edges(self):
        b = self._bundle_containing("0x00100001")
        context_fids = {m["form_id"]: m for m in b["members"] if m["role"] == "context"}
        self.assertIn("0x00100030", context_fids)  # NPC_TestRaider
        self.assertIn("0x00100031", context_fids)  # CONT_TestContainer
        self.assertEqual(context_fids["0x00100030"]["status"], "unchanged")
        self.assertEqual(context_fids["0x00100030"]["record_type"], "NPC_")
        self.assertEqual(context_fids["0x00100031"]["record_type"], "CONT")

        npc_edges = [e for e in b["edges"] if e["from"] == "0x00100030"]
        self.assertTrue(any(e["relation"] == "carried_by" and e["label"] == "carried by" for e in npc_edges))
        cont_edges = [e for e in b["edges"] if e["from"] == "0x00100031"]
        self.assertTrue(any(e["relation"] == "found_in" and e["label"] == "found in" for e in cont_edges))

    def test_cobj_crafts_and_crafted_from_and_crafted_at_edges(self):
        b = self._bundle_containing("0x00100040")
        crafts = [e for e in b["edges"] if e["from"] == "0x00100040" and e["to"] == "0x00100001"]
        self.assertTrue(any(e["relation"] == "crafts" for e in crafts))
        crafted_from = [e for e in b["edges"] if e["from"] == "0x00100040" and e["to"] == "0x00200001"]
        self.assertTrue(any(e["relation"] == "crafted_from" for e in crafted_from))
        crafted_at = [e for e in b["edges"] if e["from"] == "0x00100040" and e["to"] == "0x00200002"]
        self.assertTrue(any(e["relation"] == "crafted_at" for e in crafted_at))

    def test_hub_keyword_does_not_merge_unrelated_bundles_but_appears_as_member(self):
        hub_referrer_fids = {
            "0x00100091", "0x00100092", "0x00100093", "0x00100094",
            "0x00100095", "0x00100096", "0x00100097", "0x00100098",
            "0x00100099", "0x0010009A", "0x0010009B", "0x0010009C",
        }
        hub_bundles = [
            b for b in self.result["bundles"] if any(m["form_id"] == "0x00100090" for m in b["members"])
        ]
        self.assertEqual(len(hub_bundles), 12, "each hub referrer must stay in its own bundle")

        anchors = {b["anchor"]["form_id"] for b in hub_bundles}
        self.assertEqual(anchors, hub_referrer_fids)

        for b in hub_bundles:
            non_context = [m for m in b["members"] if m["role"] != "context"]
            self.assertEqual(len(non_context), 1, "the hub keyword must not have unioned its referrers together")
            hub_member = next(m for m in b["members"] if m["form_id"] == "0x00100090")
            self.assertEqual(hub_member["role"], "context")
            self.assertEqual(hub_member["record_type"], "KYWD")

    def test_singleton_orphan_records_become_their_own_bundles(self):
        b1 = self._bundle_containing("0x00100072")  # TestPerk02_orphan
        self.assertEqual(len(b1["members"]), 1)
        self.assertEqual(b1["anchor"]["form_id"], "0x00100072")

        b2 = self._bundle_containing("0x00100080")  # OrphanKeyword
        self.assertEqual(len(b2["members"]), 1)
        self.assertEqual(b2["anchor"]["form_id"], "0x00100080")
        self.assertNotEqual(b1["id"], b2["id"])

    def test_perk_pcrd_pair_card_for_edge(self):
        b = self._bundle_containing("0x00100070")
        self.assertEqual(b["anchor"]["form_id"], "0x00100070")
        pcrd_member = next(m for m in b["members"] if m["form_id"] == "0x00100071")
        self.assertEqual(pcrd_member["role"], "context")
        edge = next(e for e in b["edges"] if e["from"] == "0x00100071" and e["to"] == "0x00100070")
        self.assertEqual(edge["relation"], "card_for")
        self.assertEqual(edge["label"], "card for")
        self.assertEqual(b["category"], "perks")

    def test_uncategorized_fallback_for_unmapped_record_type(self):
        b = self._bundle_containing("0x00300002")  # CONT_UncategorizedTest
        self.assertEqual(b["category"], "uncategorized")
        self.assertIsNone(b["category_rule"])

    def test_removed_record_is_queried_against_old_esm(self):
        class _SpyClient:
            def __init__(self, inner):
                self.inner = inner
                self.calls = []

            def refs(self, esm, formid, *, depth=2, limit=0):
                self.calls.append((esm, formid))
                return self.inner.refs(esm, formid, depth=depth, limit=limit)

            def record(self, esm, formid, *, resolve="stub"):
                return self.inner.record(esm, formid, resolve=resolve)

        spy = _SpyClient(esm_daemon.FakeClient(REFS_FIXTURE_PATH))
        bb.build_bundles(self.comp, spy, "OLD.esm", "NEW.esm", self.config)

        removed_calls = [c for c in spy.calls if c[1] == "0x00300001"]  # ALCH_TestChem, status=removed
        self.assertTrue(removed_calls)
        self.assertTrue(all(esm == "OLD.esm" for esm, _fid in removed_calls))

        changed_calls = [c for c in spy.calls if c[1] == "0x00100001"]  # WEAP_TestRifle, status=changed
        self.assertTrue(changed_calls)
        self.assertTrue(all(esm == "NEW.esm" for esm, _fid in changed_calls))

    def test_deterministic_ids_across_two_runs(self):
        client2 = esm_daemon.FakeClient(REFS_FIXTURE_PATH)
        result2 = bb.build_bundles(self.comp, client2, "OLD.esm", "NEW.esm", self.config)
        ids1 = [(b["id"], b["category"], b["anchor"]["form_id"]) for b in self.result["bundles"]]
        ids2 = [(b["id"], b["category"], b["anchor"]["form_id"]) for b in result2["bundles"]]
        self.assertEqual(ids1, ids2)

    def test_meta_counts_match_bundle_list(self):
        counts = self.result["meta"]["counts"]
        self.assertEqual(counts["bundles"], len(self.result["bundles"]))
        singleton_count = sum(
            1 for b in self.result["bundles"]
            if sum(1 for m in b["members"] if m["role"] != "context") == 1
        )
        self.assertEqual(counts["singletons"], singleton_count)
        uncategorized_count = sum(1 for b in self.result["bundles"] if b["category"] == "uncategorized")
        self.assertEqual(counts["uncategorized"], uncategorized_count)


class TestBundleShapeContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        comp = load_json(COMPREHENSIVE_MINI_PATH)
        config = load_json(CATEGORIES_PATH)
        client = esm_daemon.FakeClient(REFS_FIXTURE_PATH)
        cls.result = bb.build_bundles(comp, client, "OLD.esm", "NEW.esm", config)
        cls.categories = config["categories"]

    def test_top_level_shape(self):
        for key in ("schema_version", "meta", "bundles", "lints"):
            self.assertIn(key, self.result)
        self.assertEqual(self.result["schema_version"], 1)
        self.assertEqual(self.result["lints"], [])

    def test_meta_shape(self):
        meta = self.result["meta"]
        for key in ("patch_date", "generated_at", "source", "refs_depth", "hub_degree", "max_members", "counts"):
            self.assertIn(key, meta)
        self.assertEqual(meta["source"], "comprehensive.json")
        for key in ("bundles", "singletons", "uncategorized"):
            self.assertIn(key, meta["counts"])

    def test_bundle_shape(self):
        for b in self.result["bundles"]:
            for key in (
                "id", "category", "category_label", "category_rule", "title",
                "anchor", "members", "edges", "bug_watch", "lint_ids",
            ):
                self.assertIn(key, b)
            self.assertRegex(b["id"], r"^B\d{4}$")
            self.assertFalse(b["bug_watch"])
            self.assertEqual(b["lint_ids"], [])
            for key in ("form_id", "record_type", "editor_id", "name", "status"):
                self.assertIn(key, b["anchor"])
            for m in b["members"]:
                for key in ("form_id", "record_type", "editor_id", "name", "status", "role"):
                    self.assertIn(key, m)
                self.assertIn(m["role"], ("anchor", "satellite", "context"))
            for e in b["edges"]:
                for key in ("from", "to", "relation", "label", "via", "source"):
                    self.assertIn(key, e)
                self.assertIn(e["source"], ("forward", "reverse"))

    def test_bundle_ids_are_sequential_and_unique(self):
        ids = [b["id"] for b in self.result["bundles"]]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertEqual(ids, [f"B{i:04d}" for i in range(1, len(ids) + 1)])

    def test_bundle_sort_order_is_category_then_anchor_form_id(self):
        cat_order = {c["id"]: i for i, c in enumerate(self.categories)}
        keys = [(cat_order.get(b["category"], len(self.categories)), bb._int_fid(b["anchor"]["form_id"])) for b in self.result["bundles"]]
        self.assertEqual(keys, sorted(keys))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


class TestCli(unittest.TestCase):
    def _run(self, *extra_args):
        return subprocess.run(
            [
                sys.executable, str(SCRIPT_PATH), str(COMPREHENSIVE_MINI_PATH),
                "--old-esm", "old.esm", "--new-esm", "new.esm",
                *extra_args,
            ],
            capture_output=True, text=True,
        )

    def test_offline_requires_refs_fixture(self):
        result = self._run("--offline")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--refs-fixture", result.stderr)

    def test_missing_comprehensive_json_is_hard_error(self):
        result = subprocess.run(
            [
                sys.executable, str(SCRIPT_PATH), "/nonexistent/comprehensive.json",
                "--old-esm", "old.esm", "--new-esm", "new.esm",
                "--offline", "--refs-fixture", str(REFS_FIXTURE_PATH),
            ],
            capture_output=True, text=True,
        )
        self.assertNotEqual(result.returncode, 0)

    def test_missing_categories_file_is_hard_error(self):
        result = self._run("--offline", "--refs-fixture", str(REFS_FIXTURE_PATH), "--categories", "/nonexistent/cats.json")
        self.assertNotEqual(result.returncode, 0)

    def test_full_subprocess_run_writes_valid_bundles_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            out_path = Path(tmp) / "bundles.json"
            result = self._run("--offline", "--refs-fixture", str(REFS_FIXTURE_PATH), "--out", str(out_path))
            self.assertEqual(result.returncode, 0, result.stderr)
            data = json.loads(out_path.read_text())
            self.assertEqual(data["schema_version"], 1)
            self.assertIn("bundles", data)
            self.assertEqual(data["meta"]["counts"]["bundles"], len(data["bundles"]))

    def test_cli_flag_overrides_config_settings(self):
        with tempfile.TemporaryDirectory() as tmp:
            out_path = Path(tmp) / "bundles.json"
            result = self._run(
                "--offline", "--refs-fixture", str(REFS_FIXTURE_PATH),
                "--hub-degree", "1000", "--out", str(out_path),
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            data = json.loads(out_path.read_text())
            self.assertEqual(data["meta"]["hub_degree"], 1000)
            # With no hub exemption, the 12 hub-keyword referrers all
            # collapse into a single bundle instead of 12 singletons.
            self.assertLess(data["meta"]["counts"]["bundles"], 18)


if __name__ == "__main__":
    unittest.main()
