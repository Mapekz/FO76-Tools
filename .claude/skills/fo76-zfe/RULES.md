# FO76 ZFE — quick rules

> Load full `SKILL.md` for capability, data, input, and verification guidance.

- Pin ZFE, game, loader, host, platform, and capability versions.
- Prefer `__ZFE`; require positive runtime/capability evidence.
- Generic `__SFCodeObj` is not ZFE proof.
- Parse `success`; non-empty strings are not success.
- Use cache-first remote data and bounded polling.
- Production sources are HTTPS/443/DNS/global-host only.
- Never put secrets or identifiers in URLs or logs.
- Use `Enabled=yes` and `FragmentSources=yes`; `1` is ignored.
- Localhost requires dual explicit opt-in for development only.
- Keep storage vendor-scoped and paths safe.
- Events are local, bounded, cursor-aware, and lossy.
- Imports are allow-listed; never arbitrary filesystem access.
- Prefer child widgets and named HUD actions.
- Balance text-input/focus ownership on every exit path.
- No raw keys, sockets, injection, memory reads, or DLL/process inspection.
