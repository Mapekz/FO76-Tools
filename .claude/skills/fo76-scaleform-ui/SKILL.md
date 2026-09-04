---
name: fo76-scaleform-ui
description: Use when building, reviewing, or debugging Fallout 76 in-game Scaleform/GFx HUD widgets, SWF patches, input paths, or extender-backed UI.
---

# Fallout 76 Scaleform UI

Treat Fallout 76's Scaleform/GFx runtime as a versioned game contract, not as normal Flash Player.
Every class, font binding, coordinate system, input event, bridge object, and SWF packaging decision
needs evidence from the target build.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip this skill's
constraints.

**REQUIRED SUB-SKILL:** Use `fo76-hudmodloader` when the feature can be a child widget or needs
HUDModLoader/SharedHUDTools lifecycle and input behavior.

**REQUIRED SUB-SKILL:** Use `fo76-hud-permissions` before modifying, deriving, packaging, or
redistributing game-owned or third-party UI assets.

## Runtime contract

1. Pin game/platform/build, loader and extender versions, target BA2/member, SWF signature/version,
   hash, and toolchain. Keep extracted files and decompiler output outside this repository.
2. Label evidence `[Confirmed]`, `[Deduced]`, or `[Unknown]`. A parseable SWF, community decompile,
   or Flash preview is not runtime certification.
3. Use the smallest compatible surface. Prefer a separate HML child SWF in a BA2; reserve a direct
   `HUDMenu.swf` replacement for a version-pinned, explicitly justified surgery plan. HUDModLoader
   entries and `sResourceArchive2List` are user-owned configuration: merge/append, detect path
   collisions, and never replace the complete list.
4. Treat coordinates and loader order as observed contracts. Verify viewport/safe-area behavior,
   aspect ratios, and precedence in the active build rather than scattering hard-coded screen
   positions.

## GFx-safe rendering

- Do not assign `.filters` to a display object unless the target build has positively demonstrated
  that exact operation is safe.
- Prefer solid fills, alpha, transforms, masks, and proven vector/bitmap assets. Do not assume
  shaders, Pixel Bender, gradient masks, `paletteMap`, `beginShaderFill`, or `fl.motion.*` exist.
- Dynamic text needs a font binding proven in the movie's application domain. Verify aliases in the
  active artifact and keep `embedFonts`/font family behavior explicit.
- Sanitize text before `htmlText`; use JSON strings across bridge calls; parse and validate every
  result. Update fields on data change, not every frame. Prefer incremental `appendHtml` where the
  target proves it, and avoid stylesheets unless independently validated.

## Input and providers

- A HUD-layer child does not automatically receive raw keyboard characters. Consume named
  `HUDMod::UserEvent`/control-map actions for loader-forwarded actions and keep a per-action edge
  latch when both key-down and key-up arrive.
- Text entry must use the host's documented text-edit helper or an extender capability that owns
  the input lifecycle. Balance start/end on submit, cancel, focus loss, menu handoff, provider loss,
  reload, and unload. Do not dynamically dispatch child-owned `ControlMap::StartEditText` events or
  globally swallow gameplay keys.
- Identify bridges positively. `__ZFE`/`ZFECodeObj` require a successful ZFE runtime/capability
  response. xScal's `chatInterface` is a different surface. A generic `__SFCodeObj.call` or an
  object name alone proves neither provider; never fall back to a raw socket, HTTP client, DLL,
  process inspection, memory read, injection, or port scan.

## Effects, animation, and patching

An observed MovieClip, frame label, class, or path is a candidate, not a stable ABI. Record the
archive, SWF hash, symbol/linkage, labels, rate/count, dependencies, and build. Candidate names
such as `LevelUpAnimation_mc` and paths such as `Underarmor/VaultBoy/White.dds` require exact-build
validation; a texture path is not animation proof. Prefer newly authored timeline/sprite behavior.
A static frame-sheet preview does not prove GFx behavior.

If direct HUD surgery is required, preserve the pristine target and use the narrowest lossless
bytecode patch available. A full decompiler recompile can change ABC namespaces, types, exports,
fonts, or runtime extensions and crash GFx. Compare structure, dependencies, and hashes before
packaging, then run exact-build smoke, resolution, localization, HUD-mode, input/focus, reload, and
rollback tests.

For JPEXS/FFDec CLI inspection or patching, read `references/ffdec-workflow.md`. Use targeted
structural dumps and class/resource exports from an external scratch run; never treat FFDec's
ActionScript output or successful replacement as runtime proof. Do not transfer browser-Flash
patterns such as `ExternalInterface`, CDN rewrites, ad bypasses, or guessed embedded-data IDs into
Fallout 76.
