"""`python3 -m pn <verb> [args]`: one entry point for every pipeline stage.

Each verb is a stage module's `main(argv)`; `python3 -m pn <verb> --help`
lists its arguments.
"""

from __future__ import annotations

import sys

from pn import (
    build_bundles,
    check_claims,
    check_coverage,
    discord_chunker,
    fetch_official_notes,
    make_patch_notes,
    render_comprehensive,
    run_lints,
    slice_bundles,
    triage_bundles,
    update_manifest,
    workflow,
)

VERBS = {
    "prepare": (workflow.prepare, "skill step: snapshots, pipeline (or reuse), cache, triage, slices"),
    "merge-assessment": (workflow.merge_assessment, "skill step: fold the assessor's tiers in, re-slice"),
    "gate": (workflow.gate, "skill step: claims + coverage over the drafts (--summary: and summary)"),
    "publish": (workflow.publish, "skill step: validate the review, chunk for Discord, record manifest"),
    "run": (make_patch_notes.main, "mechanical stage: esm diff through manifest.json"),
    "render": (render_comprehensive.main, "diff.json -> comprehensive.json"),
    "bundles": (build_bundles.main, "comprehensive.json -> bundles.json"),
    "lints": (run_lints.main, "lint checks -> lints.json"),
    "triage": (triage_bundles.main, "tier bundles into work/ (or merge an assessment)"),
    "extract": (slice_bundles.main, "print records from comprehensive.json"),
    "claims": (check_claims.main, "gate: re-derive every drafted claim"),
    "coverage": (check_coverage.main, "gate: every DEEP bundle covered or cut"),
    "chunk": (discord_chunker.main, "patch-summary.md -> Discord chunks"),
    "manifest": (update_manifest.main, "record the narrative stage in manifest.json"),
    "fetch-notes": (fetch_official_notes.main, "official patch notes -> plain text"),
}


def usage() -> str:
    width = max(map(len, VERBS))
    lines = ["usage: python3 -m pn <verb> [args]", "", "verbs:"]
    lines += [f"  {verb:<{width}}  {summary}" for verb, (_, summary) in VERBS.items()]
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if not argv or argv[0] in ("-h", "--help"):
        print(usage())
        return 0 if argv else 2
    verb, rest = argv[0], argv[1:]
    if verb not in VERBS:
        print(f"unknown verb {verb!r}\n\n{usage()}", file=sys.stderr)
        return 2
    return VERBS[verb][0](rest) or 0


if __name__ == "__main__":
    sys.exit(main())
