#!/usr/bin/env python3
"""Tests for tools/lvli_audit.py.

`lvli_audit.py` had zero test coverage before this file — every rule-check
function and `analyze_record` take plain dicts/sets and return plain dicts,
with no gateway dependency, so they're unit-tested directly against
synthetic fixtures built to actually exercise each rule's documented logic
(see the module docstring's rules A-D). The one gateway-facing seam,
`list_lvli_form_ids`, is wired through the existing `FakeGateway` (see
`tools/tests/fake_gateway.py`) rather than a second fake.
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import lvli_audit as la  # noqa: E402
from fake_gateway import FakeGateway  # noqa: E402


def cond(function: str, operator: str, value) -> dict:
    """One `Leveled List Entry`'s Conditions[]-shaped entry, matching what
    `entry_conditions()` unwraps: `{"Condition": {"Condition Data": {...}}}`."""
    return {
        "Condition": {
            "Condition Data": {
                "Function": function,
                "Operator": operator,
                "Comparison Value": value,
            }
        }
    }


def entry(ref_edid: str, *, conditions: list[dict] | None = None, min_level=None) -> dict:
    """An already-unwrapped LVLI entry dict (the shape `analyze_record`'s
    rule-check functions consume, i.e. post `lvli_entry.unwrap_entry`)."""
    e: dict = {"Reference": {"editor_id": ref_edid}}
    if conditions is not None:
        e["Conditions"] = {"Conditions": conditions}
    if min_level is not None:
        e["Minimum Level"] = min_level
    return e


# ---------------------------------------------------------------------------
# Rule A: check_use_first_starvation
# ---------------------------------------------------------------------------


class TestCheckUseFirstStarvation(unittest.TestCase):
    def test_not_use_first_match_never_fires(self):
        entries = [entry("A"), entry("B")]
        self.assertIsNone(la.check_use_first_starvation(entries, set(), 95.0, {}))

    def test_unconditioned_early_entry_starves_the_rest(self):
        # Entry 0 has NO Conditions at all -> always-true -> under Use First
        # Match, entry 1 is unreachable. This is the exact shape rule A's
        # docstring describes ("an entry with no Conditions ... every entry
        # listed after it becomes unreachable").
        entries = [entry("Common"), entry("Rare", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)])]
        flags = {la.USE_FIRST_MATCH_FLAG}
        result = la.check_use_first_starvation(entries, flags, 95.0, {})
        assert result is not None
        self.assertEqual(len(result["hits"]), 1)
        hit = result["hits"][0]
        self.assertEqual(hit["index"], 0)
        self.assertEqual(hit["reference"], "Common")
        self.assertIn("always-true", hit["certainty"])
        self.assertEqual(hit["starved_count"], 1)

    def test_near_certain_random_percent_also_starves(self):
        # Entry 0's only Condition is GetRandomPercent <= 99 (>= the default
        # 95 near-certain threshold) -> near-always true.
        entries = [
            entry("Common", conditions=[cond("GetRandomPercent", "Less Than Or Equal To", 99.0)]),
            entry("Rare", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)]),
        ]
        flags = {la.USE_FIRST_MATCH_FLAG}
        result = la.check_use_first_starvation(entries, flags, 95.0, {})
        assert result is not None
        self.assertIn("near-certain", result["hits"][0]["certainty"])

    def test_real_conditions_on_every_entry_does_not_fire(self):
        # A well-ordered Use-First-Match list where every entry carries a
        # real, non-near-certain, non-GetRandomPercent Condition (e.g.
        # "give the first recipe not yet learned") is legitimately
        # order-dependent -- rule A must NOT flag it, per the docstring's
        # explicit carve-out.
        entries = [
            entry("RecipeA", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)], min_level=5),
            entry("RecipeB", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)], min_level=10),
            entry("RecipeC", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)], min_level=15),
        ]
        flags = {la.USE_FIRST_MATCH_FLAG}
        result = la.check_use_first_starvation(entries, flags, 95.0, {})
        self.assertIsNone(result)

    def test_ascending_level_alone_on_real_conditions_is_not_flagged(self):
        # Corroborating-evidence-only check: ascending Minimum Level plus a
        # real (non-near-certain) Condition must still not trigger by
        # itself -- level ordering is never an independent trigger.
        entries = [
            entry("A", conditions=[cond("GetIsInRegion", "Equal To", 1.0)], min_level=1),
            entry("B", conditions=[cond("GetIsInRegion", "Equal To", 1.0)], min_level=50),
        ]
        flags = {la.USE_FIRST_MATCH_FLAG}
        self.assertIsNone(la.check_use_first_starvation(entries, flags, 95.0, {}))


# ---------------------------------------------------------------------------
# Rule B: check_bundle_name_uniform_pick
# ---------------------------------------------------------------------------


class TestCheckBundleNameUniformPick(unittest.TestCase):
    def test_bundle_suggestive_name_with_enough_entries_fires(self):
        entries = [entry("A"), entry("B"), entry("C")]
        result = la.check_bundle_name_uniform_pick(entries, set(), "LL_StarterRewardBundle")
        self.assertEqual(result, {"entry_count": 3})

    def test_plain_variant_pick_list_does_not_fire(self):
        # Normal "pick one of N variants" shape: multi-entry, no flags, but
        # the EditorID/Name has no bundle-suggestive substring -- per the
        # docstring this is the common, non-flagged case.
        entries = [entry("VariantA"), entry("VariantB"), entry("VariantC")]
        result = la.check_bundle_name_uniform_pick(entries, set(), "LL_WeaponSkinVariants")
        self.assertIsNone(result)

    def test_use_all_flag_suppresses_even_with_bundle_name(self):
        entries = [entry("A"), entry("B"), entry("C")]
        result = la.check_bundle_name_uniform_pick(entries, {la.USE_ALL_FLAG}, "RewardSetBundle")
        self.assertIsNone(result)

    def test_fewer_than_three_entries_does_not_fire(self):
        entries = [entry("A"), entry("B")]
        result = la.check_bundle_name_uniform_pick(entries, set(), "RewardBundle")
        self.assertIsNone(result)


# ---------------------------------------------------------------------------
# Rule C: check_level_tier_starvation
# ---------------------------------------------------------------------------


class TestCheckLevelTierStarvation(unittest.TestCase):
    def test_multiple_min_levels_without_calc_all_flag_fires(self):
        entries = [entry("A", min_level=5), entry("B", min_level=25), entry("C", min_level=50)]
        result = la.check_level_tier_starvation(entries, set(), {})
        self.assertEqual(result, {"levels": [5.0, 25.0, 50.0]})

    def test_calc_all_levels_flag_set_does_not_fire(self):
        # The flag that (per the docstring's caveat) tells the engine to
        # consider every entry <= player level, not just the single
        # highest -- with it set, multiple tiers are exactly the intended
        # shape, not a starvation risk.
        entries = [entry("A", min_level=5), entry("B", min_level=25), entry("C", min_level=50)]
        result = la.check_level_tier_starvation(entries, {la.CALC_ALL_LEVELS_FLAG}, {})
        self.assertIsNone(result)

    def test_single_level_does_not_fire(self):
        entries = [entry("A", min_level=10), entry("B", min_level=10)]
        result = la.check_level_tier_starvation(entries, set(), {})
        self.assertIsNone(result)


# ---------------------------------------------------------------------------
# Rule D: check_overlap_ladder
# ---------------------------------------------------------------------------


class TestCheckOverlapLadder(unittest.TestCase):
    def test_two_sided_random_percent_ladder_fires(self):
        # The classic hand-authored ladder shape from the docstring: two
        # entries each gated on a one-sided GetRandomPercent threshold
        # (<=90, <=10), same function -> same_function ladder, with exact
        # pool odds computed (both thresholds are plain floats, no GLOBs).
        entries = [
            entry("Common", conditions=[cond("GetRandomPercent", "Less Than Or Equal To", 90.0)]),
            entry("Rare", conditions=[cond("GetRandomPercent", "Less Than Or Equal To", 10.0)]),
        ]
        result = la.check_overlap_ladder(entries, set(), {})
        assert result is not None
        self.assertTrue(result["same_function"])
        self.assertEqual(len(result["entries"]), 2)
        # Odds should be computed (both gates are GetRandomPercent w/ a
        # resolved literal threshold, and entry count is well under the cap).
        self.assertIsNotNone(result["entries"][0]["pool_odds"])
        self.assertIsNotNone(result["entries"][0]["naive_cascade_odds"])

    def test_use_first_match_flag_suppresses(self):
        entries = [
            entry("Common", conditions=[cond("GetRandomPercent", "Less Than Or Equal To", 90.0)]),
            entry("Rare", conditions=[cond("GetRandomPercent", "Less Than Or Equal To", 10.0)]),
        ]
        result = la.check_overlap_ladder(entries, {la.USE_FIRST_MATCH_FLAG}, {})
        self.assertIsNone(result)

    def test_bounded_range_condition_does_not_count_as_unbounded(self):
        # entry 0's Conditions form a genuinely BOUNDED range on the same
        # Function/Parameters (GetLevel >= 10 AND GetLevel < 20) -- per
        # find_unbounded_gate's docstring this must not be mistaken for a
        # one-sided gate. entry 1 is unconditioned (no overlap risk by
        # itself). Fewer than 2 gated entries overall -> rule does not fire.
        entries = [
            entry(
                "MidTier",
                conditions=[
                    cond("GetLevel", "Greater Than Or Equal To", 10.0),
                    cond("GetLevel", "Less Than", 20.0),
                ],
            ),
            entry("Unconditioned"),
        ]
        result = la.check_overlap_ladder(entries, set(), {})
        self.assertIsNone(result)

    def test_single_gated_entry_does_not_fire(self):
        # Only one entry carries a one-sided gate -- need 2+ for "overlap".
        entries = [
            entry("Gated", conditions=[cond("GetRandomPercent", "Less Than Or Equal To", 50.0)]),
            entry("Unconditioned"),
        ]
        result = la.check_overlap_ladder(entries, set(), {})
        self.assertIsNone(result)


# ---------------------------------------------------------------------------
# analyze_record
# ---------------------------------------------------------------------------


def make_record(*, form_id="0x00123456", editor_id="LL_Test", flags=None, raw_entries=None, name=None):
    return {
        "header": {"form_id": form_id},
        "editor_id": editor_id,
        "fields": {
            "Flags": {"flags": sorted(flags or [])},
            "Leveled List Entries": raw_entries or [],
            "Override Name": name,
        },
    }


class TestAnalyzeRecord(unittest.TestCase):
    def test_error_record_passes_through_unanalyzed(self):
        rec = {"error": "bulk_get failed", "sel": "0x00000001"}
        result = la.analyze_record(rec, la.DEFAULT_NEAR_CERTAIN_THRESHOLD, {})
        self.assertEqual(result, {"error": "bulk_get failed", "sel": "0x00000001"})

    def test_record_with_starvation_produces_finding_a(self):
        raw_entries = [
            {"Leveled List Entry": entry("Common")},
            {"Leveled List Entry": entry("Rare", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)])},
        ]
        rec = make_record(flags={la.USE_FIRST_MATCH_FLAG}, raw_entries=raw_entries)
        result = la.analyze_record(rec, la.DEFAULT_NEAR_CERTAIN_THRESHOLD, {})
        assert result is not None
        self.assertEqual(result["form_id"], "0x00123456")
        self.assertEqual(result["editor_id"], "LL_Test")
        self.assertIn("A", result["findings"])

    def test_wrapped_and_unwrapped_entries_both_analyzed(self):
        # Regression cover for the Stage H unwrap fix: an entry with no
        # "Leveled List Entry" wrapper key at all must still be read (the
        # safe pass-through), not silently dropped, matching
        # `lvli_entry.unwrap_entry`'s documented behavior.
        raw_entries = [
            entry("Common"),  # already-unwrapped -- no wrapper key
            {"Leveled List Entry": entry("Rare", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)])},
        ]
        rec = make_record(flags={la.USE_FIRST_MATCH_FLAG}, raw_entries=raw_entries)
        result = la.analyze_record(rec, la.DEFAULT_NEAR_CERTAIN_THRESHOLD, {})
        assert result is not None
        self.assertIn("A", result["findings"])
        self.assertEqual(result["findings"]["A"]["hits"][0]["starved_count"], 1)

    def test_clean_record_produces_no_findings(self):
        # No flags, two entries each with a real equality Condition (no
        # bundle-suggestive name, no varying Minimum Level, no unbounded
        # range gates) -- none of the four rules should fire, and
        # analyze_record should return None (not a dict with empty findings).
        raw_entries = [
            {"Leveled List Entry": entry("VariantA", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)])},
            {"Leveled List Entry": entry("VariantB", conditions=[cond("HasLearnedRecipe", "Equal To", 0.0)])},
        ]
        rec = make_record(flags=set(), raw_entries=raw_entries, name="LL_PlainVariantPick")
        result = la.analyze_record(rec, la.DEFAULT_NEAR_CERTAIN_THRESHOLD, {})
        self.assertIsNone(result)


# ---------------------------------------------------------------------------
# list_lvli_form_ids, via FakeGateway (see tools/tests/fake_gateway.py) --
# the one gateway-facing seam in this file besides main()'s daemon-driving
# glue. Matches how test_orchestrator.py drives make_patch_notes.py through
# FakeGateway; lvli_audit.py has no --offline/fixture-driven CLI entry point
# of its own, so this calls the seam function directly with a FakeGateway
# instance rather than adding new CLI surface for a testing-only stage.
# ---------------------------------------------------------------------------


class TestListLvliFormIds(unittest.TestCase):
    def test_filters_to_lvli_type_only_sorted_ascending(self):
        fixture = {
            "records": {
                "0x00002222": {"record_type": "LVLI", "editor_id": "LL_Second"},
                "0x00000001": {"record_type": "LVLI", "editor_id": "LL_First"},
                "0x00003333": {"record_type": "WEAP", "editor_id": "NotAList"},
            }
        }
        gateway = FakeGateway(fixture)
        form_ids = la.list_lvli_form_ids(gateway, Path("dummy.esm"))
        self.assertEqual(form_ids, ["0x00000001", "0x00002222"])

    def test_no_lvli_records_returns_empty(self):
        fixture = {"records": {"0x00000001": {"record_type": "WEAP", "editor_id": "SomeGun"}}}
        gateway = FakeGateway(fixture)
        self.assertEqual(la.list_lvli_form_ids(gateway, Path("dummy.esm")), [])


if __name__ == "__main__":
    unittest.main()
