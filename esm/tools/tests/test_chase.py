#!/usr/bin/env python3
"""Tests for tools/chase/chase.py.

`chase.py` lives in a subdirectory (`tools/chase/`) with no `__init__.py`, so
it's loaded here via `importlib.util.spec_from_file_location` (as
`chase_script`) rather than a plain `import chase` -- avoids any ambiguity
with `tools/chase/` itself being visible as a namespace-package candidate
once `tools/` is on `sys.path` (which it must be, for `chase.py`'s own
`import esm_gateway as eg` to resolve).

Covers the library entry point, `chase()`, against a `FakeGateway` fixture
built to exercise all three OMOD-property patterns the chase pattern
classifies (see chase.py's module docstring):

  - `direct_property` with a bare numeric `Value 1` -- no further chase.
  - `perk_grant` (`Value 1` is a PERK) -- forward `bulk_get`, evidence is the
    granted PERK's own Description/Effects.
  - `keyword_hook` (`Value 1` is a KYWD) -- reverse `refs(type_filter=...,
    paths=True)` to find the gating SPEL/PERK, then a `bulk_get` + the
    `field_paths`-sliced `Effects[N]` entry.

Also covers: non-OMOD input is rejected, an unresolvable selector surfaces
as `ChaseError`, and `main()`'s CLI wiring (`find_esm_binary`/`ensure_daemon`
construction) via monkeypatching -- all with `FakeGateway`, no real `esm`
binary or daemon involved. `chase.py`'s own domain rendering (condition/
effect summarization, FormID-stub detection) is exercised incidentally by
these fixtures but is NOT the target of this file -- it's out of scope for
the transport-only refactor this file was added alongside (see esm/CLAUDE.md's
patch-notes pipeline notes).
"""

from __future__ import annotations

import importlib.util
import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

TESTS_DIR = Path(__file__).resolve().parent
TOOLS_DIR = TESTS_DIR.parent
CHASE_PATH = TOOLS_DIR / "chase" / "chase.py"

sys.path.insert(0, str(TOOLS_DIR))

import esm_gateway  # noqa: E402

_spec = importlib.util.spec_from_file_location("chase_script", CHASE_PATH)
chase_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chase_mod)


# ---------------------------------------------------------------------------
# Fixture: one synthetic OMOD exercising all three property patterns
# ---------------------------------------------------------------------------
#
# Field shapes below (Property/Function Type/Value 1/Value 2, the FormID-stub
# dict, Effects[]/Conditions nesting) mirror real `esm -p get --resolve stub`
# output verified against a live ESM (see mod_Custom_AllRise, 0x0047187E).

OMOD_FID = "0x00500000"
PERK_FID = "0x00500020"
KYWD_FID = "0x00500010"
SPEL_FID = "0x00500030"
WEAP_FID = "0x00500099"  # non-OMOD selector, for the rejection test


def _fixture():
    return {
        "records": {
            OMOD_FID: {
                "record_type": "OMOD",
                "editor_id": "mod_Custom_Test",
                "header": {"form_id": OMOD_FID, "signature": "OMOD"},
                "fields": {
                    "_record_type": "Object Modification",
                    "Editor ID": "mod_Custom_Test",
                    "Name": "Test Unique Mod",
                    "Description": "Grants a unique effect.",
                    "Data": {
                        "Properties": [
                            {
                                "Property": {"value": 1, "name": "SomeStat"},
                                "Function Type": {"value": 2, "name": "Multiply"},
                                "Value 1": 1.5,
                                "Value 2": 0,
                                "Curve Table": None,
                            },
                            {
                                "Property": {"value": 116, "name": "Perks"},
                                "Function Type": {"value": 2, "name": "ADD"},
                                "Value 1": {
                                    "formid": PERK_FID,
                                    "editor_id": "TestGrantedPerk",
                                    "record_type": "PERK",
                                },
                                "Value 2": 0,
                                "Curve Table": None,
                            },
                            {
                                "Property": {"value": 31, "name": "Keywords"},
                                "Function Type": {"value": 2, "name": "ADD"},
                                "Value 1": {
                                    "formid": KYWD_FID,
                                    "editor_id": "if_tmp_TestTag",
                                    "record_type": "KYWD",
                                },
                                "Value 2": 0,
                                "Curve Table": None,
                            },
                        ]
                    },
                },
            },
            PERK_FID: {
                "record_type": "PERK",
                "editor_id": "TestGrantedPerk",
                "header": {"form_id": PERK_FID, "signature": "PERK"},
                "fields": {
                    "Description": "Grants bonus damage.",
                    "Effects": [
                        {
                            "Effect": {
                                "Base Effect": {
                                    "formid": "0x00500021",
                                    "editor_id": "TestPerkEffect",
                                },
                                "Effect Item Data": {"Magnitude": 10},
                            }
                        }
                    ],
                },
            },
            SPEL_FID: {
                "record_type": "SPEL",
                "editor_id": "TestGatedSpell",
                "header": {"form_id": SPEL_FID, "signature": "SPEL"},
                "fields": {
                    "Effects": [
                        {
                            "Effect": {
                                "Base Effect": {
                                    "formid": "0x00500031",
                                    "editor_id": "TestSpellEffect",
                                },
                                "Conditions": {
                                    "Conditions": [
                                        {
                                            "Function": "WornHasKeyword",
                                            "Operator": "EqualTo",
                                            "Comparison Value": 1.0,
                                            "Parameter 1": {
                                                "formid": KYWD_FID,
                                                "editor_id": "if_tmp_TestTag",
                                            },
                                        }
                                    ]
                                },
                                "Effect Item Data": {"Magnitude": 25},
                            }
                        }
                    ]
                },
            },
            WEAP_FID: {
                "record_type": "WEAP",
                "editor_id": "NotAnOmod",
                "header": {"form_id": WEAP_FID, "signature": "WEAP"},
                "fields": {"_record_type": "Weapon", "Editor ID": "NotAnOmod"},
            },
        },
        "refs": {
            # Direct (depth-1) referencers of the KYWD: the SPEL whose
            # Conditions test WornHasKeyword(this keyword). field_paths is
            # fixture-authored (see FakeGateway's docstring) -- it's what a
            # real `Database.formid_reference_paths` call would compute.
            KYWD_FID: [
                {
                    "form_id": SPEL_FID,
                    "record_type": "SPEL",
                    "editor_id": "TestGatedSpell",
                    "field_paths": ["Effects[0].Conditions.Conditions[0].Parameter 1"],
                },
            ],
        },
    }


def gateway():
    return esm_gateway.FakeGateway(_fixture())


# ---------------------------------------------------------------------------
# chase() -- the library entry point
# ---------------------------------------------------------------------------


class ChaseTests(unittest.TestCase):
    def setUp(self):
        self.gw = gateway()

    def test_omod_stub_fields(self):
        tree = chase_mod.chase(self.gw, "esm", OMOD_FID)
        self.assertEqual(
            tree["omod"],
            {
                "formid": OMOD_FID,
                "editor_id": "mod_Custom_Test",
                "name": "Test Unique Mod",
                "description": "Grants a unique effect.",
            },
        )

    def test_three_hops_classified_by_pattern(self):
        tree = chase_mod.chase(self.gw, "esm", OMOD_FID)
        hops = tree["hops"]
        self.assertEqual(len(hops), 3)
        self.assertEqual(hops[0]["kind"], "direct_property")
        self.assertEqual(hops[1]["kind"], "perk_grant")
        self.assertEqual(hops[2]["kind"], "keyword_hook")

    def test_direct_property_scalar_has_no_evidence(self):
        tree = chase_mod.chase(self.gw, "esm", OMOD_FID)
        hop = tree["hops"][0]
        self.assertEqual(hop["value1"], 1.5)
        self.assertNotIn("target", hop)
        self.assertEqual(hop["evidence"], [])

    def test_perk_grant_forward_evidence_via_bulk_get(self):
        tree = chase_mod.chase(self.gw, "esm", OMOD_FID)
        hop = tree["hops"][1]
        self.assertEqual(hop["target"]["formid"], PERK_FID)
        evidence = hop["evidence"]
        self.assertEqual(len(evidence), 1)
        self.assertIsNone(evidence[0]["via"])
        self.assertEqual(evidence[0]["detail"]["description"], "Grants bonus damage.")
        self.assertEqual(len(evidence[0]["detail"]["effects"]), 1)

    def test_keyword_hook_reverse_evidence_slices_gated_effect(self):
        tree = chase_mod.chase(self.gw, "esm", OMOD_FID)
        hop = tree["hops"][2]
        self.assertEqual(hop["target"]["formid"], KYWD_FID)
        evidence = hop["evidence"]
        self.assertEqual(len(evidence), 1)
        item = evidence[0]
        self.assertEqual(item["source"]["formid"], SPEL_FID)
        self.assertEqual(item["via"], "Effects[0].Conditions.Conditions[0].Parameter 1")
        # The sliced evidence is the whole gated Effects[0] entry, not the
        # full SPEL record -- see chase.py's _slice_effect.
        self.assertEqual(
            item["detail"]["effect"]["Effect"]["Base Effect"]["editor_id"], "TestSpellEffect"
        )

    def test_keyword_hook_with_no_matching_consumer_is_a_dead_end(self):
        fixture = _fixture()
        fixture["refs"][KYWD_FID] = []  # no SPEL/PERK references this keyword
        tree = chase_mod.chase(esm_gateway.FakeGateway(fixture), "esm", OMOD_FID)
        self.assertEqual(tree["hops"][2]["evidence"], [])

    def test_non_omod_selector_raises_chase_error(self):
        with self.assertRaises(chase_mod.ChaseError) as ctx:
            chase_mod.chase(self.gw, "esm", WEAP_FID)
        self.assertIn("not an OMOD", str(ctx.exception))

    def test_unresolvable_selector_raises_chase_error(self):
        with self.assertRaises(chase_mod.ChaseError):
            chase_mod.chase(self.gw, "esm", "0xFFFFFFFF")

    def test_omod_with_no_properties_has_empty_hops(self):
        fixture = _fixture()
        fixture["records"][OMOD_FID]["fields"]["Data"]["Properties"] = []
        tree = chase_mod.chase(esm_gateway.FakeGateway(fixture), "esm", OMOD_FID)
        self.assertEqual(tree["hops"], [])


# ---------------------------------------------------------------------------
# render_text() -- light coverage only (rendering is out of scope for this
# transport refactor; this just confirms the transport-produced tree renders
# without raising and mentions the key facts).
# ---------------------------------------------------------------------------


class RenderTextTests(unittest.TestCase):
    def test_render_text_mentions_omod_and_hop_kinds(self):
        tree = chase_mod.chase(gateway(), "esm", OMOD_FID)
        text = chase_mod.render_text(tree)
        self.assertIn("mod_Custom_Test", text)
        self.assertIn("perk_grant", text)
        self.assertIn("keyword_hook", text)
        self.assertIn("TestSpellEffect", text)

    def test_render_text_no_properties_message(self):
        tree = {"omod": {"formid": OMOD_FID, "editor_id": "x", "name": None, "description": None}, "hops": []}
        text = chase_mod.render_text(tree)
        self.assertIn("nothing to chase", text)


# ---------------------------------------------------------------------------
# main() -- CLI wiring (find_esm_binary / ensure_daemon construction), with
# both monkeypatched to hand back a FakeGateway; no real esm binary/daemon.
# ---------------------------------------------------------------------------


class MainCliTests(unittest.TestCase):
    def _run_main(self, argv):
        with mock.patch.object(chase_mod.eg, "find_esm_binary", return_value=Path("/fake/esm")), \
             mock.patch.object(chase_mod.eg, "ensure_daemon", return_value=gateway()):
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc = chase_mod.main(argv)
            return rc, buf.getvalue()

    def test_main_json_output(self):
        rc, out = self._run_main(["--esm", "fake.esm", "--json", OMOD_FID])
        self.assertEqual(rc, 0)
        self.assertIn('"mod_Custom_Test"', out)

    def test_main_text_output(self):
        rc, out = self._run_main(["--esm", "fake.esm", OMOD_FID])
        self.assertEqual(rc, 0)
        self.assertIn("mod_Custom_Test", out)

    def test_main_requires_esm(self):
        # --esm defaults to $FO76_ESM_PATH -- explicitly clear it so this
        # test is deterministic regardless of the runner's environment.
        with mock.patch.dict(chase_mod.os.environ):
            chase_mod.os.environ.pop("FO76_ESM_PATH", None)
            rc, _out = self._run_main([OMOD_FID])
        self.assertEqual(rc, 2)

    def test_main_chase_error_returns_1(self):
        rc, _out = self._run_main(["--esm", "fake.esm", WEAP_FID])
        self.assertEqual(rc, 1)

    def test_main_closes_gateway_even_on_error(self):
        fake_gw = gateway()
        with mock.patch.object(chase_mod.eg, "find_esm_binary", return_value=Path("/fake/esm")), \
             mock.patch.object(chase_mod.eg, "ensure_daemon", return_value=fake_gw), \
             mock.patch.object(fake_gw, "close") as close_mock:
            chase_mod.main(["--esm", "fake.esm", WEAP_FID])
        close_mock.assert_called_once()


if __name__ == "__main__":
    unittest.main()
