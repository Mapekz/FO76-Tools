# FO76 Parser — Out-of-scope / follow-ups

## Localization
- [ ] BA2 archive parser to extract localization `.strings`/`.dlstrings`/`.ilstrings` (FO76 BA2 + LZ4)
- [ ] .STRINGS/.DLSTRINGS/.ILSTRINGS reader; resolve LString u32 ids → text
      (format: count u32, dataSize u32, [id u32, offset u32]*, data block; STRINGS=zstring,
       DL/ILSTRINGS=len-prefixed; per-field table choice; ref Core/wbLocalization.pas)
- [ ] Support loose Strings/ folder as a runtime option

## Curve tables
- [ ] Load curve JSON from a Startup BA2 and resolve `CURV` FormID references to
      evaluable `{ "curve": [{ "x", "y" }, …] }` data (FO76 BA2 + LZ4; reuse BA2 parser from Localization work)
- [ ] Parse `CURV` records: `EDID` + `CRVE` or `JASF` subrecord holds a zstring path such as
      `Weapons\Weap_10mmSMGDMG.json` or `Creatures\Weapon\Damage_Universal_Tier24.json`
      (`wbDefinitionsFO76.pas` ~16405)
- [ ] Map CURV path → BA2 internal path: prefix `misc/curvetables/json/`, backslash → slash, lowercase
      (e.g. `Weapons\Weap_10mmSMGDMG.json` → `misc/curvetables/json/weapons/weap_10mmsmgdmg.json`)
- [ ] Index CURV FormID → parsed curve; optional on-disk cache keyed by Startup.ba2 mtime/size
- [ ] When decoding `formid` fields with `valid_refs: ["CURV"]`, inline resolved curve path + points
      (or lazy lookup handle) instead of bare `0xXXXXXXXX`
- [ ] Cover common referrers: WEAP `CVT0`–`CVT5` (damage/durability curves), `DAMA` array
      (`Type` + `Amount` + `Curve Table`), ARMO `CVT0`–`CVT3` + `DAMA` resistances, EXPL
      `Damage Curve Table` (`wbFromVersion` 150), and other `wbDamageTypeArray` / `wbFormIDCk(…, [CURV])` sites
- [ ] Expose curve evaluation helper: linear interpolate `y` at level `x`, clamp to curve range
      (same semantics as `dps-76/src/lib/curve-tables.ts`)

## Schema coverage
- [ ] Hand-model hard union deciders: wbConditions (CTDA), wbMGEFAssocItemDecider,
      wbPerkEffectDataDecider + EPF* (so SPEL/MGEF/PERK fully decode, not raw fallback)
- [ ] Expand record whitelist beyond the initial 8 (re-run extractor)
- [ ] RArray/RStruct grouping fidelity (vs flat per-subrecord presentation)
- [ ] VMAD (script) decoding
- [ ] Consider compile-and-introspect extraction via Free Pascal for full fidelity
      (build a small program that runs DefineFO76 and serializes the def tree) — higher
      fidelity than source-parsing, but Windows-API portability risk

## FormID / load order
- [ ] Cross-file FormID resolution & load-order fixup across masters (multi-plugin)
- [ ] Follow references (resolve FormID fields to their target record)

## Productization (post-POC)
- [ ] napi-rs / WASM binding for Electron/TS frontend
- [ ] axum HTTP + MCP server; chatbot front page
- [ ] Tree-navigation API (browse groups → records → fields)
- [ ] Write support (POC is read-only)
