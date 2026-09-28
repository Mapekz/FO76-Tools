"""Tests for pn/schemas.py: the agent-written artifacts' strict validators,
and that the skill's prompt examples match them."""

from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

from pn import schemas

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


class TestPromptExamplesMatchSchemas(unittest.TestCase):
    def test_writer_report_example(self):
        (example,) = json_blocks(SKILL_DIR / "deep-writer-prompt.md")
        schemas.validate_report(example)

    def test_review_example(self):
        (example,) = json_blocks(SKILL_DIR / "review-prompt.md")
        schemas.validate_review(example)


if __name__ == "__main__":
    unittest.main()
