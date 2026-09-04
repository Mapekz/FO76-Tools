# FO76 UI effects — quick rules

> Load full `SKILL.md` for the workflow and evidence schema.

- Pin build, archive/member, SWF hash, tool version, and platform.
- Keep game files, extracted assets, and decompiler exports outside the repo.
- Static inspection is not runtime proof.
- Default to recreating behavior with newly authored assets.
- A public mod page is not an asset license.
- Require a permission record for reuse or redistribution.
- Do not use `.filters` without positive evidence from the target Fallout 76 build.
- Avoid unproven shaders, gradients, Pixel Bender, and `fl.motion.*`.
- Use labels, not guessed frame numbers or symbols.
- Bound frame loops, timers, textures, and redraws.
- Clean up playback, listeners, and children on unload.
- Prefer minimal lossless SWF patches over full recompiles.
- Validate exact build, resolution, HUD modes, and localization.
- Keep payloads and raw decompiler output out of this repository.
