# FO76 Haxe UI — quick rules

> Load full `SKILL.md` for Haxe/SWF authoring and verification.

- Pin Haxe, target SWF, game, loader, extender, and tool versions.
- Treat Haxe output as a candidate SWF, not GFx runtime proof.
- Prefer standard Flash APIs and small child widgets over framework-heavy output.
- Use externs only for positively identified native surfaces.
- Preserve native names, linkage, namespaces, roots, and reachable entrypoints.
- Use `--no-output` checks before generating a candidate SWF.
- Set SWF version/header from target evidence; never copy a generic example.
- Keep source, externs, payloads, and decompiler exports separated.
- Embed only authored or rights-cleared assets.
- Use FFDec structural verification outside the repository.
- Prefer HUDModLoader child movies over HUDMenu replacement.
- Test exact-build load, input, localization, reload, and rollback.
