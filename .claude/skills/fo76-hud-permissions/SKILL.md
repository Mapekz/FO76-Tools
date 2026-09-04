---
name: fo76-hud-permissions
description: Use when deciding whether Fallout 76 HUD files from a mod host, public repository, or vanilla game data may be inspected, modified, derived, packaged, published, or redistributed.
---

# HUD artifact permissions

Make rights a per-artifact, per-action decision and fail closed when evidence is missing. A public
page, successful download, local possession, educational intent, or repository clone never grants
permission to copy, modify, derive, package, publish, or redistribute.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip a rights gate.

## Decision procedure

1. Identify every input and generated artifact, including embedded SWFs, textures, fonts, sounds,
   framework code, decompiler output, and vanilla/game files.
2. Record the exact source URL, author/rightsholder, mod/file IDs, version, source hash, license or
   written grant, requested action, conditions, attribution, reviewer, and review date.
3. Assign a status per artifact and action. `inspect-only`, `unknown`, conflicting evidence, or a
   missing rightsholder fails a release gate; there is no `--force` override.
4. Release only the independently owned or explicitly cleared subset. Generate notices and
   attribution from the ledger; never apply this repository's license to uncleared material.

## Ledger

For every input record origin (`owned`, `vanilla`, or `third_party`), source URL, author/rights
holder, mod/file IDs, source hash, license or permission evidence, requested rights, conditions,
attribution, reviewer, and review date. Keep the ledger outside the downloadable corpus when it
contains private correspondence. Use `unknown` or `needs-human-review` when evidence is absent,
ambiguous, conflicting, stale, or only implied by a page.

Default states:

- User-owned source: may be used within the stated ownership scope.
- Third-party mod-host or public-repository asset: `inspect-only`/`unknown` until the exact artifact
  and derivative action are cleared.
- Vanilla/game files: local runtime input only; never commit or redistribute.
- Decompiled source, extracted textures/fonts/sounds, and bundled framework assets: retain only in
  a local research run unless their separate rights are proven.

## Release gate

Modification and redistribution must both be explicitly allowed for each shipped artifact. A
Mod-host permission preset is file-specific and does not automatically cover conversion, derivative
work, asset reuse, or another author's file. Generate attribution and notices from the ledger;
do not label a mixed output with this repository's license.

There is no agent `--force` override. Stop and ask for a human rights decision when the ledger is
not a clear pass. A future package may contain independent code and generated assets, but never
raw FO76 game data or decompiled third-party source by default.

## Red flags

- “Public” or “open source” without an actual license or written grant.
- A license that covers viewing/download but not modification, derivatives, redistribution, or
  relicensing.
- Decompiled code or extracted assets treated as if they inherit the archive's permission.
- A combined package whose most restrictive dependency is not separated.
- “Educational,” “personal,” or “the user owns the workstation” used as a substitute for release
  permission.

## Mod-service/API boundary

Use a personal mod-service API key only for bounded, human-directed actions and send the stable
identifying headers required by that service. Never put a key in argv, source, logs, reports, or a
server. Follow the service's current API policy and terms; they may restrict automated mining and
using site data to develop, train, fine-tune, or validate AI systems. Do not create an agent
training corpus from service data unless an applicable permission or exemption is documented.

Consult the active service's API policy, Terms of Service, file-license rules, and submission
guidelines before automating requests or releasing derived material.
