#!/usr/bin/env python3
"""
build_bundles.py — Tool 2 of the FO76 patch-notes pipeline.

Consumes `comprehensive.json` (Tool 1, `render_comprehensive.py`'s output —
see `build_comprehensive()` there for the authoritative record shape) and
clusters the flat per-FormID diff records into narrative "bundles": groups of
related records (e.g. a weapon + its mod slots + the leveled list that drops
it + the keyword that marks it "unique") that a human patch-notes writer (or
an LLM writer subagent, see `slice_bundles.py`) should describe together
rather than as N disconnected bullet points.

Pipeline position: render_comprehensive.py -> **build_bundles.py** -> a lint
tool (fills `bug_watch`/`lint_ids`/`lints`, not built here) -> slice_bundles.py
-> per-category writer subagents.

Algorithm (see module docstring sections below for each step):
  1. Universe = diff records minus WRLD/CELL (excluded upstream already, but
     re-filtered defensively).
  2. Forward edges from each record's own `refs_out`.
  3. Reverse edges from `client.refs()` (a depth-bounded BFS over the live
     ESM reference graph) for every record in the universe.
  4. Union-find over the universe, with a "hub" exemption: very
     highly-referenced nodes (KYWD, common containers, ...) never force
     unrelated records into one giant bundle.
  5. Component -> bundle: anchor selection, oversized-component splitting,
     same-anchor / high-overlap bundle merging.
  6. Context-member attachment (nodes outside the diff that the bundle
     references or is referenced by), capped and preference-ordered.
  7. Categorization against `patch_notes_categories.json`.
  8. Deterministic sort + ID assignment + meta counts.

`build_bundles(comp, client, old_esm, new_esm, config) -> dict` is the
library entry point; `main()` is a thin CLI wrapper. `client` is anything
implementing `esm_daemon.DaemonClient`'s `refs()`/`record()` surface —
normally a live warm-daemon `DaemonClient` (see `esm_daemon.ensure_daemon`),
or `esm_daemon.FakeClient` for `--offline` / tests.

Python 3, stdlib only.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import shutil
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

import esm_daemon

# --------------------------------------------------------------------------
# Constants / tunables (mirror patch_notes_categories.json's "settings")
# --------------------------------------------------------------------------

DEFAULT_SETTINGS = {
    "hub_degree": 8,
    "max_members": 40,
    "refs_depth": 2,
    "context_cap": 12,
    "unique_keyword_patterns": ["if_tmp_*"],
}

# Record types never meaningfully decoded / already excluded upstream.
EXCLUDED_TYPES = {"WRLD", "CELL"}

# Types whose "true" bundle-mates are typically >1 direct hop away (e.g. a
# KYWD's actual family is reached via the item that carries it, then that
# item's container/NPC) -- these get base_depth + 1 for the reverse BFS.
SPECIAL_DEPTH_TYPES = {"KYWD", "LVLI", "OMOD", "ENCH", "MGEF"}

# Anchor selection priority (index 0 = highest priority). Unlisted types
# rank below every listed type.
ANCHOR_PRIORITY = [
    "QUST", "NPC_", "WEAP", "ARMO", "COBJ", "ALCH", "PERK", "PCRD",
    "AVTR", "CHAL", "LVLI", "OMOD", "ENCH", "SPEL", "MGEF", "MISC", "KYWD", "GLOB",
]
_ANCHOR_RANK = {t: i for i, t in enumerate(ANCHOR_PRIORITY)}
_UNLISTED_RANK = len(ANCHOR_PRIORITY)

# added > changed > removed, per the anchor tie-break rule.
_STATUS_WEIGHT = {"added": 2, "changed": 1, "removed": 0}

# Context-member attachment preference (drop-source relevance), in rank
# order; anything else (besides a unique-keyword-pattern KYWD, ranked just
# after these) sorts last. A candidate connected to a bundle member via one
# of CONTEXT_TOP_TIER_RELATIONS outranks all of these -- see _context_rank.
CONTEXT_PREFERRED_TYPES = ["NPC_", "CONT", "QUST", "COBJ"]

# Edge relations whose target is the single most story-relevant context node
# for this bundle -- an OMOD's own weapon/armor ("mod_for"), or a COBJ's
# crafted item ("crafts") -- and so rank above even NPC_/CONT/QUST/COBJ in
# attach_context's preference order (see _context_rank).
CONTEXT_TOP_TIER_RELATIONS = {"mod_for", "crafts"}

# Bundle-merge overlap threshold (non-context member Jaccard-ish ratio, see
# `merge_by_overlap`).
OVERLAP_MERGE_THRESHOLD = 0.6


def _iso_now():
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def _int_fid(fid):
    """Parse a '0x...'-or-decimal FormID string to an int; 0 on anything
    unparsable (defensive -- never raises on malformed input)."""
    if isinstance(fid, int):
        return fid
    try:
        s = str(fid).strip()
        return int(s, 16) if s.lower().startswith("0x") else int(s)
    except (TypeError, ValueError):
        return 0


def _priority_rank(record_type):
    return _ANCHOR_RANK.get(record_type, _UNLISTED_RANK)


# --------------------------------------------------------------------------
# Edge relation / label mapping (ordered rule table)
# --------------------------------------------------------------------------
#
# Each rule: (src_types: set|None, dst_types: set|None, path_substrings:
# list|None, relation: str). `None` means "match any". `path_substrings`
# rules can only match when a path string is available -- in practice this
# means forward edges only (reverse edges carry `via`, a list of
# intermediate-hop FormIDs, not a source-record field path, so a COBJ
# reverse edge with no path falls through to the "references" fallback
# rather than misidentifying which of crafts/crafted_from/crafted_at it is).
#
# `relation` is always one of the fixed wire-schema values. Its human
# `label` is derived by replacing '_' with ' ', EXCEPT "contains", which is
# direction-aware: forward discovery (the LVLI's own refs_out lists the
# item) reads as "contains"; reverse discovery (found while walking the
# item's referencers) reads as "dropped via" -- same relation, different
# phrasing, since the wire schema's relation enum has no separate
# "dropped_via" value.
#
# "mod_for" is deliberately listed in BOTH directions (OMOD -> WEAP/ARMO and
# WEAP/ARMO -> OMOD): a WEAP/ARMO's own data forward-references its
# compatible OMODs (e.g. via its Object Template), so when the OMOD is the
# one in the diff universe and the WEAP/ARMO isn't, `client.refs()` on the
# OMOD surfaces the WEAP/ARMO as the referencer -- i.e. `from`=WEAP/ARMO,
# `to`=OMOD (verified against a live ESM: refs(mod_Custom_SaltOfTheEarth)
# returns DoubleBarrelShotgun). Both directions describe the same real-world
# relationship and both map to the same relation/label ("mod for").

_EDGE_RULES = [
    ({"LVLI"}, None, None, "contains"),
    ({"NPC_"}, None, None, "carried_by"),
    ({"CONT"}, None, None, "found_in"),
    ({"OMOD"}, {"WEAP", "ARMO"}, None, "mod_for"),
    ({"WEAP", "ARMO"}, {"OMOD"}, None, "mod_for"),
    ({"COBJ"}, None, ["Created Object"], "crafts"),
    ({"COBJ"}, None, ["Components", "Component"], "crafted_from"),
    ({"COBJ"}, None, ["Workbench Keyword"], "crafted_at"),
    ({"ENCH", "SPEL", "MGEF"}, None, None, "grants_effect"),
    ({"QUST"}, None, None, "rewarded_by"),
    ({"PCRD"}, {"PERK"}, None, "card_for"),
]


def edge_relation_and_label(src_type, dst_type, path, source):
    """Look up (relation, label) for an edge from a record of `src_type` to
    a record of `dst_type`, optionally guided by `path` (the source
    record's field path to the reference, forward edges only -- None for
    reverse edges) and `source` ("forward"|"reverse", used only to pick the
    direction-aware "contains"/"dropped via" phrasing). Never raises; falls
    back to ("references", "references") when nothing matches."""
    for src_types, dst_types, path_substrings, relation in _EDGE_RULES:
        if src_types is not None and src_type not in src_types:
            continue
        if dst_types is not None and dst_type not in dst_types:
            continue
        if path_substrings is not None:
            if not path or not any(sub in path for sub in path_substrings):
                continue
        if relation == "contains":
            label = "contains" if source == "forward" else "dropped via"
        else:
            label = relation.replace("_", " ")
        return relation, label
    return "references", "references"


# --------------------------------------------------------------------------
# Step 1: universe
# --------------------------------------------------------------------------


def build_universe(comp):
    """Return {form_id: record} for every comprehensive.json record whose
    record_type isn't in EXCLUDED_TYPES (defensive re-filter; upstream
    already excludes these, see render_comprehensive.py)."""
    records = comp.get("records") or {}
    return {
        fid: rec
        for fid, rec in records.items()
        if (rec or {}).get("record_type") not in EXCLUDED_TYPES
    }


# --------------------------------------------------------------------------
# Steps 2-3: forward / reverse edge gathering
# --------------------------------------------------------------------------


def _ref_names_stub(fid, ref_names):
    info = (ref_names or {}).get(fid) or {}
    return {
        "form_id": fid,
        "record_type": info.get("record_type"),
        "editor_id": info.get("editor_id"),
        "name": info.get("name"),
    }


def gather_forward_edges(u, ref_names):
    """Step 2. Walk each U record's own `refs_out`. A target inside U
    becomes a forward edge; a target outside U becomes a context-node
    candidate (resolved via `ref_names`, since a bare refs_out entry has no
    record_type/editor_id/name of its own)."""
    edges = []
    context = {}
    for fid, rec in u.items():
        for ref in (rec or {}).get("refs_out") or []:
            tgt = ref.get("formid")
            if not tgt or tgt == fid:
                continue
            edges.append({"from": fid, "to": tgt, "path": ref.get("path"), "via": [], "source": "forward"})
            if tgt not in u:
                context.setdefault(tgt, _ref_names_stub(tgt, ref_names))
    return edges, context


def gather_reverse_edges(u, client, old_esm, new_esm, refs_depth):
    """Step 3. For every fid in U, call `client.refs()` (a depth-bounded
    reverse BFS over the live ESM graph). Every returned row becomes an
    edge `row.form_id -> fid` regardless of the row's own hop depth --
    `via` carries the intermediate hop chain -- which is what lets a
    multi-hop type (KYWD/LVLI/OMOD/ENCH/MGEF, `SPECIAL_DEPTH_TYPES`) reach
    its true bundle-mates through one extra hop. `removed`-status records
    only exist pre-patch, so they're queried against `old_esm`; everything
    else against `new_esm`. A daemon error for one record is skipped rather
    than aborting the whole run (never panics on missing/unreachable data)."""
    edges = []
    context = {}
    for fid, rec in u.items():
        record_type = (rec or {}).get("record_type")
        depth = refs_depth + (1 if record_type in SPECIAL_DEPTH_TYPES else 0)
        esm_path = old_esm if (rec or {}).get("status") == "removed" else new_esm
        if not esm_path:
            continue
        try:
            result = client.refs(esm_path, fid, depth=depth, limit=0)
        except esm_daemon.DaemonError:
            continue
        for row in (result or {}).get("rows") or []:
            rf = row.get("form_id")
            if not rf or rf == fid:
                continue
            via = [p.get("form_id") for p in (row.get("path") or []) if p.get("form_id")]
            edges.append({"from": rf, "to": fid, "path": None, "via": via, "source": "reverse"})
            if rf not in u:
                context.setdefault(
                    rf,
                    {
                        "form_id": rf,
                        "record_type": row.get("record_type"),
                        "editor_id": row.get("editor_id"),
                        "name": row.get("name"),
                    },
                )
    return edges, context


def dedupe_edges(edges):
    """Keep the first edge seen per (from, to) pair. Callers pass forward
    edges before reverse edges so that a link discovered both ways (common,
    since forward-refs_out and the live reverse graph describe the same
    underlying reference) keeps the forward edge's more precise path-based
    relation/label rather than the reverse edge's path-less fallback."""
    seen = set()
    out = []
    for e in edges:
        key = (e["from"], e["to"])
        if key in seen:
            continue
        seen.add(key)
        out.append(e)
    return out


def _record_type_of(fid, u, *context_maps):
    if fid in u:
        return (u[fid] or {}).get("record_type")
    for cm in context_maps:
        if fid in cm:
            return cm[fid].get("record_type")
    return None


def build_edges(u, ref_names, client, old_esm, new_esm, refs_depth):
    """Steps 2+3 combined: gather forward + reverse edges, attach
    relation/label, and dedupe. Returns (edges, context_stubs)."""
    fwd_edges, fwd_context = gather_forward_edges(u, ref_names)
    rev_edges, rev_context = gather_reverse_edges(u, client, old_esm, new_esm, refs_depth)

    context_stubs = {}
    context_stubs.update(rev_context)
    context_stubs.update(fwd_context)  # forward wins if both discovered the same node

    edges = []
    for e in fwd_edges + rev_edges:
        src_type = _record_type_of(e["from"], u, fwd_context, rev_context)
        dst_type = _record_type_of(e["to"], u, fwd_context, rev_context)
        relation, label = edge_relation_and_label(src_type, dst_type, e.get("path"), e["source"])
        edges.append(
            {
                "from": e["from"],
                "to": e["to"],
                "relation": relation,
                "label": label,
                "via": e.get("via") or [],
                "source": e["source"],
            }
        )

    return dedupe_edges(edges), context_stubs


# --------------------------------------------------------------------------
# Degree computation
# --------------------------------------------------------------------------


def compute_degrees(u, edges):
    """full_degree[fid]: count of distinct neighbors (any edge, forward or
    reverse, U or context side) -- used for the "is this U node itself a
    hub" check. context_u_degree[fid]: for a context (non-U) node, count of
    distinct U neighbors specifically -- used for the "does this context
    node touch too many unrelated U records to safely bridge them" check
    (these are deliberately different notions of degree, see
    `union_find`)."""
    neighbors = defaultdict(set)
    for e in edges:
        a, b = e["from"], e["to"]
        neighbors[a].add(b)
        neighbors[b].add(a)

    full_degree = {fid: len(ns) for fid, ns in neighbors.items()}
    context_u_degree = {
        fid: sum(1 for n in ns if n in u) for fid, ns in neighbors.items() if fid not in u
    }
    return full_degree, context_u_degree


# --------------------------------------------------------------------------
# Step 4: union-find
# --------------------------------------------------------------------------


class DSU:
    """Plain union-find over an arbitrary hashable universe."""

    def __init__(self, items):
        self.parent = {x: x for x in items}

    def find(self, x):
        root = x
        while self.parent[root] != root:
            root = self.parent[root]
        while self.parent[x] != root:
            self.parent[x], x = root, self.parent[x]
        return root

    def union(self, a, b):
        ra, rb = self.find(a), self.find(b)
        if ra == rb:
            return
        # Deterministic: smaller key wins as the surviving root, so the
        # final partition doesn't depend on edge iteration order.
        if ra < rb:
            self.parent[rb] = ra
        else:
            self.parent[ra] = rb


def union_find(u, edges, full_degree, context_u_degree, hub_degree):
    """Step 4. Phase A: union both endpoints of any U<->U edge, UNLESS
    either endpoint is itself a hub (full_degree > hub_degree) -- a hub
    node never causes a union; it (or, if it's a context node, see Phase B)
    attaches as a member to every bundle it touches instead, without
    merging those bundles together. Phase B: for each context node
    touching >=2 distinct U records, union those U records' (post-Phase-A)
    roots together, UNLESS the context node is itself a hub (measured by
    `context_u_degree`: how many *distinct U records* it touches -- a
    highly-connected KYWD like a genre marker keyword should not weld
    together every record that happens to carry it)."""
    dsu = DSU(u.keys())

    for e in edges:
        a, b = e["from"], e["to"]
        if a in u and b in u:
            if full_degree.get(a, 0) <= hub_degree and full_degree.get(b, 0) <= hub_degree:
                dsu.union(a, b)

    context_to_u = defaultdict(set)
    for e in edges:
        a, b = e["from"], e["to"]
        if a in u and b not in u:
            context_to_u[b].add(a)
        elif b in u and a not in u:
            context_to_u[a].add(b)

    for c in sorted(context_to_u):
        if context_u_degree.get(c, 0) > hub_degree:
            continue
        roots = sorted({dsu.find(n) for n in context_to_u[c]})
        for r in roots[1:]:
            dsu.union(roots[0], r)

    return dsu


def build_components(u, dsu):
    groups = defaultdict(set)
    for fid in u:
        groups[dsu.find(fid)].add(fid)
    return list(groups.values())


# --------------------------------------------------------------------------
# Anchor selection
# --------------------------------------------------------------------------


def _anchor_key(fid, rec, degree):
    """Sort key such that max() picks the correct anchor: highest
    ANCHOR_PRIORITY, then status weight (added > changed > removed), then
    has-a-name, then edge degree, then LOWEST form_id (negated, since max()
    wants the biggest key)."""
    return (
        -_priority_rank((rec or {}).get("record_type")),
        _STATUS_WEIGHT.get((rec or {}).get("status"), -1),
        1 if (rec or {}).get("name") else 0,
        degree,
        -_int_fid(fid),
    )


def select_anchor(member_fids, u, full_degree):
    return max(member_fids, key=lambda fid: _anchor_key(fid, u.get(fid), full_degree.get(fid, 0)))


# --------------------------------------------------------------------------
# Step 6 (part 1): oversized-component splitting
# --------------------------------------------------------------------------


def _internal_adjacency(member_fids, edges):
    adj = defaultdict(set)
    for e in edges:
        a, b = e["from"], e["to"]
        if a in member_fids and b in member_fids:
            adj[a].add(b)
            adj[b].add(a)
    return adj


def split_oversized(component, u, edges, max_members):
    """Step 6a. A component over `max_members` containing >=2
    anchor-priority-type nodes splits into one bundle per such node ("top
    anchor"): every other member is assigned to its BFS-nearest top anchor
    over the component's internal (U<->U) edges, ties broken toward the
    higher-priority anchor. A node unreachable from any anchor (possible if
    it was only unioned in via a context-node bridge, which isn't an
    internal edge) falls back to the single highest-priority anchor.
    Components at/under the cap, or with <2 anchor-priority-type nodes, are
    returned unsplit."""
    if len(component) <= max_members:
        return [component]

    anchor_candidates = [fid for fid in component if (u[fid] or {}).get("record_type") in _ANCHOR_RANK]
    if len(anchor_candidates) < 2:
        return [component]

    adjacency = _internal_adjacency(component, edges)

    def _anchor_sort_key(fid):
        return (_priority_rank((u[fid] or {}).get("record_type")), _int_fid(fid))

    dist = {a: 0 for a in anchor_candidates}
    owner = {a: a for a in anchor_candidates}

    current_layer = list(anchor_candidates)
    layer = 0
    while current_layer:
        candidates = defaultdict(list)
        for node in current_layer:
            for neigh in adjacency.get(node, ()):
                if neigh not in dist:
                    candidates[neigh].append(owner[node])
        if not candidates:
            break
        layer += 1
        next_layer = []
        for node, owners in candidates.items():
            if node in dist:
                continue
            dist[node] = layer
            owner[node] = min(owners, key=_anchor_sort_key)
            next_layer.append(node)
        current_layer = next_layer

    if len(owner) < len(component):
        fallback_anchor = min(anchor_candidates, key=_anchor_sort_key)
        for fid in component:
            owner.setdefault(fid, fallback_anchor)

    groups = defaultdict(set)
    for fid in component:
        groups[owner[fid]].add(fid)
    return list(groups.values())


# --------------------------------------------------------------------------
# Step 6 (part 2): merging
# --------------------------------------------------------------------------


def merge_same_anchor(groups, u, full_degree):
    """Merge any groups that independently select the same anchor (e.g. two
    split fragments of an oversized component whose BFS-nearest assignment
    both happened to keep the same top anchor closest)."""
    by_anchor = {}
    order = []
    for g in groups:
        a = select_anchor(g, u, full_degree)
        if a not in by_anchor:
            by_anchor[a] = set()
            order.append(a)
        by_anchor[a] |= g
    return [by_anchor[a] for a in order]


def _overlap_ratio(a, b):
    denom = min(len(a), len(b))
    return (len(a & b) / denom) if denom else 0.0


def merge_by_overlap(groups, threshold=OVERLAP_MERGE_THRESHOLD):
    """Merge any two groups whose non-context member overlap / min(member
    count) is >= threshold -- e.g. two oversized-split fragments that
    shared enough satellites that splitting them was effectively spurious.
    Iterates to a fixpoint (a 3-way merge can create a new overlap)."""
    groups = [set(g) for g in sorted(groups, key=lambda g: min(_int_fid(f) for f in g))]
    changed = True
    while changed:
        changed = False
        merged = []
        used = [False] * len(groups)
        for i in range(len(groups)):
            if used[i]:
                continue
            cur = set(groups[i])
            used[i] = True
            for j in range(i + 1, len(groups)):
                if used[j]:
                    continue
                if _overlap_ratio(cur, groups[j]) >= threshold:
                    cur |= groups[j]
                    used[j] = True
                    changed = True
            merged.append(cur)
        groups = merged
    return groups


# --------------------------------------------------------------------------
# Step 7 (part 1): context-member attachment
# --------------------------------------------------------------------------


def build_context_incidence(edges, u):
    """context_fid -> [(u_fid, edge), ...] for every edge with exactly one
    endpoint in U."""
    inc = defaultdict(list)
    for e in edges:
        a, b = e["from"], e["to"]
        if a in u and b not in u:
            inc[b].append((a, e))
        elif b in u and a not in u:
            inc[a].append((b, e))
    return inc


def _context_rank(stub, incident_pairs, unique_keyword_patterns):
    """Rank 0 (highest): connected to a bundle member via a
    CONTEXT_TOP_TIER_RELATIONS edge (e.g. an OMOD's own mod target, or a
    COBJ's crafted item) -- the single most story-relevant context node.
    Rank 1: NPC_/CONT/QUST/COBJ. Rank 2: a unique-keyword-pattern KYWD.
    Rank 3: everything else."""
    record_type = (stub or {}).get("record_type")
    editor_id = (stub or {}).get("editor_id") or ""
    if any(e.get("relation") in CONTEXT_TOP_TIER_RELATIONS for _u_fid, e in incident_pairs):
        return (0, 0)
    if record_type in CONTEXT_PREFERRED_TYPES:
        return (1, CONTEXT_PREFERRED_TYPES.index(record_type))
    if record_type == "KYWD" and any(
        fnmatch.fnmatch(editor_id.lower(), pat.lower()) for pat in unique_keyword_patterns
    ):
        return (2, 0)
    return (3, 0)


def attach_context(members, context_incidence, context_stubs, cap, unique_keyword_patterns):
    """Step 7a. Candidate context nodes = anything incident to a member of
    this bundle. Sorted by preference (a mod_for/crafts-linked node --
    e.g. an OMOD's own weapon/armor -- first, then NPC_/CONT/QUST/COBJ, then
    a unique-keyword-pattern KYWD, then anything else; form_id as a
    deterministic final tie-break), capped at `cap`. Returns (member dicts
    with role="context"/status="unchanged", their connecting edges)."""
    candidates = {
        c: [(u_fid, e) for (u_fid, e) in pairs if u_fid in members]
        for c, pairs in context_incidence.items()
    }
    candidates = {c: pairs for c, pairs in candidates.items() if pairs}

    ordered = sorted(
        candidates.keys(),
        key=lambda c: _context_rank(context_stubs.get(c), candidates[c], unique_keyword_patterns)
        + (_int_fid(c),),
    )
    chosen = ordered[:cap]

    context_members = []
    context_edges = []
    for c in chosen:
        stub = context_stubs.get(c) or {}
        context_members.append(
            {
                "form_id": c,
                "record_type": stub.get("record_type"),
                "editor_id": stub.get("editor_id"),
                "name": stub.get("name"),
                "status": "unchanged",
                "role": "context",
            }
        )
        for _u_fid, e in candidates[c]:
            context_edges.append(e)
    return context_members, context_edges


# --------------------------------------------------------------------------
# Step 7 (part 2): categorization
# --------------------------------------------------------------------------


def _fnmatch_any(value, patterns):
    if not value:
        return False
    v = value.lower()
    return any(fnmatch.fnmatch(v, pat.lower()) for pat in patterns)


def _scope_members(scope, anchor_member, all_members):
    if scope == "anchor":
        return [anchor_member]
    if scope == "member":
        return [m for m in all_members if m["role"] != "context"]
    return list(all_members)  # "any"


def _rule_fields_match(rule, scope_members):
    record_types = rule.get("record_type")
    edids = rule.get("edid")
    names = rule.get("name")
    if record_types is None and edids is None and names is None:
        return True  # nothing to check against members (e.g. a keyword-only rule)
    for m in scope_members:
        if record_types is not None and m.get("record_type") not in record_types:
            continue
        if edids is not None and not _fnmatch_any(m.get("editor_id"), edids):
            continue
        if names is not None and not _fnmatch_any(m.get("name"), names):
            continue
        return True
    return False


def _anchor_keyword_edids(client, esm, anchor_fid, cache):
    """Resolve the anchor's own decoded Keywords list to a list of editor
    IDs, via one `client.record(esm, anchor_fid, resolve="stub")` call,
    cached per anchor_fid. Any failure (daemon error, missing/malformed
    Keywords field) is treated as "no keywords" -- never raises."""
    if anchor_fid in cache:
        return cache[anchor_fid]
    edids = []
    try:
        rec = client.record(esm, anchor_fid, resolve="stub")
        for kw in (rec.get("fields") or {}).get("Keywords") or []:
            if isinstance(kw, dict):
                e = kw.get("editor_id")
                if e:
                    edids.append(e)
            elif isinstance(kw, str):
                edids.append(kw)
    except esm_daemon.DaemonError:
        edids = []
    cache[anchor_fid] = edids
    return edids


def rule_matches(rule, anchor_member, all_members, client, esm, keyword_cache):
    scope = rule.get("scope", "any")
    if not _rule_fields_match(rule, _scope_members(scope, anchor_member, all_members)):
        return False
    keyword_patterns = rule.get("keyword")
    if keyword_patterns:
        edids = _anchor_keyword_edids(client, esm, anchor_member["form_id"], keyword_cache)
        if not any(_fnmatch_any(e, keyword_patterns) for e in edids):
            return False
    return True


def categorize_bundle(anchor_member, all_members, categories, client, esm, keyword_cache):
    """Step 7b. Categories are evaluated in the config's fixed order; the
    first category with any matching rule wins (rule matches = ALL of that
    rule's specified fields hold). A category with an empty `rules` list
    (the config's trailing "uncategorized" entry) always matches -- that's
    the fallback, so its `category_rule` is None."""
    for cat in categories:
        rules = cat.get("rules") or []
        if not rules:
            return cat["id"], cat.get("label", cat["id"]), None
        for i, rule in enumerate(rules):
            if rule_matches(rule, anchor_member, all_members, client, esm, keyword_cache):
                return cat["id"], cat.get("label", cat["id"]), f"{cat['id']}/rule_{i}"
    return None, None, None


# --------------------------------------------------------------------------
# Orchestration
# --------------------------------------------------------------------------


def resolve_settings(config):
    """Merge patch_notes_categories.json's "settings" section over the
    hardcoded defaults. (CLI-flag overrides are merged into config by
    main() before it calls build_bundles(), per the documented "CLI flag >
    config settings > defaults" precedence.)"""
    return {**DEFAULT_SETTINGS, **(config.get("settings") or {})}


def _esm_for_status(status, old_esm, new_esm):
    return old_esm if status == "removed" else new_esm


def _bundle_title(anchor_rec, anchor_fid):
    label = (anchor_rec or {}).get("name") or (anchor_rec or {}).get("editor_id") or anchor_fid
    return f"{label} ({(anchor_rec or {}).get('record_type')})"


def build_bundles(comp, client, old_esm, new_esm, config):
    """Library entry point: comprehensive.json dict -> bundles.json dict.
    See module docstring for the full algorithm."""
    settings = resolve_settings(config)
    categories = config.get("categories") or []
    ref_names = comp.get("ref_names") or {}

    u = build_universe(comp)

    edges, context_stubs = build_edges(u, ref_names, client, old_esm, new_esm, settings["refs_depth"])
    full_degree, context_u_degree = compute_degrees(u, edges)

    dsu = union_find(u, edges, full_degree, context_u_degree, settings["hub_degree"])
    components = build_components(u, dsu)

    split_groups = []
    for component in components:
        split_groups.extend(split_oversized(component, u, edges, settings["max_members"]))

    merged_groups = merge_same_anchor(split_groups, u, full_degree)
    merged_groups = merge_by_overlap(merged_groups, OVERLAP_MERGE_THRESHOLD)

    context_incidence = build_context_incidence(edges, u)
    keyword_cache = {}

    raw_bundles = []
    for member_fids in merged_groups:
        anchor_fid = select_anchor(member_fids, u, full_degree)
        anchor_rec = u[anchor_fid] or {}

        member_dicts = [
            {
                "form_id": fid,
                "record_type": (u[fid] or {}).get("record_type"),
                "editor_id": (u[fid] or {}).get("editor_id"),
                "name": (u[fid] or {}).get("name"),
                "status": (u[fid] or {}).get("status"),
                "role": "anchor" if fid == anchor_fid else "satellite",
            }
            for fid in member_fids
        ]
        member_dicts.sort(key=lambda m: (0 if m["role"] == "anchor" else 1, _int_fid(m["form_id"])))
        anchor_member = member_dicts[0]

        context_members, context_edges = attach_context(
            member_fids, context_incidence, context_stubs,
            settings["context_cap"], settings["unique_keyword_patterns"],
        )

        internal_edges = [e for e in edges if e["from"] in member_fids and e["to"] in member_fids]
        bundle_edges = dedupe_edges(internal_edges + context_edges)

        all_members = member_dicts + context_members
        esm_for_anchor = _esm_for_status(anchor_rec.get("status"), old_esm, new_esm)
        cat_id, cat_label, cat_rule = categorize_bundle(
            anchor_member, all_members, categories, client, esm_for_anchor, keyword_cache,
        )

        raw_bundles.append(
            {
                "category": cat_id,
                "category_label": cat_label,
                "category_rule": cat_rule,
                "title": _bundle_title(anchor_rec, anchor_fid),
                "anchor": {
                    "form_id": anchor_fid,
                    "record_type": anchor_rec.get("record_type"),
                    "editor_id": anchor_rec.get("editor_id"),
                    "name": anchor_rec.get("name"),
                    "status": anchor_rec.get("status"),
                },
                "members": all_members,
                "edges": bundle_edges,
                "bug_watch": False,
                "lint_ids": [],
                "_anchor_fid": anchor_fid,
            }
        )

    cat_order = {c.get("id"): i for i, c in enumerate(categories)}
    raw_bundles.sort(
        key=lambda b: (cat_order.get(b["category"], len(categories)), _int_fid(b["_anchor_fid"]))
    )
    for i, b in enumerate(raw_bundles, start=1):
        b["id"] = f"B{i:04d}"
        del b["_anchor_fid"]

    n_bundles = len(raw_bundles)
    n_singletons = sum(
        1 for b in raw_bundles if sum(1 for m in b["members"] if m["role"] != "context") == 1
    )
    n_uncategorized = sum(1 for b in raw_bundles if b["category"] == "uncategorized")

    meta = {
        "patch_date": (comp.get("meta") or {}).get("patch_date", ""),
        "generated_at": _iso_now(),
        "source": "comprehensive.json",
        "refs_depth": settings["refs_depth"],
        "hub_degree": settings["hub_degree"],
        "max_members": settings["max_members"],
        "counts": {
            "bundles": n_bundles,
            "singletons": n_singletons,
            "uncategorized": n_uncategorized,
        },
    }

    return {"schema_version": 1, "meta": meta, "bundles": raw_bundles, "lints": []}


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

SCRIPT_DIR = Path(__file__).resolve().parent
WORKSPACE_ROOT = SCRIPT_DIR.parent
DEFAULT_CATEGORIES_PATH = SCRIPT_DIR / "patch_notes_categories.json"


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def find_esm_binary(explicit):
    """Locate the `esm` CLI binary (used to trigger warm-daemon auto-spawn
    via `esm_daemon.ensure_daemon`), mirroring make_patch_notes.py's
    find_esm_binary(): explicit path, else the workspace release build,
    else $PATH."""
    if explicit:
        p = Path(explicit)
        if p.is_file() and os.access(p, os.X_OK):
            return p
        raise SystemExit(f"--esm-bin path not executable: {explicit}")

    release = WORKSPACE_ROOT / "target" / "release" / "esm"
    if release.is_file() and os.access(release, os.X_OK):
        return release

    found = shutil.which("esm")
    if found:
        return Path(found)

    raise SystemExit(
        "Cannot find esm binary. Build it first:\n  cargo build --release --features server\n"
        "Or pass --esm-bin /path/to/esm"
    )


def build_arg_parser():
    ap = argparse.ArgumentParser(
        prog="build_bundles.py",
        description="Tool 2: cluster comprehensive.json diff records into narrative bundles.json.",
    )
    ap.add_argument("comprehensive_json", help="Path to comprehensive.json (Tool 1 output).")
    ap.add_argument("--new-esm", required=True, help="Path to the NEW .esm.")
    ap.add_argument("--old-esm", required=True, help="Path to the OLD .esm.")
    ap.add_argument("--out", default="bundles.json", help="Output path (default: bundles.json).")
    ap.add_argument(
        "--categories", default=str(DEFAULT_CATEGORIES_PATH),
        help="Path to patch_notes_categories.json (default: the copy next to this script).",
    )
    ap.add_argument("--refs-depth", type=int, default=None, help="Override base reverse-ref BFS depth.")
    ap.add_argument("--hub-degree", type=int, default=None, help="Override the hub-degree threshold.")
    ap.add_argument("--max-members", type=int, default=None, help="Override the oversized-split threshold.")
    ap.add_argument("--esm-bin", default=None, help="Path to the esm CLI binary (live daemon mode only).")
    ap.add_argument(
        "--offline", action="store_true",
        help="Use esm_daemon.FakeClient (--refs-fixture) instead of a live warm daemon.",
    )
    ap.add_argument("--refs-fixture", default=None, help="FakeClient fixture JSON (required with --offline).")
    return ap


def main(argv=None):
    args = build_arg_parser().parse_args(argv)

    if args.offline and not args.refs_fixture:
        eprint("error: --offline requires --refs-fixture")
        return 1

    try:
        with open(args.comprehensive_json, encoding="utf-8") as f:
            comp = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        eprint(f"error: failed to load {args.comprehensive_json}: {e}")
        return 1

    try:
        with open(args.categories, encoding="utf-8") as f:
            config = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        eprint(f"error: failed to load {args.categories}: {e}")
        return 1

    settings = dict(config.get("settings") or {})
    for key, value in (
        ("refs_depth", args.refs_depth),
        ("hub_degree", args.hub_degree),
        ("max_members", args.max_members),
    ):
        if value is not None:
            settings[key] = value
    config = {**config, "settings": settings}

    if args.offline:
        client = esm_daemon.FakeClient(args.refs_fixture)
    else:
        esm_bin = find_esm_binary(args.esm_bin)
        client = esm_daemon.ensure_daemon(esm_bin, args.new_esm)

    result = build_bundles(comp, client, args.old_esm, args.new_esm, config)

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(result, f, indent=2, ensure_ascii=False)
        f.write("\n")

    counts = result["meta"]["counts"]
    eprint(
        f"wrote {args.out} ({counts['bundles']} bundles, {counts['singletons']} singletons, "
        f"{counts['uncategorized']} uncategorized)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
