# FO76-Tools — Backlog

Tracked, actionable work lives in [GitHub Issues](https://github.com/Mapekz/FO76-Tools/issues)
(`gh`, see `docs/agents/issue-tracker.md`) — that's the live, priority-triaged queue as of
2026-07-22. This file is the complement: informal dated notes not yet promoted to an issue,
scope decisions deliberately *not* tracked as work items, and a historical record of resolved
or removed items, grouped under the project they belong to. Do not reintroduce per-project
`todos.md` files or a `todos/` directory — this stays the one file.

Scope checks are dated so a stale claim is obvious on sight.

---

## `esm/`

No tracked follow-ups. Outstanding items as of 2026-07-22 were migrated to GitHub Issues:
[#2](https://github.com/Mapekz/FO76-Tools/issues/2) (diff Changed-count investigation, P4),
[#3](https://github.com/Mapekz/FO76-Tools/issues/3) (INFO BNAM LString mislabeling, P5),
[#4](https://github.com/Mapekz/FO76-Tools/issues/4) (chatbot front page, P6),
[#5](https://github.com/Mapekz/FO76-Tools/issues/5) (server-side subtree filter, conditional),
[#6](https://github.com/Mapekz/FO76-Tools/issues/6) (chase/walk N-API/HTTP/MCP exposure,
conditional).

The 2026-07-22 architecture review left eight further findings unimplemented, all filed rather
than tracked here: [#7](https://github.com/Mapekz/FO76-Tools/issues/7) (delete `slice_bundles.py`
Mode 1), [#8](https://github.com/Mapekz/FO76-Tools/issues/8) (daemon-lifecycle seam),
[#9](https://github.com/Mapekz/FO76-Tools/issues/9) (N-API envelope typing),
[#10](https://github.com/Mapekz/FO76-Tools/issues/10) (point N-API diff at `diff_pair`),
[#11](https://github.com/Mapekz/FO76-Tools/issues/11) (`run_lints` silent per-record swallows),
[#12](https://github.com/Mapekz/FO76-Tools/issues/12) (`extract.py` has no tests),
[#13](https://github.com/Mapekz/FO76-Tools/issues/13) (scalar decode duplication),
[#14](https://github.com/Mapekz/FO76-Tools/issues/14) (`hardcoded.py` duplicates `reader.rs`),
[#15](https://github.com/Mapekz/FO76-Tools/issues/15) (clustering knobs split JSON/Python),
[#16](https://github.com/Mapekz/FO76-Tools/issues/16) (vitest excludes `.tsx`),
[#17](https://github.com/Mapekz/FO76-Tools/issues/17) (`expand_call` branches vs a data table).

One review finding was deliberately **not** filed: the seven IPC methods carved out of
`esm-viewer`'s `CONTRACT` table and hand-written in three places. The carve-out is documented
and intentional (those methods have non-uniform shapes — `diff` needs two registry lookups), so
it reads as a considered partial refactor rather than an oversight.

---

## `ba2/`

No tracked follow-ups.

Note: DX10 texture archives are **deliberately** detected and rejected — that is a documented
invariant (GNRL-only), not a gap. Adding DX10 support needs an explicit design and a separate
code path, so it is not a backlog item.

---

## `esm-viewer/`

No tracked follow-ups.

---

## Cross-cutting

No tracked follow-ups.

The one cross-project seam is `esm-viewer/` → `esm/bindings/napi` (the `@fo76/esm-napi` addon,
a local `file:` dependency). Anything that changes the `EsmDatabase` N-API surface has to land
on both sides — run `just gen-types` in `esm/` to regenerate the shared TypeScript DTOs. As of
2026-07-22 the drift guard runs in CI too, so forgetting the regen now fails the build rather
than passing silently.

Worth knowing about that seam: `dispatch_op` serializes typed Rust structs, and `ts-rs` derives
the generated `.ts` DTOs from those same structs, so the shapes are honest. But every N-API
method returns `serde_json::Value`, which NAPI-RS renders as `any` in
`bindings/napi/index.d.ts` — so `npm run typecheck` cannot actually check `Fo76Api`'s
hand-written assertions against reality. Typing the *envelopes* (`FileInfo`, `RecordResult`,
`DiffResult`, `RefList`) at that boundary would close the gap; record *bodies* are
schema-driven and legitimately stay `Record<string, unknown>`. Tracked as
[#9](https://github.com/Mapekz/FO76-Tools/issues/9).

---

## Resolved 2026-07-22 — architecture deepening pass

An architecture review surfaced eight deepening opportunities; five landed together. All were
verified against the full check suite (`just check`, the Python suite, and the esm-viewer
checks) both individually and after merge.

- **P4 — `--json` stdout hygiene in daemon mode** — resolved. Root cause was not the daemon at
  all: `main()` ran the requested subcommand and then *fell into* `run_repl` whenever `-p` was
  absent, and `run_repl` wrote its `esm> ` prompt to the same stdout handle the JSON had just
  gone to. Fixed by exiting after a subcommand regardless of `-p`, and moving the prompt to
  stderr (matching the existing precedent that capped-output notices go to stderr so `--json`
  stays parseable). Landed with the CLI enum unification.
- **CLI/REPL dual command enums** — `ReplCommand` deleted; one enum and one dispatch path now
  serve both one-shot and REPL invocation. `chase` and `walk` are reachable from the REPL for
  the first time (they had no `ReplCommand` variant, so the *default* mode couldn't reach them).
- **`decode.rs` split** — 4465 lines separated into `decode/{mod,scope,vmad,rules}.rs`. The
  generic engine no longer carries FO76 business rules inline; `MemberDef` gained
  `sig()`/`contains_sig()`, collapsing three duplicated variant matches. No behaviour change.
- **Legacy HTTP routes bypassing `ipc::dispatch_op`** — routed through the canonical surface.
  This also fixed a latent **self-deadlock**: `diff_route` took two registry handles with no
  same-database check, so comparing a file against itself locked the same non-reentrant
  `Mutex` twice. Pre-lock policy now lives in one shared `diff_pair` helper.
- **`esm coverage --gate` gated on only one of four markers** — now also fails on `unmapped`
  and `unknown_record`. `_unresolved` is deliberately still excluded: per
  `tests/decode_coverage.rs`, it signals a missing localization BA2, not a decode bug. Note
  this gate still cannot run in CI (it needs a real ESM, and game data is gitignored), so it
  remains a local-only check.
- **P4 — `rollout_shapes` rides on a dict subclass** — resolved, along with the wider problem
  it was a symptom of. The pipeline had no declared shape for anything crossing its ten stages
  (no `TypedDict`, `dataclass`, or schema anywhere), so a renamed key degraded to `None` rather
  than raising. `RecordEntry`/`Bundle`/`Member`/`Edge`/`TierInfo`/`LintFinding` are now declared
  in `patchnotes_lib.py`, and — because the stages that matter cross a *process* boundary
  (`triage_bundles.py` is a separate process, `slice_bundles.py --extract` a subprocess, and the
  `/patch-notes` skill an LLM reading files) — validation runs where JSON re-enters a process,
  which is the part a `TypedDict` structurally cannot cover. `compute_bundle_tiers` now returns
  `(tiers, rollout_shapes)` explicitly and the `getattr` default is gone.
  `tools/tests/fixtures/comprehensive_mini.json` was deliberately **not** regenerated — it is
  hand-engineered to hit hub-exemption, oversized-split and orphan-singleton paths a real diff
  fixture wouldn't reproduce; a conformance test against the real producer was added instead.
- **Python lint/typecheck** — `ruff` and `ty` now run in CI and via `just patch-tools-lint`,
  both pinned (`ruff@0.15.22`, `ty@0.0.62` — the latter is a 0.0.x preview). They run through
  `uvx` and are never imported, so `esm/tools/` keeps its zero-runtime-dependency property: no
  `requirements.txt`, no lockfile, and the suite still runs on bare `python3`.
- **CI enforced what `just` already ran** — `cargo test --features server`, the generated-types
  drift guard, the Python tooling suite (529 tests), and the esm-viewer typecheck + vitest
  (77 tests) are now CI jobs. Previously ~19k lines of Python and ~4.3k of TypeScript had zero
  CI enforcement, and the one guard keeping the N-API seam honest fired only if a human
  remembered to run `just` in `esm/`. Two false comments were corrected at the same time:
  `ci.yml` claimed a single `#[ignore]`-gated game-data test (there are two, and the repo has
  no `#[ignore]` attributes at all), and `esm/justfile` claimed to mirror CI exactly.

## Resolved 2026-07-20

- **P3 — CNDF condition decoding (RA_SCORE stub suspicion)** — resolved as hypothesis (a):
  the records are genuine live-service placeholders; decoder and TES5Edit definitions are
  both correct. Evidence: (1) surveyed the 1,051 CNDF records in 20260717 — non-SCORE CNDFs
  decode into varied, parameterized, sensible conditions (GetValue, HasKeyword,
  HasLearnedRecipe, WornHasKeyword, nested `IsTrueForConditionForm`, …), which validates the
  32-byte CTDA layout and the FO76 function-index table in CNDF context; (2) raw dump of
  0x0086A8A5 shows exactly EDID + 64 CTDA subrecords (no CIS1/CIS2, no CITC; subrecord sizes
  sum precisely to the record's data_size of 2484), all 64 byte-identical — identical bytes
  cannot encode varied semantics under *any* layout, so no alternative decoding exists;
  (3) the stub family is exactly the three adjacent records 0x0086A8A5/A6/A7
  (`RA_SCORE_ContextualSeason{CollectAll,ConsumableSafe,ItemJunkSafe}_Condition`, 64/14/28
  rows), referenced only from `LLE_Safe_*` leveled-list entry filters via
  `IsTrueForConditionForm` — season-contextual safe loot, whose real conditions live in
  Bethesda's server-side data (FO76 loot rolls are server-authoritative). The
  `GetDead == 1.0, OR` rows even share the healthy-row idiom (Param3 = -1 sentinel), i.e.
  tool-authored slot padding, always-false client-side. Follow-up cross-check (same day):
  31 MGEFs use function index 0x2E in semantically-known death contexts — the recon scope
  highlights living targets (`GetDead == 0`), stun effects guard `Run On Target, GetDead == 0`,
  and `EN07_ApplyVaporizeVisualEffectEffect` gates its goo-pile visual on
  `GetDead == 1.0 OR GetDying == 1.0` with a CTDA row **byte-identical** to the challenge
  stub — so index 46 = GetDead is confirmed at the raw-byte level and the stub rows are
  literal copies of a standard death-check condition. No code or schema change needed.

## Removed 2026-07-14

Two items were dropped as no longer worth carrying (full write-ups in git history at 668ceb1):

- **FPC compile-and-introspect schema extraction** — its motivating problem (unreachable
  closure deciders) was solved by runtime `ByteAtOffset` branching; the Python parser covers
  181 record types with 0 failures and `coverage --gate` holds the line. If the parser breaks
  on a future TES5Edit revision, that failure is the signal to reconsider.
- **Curve-table on-disk cache** — the warm daemon keeps `CurveIndex` resident, so only cold
  in-process opens (`get --startup-ba2`) would benefit. Revisit only if cold-open latency
  starts mattering.
