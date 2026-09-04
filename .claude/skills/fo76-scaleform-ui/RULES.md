# FO76 Scaleform UI — quick rules

> Load full `SKILL.md` for runtime, input, effects, and patching guidance.

- Pin exact game, loader, extender, BA2/SWF, and tool versions.
- Static parse or Flash preview is not runtime proof.
- Keep extracted payloads and decompiler output outside the repo.
- Prefer a HUDModLoader child widget.
- Append/merge configs and archive lists; never replace wholesale.
- Verify coordinate space, safe area, and load order.
- Do not use `.filters` without positive evidence from the target Fallout 76 build.
- Avoid unproven GFx shaders, gradients, and `fl.motion.*`.
- Verify child-widget font aliases in the target domain.
- Use named HUD actions, not raw global keyboard capture.
- Balance every text-edit session and focus handoff.
- Positively identify ZFE/xScal; generic `__SFCodeObj` is not proof.
- Use sanctioned bridges only; no sockets, injection, memory, or DLL inspection.
- Prefer minimal lossless SWF patches over full recompiles.
- For FFDec, use targeted dumps/exports and post-patch re-export verification from an external
  scratch run; never copy browser-Flash recipes or guess embedded-data IDs.
- Test runtime, modes, localization, reload, and rollback.
