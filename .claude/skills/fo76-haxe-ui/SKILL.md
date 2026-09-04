---
name: fo76-haxe-ui
description: Use when authoring, compiling, reviewing, or packaging Haxe source for Fallout 76 Scaleform/GFx HUD widgets or extender-aware SWFs.
---

# Fallout 76 Haxe UI

Use Haxe as a typed authoring language for a deliberately small AVM2/SWF surface. A Haxe build is
not automatically a Fallout 76-compatible movie: the generated SWF still has to satisfy the active
GFx, HUDModLoader, bridge, font, symbol, and packaging contracts.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip this skill's
compiler, runtime, or permission constraints.

**REQUIRED SUB-SKILL:** Use `fo76-scaleform-ui` for GFx, SWF, input, loader, and runtime decisions.

**REQUIRED SUB-SKILL:** Use `fo76-hud-permissions` before embedding, modifying, deriving, packaging,
or redistributing game-owned or third-party assets.

## Authoring contract

1. Pin Haxe, Java/toolchain, target game/build, HUDModLoader or extender versions, target SWF
   signature/version/compression, stage/header, root symbol, and every external class/bridge used.
2. Keep Haxe source, externs, synthetic fixtures, and build profiles separate from game files,
   extracted assets, downloaded payloads, decompiler exports, and rebuilt release artifacts.
3. Prefer the Flash standard display APIs and a small authored widget over a framework-heavy runtime.
   OpenFL can be useful for desktop previews or asset experiments, but its renderer and generated
   runtime are not evidence of Fallout 76 GFx compatibility.
4. Model only positively identified native surfaces with Haxe `extern` types. Use `@:native` for a
   proven native name, `#if swf` for target-specific code, and JSON strings plus explicit validation
   at extender boundaries. An extern declaration does not create or authorize a runtime provider.
5. Treat Haxe dead-code elimination, generated names, namespaces, linkage, and constructor order as
   ABI concerns. Keep entrypoints reachable with explicit roots or `@:keep` only where evidence
   requires it; do not use `untyped`, reflection, or arbitrary bridge calls to hide uncertainty.
6. Use `@:bitmap`, `@:file`, and `@:font` only with authored or separately cleared files. Do not
   embed vanilla textures, fonts, sounds, or decompiled classes in this repository or a release.

## Build and verification

Use `--no-output` with the Flash target still selected, then compile a candidate SWF with an
explicit `--swf-version`, `-D swf-header=...`, and `-D flash-strict` profile derived from the target
evidence. Use `--swf-lib-extern` for a cleared SWC needed only for type checking; use `--swf-lib` only
when the library is intentionally packaged and compatible. Read
`references/haxe-build.md` for profile examples and extern patterns.

Inspect the generated SWF with the Scaleform skill's FFDec workflow: fingerprint headers/tags,
ABC/classes, symbols, fonts, and link dependencies; compare hashes and structure; and keep all
exports outside the repository. A Haxe compile pass, desktop preview, or parseable SWF is not
runtime certification. Test the child-widget or targeted-patch path in the exact game build for
load order, HUD modes, resolution/safe area, localization, focus/input, reload/unload, and rollback.

Never turn a Haxe SWF into a direct `HUDMenu.swf` replacement merely because it compiles. Prefer a
HUDModLoader child movie; reserve vanilla surgery for a version-pinned, rights-cleared, minimal
patch plan.

See `references/haxe-build.md` and
`.claude/skills/fo76-scaleform-ui/references/ffdec-workflow.md`.
