# `--resolve stub` inlines value-bearing leaf types (GLOB, CURV) onto the reference

Status: accepted (2026-08-27)

`--resolve stub` annotates a FormID reference with `{formid, editor_id, record_type}` — enough to
identify the target, never enough to read its value. For a GLOB reference (a tunable constant —
magnitudes, durations, required counts, condition thresholds) that meant `--resolve full`
(recursive, unbounded) or a second `get` just to read one float, and three call sites
(`src/walk/mod.rs`'s `resolve_glob_ref`/`resolve_condition_row`, `src/lvli.rs`'s
`resolve_glob_value`, `tools/lvli_audit.py`'s second `bulk_get(..., resolve="none")`) paid that
round-trip themselves rather than have `stub` carry it.

CURV already inlines its points onto a reference (`resolve_formid`'s first branch in
`src/decode/mod.rs`), and at *every* resolve depth including `none`. That is not the same
mechanism this ADR adds, even though both end up flat in the output JSON: a CURV record's schema
is only `EDID` plus two file-path strings — its points live in an external curve-table JSON loaded
from the Startup BA2 (`src/curves.rs`), so nothing else can ever surface them, and the inline is
keyed on the *declaring field's* `valid_refs` because no resolver exists at `ResolveDepth::None`,
where it must still fire. A GLOB's `Value` is already inside the record; inlining it is pure
round-trip elimination, and it fires only at `Stub`/`Full`.

## Decision

A small registry (`src/decode/leaf_values.rs::VALUE_BEARING_LEAVES`), keyed by the **target
record's own signature**, not the declaring field's `valid_refs`. Two rows today:

- `GLOB` → lift `["Value"]` flat onto the stub. Requires a resolver, so fires only at
  `ResolveDepth::Stub`/`Full`.
- `CURV` → the existing curve-index projection (`{formid, editor_id, curve_path, curve}`, no
  `record_type`). Unchanged shape and unchanged depth-independence; the table's row for it is
  consulted from `resolve_formid`'s existing `valid_refs`-keyed branch, not from the resolver path.

A type earns a row only if it passes three tests: bounded scalar payload (not FLST's unbounded
list, not AVIF's 13 members plus prose); the EditorID doesn't already tell the reader the value
(ruling out KYWD, whose decoded body is an editor swatch and whose EditorID *is* the semantics);
and it's actually referenced by other records (ruling out GMST, a perfect scalar leaf nothing can
point at — 0 real referrers, verified via `esm refs` against a live snapshot). The test admits more
than the two shipped rows (ZOOM was identified as a strong candidate — 803 real WEAP/OMOD
references, and its `FOV Mult` field is actively misleading relative to its EditorID, e.g.
`ZM_Standard_Scope_x4` → `2.0`) but adding it was deferred; the test is written down so a future
row is a one-line addition, argued on its own merits.

Keying on the target's signature (rather than `valid_refs`) is why this reaches GLOB references
inside CTDA conditions and VMAD script properties: those call sites
(`src/ctda.rs:280,312,358`, `src/decode/vmad.rs:247,746`) pass an empty `valid_refs` slice, which a
`valid_refs`-keyed rule could never see — and CTDA operands are exactly where GLOB comparison
values live. The same keying also fixes `PROJ."Speed Curve Table"`/`"Seek Strength Curve Table"`
(empty `valid_refs` in the schema, bare hex today) and starts inlining CURV points on ~53 VMAD
script-property references that were bare hex before (real cost: ~10 KB on the worst sampled QUST
— accepted).

### Why GLOB's inline is gated to `Stub`/`Full`, not unconditional like CURV's

Two reasons hold, and a third — the one that looked most likely going in — turned out not to:

1. **Contract.** The MCP tool description promises `none` "leaves raw hex FormIDs unchanged."
2. **Cost.** `Database::record_at_meta_with_depth` skips constructing a resolver at all when
   `depth == ResolveDepth::None`. Firing at `None` would force one on every such decode —
   including `diff::run`, which decodes both snapshot sides at `None` and can process tens of
   thousands of records in a single call — for callers who never asked for resolution.
3. ~~Diff re-stamp noise (issues #18/#22).~~ **Measured, and false.** `diff::run` skips
   byte-identical records before either side is decoded, so a GLOB value change cannot cascade
   into unrelated referencing records that didn't themselves change. Across all seven
   `Data/notes/*_to_*` snapshot diffs on disk at the time of this change, changed GLOB records
   numbered 0–6 per week, CURV records never changed, and the intersection of *changed GLOBs'
   referrers* with *that week's own changed-record set* was zero, every week (e.g.
   `UniqueWeaponSkinDropChance`: 5 referrers, 0 in the changed set; three
   `ContextualAmmoReward_Count_*` GLOBs: 1 referrer each, 0 in the changed set). Unconditional
   inlining would not have reintroduced that noise class in practice — it's excluded on contract
   and cost grounds alone.

### Rejected alternatives

- **Grow `FormIdStub`** (`#[serde(flatten)]` extras, or a fourth field) instead of a resolver
  method. Rejected: `FormIdStub` is `ts_rs`-exported to `esm-viewer`'s generated TypeScript, and is
  also the hot path for `UnionDecider::FormIdTargetType`, which wants only `record_type`. A new
  defaulted `FormIdRefResolver::leaf_inline` method costs zero call sites and zero generated-type
  churn.
- **Unify CURV's and GLOB's firing rule** (either always-on including GLOB, or gated including
  CURV). Both were considered and rejected: gating CURV would break `tests/curves.rs` and the
  documented invariant that a CURV reference's points survive at any depth; firing GLOB
  unconditionally fails the contract/cost reasons above even though the noise reason turned out
  moot. The two rows are not interchangeable, and the module doc says so rather than pretending
  otherwise.
- **A flat `Value` on `Full` resolution too.** Rejected: a genuine `Full` expansion already carries
  the value at `fields.Value`; duplicating it at the top level would erase the one shape signal
  that distinguishes a real expansion from `decode_full`'s two stub-shaped fallbacks (depth limit,
  index miss) — which now *do* route through the same leaf-inline path, so `--resolve full` is
  never less informative than `--resolve stub` for a value-bearing leaf reached past hop 0.
- **Memoizing resolved leaves.** Rejected: `DatabaseResolver::stub` already calls
  `parse_record_at`, which decompresses and splits every subrecord out of the target record — the
  incremental cost of a leaf inline is one schema decode over a record with at most a handful of
  members. Measured worst case: 43 GLOB references on a single LVLI record. A cache would mean
  giving `DatabaseResolver` (a `Send + Sync` trait object) interior mutability for a cost this
  small.

## Consequences

- Three round-trip workarounds are deleted: `src/walk/mod.rs`'s `resolve_glob_ref`/
  `resolve_condition_row` (and the `collect_condition_refs`/`collect_ref_formids` helpers that
  existed only to feed them), `src/lvli.rs`'s `resolve_glob_value` and its batched `*_Global`/
  condition-ref prefetch, and `tools/lvli_audit.py`'s second `bulk_get(..., resolve="none")` plus
  `collect_glob_refs`. Each of these read a stub, then paid a second fetch to learn what `stub`
  now already says.
- `esm walk --json`'s digest payload changes: the injected key renames from `"resolved_value"` to
  `"Value"`, matching the wire shape everywhere else. Confirmed confined to `src/walk/`,
  `tools/lvli_audit.py`, and one generated TypeScript doc comment (`MagicEffectRow.ts`, regenerated
  via `just gen-types`) — absent from `src/chase.rs`, so this does not touch the `chase` JSON
  pipeline contract ADR 0001 established.
- `esm-viewer` does not benefit yet: `bindings/napi` defaults to `ResolveDepth::None`, and
  `RecordTable.tsx`'s `isFormIdStub` rendering discards any key beyond `formid`/`editor_id`/
  `record_type`. Filed as a follow-up issue, not fixed here — it needs a viewer-side resolve
  toggle, which is its own design question.
