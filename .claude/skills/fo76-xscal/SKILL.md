---
name: fo76-xscal
description: Use when building Fallout 76 HUD widgets or keybind workflows that consume xScal's documented runtime callbacks, chatInterface, asynchronous connection state, or xscal.ini configuration.
---

# Fallout 76 xScal integration

Treat xScal as an optional, versioned third-party extender. Pin the game build, xScal build,
HUDModLoader/SharedHUDTools artifact, target platform, and the exact documented API surface before
writing a widget. “XCal” is treated here as the xScal extender identified by the project research.

## Portable loading

This entrypoint is plain Markdown. If the host has no skill-composition feature, resolve named
dependencies through `skills.manifest.yaml` and open their `SKILL.md` files manually. Translate
examples to host capabilities, report unavailable tools, and never invent or skip this skill's
provider and version constraints.

**REQUIRED SUB-SKILL:** Use `fo76-scaleform-ui` for GFx rendering, focus, bridge marshaling, and
SWF packaging.

**REQUIRED SUB-SKILL:** Use `fo76-hudmodloader` for child-widget lifecycle, named HUD actions, and
SharedHUDTools text editing when HUDModLoader is the target.

**REQUIRED SUB-SKILL:** Use `fo76-hud-permissions` before modifying or redistributing game-owned or
third-party assets.

## Provider identity

Do not classify a provider from a property name alone. Probe only the documented xScal surface:

- Prefer `__SFECodeObj.chatInterface` or `__SFCodeObj.chatInterface` when it exposes the required
  documented methods (`connect`, `pollEvents`, `sendMessage`, or the exact methods supported by the
  pinned build).
- A generic call-only `__SFCodeObj` is not xScal chat. `__SFECodeObj` by name is not enough either.
- Use a positive runtime/capability response before calling feature methods. If the interface is
  absent or ambiguous, report `provider-unavailable` and stop; do not discover methods by invoking
  arbitrary names.
- Keep xScal callbacks distinct from ZFE's `chat.v1.*` commands. Never mix their payloads,
  readiness states, or error handling without evidence from the target build.

## Connection and data flow

1. Call the documented `connect` with bounded, validated input and an explicit timeout.
2. Treat `success:true` with `status:"connecting"` as pending startup, not authenticated readiness.
   Keep the widget alive while polling the documented state/event method.
3. Reconnect only from documented terminal states such as disconnected/rejected, with bounded
   backoff and a terminal unavailable state. Do not reconnect every frame or on every parse error.
4. Validate message shape, size, and rate before rendering. Sanitize text and redact secrets from
   logs. Treat a provider response as untrusted data until its success/status fields are parsed.
5. Use xScal's sanctioned bridge for transport. Do not open a raw WebSocket/HTTP connection from
   ActionScript, guess an endpoint, inspect DLLs/processes, read game memory, inject code, or scan
   ports.

## HUD widget and keybinds

- Prefer a separate child SWF registered with HUDModLoader; do not replace `HUDMenu.swf` just to
  consume xScal. Register/tear down listeners, timers, display children, and callbacks on reload.
- Use named `HUDMod::UserEvent` control-map actions for forwarded keybinds. A child widget does not
  automatically receive raw keyboard characters. Use a per-action edge latch when both key-down and
  key-up events can arrive, and do not globally swallow gameplay keys.
- For text entry, use the exact host-owned SharedHUDTools editor or xScal input surface documented
  for the pinned build. Release focus/input ownership on Enter, Escape, focus loss, menu handoff,
  provider failure, reload, and unload.
- Keep open-key config and action-event fallback behavior explicit. Validate action names against
  the active loader's deliverable set; invalid values fall back without crashing.
- Keep updates event-driven or timer-bounded. Render connection states (`starting`, `ready`,
  `offline`, `rejected`) so missing xScal is a graceful feature downgrade.

## Configuration and verification

Merge the documented `[Chat]` settings in the user's `xscal.ini` only when the installer is
explicitly authorized to do so. Preserve unknown keys/comments, make a backup, use a narrow
idempotent edit, and provide rollback. Never overwrite the provider file wholesale, write secrets
into it without a documented contract, or silently change another mod's settings.

Verify: exact provider identity and version; missing/partial methods; async `connecting` behavior;
timeouts/reconnect bounds; malformed/oversized events; key-down/up duplication; focus/menu
handoff; reload/unload; archive/config collisions; redacted logs; and each supported game/platform.
Record `[Confirmed]`, `[Deduced]`, and `[Unknown]` evidence. A working mock or parseable SWF is not
runtime proof.
