# FO76 HUD Mod Loader — quick rules

> Load full `SKILL.md` when executing this workflow.

- Pin game, loader, SharedHUDTools, archive, and SWF versions.
- Treat the loader as a versioned runtime contract.
- Prefer a child widget over a vanilla movie replacement.
- Use unique module names and bounded versioned messages.
- Balance registration, listeners, timers, children, and text-edit cleanup.
- Never overwrite user configuration or archive registries.
- Detect module, path, symbol, and replacement collisions.
- Preserve BA2 paths and compression facts.
- Do not infer runtime compatibility from parsing or building.
- Treat old wiki/source snapshots as versioned evidence.
- Do not copy source/assets without rights clearance.
- Do not use DLLs, hooks, memory reads, raw sockets, or bypasses.
- Report unknown runtime behavior as `unknown` or `needs-review`.
