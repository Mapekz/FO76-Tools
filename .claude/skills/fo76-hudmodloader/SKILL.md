---
name: fo76-hudmodloader
description: Use when a Fallout 76 HUD widget must interoperate with HUD Mod Loader, SharedHUDTools, hudmodloader.ini, or shared ActionScript application-domain classes.
---

# HUD Mod Loader interoperability

Treat HUD Mod Loader as a versioned runtime contract, not a generic Flash plugin. The deliverable
is a compatibility-aware widget or review record; a parseable SWF is only a structural result.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip its lifecycle or
compatibility constraints.

## Before implementation

Pin the target game build, loader distribution/file version, SharedHUDTools artifact, SWF hashes,
BA2 internal paths, and intended load mode. Treat public source and distributed packages as
separate versioned inputs and verify their rights and build metadata independently. If versions,
rights, or the supported extension point are unknown, report the blocker instead of filling the
gap with a DLL, process hook, or guessed API.

## Runtime topology

- The modified HUD menu loads `hudmodloader.swf`; the loader reads a relative
  `hudmodloader.ini`, loads the shared HUDTools movie, and then loads configured mod movies.
- Loaded movies share an ActionScript application domain. Package names, root symbols, shared
  classes, and exact runtime paths therefore matter.
- A movie that supports optional unload/reload exposes the loader's proven `isReloadable`
  convention. Balance registration, listeners, timers, display children, and text-edit sessions
  on every shutdown/reload path.
- Prefer the narrowest child widget that supplies the required data and input. Do not assume child
  events suppress gameplay or inherit the host's `ProcessUserEvent` handled Boolean.
- Reserve direct replacement of `HUDMenu` or another vanilla movie for a separately justified,
  version-pinned compatibility plan.

## SharedHUDTools surface

The public v1.2-era source and current decompiled artifacts show a wrapper with these conceptual
operations: register/shutdown, send or broadcast a message, text edit and keyboard formatting,
menu registration/add/show/close/format, language selection, and an active-state property.
HUD modes include normal HUD, power armor, Pip-Boy, map, VATS, workshop, terminal, dialogue,
death/respawn, and other transient states. Use the exact symbols and protocol version from the
active artifact; this list is an orientation, not a compatibility guarantee.

Keep messages bounded and versioned. Register a unique module name, scope it to the needed HUD
mode, reject malformed/unknown messages, and shut down cleanly. Do not invent a new wire format
when the active shared wrapper already supplies one.

## Configuration and packaging

- Append/merge loader entries; never replace a user's `hudmodloader.ini`, `Fallout76Custom.ini`,
  or archive registry wholesale. Preserve a backup and make edits reversible.
- Detect duplicate module IDs, archive paths, root symbols, and direct-replacement files before
  installing or proposing a load order.
- Preserve exact BA2 filenames and internal paths. Record whether entries are stored, LZ4, zlib,
  or unknown; an extracted DDS/SWF is not proof of the original on-disk representation.
- Validate SWF signature/version, declared and decompressed lengths, frame rectangle, tag table,
  ABC blocks, and End tag. Choose FWS/CWS/ZWS only from active runtime evidence.
- Do not copy the public checkout's source, bundled `assets.swf`, generated classes, or framework
  code without explicit rights and a reproducible build decision.

## Verification gate

Record evidence as `[Confirmed]`, `[Deduced]`, or `[Unknown]`. Test malformed config, duplicate
module names, stale loader entries, mode changes, reload/unload, focus loss, and missing
providers. Runtime success requires an exact-build smoke test and reproducible logs; until then
use `needs-review` or `unknown`, not “works.”

## Common mistakes

- Copying public loader source/assets without a rights decision.
- Treating the older wiki or source snapshot as the current protocol.
- Replacing configuration or assuming archive precedence.
- Leaving timers, listeners, display children, or text-edit sessions alive after reload.
- Using global input capture, DLL inspection, process injection, game-memory reads, raw sockets,
  or anti-cheat/security-bypass techniques.

## Minimal evidence note

`[Confirmed]` `interface\\hudmodloader.swf` exists in a pinned BA2 with hash H. `[Unknown]` the
current game loads that movie, resolves its shared classes, and preserves input after reload.
This distinction is the required reporting pattern.

References: [HUD Mod Loader source](https://github.com/GitCrazy-wc/hudmodloader),
[SharedHUDTools wiki](https://github.com/GitCrazy-wc/hudmodloader/wiki/SharedHUDTools), and
[HUDTools instructions](https://github.com/GitCrazy-wc/hudmodloader/wiki/HUDTools-Instructions).
