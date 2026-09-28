# Patch notes

Scoped guidance for the Python patch-notes pipeline. Shared policy and validation mapping live
in [../AGENTS.md](../AGENTS.md).

## Build and validation

Run commands from `patch-notes/`; `justfile` owns the recipes.

- `just` runs `test` (the hermetic unittest suite: no game data, no `esm` binary) and `lint`
  (ruff + ty, pinned to CI's versions). CI runs both.
- `just run OLD NEW` runs the mechanical stage; the `/patch-notes` skill drives the whole run.
- Every stage is a verb: `python3 -m pn <verb>` from here, or `python3 patch-notes/cli.py
  <verb>` from anywhere; `python3 -m pn --help` lists them.
- `pn/` is stdlib-only at runtime. `pn/esmcli.py` finds the `esm` binary at
  `../target/release/esm` (the workspace target), then on `PATH`.
- Tests import `pn` and `tests` as packages; a stage that talks to `esm` takes its gateway as a
  `client=` argument, so tests pass `tests/fake_gateway.FakeGateway` and production code never
  imports `tests/`.

## Layout

| Path | Purpose |
|---|---|
| `pn/` | The package: one module per stage, `__main__.py` (verbs), shared `esmcli` (the `esm` gateway), `formids`, `jsonio`, `layout` (artifact paths), `schemas` (every artifact's shape and strict validator, agent-written ones included, and `PIPELINE_VERSION`), `patchnotes_lib`; the tier rules (`patch_notes_tiers.json`); bundling tunables are `build_bundles.DEFAULT_SETTINGS` |
| `cli.py` | Launcher for running verbs from outside `patch-notes/` |
| `tests/` | unittest suite, `fixtures/`, `builders.py`, and `fake_gateway.py` (the fixture-backed gateway) |
| `skill/` | The `/patch-notes` skill: `SKILL.md`, the writer and review prompts, `style-guide.md`, and `kb/` |

## Pipeline

The patch-notes pipeline has a **mechanical stage** (deterministic Python, no LLM) and a
**narrative stage** (the `/patch-notes` skill in `skill/`). The mechanical stage is `pn run`
(`pn/make_patch_notes.py`), in a fixed order:

```
esm diff (subprocess)                 → diff.json
  │
render_comprehensive.py               → comprehensive.json
  │   uses change_entries.py's ChangeEntry construction + array-diff reading
  ▼
build_bundles.py                      → bundles.json
  │   clusters related records (weapon + mod slots + drop list + unique keyword)
  ▼
run_lints.py                          → lints.json (each lint names its bundle_id)
  │   rule registry, consults esmcli's EsmGateway for reference-graph checks
  ▼
patchnotes_lib.py manifest helpers    → manifest.json
```

`triage_bundles.py` then assigns each bundle a tier — `rollout`, `deep`, `brief`, `drop`, or
`ambiguous` — against `patch_notes_tiers.json`'s rules, writing `work/triage.json`,
`work/deep-slice.json`, `work/ambiguous.json`, `work/brief-lines.md`, and `work/rollouts.md`.
`esmcli`'s `EsmGateway` is the one seam every stage uses to reach the `esm` CLI — `bulk_get`,
`list_type`, `refs`, `diff` — so nothing else in `pn/` shells out to `esm`, apart from the
cache build in `workflow.prepare`.

The skill drives everything through four verbs in `pn/workflow.py`, each printing a JSON
summary on stdout: `prepare` (snapshot resolution, the reuse check, the mechanical stage,
the new snapshot's cache, triage, and the DEEP slices, split in two above 20 bundles),
`merge-assessment` (the assessor's tiers, then re-slicing), `gate` (`check_claims.py`
re-derives every number a writer claimed, from `comprehensive.json` or live `esm` lookups;
`check_coverage.py` asserts every DEEP bundle id is covered by exactly one draft and, with
`--summary`, reaches the summary or `work/cuts.json`), and `publish` (validates the review,
chunks `patch-summary.md` for Discord, records the narrative stage in the manifest).
Between them, 1-2 deep-writer agents armed with `deep-writer-prompt.md`/`style-guide.md`/`kb/`
write the DEEP tier, one assessor resolves the `ambiguous` tier, and a cold reviewer reads the
result. `fetch_official_notes.py` extracts the newest section of an official patch-notes page
for the discrepancy callouts.

## Where to tweak what

| Want to... | Look in |
|---|---|
| Add a new lint rule | `pn/run_lints.py`'s rule registry |
| Change bundle clustering | `pn/build_bundles.py` |
| Change tier assignment (DEEP/BRIEF/DROP) | `pn/patch_notes_tiers.json`, `pn/triage_bundles.py` |
