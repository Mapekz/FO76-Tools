---
name: fo76-hud-research
description: Use when a human-directed Fallout 76 HUD investigation needs bounded mod metadata, local BA2/SWF/config evidence, or a reproducible compatibility report.
---

# FO76 HUD research

Produce a small, reproducible evidence set for a deliberately scoped HUD question. This is a
static research workflow, not a catalog scraper, runtime automation recipe, or source-reuse
permission.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip its evidence or
rights gates.

## Use and stop conditions

Use it for an explicit shortlist, a local archive/SWF comparison, or a compatibility report.
Narrow to metadata-only or stop when the request asks to scrape the catalog, skip provenance or
rights review, execute downloaded content, or create a mod-service-derived agent-training corpus.
**REQUIRED SUB-SKILL:** Use `fo76-hud-permissions` before modification, derivation, packaging,
or redistribution decisions.

## Safety and rights boundary

- Never print, commit, or place credentials in a report. Read only the provider-specific secret
  variable needed for the selected request, either from the inherited environment or an explicitly
  selected env file; never source the whole file.
- Keep downloads, game files, extracted BA2 members, decompiler exports, and signed download
  URLs outside the repository. Do not execute a mod installer, SWF, DLL, game binary, or
  downloaded build script.
- Static inspection is allowed only to understand file formats and interoperability. Do not
  copy decompiled ActionScript, textures, fonts, sounds, or bundled framework assets into this
  repository or repackage them.
- The source service's current terms may prohibit automated text/data mining and using site data to
  develop, train, fine-tune, or validate AI systems. Do not turn downloads or descriptions into an
  agent-training/evaluation corpus without documented permission. Build reusable agent guidance
  from human-permitted interface facts and synthetic fixtures.
- A public repository or a mod-page download is not a license. Route any modification,
  conversion, redistribution, or asset reuse through `fo76-hud-permissions`.

## Bounded workflow

1. Create a run ID and an output directory outside the repo. Record scope, exact game/mod/file
   selectors, source URLs, fetch time, tool versions, and SHA-256 values.
2. For a deliberately small, human-selected candidate set, query the source service's official
   metadata endpoint only when its policy permits it. Preserve feed ranks separately; do not
   invent a site-wide popularity score or enumerate the whole game. Download or inspect only
   explicitly selected items.
3. Use a pinned external archive/SWF inspector for shape-only inspection and manifests. Use a
   pinned BA2 extractor only after choosing an explicit archive/member or reviewed filter; keep
   all output in the external run root.
4. Inspect SWF containers with bounded, read-only tooling. If using an external decompiler,
   allowlist the exact executable, record its version/hash, and export to a fresh run directory.
   Report interfaces and signals, not decompiled source text. For JPEXS/FFDec CLI mechanics,
   read `.claude/skills/fo76-scaleform-ui/references/ffdec-workflow.md` and keep its exports in the
   external run root.
5. Separate `[Confirmed]` facts (hash/path/source revision), `[Deduced]` interpretations, and
   `[Unknown]` runtime behavior. Do not call static evidence runtime certification.
6. Finish with a permissions status and a list of unknowns before proposing a build.

## Quick reference

| Need | Required record |
|---|---|
| Compare packages | `game_domain + mod_id + file_id + source_sha256 + logical_entry_path` |
| Inspect a BA2/SWF | command, tool version, input hash, bounded result |
| Describe a runtime claim | exact build, evidence reference, confidence, or `[Unknown]` |
| Propose a release | per-artifact permission status and unresolved rights |

## Red flags

- “Download everything,” “skip hashes,” or “the key is already available” means the scope is not
  ready; do not turn the request into a bulk script.
- A successful parse, decompiler export, or public download proves neither runtime compatibility
  nor permission to copy, modify, or redistribute.
- Raw decompiler output, game files, signed URLs, cookies, API keys, and third-party payloads do
  not belong in the repository or an agent-training dataset.

## Evidence joins

Use `game_domain + source_item_id + file_id + source_sha256 + logical_entry_path` as the stable
join key. Filenames, titles, author names, and class names are labels only. A distributed package
can be newer than its public source checkout; compare both versions explicitly.

Useful public references:

- [HUD Mod Loader](https://github.com/GitCrazy-wc/hudmodloader)
- [HUDTools instructions](https://github.com/GitCrazy-wc/hudmodloader/wiki/HUDTools-Instructions)
