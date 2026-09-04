# FO76 HUD research — quick rules

> Load full `SKILL.md` when executing this workflow.

- Keep the investigation human-directed and explicitly bounded.
- Use explicit mod/file selectors; never enumerate the catalog.
- Read only the provider-specific API secret needed for the selected request.
- Keep downloads and derived artifacts outside the repository.
- Never execute downloaded archives, SWFs, DLLs, installers, or scripts.
- Record source IDs, hashes, timestamps, and tool versions.
- Preserve feed ranks; do not invent a popularity score.
- Join evidence on domain, source item ID, file ID, source hash, and logical path.
- Report static facts separately from deductions and runtime unknowns.
- Treat public availability as neither a license nor permission.
- Do not copy third-party source, assets, or raw decompiler output.
- Do not create an AI-training corpus from mod-service data without permission.
- Route release decisions through `fo76-hud-permissions`.
