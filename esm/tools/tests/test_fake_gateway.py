#!/usr/bin/env python3
"""Tests for tools/tests/fake_gateway.py.

Covers:
  - `FakeGateway`'s BFS reverse-reference walk against the checked-in
    `refs_graph.json` fixture (depth-1 vs depth-3 expansion, cycle safety,
    path/depth fields, int vs hex FormID acceptance, type_filter narrowing).
  - `paths=`/`bulk_get`/`list_type` against a small inline fixture.

Every test above uses only synthetic fixtures -- no real daemon or game
data. `FakeGatewayConformanceTests` at the bottom is the one exception: it
asserts `FakeGateway`'s Python reimplementation of the reverse-reference BFS
agrees with the REAL gateway/backend's own `ipc::referenced_by_enriched`
walk. Gated on `$FO76_ESM_PATH` (see esm/CLAUDE.local.md) exactly like
`test_esm_gateway.py`'s `RealEsmIntegrationTests` -- skips silently when
unset, so it is a no-op in CI/sandboxes without game data. This is the
actual drift guard for the ~250-line Python BFS reimplementation in
fake_gateway.py, previously guaranteed only by that class's own docstring.
"""

from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import esm_gateway  # noqa: E402
from esm_gateway import DaemonError, formid_to_hex  # noqa: E402
from fake_gateway import FakeGateway  # noqa: E402

FIXTURE_PATH = Path(__file__).resolve().parent / "fixtures" / "refs_graph.json"


# ─── FakeGateway BFS tests ────────────────────────────────────────────────────


class FakeGatewayRefsTests(unittest.TestCase):
    def setUp(self):
        self.client = FakeGateway(FIXTURE_PATH)
        self.WEAP = 0x00100001
        self.OMOD1 = 0x00100010
        self.OMOD2 = 0x00100011
        self.LVLI = 0x00100020
        self.NPC = 0x00100030
        self.CONT = 0x00100031
        self.COBJ = 0x00100040
        self.QUST = 0x00100060

    def _form_ids(self, rows) -> set:
        return {r["form_id"] for r in rows}

    def test_depth_1_returns_only_direct_referencers(self):
        result = self.client.refs("esm", self.WEAP, depth=1)
        self.assertEqual(result["target"], "0x00100001")
        self.assertEqual(result["total"], 4)
        self.assertEqual(
            self._form_ids(result["rows"]),
            {formid_to_hex(self.OMOD1), formid_to_hex(self.OMOD2), formid_to_hex(self.LVLI), formid_to_hex(self.COBJ)},
        )
        for r in result["rows"]:
            self.assertEqual(r["depth"], 1)
            self.assertNotIn("path", r)  # skip_serializing_if empty, mirrored

    def test_depth_2_adds_lvli_referencers(self):
        result = self.client.refs("esm", self.WEAP, depth=2)
        ids = self._form_ids(result["rows"])
        self.assertIn(formid_to_hex(self.NPC), ids)
        self.assertIn(formid_to_hex(self.CONT), ids)
        self.assertEqual(result["total"], 6)  # 4 depth-1 + NPC_ + CONT

        npc_row = next(r for r in result["rows"] if r["form_id"] == formid_to_hex(self.NPC))
        self.assertEqual(npc_row["depth"], 2)
        self.assertEqual(npc_row["path"], [{"form_id": formid_to_hex(self.LVLI), "record_type": "LVLI", "editor_id": "LVLI_TestList"}])

    def test_depth_3_adds_qust_via_cont(self):
        result = self.client.refs("esm", self.WEAP, depth=3)
        ids = self._form_ids(result["rows"])
        self.assertIn(formid_to_hex(self.QUST), ids)
        self.assertEqual(result["total"], 7)  # 6 from depth 2 + QUST

        qust_row = next(r for r in result["rows"] if r["form_id"] == formid_to_hex(self.QUST))
        self.assertEqual(qust_row["depth"], 3)
        self.assertEqual(
            qust_row["path"],
            [
                {"form_id": formid_to_hex(self.LVLI), "record_type": "LVLI", "editor_id": "LVLI_TestList"},
                {"form_id": formid_to_hex(self.CONT), "record_type": "CONT", "editor_id": "CONT_TestContainer"},
            ],
        )

    def test_depth_beyond_graph_size_is_a_noop_plateau(self):
        result_3 = self.client.refs("esm", self.WEAP, depth=3)
        result_6 = self.client.refs("esm", self.WEAP, depth=6)
        self.assertEqual(result_3["total"], result_6["total"])

    def test_cycle_safety_target_not_reemitted(self):
        # The fixture deliberately makes LVLI reference the WEAP back (in
        # addition to the WEAP being referenced by LVLI), i.e. WEAP <-> LVLI.
        # The target must never appear in its own result set, and the walk
        # must terminate rather than looping forever.
        result = self.client.refs("esm", self.WEAP, depth=6)
        ids = self._form_ids(result["rows"])
        self.assertNotIn(formid_to_hex(self.WEAP), ids)
        # Every form_id appears at most once.
        self.assertEqual(len(result["rows"]), len(ids))

    def test_cycle_safety_via_kywd_reaches_weap_referencers_once(self):
        kywd = 0x00100050  # if_tmp_WeaponMod, referenced only by WEAP
        result = self.client.refs("esm", kywd, depth=6)
        ids = [r["form_id"] for r in result["rows"]]
        # No duplicates even though the graph loops back toward the KYWD's
        # own referencer chain via WEAP <-> LVLI.
        self.assertEqual(len(ids), len(set(ids)))
        self.assertIn(formid_to_hex(self.WEAP), ids)

    def test_accepts_int_and_hex_string_formid_interchangeably(self):
        by_int = self.client.refs("esm", self.WEAP, depth=1)
        by_hex = self.client.refs("esm", "0x00100001", depth=1)
        self.assertEqual(by_int, by_hex)

    def test_hub_keyword_has_more_than_8_referencers(self):
        result = self.client.refs("esm", 0x00100090, depth=1)
        self.assertGreater(result["total"], 8)

    def test_orphan_perk_and_kywd_have_no_referencers(self):
        self.assertEqual(self.client.refs("esm", 0x00100072, depth=6)["total"], 0)  # PERK2
        self.assertEqual(self.client.refs("esm", 0x00100080, depth=6)["total"], 0)  # OrphanKeyword

    def test_perk_with_pcrd_referencer(self):
        result = self.client.refs("esm", 0x00100070, depth=1)
        self.assertEqual(result["total"], 1)
        self.assertEqual(result["rows"][0]["record_type"], "PCRD")

    def test_limit_caps_and_sets_capped_flag(self):
        result = self.client.refs("esm", self.WEAP, depth=3, limit=2)
        self.assertEqual(len(result["rows"]), 2)
        self.assertTrue(result["capped"])
        self.assertEqual(result["total"], 7)

    def test_record_lookup_by_formid_and_edid(self):
        rec = self.client.record("esm", self.WEAP)
        self.assertEqual(rec["editor_id"], "WEAP_TestRifle")
        rec2 = self.client.record_by_edid("esm", "WEAP_TestRifle")
        self.assertEqual(rec, rec2)

    def test_record_not_found_raises(self):
        with self.assertRaises(DaemonError):
            self.client.record("esm", 0xFFFFFFFF)
        with self.assertRaises(DaemonError):
            self.client.record_by_edid("esm", "NoSuchEditorId")

    def test_exists(self):
        self.assertTrue(self.client.exists("esm", self.WEAP))
        self.assertFalse(self.client.exists("esm", 0xFFFFFFFF))

    def test_generic_op_matches_convenience_method(self):
        via_op = self.client.op(
            "esm",
            {"op": "referenced_by", "sel": {"kind": "form_id", "value": self.WEAP}, "limit": 0, "depth": 2},
        )
        via_method = self.client.refs("esm", self.WEAP, depth=2)
        self.assertEqual(via_op, via_method)

    def test_type_filter_narrows_emission_but_keeps_traversal(self):
        # OMOD1/OMOD2/LVLI/COBJ are the direct (depth-1) referencers, none of
        # them NPC_ -- type_filter must drop all four from the emitted rows
        # while still traversing through LVLI to reach the depth-2 NPC_.
        result = self.client.refs("esm", self.WEAP, depth=2, type_filter="NPC_")
        self.assertEqual(self._form_ids(result["rows"]), {formid_to_hex(self.NPC)})
        self.assertEqual(result["total"], 1)

    def test_type_filter_is_case_insensitive(self):
        result = self.client.refs("esm", self.WEAP, depth=1, type_filter="omod")
        self.assertEqual(
            self._form_ids(result["rows"]), {formid_to_hex(self.OMOD1), formid_to_hex(self.OMOD2)}
        )

    def test_list_type_returns_records_of_that_type_sorted_by_formid(self):
        result = self.client.list_type("esm", "OMOD")
        self.assertEqual([r["form_id"] for r in result], [formid_to_hex(self.OMOD1), formid_to_hex(self.OMOD2)])
        for r in result:
            self.assertEqual(r["record_type"], "OMOD")

    def test_list_type_is_case_insensitive_and_respects_limit(self):
        result = self.client.list_type("esm", "omod", limit=1)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["form_id"], formid_to_hex(self.OMOD1))

    def test_list_type_unknown_type_returns_empty(self):
        self.assertEqual(self.client.list_type("esm", "ZZZZ"), [])

    def test_list_type_via_generic_op_matches_convenience_method(self):
        via_op = self.client.op("esm", {"op": "list_type_records", "sig": "OMOD", "offset": 0, "limit": 0})
        via_method = self.client.list_type("esm", "OMOD")
        self.assertEqual(via_op, via_method)


# ─── FakeGateway paths= / bulk_get tests (inline fixture) ───────────────────


class FakeGatewayPathsAndBulkGetTests(unittest.TestCase):
    """Uses a small inline fixture (rather than the shared refs_graph.json)
    because it needs a `fields` payload on records and a `field_paths` entry
    on an adjacency row -- extensions to the fixture schema that the other
    shared-fixture tests above don't exercise."""

    def setUp(self):
        self.client = FakeGateway(
            {
                "records": {
                    "0x00000010": {"record_type": "KYWD", "editor_id": "if_tmp_Test"},
                    "0x00000020": {
                        "record_type": "SPEL",
                        "editor_id": "TestSpell",
                        "fields": {"Effects": [{"Effect": {"Magnitude": 5}}]},
                    },
                    "0x00000030": {
                        "record_type": "OMOD",
                        "editor_id": "mod_Custom_Test",
                        "fields": {"Data": {"Properties": []}},
                    },
                },
                "refs": {
                    "0x00000010": [
                        {
                            "form_id": "0x00000020",
                            "record_type": "SPEL",
                            "editor_id": "TestSpell",
                            "field_paths": ["Effects[0].Conditions.Conditions[0].Parameter 1"],
                        },
                    ],
                },
            }
        )

    def test_paths_true_passes_through_fixture_field_paths(self):
        result = self.client.refs("esm", 0x10, depth=1, paths=True)
        self.assertEqual(
            result["rows"][0]["field_paths"],
            ["Effects[0].Conditions.Conditions[0].Parameter 1"],
        )

    def test_paths_false_omits_field_paths_key(self):
        result = self.client.refs("esm", 0x10, depth=1)
        self.assertNotIn("field_paths", result["rows"][0])

    def test_bulk_get_isolates_errors_per_selector(self):
        entries = self.client.bulk_get("esm", [0x20, 0xFFFFFFFF, "mod_Custom_Test"])
        self.assertEqual(entries[0], {
            "sel": "0x00000020",
            "header": None,
            "editor_id": "TestSpell",
            "fields": {"Effects": [{"Effect": {"Magnitude": 5}}]},
        })
        self.assertEqual(entries[1]["sel"], "0xFFFFFFFF")
        self.assertIn("error", entries[1])
        # EditorID selectors display as the literal input text (mirrors
        # RecordSel::display() in ipc.rs), not the resolved FormID.
        self.assertEqual(entries[2]["sel"], "mod_Custom_Test")
        self.assertEqual(entries[2]["fields"], {"Data": {"Properties": []}})

    def test_bulk_get_via_generic_op_matches_convenience_method(self):
        via_op = self.client.op(
            "esm", {"op": "record_bulk", "sels": [{"kind": "form_id", "value": 0x20}]}
        )
        via_method = self.client.bulk_get("esm", [0x20])
        self.assertEqual(via_op, via_method)

    def test_bulk_get_empty_list_returns_empty_list(self):
        self.assertEqual(self.client.bulk_get("esm", []), [])


# ─── Conformance: FakeGateway's BFS vs the real gateway's BFS ──────────────


def _live_fixture_from_gateway(gateway, esm_path: str, target_hex: str, *, rounds: int) -> dict:
    """Build a `refs_graph.json`-shaped fixture purely from independent
    depth-1 (`refs(..., depth=1)`) calls against the REAL gateway, expanding
    breadth-first from `target_hex` for `rounds` rounds. Each depth-1 call is
    a flat scan, not a traversal -- the real BFS is never invoked here -- so
    this is ground truth the conformance test below can check the REAL
    multi-hop BFS result against a SEPARATE, independently-computed
    `FakeGateway` multi-hop walk over the same adjacency facts."""
    records: dict[str, dict] = {}
    refs_adj: dict[str, list[dict]] = {}
    frontier = {target_hex}
    visited = {target_hex}
    for _ in range(rounds):
        next_frontier: set[str] = set()
        for node in sorted(frontier):
            result = gateway.refs(esm_path, node, depth=1, limit=0)
            rows = result["rows"]
            refs_adj[node] = [
                {
                    "form_id": r["form_id"],
                    "record_type": r.get("record_type"),
                    "editor_id": r.get("editor_id"),
                    "name": r.get("name"),
                    "offset": r.get("offset", 0),
                }
                for r in rows
            ]
            for r in rows:
                fid = r["form_id"]
                records.setdefault(
                    fid,
                    {
                        "record_type": r.get("record_type"),
                        "editor_id": r.get("editor_id"),
                        "name": r.get("name"),
                    },
                )
                if fid not in visited:
                    visited.add(fid)
                    next_frontier.add(fid)
        frontier = next_frontier
        if not frontier:
            break
    return {"records": records, "refs": refs_adj}


class FakeGatewayConformanceTests(unittest.TestCase):
    """Asserts `FakeGateway.refs()`'s Python BFS reimplementation agrees
    with the REAL gateway/backend's own `ipc::referenced_by_enriched` walk.

    Gated on `$FO76_ESM_PATH`, mirroring `test_esm_gateway.py`'s
    `RealEsmIntegrationTests` silent-skip convention -- a no-op in
    CI/sandboxes without real game data. This is intentionally LOCAL-ONLY:
    there is no committed real `.esm` the real gateway could run against
    (game data is gitignored/non-redistributable), so exercising the real
    BFS requires a real ESM on disk, same as every other real-ESM test in
    this suite.

    The checked-in `refs_graph.json` fixture can't be replayed against a
    real ESM directly -- its FormIDs are synthetic and don't resolve against
    any real snapshot. Instead this builds a fixture of the SAME SHAPE from
    genuinely independent depth-1 queries against the real gateway (see
    `_live_fixture_from_gateway`) and asserts `FakeGateway`'s own multi-hop
    walk over that fixture reproduces the SAME multi-hop result the real
    gateway computes independently over the live ESM. Agreement is a real
    conformance signal for the traversal logic itself (visited-once dedup,
    depth accumulation, `path` recording, ascending-FormID sort) -- not a
    tautology, since each side's multi-hop answer is computed by a
    different implementation from the same underlying one-hop facts.

    Targets an OMOD (not a "hub" KYWD) deliberately: an OMOD's own direct
    referencers are typically 0-2 WEAP/ARMO records (see
    `test_esm_gateway.py`'s `test_refs_with_type_filter_and_paths_matches_a_
    real_omod_keyword`), which keeps the number of depth-1 probes
    `_live_fixture_from_gateway` needs to build a 2-round fixture small and
    bounded regardless of ESM size.

    One deliberate exception to "same result": when a node is reachable via
    *two or more* same-depth predecessors (e.g. two sibling WEAP variants
    both referencing the same LVLI), which predecessor "wins" the emitted
    `path` is an artifact of `db.referenced_by`'s internal xref-storage
    order on the real side -- not a documented contract, and not something
    `_live_fixture_from_gateway` can observe (its own depth-1 probes come
    back FormID-sorted, a different order than the real BFS's internal,
    unsorted expansion order). `_assert_rows_equivalent` below checks every
    row's identity/depth/record_type/editor_id/name/offset exactly and only
    checks `path` *length* (== depth - 1), not the specific predecessor
    chain, to avoid asserting on that unspecified tie-break.
    """

    esm_path: str
    gateway: esm_gateway.EsmGateway
    target: str

    @classmethod
    def setUpClass(cls):
        esm_path = os.environ.get("FO76_ESM_PATH")
        if not esm_path or not Path(esm_path).is_file():
            raise unittest.SkipTest(
                "FO76_ESM_PATH not set (or not a file) -- skipping real-ESM conformance test"
            )
        try:
            esm_bin = esm_gateway.find_esm_binary(None)
        except DaemonError as exc:
            raise unittest.SkipTest(f"esm binary not found -- skipping: {exc}")
        cls.esm_path = esm_path
        cls.gateway = esm_gateway.ensure_daemon(esm_bin, esm_path)

        target = None
        for stub in cls.gateway.search(esm_path, "*", record_type="OMOD", limit=40):
            result = cls.gateway.refs(esm_path, stub["form_id"], depth=1, limit=0)
            if result["total"] > 0:
                target = stub["form_id"]
                break
        if target is None:
            raise unittest.SkipTest(
                "no OMOD in this ESM has any direct referencer -- cannot build a conformance fixture"
            )
        cls.target = target

    @classmethod
    def tearDownClass(cls):
        gateway = getattr(cls, "gateway", None)
        if gateway is not None:
            gateway.close()

    def _assert_rows_equivalent(self, fake_result: dict, real_result: dict, *, depth: int):
        for key in (
            "target", "total", "capped", "requested_depth", "effective_depth",
            "depth_capped", "frontier_remaining", "per_depth_totals", "shown_max_depth",
        ):
            self.assertEqual(fake_result[key], real_result[key], f"{key} mismatch at depth={depth}")

        fake_by_fid = {r["form_id"]: r for r in fake_result["rows"]}
        real_by_fid = {r["form_id"]: r for r in real_result["rows"]}
        self.assertEqual(
            set(fake_by_fid), set(real_by_fid), f"row form_id set mismatch at depth={depth}"
        )
        for fid, real_row in real_by_fid.items():
            fake_row = fake_by_fid[fid]
            for key in ("record_type", "editor_id", "name", "offset", "depth"):
                self.assertEqual(
                    fake_row.get(key), real_row.get(key), f"{fid}.{key} mismatch at depth={depth}"
                )
            # Path LENGTH (== depth - 1), not the specific predecessor chain --
            # see this class's docstring for why the exact chain is excluded.
            self.assertEqual(
                len(fake_row.get("path", [])),
                len(real_row.get("path", [])),
                f"{fid}.path length mismatch at depth={depth}",
            )

    def test_multi_hop_bfs_matches_real_gateway(self):
        fixture = _live_fixture_from_gateway(self.gateway, self.esm_path, self.target, rounds=2)
        fake = FakeGateway(fixture)
        for depth in (1, 2):
            real_result = self.gateway.refs(self.esm_path, self.target, depth=depth, limit=0)
            fake_result = fake.refs(self.esm_path, self.target, depth=depth, limit=0)
            self._assert_rows_equivalent(fake_result, real_result, depth=depth)


if __name__ == "__main__":
    unittest.main()
