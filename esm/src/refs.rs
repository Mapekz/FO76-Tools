//! Reverse-reference graph engine: seed resolution, the depth-bounded BFS
//! walk, and bidirectional path search over the reverse-reference index
//! `Index` builds (`ipc.rs`'s `Op::ReferencedBy`/`Op::RefPath` are thin
//! dispatch wrappers around this module).
//!
//! Extracted out of `ipc.rs` (Stage A of the architecture-deepening plan):
//! the wire protocol (`Op`, `dispatch`/`dispatch_op`, and the DTOs that cross
//! the process boundary — `RefRow`, `RefList`, `RefSort`, `RefPathNode`)
//! stays in `ipc.rs`; this module owns the seed-selector/walk/path-search
//! *algorithm* those DTOs describe the result of. See
//! [`docs/adr/0004-refs-seed-selectors.md`](https://github.com/Mapekz/FO76-Tools/blob/main/esm/docs/adr/0004-refs-seed-selectors.md)
//! for the Direct/Carriers seed-selector vocabulary ([`RefSeeds`] is that
//! ADR's central type).

use crate::ipc::{
    DEFAULT_MAX_DEPTH, RecordSel, RefList, RefPathNode, RefRow, RefSort, resolve_sel,
};
use crate::{CarrierTag, Database, EntryPointSpec, FormId, OmodPropertySpec, RecordRow};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Walk reverse references from `target` up to `depth` hops using BFS.
///
/// A `depth` of 1 (the default) returns the same set as the old single-level
/// lookup.  Higher values follow the reverse-reference graph breadth-first,
/// visiting each node at most once (cycle-safe).  `depth` is clamped to
/// `[1, DEFAULT_MAX_DEPTH]`; `depth == 0` requests an unbounded walk instead
/// (no fixed hop cap — see [`RefList::effective_depth`]).
///
/// Each `RefRow` carries:
/// - `depth`: hop distance from `target` (1 = direct referencer).
/// - `path`: intermediate nodes between `target` and this row; empty for
///   depth-1 rows (and omitted from serialized JSON when empty).
///
/// `type_filter`, if set, must be a 4-character record-type signature
/// (case-insensitive); only rows of that type are emitted. The filter is
/// applied to *emission*, not traversal — the walk still expands through
/// non-matching nodes so a matching node further away stays reachable, and
/// `limit`/`total`/`capped` are computed against the filtered set.
///
/// `include_paths`, if true, decodes every emitted row's record body and
/// annotates it with [`RefRow::field_paths`] (see
/// [`Database::formid_reference_paths`]) — opt-in because it requires a full
/// decode per row, unlike the rest of this walk.
pub fn referenced_by_enriched(
    db: &mut Database,
    target: FormId,
    depth: usize,
    limit: usize,
    type_filter: Option<&str>,
    include_paths: bool,
    sort: RefSort,
) -> anyhow::Result<RefList> {
    let (rows, stats) = referenced_by_walk(
        db,
        &[(target, Vec::new())],
        false,
        depth,
        limit,
        type_filter,
        include_paths,
        sort,
    )?;
    Ok(RefList {
        target: target.display(),
        rows,
        total: stats.total,
        capped: stats.capped,
        carrier_total: None,
        tag_total: None,
        requested_depth: stats.requested_depth,
        effective_depth: stats.effective_depth,
        depth_capped: stats.depth_capped,
        frontier_remaining: stats.frontier_remaining,
        per_depth_totals: stats.per_depth_totals,
        shown_max_depth: stats.shown_max_depth,
    })
}

/// Multi-seed reverse-reference walk: every entry in `seeds` is emitted as
/// its own `depth: 0` "carrier" row — unlike [`referenced_by_enriched`],
/// whose single target is only a BFS root and never appears in the output —
/// then the BFS proceeds from all seeds at once, so a record referencing two
/// different seeds is still only emitted once. Used by [`resolve_ref_seeds`]'s
/// entry-point path (see [`crate::EntryPointSpec`]).
///
/// `seeds` carries per-carrier tags that are copied onto every descendant
/// row (and unioned on equal-depth re-reaches). Caller order is preserved
/// end-to-end — do not re-sort here; see [`Database::perks_by_entry_point`].
#[allow(clippy::too_many_arguments)]
pub fn referenced_by_enriched_multi(
    db: &mut Database,
    seeds: &[(FormId, Vec<CarrierTag>)],
    label: String,
    depth: usize,
    limit: usize,
    type_filter: Option<&str>,
    include_paths: bool,
    sort: RefSort,
) -> anyhow::Result<RefList> {
    let tag_total = seeds
        .iter()
        .flat_map(|(_, tags)| tags.iter().map(|t| (t.kind, t.scope.as_deref(), t.id)))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let (rows, stats) = referenced_by_walk(
        db,
        seeds,
        true,
        depth,
        limit,
        type_filter,
        include_paths,
        sort,
    )?;
    Ok(RefList {
        target: label,
        rows,
        total: stats.total,
        capped: stats.capped,
        carrier_total: Some(stats.carrier_total),
        tag_total: Some(tag_total),
        requested_depth: stats.requested_depth,
        effective_depth: stats.effective_depth,
        depth_capped: stats.depth_capped,
        frontier_remaining: stats.frontier_remaining,
        per_depth_totals: stats.per_depth_totals,
        shown_max_depth: stats.shown_max_depth,
    })
}

/// Shared BFS behind [`referenced_by_enriched`] / [`referenced_by_enriched_multi`].
///
/// `seeds` are the BFS roots, optionally tagged with carrier tags. When
/// `emit_seeds` is true, each seed is also emitted as its own `depth: 0` row
/// *before* the BFS-found referencer rows, and the queue is seeded with the
/// carrier's own [`RefPathNode`] so descendants' `path`/`VIA` trace back to
/// that carrier. When false (the legacy single-target path), seeds are only
/// BFS roots with empty paths — exactly as `target` always was.
///
/// Seed order is preserved (stable-dedup by FormID only). Callers that care
/// about display/attribution order — notably
/// [`Database::perks_by_entry_point`] — must sort before calling.
///
/// Returns `(rows, stats)` — see [`WalkStats`].
#[allow(clippy::too_many_arguments)]
fn referenced_by_walk(
    db: &mut Database,
    seeds: &[(FormId, Vec<CarrierTag>)],
    emit_seeds: bool,
    depth: usize,
    limit: usize,
    type_filter: Option<&str>,
    include_paths: bool,
    sort: RefSort,
) -> anyhow::Result<(Vec<RefRow>, WalkStats)> {
    let requested_depth = depth;
    // `depth == 0` requests an unbounded walk (no fixed hop cap); any other
    // value clamps to `[1, DEFAULT_MAX_DEPTH]` as before.
    let max_depth = if depth == 0 {
        usize::MAX
    } else {
        depth.clamp(1, DEFAULT_MAX_DEPTH)
    };
    let effective_depth = if depth == 0 { None } else { Some(max_depth) };
    let type_filter = match type_filter {
        Some(t) => {
            if t.len() != 4 {
                bail!("record type '{}' must be a 4-character signature", t);
            }
            Some(t.to_uppercase())
        }
        None => None,
    };
    let type_matches = |record_type: &Option<String>| match &type_filter {
        Some(f) => record_type.as_deref() == Some(f.as_str()),
        None => true,
    };

    // Stable-dedup by FormID, preserving first-occurrence order. Do NOT
    // re-sort by form_id — caller order drives carrier display grouping and
    // BFS attribution priority.
    let mut seen_seed = HashSet::new();
    let seeds: Vec<(FormId, Vec<CarrierTag>)> = seeds
        .iter()
        .filter(|(f, _)| seen_seed.insert(*f))
        .cloned()
        .collect();
    let seed_tags: HashMap<FormId, Vec<CarrierTag>> =
        seeds.iter().map(|(f, tags)| (*f, tags.clone())).collect();
    let seed_ids: Vec<FormId> = seeds.iter().map(|(f, _)| *f).collect();

    // `seen` is both the dedup set for emitted referencer rows and the BFS
    // visited set. Seeding with every entry in `seeds` prevents a seed from
    // appearing as another seed's own referencer and breaks self-referential
    // cycles.
    let mut seen: HashSet<FormId> = seed_ids.iter().copied().collect();

    // Queue entries: (node_to_expand, originating_carrier, path).
    // In EP mode (`emit_seeds`), path[0] is the carrier itself; hop_depth
    // subtracts 1 so direct referencers still report depth 1. In Direct mode
    // the origin is None and the path starts empty (legacy behavior).
    let mut queue: VecDeque<(FormId, Option<FormId>, Vec<RefPathNode>)> = VecDeque::new();
    let mut seed_rows: Vec<RefRow> = Vec::new();

    if emit_seeds {
        for &seed in &seed_ids {
            let Some(row) = db.record_row_for(seed)? else {
                continue;
            };
            let seed_node = RefPathNode {
                form_id: row.form_id.clone(),
                record_type: row.record_type.clone(),
                editor_id: row.editor_id.clone(),
            };
            if type_matches(&row.record_type) {
                seed_rows.push(RefRow {
                    form_id: row.form_id,
                    record_type: row.record_type,
                    editor_id: row.editor_id,
                    name: row.name,
                    offset: row.offset,
                    depth: 0,
                    path: Vec::new(),
                    field_paths: None,
                    tags: seed_tags.get(&seed).cloned().unwrap_or_default(),
                });
            }
            // Type-filtered carriers still contribute a RefPathNode so their
            // descendants' path/VIA remain attributable.
            queue.push_back((seed, Some(seed), vec![seed_node]));
        }
    } else {
        for &seed in &seed_ids {
            queue.push_back((seed, None, Vec::new()));
        }
    }

    let carrier_total = seed_rows.len();
    let mut rows: Vec<RefRow> = Vec::new();
    // FormId → index into `rows` for equal-depth carrier-tag unions.
    let mut emitted: HashMap<FormId, usize> = HashMap::new();
    // Newly-discovered nodes at `max_depth` that were not expanded further —
    // the unexplored BFS frontier. See `RefList::depth_capped`.
    let mut frontier_remaining: usize = 0;

    while let Some((current, origin, path_here)) = queue.pop_front() {
        for r in db.referenced_by(current)? {
            let fid = match crate::parse_form_id_input(&r.form_id) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let hop_depth = path_here.len() + 1 - usize::from(emit_seeds);

            if !seen.insert(fid) {
                // Equal-depth re-reach: union tags onto the already-emitted
                // row without changing its path/VIA (first-reach wins for the
                // path). Deeper re-reaches and seed self-hits are ignored as
                // before.
                if let Some(&idx) = emitted.get(&fid)
                    && rows[idx].depth == hop_depth
                    && let Some(origin_fid) = origin
                {
                    merge_tags(
                        &mut rows[idx].tags,
                        seed_tags
                            .get(&origin_fid)
                            .map(|v| v.as_slice())
                            .unwrap_or(&[]),
                    );
                }
                continue;
            }

            let record_type = db
                .index
                .get_by_formid(fid)
                .map(|m| m.signature.as_str().to_owned());

            if type_matches(&record_type) {
                let field_paths = if include_paths {
                    Some(db.formid_reference_paths(fid, current))
                } else {
                    None
                };
                let tags = origin
                    .and_then(|o| seed_tags.get(&o).cloned())
                    .unwrap_or_default();
                let idx = rows.len();
                rows.push(RefRow {
                    form_id: r.form_id.clone(),
                    record_type: record_type.clone(),
                    editor_id: r.editor_id.clone(),
                    name: r.name.clone(),
                    offset: r.offset,
                    depth: hop_depth,
                    path: path_here.clone(),
                    field_paths,
                    tags,
                });
                emitted.insert(fid, idx);
            }

            if hop_depth < max_depth {
                let mut new_path = path_here.clone();
                new_path.push(RefPathNode {
                    form_id: r.form_id,
                    record_type,
                    editor_id: r.editor_id,
                });
                queue.push_back((fid, origin, new_path));
            } else {
                frontier_remaining += 1;
            }
        }
    }

    let form_id_key = |r: &RefRow| {
        crate::parse_form_id_input(&r.form_id)
            .map(|f| f.0)
            .unwrap_or(u32::MAX)
    };
    match sort {
        RefSort::Formid => rows.sort_by_key(form_id_key),
        RefSort::Depth => rows.sort_by_key(|r| (r.depth, form_id_key(r))),
    }

    let mut all_rows = seed_rows;
    all_rows.append(&mut rows);

    let max_depth_seen = all_rows.iter().map(|r| r.depth).max().unwrap_or(0);
    let mut per_depth_totals = vec![0usize; max_depth_seen + 1];
    for r in &all_rows {
        per_depth_totals[r.depth] += 1;
    }

    let total = all_rows.len();
    let capped = limit > 0 && total > limit;
    let limited: Vec<RefRow> = if limit > 0 {
        all_rows.into_iter().take(limit).collect()
    } else {
        all_rows
    };
    let shown_max_depth = limited.iter().map(|r| r.depth).max().unwrap_or(0);

    Ok((
        limited,
        WalkStats {
            total,
            capped,
            carrier_total,
            requested_depth,
            effective_depth,
            depth_capped: frontier_remaining > 0,
            frontier_remaining,
            per_depth_totals,
            shown_max_depth,
        },
    ))
}

/// Non-row outcome of one [`referenced_by_walk`] call — the shared "how did
/// this walk go" facts both [`referenced_by_enriched`] and
/// [`referenced_by_enriched_multi`] copy into their own `RefList` (each adds
/// its own `target`/`tag_total` on top).
struct WalkStats {
    total: usize,
    capped: bool,
    /// Depth-0 rows that survived the type filter. 0 when `emit_seeds` is
    /// false.
    carrier_total: usize,
    requested_depth: usize,
    effective_depth: Option<usize>,
    depth_capped: bool,
    frontier_remaining: usize,
    per_depth_totals: Vec<usize>,
    shown_max_depth: usize,
}

/// Merge `incoming` into `dst` by `(kind, scope, id)`, keeping sorted+deduped.
fn merge_tags(dst: &mut Vec<CarrierTag>, incoming: &[CarrierTag]) {
    if incoming.is_empty() {
        return;
    }
    dst.extend(incoming.iter().cloned());
    dst.sort();
    dst.dedup_by(|a, b| a.kind == b.kind && a.scope == b.scope && a.id == b.id);
}

/// Default `--max-hops` for [`find_ref_path`] when the caller passes 0.
pub const DEFAULT_MAX_PATH_HOPS: usize = 12;

/// Node-visit ceiling for [`find_ref_path`]'s bidirectional search, combined
/// across both frontiers — a backstop against a disconnected/near-total
/// closure search still trying to enumerate hundreds of thousands of nodes
/// one at a time. When hit, the answer is genuinely "don't know" (see
/// [`RefPathResult::budget_exhausted`]), not "definitely no path".
const REF_PATH_NODE_BUDGET: usize = 200_000;

/// One node on a [`RefPathResult`] chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefPathHop {
    pub form_id: String,
    pub record_type: Option<String>,
    pub editor_id: Option<String>,
    pub name: Option<String>,
    /// JSON field path(s) inside *this* hop's own decoded body where it
    /// references the previous hop in the chain (the one closer to
    /// `RefPathResult::from`). `None` unless `paths` was requested; always
    /// absent on the first hop (`from` itself has no predecessor to point at).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_paths: Option<Vec<String>>,
}

/// Outcome of [`find_ref_path`] — a chain of reverse-reference hops
/// connecting `from` to `to` (`from` first, `to` last), or a report of why
/// none was found.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RefPathResult {
    pub from: String,
    pub to: String,
    /// `Some(chain)` when a path was found within `max_hops`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<Vec<RefPathHop>>,
    /// `chain.len() - 1` (0 when `from == to`). `None` alongside `chain: None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hops: Option<usize>,
    /// True when [`REF_PATH_NODE_BUDGET`] was exhausted before a path was
    /// found or definitively ruled out within `max_hops` — the search result
    /// is inconclusive, not a confirmed "no path exists".
    pub budget_exhausted: bool,
}

/// Bidirectional BFS connecting `from` to `to` via the reverse-reference
/// relation — the same relation [`referenced_by_walk`] follows one-directionally
/// (`n` is connected to `m` when `m` is in `referenced_by(n)`, i.e. `m`'s body
/// contains `n`'s FormID).
///
/// A plain one-directional walk from `from` must expand *every* referencer at
/// every hop, which balloons on hub-heavy graphs (CELL/REFR nodes routinely
/// have thousands of referencers) long before it reaches a `to` with a small
/// number of hops between them. This function instead grows two frontiers at
/// once and expands whichever is smaller each round:
/// - **back** (from `from`): expands via [`Database::referenced_by`] — who
///   references this node. Exactly [`referenced_by_walk`]'s own direction.
/// - **fwd** (from `to`): expands via [`Database::outgoing_formids`] — what
///   this node's own body references. This discovers the *predecessor* of a
///   node in the same reverse-reference chain (if `X`'s body contains `Y`'s
///   FormID, then `X` is a referencer of `Y`, i.e. `X` is one hop closer to
///   `from` than `Y` is) — cheap because it's bounded by each node's own
///   field count, not by how many other records happen to reference it.
///
/// The two frontiers meet at a node `M` such that `from` reaches `M` via some
/// number of back-hops and `to` reaches `M` via some number of fwd-hops;
/// splicing those two partial chains at `M` yields a complete `from..=to`
/// chain in the same hop-by-hop shape [`referenced_by_walk`]'s `path`/`VIA`
/// already uses.
///
/// `max_hops` is the combined hop-count ceiling (0 = [`DEFAULT_MAX_PATH_HOPS`]).
pub fn find_ref_path(
    db: &mut Database,
    from: FormId,
    to: FormId,
    max_hops: usize,
    include_paths: bool,
) -> anyhow::Result<RefPathResult> {
    let max_hops = if max_hops == 0 {
        DEFAULT_MAX_PATH_HOPS
    } else {
        max_hops
    };

    if from == to {
        let hop = ref_path_hop(db, from, None)?;
        return Ok(RefPathResult {
            from: from.display(),
            to: to.display(),
            chain: Some(vec![hop]),
            hops: Some(0),
            budget_exhausted: false,
        });
    }

    // parent maps: back_parent[n] = the node one hop closer to `from` that
    // discovered `n` (n is a referencer of back_parent[n]); fwd_parent[n] =
    // the node one hop closer to `to` that discovered `n` (fwd_parent[n]'s
    // body references n, i.e. fwd_parent[n] is a referencer of n).
    let mut back_parent: HashMap<FormId, FormId> = HashMap::new();
    let mut fwd_parent: HashMap<FormId, FormId> = HashMap::new();
    back_parent.insert(from, from);
    fwd_parent.insert(to, to);
    let mut back_frontier = vec![from];
    let mut fwd_frontier = vec![to];
    let mut back_depth = 0;
    let mut fwd_depth = 0;
    // +2 for the two seeds themselves.
    let mut visited = 2;

    loop {
        if back_depth + fwd_depth >= max_hops {
            return Ok(RefPathResult {
                from: from.display(),
                to: to.display(),
                chain: None,
                hops: None,
                budget_exhausted: false,
            });
        }
        if back_frontier.is_empty() && fwd_frontier.is_empty() {
            return Ok(RefPathResult {
                from: from.display(),
                to: to.display(),
                chain: None,
                hops: None,
                budget_exhausted: false,
            });
        }

        // Expand whichever frontier is smaller (an empty frontier — the
        // other side ran dry — never gets picked over a non-empty one).
        let expand_back = !back_frontier.is_empty()
            && (fwd_frontier.is_empty() || back_frontier.len() <= fwd_frontier.len());

        let mut next_frontier = Vec::new();
        if expand_back {
            for &node in &back_frontier {
                for r in db.referenced_by(node)? {
                    let Ok(fid) = crate::parse_form_id_input(&r.form_id) else {
                        continue;
                    };
                    if back_parent.contains_key(&fid) {
                        continue;
                    }
                    back_parent.insert(fid, node);
                    visited += 1;
                    if fwd_parent.contains_key(&fid) {
                        return build_ref_path_result(
                            db,
                            from,
                            to,
                            fid,
                            &back_parent,
                            &fwd_parent,
                            include_paths,
                        );
                    }
                    if visited >= REF_PATH_NODE_BUDGET {
                        return Ok(RefPathResult {
                            from: from.display(),
                            to: to.display(),
                            chain: None,
                            hops: None,
                            budget_exhausted: true,
                        });
                    }
                    next_frontier.push(fid);
                }
            }
            back_frontier = next_frontier;
            back_depth += 1;
        } else {
            for &node in &fwd_frontier {
                for fid in db.outgoing_formids(node) {
                    if fwd_parent.contains_key(&fid) {
                        continue;
                    }
                    fwd_parent.insert(fid, node);
                    visited += 1;
                    if back_parent.contains_key(&fid) {
                        return build_ref_path_result(
                            db,
                            from,
                            to,
                            fid,
                            &back_parent,
                            &fwd_parent,
                            include_paths,
                        );
                    }
                    if visited >= REF_PATH_NODE_BUDGET {
                        return Ok(RefPathResult {
                            from: from.display(),
                            to: to.display(),
                            chain: None,
                            hops: None,
                            budget_exhausted: true,
                        });
                    }
                    next_frontier.push(fid);
                }
            }
            fwd_frontier = next_frontier;
            fwd_depth += 1;
        }
    }
}

/// Splice the two partial chains at meeting node `meet` into one complete
/// `from..=to` chain and decode each hop. See [`find_ref_path`] for the
/// direction convention `back_parent`/`fwd_parent` follow.
fn build_ref_path_result(
    db: &mut Database,
    from: FormId,
    to: FormId,
    meet: FormId,
    back_parent: &HashMap<FormId, FormId>,
    fwd_parent: &HashMap<FormId, FormId>,
    include_paths: bool,
) -> anyhow::Result<RefPathResult> {
    let mut nodes = vec![meet];
    let mut cur = meet;
    while cur != from {
        cur = back_parent[&cur];
        nodes.push(cur);
    }
    nodes.reverse(); // [from, ..., meet]

    let mut cur = meet;
    while cur != to {
        cur = fwd_parent[&cur];
        nodes.push(cur);
    }
    // nodes is now [from, ..., meet, ..., to].

    let hops = nodes.len() - 1;
    let mut chain = Vec::with_capacity(nodes.len());
    for (i, &n) in nodes.iter().enumerate() {
        let predecessor = if i > 0 { Some(nodes[i - 1]) } else { None };
        chain.push(ref_path_hop(db, n, predecessor.filter(|_| include_paths))?);
    }

    Ok(RefPathResult {
        from: from.display(),
        to: to.display(),
        chain: Some(chain),
        hops: Some(hops),
        budget_exhausted: false,
    })
}

/// Decode one chain node into a [`RefPathHop`], annotating `field_paths`
/// (this node's own reference to `predecessor`, its immediate neighbor
/// closer to `from`) when `predecessor` is `Some`.
fn ref_path_hop(
    db: &mut Database,
    node: FormId,
    predecessor: Option<FormId>,
) -> anyhow::Result<RefPathHop> {
    let row = db.record_row_for(node)?.unwrap_or_else(|| RecordRow {
        form_id: node.display(),
        record_type: None,
        editor_id: None,
        name: None,
        offset: 0,
    });
    let field_paths = predecessor.map(|p| db.formid_reference_paths(node, p));
    Ok(RefPathHop {
        form_id: row.form_id,
        record_type: row.record_type,
        editor_id: row.editor_id,
        name: row.name,
        field_paths,
    })
}

/// Seeds resolved from a [`RecordSel`] for [`crate::ipc::Op::ReferencedBy`] — either a
/// single direct target or every carrier matched by an entry-point or OMOD-
/// property selector.
pub enum RefSeeds {
    /// A resolved FormID/EditorID/Auto target. The target is only a BFS
    /// root — [`referenced_by_enriched`] never emits it as a row.
    Direct(FormId),
    /// One or more carrier records matched by a virtual selector, each
    /// emitted as its own `depth: 0` row by [`referenced_by_enriched_multi`].
    Carriers {
        label: String,
        seeds: crate::Carriers,
    },
}

/// Resolve a [`RecordSel`] to BFS seeds for [`crate::ipc::Op::ReferencedBy`] specifically
/// — the one place carrier selectors are handled, and the one place an
/// EditorID lookup miss falls back to an entry-point name match (so a bare
/// positional token like `'Mod Percent Blocked'` — parsed as
/// [`RecordSel::Edid`] by [`RecordSel::from_input`], since it isn't FormID-
/// shaped — resolves without needing the explicit `--entry-point` flag).
/// Every other `Op` uses [`resolve_sel`], which rejects carrier selectors.
pub(crate) fn resolve_ref_seeds(db: &mut Database, sel: &RecordSel) -> anyhow::Result<RefSeeds> {
    match sel {
        RecordSel::EntryPoint(token) => {
            let spec = EntryPointSpec::parse(token)?;
            let (label, seeds) = db.perks_by_entry_point(&spec)?;
            Ok(RefSeeds::Carriers { label, seeds })
        }
        RecordSel::OmodProperty(token) => {
            let spec = OmodPropertySpec::parse(token)?;
            let (label, seeds) = db.omods_by_property(&spec)?;
            Ok(RefSeeds::Carriers { label, seeds })
        }
        RecordSel::Edid(edid) => match resolve_sel(db, sel) {
            Ok(fid) => Ok(RefSeeds::Direct(fid)),
            Err(edid_err) => {
                // OMOD-property names are deliberately flag-only: short,
                // generic names collide with real EditorIDs and hardcoded
                // AVIF records (`Health` is both). Never add a property-name
                // fallback here; see docs/adr/0004-refs-seed-selectors.md.
                // Unlike the explicit `--entry-point` path above, a parse
                // failure here (e.g. `edid` happens to look hex-prefixed,
                // which `RecordSel::Edid` shouldn't produce in practice) is
                // just another way to *not* be an entry point — fold it into
                // the same "neither interpretation matched" message rather
                // than surfacing `EntryPointSpec::parse`'s FormID-specific
                // wording, which would be confusing here.
                let carriers = EntryPointSpec::parse(edid)
                    .ok()
                    .and_then(|spec| db.perks_by_entry_point(&spec).ok())
                    .filter(|(_, seeds)| !seeds.is_empty());
                match carriers {
                    Some((label, seeds)) => Ok(RefSeeds::Carriers { label, seeds }),
                    None => bail!(
                        "'{edid}' did not resolve as EditorID ({edid_err:#}) or as a \
                         PERK entry point (no carriers matched)"
                    ),
                }
            }
        },
        _ => Ok(RefSeeds::Direct(resolve_sel(db, sel)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── Minimal synthetic-ESM byte builders ──────────────────────────────
    //
    // Colocated unit tests live inside the `esm` crate itself, so they can't
    // pull in `tests/common/mod.rs`'s helpers of the same name the way an
    // integration test under `tests/` does (that file's `use esm::Database`
    // et al. only resolve for a crate depending on `esm` externally). These
    // mirror that file's byte-level conventions (and `tests/refs.rs`'s own
    // local copies of them) with `crate::` paths instead.

    const TEST_FORM_VERSION: u16 = 208;

    fn append_subrecord(out: &mut Vec<u8>, sig: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(sig);
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
    }

    fn edid_bytes(name: &str) -> Vec<u8> {
        let mut v = name.as_bytes().to_vec();
        v.push(0);
        v
    }

    fn append_record(out: &mut Vec<u8>, sig: &[u8; 4], form_id: u32, subrecords: &[u8]) {
        out.extend_from_slice(sig);
        out.extend_from_slice(&(subrecords.len() as u32).to_le_bytes()); // data_size
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&form_id.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // vcs1
        out.extend_from_slice(&TEST_FORM_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // vcs2
        out.extend_from_slice(subrecords);
    }

    fn wrap_grup(label: &[u8; 4], records: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let group_size = (24 + records.len()) as u32;
        buf.extend_from_slice(b"GRUP");
        buf.extend_from_slice(&group_size.to_le_bytes());
        buf.extend_from_slice(label);
        buf.extend_from_slice(&0i32.to_le_bytes()); // group_type = 0 (top-level)
        buf.extend_from_slice(&0u32.to_le_bytes()); // stamp
        buf.extend_from_slice(&0u32.to_le_bytes()); // unknown
        buf.extend_from_slice(records);
        buf
    }

    fn tes4_header() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"TES4");
        buf.extend_from_slice(&0u32.to_le_bytes()); // data_size
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags (unset Localized bit)
        buf.extend_from_slice(&0u32.to_le_bytes()); // form_id
        buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
        buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
        buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2
        buf
    }

    fn unique_temp_path(stem: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "fo76_esm_refs_unit_{stem}_{}_{n}.esm",
            std::process::id()
        ))
    }

    fn write_and_open(buf: &[u8], stem: &str) -> (PathBuf, Database) {
        let tmp = unique_temp_path(stem);
        {
            let mut f = std::fs::File::create(&tmp).expect("create temp esm");
            f.write_all(buf).expect("write temp esm");
        }
        let db = Database::open(&tmp).expect("open db");
        (tmp, db)
    }

    // ── resolve_ref_seeds ─────────────────────────────────────────────────

    /// A plain EditorID selector resolves to `RefSeeds::Direct` — the single
    /// FormID it names, per ADR-0004's Direct shape. The target itself is
    /// never emitted as a row (only its referencers are); `resolve_ref_seeds`
    /// only has to hand back the seed FormID.
    #[test]
    fn resolve_ref_seeds_direct_selector_resolves_edid_to_single_target() {
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &edid_bytes("TestWeap"));
        let mut rec = Vec::new();
        append_record(&mut rec, b"WEAP", 1, &subs);
        let mut buf = tes4_header();
        buf.extend(wrap_grup(b"WEAP", &rec));

        let (path, mut db) = write_and_open(&buf, "resolve_ref_seeds_direct");

        let seeds = resolve_ref_seeds(&mut db, &RecordSel::Edid("TestWeap".to_string()))
            .expect("resolve_ref_seeds");
        match seeds {
            RefSeeds::Direct(fid) => assert_eq!(fid, FormId(1)),
            RefSeeds::Carriers { .. } => panic!("expected Direct, got Carriers"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// An `--entry-point`/`--ep` selector (`RecordSel::EntryPoint`) resolves to
    /// `RefSeeds::Carriers` — every PERK declaring that entry point, per
    /// ADR-0004's Carriers shape, each carried alongside its `CarrierTag`s.
    #[test]
    fn resolve_ref_seeds_carriers_selector_resolves_entry_point_to_seeds() {
        // One PERK carrying entry point id 39 ("Mod Percent Blocked"): a PRKE
        // (Effect Type=2 "Entry Point", Rank=0) + DATA (id, Function=0, Perk
        // Condition Tab Count=0, unused=0) + empty PRKF trailer — same shape
        // `tests/refs.rs::build_perk_entry_points` verifies against real
        // `esm get` output byte-for-byte.
        let mut subs = Vec::new();
        append_subrecord(&mut subs, b"EDID", &edid_bytes("CarrierPerk"));
        append_subrecord(&mut subs, b"DATA", &[0x01, 0x00, 0x01]); // top-level Data
        append_subrecord(&mut subs, b"PRKE", &[0x02, 0x00]);
        append_subrecord(&mut subs, b"DATA", &[39, 0x00, 0x00, 0x00]);
        append_subrecord(&mut subs, b"PRKF", &[]);
        let mut rec = Vec::new();
        append_record(&mut rec, b"PERK", 10, &subs);
        let mut buf = tes4_header();
        buf.extend(wrap_grup(b"PERK", &rec));

        let (path, mut db) = write_and_open(&buf, "resolve_ref_seeds_carriers");

        let seeds = resolve_ref_seeds(&mut db, &RecordSel::EntryPoint("39".to_string()))
            .expect("resolve_ref_seeds");
        match seeds {
            RefSeeds::Carriers { label, seeds } => {
                assert!(label.contains("39"), "unexpected label: {label}");
                assert_eq!(seeds.len(), 1, "expected exactly one carrier: {seeds:?}");
                assert_eq!(seeds[0].0, FormId(10));
            }
            RefSeeds::Direct(_) => panic!("expected Carriers, got Direct"),
        }

        let _ = std::fs::remove_file(&path);
    }

    // ── merge_tags ────────────────────────────────────────────────────────

    /// `merge_tags` unions by `(kind, scope, id)` identity, not full struct
    /// equality — a duplicate tag with a different `name` still collapses to
    /// one entry (whichever sorts first by the derived `Ord`, since `dedup_by`
    /// runs after a full-struct `sort`) — and keeps tags with a distinct
    /// `(kind, scope, id)`. This is the equal-depth carrier-tag union
    /// `referenced_by_walk` relies on when a record is reachable from two
    /// different carriers at the same hop depth.
    #[test]
    fn merge_tags_unions_by_kind_scope_id_and_dedupes() {
        let tag_a = CarrierTag {
            kind: crate::CarrierKind::EntryPoint,
            id: 39,
            name: Some("Mod Percent Blocked".to_string()),
            scope: None,
        };
        let tag_a_again = CarrierTag {
            kind: crate::CarrierKind::EntryPoint,
            id: 39,
            name: None, // same (kind, scope, id) as tag_a, different name
            scope: None,
        };
        let tag_b = CarrierTag {
            kind: crate::CarrierKind::EntryPoint,
            id: 40,
            name: None,
            scope: None,
        };

        let mut dst = vec![tag_a.clone()];
        merge_tags(&mut dst, &[tag_a_again, tag_b.clone()]);

        assert_eq!(dst.len(), 2, "expected exactly 2 distinct tags: {dst:?}");
        let ep39: Vec<&CarrierTag> = dst
            .iter()
            .filter(|t| t.kind == crate::CarrierKind::EntryPoint && t.id == 39)
            .collect();
        assert_eq!(
            ep39.len(),
            1,
            "the two id-39 tags must collapse to exactly one entry: {dst:?}"
        );
        assert!(dst.contains(&tag_b));
    }

    #[test]
    fn merge_tags_is_a_noop_on_empty_incoming() {
        let mut dst = vec![CarrierTag {
            kind: crate::CarrierKind::OmodProperty,
            id: 1,
            name: None,
            scope: Some("weap".to_string()),
        }];
        let before = dst.clone();
        merge_tags(&mut dst, &[]);
        assert_eq!(dst, before);
    }

    // ── ref_path_hop ──────────────────────────────────────────────────────

    /// `ref_path_hop` always decodes the node's own row (form_id/record_type/
    /// editor_id/name), but only computes `field_paths` when a `predecessor`
    /// is supplied — the `None` case is what `find_ref_path`'s first hop
    /// (which has no predecessor to point at) and a `--paths`-less search use.
    #[test]
    fn ref_path_hop_annotates_field_paths_only_when_predecessor_given() {
        // WEAP(1): target, no subrecords. WEAP(2): referencer, YNAM -> WEAP(1)
        // (same layout `tests/common::make_xref_esm` uses).
        let mut ref_subs = Vec::new();
        append_subrecord(&mut ref_subs, b"YNAM", &1u32.to_le_bytes());

        let mut recs = Vec::new();
        append_record(&mut recs, b"WEAP", 1, &[]);
        append_record(&mut recs, b"WEAP", 2, &ref_subs);
        let mut buf = tes4_header();
        buf.extend(wrap_grup(b"WEAP", &recs));

        let (path, mut db) = write_and_open(&buf, "ref_path_hop");

        let hop_no_pred = ref_path_hop(&mut db, FormId(2), None).expect("ref_path_hop");
        assert_eq!(hop_no_pred.form_id, FormId(2).display());
        assert_eq!(hop_no_pred.record_type.as_deref(), Some("WEAP"));
        assert!(
            hop_no_pred.field_paths.is_none(),
            "field_paths must stay None without a predecessor"
        );

        let hop_with_pred =
            ref_path_hop(&mut db, FormId(2), Some(FormId(1))).expect("ref_path_hop");
        assert_eq!(
            hop_with_pred.field_paths,
            Some(vec!["Sound - Pickup".to_string()]),
            "expected the YNAM ('Sound - Pickup') field path pointing at FormId(1): {:?}",
            hop_with_pred.field_paths
        );

        let _ = std::fs::remove_file(&path);
    }
}
