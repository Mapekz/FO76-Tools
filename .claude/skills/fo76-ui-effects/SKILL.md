---
name: fo76-ui-effects
description: Use when designing or analyzing Fallout 76 HUD visual effects, Scaleform/GFx timelines, MovieClip or sprite assets, or Vault Boy-style UI animation.
---

# Fallout 76 UI effects and animation

Build effects from evidence, not from assumptions about Flash Player. A decompiled symbol or
timeline is a research observation; it is not a stable public API, a permission grant, or proof
that the current game build can load a modified asset.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip this skill's
constraints.

**REQUIRED SUB-SKILL:** Use `fo76-hud-permissions` before reusing, modifying, packaging, or
redistributing a game-owned or third-party asset.

**REQUIRED SUB-SKILL:** Use `fo76-scaleform-ui` for any in-game SWF, GFx, input, bridge, or
HUDModLoader runtime decision.

## Workflow

1. Pin the game/platform/build, loader/extender versions, target archive/member, and source SHA-256.
   Keep game files, extracted members, decompiler exports, and previews in an external scratch
   research root. Do not execute SWFs, installers, DLLs, or downloaded build scripts.
2. Inspect the smallest useful surface with static tools. Record SWF signature/version, tags, ABC
   blocks, root symbol, linkage/class name, frame labels/count/rate, display-list children, asset
   references, and unknowns. Use a pinned external static inspector/BA2 tool or decompiler; export
   reports rather than copying decompiled source into this repository.
3. Choose the reuse mode explicitly:
   - **Behavior reference:** reproduce timing, staging, easing, and visual intent with newly authored
     assets. This is the default.
   - **Local-only reuse:** use a game-owned asset only in a private, rights-cleared build or
     external preview. Keep the payload and derivative outside the repository.
   - **Replacement/derivative:** require a permission record, target-specific package, rollback,
     and exact-build validation. A public mod page is not a license.
4. Prefer an authored `MovieClip`/symbol timeline with bounded frame playback, or an authored
   sprite/bitmap sequence when the target runtime proves it. Use `gotoAndPlay`/frame labels only
   when the local SWF evidence shows those labels. A rasterized frame sheet is useful for a desktop
   preview, but does not prove that GFx can reproduce masks, blend modes, hit areas, or script hooks.
5. Use conservative primitives: transforms, alpha, solid fills, and proven vector/bitmap assets.
   Do not assign `.filters` to a display object without positive evidence from the target build.
   Do not assume shaders, gradients, Pixel Bender,
   or `fl.motion.*` work.
6. Bound timers/frame loops, texture sizes, retained frames, and redraws. Stop playback and remove
   listeners on hide/unload/reload. Keep effects cosmetic and non-interactive unless input ownership
   has been separately designed and tested.
7. Verify in layers: structural report, external visual preview, exact-build runtime smoke test,
   resolution/HUD-mode/localization checks, input/focus checks, and clean rollback. Label each result
   `[Confirmed]`, `[Deduced]`, or `[Unknown]`; never call static inspection runtime certification.

## Vault Boy evidence pattern

Treat names such as `LevelUpAnimation_mc` as candidate leads, not guaranteed exports. A path such as
`Underarmor/VaultBoy/White.dds` is a static texture observation, not proof of an animation. Community
extraction tools can provide search leads, but they are not an official SDK or redistribution
license. Keep animation support `[Unknown]` until the exact game build renders it. Record the exact
archive, SWF hash, symbol, and frame evidence before depending on any Vault Boy animation.

## Deliverable boundary

This repository may contain manifests, hashes, synthetic fixtures, and authored tooling guidance.
It must not contain extracted Bethesda assets, raw decompiler exports, downloaded mod payloads, or
an AI-training corpus made from third-party mod-service content. If a recompile is required,
preserve the pristine target and prefer a minimal, lossless patch; a successful decompiler
round-trip alone is not a shippable result.

See `references/animation-manifest.md` for the effect evidence schema and compatibility constraints.
