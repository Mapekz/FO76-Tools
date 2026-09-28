"""Tests for pn/schemas.py: the agent-written artifacts' strict validators,
and that the skill's prompt examples match them."""

from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

from pn import schemas
from tests import builders

SKILL_DIR = Path(__file__).resolve().parents[1] / "skill"


def json_blocks(path: Path) -> list[object]:
    """Every ```json block in a markdown file, parsed."""
    text = path.read_text(encoding="utf-8")
    return [json.loads(block) for block in re.findall(r"```json\n(.*?)```", text, re.S)]


def report(**overrides):
    return {"bundles_covered": ["B0001"], "claims": [], **overrides}


class TestReport(unittest.TestCase):
    def test_minimal_report_is_valid(self):
        validated = schemas.validate_report(report())
        self.assertEqual(validated["deferred"], [])

    def test_unknown_key_is_rejected(self):
        with self.assertRaisesRegex(KeyError, "bundle_covered"):
            schemas.validate_report(report(bundle_covered=["B0002"]))

    def test_bundle_ids_must_look_like_bundle_ids(self):
        with self.assertRaisesRegex(ValueError, r"bundles_covered\[0\]"):
            schemas.validate_report(report(bundles_covered=["0x00012345"]))

    def test_claims_have_exactly_one_kind(self):
        ok = [
            {"record": "0x01", "path": "Data / Damage", "from": 1, "to": 2},
            {"record": "0x01", "status": "added"},
            {"record": "0x01", "path": "Data / Value", "value": 3, "side": "new"},
        ]
        schemas.validate_report(report(claims=ok))
        for bad in (
            {"record": "0x01", "path": "Data / Damage", "from": 1},
            {"record": "0x01", "status": "renamed"},
            {"record": "0x01", "path": "Data / Value", "value": 3, "side": "new", "to": 4},
        ):
            with self.subTest(bad=bad), self.assertRaises((KeyError, ValueError)):
                schemas.validate_report(report(claims=[bad]))


class TestOtherAgentArtifacts(unittest.TestCase):
    def test_cuts_need_a_reason(self):
        schemas.validate_cuts({"cuts": [{"bundle_id": "B0003", "reason": "over budget"}]})
        with self.assertRaises(ValueError):
            schemas.validate_cuts({"cuts": [{"bundle_id": "B0003", "reason": " "}]})

    def test_usage_tokens_are_ints(self):
        schemas.validate_usage({"assessor": {"tokens": 10}, "writers": [{"tokens": 20}]})
        with self.assertRaises(TypeError):
            schemas.validate_usage({"reviewer": {"tokens": 1.5}})

    def test_review_severity_is_one_of_three(self):
        with self.assertRaises(ValueError):
            schemas.validate_review(
                {"findings": [{"severity": "critical", "summary": "", "location": "", "evidence": ""}], "checked": {}}
            )


def array_payload(**overrides):
    return {
        "strategy": "keyed",
        "reorder_only": False,
        "key_fields": ["Keyword"],
        "count_from": 1,
        "count_to": 2,
        "added": [{"key_display": "Keyword=Foo", "display": "Foo", "raw": {"Keyword": "0x00000010"}}],
        "removed": [],
        "changed": [{"key_display": "Keyword=Bar", "changes": [builders.change()]}],
        **overrides,
    }


def lint(**overrides):
    return {
        "rule": "orphan",
        "severity": "warn",
        "form_id": "0x00000001",
        "message": "m",
        "data": {},
        "id": "L0001",
        "bundle_id": None,
        **overrides,
    }


def rollout_shape(**overrides):
    return {
        "record_type": "NPC_",
        "paths": ["Editor ID"],
        "record_count": 90,
        "example_form_ids": ["0x007CFA8C"],
        "numeric_excluded_count": 0,
        **overrides,
    }


def deep_bundle(**overrides):
    return {**builders.bundle(members=[builders.member()]), "bug_watch": False, "lint_ids": ["L0001"], **overrides}


class TestMechanicalArtifacts(unittest.TestCase):
    def test_well_formed_artifacts_are_valid(self):
        schemas.validate_change_entry(builders.change(kind="array", array=array_payload()))
        schemas.validate_triage({**builders.triage(), "rollout_shapes": [rollout_shape()]})
        schemas.validate_deep_slice({"bundles": [deep_bundle()], "lints": [lint()]})
        schemas.validate_lints_payload(
            {"meta": {"generated_at": "t", "rules_run": [], "counts": {}}, "lints": [lint(bundle_id="B0001")]}
        )
        schemas.validate_diff_payload(
            {"added": [{"form_id": "0x1"}], "removed": [], "changed": [{"stub": {"form_id": "0x2"}, "field_changes": {}}]}
        )

    def test_an_array_change_needs_a_whole_typed_array_edit(self):
        for bad in (
            {},
            array_payload(added=False),
            array_payload(removed=0),
            array_payload(changed=""),
            array_payload(strategy=8),
            array_payload(reorder_only="false"),
            array_payload(count_from={}),
            array_payload(key_fields=False),
            array_payload(added=[{}]),
            array_payload(changed=[{"key_display": "k", "changes": [{}]}]),
        ):
            with self.subTest(bad=bad), self.assertRaises((KeyError, TypeError, ValueError)):
                schemas.validate_change_entry(builders.change(kind="array", array=bad))

    def test_only_an_array_change_carries_an_array_edit(self):
        with self.assertRaises(TypeError):
            schemas.validate_change_entry(builders.change(kind="array"))
        with self.assertRaises(ValueError):
            schemas.validate_change_entry(builders.change(kind="scalar", array=array_payload()))

    def test_rollout_shapes_are_complete_and_typed(self):
        for bad in ({}, rollout_shape(record_count="90"), rollout_shape(paths="Editor ID"), rollout_shape(extra=1)):
            with self.subTest(bad=bad), self.assertRaises((KeyError, TypeError)):
                schemas.validate_triage({**builders.triage(), "rollout_shapes": [bad]})

    def test_deep_slice_bundles_and_lints_are_strict(self):
        for bad in (
            {"bundles": [deep_bundle(members=[builders.member(record_typo="MISC")])], "lints": []},
            {"bundles": [deep_bundle(lint_ids="L0001")], "lints": []},
            {"bundles": [deep_bundle(bug_watch="false")], "lints": []},
            {"bundles": [{k: v for k, v in deep_bundle().items() if k != "bug_watch"}], "lints": []},
            {"bundles": [], "lints": [False]},
            {"bundles": [], "lints": [lint(severity="fatal")]},
            {"bundles": [], "lints": [lint(id="1")]},
        ):
            with self.subTest(bad=bad), self.assertRaises((KeyError, TypeError, ValueError)):
                schemas.validate_deep_slice(bad)

    def test_lints_and_diff_payloads_need_their_structure(self):
        for bad in ({}, {"meta": {}, "lints": []}, {"meta": {"generated_at": "t", "rules_run": [], "counts": {}}}):
            with self.subTest(bad=bad), self.assertRaises((KeyError, TypeError)):
                schemas.validate_lints_payload(bad)
        for bad in (
            {},
            {"added": [], "removed": [], "changed": {}},
            {"added": [{}], "removed": [], "changed": []},
            {"added": [], "removed": [], "changed": [{"stub": {"form_id": "0x2"}}]},
        ):
            with self.subTest(bad=bad), self.assertRaises((KeyError, TypeError)):
                schemas.validate_diff_payload(bad)


class TestPromptExamplesMatchSchemas(unittest.TestCase):
    def test_writer_report_example(self):
        (example,) = json_blocks(SKILL_DIR / "deep-writer-prompt.md")
        schemas.validate_report(example)

    def test_review_example(self):
        (example,) = json_blocks(SKILL_DIR / "review-prompt.md")
        schemas.validate_review(example)


if __name__ == "__main__":
    unittest.main()
