#!/usr/bin/env python3
"""Data tables for Pascal constructs `extract.py` can't resolve statically.

TES5Edit's Pascal record definitions reference helper functions (compiled
Delphi closures) and bare `wbXxx` variables whose real value lives in Delphi
code, not in the `:=` assignment text `extract.py`'s `_collect_vars` scrapes
out of `wbDefinitionsFO76.pas`. This module holds the reverse-engineered
substitutes for both, as data, the same way `extract.py`'s
`KNOWN_UNION_DECIDERS` already models union deciders:

- `BUILTIN_HELPERS` / `BUILTIN_HELPERS_DEFAULTS`: literal Pascal-equivalent
  expansions for bare `wbXxx` variables, injected into `Extractor.vars` by
  `Extractor._inject_builtin_helpers` before parsing begins. `BUILTIN_HELPERS`
  entries overwrite any auto-collected Pascal assignment unconditionally
  (`self.vars[name] = value`); `BUILTIN_HELPERS_DEFAULTS` entries only apply
  when `_collect_vars` didn't already populate that name (`dict.setdefault`
  semantics) — the split mirrors which of the two the original hand-written
  override used, and is preserved exactly.
- `CALL_EXPANSIONS`: expansions for `wbXxx(args)` call syntax, keyed by
  function name and consulted from `Extractor.expand_call`. A table value is
  either a fixed string (the call's args don't affect its expansion) or a
  `(args, ExpandContext) -> str | None` callable that substitutes the parsed
  args into a template; returning `None` means "no expansion, fall through
  to the caller's self.vars/raw-expression default" (mirrors an `if` branch
  in the original code with no covering `else`).

Example: `CALL_EXPANSIONS["wbModelInfo"]` is a callable — `wbModelInfo(MODT)`
substitutes the `MODT` signature into a `wbByteArray(MODT, 'Model
Information', 0)` template.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass

# ============================================================================
# BUILTIN_HELPERS / BUILTIN_HELPERS_DEFAULTS
# ============================================================================
# Local building blocks referenced from entries below (kept as loop-built
# code rather than flattened by hand — the values are algorithmically
# generated, not hand-transcribable without risk of a transcription error).

# RACE character-creation builders — Pascal functions (not variables)
# at wbDefinitionsFO76.pas:3940/4007/4028; expanded via expand_call.
_blend_op_enum = (
    "wbEnum(['Default','Multiply','Overlay','Soft Light','Hard Light'])"
)
_tint_slot_enum = (
    "wbEnum(["
    "'Forehead Mask','Eyes Mask','Nose Mask','Ears Mask',"
    "'Cheeks Mask','Mouth Mask','Neck Mask','Lip Color',"
    "'Cheek Color','Eyeliner','Eye Socket Upper','Eye Socket Lower',"
    "'Skin Tone','Paint','Laugh Lines','Cheek Color Lower',"
    "'Nose','Chin','Neck','Forehead','Dirt','Scars',"
    "'Face Detail','Brows','Wrinkles','Beards'"
    "])"
)

# wbSizePosRot — bounds size + position/rotation; handled via
# CALL_EXPANSIONS with the sig arg (see _expand_wbSizePosRot below).

# WTHR (Weather) helper functions from wbDefinitionsCommon.pas.
# These are Common.pas functions missed by _collect_vars.

# wbWeatherCloudTextures — wbDefinitionsCommon.pas:8939-8991.
# FO76 uses a 32-layer cloud texture system with special 4-byte sigs.
# Layers 0-9: "00TX"-"90TX" (alphanumeric, injectable directly).
# Layers 17-31: "A0TX"-"O0TX" (alphanumeric, injectable directly).
# Layers 10-16: ":0TX"-"@0TX" (non-alphanumeric, added via record_additions).
_cloud_tex_parts: list[str] = []
for _i in range(10):  # layers 0-9: sig = chr(0x30+i)+"0TX" = "00TX"-"90TX"
    _sig = f"{_i}0TX"
    _cloud_tex_parts.append(f"wbString({_sig},'Layer #{_i}')")
for _i in range(17, 32):  # layers 17-31: "A0TX"-"O0TX"
    _sig = chr(ord("A") + _i - 17) + "0TX"
    _cloud_tex_parts.append(f"wbString({_sig},'Layer #{_i}')")

# VMAD: emit the existing vmad decoder kind.  All wbVMAD* variants
# resolve to the __vmad__ sentinel which parse_member intercepts.
_VMAD_VARS = (
    "wbVMAD",
    "wbVMADFragmentedPERK",
    "wbVMADFragmentedPACK",
    "wbVMADFragmentedQUST",
    "wbVMADFragmentedSCEN",
    "wbVMADFragmentedINFO",
)

BUILTIN_HELPERS: dict[str, str] = {
    "wbOBND": (
        "wbStruct(OBND, 'Object Bounds', ["
        "wbInteger('X1', itS16), wbInteger('Y1', itS16), wbInteger('Z1', itS16),"
        "wbInteger('X2', itS16), wbInteger('Y2', itS16), wbInteger('Z2', itS16)])"
    ),
    "wbKeywords": (
        "wbRStruct('Keywords', ["
        "wbInteger(KSIZ, 'Keyword Count', itU32),"
        "wbArrayS(KWDA, 'Keywords', wbFormIDCk('Keyword', [KYWD,NULL]))])"
    ),
    "wbGenericModel": (
        "wbRStruct('Model', ["
        "wbString(MODL, 'Model FileName'),"
        "wbByteArray(MODT, 'Model Information', 0),"
        "wbByteArray(MODC, 'Model Color', 0),"
        "wbByteArray(MODS, 'Model Data', 0),"
        "wbByteArray(MODF, 'Model Flags', 0),"
        "wbENLM,"
        "wbModelXFLG,"
        "wbENLT,"
        "wbENLS,"
        "wbAUUV,"
        "wbMODD])"
    ),
    # wbDEST: full rstruct matching wbDefinitionsFO76.pas:6905-6991.
    # Includes the Stages rarray with DSTD (Destruction Stage Data).
    # wbConditions is resolved below via self.vars lookup.
    "wbDEST": (
        "wbRStruct('Destructible',["
        "wbStruct(DEST,'Header',["
        "wbInteger('Health',itS32),"
        "wbInteger('Stage Count',itU8),"
        "wbUnused(3),"
        "wbInteger('Flags',itU32,wbFlags(["
        "'VATS Targetable','Large Actor Destroys','Unknown 3','Unknown 4',"
        "'Unknown 5','Unknown 6','Unknown 7','Unknown 8',"
        "'Unknown 9','Unknown 10','Unknown 11','Limit DPS Taken',"
        "'Has Conditions','Unknown 14','Unknown 15','Unknown 16'"
        "])),"
        "wbFromSize(16,wbFloat('DPS Limit'))"
        "]),"
        "wbConditions,"
        "wbEmpty(DSCF,'End Condition Marker'),"
        "wbFormIDCk(HGLB,'Health Global',[GLOB,NULL]),"
        "wbArrayS(DAMC,'Resistances',wbStructSK([0],'Resistance',["
        "wbFormIDCk('Damage Type',[DMGT]),"
        "wbInteger('Value',itU32)"
        "])),"
        "wbRArray('Stages',wbRStruct('Stage',["
        "wbStruct(DSTD,'Destruction Stage Data',["
        "wbInteger('Health %',itU8),"
        "wbInteger('Index',itU8),"
        "wbInteger('Model Damage Stage',itU8),"
        "wbInteger('Flags',itU8,wbFlags(["
        "'Cap Damage','Disable','Destroy','Ignore External Damage',"
        "'Becomes Dynamic','Unknown 5','Disable Collision','Unknown 7'"
        "])),"
        "wbInteger('Self Damage per Second',itS32),"
        "wbFormIDCk('Explosion',[EXPL,NULL]),"
        "wbFormIDCk('Debris',[DEBR,NULL]),"
        "wbInteger('Debris Count',itS32),"
        "wbFormIDCk('Material Swap',[MSWP,NULL]),"
        "wbFloat('Model Swap Delay')"
        "]),"
        "wbString(DSTA,'Sequence Name'),"
        "wbRArray('Models',wbRStruct('Model',["
        "wbString(DMDL,'Model FileName',0),"
        "wbByteArray(DMDT,'Model Information',0),"
        "wbDMDC,wbDMDS,wbENLM,wbENLT,wbENLS,wbAUUV"
        "])),"
        "wbEmpty(DSTF,'End Marker')"
        "]))"
        "])"
    ),
    "wbEnchantment": "wbFormIDCk(EITM, 'Enchantment', [ENCH,NULL])",
    "wbModelInfo": "wbByteArray(MODT, 'Model Information', 0)",
    # wbDamageTypeArray — includes the form-version 152 Curve Table field.
    # wbDefinitionsCommon.pas:8014-8024.
    "wbDamageTypeArray": (
        "wbArrayS(DAMA, 'Resistances', wbStructSK([0], 'Resistance', ["
        "wbFormIDCk('Type', [DMGT]),"
        "wbInteger('Amount', itU32),"
        "wbFromVersion(152, wbFormIDCk('Curve Table', [CURV, NULL]))]))"
    ),
    "wbPTRN": "wbFormIDCk(PTRN, 'Preview Transform', [TRNS,NULL])",
    # wbPHST is auto-collected from its `:=` assignment (FO76.pas:4999) as
    # wbInteger(PHST,'Physics Sync Type',itU32,wbPHSTFlags).IncludeFlag(...) —
    # no stub override needed; the extractor already strips method chains.
    "wbSNTP": "wbFormIDCk(SNTP, 'Snap Template', [STMP])",
    # wbXALGFlags / wbXALG — wbDefinitionsFO76.pas:4815-4848 / :4931.
    "wbXALGFlags": (
        "wbFlags(["
        "'Skip HAVOK on Load','Server Authoritative','Disable Permanent Decals',"
        "'Never Visible Distant','Item Dispenser','Item Dispenser Pickedup',"
        "'Fast travel restricted','Block Item Dispenser','Premium','Visible Distant',"
        "'Camera Weapon Detectable','Fallout 1st','Bullion Reward Object',"
        "'REFR invalidates previs','Deleted REFR invalidates previs',"
        "'Container weight calculation queued','UNUSED 17',"
        "'No refresh body 3D on load','Unknown 19','Unknown 20','Unknown 21',"
        "'Unknown 22','Unknown 23','Unknown 24','Unknown 25','Unknown 26',"
        "'Unknown 27','Unknown 28','Unknown 29','Unknown 30','Unknown 31','Unknown 32'"
        "])"
    ),
    "wbXALG": "wbInteger(XALG, 'Flags', itU64, wbXALGFlags)",
    "wbFTAGs": "wbRArray('Form Tags', wbString(FTAG, 'Form Tag'))",
    # wbBipedObjectFlags — wbDefinitionsFO76.pas:4312-4345.
    # FO76's BOD2 is a single u32 of biped-object flags (no Armor Type byte).
    "wbBipedObjectFlags": (
        "wbFlags(["
        "'30 - Hair Top','31 - Hair Long','32 - FaceGen Head','33 - BODY',"
        "'34 - L Hand','35 - R Hand','36 - [U] Torso','37 - [U] L Arm',"
        "'38 - [U] R Arm','39 - [U] L Leg','40 - [U] R Leg','41 - [A] Torso',"
        "'42 - [A] L Arm','43 - [A] R Arm','44 - [A] L Leg','45 - [A] R Leg',"
        "'46 - Headband','47 - Eyes','48 - Beard','49 - Mouth',"
        "'50 - Neck','51 - Ring','52 - Scalp','53 - Decapitation',"
        "'54 - Backpack','55 - EyeOfRa','56 - Unnamed','57 - Coverall',"
        "'58 - Unnamed','59 - Shield','60 - Pipboy','61 - FX'"
        "])"
    ),
    # wbBOD2 — wbDefinitionsFO76.pas:4347-4351.
    # Single u32 of biped flags; wbBipedObjectFlags var is resolved by _parse_integer.
    "wbBOD2": (
        "wbStruct(BOD2, 'Biped Body Template', ["
        "wbInteger('First Person Flags', itU32, wbBipedObjectFlags)])"
    ),
    "wbETYP": "wbFormIDCk(ETYP, 'Equipment Type', [EQUP,NULL])",
    "wbYNAM": "wbFormIDCk(YNAM, 'Sound - Pickup', [SNDR,NULL])",
    "wbZNAM": "wbFormIDCk(ZNAM, 'Sound - Putdown', [SNDR,NULL])",
    "wbVCRY": "wbFormIDCk(VCRY, 'Value Currency', [NULL,CNCY])",
    "wbICON": "wbString(ICON, 'Icon Image')",
    "wbMICO": "wbString(MICO, 'Message Icon')",
    # wbINRD, wbEILV, wbIBSD (FO76.pas:7084-7086) are auto-collected from their
    # `:=` assignments — no stub overrides needed. wbEILV is a u32 array
    # ('Eligible Levels'); wbIBSD is a FormID→SNDR ('Break Sound').
    # wbAPPR — wbDefinitionsFO76.pas:7136.
    # Sorted array of KYWD FormIDs (attach-parent slots).  Decoded as a packed
    # array of 4-byte FormIDs via the record-context Array path.
    "wbAPPR": "wbArrayS(APPR, 'Attach Parent Slots', wbFormIDCk('Keyword', [KYWD]))",
    "wbMDOB": "wbByteArray(MDOB, 'Menu Display Object', 0)",
    "wbMIID": "wbInteger(MIID, 'Max Item ID', itU32)",
    "wbDEFL": "wbFormIDCk(DEFL, 'Default Layer', [LAYR])",
    "wbOPDSs": (
        "wbRArray('Object Placement Defaults', wbStruct(OPDS, 'Object Placement Default', ["
        "wbByteArray('Flags', 4), wbFloat('Sink'), wbFloat('Sink Var'), wbFloat('Scale'),"
        "wbFloat('Scale Var'), wbFloat('Angle X'), wbFloat('Angle X Var'),"
        "wbFloat('Angle Y'), wbFloat('Angle Y Var'), wbFloat('Angle Z'), wbFloat('Angle Z Var')]))"
    ),
    "wbHitBehaviourEnum": "wbEnum(['Normal formula behaviour','Dismember only','Explode only','No dismember or explode'])",
    "wbSoundLevelEnum": "wbEnum(['Loud','Normal','Silent','Very Loud','Quiet'])",
    "wbStaggerEnum": "wbEnum(['None','Small','Medium','Large','Extra Large'])",
    "wbBoolEnum": "wbEnum(['False','True'])",
    "wbFaceMorphElement": (
        "wbRStruct('Face Morph',["
        "wbInteger(FMRI,'Index',itU32),"
        "wbLString(FMRN,'Name')])"
    ),
    "wbMorphPreset": (
        "wbRStruct('Morph Preset',["
        "wbInteger(MPPI,'Index',itU32),"
        "wbLString(MPPN,'Name'),"
        "wbString(MPPM,'Morph Type'),"
        "wbFormIDCk(MPPT,'Texture',[TXST]),"
        "wbInteger(MPPF,'Playable',itU8,wbBoolEnum)])"
    ),
    "wbMorphGroupElement": (
        "wbRStruct('Morph Group',["
        "wbString(MPGN,'Name'),"
        "wbInteger(MPPC,'Count',itU32),"
        "wbRArray('Morph Presets',wbMorphPreset).SetCountPath('Count'),"
        "wbInteger(MPPK,'Tint Layer Face Region Index',itU16),"
        "wbArray(MPGS,'Morph Value Indexs',wbInteger('Index',itU32))])"
    ),
    "wbTintTemplateOption": (
        "wbRStruct('Option',["
        "wbStruct(TETI,'Index',["
        f"wbInteger('Slot',itU16,{_tint_slot_enum}),"
        "wbInteger('Index',itU16)]),"
        "wbLString(TTGP,'Name'),"
        "wbInteger(TTEF,'Flags',itU16,wbFlags(["
        "'On/Off only','Chargen Detail','Takes Skin Tone','Unknown 3'"
        "])),"
        "wbConditions,"
        "wbRArray('Textures',wbString(TTET,'Texture')),"
        f"wbInteger(TTEB,'Blend Operation',itU32,{_blend_op_enum}),"
        "wbArray(TTEC,'Template Colors',wbStruct('Template Color',["
        "wbFormIDCk('Color',[CLFM]),"
        "wbFloat('Alpha'),"
        "wbInteger('Template Index',itU16),"
        f"wbInteger('Blend Operation',itU32,{_blend_op_enum})])),"
        "wbFloat(TTED,'Default')])"
    ),
    "wbTintTemplateGroupElement": (
        "wbRStruct('Group',["
        "wbLString(TTGP,'Group Name'),"
        "wbRArray('Options',wbTintTemplateOption),"
        "wbInteger(TTGE,'Category Index',itU32)])"
    ),
    # ----------------------------------------------------------------
    # wbDefinitionsCommon.pas functions — not collected by _collect_vars
    # because that method only scans DefineFO76; inject them here.
    # ----------------------------------------------------------------
    # wbFaction: FO76 = FO4+, so IsFO4Plus(nil, wbUnused(3)) → wbUnused(3).
    "wbFaction": (
        "wbStructSK(SNAM, [0], 'Faction', ["
        "wbFormIDCk('Faction', [FACT]),"
        "wbInteger('Rank', itS8),"
        "wbUnused(3)])"
    ),
    # wbHeadPart: FO76 is not Oblivion/FO3, so uses HEAD formid variant.
    "wbHeadPart": (
        "wbRStructSK([0], 'Head Part', ["
        "wbInteger(INDX, 'Head Part Number', itU32),"
        "wbFormIDCk(HEAD, 'Head', [HDPT, NULL])])"
    ),
    # ----------------------------------------------------------------
    # IMAD color interpolator — used in wbArray(TNAM/NAM3, ...) calls.
    # ----------------------------------------------------------------
    "wbFloatRGBA": (
        "wbStruct('Color', ["
        "wbFloat('Red'), wbFloat('Green'), wbFloat('Blue'), wbFloat('Alpha')])"
    ),
    "wbColorInterpolator": (
        "wbStructSK([0], 'Data', [wbFloat('Time'), wbStruct('Value', ["
        "wbFloat('Red'), wbFloat('Green'), wbFloat('Blue'), wbFloat('Alpha')])])"
    ),
    # ----------------------------------------------------------------
    # CTDA / Conditions — modeled as a structural (no-Rust) helper.
    # Wrapped in an RStruct so the preceding CITC count subrecord
    # (wbDefinitionsFO76.pas:6888 SetCountPath(CITC)) is consumed and
    # not left in _unmapped.  CITC is optional — records without it
    # (e.g. direct CTDA blocks) simply skip the missing integer.
    # ----------------------------------------------------------------
    "wbConditions": (
        "wbRStruct('Conditions',["
        "wbInteger(CITC,'Condition Count',itU32),"
        "wbRArray('Conditions',"
        "wbRStruct('Condition',["
        "wbStruct(CTDA,'Condition Data',["
        "wbInteger('Type',itU8),"
        "wbUnused(3),"
        "wbByteArray('Comparison Value',4),"
        "wbInteger('Function',itU16),"
        "wbUnused(2),"
        "wbByteArray('Parameter #1',4),"
        "wbByteArray('Parameter #2',4),"
        "wbInteger('Run On',itU32,wbEnum(["
        "'Subject','Target','Reference','Combat Target',"
        "'Linked Reference','Quest Alias','Package Data','Event Data',"
        "'Unknown 8','Command Target','Event Camera Ref','My Killer',"
        "'Active Players','Potential Players','Player Teammates',"
        "'Target List','Instance Owner'"
        "])),"
        "wbByteArray('Reference',4),"
        "wbByteArray('Parameter #3',4)"
        "]),"
        "wbString(CIS1,'Parameter #1'),"
        "wbString(CIS2,'Parameter #2')"
        "]))"
        "])"
    ),
    # EFIT layout is form-version-conditional. The bands come from the comment at
    # wbDefinitionsFO76.pas:6727 (the Pascal itself uses a bare wbUnknown): Effect ID
    # only at fv>=166; trailing unknown 12 bytes for fv 154-165, 8 bytes for 166-182.
    "wbEFIT": (
        "wbStruct(EFIT,'Effect Item Data',["
        "wbFromVersion(166, wbInteger('Effect ID',itU32)),"
        "wbFloat('Magnitude'),"
        "wbInteger('Area',itU32),"
        "wbInteger('Duration',itU32),"
        "wbFromVersion(154, wbBelowVersion(166, wbByteArray('_unknown',12))),"
        "wbFromVersion(166, wbBelowVersion(183, wbByteArray('_unknown',8)))"
        "])"
    ),
    "wbEffect": (
        "wbRStruct('Effect',["
        "wbFormIDCk(EFID,'Base Effect',[MGEF]),"
        "wbStruct(EFIT,'Effect Item Data',["
        "wbFromVersion(166, wbInteger('Effect ID',itU32)),"
        "wbFloat('Magnitude'),"
        "wbInteger('Area',itU32),"
        "wbInteger('Duration',itU32),"
        "wbFromVersion(154, wbBelowVersion(166, wbByteArray('_unknown',12))),"
        "wbFromVersion(166, wbBelowVersion(183, wbByteArray('_unknown',8)))"
        "]),"
        "wbFormIDCk(CVT0,'Curve Table',[CURV,NULL]),"
        "wbFormIDCk(MAGA,'Actor Value',[AVIF,NULL]),"
        "wbInteger(MAGF,'Effect Flags',itU32,wbFlags(["
        "'Unknown 0','Unknown 1','Unknown 2','Unknown 3',"
        "'Unknown 4','Unknown 5','Unknown 6','Unknown 7',"
        "'Unknown 8','Unknown 9','Unknown 10','Unknown 11',"
        "'Unknown 12','Unknown 13','Unknown 14','Unknown 15',"
        "'Unknown 16','Unknown 17','Unknown 18','Unknown 19',"
        "'Unknown 20','Unknown 21','Unknown 22','Unknown 23',"
        "'Unknown 24','Unknown 25','Unknown 26','Unknown 27',"
        "'Unknown 28','Unknown 29','Unknown 30','Unknown 31'"
        "])),"
        "wbConditions,"
        "wbFormIDCk(DURG,'Duration',[GLOB,NULL]),"
        "wbFormIDCk(MAGG,'Magnitude',[GLOB,NULL]),"
        "wbFormIDCk(EIES,'Next Stage',[SPEL,NULL]),"
        "wbFormIDCk(CODG,'Cooldown Global',[GLOB,NULL]),"
        "wbInteger(CODV,'Cooldown Duration',itU32)"
        "])"
    ),
    "wbEffectsReq": "wbRArray('Effects',wbEffect)",
    # wbOBTSReq and wbObjectTemplate are collected from the Pascal assignments
    # in DefineFO76 (wbDefinitionsFO76.pas:7399-7430).  The real OBTS struct
    # includes Includes→OMOD references and a Keywords count-prefix array
    # (the -4 count argument is handled by _parse_array).
    # wbObjectModProperties provides the Properties array with
    # SetCountPath('Property Count').
    # ----------------------------------------------------------------
    # Common.pas function helpers not captured by _collect_vars.
    # These are functions (not := assignments) in wbDefinitionsCommon.pas.
    # ----------------------------------------------------------------
    # wbWeatherMagic — wbDefinitionsCommon.pas:9183-9196
    # UNAM 'Magic' struct: Lightning Strike spell/threshold + Weather Activate spell/threshold.
    "wbWeatherMagic": (
        "wbStruct(UNAM,'Magic',["
        "wbStruct('Lighting Strike',["
        "wbFormIDCk('Spell',[SPEL,NULL]),"
        "wbFloat('Threshold')"
        "]),"
        "wbStruct('Weather Activate',["
        "wbFormIDCk('Spell',[SPEL,NULL]),"
        "wbFloat('Threshold')"
        "])"
        "])"
    ),
    # wbRagdoll — wbDefinitionsCommon.pas:8694-8710
    # Ragdoll bone data (XRGD) + biped rotation (XRGB, non-TES4 only).
    "wbRagdoll": (
        "wbRStruct('Ragdoll Data',["
        "wbArray(XRGD,'Bones',wbStruct('Bone',["
        "wbInteger('Bone Id',itU8),"
        "wbUnused(3),"
        "wbByteArray('Position/Rotation',24)"
        "])),"
        "wbVec3(XRGB,'Biped Rotation')"
        "])"
    ),
    # wbKWDAs — used in REFR/ACHR to add keywords.
    # Minimal: array of keyword formids with KWDA sig.
    "wbKWDAs": (
        "wbRStruct('Keywords',["
        "wbInteger(KSIZ,'Keyword Count',itU32),"
        "wbArrayS(KWDA,'Keywords',wbFormIDCk('Keyword',[KYWD,NULL]))"
        "])"
    ),
    # wbOwnership — ownership data (owner ref + rank).
    # wbDefinitionsCommon.pas:8655 (simplified: XOWN + XRNK).
    "wbOwnership": (
        "wbRStruct('Ownership',["
        "wbFormIDCk(XOWN,'Owner',[FACT,NPC_,NULL]),"
        "wbInteger(XRNK,'Faction Rank',itS32)"
        "])"
    ),
    # wbActionFlag — wbDefinitionsCommon.pas (single flag byte, XACT).
    "wbActionFlag": "wbInteger(XACT,'Action Flag',itU32)",
    # wbWaterData — wbDefinitionsFO76.pas:4973-4985. FO76 uses XWCN (count u32) + XWCU (velocity array).
    # Not XWAT (old FO4 sig that no longer appears in FO76).
    "wbWaterData": (
        "wbRStruct('Water Current Velocities',"
        "[wbInteger(XWCN,'Velocity Count',itU32),"
        "wbArray(XWCU,'Velocities',"
        "wbStruct('Current',[wbVec3('Velocity'),wbFloat('Unknown')]))])"
    ),
    # wbAmbientColors — ambient lighting colors (no-sig struct form; sig form handled in expand_call).
    # FO76 branch: Directional (6×4-byte color entries) + wbUnused(4) + wbUnused(4).
    "wbAmbientColors": (
        "wbStruct('Directional Ambient Lighting Colors',"
        "[wbStruct('Directional',"
        "[wbByteColors('X+'),wbByteColors('X-'),wbByteColors('Y+'),"
        "wbByteColors('Y-'),wbByteColors('Z+'),wbByteColors('Z-')]),"
        "wbUnused(4),wbUnused(4)])"
    ),
    # wbByteColors — byte-precision color (no-sig struct form; sig form handled in expand_call).
    # 4 bytes: Red u8, Green u8, Blue u8, Unused u8.
    "wbByteColors": (
        "wbStruct('Color',"
        "[wbInteger('Red',itU8),wbInteger('Green',itU8),wbInteger('Blue',itU8),wbUnused(1)])"
    ),
    # wbWorldFixedCenter — wbDefinitionsCommon.pas:9262 (WCTR struct, 4 bytes).
    "wbWorldFixedCenter": (
        "wbStruct(WCTR,'Fixed Dimensions Center Cell',"
        "[wbInteger('X',itS16),wbInteger('Y',itS16)])"
    ),
    # wbWorldLODData — wbDefinitionsCommon.pas:9274 (rstruct, NAM3 formid + NAM4 float).
    "wbWorldLODData": (
        "wbRStruct('LOD Data',"
        "[wbFormIDCk(NAM3,'LOD Water',[WATR]),wbFloat(NAM4,'LOD Water Height')])"
    ),
    # wbWorldLandData — wbDefinitionsCommon.pas:9283 (DNAM struct, 2 floats).
    "wbWorldLandData": (
        "wbStruct(DNAM,'Land Data',"
        "[wbFloat('Default Land Height'),wbFloat('Default Water Height')])"
    ),
    # wbWorldLargeRefs — wbDefinitionsCommon.pas:9297 (RNAM cell grid of placed refs).
    "wbWorldLargeRefs": (
        "wbRArray('Large References',"
        "wbStruct(RNAM,'Cell',"
        "[wbInteger('Y',itS16),wbInteger('X',itS16),"
        "wbArray('References',"
        "wbStruct('Reference',"
        "[wbFormIDCk('Ref',[REFR]),wbInteger('Y',itS16),wbInteger('X',itS16)]),"
        "-1)]))"
    ),
    # wbWorldMapData — wbDefinitionsCommon.pas:9349 (MNAM struct). The IsTES5(...)
    # Camera Data branch is omitted: FO76 is not TES5, so it always resolves to nil.
    "wbWorldMapData": (
        "wbStruct(MNAM,'World Map Data',"
        "[wbStruct('Usable Dimensions',[wbInteger('X',itS32),wbInteger('Y',itS32)]),"
        "wbStruct('Cell Coordinates',"
        "[wbStruct('NW Cell',[wbInteger('X',itS16),wbInteger('Y',itS16)]),"
        "wbStruct('SE Cell',[wbInteger('X',itS16),wbInteger('Y',itS16)])])])"
    ),
    # wbWorldMapOffset — wbDefinitionsCommon.pas:9393 (ONAM struct, 4 floats).
    # IsSF1/IsFO3 branches all resolve to the plain (non-Starfield, non-FO3) form
    # for FO76; the "scale factor" args elsewhere are cosmetic edit-UI-only and
    # don't change the on-disk byte format, so they are omitted.
    "wbWorldMapOffset": (
        "wbStruct(ONAM,'World Map Offset Data',"
        "[wbFloat('World Map Scale'),wbFloat('Cell X Offset'),"
        "wbFloat('Cell Y Offset'),wbFloat('Cell Z Offset')])"
    ),
    # wbWorldObjectBounds — wbDefinitionsCommon.pas:9466 (NAM0/NAM9 min/max structs).
    "wbWorldObjectBounds": (
        "wbRStruct('Worldspace Bounds',"
        "[wbStruct(NAM0,'Min',[wbFloat('X'),wbFloat('Y')]),"
        "wbStruct(NAM9,'Max',[wbFloat('X'),wbFloat('Y')])])"
    ),
    # wbWorldRegionEditorMap — wbDefinitionsCommon.pas:9528 (NAM5 string + NAM6 bounds).
    "wbWorldRegionEditorMap": (
        "wbRStruct('Region Editor Map',"
        "[wbString(NAM5,'Texture'),"
        "wbStruct(NAM6,'Bounds',"
        "[wbInteger('NW Cell X',itS16),wbInteger('SE Cell Y',itS16),"
        "wbInteger('SE Cell X',itS16),wbInteger('NW Cell Y',itS16)])])"
    ),
    # wbWorldWaterHeightData — wbDefinitionsCommon.pas:9601 (XCLW/WHGT arrays).
    "wbWorldWaterHeightData": (
        "wbRStruct('Water Height Data',"
        "[wbArray(XCLW,'Cell Water Height Locations',"
        "wbStruct('Cell Water Height Location',"
        "[wbInteger('Cell Y',itS16),wbInteger('Cell X',itS16)])),"
        "wbArray(WHGT,'Water Heights',wbFloat('Water Height'))])"
    ),
    # wbWorldSwapsImpactData — wbDefinitionsCommon.pas:9547 (IMPS array + IMPF struct).
    "wbWorldSwapsImpactData": (
        "wbRStruct('Swaps Impact Data',"
        "[wbRArrayS('Impact Data',"
        "wbStructExSK(IMPS,[0,1],[2],'Impact Swap Data',"
        "[wbInteger('Material Type',itU32),"
        "wbFormIDCk('Original Data',[IPCT]),"
        "wbFormIDCk('New Data',[IPCT,NULL])])),"
        "wbStruct(IMPF,'Footstep Materials',"
        "[wbString('ConcSolid',30),wbString('ConcBroken',30),wbString('MetalSolid',30),"
        "wbString('MetalHollow',30),wbString('MetalSheet',30),wbString('Wood',30),"
        "wbString('Sand',30),wbString('Dirt',30),wbString('Grass',30),wbString('Water',30)])])"
    ),
    # wbWorldLevelData — wbDefinitionsCommon.pas:9325 ("World Default Level Data").
    # The Pascal has two sibling members both anchored on sig WLEV (a struct, then
    # a trailing byte array) inside one rstruct, which is an unusual/ambiguous
    # double-sig pattern this extractor doesn't have a clean way to express safely.
    # Represent the whole thing as one opaque byte blob instead of guessing at the
    # split — this is a rare, niche field (worldspace default level-of-detail data)
    # and opaque-bytes-when-uncertain is an established, safe pattern in this schema.
    "wbWorldLevelData": "wbByteArray(WLEV,'World Default Level Data',0)",
    # wbWorldCellSizeData / wbWorldOffsetData / wbWorldVisibleCellsData /
    # wbWorldMaxHeight's "Cell Heights" — these are dynamically-sized 2D grids whose
    # row/column counts come from runtime Pascal callback functions
    # (wbWorldColumnsCounter/wbWorldRowsCounter/wbMHDTColumnsCounter) that compute
    # extents from the Worldspace Bounds min/max — not modelled by this extractor.
    # Represent as opaque byte blobs (same pattern already used for the NVNM navmesh
    # geometry union across ACTI/FURN/STAT/TRAP, which is proven safe and does not
    # count as an "unmapped"/"raw_fallback" marker).
    "wbWorldCellSizeData": "wbByteArray(CLSZ,'Cell Sizes',0)",
    "wbWorldOffsetData": "wbByteArray(OFST,'Offsets',0)",
    "wbWorldVisibleCellsData": "wbByteArray(VISI,'Visible Cells',0)",
    "wbWorldMaxHeight": (
        "wbStruct(MHDT,'Max Height Data',"
        "[wbStruct('Dimensions',"
        "[wbStruct('Min',[wbInteger('X',itS16),wbInteger('Y',itS16)]),"
        "wbStruct('Max',[wbInteger('X',itS16),wbInteger('Y',itS16)])]),"
        "wbByteArray('Cell Heights',0)])"
    ),
    # wbCellGrid — wbDefinitionsCommon.pas:7976 (XCLC struct: X/Y s32 + Land Flags
    # u8 + 3 bytes padding). The trailing Land Flags/padding is Pascal's optional
    # "required count 2" pattern (only X/Y are mandatory) — this extractor doesn't
    # need to model that separately: decode_struct_fields already reads struct
    # fields sequentially and stops naturally when the subrecord's bytes run out,
    # so a shorter (X/Y-only) DATA payload decodes correctly without special-casing.
    "wbCellGrid": (
        "wbStruct(XCLC,'Grid',"
        "[wbInteger('X',itS32),wbInteger('Y',itS32),"
        "wbInteger('Land Flags',itU8),wbUnused(3)])"
    ),
    # wbMHDTCELL — wbDefinitionsCommon.pas:8442. IfThen(wbSimpleRecords, ...) always
    # resolves to the structured (non-simple) branch per this project's convention.
    # The grid is IsSF1(50, 32) rows/cols — 32 for FO76 (not Starfield).
    "wbMHDTCELL": (
        "wbStruct(MHDT,'Max Height Data',"
        "[wbFloat('Offset'),"
        "wbArray('Max Heights',wbArray('Row',wbInteger('Column',itU8),32),32)])"
    ),
    "wbWeatherCloudTextures": "wbRStruct('Cloud Textures',[" + ",".join(_cloud_tex_parts) + "])",
    # wbWeatherCloudSpeed — wbDefinitionsCommon.pas:8918-8937.
    # RStruct with RNAM (Y Speeds) and QNAM (X Speeds), each a 32-element byte array.
    "wbWeatherCloudSpeed": (
        "wbRStruct('Cloud Speeds',["
        "wbArray(RNAM,'Y Speeds',wbInteger('Layer',itU8),32),"
        "wbArray(QNAM,'X Speeds',wbInteger('Layer',itU8),32)"
        "])"
    ),
    # wbWeatherCloudColors — wbDefinitionsCommon.pas:8906-8916.
    # PNAM: array of cloud layer colors (wbWeatherTimeOfDay structs — complex union,
    # use bytearray to consume the subrecord without version-conditional parsing).
    "wbWeatherCloudColors": "wbByteArray(PNAM,'Cloud Colors',0)",
    # wbWeatherCloudAlphas — wbDefinitionsCommon.pas:8863-8904.
    # JNAM: array of 32 layers, each with 8 floats (time-of-day alpha values).
    "wbWeatherCloudAlphas": (
        "wbArray(JNAM,'Cloud Alphas',wbStruct('Layer',["
        "wbFloat('Sunrise'),wbFloat('Day'),wbFloat('Sunset'),wbFloat('Night'),"
        "wbFloat('Early Sunrise'),wbFloat('Late Sunrise'),"
        "wbFloat('Early Sunset'),wbFloat('Late Sunset')"
        "]),32)"
    ),
    # wbWeatherColors — wbDefinitionsCommon.pas:8993-9043.
    # NAM0: large struct of wbWeatherTimeOfDay entries — use bytearray.
    "wbWeatherColors": "wbByteArray(NAM0,'Weather Colors',0)",
    # wbWeatherFogDistance — wbDefinitionsCommon.pas:9081-9132.
    # FNAM: fog near/far distances + powers + heights — use bytearray.
    "wbWeatherFogDistance": "wbByteArray(FNAM,'Fog Distance',0)",
    # wbWeatherDisabledLayers — wbDefinitionsCommon.pas:9068-9079.
    # NAM1: 32-bit flags, one bit per cloud layer.
    "wbWeatherDisabledLayers": "wbInteger(NAM1,'Disabled Cloud Layers',itU32)",
    # wbWeatherImageSpaces — wbDefinitionsCommon.pas:9149-9170.
    # IMSP: struct of 8 IMGS formids (Sunrise/Day/Sunset/Night + Early/Late variants).
    "wbWeatherImageSpaces": (
        "wbStruct(IMSP,'Image Spaces',["
        "wbFormIDCk('Sunrise',[IMGS,NULL]),"
        "wbFormIDCk('Day',[IMGS,NULL]),"
        "wbFormIDCk('Sunset',[IMGS,NULL]),"
        "wbFormIDCk('Night',[IMGS,NULL]),"
        "wbFormIDCk('Early Sunrise',[IMGS,NULL]),"
        "wbFormIDCk('Late Sunrise',[IMGS,NULL]),"
        "wbFormIDCk('Early Sunset',[IMGS,NULL]),"
        "wbFormIDCk('Late Sunset',[IMGS,NULL])"
        "])"
    ),
    # wbWeatherGodRays — wbDefinitionsCommon.pas:9134-9147.
    # WGDR: struct of 8 GDRY formids.
    "wbWeatherGodRays": (
        "wbStruct(WGDR,'God Rays',["
        "wbFormIDCk('Sunrise',[GDRY,NULL]),"
        "wbFormIDCk('Day',[GDRY,NULL]),"
        "wbFormIDCk('Sunset',[GDRY,NULL]),"
        "wbFormIDCk('Night',[GDRY,NULL]),"
        "wbFormIDCk('Early Sunrise',[GDRY,NULL]),"
        "wbFormIDCk('Late Sunrise',[GDRY,NULL]),"
        "wbFormIDCk('Early Sunset',[GDRY,NULL]),"
        "wbFormIDCk('Late Sunset',[GDRY,NULL])"
        "])"
    ),
    # wbWeatherVolumetricLighting — wbDefinitionsCommon.pas:9219-9240.
    # HNAM: struct of 8 VOLI formids.
    "wbWeatherVolumetricLighting": (
        "wbStruct(HNAM,'Volumetric Lighting',["
        "wbFormIDCk('Sunrise',[VOLI,NULL]),"
        "wbFormIDCk('Day',[VOLI,NULL]),"
        "wbFormIDCk('Sunset',[VOLI,NULL]),"
        "wbFormIDCk('Night',[VOLI,NULL]),"
        "wbFormIDCk('Early Sunrise',[VOLI,NULL]),"
        "wbFormIDCk('Late Sunrise',[VOLI,NULL]),"
        "wbFormIDCk('Early Sunset',[VOLI,NULL]),"
        "wbFormIDCk('Late Sunset',[VOLI,NULL])"
        "])"
    ),
    # wbWeatherDirectionalLighting — wbDefinitionsCommon.pas:9045-9066.
    # Multiple DALC subrecords (one per time-of-day) in a wrapping RStruct.
    # Each DALC is 28 bytes (6 byteColors directional + 4 unused + 4 unused in FO76).
    "wbWeatherDirectionalLighting": (
        "wbRStruct('Directional Ambient Lighting Colors',["
        "wbByteArray(DALC,'Sunrise',0),"
        "wbByteArray(DALC,'Day',0),"
        "wbByteArray(DALC,'Sunset',0),"
        "wbByteArray(DALC,'Night',0),"
        "wbByteArray(DALC,'Early Sunrise',0),"
        "wbByteArray(DALC,'Late Sunrise',0),"
        "wbByteArray(DALC,'Early Sunset',0),"
        "wbByteArray(DALC,'Late Sunset',0)"
        "])"
    ),
    **{_v: "__vmad__" for _v in _VMAD_VARS},
}

# Binary IAD sig constants for IMAD record.
# Pascal: _00_IAD : TwbSignature = #$00'IAD', …, _54_IAD : TwbSignature = #$54'IAD'.
# wbDefinitionsSignatures.pas:1808-1866.
# Stored as 4-char Python strings; sig_id() accepts them via the IAD rule.
_IAD_SIGS: dict[str, str] = {}
for _iad_i in range(0x15):  # 0x00 .. 0x14 (Mult)
    _IAD_SIGS[f"_{_iad_i:02X}_IAD"] = chr(_iad_i) + "IAD"
for _iad_i in range(0x40, 0x55):  # 0x40 .. 0x54 (Add)
    _IAD_SIGS[f"_{_iad_i:02X}_IAD"] = chr(_iad_i) + "IAD"

BUILTIN_HELPERS_DEFAULTS: dict[str, str] = {
    "wbEDID": "wbStringKC(EDID, 'Editor ID', 0, cpOverride)",
    "wbFULL": "wbLStringKC(FULL, 'Name', 0, cpTranslate)",
    "wbDESC": "wbLStringKC(DESC, 'Description', 0, cpTranslate)",
    "wbDESCReq": "wbLStringKC(DESC, 'Description', 0, cpTranslate, True)",
    # Pascal functions (not variables) — inject as builtin helpers so they
    # appear in self.vars and are expanded by _parse_member_list.
    "wbMagicEffectSounds": (
        "wbArrayS(SNDD, 'Sounds', wbStruct('Sound', ["
        "wbInteger('Type', itU32, wbEnum(["
        "'Sheathe/Draw', 'Charge', 'Ready', 'Release',"
        "'Concentration Cast Loop', 'On Hit'])),"
        "wbFormIDCk('Sound', [SNDR])]))"
    ),
    "wbWeatherSounds": (
        "wbRArray('Sounds', wbStruct(SNAM, 'Sound', ["
        "wbFormIDCk('Sound', [SNDR]),"
        "wbInteger('Type', itU32, wbEnum(["
        "'Default', 'Precipitation', 'Wind', 'Thunder']))]))"
    ),
    # wbSoundDescriptorSounds — wbDefinitionsCommon.pas:8761-8764.
    # SNDR uses an RArray of ANAM strings (sound file paths), not a FormID.
    "wbSoundDescriptorSounds": "wbRArray('Sounds', wbString(ANAM, 'Sound'))",
    # ----------------------------------------------------------------
    # Effects — rstruct wrapping EFID + EFIT (bytes) + optional fields.
    # wbConditions is already modeled above.
    # ----------------------------------------------------------------
    "wbEFID": "wbFormIDCk(EFID,'Base Effect',[MGEF])",
    # ----------------------------------------------------------------
    # wbXLOD — wbDefinitionsCommon.pas:9624-9629.
    # XLOD subrecord: fixed array of 3 floats (Distant LOD data).
    # ----------------------------------------------------------------
    "wbXLOD": "wbArray(XLOD,'Distant LOD Data',wbFloat('Unknown'),3)",
    # ----------------------------------------------------------------
    # wbWeatherLightningColor — wbDefinitionsCommon.pas:9172-9179.
    # Sigless struct field: Red/Green/Blue u8.  Used bare (no parens)
    # inside a wbArray element struct in WTHR.
    # ----------------------------------------------------------------
    "wbWeatherLightningColor": (
        "wbStruct('Lightning Color',["
        "wbInteger('Red',itU8),"
        "wbInteger('Green',itU8),"
        "wbInteger('Blue',itU8)"
        "])"
    ),
    # ----------------------------------------------------------------
    # wbVec3PosRot — bare (sigless) usage inside struct member lists.
    # The (SIG) form is handled in expand_call; the bare var reference
    # (e.g. wbVec3PosRot inside wbStruct XTEL) needs the var map.
    # wbDefinitionsCommon.pas:8715-8720 — 24-byte position+rotation block.
    # ----------------------------------------------------------------
    "wbVec3PosRot": "wbByteArray('Position/Rotation', 24)",
    # ----------------------------------------------------------------
    # wbINOA / wbINOM — editor-only INFO-order arrays (DIAL record).
    # wbDefinitionsCommon.pas:8164-8182.  These are flagged dfDontSave
    # and should not appear in binary ESM data, but we model them anyway.
    # ----------------------------------------------------------------
    "wbINOA": "wbArray(INOA,'INFO Order (All previous modules)',wbFormIDCk('INFO',[INFO]))",
    "wbINOM": "wbArray(INOM,'INFO Order (Masters only)',wbFormIDCk('INFO',[INFO]))",
    # ----------------------------------------------------------------
    # wbFactionRelations — wbDefinitionsCommon.pas:8100-8117.
    # RArrayS of XNAM structs: faction/race formid + s32 modifier + enum.
    # IsTES4(nil, ...) → FO76 includes the Group Combat Reaction field.
    # ----------------------------------------------------------------
    "wbFactionRelations": (
        "wbRArrayS('Relations',"
        "wbStructSK(XNAM,[0],'Relation',["
        "wbFormIDCk('Faction',[FACT,RACE]),"
        "wbInteger('Modifier',itS32),"
        "wbInteger('Group Combat Reaction',itU32,wbEnum(["
        "'Neutral','Enemy','Ally','Friend'"
        "]))]))"
    ),
    # ----------------------------------------------------------------
    # wbActorSounds — wbDefinitionsCommon.pas:7959-7975.
    # RArrayS of (CS2K keyword + CS2D sound) pairs, count from CS2H.
    # ----------------------------------------------------------------
    "wbActorSounds": (
        "wbRArrayS('Sounds',"
        "wbRStructSK([0],'Sound',["
        "wbFormIDCk(CS2K,'Keyword',[KYWD]),"
        "wbFormIDCk(CS2D,'Sound',[SNDR])]))"
    ),
    # ----------------------------------------------------------------
    # wbIdleAnimation — wbDefinitionsCommon.pas:8186-8223.
    # FO76 branch: IDLF flags (u8) + IDLC animation count (u8) +
    # IDLT timer float + IDLA animations array + IDLB unknown.
    # IsFO3(a, b) → b for FO76; IsSF1(a, b) → b for FO76.
    # ----------------------------------------------------------------
    "wbIdleAnimation": (
        "wbRStruct('Idle Animations',["
        "wbInteger(IDLF,'Flags',itU8,wbFlags(["
        "'Run In Sequence','','Do Once','Loose Only',"
        "'','','','',"
        "'Ignored By Sandbox'])),"
        "wbInteger(IDLC,'Animation Count',itU8),"
        "wbFloat(IDLT,'Idle Timer Setting'),"
        "wbArray(IDLA,'Animations',wbFormIDCk('Animation',[IDLE,NULL])),"
        "wbUnknown(IDLB)"
        "])"
    ),
    # ----------------------------------------------------------------
    # wbRegionAreas — wbDefinitionsCommon.pas:8712-8728.
    # FO76 branch includes ANAM (unknown extra bytes).
    # ----------------------------------------------------------------
    "wbRegionAreas": (
        "wbRArray('Region Areas',"
        "wbRStruct('Region Area',["
        "wbInteger(RPLI,'Edge Fall-off',itU32),"
        "wbArray(RPLD,'Points',wbStruct('Point',[wbFloat('X'),wbFloat('Y')])),"
        "wbByteArray(ANAM,'Unknown',0)"
        "]))"
    ),
    # ----------------------------------------------------------------
    # wbRegionSounds — wbDefinitionsCommon.pas:8729-8766.
    # FO76 branch: RDSA sig, wbFloat('Chance') (not wbScaledInt4).
    # ----------------------------------------------------------------
    "wbRegionSounds": (
        "wbArrayS(RDSA,'Sounds',"
        "wbStructSK([0],'Sound',["
        "wbFormIDCk('Sound',[SNDR,SOUN,NULL]),"
        "wbInteger('Flags',itU32,wbFlags(["
        "'Pleasant','Cloudy','Rainy','Snowy'"
        "])),"
        "wbFloat('Chance')"
        "]))"
    ),
    # ----------------------------------------------------------------
    # wbStaticPartPlacements — wbDefinitionsCommon.pas:8784-8800.
    # DATA array of Placement structs: position (3 floats) + rotation
    # (3 floats, same wire format as floats) + scale float.
    # ----------------------------------------------------------------
    "wbStaticPartPlacements": (
        "wbArrayS(DATA,'Placements',"
        "wbStruct('Placement',["
        "wbStruct('Position',[wbFloat('X'),wbFloat('Y'),wbFloat('Z')]),"
        "wbStruct('Rotation',[wbFloat('X'),wbFloat('Y'),wbFloat('Z')]),"
        "wbFloat('Scale')"
        "]))"
    ),
    **_IAD_SIGS,
}


# ============================================================================
# CALL_EXPANSIONS
# ============================================================================


@dataclass
class ExpandContext:
    """Runtime hooks a CALL_EXPANSIONS callable needs from
    `Extractor.expand_call` — its live `self.vars`, the un-truncated original
    call expression, and the handful of module-level parsing primitives
    `extract.py` defines (`split_top_level`, `unquote`, `sig_id`,
    `find_matching_bracket`) — passed in by the caller so this module never
    has to import back from `extract.py`."""

    vars: dict[str, str]
    raw_expr: str
    split_top_level: Callable[[str], list[str]]
    unquote: Callable[[str], str]
    sig_id: Callable[[str], str | None]
    find_matching_bracket: Callable[[str, int], int]


def _expand_wbGenericModel(args: str, ctx: ExpandContext) -> str | None:
    return ctx.vars["wbGenericModel"]


def _expand_wbEnchantment(args: str, ctx: ExpandContext) -> str | None:
    return ctx.vars["wbEnchantment"]


def _expand_wbOBND(args: str, ctx: ExpandContext) -> str | None:
    return ctx.vars["wbOBND"]


def _expand_wbDamageTypeArray(args: str, ctx: ExpandContext) -> str | None:
    parts = ctx.split_top_level(args)
    name = ctx.unquote(parts[0]) if parts else "Item"
    # Include the form-version 152 Curve Table field.
    # wbDefinitionsCommon.pas:8014-8024.
    return (
        f"wbArrayS(DAMA, '{name}s', wbStructSK([0], '{name}', ["
        f"wbFormIDCk('Type', [DMGT]),"
        f"wbInteger('Amount', itU32),"
        f"wbFromVersion(152, wbFormIDCk('Curve Table', [CURV, NULL]))]))"
    )


def _expand_wbModelInfo(args: str, ctx: ExpandContext) -> str | None:
    parts = ctx.split_top_level(args)
    sig = parts[0].strip() if parts else "MODT"
    return f"wbByteArray({sig}, 'Model Information', 0)"


def _expand_wbFloatRGBA(args: str, ctx: ExpandContext) -> str | None:
    # wbFloatRGBA(SIG) → wbStruct(SIG, 'Color', [...]) — substitute sig
    parts = ctx.split_top_level(args)
    sig2 = parts[0].strip() if parts else ""
    if ctx.sig_id(sig2):
        return (
            f"wbStruct({sig2}, 'Color', ["
            f"wbFloat('Red'), wbFloat('Green'), wbFloat('Blue'), wbFloat('Alpha')])"
        )
    return ctx.vars.get("wbFloatRGBA", ctx.raw_expr)


def _expand_wbByteColors(args: str, ctx: ExpandContext) -> str | None:
    # wbByteColors([SIG,] ['name']) → 4-byte struct (R u8, G u8, B u8, Unused u8).
    # wbDefinitionsCommon.pas:6291-6305.  The no-arg/bare form falls through to
    # the BUILTIN_HELPERS["wbByteColors"] substitution.
    bc_parts = ctx.split_top_level(args)
    if bc_parts and ctx.sig_id(bc_parts[0].strip()):
        sig2 = bc_parts[0].strip()
        bc_name = ctx.unquote(bc_parts[1]) if len(bc_parts) > 1 else "Color"
        return (
            f"wbStruct({sig2},'{bc_name}',"
            f"[wbInteger('Red',itU8),wbInteger('Green',itU8),"
            f"wbInteger('Blue',itU8),wbUnused(1)])"
        )
    bc_name = (
        ctx.unquote(bc_parts[0])
        if bc_parts and bc_parts[0].strip().startswith("'")
        else "Color"
    )
    return (
        f"wbStruct('{bc_name}',"
        f"[wbInteger('Red',itU8),wbInteger('Green',itU8),"
        f"wbInteger('Blue',itU8),wbUnused(1)])"
    )


def _expand_wbAmbientColors(args: str, ctx: ExpandContext) -> str | None:
    # wbAmbientColors([SIG,] ['name']) → 32-byte struct (FO76 branch).
    # Layout: Directional inner-struct (6×4-byte wbByteColors) + wbUnused(4) + wbUnused(4).
    # wbDefinitionsCommon.pas:6238-6263 (IsFO76 branch = wbUnused(4) for both SF1 slots).
    ac_parts = ctx.split_top_level(args)
    _directional = (
        "wbStruct('Directional',"
        "[wbByteColors('X+'),wbByteColors('X-'),wbByteColors('Y+'),"
        "wbByteColors('Y-'),wbByteColors('Z+'),wbByteColors('Z-')])"
    )
    if ac_parts and ctx.sig_id(ac_parts[0].strip()):
        sig2 = ac_parts[0].strip()
        ac_name = (
            ctx.unquote(ac_parts[1]) if len(ac_parts) > 1 else "Directional Ambient Lighting Colors"
        )
        return (
            f"wbStruct({sig2},'{ac_name}',"
            f"[{_directional},wbUnused(4),wbUnused(4)])"
        )
    ac_name = (
        ctx.unquote(ac_parts[0])
        if ac_parts and ac_parts[0].strip().startswith("'")
        else "Directional Ambient Lighting Colors"
    )
    return (
        f"wbStruct('{ac_name}',"
        f"[{_directional},wbUnused(4),wbUnused(4)])"
    )


def _expand_wbVec3PosRot(args: str, ctx: ExpandContext) -> str | None:
    # wbVec3PosRot(SIG) → bytes (24 bytes = pos xyz + rot xyz)
    parts = ctx.split_top_level(args)
    sig2 = parts[0].strip() if parts else "DATA"
    return f"wbByteArray({sig2}, 'Position/Rotation', 24)"


def _expand_wbSizePosRot(args: str, ctx: ExpandContext) -> str | None:
    # wbSizePosRot(SIG, name) → bytes (36 bytes: Size 2f + Pos 3f + Quat 4f).
    # wbDefinitionsCommon.pas:6205-6234.
    parts = ctx.split_top_level(args)
    sig2 = parts[0].strip() if parts else ""
    spr_name = ctx.unquote(parts[1]) if len(parts) > 1 else "Size/Pos/Rot"
    if ctx.sig_id(sig2):
        return f"wbByteArray({sig2}, '{spr_name}', 36)"
    return None  # unrecognized sig — fall through to the caller's default


def _expand_wbDebrisModel(args: str, ctx: ExpandContext) -> str | None:
    # wbDebrisModel(textureHashes) → rstruct with DATA struct + hashes
    return (
        "wbRStruct('Model',["
        "wbStruct(DATA,'Data',["
        "wbInteger('Percentage',itU8),"
        "wbString('Model FileName'),"
        "wbInteger('Has Collision',itU8,wbBoolEnum)]),"
        f"{args}])"
    )


def _expand_wbTexturedModel(args: str, ctx: ExpandContext) -> str | None:
    # wbTexturedModel('Name', [modSig, txtSig], [extras...])
    # wbDefinitionsCommon.pas:8799-8830 (FO76 branch = filename + model-info + extras).
    # Emits an rstruct with: model filename string, model-info bytes, then the
    # extra subrecord members (MODC, MO2S/MO4S, ENLT, ENLS, AUUV, etc.).
    t_parts = ctx.split_top_level(args)
    t_name = ctx.unquote(t_parts[0]) if t_parts else "Model"
    # Parse signature list: [MOD2, MO2T]
    mod_sig, txt_sig = "MODL", "MODT"
    if len(t_parts) > 1:
        sig_part = t_parts[1].strip()
        if sig_part.startswith("["):
            sig_end = ctx.find_matching_bracket(sig_part, 0)
            sig_inner = sig_part[1:sig_end]
            sig_list = [s.strip() for s in ctx.split_top_level(sig_inner)]
            if len(sig_list) >= 1:
                mod_sig = sig_list[0]
            if len(sig_list) >= 2:
                txt_sig = sig_list[1]
    members_out = [
        f"wbString({mod_sig},'Model Filename')",
        f"wbByteArray({txt_sig},'Model Information',0)",
    ]
    # Parse extras list: [wbMODC, wbMO2S, ...]
    if len(t_parts) > 2:
        extras_part = t_parts[2].strip()
        if extras_part.startswith("["):
            ext_end = ctx.find_matching_bracket(extras_part, 0)
            extras_inner = extras_part[1:ext_end]
            for e in ctx.split_top_level(extras_inner):
                e = e.strip()
                if e:
                    members_out.append(e)
    return f"wbRStruct('{t_name}',[{','.join(members_out)}])"


def _expand_wbStructs(args: str, ctx: ExpandContext) -> str | None:
    # wbStructs(sig, groupName, elementName, [fields]) →
    # wbArrayS(sig, groupName, wbStruct(elementName, [fields]))
    # wbDefinitionsCommon.pas interface lines 4467-4475.
    ws_parts = ctx.split_top_level(args)
    if len(ws_parts) >= 4 and ctx.sig_id(ws_parts[0].strip()):
        sig2 = ws_parts[0].strip()
        ws_name = ctx.unquote(ws_parts[1])
        ws_elem = ctx.unquote(ws_parts[2])
        ws_fields = ws_parts[3].strip()
        return f"wbArrayS({sig2},'{ws_name}',wbStruct('{ws_elem}',{ws_fields}))"
    if len(ws_parts) >= 3:
        ws_name = ctx.unquote(ws_parts[0])
        ws_elem = ctx.unquote(ws_parts[1])
        ws_fields = ws_parts[2].strip()
        return f"wbArray('{ws_name}',wbStruct('{ws_elem}',{ws_fields}))"
    return ctx.raw_expr


# wbClimateTiming(timeCallback, phaseCallback) → wbStruct(TNAM, 'Timing', [...]).
# Callbacks are display-only; the FO76 phase field is always present (callback
# is non-nil).  wbDefinitionsCommon.pas:7995-8010.  Args are ignored entirely
# by the original branch, so this is a plain template rather than a callable.
_WB_CLIMATE_TIMING = (
    "wbStruct(TNAM,'Timing',["
    "wbStruct('Sunrise',["
    "wbInteger('Begin',itU8),"
    "wbInteger('End',itU8)]),"
    "wbStruct('Sunset',["
    "wbInteger('Begin',itU8),"
    "wbInteger('End',itU8)]),"
    "wbInteger('Volatility',itU8),"
    "wbInteger('Moons / Phase Length',itU8)"
    "])"
)


def _expand_wbRFloatColors(args: str, ctx: ExpandContext) -> str | None:
    # wbRFloatColors(name, [sig0, sig1, sig2]) →
    # wbRStruct(name, [wbFloat(sig0,'Red'), wbFloat(sig1,'Green'), wbFloat(sig2,'Blue')])
    # wbDefinitionsCommon.pas:6450-6465.
    rf_parts = ctx.split_top_level(args)
    rf_name = ctx.unquote(rf_parts[0]) if rf_parts else "Color"
    sigs = ["ENAM", "FNAM", "GNAM"]
    if len(rf_parts) > 1:
        sig_str = rf_parts[1].strip()
        if sig_str.startswith("["):
            found = re.findall(r"[A-Z0-9_]{4}", sig_str)
            if len(found) >= 3:
                sigs = found[:3]
    return (
        f"wbRStruct('{rf_name}',"
        f"[wbFloat({sigs[0]},'Red'),"
        f"wbFloat({sigs[1]},'Green'),"
        f"wbFloat({sigs[2]},'Blue')])"
    )


def _expand_wbNPCTemplateActorEntry(args: str, ctx: ExpandContext) -> str | None:
    # wbNPCTemplateActorEntry('Name') → wbFormIDCk('Name', [BMMO, LVLN, NPC_, NULL])
    # wbDefinitionsCommon.pas:7834-7836.
    t_parts = ctx.split_top_level(args)
    t_name = ctx.unquote(t_parts[0]) if t_parts else "Actor"
    return f"wbFormIDCk('{t_name}', [BMMO, LVLN, NPC_, NULL])"


def _expand_wbFaceMorphs(args: str, ctx: ExpandContext) -> str | None:
    fm_parts = ctx.split_top_level(args)
    fm_name = ctx.unquote(fm_parts[0]) if fm_parts else "Face Morphs"
    return f"wbRArray('{fm_name}',wbFaceMorphElement)"


def _expand_wbMorphGroups(args: str, ctx: ExpandContext) -> str | None:
    mg_parts = ctx.split_top_level(args)
    mg_name = ctx.unquote(mg_parts[0]) if mg_parts else "Morph Groups"
    return f"wbRArray('{mg_name}',wbMorphGroupElement)"


def _expand_wbTintTemplateGroups(args: str, ctx: ExpandContext) -> str | None:
    tt_parts = ctx.split_top_level(args)
    tt_name = ctx.unquote(tt_parts[0]) if tt_parts else "Tint Layers"
    return f"wbRArray('{tt_name}',wbTintTemplateGroupElement)"


def _expand_wbIMADMultAddCount(args: str, ctx: ExpandContext) -> str | None:
    # wbIMADMultAddCount(name) → wbStruct with Mult Count + Add Count u32 fields.
    # wbDefinitionsCommon.pas:7768-7789.
    imad_parts = ctx.split_top_level(args)
    imad_name = ctx.unquote(imad_parts[0]) if imad_parts else "Unknown"
    return (
        f"wbStruct('{imad_name}',"
        f"[wbInteger('Mult Count',itU32),wbInteger('Add Count',itU32)])"
    )


def _expand_wbTimeInterpolators(args: str, ctx: ExpandContext) -> str | None:
    # wbTimeInterpolators(sig, name)  — array of {Time float, Value float} structs.
    # wbTimeInterpolators(name)       — sigless form used inside wbFromVersion wrappers.
    # wbDefinitionsCommon.pas:7886-7893 (no-sig), 8832-8841 (with-sig).
    ti_parts = ctx.split_top_level(args)
    _elem = "wbStruct('Data',[wbFloat('Time'),wbFloat('Value')])"
    if len(ti_parts) >= 2 and ctx.sig_id(ti_parts[0].strip()):
        ti_sig = ti_parts[0].strip()
        ti_name = ctx.unquote(ti_parts[1])
        return f"wbArray({ti_sig},'{ti_name}',{_elem})"
    if ti_parts:
        ti_name = ctx.unquote(ti_parts[0])
        return f"wbArray('{ti_name}',{_elem})"
    return ctx.raw_expr


CALL_EXPANSIONS: dict[str, str | Callable[[str, ExpandContext], str | None]] = {
    "wbGenericModel": _expand_wbGenericModel,
    "wbEnchantment": _expand_wbEnchantment,
    "wbOBND": _expand_wbOBND,
    "wbDamageTypeArray": _expand_wbDamageTypeArray,
    "wbModelInfo": _expand_wbModelInfo,
    "wbFloatRGBA": _expand_wbFloatRGBA,
    "wbByteColors": _expand_wbByteColors,
    "wbAmbientColors": _expand_wbAmbientColors,
    "wbVec3PosRot": _expand_wbVec3PosRot,
    "wbSizePosRot": _expand_wbSizePosRot,
    "wbDebrisModel": _expand_wbDebrisModel,
    "wbTexturedModel": _expand_wbTexturedModel,
    "wbStructs": _expand_wbStructs,
    "wbClimateTiming": _WB_CLIMATE_TIMING,
    "wbRFloatColors": _expand_wbRFloatColors,
    "wbNPCTemplateActorEntry": _expand_wbNPCTemplateActorEntry,
    "wbFaceMorphs": _expand_wbFaceMorphs,
    "wbMorphGroups": _expand_wbMorphGroups,
    "wbTintTemplateGroups": _expand_wbTintTemplateGroups,
    "wbIMADMultAddCount": _expand_wbIMADMultAddCount,
    "wbTimeInterpolators": _expand_wbTimeInterpolators,
    # wbTimeInterpolatorsMultAdd is absent: it builds and caches a
    # pre-parsed schema dict via Extractor._inline_members/_inline_counter
    # (state this table's plain str | None contract can't carry), so it
    # stays a coded special case in Extractor.expand_call.
}
