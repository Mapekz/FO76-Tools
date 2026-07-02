#!/usr/bin/env python3
"""Extract FO76 record schemas from xEdit Pascal definitions → schema/fo76.json."""

from __future__ import annotations

import json
import re
import sys
import copy
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TES5 = ROOT.parent / "TES5Edit"
FO76_PAS = TES5 / "Core" / "wbDefinitionsFO76.pas"
COMMON_PAS = TES5 / "Core" / "wbDefinitionsCommon.pas"
OUT = ROOT / "schema" / "fo76.json"
CTDA_OUT = ROOT / "schema" / "fo76.ctda.json"
OVERRIDES = ROOT / "schema" / "fo76.overrides.json"

# Record types present in the FO76 ESM (excluding CELL and WRLD until Part C).
# Generated from: esm tree /path/to/data | jq '[.[].label.sig] | unique | sort | .[]'
WHITELIST = [
    "AACT", "AAMD", "AAPD", "ACHR", "ACTI", "ADDN", "AECH", "ALCH", "AMDL", "AMMO",
    "ANIO", "AORU", "ARMA", "ARMO", "ARTO", "ASPC", "ASTM", "ASTP", "ATXO",
    "AUVF", "AVIF", "AVTR", "BNDS", "BOOK", "BPTD", "CAMS", "CHAL", "CLAS",
    "CLFM", "CLMT", "CMPO", "CMPT", "CNCY", "CNDF", "COBJ", "COEN", "COLL",
    "CONT", "CPRD", "CPTH", "CSEN", "CSTY", "CURV", "DCGF", "DEBR", "DFOB",
    "DIAL", "DLBR", "DIST", "DLVW", "DMGT", "DOBJ", "DOOR", "ECAT", "EFSH", "EMOT", "ENCH",
    "ENTM", "EQUP", "EXPL", "FACT", "FISH", "FLOR", "FLST", "FSTP", "FSTS",
    "FURN", "GCVR", "GDRY", "GLOB", "GMRW", "GMST", "GRAS", "HAZD", "HDPT",
    "IDLE", "IDLM", "IMAD", "IMGS", "INFO", "INGR", "INNR", "IPCT", "IPDS", "KEYM",
    "KSSM", "KYWD", "LAYR", "LCRT", "LCTN", "LENS", "LGDI", "LGTM", "LIGH",
    "LOUT", "LSCR", "LTEX", "LVLI", "LVLN", "LVLP", "LVPC", "MATO", "MATT",
    "MDSP", "MESG", "MGEF", "MISC", "MOVT", "MSTT", "MSWP", "MUSC", "MUST",
    "NAVI", "NOCM", "NOTE", "NPC_", "OMOD", "OTFT", "OVIS", "PACH", "PACK",
    "PCRD", "PEPF", "PERK", "PGRE", "PHZD", "PKIN", "PLYR", "PLYT", "PMFT", "PMIS", "PPAK", "PROJ", "QMDL",
    "QUST", "RACE", "REFR", "REGN", "RELA", "RESO", "REVB", "RFCT", "RFGP", "SCCO",
    "SCEN", "SCOL", "SCSN", "SECH", "SMBN", "SMEN", "SMQN", "SNCT", "SNDR", "SOPM",
    "SOUN", "SPEL", "SPGD", "STAG", "STAT", "STHD", "STMP", "STND", "TACT",
    "TEPF", "TERM", "TRAP", "TREE", "TRNS", "TXST", "UTIL", "VOLI", "VTYP",
    "WATR", "WAVE", "WEAP", "WSPR", "WTHR", "ZOOM",
]

# xEdit encodes the inline count-prefix byte width in the negative wbArray count
# argument; see TwbArrayDef.GetPrefixLength in TES5Edit/Core/wbInterface.pas.
#   -1  →  4-byte (u32) prefix
#   -2  →  2-byte (u16) prefix
#   -4  →  1-byte (u8)  prefix
# Any other negative value defaults to 4 (safe fallback; emits a warning).
_PREFIX_WIDTHS: dict[int, int] = {-1: 4, -2: 2, -4: 1}

# Closure decider names that we can substitute with a known type.
# Values may be a flat field dict OR a full {"kind": "union", ...} dict.
# Used in _parse_union to produce structured output for common OMOD deciders.
KNOWN_UNION_DECIDERS: dict[str, dict] = {
    # Function Type: variant determined by Value Type.
    #   0=Int/1=Float/6=FormID,Float → [SET, MUL+ADD, ADD]
    #   2=Bool                        → [SET, AND, OR]
    #   5=Enum                        → [SET]
    #   4=FormID,Int                  → [SET, REM, ADD]
    "wbOMODDataFunctionTypeDecider": {
        "kind": "union",
        "name": "Function Type",
        "decider": {
            "field": "Value Type",
            "default_variant": 0,
            "map": {"0": 0, "1": 0, "2": 1, "4": 3, "5": 2, "6": 0},
        },
        "variants": [
            {"kind": "integer", "name": "Function Type", "width": "u8", "signed": False,
             "format": {"enum": ["SET", "MUL+ADD", "ADD"]}},
            {"kind": "integer", "name": "Function Type", "width": "u8", "signed": False,
             "format": {"enum": ["SET", "AND", "OR"]}},
            {"kind": "integer", "name": "Function Type", "width": "u8", "signed": False,
             "format": {"enum": ["SET"]}},
            {"kind": "integer", "name": "Function Type", "width": "u8", "signed": False,
             "format": {"enum": ["SET", "REM", "ADD"]}},
        ],
    },
    # Value 1: type determined by Value Type.
    #   0=Int → s32,  1=Float → f32,  2=Bool → u32 bool,
    #   4=FormID,Int / 6=FormID,Float → formid,  5=Enum → u32,  default → bytes(4)
    "wbOMODDataPropertyValue1Decider": {
        "kind": "union",
        "name": "Value 1",
        "decider": {
            "field": "Value Type",
            "default_variant": 0,
            "map": {"0": 1, "1": 2, "2": 3, "4": 4, "5": 5, "6": 4},
        },
        "variants": [
            {"kind": "bytes", "name": "Value 1", "len": 4},
            {"kind": "integer", "name": "Value 1", "width": "s32", "signed": True},
            {"kind": "float", "name": "Value 1"},
            {"kind": "integer", "name": "Value 1", "width": "u32", "signed": False,
             "format": {"enum": ["False", "True"]}},
            {"kind": "formid", "name": "Value 1", "valid_refs": []},
            {"kind": "integer", "name": "Value 1", "width": "u32", "signed": False},
        ],
    },
    # Value 2: type determined by Value Type.
    #   0=Int / 4=FormID,Int → u32,  1=Float / 6=FormID,Float → f32,
    #   2=Bool → u32 bool,  default → unused (4 bytes)
    "wbOMODDataPropertyValue2Decider": {
        "kind": "union",
        "name": "Value 2",
        "decider": {
            "field": "Value Type",
            "default_variant": 0,
            "map": {"0": 1, "1": 2, "2": 3, "4": 1, "6": 2},
        },
        "variants": [
            {"kind": "unused", "bytes": 4},
            {"kind": "integer", "name": "Value 2", "width": "u32", "signed": False},
            {"kind": "float", "name": "Value 2"},
            {"kind": "integer", "name": "Value 2", "width": "u32", "signed": False,
             "format": {"enum": ["False", "True"]}},
        ],
    },
    # wbRecordSizeDecider(1): QRRI is 0 bytes (empty) or 1 byte (u8 bool).
    "wbRecordSizeDecider": {"kind": "bytes", "name": None, "len": None},

    # ---- Decider-only substitutions (no "kind" key): use Pascal variants as-is ----
    # wbGMSTUnionDecider: first char of EDID → variant index.
    #   's' → 0 (lstring Name), 'i' → 1 (s32 Int, default), 'f' → 2 (Float),
    #   'b' → 3 (Bool u32), 'u' → 4 (UInt u32)
    "wbGMSTUnionDecider": {
        "edid_prefix": {"s": 0, "f": 2, "b": 3, "u": 4},
        "edid_default": 1,
    },

    # wbBOOKTeachesDecider: bitmask on sibling 'Flags' (u8 in DNAM struct).
    #   bit 0x01 → 1 (Actor Value), bit 0x04 → 2 (Spell), bit 0x10 → 3 (Perk), else 0 (Unused)
    "wbBOOKTeachesDecider": {
        "field": "Flags",
        "default_variant": 0,
        "map": {},
        "bits": [[0x01, 1], [0x04, 2], [0x10, 3]],
    },

    # wbACBSLevelDecider: bitmask on sibling 'Flags' (u32 in NPC_ ACBS struct).
    #   bit 0x80 → 1 (Level Mult / PC-Level-Mult), else 0 (absolute Level u16)
    "wbACBSLevelDecider": {
        "field": "Flags",
        "default_variant": 0,
        "map": {},
        "bits": [[0x80, 1]],
    },

    # wbSNDRDataDecider: field-value check on 'Descriptor Type' (CNAM enum).
    #   0xED157AE3 = 3977742051 = 'AutoWeapon' → 1 (Base Descriptor formid)
    #   all other types → 0 (Values struct)
    "wbSNDRDataDecider": {
        "field": "Descriptor Type",
        "default_variant": 0,
        "map": {"3977742051": 1},
        "bits": [],
    },

    # wbNoteTypeDecider: field-value check on sibling 'Type' (DNAM integer).
    #   0 → 1 (Sound [SNDR]), 1 → 2 (Scene [SCEN]), 3 → 3 (Terminal [TERM])
    #   else → 0 (Unused 4 bytes)
    "wbNoteTypeDecider": {
        "field": "Type",
        "default_variant": 0,
        "map": {"0": 1, "1": 2, "3": 3},
        "bits": [],
    },

    # wbSceneActionTypeDecider: field-value check on sibling ANAM 'Type' (u16).
    #   0=Dialogue, 1=Package, 2=Timer, 3=Player Dialogue,
    #   4=Start Scene, 5=NPC Response, 6=Radio
    "wbSceneActionTypeDecider": {
        "field": "Type",
        "default_variant": 0,
        "map": {"0": 0, "1": 1, "2": 2, "3": 3, "4": 4, "5": 5, "6": 6},
        "bits": [],
    },

    # wbPubPackCNAMDecider: field-value check on sibling ANAM 'Type' (string).
    #   'Bool' → 1 (u8 bool), 'Int' → 2 (u32 integer), 'Float' → 3 (float)
    #   anything else (ObjectList, Location, Target, Topic, …) → 0 (bytes)
    "wbPubPackCNAMDecider": {
        "field": "Type",
        "default_variant": 0,
        "map": {"Bool": 1, "Int": 2, "Float": 3},
        "bits": [],
    },

    # wbMGEFAssocItemDecider: reads Archetype at byte-offset 56 from the union's
    # position in the DATA payload (4-byte LE u32); maps archetype → variant index.
    # Variants: 0=Unused, 1=Light, 2=BoundItem, 3=Summon, 4=Hazd/Guide, 5=Cloak,
    #           6=Race, 7=Ench, 8=Kywd, 9=Sthd(ValMod), 10=Dmgt, 11=Emot, 12=Flst
    "wbMGEFAssocItemDecider": {
        "kind": "union",  # full substitution — overrides name and variants too
        "name": "Assoc. Item",
        "decider": {
            "byte_offset": 56,
            "width_bytes": 4,
            "default_variant": 0,
            "map": {
                "0": 9, "1": 0, "4": 0, "5": 0, "6": 0, "7": 0, "8": 0, "9": 0,
                "11": 0, "12": 1, "17": 2, "18": 3, "20": 11, "21": 0, "25": 4,
                "28": 0, "31": 0, "33": 0, "34": 8, "35": 5, "36": 6, "37": 0,
                "39": 7, "40": 4, "45": 10, "46": 6, "47": 0, "48": 0, "49": 0,
                "50": 12,
            },
        },
        "variants": [
            {"kind": "formid", "name": "Unused", "valid_refs": ["NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["LIGH", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["WEAP", "ARMO", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["NPC_", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["HAZD", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["SPEL", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["RACE", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["ENCH", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["KYWD", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["STHD", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["DMGT", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["EMOT", "NULL"]},
            {"kind": "formid", "name": "Assoc. Item", "valid_refs": ["FLST", "NULL"]},
        ],
    },
    # wbCOEDOwnerDecider: resolve sibling Owner FormID → NPC_=Global Variable,
    # FACT=Required Rank, else Unused(4).
    "wbCOEDOwnerDecider": {
        "form_id_target_type": "Owner",
        "default_variant": 0,
        "map": {"NPC_": 1, "FACT": 2},
    },
    # wbTypeDecider: reads sibling field named "Type" as variant index (FO76.pas:3123).
    # Identity map is materialized per-site in _parse_union once the variant count is known.
    "wbTypeDecider": {"field": "Type", "__identity_map__": True},
    # wbAECHDataDecider (FO76.pas:2301): reads the outer Effect rstruct's KNAM "Type"
    # sparse enum. Keys are the decimal Sig2Int values of BSOverdrive (0x864804BE),
    # BSStateVariableFilter (0xEF575F7F), BSDelayEffect (0x18837B4F).
    "wbAECHDataDecider": {
        "field": "Type",
        "map": {"2252866750": 0, "4015480703": 1, "411269967": 2},
    },
    # wbNAVIIslandDataDecider (Common.pas:5329): sibling "Has Island Data" u8 bool.
    "wbNAVIIslandDataDecider": {
        "field": "Has Island Data",
        "default_variant": 0,
        "map": {"0": 0, "1": 1},
    },
    # wbNAVIParentDecider (Common.pas:5350): null Parent World FormID → variant 1
    # (Parent Cell); any non-null worldspace → variant 0 (Grid coords).
    "wbNAVIParentDecider": {
        "field": "Parent World",
        "default_variant": 0,
        "map": {"null": 1},
    },
}

# Property index → name enums for wbObjectModPropertyToStr.
# Selected at decode time via FieldValue on "Form Type" (WEAP/ARMO/NPC_).
# Note: TES5Edit has a typo labelling ImpactDataSet as {50}; it is index 60.
_WEAPON_PROPERTIES: list[str] = [
    "Speed", "Reach", "MinRange", "MaxRange", "AttackDelaySec",
    "Unused 5", "OutOfRangeDamageMult", "SecondaryDamage", "CriticalChargeBonus",
    "HitBehaviour", "Rank", "Unknown 11", "AmmoCapacity", "Unknown 13", "Unknown 14",
    "Type", "IsPlayerOnly", "NPCsUseAmmo", "HasChargingReload", "IsMinorCrime",
    "IsFixedRange", "HasEffectOnDeath", "HasAlternateRumble", "IsNonHostile",
    "IgnoreResist", "IsAutomatic", "CantDrop", "IsNonPlayable", "AttackDamage",
    "Value", "Weight", "Keywords", "AimModel", "AimModelMinConeDegrees",
    "AimModelMaxConeDegrees", "AimModelConeIncreasePerShot", "AimModelConeDecreasePerSec",
    "AimModelConeDecreaseDelayMs", "AimModelConeSneakMultiplier",
    "AimModelRecoilDiminishSpringForce", "AimModelRecoilDiminishSightsMult",
    "AimModelRecoilMaxDegPerShot", "AimModelRecoilMinDegPerShot",
    "AimModelRecoilHipMult", "AimModelRecoilShotsForRunaway",
    "AimModelRecoilArcDeg", "AimModelRecoilArcRotateDeg",
    "AimModelConeIronSightsMultiplier", "HasScope", "ZoomDataFOVMult",
    "FireSeconds", "NumProjectiles", "AttackSound", "AttackSound2D", "AttackLoop",
    "AttackFailSound", "IdleSound", "EquipSound", "UnEquipSound", "SoundLevel",
    "ImpactDataSet",  # index 60 (TES5Edit typo labels this {50})
    "Ammo", "CritEffect", "BashImpactDataSet", "BlockMaterial", "Enchantments",
    "AimModelBaseStability", "ZoomData", "ZoomDataOverlay", "ZoomDataImageSpace",
    "ZoomDataCameraOffsetX", "ZoomDataCameraOffsetY", "ZoomDataCameraOffsetZ",
    "EquipSlot", "SoundLevelMult", "NPCAmmoList", "ReloadSpeed", "DamageTypeValues",
    "AccuracyBonus", "AttackActionPointCost", "OverrideProjectile", "HasBoltAction",
    "StaggerValue", "SightedTransitionSeconds", "FullPowerSeconds", "HoldInputToPower",
    "HasRepeatableSingleFire", "MinPowerPerShot", "ColorRemappingIndex", "MaterialSwaps",
    "CriticalDamageMult", "FastEquipSound", "DisableShells", "HasChargingAttack",
    "ActorValues", "ReachEngagementMult", "Health", "Durability", "NPCReloadDelay",
    "ZoomDataFOVMultB", "ZoomDataFOVMultC", "UnsightedTransitionSeconds",
    "MinWeaponDrawTime", "ModelSwap", "MinChargeTime", "PowerAffectsProjectileSpeed",
    "DamageBonusMult", "AimAssistModel", "WeightMult", "AmmoConsumption",
    "Overheating", "OverheatRateUp", "OverheatRateDown", "SoundTagSet",
    "SneakAttackMult",
]
_ARMOR_PROPERTIES: list[str] = [
    "Enchantments", "Bash Impact Data Set", "Block Material", "Keywords",
    "Weight", "Value", "Rating", "Addon Index", "Body Part", "Damage Type Value",
    "Actor Values", "Health", "Color Remapping Index", "Material Swaps",
    "Durability", "Biped World Model", "Model Swap", "Weight Mult", "Perk",
]
_ACTOR_PROPERTIES: list[str] = [
    "Keywords", "Forced Inventory", "XP Offset", "Enchantments",
    "Color Remapping Index", "Material Swaps",
]

# Vars that use runtime Pascal deciders we cannot model — emit raw fallback.
HARD_RAW_VARS = {
    "wbPerkEffect",
    "wbPERKData",
    # wbMagicEffectSounds is modeled via _inject_builtin_helpers below.
}

INT_MAP = {
    "itU8": ("u8", False),
    "itS8": ("s8", True),
    "itU16": ("u16", False),
    "itS16": ("s16", True),
    "itU32": ("u32", False),
    "itS32": ("s32", True),
    "itU64": ("u64", False),
    "itS64": ("s64", True),
}


class CoverageReport:
    """Accumulates extraction diagnostics for the fail-loud coverage report.

    Threaded through the ``Extractor`` instance as ``self.report``.  Non-zero
    ``defaulted_int_tokens`` is the loudest signal: it means an integer type
    token was not found in ``INT_MAP`` and silently defaulted to ``(u32,
    False)`` — a width-skew risk.
    """

    def __init__(self, strict: bool = False) -> None:
        self.strict = strict
        self.failed_records: int = 0
        self.missing_int_type: int = 0
        # Tokens not in INT_MAP → silently defaulted to (u32, False).
        # MUST be empty after a correct extraction; non-empty = width-skew risk.
        self.defaulted_int_tokens: dict[str, int] = {}
        # Pascal constructs that hit parse_member's terminal ``return None``
        # (silent member drop).  Populated for visibility; --strict does not
        # fail on these because some drops are expected (unmodelled helpers).
        self.unrecognized_constructs: dict[str, int] = {}
        # Per-record breakdown of unrecognized construct drops.
        # Keys are record sig strings; values are {construct_key: count} dicts.
        # Used by audit.py to emit per-record dropped findings with path context.
        self.unrecognized_by_record: dict[str, dict[str, int]] = {}


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def find_matching_paren(s: str, start: int) -> int:
    depth = 0
    i = start
    in_str = False
    quote = ""
    while i < len(s):
        c = s[i]
        if in_str:
            if c == quote:
                # Pascal uses '' to escape a single quote inside a string.
                if c == "'" and i + 1 < len(s) and s[i + 1] == "'":
                    i += 2  # skip the escape pair, stay in string
                    continue
                in_str = False
            i += 1
            continue
        # Skip // line comments — they can contain unbalanced parens/brackets.
        if c == "/" and i + 1 < len(s) and s[i + 1] == "/":
            while i < len(s) and s[i] != "\n":
                i += 1
            continue
        # Skip Pascal (* ... *) block comments (used in REGN to comment out
        # alternate struct variants). Pascal comments do NOT nest.
        if c == "(" and i + 1 < len(s) and s[i + 1] == "*":
            i += 2  # skip '(*'
            while i < len(s):
                if s[i] == "*" and i + 1 < len(s) and s[i + 1] == ")":
                    i += 2  # skip '*)'
                    break
                i += 1
            continue
        if c in ("'", '"'):
            in_str = True
            quote = c
            i += 1
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise ValueError(f"unmatched paren at {start}")


def find_matching_bracket(s: str, start: int) -> int:
    depth = 0
    i = start
    in_str = False
    quote = ""
    while i < len(s):
        c = s[i]
        if in_str:
            if c == quote:
                # Pascal uses '' to escape a single quote inside a string.
                if c == "'" and i + 1 < len(s) and s[i + 1] == "'":
                    i += 2  # skip the escape pair, stay in string
                    continue
                in_str = False
            i += 1
            continue
        # Skip // line comments — they can contain unbalanced parens/brackets.
        if c == "/" and i + 1 < len(s) and s[i + 1] == "/":
            while i < len(s) and s[i] != "\n":
                i += 1
            continue
        # Skip Pascal (* ... *) block comments (used in REGN to comment out
        # alternate struct variants). Pascal comments do NOT nest.
        if c == "(" and i + 1 < len(s) and s[i + 1] == "*":
            i += 2  # skip '(*'
            while i < len(s):
                if s[i] == "*" and i + 1 < len(s) and s[i + 1] == ")":
                    i += 2  # skip '*)'
                    break
                i += 1
            continue
        if c in ("'", '"'):
            in_str = True
            quote = c
            i += 1
            continue
        if c == "[":
            depth += 1
        elif c == "]":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise ValueError(f"unmatched bracket at {start}")


def split_top_level(text: str) -> list[str]:
    parts: list[str] = []
    cur: list[str] = []
    depth_paren = depth_bracket = 0
    in_str = False
    quote = ""
    i = 0
    while i < len(text):
        c = text[i]
        if in_str:
            cur.append(c)
            if c == quote:
                # Pascal uses '' to escape a single quote inside a string.
                if c == "'" and i + 1 < len(text) and text[i + 1] == "'":
                    cur.append(text[i + 1])
                    i += 2  # skip the escape pair, stay in string
                    continue
                in_str = False
            i += 1
            continue
        # Skip // line comments — they can contain unbalanced parens/brackets/commas.
        if c == "/" and i + 1 < len(text) and text[i + 1] == "/":
            while i < len(text) and text[i] != "\n":
                i += 1
            continue
        # Skip Pascal (* ... *) block comments. Pascal comments do NOT nest.
        if c == "(" and i + 1 < len(text) and text[i + 1] == "*":
            i += 2  # skip '(*'
            while i < len(text):
                if text[i] == "*" and i + 1 < len(text) and text[i + 1] == ")":
                    i += 2  # skip '*)'
                    break
                i += 1
            continue
        # Skip Pascal { } block comments (e.g. itS16{, wbOBTEAddonIndexToStr, ...}).
        # These are NOT nested and must not be treated as argument separators.
        if c == "{":
            i += 1
            while i < len(text) and text[i] != "}":
                i += 1
            if i < len(text):
                i += 1  # skip '}'
            continue
        if c in ("'", '"'):
            in_str = True
            quote = c
            cur.append(c)
            i += 1
            continue
        if c == "(":
            depth_paren += 1
        elif c == ")":
            depth_paren -= 1
        elif c == "[":
            depth_bracket += 1
        elif c == "]":
            depth_bracket -= 1
        elif c == "," and depth_paren == 0 and depth_bracket == 0:
            parts.append("".join(cur).strip())
            cur = []
            i += 1
            continue
        cur.append(c)
        i += 1
    tail = "".join(cur).strip()
    if tail:
        parts.append(tail)
    return parts


def unquote(s: str) -> str:
    s = s.strip()
    if (s.startswith("'") and s.endswith("'")) or (s.startswith('"') and s.endswith('"')):
        return s[1:-1].replace("''", "'")
    return s


def _pascal_str(m: re.Match) -> str:
    """Unescape a Pascal string match (replace '' → ')."""
    return m.group(1).replace("''", "'")


def parse_flags_list(text: str) -> list[str]:
    # Pascal strings use '' for a literal single-quote inside the string.
    names: list[str] = []
    for m in re.finditer(r"'((?:[^']|'')*)'", text):
        names.append(_pascal_str(m))
    return names


def parse_enum_dense(text: str) -> list[str]:
    names: list[str] = []
    for m in re.finditer(r"(?:\{\d+\}\s*)?'((?:[^']|'')*)'", text):
        names.append(_pascal_str(m))
    return names


def parse_enum_sparse(text: str) -> dict[str, str]:
    out: dict[str, str] = {}
    # Sig2Int(XXXX) — 4-char signature constant interpreted as little-endian u32
    for m in re.finditer(r"Sig2Int\('?([A-Z0-9_]{4})'?\)\s*,\s*'([^']*)'", text):
        key = str(int.from_bytes(m.group(1).encode("ascii"), "little"))
        out[key] = m.group(2)
    for m in re.finditer(r"(Int64\((\d+)\)|\$[0-9A-Fa-f]+|0x[0-9A-Fa-f]+|-?\d+)\s*,\s*'([^']*)'", text):
        key = m.group(1)
        if key.startswith("Int64("):
            key = m.group(2)
        elif key.startswith("$"):
            key = str(int(key[1:], 16))
        elif key.startswith("0x"):
            key = str(int(key, 16))
        out[key] = m.group(3)
    return out


def parse_format_arg(arg: str) -> dict | None:
    arg = arg.strip()
    if arg.startswith("wbFlags("):
        paren_pos = arg.index("(")
        close = find_matching_paren(arg, paren_pos)
        inner = arg[paren_pos + 1 : close]
        return {"flags": parse_flags_list(inner)}
    if arg.startswith("wbEnum("):
        paren_pos = arg.index("(")
        close = find_matching_paren(arg, paren_pos)
        inner = arg[paren_pos + 1 : close]
        parts = split_top_level(inner)
        if len(parts) >= 2 and parts[1].strip().startswith("["):
            sparse = parse_enum_sparse(parts[1])
            return {"enum": sparse}
        return {"enum": parse_enum_dense(inner)}
    return None


def sig_id(token: str) -> str | None:
    # Binary IAD-type sigs must be checked BEFORE strip() because some first
    # bytes are ASCII whitespace (e.g. \x09=tab, \x0a=newline, \x0d=CR) and
    # strip() would corrupt them.  These come from Pascal constants like
    # _09_IAD = #$09'IAD', injected into self.vars by _inject_builtin_helpers.
    if len(token) == 4 and token[1:] == "IAD":
        return token
    token = token.strip()
    if re.fullmatch(r"[A-Z0-9_]{4}", token):
        return token
    return None


# ---------------------------------------------------------------------------
# stop_before annotation pass
# ---------------------------------------------------------------------------

def _anchor_sig(member: dict | None) -> str | None:
    """Return the first 4-char subrecord signature found in a parsed member dict.

    Mirrors decode.rs `anchor_sig`: recurses into rstruct members and rarray
    elements to find the leading sig.  Used to set `stop_before` boundaries on
    Conditions-style rarrays so they don't greedily consume CTDAs that belong
    to following entries.
    """
    if member is None:
        return None
    sig = member.get("sig")
    if sig and re.fullmatch(r"[A-Z0-9_]{4}", sig):
        return sig
    kind = member.get("kind")
    if kind == "rstruct":
        for m in member.get("members", []):
            s = _anchor_sig(m)
            if s:
                return s
    elif kind == "rarray":
        elem = member.get("element")
        if elem:
            return _anchor_sig(elem)
    return None


def _annotate_stop_before(members: list[dict], outer_stops: list[str]) -> None:
    """Walk a member list and set ``stop_before`` on rarrays that need boundaries.

    Two cases receive a boundary:

    1. **Conditions rarrays** (element anchor ``CTDA``) — the original case.
       Without a boundary hint the decoder consumes CTDAs greedily from the
       shared ``by_sig`` map, so per-entry conditions are stolen by the
       record-level Conditions slot.

    2. **Any rarray nested inside a repeating record group** (``outer_stops``
       non-empty).  When an rarray sits inside an rstruct element of an outer
       rarray, ``outer_stops`` carries the element anchor of that outer rarray
       (and its siblings), making it non-empty.  Without a boundary the nested
       array over-consumes across entry boundaries — e.g. all ``QSRD``
       "Rewarded Items" in a GMRW record are pulled into the first reward entry
       rather than being split per-reward by the ``ITME`` end-marker.

    ``stop_before`` is set to the union of:
      • anchor sigs of all sibling members that follow this rarray, and
      • ``outer_stops`` — boundaries propagated from enclosing rarrays.

    The function recurses into rstruct members, rarray elements, and union
    variants so every nesting depth is covered.
    """
    for i, member in enumerate(members):
        # Collect anchor sigs of all siblings that follow position i.
        sibling_stops: list[str] = []
        for j in range(i + 1, len(members)):
            s = _anchor_sig(members[j])
            if s and s not in sibling_stops:
                sibling_stops.append(s)

        kind = member.get("kind")

        if kind == "rarray":
            elem = member.get("element")
            elem_anchor = _anchor_sig(elem) if elem else None

            if elem_anchor == "CTDA":
                # Conditions rarray: add stop_before so consumption halts at
                # the next structural boundary.  Use sibling_stops + outer_stops
                # so the boundary propagates from enclosing levels.
                stops: list[str] = sibling_stops[:]
                for s in outer_stops:
                    if s not in stops:
                        stops.append(s)
                if stops:
                    member["stop_before"] = stops
                    # Also annotate the immediately-preceding CITC count
                    # integer with the same boundary list so it defers when
                    # the conditions appear out-of-position (e.g. FO76
                    # NPC_ camp-pet tail CITC).
                    for prev in reversed(members[:i]):
                        if prev.get("kind") == "integer" and prev.get("sig") == "CITC":
                            prev["stop_before"] = stops
                            break
            elif outer_stops and sibling_stops and elem and elem.get("kind") == "struct":
                # Non-CTDA rarray nested inside a repeating record group, where
                # the element is an atomic single-subrecord struct (kind="struct").
                #
                # For struct elements the anchor sig IS the element itself: if the
                # anchor is absent there is genuinely nothing to consume, so
                # stop_before_check's "anchor absent → halt" is correct.
                #
                # For rstruct elements the anchor is only the FIRST member — the
                # anchor can be absent while later members (e.g. CS2D without CS2K
                # in NPC_ Actor Sounds, or OBTS without OBTF in Object Templates)
                # are still present.  Adding stop_before there causes an immediate
                # false halt and orphans those subrecords.  So rstruct-element
                # rarrays are excluded from this branch.
                #
                # Use ONLY sibling_stops (not outer_stops) as the boundary: outer_stops
                # propagates record-level sigs (e.g. CNAM, FULL) that appear before
                # the array elements in document order and would cause false halts.
                stops = sibling_stops[:]
                member["stop_before"] = stops

            # Recurse into the element.  From inside the element the enclosing
            # rarray's element anchor is itself a repeat boundary (e.g. LVLO
            # marks the start of each new leveled-list entry).
            if elem:
                inner_outer: list[str] = []
                if elem_anchor:
                    inner_outer.append(elem_anchor)
                for s in sibling_stops:
                    if s not in inner_outer:
                        inner_outer.append(s)
                for s in outer_stops:
                    if s not in inner_outer:
                        inner_outer.append(s)
                _annotate_stop_before_member(elem, inner_outer)

        elif kind == "rstruct":
            inner_outer = sibling_stops[:]
            for s in outer_stops:
                if s not in inner_outer:
                    inner_outer.append(s)
            _annotate_stop_before(member.get("members", []), inner_outer)

        elif kind == "union":
            inner_outer = sibling_stops[:]
            for s in outer_stops:
                if s not in inner_outer:
                    inner_outer.append(s)
            for variant in member.get("variants", []):
                _annotate_stop_before_member(variant, inner_outer)


def _annotate_stop_before_member(member: dict | None, outer_stops: list[str]) -> None:
    """Recurse into a single member for stop_before annotation."""
    if member is None:
        return
    kind = member.get("kind")
    if kind == "rstruct":
        _annotate_stop_before(member.get("members", []), outer_stops)
    elif kind == "rarray":
        elem = member.get("element")
        elem_anchor = _anchor_sig(elem) if elem else None
        if elem_anchor == "CTDA":
            stops = list(outer_stops)
            if stops:
                member["stop_before"] = stops
        if elem:
            inner_outer: list[str] = []
            if elem_anchor:
                inner_outer.append(elem_anchor)
            for s in outer_stops:
                if s not in inner_outer:
                    inner_outer.append(s)
            _annotate_stop_before_member(elem, inner_outer)
    elif kind == "union":
        for variant in member.get("variants", []):
            _annotate_stop_before_member(variant, outer_stops)


def _patch_qust_location_fill_type(rec: dict, extractor: "Extractor") -> None:
    """Live Location aliases sometimes use ALFA+ALRT fill (Reference-style).

    The Pascal ALLS union only documents ALFA+KNAM; append the Reference Alias
    'Location Alias Reference' fill variant so those records decode.
    """
    aliases = None
    for member in rec.get("members", []):
        if member.get("kind") == "rarray" and member.get("name") == "Aliases":
            aliases = member.get("element")
            break
    if not aliases or aliases.get("kind") != "union":
        return

    ref_alrt_variant: dict | None = None
    loc_fill: dict | None = None
    for variant in aliases.get("variants", []):
        vname = variant.get("name", "")
        for member in variant.get("members", []):
            if member.get("kind") != "union" or member.get("name") != "Fill Type":
                continue
            if "Reference Alias" in vname:
                for fv in member.get("variants", []):
                    if any(c.get("sig") == "ALRT" for c in fv.get("members", [])):
                        ref_alrt_variant = fv
            elif "Location Alias" in vname:
                loc_fill = member

    if not ref_alrt_variant or not loc_fill:
        return
    if any(
        any(c.get("sig") == "ALRT" for c in fv.get("members", []))
        for fv in loc_fill.get("variants", [])
    ):
        return

    loc_fill["variants"].append(copy.deepcopy(ref_alrt_variant))
    anchors = [extractor._extract_anchor_sigs(v) for v in loc_fill["variants"]]
    for i, a in enumerate(anchors):
        if not a:
            first = extractor._extract_first_anchor_sig(loc_fill["variants"][i])
            anchors[i] = [first] if first else []
    loc_fill["decider"]["present_signature"] = anchors


class Extractor:
    def __init__(self, fo76: str, common: str):
        self.fo76 = fo76
        self.common = common
        self.vars: dict[str, str] = {}
        self.sig_lists: dict[str, list[str]] = {}
        self._collect_sig_lists(fo76)
        self._collect_vars(fo76)
        # Skip common.pas — helpers are injected below.
        self._inject_builtin_helpers()
        # Case-insensitive lookup map: lowercase(var_name) → canonical var_name.
        # Pascal identifiers are case-insensitive; some usage sites (e.g. wbDesc vs
        # wbDESC) use a different case than the `:=` assignment that _collect_vars
        # recorded.  Rebuilt lazily via _vars_lower_map property.
        self._vars_lower: dict[str, str] | None = None
        # Coverage / fail-loud diagnostics.  Set report.strict = True before
        # calling run() to enable strict mode (also via EXTRACT_STRICT=1 env).
        self.report: CoverageReport = CoverageReport()
        # Tracks the record sig currently being extracted for error attribution.
        self._current_record: str = ""
        # Inline member cache: pre-built schema dicts returned via __inline__:KEY
        # sentinel from expand_call → parse_member.  Used for members whose sigs
        # contain bytes that split_top_level.strip() would destroy (e.g. \x09=tab,
        # \x0a=newline, \x0d=CR as the first byte of a binary IAD sig).
        self._inline_members: dict[str, dict] = {}
        self._inline_counter: int = 0

    @property
    def _vars_lower_map(self) -> dict[str, str]:
        """Lazily built lowercase → canonical-key map for case-insensitive var lookup."""
        if self._vars_lower is None:
            self._vars_lower = {k.lower(): k for k in self.vars}
        return self._vars_lower

    def _collect_sig_lists(self, text: str) -> None:
        """Parse named TwbSignatures array constants from the Pascal global scope.

        These are declared as ``sigXxx : TwbSignatures = ['A', 'B', ...]`` and
        used in ``wbFormIDCk`` calls instead of inline bracket lists.  The most
        important one is ``sigBaseObjects``.
        """
        for m in re.finditer(
            r"\b(sig[A-Za-z0-9_]+)\s*:\s*TwbSignatures\s*=\s*\[([^\]]+)\]",
            text,
        ):
            sigs = re.findall(r"'([A-Z0-9_]{4})'", m.group(2))
            if sigs:
                self.sig_lists[m.group(1)] = sigs

    def _collect_vars(self, text: str) -> None:
        start = text.find("procedure DefineFO76")
        if start < 0:
            start = 0
        end = text.find("\nend.", start)
        if end < 0:
            end = len(text)
        chunk = text[start:end]
        for m in re.finditer(r"\b(wb[A-Za-z0-9_]+)\s*:=\s*", chunk):
            name = m.group(1)
            i = m.end()
            if i >= len(chunk):
                continue
            if chunk[i] == "(":
                r = find_matching_paren(chunk, i)
                self.vars[name] = chunk[i : r + 1]
            else:
                semi = chunk.find(";", i)
                if semi < 0:
                    continue
                self.vars[name] = chunk[i:semi].strip()

    def _inject_builtin_helpers(self) -> None:
        self.vars.setdefault(
            "wbEDID",
            "wbStringKC(EDID, 'Editor ID', 0, cpOverride)",
        )
        self.vars.setdefault(
            "wbFULL",
            "wbLStringKC(FULL, 'Name', 0, cpTranslate)",
        )
        self.vars.setdefault(
            "wbDESC",
            "wbLStringKC(DESC, 'Description', 0, cpTranslate)",
        )
        self.vars.setdefault(
            "wbDESCReq",
            "wbLStringKC(DESC, 'Description', 0, cpTranslate, True)",
        )
        self.vars["wbOBND"] = (
            "wbStruct(OBND, 'Object Bounds', ["
            "wbInteger('X1', itS16), wbInteger('Y1', itS16), wbInteger('Z1', itS16),"
            "wbInteger('X2', itS16), wbInteger('Y2', itS16), wbInteger('Z2', itS16)])"
        )
        self.vars["wbKeywords"] = (
            "wbRStruct('Keywords', ["
            "wbInteger(KSIZ, 'Keyword Count', itU32),"
            "wbArrayS(KWDA, 'Keywords', wbFormIDCk('Keyword', [KYWD,NULL]))])"
        )
        self.vars["wbGenericModel"] = (
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
        )
        # wbDEST: full rstruct matching wbDefinitionsFO76.pas:6905-6991.
        # Includes the Stages rarray with DSTD (Destruction Stage Data).
        # wbConditions is resolved below via self.vars lookup.
        self.vars["wbDEST"] = (
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
            "wbFloat('DPS Limit')"
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
        )
        self.vars["wbEnchantment"] = "wbFormIDCk(EITM, 'Enchantment', [ENCH,NULL])"
        self.vars["wbModelInfo"] = "wbByteArray(MODT, 'Model Information', 0)"
        # wbDamageTypeArray — includes the form-version 152 Curve Table field.
        # wbDefinitionsCommon.pas:8014-8024.
        self.vars["wbDamageTypeArray"] = (
            "wbArrayS(DAMA, 'Resistances', wbStructSK([0], 'Resistance', ["
            "wbFormIDCk('Type', [DMGT]),"
            "wbInteger('Amount', itU32),"
            "wbFromVersion(152, wbFormIDCk('Curve Table', [CURV, NULL]))]))"
        )
        self.vars["wbPTRN"] = "wbFormIDCk(PTRN, 'Preview Transform', [TRNS,NULL])"
        self.vars["wbPHST"] = "wbByteArray(PHST, 'Photo Studio', 0)"
        self.vars["wbSNTP"] = "wbFormIDCk(SNTP, 'Snap Template', [STMP])"
        # wbXALGFlags / wbXALG — wbDefinitionsFO76.pas:4815-4848 / :4931.
        self.vars["wbXALGFlags"] = (
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
        )
        self.vars["wbXALG"] = "wbInteger(XALG, 'Flags', itU64, wbXALGFlags)"
        self.vars["wbFTAGs"] = "wbRArray('Form Tags', wbString(FTAG, 'Form Tag'))"
        # wbBipedObjectFlags — wbDefinitionsFO76.pas:4312-4345.
        # FO76's BOD2 is a single u32 of biped-object flags (no Armor Type byte).
        self.vars["wbBipedObjectFlags"] = (
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
        )
        # wbBOD2 — wbDefinitionsFO76.pas:4347-4351.
        # Single u32 of biped flags; wbBipedObjectFlags var is resolved by _parse_integer.
        self.vars["wbBOD2"] = (
            "wbStruct(BOD2, 'Biped Body Template', ["
            "wbInteger('First Person Flags', itU32, wbBipedObjectFlags)])"
        )
        self.vars["wbETYP"] = "wbFormIDCk(ETYP, 'Equipment Type', [EQUP,NULL])"
        self.vars["wbYNAM"] = "wbFormIDCk(YNAM, 'Sound - Pickup', [SNDR,NULL])"
        self.vars["wbZNAM"] = "wbFormIDCk(ZNAM, 'Sound - Putdown', [SNDR,NULL])"
        self.vars["wbVCRY"] = "wbFormIDCk(VCRY, 'Value Currency', [NULL,CNCY])"
        self.vars["wbICON"] = "wbString(ICON, 'Icon Image')"
        self.vars["wbMICO"] = "wbString(MICO, 'Message Icon')"
        self.vars["wbINRD"] = "wbFormIDCk(INRD, 'Instance Naming', [INNR])"
        self.vars["wbEILV"] = "wbByteArray(EILV, 'EILV', 0)"
        self.vars["wbIBSD"] = "wbByteArray(IBSD, 'IBSD', 0)"
        # wbAPPR — wbDefinitionsFO76.pas:7136.
        # Sorted array of KYWD FormIDs (attach-parent slots).  Decoded as a packed
        # array of 4-byte FormIDs via the record-context Array path.
        self.vars["wbAPPR"] = "wbArrayS(APPR, 'Attach Parent Slots', wbFormIDCk('Keyword', [KYWD]))"
        self.vars["wbMDOB"] = "wbByteArray(MDOB, 'Menu Display Object', 0)"
        self.vars["wbMIID"] = "wbInteger(MIID, 'Max Item ID', itU32)"
        self.vars["wbDEFL"] = "wbFormIDCk(DEFL, 'Default Layer', [LAYR])"
        self.vars["wbOPDSs"] = (
            "wbRArray('Object Placement Defaults', wbStruct(OPDS, 'Object Placement Default', ["
            "wbByteArray('Flags', 4), wbFloat('Sink'), wbFloat('Sink Var'), wbFloat('Scale'),"
            "wbFloat('Scale Var'), wbFloat('Angle X'), wbFloat('Angle X Var'),"
            "wbFloat('Angle Y'), wbFloat('Angle Y Var'), wbFloat('Angle Z'), wbFloat('Angle Z Var')]))"
        )
        self.vars["wbHitBehaviourEnum"] = "wbEnum(['Normal formula behaviour','Dismember only','Explode only','No dismember or explode'])"
        self.vars["wbSoundLevelEnum"] = "wbEnum(['Loud','Normal','Silent','Very Loud','Quiet'])"
        self.vars["wbStaggerEnum"] = "wbEnum(['None','Small','Medium','Large','Extra Large'])"
        self.vars["wbBoolEnum"] = "wbEnum(['False','True'])"
        # Pascal functions (not variables) — inject as builtin helpers so they
        # appear in self.vars and are expanded by _parse_member_list.
        self.vars.setdefault(
            "wbMagicEffectSounds",
            (
                "wbArrayS(SNDD, 'Sounds', wbStruct('Sound', ["
                "wbInteger('Type', itU32, wbEnum(["
                "'Sheathe/Draw', 'Charge', 'Ready', 'Release',"
                "'Concentration Cast Loop', 'On Hit'])),"
                "wbFormIDCk('Sound', [SNDR])]))"
            ),
        )
        self.vars.setdefault(
            "wbWeatherSounds",
            (
                "wbRArray('Sounds', wbStruct(SNAM, 'Sound', ["
                "wbFormIDCk('Sound', [SNDR]),"
                "wbInteger('Type', itU32, wbEnum(["
                "'Default', 'Precipitation', 'Wind', 'Thunder']))]))"
            ),
        )

        # wbSoundDescriptorSounds — wbDefinitionsCommon.pas:8761-8764.
        # SNDR uses an RArray of ANAM strings (sound file paths), not a FormID.
        self.vars.setdefault(
            "wbSoundDescriptorSounds",
            "wbRArray('Sounds', wbString(ANAM, 'Sound'))",
        )

        # ----------------------------------------------------------------
        # RACE character-creation builders — Pascal functions (not variables)
        # at wbDefinitionsFO76.pas:3940/4007/4028; expanded via expand_call.
        # ----------------------------------------------------------------
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
        self.vars["wbFaceMorphElement"] = (
            "wbRStruct('Face Morph',["
            "wbInteger(FMRI,'Index',itU32),"
            "wbLString(FMRN,'Name')])"
        )
        self.vars["wbMorphPreset"] = (
            "wbRStruct('Morph Preset',["
            "wbInteger(MPPI,'Index',itU32),"
            "wbLString(MPPN,'Name'),"
            "wbString(MPPM,'Morph Type'),"
            "wbFormIDCk(MPPT,'Texture',[TXST]),"
            "wbInteger(MPPF,'Playable',itU8,wbBoolEnum)])"
        )
        self.vars["wbMorphGroupElement"] = (
            "wbRStruct('Morph Group',["
            "wbString(MPGN,'Name'),"
            "wbInteger(MPPC,'Count',itU32),"
            "wbRArray('Morph Presets',wbMorphPreset).SetCountPath('Count'),"
            "wbInteger(MPPK,'Tint Layer Face Region Index',itU16),"
            "wbArray(MPGS,'Morph Value Indexs',wbInteger('Index',itU32))])"
        )
        self.vars["wbTintTemplateOption"] = (
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
        )
        self.vars["wbTintTemplateGroupElement"] = (
            "wbRStruct('Group',["
            "wbLString(TTGP,'Group Name'),"
            "wbRArray('Options',wbTintTemplateOption),"
            "wbInteger(TTGE,'Category Index',itU32)])"
        )

        # ----------------------------------------------------------------
        # wbDefinitionsCommon.pas functions — not collected by _collect_vars
        # because that method only scans DefineFO76; inject them here.
        # ----------------------------------------------------------------
        # wbFaction: FO76 = FO4+, so IsFO4Plus(nil, wbUnused(3)) → wbUnused(3).
        self.vars["wbFaction"] = (
            "wbStructSK(SNAM, [0], 'Faction', ["
            "wbFormIDCk('Faction', [FACT]),"
            "wbInteger('Rank', itS8),"
            "wbUnused(3)])"
        )
        # wbHeadPart: FO76 is not Oblivion/FO3, so uses HEAD formid variant.
        self.vars["wbHeadPart"] = (
            "wbRStructSK([0], 'Head Part', ["
            "wbInteger(INDX, 'Head Part Number', itU32),"
            "wbFormIDCk(HEAD, 'Head', [HDPT, NULL])])"
        )

        # ----------------------------------------------------------------
        # IMAD color interpolator — used in wbArray(TNAM/NAM3, ...) calls.
        # ----------------------------------------------------------------
        self.vars["wbFloatRGBA"] = (
            "wbStruct('Color', ["
            "wbFloat('Red'), wbFloat('Green'), wbFloat('Blue'), wbFloat('Alpha')])"
        )
        self.vars["wbColorInterpolator"] = (
            "wbStructSK([0], 'Data', [wbFloat('Time'), wbStruct('Value', ["
            "wbFloat('Red'), wbFloat('Green'), wbFloat('Blue'), wbFloat('Alpha')])])"
        )

        # ----------------------------------------------------------------
        # VMAD: emit the existing vmad decoder kind.  All wbVMAD* variants
        # resolve to the __vmad__ sentinel which parse_member intercepts.
        # ----------------------------------------------------------------
        for vmad_var in (
            "wbVMAD",
            "wbVMADFragmentedPERK",
            "wbVMADFragmentedPACK",
            "wbVMADFragmentedQUST",
            "wbVMADFragmentedSCEN",
            "wbVMADFragmentedINFO",
        ):
            self.vars[vmad_var] = "__vmad__"

        # ----------------------------------------------------------------
        # CTDA / Conditions — modeled as a structural (no-Rust) helper.
        # Wrapped in an RStruct so the preceding CITC count subrecord
        # (wbDefinitionsFO76.pas:6888 SetCountPath(CITC)) is consumed and
        # not left in _unmapped.  CITC is optional — records without it
        # (e.g. direct CTDA blocks) simply skip the missing integer.
        # ----------------------------------------------------------------
        self.vars["wbConditions"] = (
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
        )

        # ----------------------------------------------------------------
        # Effects — rstruct wrapping EFID + EFIT (bytes) + optional fields.
        # wbConditions is already modeled above.
        # ----------------------------------------------------------------
        self.vars.setdefault("wbEFID", "wbFormIDCk(EFID,'Base Effect',[MGEF])")
        # EFIT layout is form-version-conditional. The bands come from the comment at
        # wbDefinitionsFO76.pas:6727 (the Pascal itself uses a bare wbUnknown): Effect ID
        # only at fv>=166; trailing unknown 12 bytes for fv 154-165, 8 bytes for 166-182.
        self.vars["wbEFIT"] = (
            "wbStruct(EFIT,'Effect Item Data',["
            "wbFromVersion(166, wbInteger('Effect ID',itU32)),"
            "wbFloat('Magnitude'),"
            "wbInteger('Area',itU32),"
            "wbInteger('Duration',itU32),"
            "wbFromVersion(154, wbBelowVersion(166, wbByteArray('_unknown',12))),"
            "wbFromVersion(166, wbBelowVersion(183, wbByteArray('_unknown',8)))"
            "])"
        )
        self.vars["wbEffect"] = (
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
        )
        self.vars["wbEffectsReq"] = "wbRArray('Effects',wbEffect)"

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
        self.vars["wbWeatherMagic"] = (
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
        )

        # wbRagdoll — wbDefinitionsCommon.pas:8694-8710
        # Ragdoll bone data (XRGD) + biped rotation (XRGB, non-TES4 only).
        self.vars["wbRagdoll"] = (
            "wbRStruct('Ragdoll Data',["
            "wbArray(XRGD,'Bones',wbStruct('Bone',["
            "wbInteger('Bone Id',itU8),"
            "wbUnused(3),"
            "wbByteArray('Position/Rotation',24)"
            "])),"
            "wbVec3(XRGB,'Biped Rotation')"
            "])"
        )

        # wbKWDAs — used in REFR/ACHR to add keywords.
        # Minimal: array of keyword formids with KWDA sig.
        self.vars["wbKWDAs"] = (
            "wbRStruct('Keywords',["
            "wbInteger(KSIZ,'Keyword Count',itU32),"
            "wbArrayS(KWDA,'Keywords',wbFormIDCk('Keyword',[KYWD,NULL]))"
            "])"
        )

        # wbOwnership — ownership data (owner ref + rank).
        # wbDefinitionsCommon.pas:8655 (simplified: XOWN + XRNK).
        self.vars["wbOwnership"] = (
            "wbRStruct('Ownership',["
            "wbFormIDCk(XOWN,'Owner',[FACT,NPC_,NULL]),"
            "wbInteger(XRNK,'Faction Rank',itS32)"
            "])"
        )

        # wbActionFlag — wbDefinitionsCommon.pas (single flag byte, XACT).
        self.vars["wbActionFlag"] = "wbInteger(XACT,'Action Flag',itU32)"

        # wbWaterData — wbDefinitionsFO76.pas:4973-4985. FO76 uses XWCN (count u32) + XWCU (velocity array).
        # Not XWAT (old FO4 sig that no longer appears in FO76).
        self.vars["wbWaterData"] = (
            "wbRStruct('Water Current Velocities',"
            "[wbInteger(XWCN,'Velocity Count',itU32),"
            "wbArray(XWCU,'Velocities',"
            "wbStruct('Current',[wbVec3('Velocity'),wbFloat('Unknown')]))])"
        )

        # wbAmbientColors — ambient lighting colors (no-sig struct form; sig form handled in expand_call).
        # FO76 branch: Directional (6×4-byte color entries) + wbUnused(4) + wbUnused(4).
        self.vars["wbAmbientColors"] = (
            "wbStruct('Directional Ambient Lighting Colors',"
            "[wbStruct('Directional',"
            "[wbByteColors('X+'),wbByteColors('X-'),wbByteColors('Y+'),"
            "wbByteColors('Y-'),wbByteColors('Z+'),wbByteColors('Z-')]),"
            "wbUnused(4),wbUnused(4)])"
        )

        # wbByteColors — byte-precision color (no-sig struct form; sig form handled in expand_call).
        # 4 bytes: Red u8, Green u8, Blue u8, Unused u8.
        self.vars["wbByteColors"] = (
            "wbStruct('Color',"
            "[wbInteger('Red',itU8),wbInteger('Green',itU8),wbInteger('Blue',itU8),wbUnused(1)])"
        )

        # wbSizePosRot — bounds size + position/rotation; handled in expand_call with the sig arg.

        # ----------------------------------------------------------------
        # WTHR (Weather) helper functions from wbDefinitionsCommon.pas.
        # These are Common.pas functions missed by _collect_vars.
        # ----------------------------------------------------------------

        # wbWeatherCloudTextures — wbDefinitionsCommon.pas:8939-8991.
        # FO76 uses a 32-layer cloud texture system with special 4-byte sigs.
        # Layers 0-9: "00TX"-"90TX" (alphanumeric, injectable directly).
        # Layers 17-31: "A0TX"-"O0TX" (alphanumeric, injectable directly).
        # Layers 10-16: ":0TX"-"@0TX" (non-alphanumeric, added via record_additions).
        _cloud_tex_parts = []
        for _i in range(10):  # layers 0-9: sig = chr(0x30+i)+"0TX" = "00TX"-"90TX"
            _sig = f"{_i}0TX"
            _cloud_tex_parts.append(f"wbString({_sig},'Layer #{_i}')")
        for _i in range(17, 32):  # layers 17-31: "A0TX"-"O0TX"
            _sig = chr(ord("A") + _i - 17) + "0TX"
            _cloud_tex_parts.append(f"wbString({_sig},'Layer #{_i}')")
        self.vars["wbWeatherCloudTextures"] = (
            "wbRStruct('Cloud Textures',[" + ",".join(_cloud_tex_parts) + "])"
        )

        # wbWeatherCloudSpeed — wbDefinitionsCommon.pas:8918-8937.
        # RStruct with RNAM (Y Speeds) and QNAM (X Speeds), each a 32-element byte array.
        self.vars["wbWeatherCloudSpeed"] = (
            "wbRStruct('Cloud Speeds',["
            "wbArray(RNAM,'Y Speeds',wbInteger('Layer',itU8),32),"
            "wbArray(QNAM,'X Speeds',wbInteger('Layer',itU8),32)"
            "])"
        )

        # wbWeatherCloudColors — wbDefinitionsCommon.pas:8906-8916.
        # PNAM: array of cloud layer colors (wbWeatherTimeOfDay structs — complex union,
        # use bytearray to consume the subrecord without version-conditional parsing).
        self.vars["wbWeatherCloudColors"] = "wbByteArray(PNAM,'Cloud Colors',0)"

        # wbWeatherCloudAlphas — wbDefinitionsCommon.pas:8863-8904.
        # JNAM: array of 32 layers, each with 8 floats (time-of-day alpha values).
        self.vars["wbWeatherCloudAlphas"] = (
            "wbArray(JNAM,'Cloud Alphas',wbStruct('Layer',["
            "wbFloat('Sunrise'),wbFloat('Day'),wbFloat('Sunset'),wbFloat('Night'),"
            "wbFloat('Early Sunrise'),wbFloat('Late Sunrise'),"
            "wbFloat('Early Sunset'),wbFloat('Late Sunset')"
            "]),32)"
        )

        # wbWeatherColors — wbDefinitionsCommon.pas:8993-9043.
        # NAM0: large struct of wbWeatherTimeOfDay entries — use bytearray.
        self.vars["wbWeatherColors"] = "wbByteArray(NAM0,'Weather Colors',0)"

        # wbWeatherFogDistance — wbDefinitionsCommon.pas:9081-9132.
        # FNAM: fog near/far distances + powers + heights — use bytearray.
        self.vars["wbWeatherFogDistance"] = "wbByteArray(FNAM,'Fog Distance',0)"

        # wbWeatherDisabledLayers — wbDefinitionsCommon.pas:9068-9079.
        # NAM1: 32-bit flags, one bit per cloud layer.
        self.vars["wbWeatherDisabledLayers"] = (
            "wbInteger(NAM1,'Disabled Cloud Layers',itU32)"
        )

        # wbWeatherImageSpaces — wbDefinitionsCommon.pas:9149-9170.
        # IMSP: struct of 8 IMGS formids (Sunrise/Day/Sunset/Night + Early/Late variants).
        self.vars["wbWeatherImageSpaces"] = (
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
        )

        # wbWeatherGodRays — wbDefinitionsCommon.pas:9134-9147.
        # WGDR: struct of 8 GDRY formids.
        self.vars["wbWeatherGodRays"] = (
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
        )

        # wbWeatherVolumetricLighting — wbDefinitionsCommon.pas:9219-9240.
        # HNAM: struct of 8 VOLI formids.
        self.vars["wbWeatherVolumetricLighting"] = (
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
        )

        # wbWeatherDirectionalLighting — wbDefinitionsCommon.pas:9045-9066.
        # Multiple DALC subrecords (one per time-of-day) in a wrapping RStruct.
        # Each DALC is 28 bytes (6 byteColors directional + 4 unused + 4 unused in FO76).
        self.vars["wbWeatherDirectionalLighting"] = (
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
        )

        # ----------------------------------------------------------------
        # wbXLOD — wbDefinitionsCommon.pas:9624-9629.
        # XLOD subrecord: fixed array of 3 floats (Distant LOD data).
        # ----------------------------------------------------------------
        self.vars.setdefault("wbXLOD", "wbArray(XLOD,'Distant LOD Data',wbFloat('Unknown'),3)")

        # ----------------------------------------------------------------
        # wbWeatherLightningColor — wbDefinitionsCommon.pas:9172-9179.
        # Sigless struct field: Red/Green/Blue u8.  Used bare (no parens)
        # inside a wbArray element struct in WTHR.
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbWeatherLightningColor",
            "wbStruct('Lightning Color',["
            "wbInteger('Red',itU8),"
            "wbInteger('Green',itU8),"
            "wbInteger('Blue',itU8)"
            "])"
        )

        # ----------------------------------------------------------------
        # wbVec3PosRot — bare (sigless) usage inside struct member lists.
        # The (SIG) form is handled in expand_call; the bare var reference
        # (e.g. wbVec3PosRot inside wbStruct XTEL) needs the var map.
        # wbDefinitionsCommon.pas:8715-8720 — 24-byte position+rotation block.
        # ----------------------------------------------------------------
        self.vars.setdefault("wbVec3PosRot", "wbByteArray('Position/Rotation', 24)")

        # ----------------------------------------------------------------
        # wbINOA / wbINOM — editor-only INFO-order arrays (DIAL record).
        # wbDefinitionsCommon.pas:8164-8182.  These are flagged dfDontSave
        # and should not appear in binary ESM data, but we model them anyway.
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbINOA",
            "wbArray(INOA,'INFO Order (All previous modules)',wbFormIDCk('INFO',[INFO]))"
        )
        self.vars.setdefault(
            "wbINOM",
            "wbArray(INOM,'INFO Order (Masters only)',wbFormIDCk('INFO',[INFO]))"
        )

        # ----------------------------------------------------------------
        # wbFactionRelations — wbDefinitionsCommon.pas:8100-8117.
        # RArrayS of XNAM structs: faction/race formid + s32 modifier + enum.
        # IsTES4(nil, ...) → FO76 includes the Group Combat Reaction field.
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbFactionRelations",
            (
                "wbRArrayS('Relations',"
                "wbStructSK(XNAM,[0],'Relation',["
                "wbFormIDCk('Faction',[FACT,RACE]),"
                "wbInteger('Modifier',itS32),"
                "wbInteger('Group Combat Reaction',itU32,wbEnum(["
                "'Neutral','Enemy','Ally','Friend'"
                "]))]))"
            ),
        )

        # ----------------------------------------------------------------
        # wbActorSounds — wbDefinitionsCommon.pas:7959-7975.
        # RArrayS of (CS2K keyword + CS2D sound) pairs, count from CS2H.
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbActorSounds",
            (
                "wbRArrayS('Sounds',"
                "wbRStructSK([0],'Sound',["
                "wbFormIDCk(CS2K,'Keyword',[KYWD]),"
                "wbFormIDCk(CS2D,'Sound',[SNDR])]))"
            ),
        )

        # ----------------------------------------------------------------
        # wbIdleAnimation — wbDefinitionsCommon.pas:8186-8223.
        # FO76 branch: IDLF flags (u8) + IDLC animation count (u8) +
        # IDLT timer float + IDLA animations array + IDLB unknown.
        # IsFO3(a, b) → b for FO76; IsSF1(a, b) → b for FO76.
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbIdleAnimation",
            (
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
        )

        # ----------------------------------------------------------------
        # Binary IAD sig constants for IMAD record.
        # Pascal: _00_IAD : TwbSignature = #$00'IAD', …, _54_IAD : TwbSignature = #$54'IAD'.
        # wbDefinitionsSignatures.pas:1808-1866.
        # Stored as 4-char Python strings; sig_id() accepts them via the IAD rule.
        # ----------------------------------------------------------------
        for _iad_i in range(0x15):   # 0x00 .. 0x14 (Mult)
            self.vars.setdefault(f"_{_iad_i:02X}_IAD", chr(_iad_i) + "IAD")
        for _iad_i in range(0x40, 0x55):  # 0x40 .. 0x54 (Add)
            self.vars.setdefault(f"_{_iad_i:02X}_IAD", chr(_iad_i) + "IAD")

        # ----------------------------------------------------------------
        # wbRegionAreas — wbDefinitionsCommon.pas:8712-8728.
        # FO76 branch includes ANAM (unknown extra bytes).
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbRegionAreas",
            (
                "wbRArray('Region Areas',"
                "wbRStruct('Region Area',["
                "wbInteger(RPLI,'Edge Fall-off',itU32),"
                "wbArray(RPLD,'Points',wbStruct('Point',[wbFloat('X'),wbFloat('Y')])),"
                "wbByteArray(ANAM,'Unknown',0)"
                "]))"
            ),
        )

        # ----------------------------------------------------------------
        # wbRegionSounds — wbDefinitionsCommon.pas:8729-8766.
        # FO76 branch: RDSA sig, wbFloat('Chance') (not wbScaledInt4).
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbRegionSounds",
            (
                "wbArrayS(RDSA,'Sounds',"
                "wbStructSK([0],'Sound',["
                "wbFormIDCk('Sound',[SNDR,SOUN,NULL]),"
                "wbInteger('Flags',itU32,wbFlags(["
                "'Pleasant','Cloudy','Rainy','Snowy'"
                "])),"
                "wbFloat('Chance')"
                "]))"
            ),
        )

        # ----------------------------------------------------------------
        # wbStaticPartPlacements — wbDefinitionsCommon.pas:8784-8800.
        # DATA array of Placement structs: position (3 floats) + rotation
        # (3 floats, same wire format as floats) + scale float.
        # ----------------------------------------------------------------
        self.vars.setdefault(
            "wbStaticPartPlacements",
            (
                "wbArrayS(DATA,'Placements',"
                "wbStruct('Placement',["
                "wbStruct('Position',[wbFloat('X'),wbFloat('Y'),wbFloat('Z')]),"
                "wbStruct('Rotation',[wbFloat('X'),wbFloat('Y'),wbFloat('Z')]),"
                "wbFloat('Scale')"
                "]))"
            ),
        )

    def resolve(self, expr: str, depth: int = 0) -> str:
        expr = expr.strip()
        if depth > 20:
            return expr
        if re.fullmatch(r"wb[A-Za-z0-9_]+", expr):
            inner = self.vars.get(expr)
            if inner and inner != expr:
                if inner.startswith("("):
                    return inner
                return self.resolve(inner, depth + 1)
        return expr

    def expand_call(self, expr: str) -> str:
        expr = expr.strip().rstrip(";")
        if not expr:
            return expr
        # identifier reference
        if re.fullmatch(r"wb[A-Za-z0-9_]+", expr):
            if expr in HARD_RAW_VARS:
                return expr
            if expr in self.vars:
                v = self.vars[expr]
                if v.startswith("("):
                    return "wbPLACEHOLDER" + v
                return v
            return expr
        # wbXxx.Method(...) pattern — var reference with trailing method chain.
        # Strip the method chain and resolve just the var part.
        # e.g. wbCNDC.IncludeFlag(dfNoReport) → wbInteger(CNDC, ...)
        # Allow optional whitespace/newline before the dot (Pascal line-continuation
        # style: wbFULL\n    .SetAfterLoad(...)).
        bare_m = re.match(r"^(wb[A-Za-z0-9_]+)\s*\.", expr)
        if bare_m:
            bare = bare_m.group(1)
            if bare not in HARD_RAW_VARS and bare in self.vars:
                v = self.vars[bare]
                if v.startswith("("):
                    return "wbPLACEHOLDER" + v
                return v
        # wbFromVersion / wbBelowVersion
        for gate, key in (
            (r"wbFromVersion\s*\(\s*(\d+)\s*,\s*", "from_version"),
            (r"wbBelowVersion\s*\(\s*(\d+)\s*,\s*", "below_version"),
        ):
            m = re.match(gate, expr)
            if m:
                ver = int(m.group(1))
                rest = expr[m.end() :]
                inner = self.expand_call(rest)
                return f"__{key}__({ver}, {inner})"
        # function call with args
        m = re.match(r"^(wb[A-Za-z0-9_]+)\s*\(", expr)
        if m:
            fn = m.group(1)
            if fn in HARD_RAW_VARS:
                return expr
            lparen = expr.index("(", m.end() - 1)
            rparen = find_matching_paren(expr, lparen)
            args = expr[lparen + 1 : rparen]
            # expand wbGenericModel(True) etc.
            if fn == "wbGenericModel":
                return self.vars["wbGenericModel"]
            if fn == "wbEnchantment":
                return self.vars["wbEnchantment"]
            if fn == "wbOBND":
                return self.vars["wbOBND"]
            if fn == "wbDamageTypeArray":
                parts = split_top_level(args)
                name = unquote(parts[0]) if parts else "Item"
                # Include the form-version 152 Curve Table field.
                # wbDefinitionsCommon.pas:8014-8024.
                return (
                    f"wbArrayS(DAMA, '{name}s', wbStructSK([0], '{name}', ["
                    f"wbFormIDCk('Type', [DMGT]),"
                    f"wbInteger('Amount', itU32),"
                    f"wbFromVersion(152, wbFormIDCk('Curve Table', [CURV, NULL]))]))"
                )
            if fn == "wbModelInfo":
                parts = split_top_level(args)
                sig = parts[0].strip() if parts else "MODT"
                return f"wbByteArray({sig}, 'Model Information', 0)"
            # wbFloatRGBA(SIG) → wbStruct(SIG, 'Color', [...]) — substitute sig
            if fn == "wbFloatRGBA":
                parts = split_top_level(args)
                sig2 = parts[0].strip() if parts else ""
                if sig_id(sig2):
                    return (
                        f"wbStruct({sig2}, 'Color', ["
                        f"wbFloat('Red'), wbFloat('Green'), wbFloat('Blue'), wbFloat('Alpha')])"
                    )
                return self.vars.get("wbFloatRGBA", expr)
            # wbByteColors([SIG,] ['name']) → 4-byte struct (R u8, G u8, B u8, Unused u8).
            # wbDefinitionsCommon.pas:6291-6305.  The no-arg/bare form falls through to
            # the self.vars["wbByteColors"] substitution above.
            if fn == "wbByteColors":
                bc_parts = split_top_level(args)
                if bc_parts and sig_id(bc_parts[0].strip()):
                    sig2 = bc_parts[0].strip()
                    bc_name = unquote(bc_parts[1]) if len(bc_parts) > 1 else "Color"
                    return (
                        f"wbStruct({sig2},'{bc_name}',"
                        f"[wbInteger('Red',itU8),wbInteger('Green',itU8),"
                        f"wbInteger('Blue',itU8),wbUnused(1)])"
                    )
                else:
                    bc_name = (
                        unquote(bc_parts[0])
                        if bc_parts and bc_parts[0].strip().startswith("'")
                        else "Color"
                    )
                    return (
                        f"wbStruct('{bc_name}',"
                        f"[wbInteger('Red',itU8),wbInteger('Green',itU8),"
                        f"wbInteger('Blue',itU8),wbUnused(1)])"
                    )
            # wbAmbientColors([SIG,] ['name']) → 32-byte struct (FO76 branch).
            # Layout: Directional inner-struct (6×4-byte wbByteColors) + wbUnused(4) + wbUnused(4).
            # wbDefinitionsCommon.pas:6238-6263 (IsFO76 branch = wbUnused(4) for both SF1 slots).
            if fn == "wbAmbientColors":
                ac_parts = split_top_level(args)
                _directional = (
                    "wbStruct('Directional',"
                    "[wbByteColors('X+'),wbByteColors('X-'),wbByteColors('Y+'),"
                    "wbByteColors('Y-'),wbByteColors('Z+'),wbByteColors('Z-')])"
                )
                if ac_parts and sig_id(ac_parts[0].strip()):
                    sig2 = ac_parts[0].strip()
                    ac_name = (
                        unquote(ac_parts[1]) if len(ac_parts) > 1 else "Directional Ambient Lighting Colors"
                    )
                    return (
                        f"wbStruct({sig2},'{ac_name}',"
                        f"[{_directional},wbUnused(4),wbUnused(4)])"
                    )
                else:
                    ac_name = (
                        unquote(ac_parts[0])
                        if ac_parts and ac_parts[0].strip().startswith("'")
                        else "Directional Ambient Lighting Colors"
                    )
                    return (
                        f"wbStruct('{ac_name}',"
                        f"[{_directional},wbUnused(4),wbUnused(4)])"
                    )
            # wbVec3PosRot(SIG) → bytes (24 bytes = pos xyz + rot xyz)
            if fn == "wbVec3PosRot":
                parts = split_top_level(args)
                sig2 = parts[0].strip() if parts else "DATA"
                return f"wbByteArray({sig2}, 'Position/Rotation', 24)"
            # wbSizePosRot(SIG, name) → bytes (36 bytes: Size 2f + Pos 3f + Quat 4f).
            # wbDefinitionsCommon.pas:6205-6234.
            if fn == "wbSizePosRot":
                parts = split_top_level(args)
                sig2 = parts[0].strip() if parts else ""
                spr_name = unquote(parts[1]) if len(parts) > 1 else "Size/Pos/Rot"
                if sig_id(sig2):
                    return f"wbByteArray({sig2}, '{spr_name}', 36)"
            # wbDebrisModel(textureHashes) → rstruct with DATA struct + hashes
            if fn == "wbDebrisModel":
                return (
                    "wbRStruct('Model',["
                    "wbStruct(DATA,'Data',["
                    "wbInteger('Percentage',itU8),"
                    "wbString('Model FileName'),"
                    "wbInteger('Has Collision',itU8,wbBoolEnum)]),"
                    f"{args}])"
                )
            # wbTexturedModel('Name', [modSig, txtSig], [extras...])
            # wbDefinitionsCommon.pas:8799-8830 (FO76 branch = filename + model-info + extras).
            # Emits an rstruct with: model filename string, model-info bytes, then the
            # extra subrecord members (MODC, MO2S/MO4S, ENLT, ENLS, AUUV, etc.).
            if fn == "wbTexturedModel":
                t_parts = split_top_level(args)
                t_name = unquote(t_parts[0]) if t_parts else "Model"
                # Parse signature list: [MOD2, MO2T]
                mod_sig, txt_sig = "MODL", "MODT"
                if len(t_parts) > 1:
                    sig_part = t_parts[1].strip()
                    if sig_part.startswith("["):
                        sig_end = find_matching_bracket(sig_part, 0)
                        sig_inner = sig_part[1:sig_end]
                        sig_list = [s.strip() for s in split_top_level(sig_inner)]
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
                        ext_end = find_matching_bracket(extras_part, 0)
                        extras_inner = extras_part[1:ext_end]
                        for e in split_top_level(extras_inner):
                            e = e.strip()
                            if e:
                                members_out.append(e)
                return f"wbRStruct('{t_name}',[{','.join(members_out)}])"
            # wbStructs(sig, groupName, elementName, [fields]) →
            # wbArrayS(sig, groupName, wbStruct(elementName, [fields]))
            # wbDefinitionsCommon.pas interface lines 4467-4475.
            if fn == "wbStructs":
                ws_parts = split_top_level(args)
                if len(ws_parts) >= 4 and sig_id(ws_parts[0].strip()):
                    sig2 = ws_parts[0].strip()
                    ws_name = unquote(ws_parts[1])
                    ws_elem = unquote(ws_parts[2])
                    ws_fields = ws_parts[3].strip()
                    return f"wbArrayS({sig2},'{ws_name}',wbStruct('{ws_elem}',{ws_fields}))"
                elif len(ws_parts) >= 3:
                    ws_name = unquote(ws_parts[0])
                    ws_elem = unquote(ws_parts[1])
                    ws_fields = ws_parts[2].strip()
                    return f"wbArray('{ws_name}',wbStruct('{ws_elem}',{ws_fields}))"
                return expr
            # wbClimateTiming(timeCallback, phaseCallback) →
            # wbStruct(TNAM, 'Timing', [...]).  Callbacks are display-only;
            # the FO76 phase field is always present (callback is non-nil).
            # wbDefinitionsCommon.pas:7995-8010.
            if fn == "wbClimateTiming":
                return (
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
            # wbRFloatColors(name, [sig0, sig1, sig2]) →
            # wbRStruct(name, [wbFloat(sig0,'Red'), wbFloat(sig1,'Green'), wbFloat(sig2,'Blue')])
            # wbDefinitionsCommon.pas:6450-6465.
            if fn == "wbRFloatColors":
                rf_parts = split_top_level(args)
                rf_name = unquote(rf_parts[0]) if rf_parts else "Color"
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
            # wbNPCTemplateActorEntry('Name') → wbFormIDCk('Name', [BMMO, LVLN, NPC_, NULL])
            # wbDefinitionsCommon.pas:7834-7836.
            if fn == "wbNPCTemplateActorEntry":
                t_parts = split_top_level(args)
                t_name = unquote(t_parts[0]) if t_parts else "Actor"
                return f"wbFormIDCk('{t_name}', [BMMO, LVLN, NPC_, NULL])"
            if fn == "wbFaceMorphs":
                fm_parts = split_top_level(args)
                fm_name = unquote(fm_parts[0]) if fm_parts else "Face Morphs"
                return f"wbRArray('{fm_name}',wbFaceMorphElement)"
            if fn == "wbMorphGroups":
                mg_parts = split_top_level(args)
                mg_name = unquote(mg_parts[0]) if mg_parts else "Morph Groups"
                return f"wbRArray('{mg_name}',wbMorphGroupElement)"
            if fn == "wbTintTemplateGroups":
                tt_parts = split_top_level(args)
                tt_name = unquote(tt_parts[0]) if tt_parts else "Tint Layers"
                return f"wbRArray('{tt_name}',wbTintTemplateGroupElement)"
            # wbFromSize(size, value) or wbFromSize(size, sig, value) —
            # conditionally decoded based on total record data length.
            # For FO76 (always the latest game version) the record is always
            # large enough, so we emit the inner value directly and skip
            # the size-guard union.
            # wbDefinitionsCommon.pas:5981-6010.
            if fn == "wbFromSize":
                fs_parts = split_top_level(args)
                if not fs_parts:
                    return expr
                # fs_parts[0] is the size threshold (integer literal)
                # If fs_parts[1] is a sig-id, the form is (size, sig, value).
                # Otherwise it is (size, value).
                if len(fs_parts) >= 3 and sig_id(fs_parts[1].strip()):
                    sig2 = fs_parts[1].strip()
                    value_expr = self.expand_call(fs_parts[2].strip())
                    # Inject the sig into the value if the value is a plain
                    # wb* call (strip any existing leading sig first).
                    # Simplest: wrap the value in a struct-member call with sig.
                    # Because this is a subrecord-level member, just return
                    # the inner value expression — the sig already identifies it.
                    # We rewrite wbSomething('name', ...) → wbSomething(SIG, 'name', ...)
                    m2 = re.match(r"^(wb[A-Za-z0-9_]+)\s*\(", value_expr)
                    if m2:
                        inner_fn = m2.group(1)
                        inner_args_start = value_expr.index("(")
                        inner_rparen = find_matching_paren(value_expr, inner_args_start)
                        inner_args = value_expr[inner_args_start + 1 : inner_rparen]
                        inner_parts = split_top_level(inner_args)
                        # Only inject sig if the first inner arg is NOT already a sig.
                        if inner_parts and not sig_id(inner_parts[0].strip()):
                            return f"{inner_fn}({sig2}, {inner_args})"
                    return value_expr
                elif len(fs_parts) >= 2:
                    # (size, value) — return the value expression directly
                    return self.expand_call(fs_parts[-1].strip())
                return expr
            # wbIMADMultAddCount(name) → wbStruct with Mult Count + Add Count u32 fields.
            # wbDefinitionsCommon.pas:7768-7789.
            if fn == "wbIMADMultAddCount":
                imad_parts = split_top_level(args)
                imad_name = unquote(imad_parts[0]) if imad_parts else "Unknown"
                return (
                    f"wbStruct('{imad_name}',"
                    f"[wbInteger('Mult Count',itU32),wbInteger('Add Count',itU32)])"
                )
            # wbTimeInterpolators(sig, name)  — array of {Time float, Value float} structs.
            # wbTimeInterpolators(name)       — sigless form used inside wbFromVersion wrappers.
            # wbDefinitionsCommon.pas:7886-7893 (no-sig), 8832-8841 (with-sig).
            if fn == "wbTimeInterpolators":
                ti_parts = split_top_level(args)
                _elem = "wbStruct('Data',[wbFloat('Time'),wbFloat('Value')])"
                if len(ti_parts) >= 2 and sig_id(ti_parts[0].strip()):
                    ti_sig = ti_parts[0].strip()
                    ti_name = unquote(ti_parts[1])
                    return f"wbArray({ti_sig},'{ti_name}',{_elem})"
                elif ti_parts:
                    ti_name = unquote(ti_parts[0])
                    return f"wbArray('{ti_name}',{_elem})"
                return expr
            # wbTimeInterpolatorsMultAdd(multSig, addSig, name) →
            # wbRStruct(name, [wbArray(multSig,'Mult',...), wbArray(addSig,'Add',...)]).
            # Mult/Add arrays hold time-interpolated floats for IMAD HDR/Cinematic parameters.
            # wbDefinitionsCommon.pas:8842-8862.
            # multSig / addSig may be binary IAD constants (_00_IAD → chr(0)+'IAD', etc.)
            # resolved from self.vars.  Because split_top_level.strip() destroys bytes
            # like \x09=tab, \x0a=newline, \x0d=CR when they are the first byte of a sig,
            # we build the result dict DIRECTLY and cache it as an __inline__:KEY sentinel
            # rather than returning a Pascal-like string for further parsing.
            if fn == "wbTimeInterpolatorsMultAdd":
                tma_parts = split_top_level(args)
                if len(tma_parts) < 3:
                    return expr
                tma_mult_id = tma_parts[0].strip()
                tma_add_id = tma_parts[1].strip()
                tma_name = unquote(tma_parts[2])
                # Resolve binary sig constants (e.g. _00_IAD → '\x00IAD').
                tma_mult_sig = self.vars.get(tma_mult_id, tma_mult_id)
                tma_add_sig = self.vars.get(tma_add_id, tma_add_id)
                # Fall back: case-insensitive lookup.
                if tma_mult_sig == tma_mult_id:
                    _k = self._vars_lower_map.get(tma_mult_id.lower())
                    if _k:
                        tma_mult_sig = self.vars[_k]
                if tma_add_sig == tma_add_id:
                    _k = self._vars_lower_map.get(tma_add_id.lower())
                    if _k:
                        tma_add_sig = self.vars[_k]
                if not sig_id(tma_mult_sig) or not sig_id(tma_add_sig):
                    return expr
                _elem_def: dict = {
                    "kind": "struct",
                    "name": "Data",
                    "fields": [
                        {"kind": "float", "name": "Time"},
                        {"kind": "float", "name": "Value"},
                    ],
                }
                _unused = tma_name == "Unused"
                _mname = tma_name if _unused else "Mult"
                _aname = tma_name if _unused else "Add"
                _built: dict = {
                    "kind": "rstruct",
                    "name": tma_name,
                    "members": [
                        {"kind": "array", "sig": tma_mult_sig, "name": _mname,
                         "element": _elem_def},
                        {"kind": "array", "sig": tma_add_sig, "name": _aname,
                         "element": _elem_def},
                    ],
                }
                self._inline_counter += 1
                _key = f"tma_{self._inline_counter}"
                self._inline_members[_key] = _built
                return f"__inline__:{_key}"
            if fn in self.vars:
                return self.vars[fn]
            # Case-insensitive var fallback (Pascal is case-insensitive).
            fn_key = self._vars_lower_map.get(fn.lower())
            if fn_key:
                return self.vars[fn_key]
            return expr
        return expr

    def _strip_method_chain(self, expr: str) -> str:
        """Strip Pascal method chains like .SetSummaryKey([...]).IncludeFlag(...)
        that appear after the outermost closing paren of a wb* constructor call.
        These are display/UI hints that carry no structural information."""
        if "(" not in expr:
            return expr
        try:
            lparen = expr.index("(")
            rparen = find_matching_paren(expr, lparen)
            # If the character immediately after the closing paren is '.', it's
            # a method chain — truncate to just the constructor call.
            rest = expr[rparen + 1 :].lstrip()
            if rest.startswith("."):
                return expr[: rparen + 1]
        except ValueError:
            pass
        return expr

    def _strip_version_sig(self, inner_expr: str) -> tuple[str, str | None]:
        """Handle the optional 3-arg form of wbBelowVersion/wbFromVersion.

        Pascal allows ``wbBelowVersion(N, SIG, element)`` where ``SIG`` is a
        4-char subrecord signature that should be attached to the element.
        After ``expand_call`` extracts the version number and comma the
        remainder is ``SIG, element`` — detect and strip the leading sig.

        Returns ``(cleaned_expr, injected_sig_or_None)``.
        """
        sig_m = re.match(r"^([A-Z0-9_]{4})\s*,\s*", inner_expr)
        if sig_m:
            return inner_expr[sig_m.end():], sig_m.group(1)
        return inner_expr, None

    def parse_member(self, expr: str) -> dict | None:
        expr = self.expand_call(expr)
        # __inline__:KEY sentinel: a pre-built schema dict cached in _inline_members.
        # Used for members whose sigs contain bytes that would be stripped by
        # split_top_level (e.g. \x09=tab, \x0a=newline, \x0d=CR as an IAD sig byte).
        if expr.startswith("__inline__:"):
            key = expr[len("__inline__:"):]
            return self._inline_members.get(key)
        # __vmad__ sentinel: reuse the existing vmad decoder in decode.rs.
        if expr == "__vmad__":
            return {"kind": "vmad", "sig": "VMAD", "name": "Virtual Machine Adapter"}
        if expr in HARD_RAW_VARS:
            return {
                "kind": "raw_fallback",
                "name": expr,
                "reason": "runtime Pascal decider",
            }
        # Extract SetCountPath before stripping method chains.
        count_path: str | None = None
        cp = re.search(r"\.SetCountPath\s*\(\s*'([^']+)'", expr)
        if cp:
            count_path = cp.group(1)
        # Strip trailing Pascal method chains before dispatch.
        expr = self._strip_method_chain(expr)

        if expr.startswith("__from_version__"):
            m = re.match(r"__from_version__\((\d+),\s*(.+)\)\s*$", expr, re.DOTALL)
            if m:
                ver = int(m.group(1))
                inner_expr, injected_sig = self._strip_version_sig(m.group(2))
                child = self.parse_member(inner_expr)
                if child:
                    child["from_version"] = ver
                    if injected_sig and "sig" not in child:
                        child["sig"] = injected_sig
                return child
        if expr.startswith("__below_version__"):
            m = re.match(r"__below_version__\((\d+),\s*(.+)\)\s*$", expr, re.DOTALL)
            if m:
                ver = int(m.group(1))
                inner_expr, injected_sig = self._strip_version_sig(m.group(2))
                child = self.parse_member(inner_expr)
                if child:
                    child["below_version"] = ver
                    if injected_sig and "sig" not in child:
                        child["sig"] = injected_sig
                return child

        # wbStruct / wbStructSK / wbStructExSK (ExSK has extra leading [exclude] arg)
        for prefix in ("wbStructExSK", "wbStructSK", "wbStruct"):
            if expr.startswith(prefix + "("):
                return self._parse_struct(expr)
        # wbRStructS must be checked before wbRStruct because the latter is a prefix.
        # wbRStructS('GroupName', 'ElemName', [...]) → rarray of rstruct elements.
        if expr.startswith("wbRStructS") and not expr.startswith("wbRStructSK"):
            return self._parse_rstructS(expr)
        if expr.startswith("wbRStruct") or expr.startswith("wbRStructSK"):
            return self._parse_rstruct(expr)
        if expr.startswith("wbRArray") or expr.startswith("wbRArrayS"):
            result = self._parse_rarray(expr)
            if count_path and isinstance(result, dict) and result.get("kind") == "rarray":
                result["count"] = {"count_path": count_path}
            return result
        if expr.startswith("wbArrayS") or expr.startswith("wbArray"):
            result = self._parse_array(expr)
            if count_path and isinstance(result, dict) and result.get("kind") == "array":
                result["count"] = {"count_path": count_path}
            return result
        if expr.startswith("wbUnion"):
            return self._parse_union(expr)
        if expr.startswith("wbInteger"):
            return self._parse_integer(expr)
        if expr.startswith("wbFloat"):
            if "(" in expr:
                return self._parse_float(expr)
            # bare wbFloat / wbFloatAngle (no parens) — anonymous struct field
            name = "Angle" if expr.startswith("wbFloatAngle") else "Float"
            return {"kind": "float", "name": name}
        if expr.startswith("wbFormIDCk") or expr.startswith("wbFormIDCK"):
            return self._parse_formid(expr)
        # wbFormId (lowercase 'd') is the unchecked variant — no valid_refs list.
        # Must come before the wbFormID (uppercase) check because Python's
        # startswith is case-sensitive; neither prefix is a prefix of the other.
        if expr.startswith("wbFormId") or expr.startswith("wbFormID"):
            return self._parse_formid(expr, ck=False)
        if expr.startswith("wbStringKC") or expr.startswith("wbString"):
            return self._parse_string(expr)
        if expr.startswith("wbLStringKC") or expr.startswith("wbLString"):
            return self._parse_lstring(expr)
        if expr.startswith("wbByteArray"):
            return self._parse_bytes(expr)
        if expr.startswith("wbByteRGBA"):
            return self._parse_byte_rgba(expr)
        if expr.startswith("wbUnused"):
            m = re.search(r"wbUnused\((\d+)\)", expr)
            return {"kind": "unused", "bytes": int(m.group(1)) if m else 0}
        if expr.startswith("wbEmpty"):
            return self._parse_empty(expr)
        if expr.startswith("wbUnknown"):
            return self._parse_unknown(expr)
        # wbVec3 / wbVec3PosRot — vec3 support
        if expr.startswith("wbVec3") and "(" in expr:
            return self._parse_vec3(expr)
        # wbRUnion — record-level polymorphic union; use PresentSignature decider.
        if expr.startswith("wbRUnion"):
            return self._parse_runion(expr)
        # wbLenString — length-prefixed string (all uses in FO76 are inside VMAD).
        if expr.startswith("wbLenString"):
            return {"kind": "raw_fallback", "name": "Length String", "reason": "wbLenString not supported"}
        # wbRecursive — recursive record type; no decoder support.
        if expr.startswith("wbRecursive"):
            return {"kind": "raw_fallback", "name": "Recursive", "reason": "wbRecursive not supported"}
        if re.fullmatch(r"[A-Z0-9_]{4}", expr):
            return self._parse_sig_ref(expr)
        # Unrecognized construct — record for the coverage report, then drop
        # the member.  A non-empty, non-whitespace expression that matches
        # nothing means a Pascal DSL construct we haven't modelled.
        if expr and not expr.isspace():
            _m = re.match(r"^(wb[A-Za-z0-9_]+)", expr)
            _key = _m.group(1) if _m else expr[:40].replace("\n", "\\n")
            self.report.unrecognized_constructs[_key] = (
                self.report.unrecognized_constructs.get(_key, 0) + 1
            )
            # Also track per-record attribution for audit.py.
            rec_key = getattr(self, "_current_record", "")
            if rec_key:
                by_rec = self.report.unrecognized_by_record.setdefault(rec_key, {})
                by_rec[_key] = by_rec.get(_key, 0) + 1
        return None

    def _parse_struct(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        args = expr[lparen + 1 : rparen]
        parts = split_top_level(args)
        sig = None
        name = ""
        # Determine sig/name: skip leading [bracket] args (SK sort keys, ExSK exclude keys).
        # Then the first sig-like token is the sig, and the next quoted string is the name.
        idx = 0
        while idx < len(parts) and parts[idx].strip().startswith("[") and not sig_id(parts[idx].strip()):
            idx += 1
        if idx < len(parts) and sig_id(parts[idx].strip()):
            sig = parts[idx].strip()
            # Sig is followed by possible sort-key arrays, then the name.
            name_idx = idx + 1
            while name_idx < len(parts) and parts[name_idx].strip().startswith("[") and not sig_id(parts[name_idx].strip()):
                name_idx += 1
            if name_idx < len(parts) and parts[name_idx].strip().startswith("'"):
                name = unquote(parts[name_idx])
                fields_search_start = name_idx + 1
            else:
                fields_search_start = name_idx
        elif idx < len(parts) and parts[idx].strip().startswith("'"):
            name = unquote(parts[idx])
            fields_search_start = idx + 1
        else:
            fields_search_start = idx
        # Find the fields list: the FIRST []-starting arg at or after fields_search_start
        # that is not a sort-key array (sort keys contain only digits and commas; field
        # lists contain wb* calls or quoted strings).  wbStructSK can have a trailing
        # summary-sort-key array [0, 1, 2, ...] as its last argument — taking parts[-1]
        # would incorrectly pick that sort key as the fields list.
        fields_part: str | None = None
        for p in parts[fields_search_start:]:
            ps = p.strip()
            if ps.startswith("["):
                # Heuristic: a sort-key array is short and contains only ints/spaces/commas.
                # A fields list contains wb* identifiers or quoted strings.
                inner = ps[1:].rstrip("]").strip()
                if inner and re.match(r"^[\d\s,]+$", inner):
                    continue  # looks like a sort-key array — skip it
                fields_part = p
                break
        if fields_part is None:
            # Fallback: use the last part (or the first []-starting part if last isn't []).
            fields_part = parts[-1]
            if not fields_part.strip().startswith("["):
                for p in parts:
                    if p.strip().startswith("["):
                        fields_part = p
                        break
        fb = fields_part.index("[")
        fe = find_matching_bracket(fields_part, fb)
        fields = self._parse_member_list(fields_part[fb + 1 : fe])
        out: dict = {"kind": "struct", "name": name or sig or "Struct", "fields": fields}
        if sig:
            out["sig"] = sig
        return out

    def _parse_rstruct(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        args = expr[lparen + 1 : rparen]
        parts = split_top_level(args)
        # skip leading [sort_key] array for SK variant
        idx = 0
        while idx < len(parts) and parts[idx].strip().startswith("["):
            idx += 1
        name = unquote(parts[idx]) if idx < len(parts) and parts[idx].strip().startswith("'") else (parts[idx] if idx < len(parts) else "")
        # wbRStruct/wbRStructSK can have trailing args after the member list
        # (e.g., [], cpNormal, False, nil, True).  The member list is the FIRST
        # []-starting part after the name that looks like a member list (not a
        # numeric sort-key array).  The same heuristic as _parse_struct applies.
        members_expr: str | None = None
        for p in parts[idx + 1:]:
            ps = p.strip()
            if ps.startswith("["):
                inner = ps[1:].rstrip("]").strip()
                if inner and re.match(r"^[\d\s,]+$", inner):
                    continue  # numeric sort-key array — skip
                members_expr = p
                break
        if members_expr is None:
            # Fallback: first []-starting arg anywhere in parts
            members_expr = next((p for p in parts if p.strip().startswith("[")), parts[-1])
        if "[" not in members_expr:
            return {"kind": "raw_fallback", "name": name or "rstruct", "reason": "rstruct variable ref"}
        start = members_expr.index("[")
        fe = find_matching_bracket(members_expr, start)
        members = self._parse_member_list(members_expr[start + 1 : fe])
        return {"kind": "rstruct", "name": name, "members": members}

    def _parse_rstructS(self, expr: str) -> dict:
        """wbRStructS('GroupName', 'ElemName', [...members...]) → rarray of rstruct.

        Unlike wbRStruct (one-shot), wbRStructS is a *repeating* rstruct — the decoder
        iterates while the element's anchor sig keeps appearing.  Maps to the rarray
        schema kind with an rstruct element (same as wbRArrayS of wbRStruct).

        Member name deduplication: if two members share the same name (e.g. both ANAM
        and BNAM are named 'Part' in AAPD), the second occurrence is renamed to
        'Name (SIG)' so JSON keys remain distinct.
        """
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        args = expr[lparen + 1 : rparen]
        parts = split_top_level(args)
        group_name = unquote(parts[0]) if parts else ""
        elem_name = unquote(parts[1]) if len(parts) > 1 else group_name
        # Members list is the first []-starting part after the two name args.
        members_expr: str | None = None
        for p in parts[2:]:
            ps = p.strip()
            if ps.startswith("["):
                members_expr = p
                break
        if members_expr is None:
            return {"kind": "raw_fallback", "name": group_name or "rstructS", "reason": "rstructS variable ref"}
        start = members_expr.index("[")
        fe = find_matching_bracket(members_expr, start)
        raw_members = self._parse_member_list(members_expr[start + 1 : fe])
        # Deduplicate member names: rename subsequent collisions to 'Name (SIG)'.
        seen_names: set[str] = set()
        members: list[dict] = []
        for mem in raw_members:
            mem_name = mem.get("name", "")
            if mem_name and mem_name in seen_names:
                mem_sig = mem.get("sig", "")
                if mem_sig:
                    mem = dict(mem)
                    mem["name"] = f"{mem_name} ({mem_sig})"
            if mem_name:
                seen_names.add(mem_name)
            members.append(mem)
        return {
            "kind": "rarray",
            "name": group_name,
            "element": {"kind": "rstruct", "name": elem_name, "members": members},
        }

    def _parse_rarray(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        args = split_top_level(expr[lparen + 1 : rparen])
        name = unquote(args[0]) if args[0].strip().startswith("'") else args[0]
        # Element is always args[1] (name is args[0]).
        # args[2:] may be count, priority, or other trailing options.
        elem_expr = args[1] if len(args) > 1 else args[-1]
        elem = self.parse_member(elem_expr)
        return {"kind": "rarray", "name": name, "element": elem or {"kind": "unknown", "name": "element"}}

    def _parse_array(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        name = ""
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            name = unquote(parts[1]) if len(parts) > 1 else ""
            # Element is at index 2 when leading sig is present.
            elem_idx = 2
        else:
            name = unquote(parts[0])
            # Element is at index 1 when no leading sig.
            elem_idx = 1
        # Clamp to valid range.
        if elem_idx >= len(parts):
            elem_idx = len(parts) - 1
        elem_expr = parts[elem_idx]
        elem = self.parse_member(elem_expr)
        out: dict = {
            "kind": "array",
            "name": name,
            "element": elem or {"kind": "unknown", "name": "element"},
        }
        if sig:
            out["sig"] = sig
        # Capture count argument.  xEdit uses negative integers to signal an inline
        # count prefix stored directly in the subrecord data.  The prefix byte width
        # is encoded in the magnitude: -1 → 4 bytes, -2 → 2 bytes, -4 → 1 byte
        # (see _PREFIX_WIDTHS and TwbArrayDef.GetPrefixLength in wbInterface.pas).
        count_arg_idx = elem_idx + 1
        if count_arg_idx < len(parts):
            count_str = parts[count_arg_idx].strip()
            try:
                cval = int(count_str)
            except ValueError:
                cval = None
            if cval is not None and cval < 0:
                width = _PREFIX_WIDTHS.get(cval)
                if width is None:
                    import sys as _sys
                    print(
                        f"warning: unhandled negative array count {cval} "
                        f"for {out.get('name')!r}; defaulting prefix width to 4",
                        file=_sys.stderr,
                    )
                    width = 4
                out["count"] = {"count_prefix": width}
        return out

    def _parse_union(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        # wbUnion can have an optional leading sig (4-char uppercase token)
        # e.g. wbUnion(LVLF, 'Flags', decider, [...])
        #   vs wbUnion('Flags', decider, [...])
        sig = None
        idx = 0
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            idx = 1
        name = unquote(parts[idx]) if parts[idx].strip().startswith("'") else parts[idx].strip()
        decider_expr = parts[idx + 1]
        identity_map_field: str | None = None
        decider: dict
        if "wbFormVersionDecider" in decider_expr:
            # wbFormVersionDecider([N, M, ...]) — multi-threshold array form (check first).
            ma = re.search(r"wbFormVersionDecider\(\[([^\]]+)\]\)", decider_expr)
            # wbFormVersionDecider(N) — single threshold
            m = re.search(r"wbFormVersionDecider\((\d+)(?:\s*,\s*(\d+))?\)", decider_expr)
            if ma:
                # Multi-threshold: N thresholds → N+1 variants.
                # Returns the index of the first threshold that is > form_version.
                thresholds = [int(x.strip()) for x in ma.group(1).split(",") if x.strip()]
                decider = {"form_version_thresholds": thresholds}
            elif m:
                decider = {
                    "form_version": {
                        "min": int(m.group(1)),
                        "max": int(m.group(2)) if m.group(2) else None,
                    }
                }
            else:
                decider = {"raw": True}
        elif "Decider" in decider_expr or "wbCondition" in decider_expr:
            # Check for known closure deciders.
            # Entries with "kind" key → full union substitution (replace everything).
            # Entries without "kind" key → decider-only substitution (keep Pascal variants).
            decider = {"raw": True}
            for known_fn, subst in KNOWN_UNION_DECIDERS.items():
                if known_fn in decider_expr:
                    if "kind" in subst:
                        # Full substitution: return the pre-built union dict as-is.
                        out = dict(subst)
                        if out.get("name") is None:
                            out["name"] = name or sig or "union"
                        if sig and "sig" not in out:
                            out["sig"] = sig
                        return out
                    elif subst.get("__identity_map__"):
                        # Identity map: variant index == the field's value; the
                        # map is materialized below once the variant count is known.
                        identity_map_field = subst["field"]
                        decider = {}
                    else:
                        # Decider-only: use the subst dict AS the decider,
                        # and fall through to parse variants from Pascal.
                        decider = dict(subst)
                    break
        else:
            decider = {"raw": True}
        if decider.get("raw"):
            out: dict = {
                "kind": "raw_fallback",
                "name": name or sig or "union",
                "reason": "closure union decider",
            }
            if sig:
                out["sig"] = sig
            return out
        variants_expr = parts[idx + 2]
        if "[" not in variants_expr:
            return {
                "kind": "raw_fallback",
                "name": name or sig or "union",
                "reason": "closure union decider",
            }
        vb = variants_expr.index("[")
        ve = find_matching_bracket(variants_expr, vb)
        variants = self._parse_member_list(variants_expr[vb + 1 : ve])
        if identity_map_field is not None:
            # No default_variant: out-of-range values fall to the safe
            # "union decider unresolved" raw path, mirroring xEdit.
            decider = {
                "field": identity_map_field,
                "map": {str(i): i for i in range(len(variants))},
            }
        out: dict = {"kind": "union", "name": name or sig or "union", "decider": decider, "variants": variants}
        if sig:
            out["sig"] = sig
        return out

    def _parse_integer(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        idx = 0
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            idx = 1
        name = unquote(parts[idx])
        if len(parts) > idx + 1:
            itype = parts[idx + 1].strip()
            # Pascal identifiers are case-insensitive, so normalise the
            # it[su]NN form (e.g. 'its32' → 'itS32', 'itu16' → 'itU16').
            # Without this, a lowercase variant silently defaults to u32
            # (a width-skew risk).
            itype = re.sub(
                r"^it([su])(\d+)$",
                lambda m: f"it{m.group(1).upper()}{m.group(2)}",
                itype,
            )
            # Instrument: if itype is still not a known token after normalisation
            # it will silently default to (u32, False) — a width-skew risk.
            if itype not in INT_MAP:
                self.report.defaulted_int_tokens[itype] = (
                    self.report.defaulted_int_tokens.get(itype, 0) + 1
                )
        else:
            itype = "itU32"
            self.report.missing_int_type += 1
        width, signed = INT_MAP.get(itype, ("u32", False))
        out: dict = {
            "kind": "integer",
            "name": name,
            "width": width,
            "signed": signed,
        }
        if sig:
            out["sig"] = sig
        if len(parts) > idx + 2:
            fmt_arg = parts[idx + 2].strip()
            # Resolve a bare wb* variable reference (e.g. wbLVLFFlags → wbFlags([...]))
            # before dispatching to parse_format_arg, which only handles literals.
            if re.fullmatch(r"wb[A-Za-z0-9_]+", fmt_arg) and fmt_arg in self.vars:
                fmt_arg = self.vars[fmt_arg]
            if "wbObjectModPropertyToStr" in fmt_arg:
                # Property index whose enum depends on the parent Data struct's Form Type.
                # Emit a FieldValue union; the decoder resolves Form Type from outer context.
                # Form Type keys are Sig2Int LE values: WEAP=1346454871, ARMO=1330467393, NPC_=1598246990.
                union: dict = {
                    "kind": "union",
                    "name": name,
                    "decider": {
                        "field": "Form Type",
                        "default_variant": 0,
                        "map": {
                            "1346454871": 1,
                            "1330467393": 2,
                            "1598246990": 3,
                        },
                    },
                    "variants": [
                        {"kind": "integer", "name": name, "width": "u16", "signed": False},
                        {"kind": "integer", "name": name, "width": "u16", "signed": False,
                         "format": {"enum": _WEAPON_PROPERTIES}},
                        {"kind": "integer", "name": name, "width": "u16", "signed": False,
                         "format": {"enum": _ARMOR_PROPERTIES}},
                        {"kind": "integer", "name": name, "width": "u16", "signed": False,
                         "format": {"enum": _ACTOR_PROPERTIES}},
                    ],
                }
                if sig:
                    union["sig"] = sig
                return union
            fmt = parse_format_arg(fmt_arg)
            if fmt:
                out["format"] = fmt
        return out

    def _parse_float(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        idx = 0
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            idx = 1
        name = unquote(parts[idx]) if idx < len(parts) else (sig or "Float")
        out: dict = {"kind": "float", "name": name}
        if sig:
            out["sig"] = sig
        return out

    def _parse_formid(self, expr: str, ck: bool = True) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        idx = 0
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            idx = 1
        name = unquote(parts[idx])
        refs: list[str] = []
        if ck and len(parts) > idx + 1:
            raw_refs = parts[idx + 1]
            if "[" in raw_refs:
                refs = re.findall(r"[A-Z0-9_]{4}", raw_refs)
            elif raw_refs.strip() in self.sig_lists:
                refs = self.sig_lists[raw_refs.strip()]
        out: dict = {"kind": "formid", "name": name, "valid_refs": refs}
        if sig:
            out["sig"] = sig
        return out

    def _parse_string(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        idx = 0
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            idx = 1
        if idx >= len(parts):
            name = sig or "String"
        else:
            name = unquote(parts[idx]) if parts[idx].strip().startswith("'") else parts[0]
        out: dict = {"kind": "string", "name": name, "keep_case": "KC" in expr[:20]}
        if sig:
            out["sig"] = sig
        for p in parts:
            if p.strip().isdigit():
                out["sized"] = int(p.strip())
        return out

    def _parse_lstring(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = parts[0].strip()
        name = unquote(parts[1]) if len(parts) > 1 else sig
        return {"kind": "lstring", "sig": sig, "name": name}

    def _parse_bytes(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        idx = 0
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            idx = 1
        name = unquote(parts[idx]) if parts[idx].strip().startswith("'") else parts[idx]
        length = None
        for p in parts:
            if p.strip().isdigit():
                length = int(p.strip())
        out: dict = {"kind": "bytes", "name": name}
        if sig:
            out["sig"] = sig
        if length:
            out["len"] = length
        return out

    def _parse_byte_rgba(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = parts[0].strip()
        name = unquote(parts[1]) if len(parts) > 1 else "Color"
        return {"kind": "byte_rgba", "sig": sig, "name": name}

    def _parse_empty(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        out: dict = {"kind": "empty", "name": "Empty"}
        if not parts:
            return out
        if sig_id(parts[0].strip()):
            out["sig"] = parts[0].strip()
            out["name"] = unquote(parts[1]) if len(parts) > 1 else out["sig"]
        else:
            out["name"] = unquote(parts[0])
        return out

    def _parse_unknown(self, expr: str) -> dict:
        m = re.match(r"wbUnknown\((.*)\)", expr, re.DOTALL)
        sig = None
        name = "Unknown"
        if m:
            args_str = m.group(1).strip()
            if args_str:
                # Use split_top_level to get only the first argument so that
                # extra args like `wbUnknown(VNAM, cpNormal, True)` don't
                # prevent sig extraction.
                parts = split_top_level(args_str)
                first = parts[0].strip() if parts else ""
                if sig_id(first):
                    sig = first
        out: dict = {"kind": "unknown", "name": name}
        if sig:
            out["sig"] = sig
        return out

    def _parse_vec3(self, expr: str) -> dict:
        """wbVec3('Name') or wbVec3(SIG, 'Name') → {kind: vec3, ...}"""
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        idx = 0
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            idx = 1
        name = unquote(parts[idx]) if idx < len(parts) and parts[idx].strip().startswith("'") else (sig or "Vec3")
        out: dict = {"kind": "vec3", "name": name}
        if sig:
            out["sig"] = sig
        return out

    def _parse_sig_ref(self, sig: str) -> dict:
        return {"kind": "unknown", "sig": sig, "name": sig}

    def _extract_anchor_sig(self, member: dict | None) -> str | None:
        """Return the first 4-char subrecord signature found in a parsed member dict."""
        sigs = self._extract_anchor_sigs(member)
        return sigs[0] if sigs else None

    def _unwrap_member_list(self, member: dict | None) -> list[dict]:
        if member is None:
            return []
        if member.get("kind") in ("rstruct", "struct"):
            return member.get("members") or member.get("fields") or []
        return [member]

    def _direct_sibling_sigs(self, member: dict | None) -> list[str]:
        """Top-level sig members of an rstruct/struct variant branch (stops at nested rstruct/union)."""
        sigs: list[str] = []
        for child in self._unwrap_member_list(member):
            if child.get("kind") == "union":
                break
            if child.get("kind") in ("rstruct", "struct"):
                break
            sig = child.get("sig")
            if sig and re.fullmatch(r"[A-Z0-9_]{4}", sig):
                sigs.append(sig)
        return sigs

    def _extract_anchor_sigs(self, member: dict | None) -> list[str]:
        """Return discriminant subrecord signatures for a wbRUnion variant.

        Normally the first sig-bearing member selects the variant.  When a variant
        begins with a nested wbRUnion (QUST Alias Fill-Type → Match Type), collect
        the first sig (plus any sibling sigs) from each nested branch so ALNA/ALFE/
        ALFD/ALCC all resolve to the same parent variant.
        """
        if member is None:
            return []

        sig = member.get("sig")
        if sig and re.fullmatch(r"[A-Z0-9_]{4}", sig):
            return [sig]

        members = self._unwrap_member_list(member)
        if members and members[0].get("kind") == "union":
            sigs: list[str] = []
            for branch in members[0].get("variants", []):
                for sig in self._direct_sibling_sigs(branch):
                    if sig not in sigs:
                        sigs.append(sig)
            return sigs

        sigs: list[str] = []
        for child in members:
            if child.get("kind") == "union":
                break
            if child.get("kind") in ("rstruct", "struct"):
                break
            sig = child.get("sig")
            if sig and re.fullmatch(r"[A-Z0-9_]{4}", sig):
                sigs.append(sig)
        if sigs:
            return sigs

        # Variant may lead with a nested union/struct (QUST General → DATA); fall back
        # to the first sig anywhere in the variant subtree.
        first = self._extract_first_anchor_sig(member)
        if first:
            return [first]
        return []

    def _extract_first_anchor_sig(self, member: dict | None) -> str | None:
        if member is None:
            return None
        sig = member.get("sig")
        if sig and re.fullmatch(r"[A-Z0-9_]{4}", sig):
            return sig
        for child in member.get("members", []):
            found = self._extract_first_anchor_sig(child)
            if found:
                return found
        for child in member.get("fields", []):
            found = self._extract_first_anchor_sig(child)
            if found:
                return found
        for child in member.get("variants", []):
            found = self._extract_first_anchor_sig(child)
            if found:
                return found
        elem = member.get("element")
        if elem:
            return self._extract_first_anchor_sig(elem)
        return None

    def _parse_runion(self, expr: str) -> dict:
        """Parse a wbRUnion(...) expression into a union MemberDef.

        Signature forms:
            wbRUnion('Name', [variants])                  — no decider (PresentSignature)
            wbRUnion('Name', wbSomeDecider, [variants])   — explicit decider (check KNOWN_UNION_DECIDERS)

        For the no-decider form, the anchor signature of each variant is determined
        by the first sig-bearing member it contains, and a PresentSignature decider
        is emitted.  For the explicit-decider form, KNOWN_UNION_DECIDERS is checked;
        if not found, a raw_fallback is returned.
        """
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        if not parts:
            return {"kind": "raw_fallback", "name": "Record Union", "reason": "wbRUnion empty args"}

        name = unquote(parts[0]) if parts[0].strip().startswith("'") else parts[0].strip()

        # Detect whether parts[1] is an explicit decider or the variant list.
        # The variant list is a '[...]' expression; a decider is a wb* identifier/call.
        decider: dict | None = None
        variants_idx = 1
        if len(parts) > 2 and not parts[1].strip().startswith("["):
            decider_expr = parts[1].strip()
            variants_idx = 2
            for known_fn, subst in KNOWN_UNION_DECIDERS.items():
                if known_fn in decider_expr:
                    if "kind" in subst:
                        # Full substitution includes its own variant list; but for
                        # wbRUnion we still want the Pascal variants.  Extract only
                        # the decider sub-dict.
                        decider = dict(subst.get("decider", {})) or dict(subst)
                    else:
                        # Decider-only substitution: use the subst dict as-is.
                        decider = dict(subst)
                    break
            if decider is None:
                return {
                    "kind": "raw_fallback",
                    "name": name or "Record Union",
                    "reason": f"wbRUnion decider not recognized: {decider_expr[:50]}",
                }

        if variants_idx >= len(parts):
            return {"kind": "raw_fallback", "name": name or "Record Union", "reason": "wbRUnion missing variants"}
        variants_expr = parts[variants_idx]
        if "[" not in variants_expr:
            return {"kind": "raw_fallback", "name": name or "Record Union", "reason": "wbRUnion variants not a list"}
        vb = variants_expr.index("[")
        ve = find_matching_bracket(variants_expr, vb)
        variants = self._parse_member_list(variants_expr[vb + 1 : ve])
        if not variants:
            return {"kind": "raw_fallback", "name": name or "Record Union", "reason": "wbRUnion no variants parsed"}

        if decider is None:
            # Build PresentSignature from all reachable anchor sigs of each variant.
            anchors = [self._extract_anchor_sigs(v) for v in variants]
            # Drop sigs shared across variants (e.g. ALID on every QUST Alias type) so
            # only discriminant anchors remain.
            from collections import Counter

            freq = Counter(sig for a in anchors for sig in a)
            shared = {sig for sig, n in freq.items() if n > 1}
            if shared:
                anchors = [[sig for sig in a if sig not in shared] for a in anchors]
            for i, a in enumerate(anchors):
                if not a:
                    first = self._extract_first_anchor_sig(variants[i])
                    anchors[i] = [first] if first else []
            decider = {"present_signature": anchors}

        return {"kind": "union", "name": name or "Record Union", "decider": decider, "variants": variants}

    def _parse_member_list(self, text: str) -> list[dict]:
        items = split_top_level(text)
        out: list[dict] = []
        for item in items:
            item = item.strip()
            if not item or item.startswith("//"):
                continue
            # Strip leading Pascal block-comment labels like {0}, {1}, {abc}
            # that xEdit uses to annotate union variant indices. They must be
            # removed before the wb* prefix check below can match.
            item = re.sub(r"^\{[^}]*\}\s*", "", item).strip()
            if not item:
                continue
            m = re.match(r"^(wb[A-Za-z0-9_]+)\s*$", item)
            if m:
                var_name = m.group(1)
                if var_name in self.vars:
                    item = self.vars[var_name]
                else:
                    # Pascal identifiers are case-insensitive.  Fall back to the
                    # O(1) cached lowercase map so that e.g. 'wbDesc' resolves to
                    # the 'wbDESC' var collected from the `:=` assignment.
                    var_key = self._vars_lower_map.get(var_name.lower())
                    if var_key:
                        item = self.vars[var_key]
            parsed = self.parse_member(item)
            if parsed:
                out.append(parsed)
        return out

    def _get_reference_record_members(self) -> list[dict]:
        """Extract and cache the member list from the ReferenceRecord procedure body.

        ReferenceRecord(SIG, name) is a Pascal procedure (wbDefinitionsFO76.pas:4057)
        that calls wbRefRecord with a fixed flags block and a shared member list.
        All PGRE/PHZD/PMIS (and PARW/PBAR/PBEA/PCON/PFLA) share the same members.
        We parse the procedure body once and cache the result.
        """
        if hasattr(self, "_reference_record_members_cache"):
            return self._reference_record_members_cache

        # Find the procedure declaration
        proc_m = re.search(r"\bprocedure\s+ReferenceRecord\s*\(", self.fo76)
        if not proc_m:
            self._reference_record_members_cache: list[dict] = []
            return []

        # Locate "begin" after the procedure signature
        after_sig = self.fo76[proc_m.end():]
        begin_m = re.search(r"\bbegin\b", after_sig[:2000])
        if not begin_m:
            self._reference_record_members_cache = []
            return []

        body_start = proc_m.end() + begin_m.end()

        # Find the wbRefRecord( call inside the body
        ref_m = re.search(r"\bwbRefRecord\s*\(", self.fo76[body_start:body_start + 20000])
        if not ref_m:
            self._reference_record_members_cache = []
            return []

        wbref_abs = body_start + ref_m.start()
        wbref_lparen = self.fo76.index("(", wbref_abs)
        wbref_rparen = find_matching_paren(self.fo76, wbref_lparen)

        # wbRefRecord args: (aSignature, aName, wbFlags(...), [...members...])
        inner = self.fo76[wbref_lparen + 1 : wbref_rparen]
        parts = split_top_level(inner)

        # Find the [...members...] arg (last top-level [...] part)
        members_expr = next(
            (p for p in reversed(parts) if p.strip().startswith("[")), None
        )
        if members_expr is None:
            self._reference_record_members_cache = []
            return []

        mb = members_expr.index("[")
        me = find_matching_bracket(members_expr, mb)
        members = self._parse_member_list(members_expr[mb + 1 : me])
        self._reference_record_members_cache = members
        return members

    def extract_record(self, sig: str) -> dict | None:
        # Track current record for error attribution in the coverage report.
        self._current_record = sig
        # Match both wbRecord(...) and wbRefRecord(...).
        # wbRefRecord is used for placed-object records (REFR, ACHR) and has the
        # same positional layout as wbRecord — the members list is always the
        # last bracketed top-level argument.
        pattern = rf"\bwbRe(?:cord|fRecord)\s*\(\s*{sig}\s*,"
        m = re.search(pattern, self.fo76)
        if not m:
            # Check for records defined via the ReferenceRecord(SIG, name) macro.
            # These share the member list from the procedure body (wbDefinitionsFO76.pas:4057).
            ref_pattern = rf"\bReferenceRecord\s*\(\s*{sig}\s*,"
            ref_m = re.search(ref_pattern, self.fo76)
            if not ref_m:
                return None
            name_m = re.search(
                rf"\bReferenceRecord\s*\(\s*{sig}\s*,\s*'([^']+)'", self.fo76
            )
            name = name_m.group(1) if name_m else sig
            members = self._get_reference_record_members()
            return {"name": name, "members": members}
        lparen = self.fo76.index("(", m.start())
        rparen = find_matching_paren(self.fo76, lparen)
        args = self.fo76[lparen + 1 : rparen]
        parts = split_top_level(args)
        name = unquote(parts[1])
        members_expr = next(p for p in reversed(parts) if p.strip().startswith("["))
        mb = members_expr.index("[")
        me = find_matching_bracket(members_expr, mb)
        members = self._parse_member_list(members_expr[mb + 1 : me])
        return {"name": name, "members": members}

    def run(self) -> dict:
        records: dict = {}
        for sig in WHITELIST:
            try:
                rec = self.extract_record(sig)
            except Exception as e:
                self.report.failed_records += 1
                print(f"WARNING: failed to extract {sig}: {e}", file=sys.stderr)
                rec = None
            if rec:
                records[sig] = rec
                print(f"extracted {sig}: {len(rec['members'])} members", file=sys.stderr)
            else:
                print(f"WARNING: missing {sig}", file=sys.stderr)

        # Annotate Conditions rarrays with stop_before boundaries so the decoder
        # does not greedily consume per-entry CTDAs into the record-level Conditions
        # slot.
        for rec in records.values():
            _annotate_stop_before(rec.get("members", []), [])

        # Fixup OBTS "Property" union default_variant per record type.
        # OBTS contains wbObjectModProperties whose "Property" field is a FieldValue
        # union keyed on "Form Type" (an OMOD DATA field absent in OBTS).  The decoder
        # falls back to default_variant when "Form Type" is not in scope; patch it to
        # the correct variant for the owning record type so property names are labelled.
        # WEAP→1 (weapon props), ARMO→2 (armor props), NPC_→3 (actor props).
        _OBTS_PROP_DEFAULT: dict[str, int] = {"ARMO": 2, "WEAP": 1, "NPC_": 3}
        for sig, rec in records.items():
            dv = _OBTS_PROP_DEFAULT.get(sig)
            if dv is not None:
                _fixup_obts_property_default(rec.get("members", []), dv)

        if "QUST" in records:
            _patch_qust_location_fill_type(records["QUST"], self)

        for rec in records.values():
            _apply_schema_kinds(rec.get("members", []))
            _dedup_field_names(rec.get("members", []))

        # ── Extraction coverage summary ──────────────────────────────────────
        # Count total raw_fallback members across all extracted records.
        def _count_rf(mlist: list) -> int:
            c = 0
            for _m in mlist:
                if not isinstance(_m, dict):
                    continue
                if _m.get("kind") == "raw_fallback":
                    c += 1
                for _k in ("members", "fields", "variants"):
                    c += _count_rf(_m.get(_k, []))
                _elem = _m.get("element")
                if isinstance(_elem, dict):
                    c += _count_rf([_elem])
            return c

        total_raw = sum(_count_rf(r.get("members", [])) for r in records.values())
        print("=== extraction coverage ===", file=sys.stderr)
        print(
            f"records: {len(records)} ok, {self.report.failed_records} failed",
            file=sys.stderr,
        )
        if self.report.defaulted_int_tokens:
            # Non-empty means an integer type token was not in INT_MAP and
            # silently defaulted to (u32, False) — a width-skew risk.
            print(
                f"defaulted int tokens (width-skew risk!): "
                f"{dict(self.report.defaulted_int_tokens)}",
                file=sys.stderr,
            )
        else:
            print("defaulted int tokens: {}  (good — no silent u32 defaults)", file=sys.stderr)
        if self.report.unrecognized_constructs:
            _top = sorted(self.report.unrecognized_constructs.items(), key=lambda x: -x[1])[:10]
            print(f"unrecognized constructs (top-10): {dict(_top)}", file=sys.stderr)
        print(f"raw_fallbacks: {total_raw}", file=sys.stderr)

        # Strict-mode enforcement: fail on critical diagnostics.
        if self.report.strict:
            if self.report.defaulted_int_tokens:
                raise RuntimeError(
                    f"[strict] unknown integer type token(s): "
                    f"{list(self.report.defaulted_int_tokens)}"
                )
            if self.report.failed_records > 0:
                raise RuntimeError(
                    f"[strict] {self.report.failed_records} record(s) failed to extract"
                )

        return {"records": records}


def _apply_schema_kinds(members: list) -> None:
    """Replace magic-string dispatch targets with explicit schema kinds.

    CTDA appears in the extractor output (and in overrides) as a *struct*
    with reference fields; the runtime decoder is `src/ctda.rs`, so the
    field list is dropped along with the kind swap. Also runs over the
    merged overrides in main() — override-sourced CTDA structs (NPC_
    Conditions, PERK Effect patch) must convert too.
    """
    for m in members:
        if not isinstance(m, dict):
            continue
        if m.get("sig") == "CTDA" and m.get("kind") in ("bytes", "struct"):
            m["kind"] = "ctda"
            m.pop("fields", None)
        elif m.get("kind") == "bytes" and m.get("name") == "Model Information":
            m["kind"] = "model_info"
        for key in ("members", "fields", "variants"):
            _apply_schema_kinds(m.get(key, []))
        elem = m.get("element")
        if isinstance(elem, dict):
            _apply_schema_kinds([elem])


def _dedup_field_names(members: list) -> None:
    """Rename duplicate sibling field names to '<name> 2', '<name> 3', …

    Bakes `insert_unique`'s runtime disambiguation (e.g. MGEF's twin
    wbActorValue slots → "Actor Value 2") into the schema so output keys
    are declared rather than patched at decode time. Union `variants` are
    deliberately NOT deduped — only one variant decodes per record, so
    same-named variants never collide.
    """
    if not members:
        return
    seen: dict[str, int] = {}
    for m in members:
        if not isinstance(m, dict):
            continue
        name = m.get("name")
        if isinstance(name, str) and name:
            count = seen.get(name, 0) + 1
            seen[name] = count
            if count > 1:
                m["name"] = f"{name} {count}"
        for key in ("members", "fields"):
            _dedup_field_names(m.get(key, []))
        elem = m.get("element")
        if isinstance(elem, dict):
            _dedup_field_names([elem])


# pt* → CTDA param class (must match ctda.rs decode_param).
# Compared lowercased: the Pascal is case-inconsistent (ptWorldspace vs ptWorldSpace).
_PT_FORM = {
    "ptacousticspace", "ptactor", "ptactorbase", "ptassociationtype", "ptbaseobject",
    "ptcell", "ptchallenge", "ptclass", "ptconditionform", "ptconstructibleobject",
    "ptcurrency", "ptdailycontentgroup", "ptdamagetype", "pteffectitem", "ptencounterzone",
    "ptentitlement", "ptequiptype", "pteventdata", "ptfaction", "ptfactionnull",
    "ptformlist", "ptfurniture", "ptglobal", "ptidleform", "ptinventoryobject",
    "ptkeyword", "ptlocation", "ptlocationreftype", "ptmagiceffect", "ptowner",
    "ptpackage", "ptperk", "ptperkcard", "ptquest", "ptrace", "ptreference",
    "ptregion", "ptscene", "ptspell", "ptvoicetype", "ptweather", "ptworldspace",
}
_PT_INT = {
    "ptinteger", "ptalias", "ptattackdata", "ptevent", "ptpackdata",
    "ptqueststage1", "ptqueststage2",
    "ptalignment", "ptaxis", "ptcastingsource", "ptcrimetype", "ptcriticalstage",
    "ptfurnitureanim", "ptmiscstat", "ptsex", "ptwardstate", "ptfurnitureentry",
}


def _pt_to_class(pt: str) -> str:
    pt = (pt or "").lower()
    if not pt or pt == "ptnone":
        return "N"
    if pt == "ptfloat":
        return "F"
    if pt == "ptstring":
        return "S"
    if pt == "ptactorvalue":
        return "A"
    if pt == "ptformtype":
        return "T"
    if pt in _PT_FORM:
        return "R"
    if pt not in _PT_INT:
        print(f"warning: unknown ParamType {pt!r} classed as 'I'", file=sys.stderr)
    return "I"


def emit_ctda_table(pas_text: str) -> dict:
    """Parse wbConditionFunctions from FO76.pas → fo76.ctda.json payload."""
    start = pas_text.index("wbConditionFunctions")
    block = pas_text[start : start + 400_000]
    end = block.index(");")
    block = block[block.index("(") + 1 : end]
    entry_re = re.compile(
        r"\(\s*Index\s*:\s*(\d+)\s*;\s*Name\s*:\s*'((?:[^']|'')*)'"
        r"(?:[^;]*;\s*Desc\s*:\s*'(?:[^']|'')*')?"
        r"(?:[^)]*Param[Tt]ype1\s*:\s*(pt\w+))?"
        r"(?:[^)]*Param[Tt]ype2\s*:\s*(pt\w+))?"
        r"(?:[^)]*Param[Tt]ype3\s*:\s*(pt\w+))?",
        re.DOTALL,
    )
    functions = []
    for m in entry_re.finditer(block):
        idx = int(m.group(1))
        name = m.group(2).replace("''", "'")
        p1 = _pt_to_class(m.group(3) or "ptNone")
        p2 = _pt_to_class(m.group(4) or "ptNone")
        p3 = _pt_to_class(m.group(5) or "ptNone")
        functions.append({"index": idx, "name": name, "p1": p1, "p2": p2, "p3": p3})
    functions.sort(key=lambda e: e["index"])
    return {"functions": functions}


def _expand_pascal_var(node: dict, ex: "Extractor") -> dict:
    """Expand ``{"$pascal_var": "wbXALG"}`` override nodes via the extractor.

    Lets an override addition reference a Pascal helper var by name so the
    member definition (names, widths, flag lists) stays in sync with the
    Pascal instead of being duplicated into the overrides JSON.
    """
    var = node.get("$pascal_var")
    if not var:
        return node
    expr = ex.vars.get(var)
    if not expr:
        raise ValueError(f"unknown Pascal var {var!r} for $pascal_var expansion")
    expanded = ex.parse_member(expr)
    if not isinstance(expanded, dict):
        raise ValueError(f"$pascal_var {var!r} did not expand to a member dict")
    return expanded


def _fixup_obts_property_default(members: list, default_variant: int) -> None:
    """Walk schema members and set the OBTS 'Property' union default_variant.

    Finds any 'Object Template' rstruct at any level and patches every
    FieldValue union named 'Property' (keyed on 'Form Type') inside OBTS
    so that the decoder picks the record-specific property enum when 'Form
    Type' is absent from the decode context.
    """
    for m in members:
        if not isinstance(m, dict):
            continue
        if m.get("kind") == "rstruct" and m.get("name") == "Object Template":
            _patch_property_union(m, default_variant)
        else:
            _fixup_obts_property_default(m.get("members", []), default_variant)
            elem = m.get("element")
            if isinstance(elem, dict):
                _fixup_obts_property_default([elem], default_variant)


def _patch_property_union(node: dict, dv: int) -> None:
    """Recursively find and patch every 'Property' FieldValue union in node."""
    for key in ("members", "fields", "variants"):
        for child in node.get(key, []):
            if isinstance(child, dict):
                if (child.get("kind") == "union"
                        and child.get("name") == "Property"
                        and isinstance(child.get("decider"), dict)
                        and "field" in child["decider"]):
                    child["decider"]["default_variant"] = dv
                _patch_property_union(child, dv)
    elem = node.get("element")
    if isinstance(elem, dict):
        _patch_property_union(elem, dv)


def _descend(node: dict, step: str) -> dict:
    """Descend one step of a record_patches path: a member's sig/name, or the
    literal 'element' to enter an array/rarray's element node."""
    if step == "element":
        if "element" not in node:
            raise ValueError(f"patch step 'element': node has no element: {node.get('name')}")
        return node["element"]
    for child in node.get("members", node.get("fields", [])):
        if child.get("sig") == step or child.get("name") == step:
            return child
    raise ValueError(f"patch step {step!r} not found under {node.get('name')}")


def _apply_patch(record: dict, path: list[str], new_node: dict) -> None:
    """Splice new_node into record at path (record_patches merge mode).

    Each path step names a child by sig/name, or is the literal 'element' to
    enter an array/rarray's element. The last step is replaced in place;
    everything else in the record is left untouched.
    """
    cur = record
    for step in path[:-1]:
        cur = _descend(cur, step)
    last = path[-1]
    if last == "element":
        if "element" not in cur:
            raise ValueError("patch target 'element' on non-array node")
        cur["element"] = new_node
        return
    lst = cur.get("members") or cur.get("fields")
    if lst is None:
        raise ValueError(f"patch target {last!r}: parent has no members/fields")
    for i, child in enumerate(lst):
        if child.get("sig") == last or child.get("name") == last:
            lst[i] = new_node
            return
    raise ValueError(f"patch target {last!r} not found")


def main() -> None:
    import argparse as _argparse
    import os as _os

    ap = _argparse.ArgumentParser(
        description="Extract FO76 record schemas from xEdit Pascal → schema/fo76.json"
    )
    ap.add_argument(
        "--strict",
        action="store_true",
        default=_os.environ.get("EXTRACT_STRICT") == "1",
        help="Fail on unexpected extraction warnings "
             "(also enabled by EXTRACT_STRICT=1 env var)",
    )
    args = ap.parse_args()

    if not FO76_PAS.exists():
        print(f"Missing {FO76_PAS}", file=sys.stderr)
        sys.exit(1)
    ex = Extractor(read_text(FO76_PAS), read_text(COMMON_PAS) if COMMON_PAS.exists() else "")
    ex.report.strict = args.strict
    schema = ex.run()

    # Merge overrides — hand-authored fixes that survive regeneration.
    # Three mechanisms:
    #   "records"          — whole-record replacement (wins over extractor output).
    #   "record_patches"   — path-addressed splice: replaces one nested node inside
    #                        the extractor-generated record (e.g. an array's element)
    #                        without touching the rest of the record. Use when the
    #                        extractor gets most of a record right but one nested
    #                        node resists static extraction (a HARD_RAW_VAR).
    #   "record_additions" — member-append: members are appended to the extractor-
    #                        generated record's members list without replacing it.
    #                        Use for genuine xEdit gaps (subrecords absent from Pascal).
    if OVERRIDES.exists():
        try:
            overrides = json.loads(OVERRIDES.read_text(encoding="utf-8"))
            merged = 0
            for sig, rec in overrides.get("records", {}).items():
                schema["records"][sig] = rec
                merged += 1
            patches = 0
            for sig, patch_list in overrides.get("record_patches", {}).items():
                if sig not in schema["records"]:
                    raise ValueError(f"record_patches: no extractor record for {sig} to patch")
                for patch in patch_list:
                    _apply_patch(schema["records"][sig], patch["path"], patch["node"])
                    patches += 1
            additions = 0
            for sig, extra_members in overrides.get("record_additions", {}).items():
                expanded = [
                    _expand_pascal_var(m, ex) if isinstance(m, dict) else m
                    for m in extra_members
                ]
                if sig in schema["records"]:
                    schema["records"][sig]["members"].extend(expanded)
                    additions += len(expanded)
                else:
                    # No generated record to append to — treat as a full record.
                    schema["records"][sig] = {"name": sig, "members": expanded}
                    merged += 1
            print(
                f"merged {merged} override(s), {patches} patch(es), "
                f"{additions} member addition(s) from {OVERRIDES.name}",
                file=sys.stderr,
            )
        except Exception as e:
            # Overrides are load-bearing (e.g. the hand-authored PERK Effect patch).
            # A failure here means the shipped schema is missing critical members.
            print(f"ERROR: failed to load overrides: {e}", file=sys.stderr)
            sys.exit(1)

    # Re-apply schema-kind conversion over the merged tree: override-sourced
    # members (record replacements, patches, additions) bypass the pass that
    # ran inside Extractor.run(), and CTDA structs must not reach the decoder.
    for rec in schema["records"].values():
        _apply_schema_kinds(rec.get("members", []))

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(schema, indent=2), encoding="utf-8")
    print(f"wrote {OUT}", file=sys.stderr)

    ctda = emit_ctda_table(read_text(FO76_PAS))
    CTDA_OUT.write_text(json.dumps(ctda, indent=2), encoding="utf-8")
    print(f"wrote {CTDA_OUT} ({len(ctda['functions'])} functions)", file=sys.stderr)


if __name__ == "__main__":
    main()
