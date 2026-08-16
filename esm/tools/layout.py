#!/usr/bin/env python3
"""
layout.py — the one module owning the patch-notes pipeline's on-disk output
layout: every artifact filename any pipeline stage reads or writes under an
`OUT_DIR`, as a `snake_case` function taking `out_dir: Path -> Path` (or, for
a bare directory name with no path component, a module-level constant).

Before this module existed, ~20 artifact filenames were pure convention,
hardcoded independently across every stage that touched them (the mechanical
scripts in this directory, plus the `/patch-notes` narrative skill's own
prose in `../.claude/skills/patch-notes/SKILL.md`). Two concrete files
already disagreed as a result (`discord_chunker.py`'s CLI default vs.
`update_manifest.py`'s hardcoded dirname) with no compiler or test to catch
it — see `DISCORD_DIRNAME` below.

Covers three groups of artifacts:

  - **Mechanical stage** (`make_patch_notes.py` + `run_lints.py`):
    `diff.json`, `comprehensive.{json,md}`, `bundles.json`, `lints.json`,
    `manifest.json`.
  - **Triage** (`triage_bundles.py`), under `work/`: `triage.json`,
    `deep-slice.json`, `ambiguous.json`, `brief-lines.md`, `rollouts.md`.
  - **Narrative stage** (the `/patch-notes` skill, an LLM orchestrator —
    no Python here reads or writes these yet, but the layout is only
    genuinely complete, and only usable as SKILL.md's future source of
    truth instead of repeated prose, if it names these too):
    `work/assessment.json` (assessor-subagent output), `work/official-
    notes.txt` (optional pasted-in official patch notes), `drafts/deep[.
    partN].md` + `drafts/deep[.partN].report.json` (deep-writer subagent
    output, optionally split across N parts), `patch-summary.md` (the
    assembled final writeup), `discord/` (chunked-for-Discord output dir).

Python 3, stdlib only.
"""

from __future__ import annotations

from pathlib import Path

#: Discord-chunk output dirname, relative to `out_dir`. Single owner shared by
#: `discord_chunker.py`'s CLI default and `update_manifest.py`'s
#: `DISCORD_DIRNAME` reference -- both import this constant instead of
#: restating the string, so a chunker invocation and manifest update always
#: agree on the directory.
DISCORD_DIRNAME = "discord"


# --------------------------------------------------------------------------
# Mechanical stage (make_patch_notes.py / run_lints.py)
# --------------------------------------------------------------------------


def diff_json(out_dir: Path) -> Path:
    """`esm --local diff --json` output -- the raw sparse diff."""
    return Path(out_dir) / "diff.json"


def comprehensive_json(out_dir: Path) -> Path:
    """`render_comprehensive.py`'s full per-record detail, keyed by FormID."""
    return Path(out_dir) / "comprehensive.json"


def comprehensive_md(out_dir: Path) -> Path:
    """`render_comprehensive.py`'s human-readable rendering of the same data."""
    return Path(out_dir) / "comprehensive.md"


def bundles_json(out_dir: Path) -> Path:
    """`build_bundles.py`'s narrative groupings -- rewritten in place by
    `run_lints.py` once lint findings are attached (`lint_ids`/`bug_watch`)."""
    return Path(out_dir) / "bundles.json"


def lints_json(out_dir: Path) -> Path:
    """`run_lints.py`'s automated lint-check findings."""
    return Path(out_dir) / "lints.json"


def manifest_json(out_dir: Path) -> Path:
    """The pipeline manifest -- mechanical-stage completion + counts, then
    the narrative stage's own section appended by `update_manifest.py`. See
    `patchnotes_lib.load_manifest`/`write_manifest`/`new_manifest`."""
    return Path(out_dir) / "manifest.json"


# --------------------------------------------------------------------------
# Triage (triage_bundles.py), under work/
# --------------------------------------------------------------------------


def work_dir(out_dir: Path) -> Path:
    return Path(out_dir) / "work"


def work_triage_json(out_dir: Path) -> Path:
    """Tier assignment (rollout/deep/brief/drop/ambiguous) + per-bundle
    reasons + summary stats."""
    return work_dir(out_dir) / "triage.json"


def work_deep_slice_json(out_dir: Path) -> Path:
    """DEEP-tier bundles in writer-slice shape (`{"bundles": [...],
    "lints": [...]}`), consumed by the deep-writer subagent(s)."""
    return work_dir(out_dir) / "deep-slice.json"


def work_ambiguous_json(out_dir: Path) -> Path:
    """Compact per-bundle field-diff digests for every `ambiguous`-tier
    bundle -- small enough to paste into one assessor-agent prompt."""
    return work_dir(out_dir) / "ambiguous.json"


def work_brief_lines_md(out_dir: Path) -> Path:
    """Templated one-liners for `brief`-tier bundles, grouped by bucket."""
    return work_dir(out_dir) / "brief-lines.md"


def work_rollouts_md(out_dir: Path) -> Path:
    """One aggregate row per recurring bulk-change shape (`rollout` tier)."""
    return work_dir(out_dir) / "rollouts.md"


# --------------------------------------------------------------------------
# Narrative stage (the /patch-notes skill) -- LLM-produced, no Python
# reader/writer today. Named here so SKILL.md can eventually reference this
# module instead of repeating these paths as prose.
# --------------------------------------------------------------------------


def work_assessment_json(out_dir: Path) -> Path:
    """The one-shot assessor subagent's tier resolution for `ambiguous`
    bundles -- input to `triage_bundles.py --merge-assessment`."""
    return work_dir(out_dir) / "assessment.json"


def work_official_notes_txt(out_dir: Path) -> Path:
    """Optional pasted-in official patch-notes text, used by the deep
    writer(s) to flag `Mismatch`/`Undocumented` claims. Absence is normal --
    not every patch has an official-notes article to diff against."""
    return work_dir(out_dir) / "official-notes.txt"


def drafts_dir(out_dir: Path) -> Path:
    return Path(out_dir) / "drafts"


def _part_suffix(part: int | None) -> str:
    return f".part{part}" if part is not None else ""


def drafts_deep_md(out_dir: Path, part: int | None = None) -> Path:
    """A deep-writer subagent's draft prose. `part`, if given, selects one
    writer's slice of a split DEEP tier (`deep.part1.md`, `deep.part2.md`,
    ...); omitted when a single writer owns the whole tier (`deep.md`)."""
    return drafts_dir(out_dir) / f"deep{_part_suffix(part)}.md"


def drafts_deep_report_json(out_dir: Path, part: int | None = None) -> Path:
    """The deep-writer's structured report alongside its draft prose (e.g.
    `deferred[]` entries the orchestrator must reconcile) -- same `part`
    convention as `drafts_deep_md`."""
    return drafts_dir(out_dir) / f"deep{_part_suffix(part)}.report.json"


def patch_summary_md(out_dir: Path) -> Path:
    """The orchestrator's single assembled writeup, before Discord chunking."""
    return Path(out_dir) / "patch-summary.md"


def discord_dir(out_dir: Path) -> Path:
    """Discord-chunked output directory (`discord_chunker.py`'s output,
    `chunk_NNN.md` files -- names not owned here, only the directory is)."""
    return Path(out_dir) / DISCORD_DIRNAME
