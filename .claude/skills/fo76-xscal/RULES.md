# FO76 xScal — quick rules

> Load full `SKILL.md` for the provider, lifecycle, input, and config workflow.

- Pin game, xScal, loader, SharedHUDTools, platform, and API versions.
- Identify xScal from `chatInterface` plus positive runtime evidence.
- Generic `__SFCodeObj.call` is not provider proof.
- Keep xScal and ZFE command surfaces separate.
- Treat `connecting` as pending, never ready.
- Poll at a bounded rate; reconnect only terminal states.
- Validate and bound all inbound data.
- Use the sanctioned bridge; never add raw sockets or HTTP.
- Prefer a HUDModLoader child widget.
- Use named HUD actions, not raw global keys.
- Balance focus and text-edit ownership on every exit path.
- Merge `xscal.ini` narrowly; preserve user state and provide rollback.
- Redact credentials and private identifiers from logs.
- Test missing provider, conflicts, reload, and exact target builds.
- Label runtime claims as confirmed, deduced, or unknown.
