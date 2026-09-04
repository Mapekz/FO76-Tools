---
name: fo76-vanilla-ui
description: Use when mapping or extending build-specific Fallout 76 vanilla Scaleform/GFx HUD surfaces where display paths, coordinates, input, focus, or localization affect compatibility.
---

# Vanilla Fallout 76 UI compatibility

Vanilla UI facts are build-specific. Produce a surface manifest and a compatibility verdict for
the named artifact/build—not a generic GFx guarantee. Cite a path, byte offset, source revision,
local hash, or fresh runtime log for every claim.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip this skill's
build-evidence constraints.

## Surface contract

Before editing, record the logical movie path, BA2 archive/type, SWF signature/version/compression,
root symbol, stage/frame rectangle, viewport scale/aspect/safe area, timeline labels, named
instances, classes/methods, external calls, localization assumptions, and available data
providers. Observed public/community names such as `HUDModeData`, `PlayerInventoryData`,
`DamageNumberUIData`, `PublicTeamsData`, and `BSUIDataManager` are search keys only; their
presence in one artifact does not prove availability or schema in another build.

Prefer a HUD Mod Loader child movie for new UI. If a vanilla movie must change, justify a targeted
ABC/bytecode splice against a pinned original and preserve a clean control copy. Do not fully
recompile a decompiled Bethesda class unless compiler, libraries, linkage, namespaces, symbols,
and runtime behavior are all proven. Never infer compatibility from an archive opening or an SWF
`End` tag.

## Coordinates, text, and input

- Prove the coordinate basis. Convert explicitly among text-field, marker-parent, global, and
  stage spaces; account for scroll, wrapping, reflow, resize, localization, and safe areas.
- Treat `getCharBoundaries` as a layout rectangle, not a tight glyph box. Measure the actual
  target field before placing an overlay.
- Named HUD user events are not raw character events. Verify event fields, edge semantics, host
  forwarding, and the handled Boolean before consuming anything. Only own navigation while the
  widget owns focus; never swallow gameplay arrows globally.
- Use the engine/native text-edit contract for typing, pair every start with an owned end, and
  clean up on cancel, reload, focus loss, and failure.
- Keep retained rows bounded and update on state/data changes rather than every frame. Treat
  filters, shaders, gradients, and image substitutions as unsupported until measured on the
  target runtime.

## Compatibility result

Compare candidate and vanilla manifests for game/archive drift, replacement path and precedence,
missing symbols/classes/methods, changed arity or external-call contracts, dynamic lookups,
resolution/input/focus assumptions, localization, per-frame work, and conflicts with other
replacements. Return `compatible`, `needs-review`, `incompatible`, or `unknown` with sorted
reasons and evidence references. `compatible` means the named static contract is covered; it
is not a runtime certification. Use `unknown` when exact-build runtime evidence is absent.

## Red flags

- A mod's instance names are not proof of vanilla display-list paths, depths, linkage IDs, or
  method contracts.
- A successful decompile/recompile is not proof of semantic or runtime equivalence.
- Global keyboard capture can strand gameplay controls, chat, rebinding, or accessibility input.
- English-only measurements can fail under localization, wrapping, fallback fonts, or IME input.
- A static manifest cannot establish provider timing, focus behavior, or runtime load success.

Keep any game-data inspector external to this skills repository. This workflow never writes game
records and never turns extracted game data into a tracked fixture.

References: [Autodesk GFx text fields](https://help.autodesk.com/cloudhelp/ENU/Scaleform-Help/scaleform_help/as3_reference/textfield_extensions.html),
[HUD Mod Loader](https://github.com/GitCrazy-wc/hudmodloader), and
[RABCDAsm](https://github.com/CyberShadow/RABCDAsm).
