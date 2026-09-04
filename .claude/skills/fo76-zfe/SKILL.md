---
name: fo76-zfe
description: Use when building Fallout 76 HUD widgets or keybind workflows that consume ZFE capabilities, remote data, storage, local events, imports, or documented native input.
---

# Fallout 76 ZFE integration

Treat ZFE as a versioned, opt-in extender API. Pin the game/platform, ZFE build, HUDModLoader and
SharedHUDTools artifacts, and each capability required by the feature. Use documented calls only;
an available bridge object is not permission to discover arbitrary runtime behavior.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip this skill's
provider, capability, and permission constraints.

**REQUIRED SUB-SKILL:** Use `fo76-scaleform-ui` for GFx rendering, input focus, bridge marshaling,
and SWF packaging.

**REQUIRED SUB-SKILL:** Use `fo76-hudmodloader` for child-widget lifecycle and named HUD actions
when HUDModLoader is the host.

**REQUIRED SUB-SKILL:** Use `fo76-hud-permissions` before modifying or redistributing game-owned or
third-party assets.

## Identify and handshake

- Prefer `__ZFE.call(command, payloadJson)`; `ZFECodeObj.call` can be a compatibility surface.
- A legacy `__SFCodeObj.call` is acceptable only after a positive ZFE response on that exact
  object. xScal may expose the same property for a different registry; the property name alone is
  never provider identity.
- Call `getRuntimeInfo` and parse JSON. Require `success:true`, the expected runtime/protocol, and
  the capability needed by the feature (`zfe-general-api-v1`, `zfe-storage-v1`,
  `zfe-events-v1`, `zfe-import-v1`, `zfe-remote-data-v1`, or the documented chat capability).
- If the provider, version, method, or capability is absent, degrade visibly to unavailable and
  stop. Do not invoke arbitrary method names, infer permissions, or substitute xScal/ZFE commands.

## Sanctioned data surfaces

1. **Remote data:** `readRemoteData` is cache-first. A cold cache can return `found:false` while a
   refresh is queued; poll later at a bounded rate and handle stale/missing data. Keep network I/O
   out of the Scaleform movie.
2. **Storage:** use `writeStorage`/`readStorage` only for vendor-owned local text. Use safe forward-
   slash paths with 1..4 segments; reject absolute paths, `..`, backslashes, and oversized values.
   Store settings/state, not executable content or arbitrary game files.
3. **Events:** `emitEvent`/`pollEvents` are a local in-process bus, not a network channel. Use stable
   topics, bounded payloads, cursor-aware low-rate polling, and graceful missed-event handling.
4. **Imports:** use only the documented `importExportFile` allow-list and source names for the
   pinned build. Imported text is untrusted user input; validate size/schema and never turn it into
   a path, script, or dynamic code.
5. **Chat/native input:** use `chat.v1.*` or native input calls only when the runtime info proves
   the relevant capability. Do not generalize a chat key poll into a universal keybind API.

## Remote-data and configuration rules

For production source fragments, use the documented HTTPS/443/DNS/global-host policy and send no
API keys, account IDs, character names, or gameplay telemetry in URLs. Bound source bytes, cache
age, timeout, and source count. Project research documents `[RemoteData] Enabled=yes` and
`FragmentSources=yes`;
for these INI booleans, `1` is silently ignored. Localhost development requires the separate INI
opt-in and `ZFE_REMOTE_DATA_ALLOW_LOCALHOST_DEVELOPMENT=1`; never make that a normal-user setup.

Keep config edits additive, target-specific, idempotent, and reversible. Preserve unknown keys and
comments where possible. Never overwrite `zfe.ini`, another mod's fragment, the archive registry,
or a user's environment wholesale. Do not place secrets in config, logs, source fragments, or
Scaleform strings.

## UI and keybinds

- Prefer a HUDModLoader child widget with explicit startup/shutdown. Remove timers, listeners,
  display children, and bridge callbacks on reload/unload.
- Use named `HUDMod::UserEvent` control-map actions for forwarded keybinds, with an edge latch when
  both key-down and key-up can arrive. A child SWF does not receive arbitrary raw characters.
- Use the exact host-owned text editor or documented ZFE native input capability for text entry.
  Release the edit/focus session on Enter, Escape, focus loss, menu handoff, timeout, provider loss,
  and unload. Do not dynamically dispatch child-owned `ControlMap::StartEditText` or globally steal
  gameplay keys.
- Keep the UI responsive: render loading/stale/offline/error states, update on events or bounded
  timers, and never poll remote data/events every frame.

## Verification gate

Test missing/partial capabilities, cold and stale cache, malformed/oversized data, invalid paths,
missing imports, event loss, duplicate key edges, focus handoff, config conflicts, reload/unload,
Steam/Game Pass cache behavior, and the exact target build. Record tool/API versions and label
claims `[Confirmed]`, `[Deduced]`, or `[Unknown]`. No mock, static parse, or successful bridge call
alone certifies a shippable mod.
