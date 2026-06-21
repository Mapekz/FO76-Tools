#!/usr/bin/env python3
"""Extract FO76 record schemas from xEdit Pascal definitions → schema/fo76.json."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TES5 = ROOT.parent / "TES5Edit"
FO76_PAS = TES5 / "Core" / "wbDefinitionsFO76.pas"
COMMON_PAS = TES5 / "Core" / "wbDefinitionsCommon.pas"
OUT = ROOT / "schema" / "fo76.json"
OVERRIDES = ROOT / "schema" / "fo76.overrides.json"

# All 168 record types present in SeventySix.esm (excluding CELL and WRLD).
# Generated from: fo76 tree SeventySix.esm | jq '[.[].label.sig] | unique | sort | .[]'
WHITELIST = [
    "AACT", "AAMD", "AAPD", "ACTI", "ADDN", "AECH", "ALCH", "AMDL", "AMMO",
    "ANIO", "AORU", "ARMA", "ARMO", "ARTO", "ASPC", "ASTM", "ASTP", "ATXO",
    "AUVF", "AVIF", "AVTR", "BNDS", "BOOK", "BPTD", "CAMS", "CHAL", "CLAS",
    "CLFM", "CLMT", "CMPO", "CMPT", "CNCY", "CNDF", "COBJ", "COEN", "COLL",
    "CONT", "CPRD", "CPTH", "CSEN", "CSTY", "CURV", "DCGF", "DEBR", "DFOB",
    "DIST", "DLVW", "DMGT", "DOBJ", "DOOR", "ECAT", "EFSH", "EMOT", "ENCH",
    "ENTM", "EQUP", "EXPL", "FACT", "FISH", "FLOR", "FLST", "FSTP", "FSTS",
    "FURN", "GCVR", "GDRY", "GLOB", "GMRW", "GMST", "GRAS", "HAZD", "HDPT",
    "IDLE", "IDLM", "IMAD", "IMGS", "INGR", "INNR", "IPCT", "IPDS", "KEYM",
    "KSSM", "KYWD", "LAYR", "LCRT", "LCTN", "LENS", "LGDI", "LGTM", "LIGH",
    "LOUT", "LSCR", "LTEX", "LVLI", "LVLN", "LVLP", "LVPC", "MATO", "MATT",
    "MDSP", "MESG", "MGEF", "MISC", "MOVT", "MSTT", "MSWP", "MUSC", "MUST",
    "NAVI", "NOCM", "NOTE", "NPC_", "OMOD", "OTFT", "OVIS", "PACH", "PACK",
    "PCRD", "PEPF", "PERK", "PKIN", "PLYT", "PMFT", "PPAK", "PROJ", "QMDL",
    "QUST", "RACE", "REGN", "RELA", "RESO", "REVB", "RFCT", "RFGP", "SCCO",
    "SCOL", "SCSN", "SECH", "SMBN", "SMEN", "SMQN", "SNCT", "SNDR", "SOPM",
    "SOUN", "SPEL", "SPGD", "STAG", "STAT", "STHD", "STMP", "STND", "TACT",
    "TEPF", "TERM", "TRAP", "TREE", "TRNS", "TXST", "UTIL", "VOLI", "VTYP",
    "WATR", "WAVE", "WEAP", "WSPR", "WTHR", "ZOOM",
]

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
# VMAD, conditions, effects, and object templates are REMOVED from this list
# because they are now modeled via injected helpers or the vmad decoder.
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


def parse_flags_list(text: str) -> list[str]:
    names: list[str] = []
    for m in re.finditer(r"'([^']*)'", text):
        names.append(m.group(1))
    return names


def parse_enum_dense(text: str) -> list[str]:
    names: list[str] = []
    for m in re.finditer(r"(?:\{\d+\}\s*)?'([^']*)'", text):
        names.append(m.group(1))
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
    token = token.strip()
    if re.fullmatch(r"[A-Z0-9_]{4}", token):
        return token
    return None


class Extractor:
    def __init__(self, fo76: str, common: str):
        self.fo76 = fo76
        self.common = common
        self.vars: dict[str, str] = {}
        self._collect_vars(fo76)
        # Skip common.pas — helpers are injected below.
        self._inject_builtin_helpers()

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
        self.vars["wbDEST"] = (
            "wbStruct(DEST, 'Destructible', ["
            "wbInteger('Health', itU32), wbInteger('Count', itU8),"
            "wbFormIDCk('Explosion', [EXPL,NULL]), wbFormIDCk('Debris', [DEBR,NULL])])"
        )
        self.vars["wbEnchantment"] = "wbFormIDCk(EITM, 'Enchantment', [ENCH,NULL])"
        self.vars["wbModelInfo"] = "wbByteArray(MODT, 'Model Information', 0)"
        self.vars["wbDamageTypeArray"] = (
            "wbArrayS(DAMA, 'Resistances', wbStructSK([0], 'Resistance', ["
            "wbFormIDCk('Type', [DMGT]), wbInteger('Amount', itU32)]))"
        )
        self.vars["wbPTRN"] = "wbFormIDCk(PTRN, 'Preview Transform', [TRNS,NULL])"
        self.vars["wbPHST"] = "wbByteArray(PHST, 'Photo Studio', 0)"
        self.vars["wbSNTP"] = "wbFormIDCk(SNTP, 'Snap Template', [STMP])"
        self.vars["wbXALG"] = "wbInteger(XALG, 'Flags', itU32)"
        self.vars["wbFTAGs"] = "wbRArray('Form Tags', wbString(FTAG, 'Form Tag'))"
        self.vars["wbBOD2"] = (
            "wbStruct(BOD2, 'Body Part Data', ["
            "wbInteger('Armor Type', itU8), wbInteger('First Person Flags', itU8)])"
        )
        self.vars["wbETYP"] = "wbFormIDCk(ETYP, 'Equipment Type', [EQUP,NULL])"
        self.vars["wbYNAM"] = "wbFormIDCk(YNAM, 'Sound - Pickup', [SNDR,NULL])"
        self.vars["wbZNAM"] = "wbFormIDCk(ZNAM, 'Sound - Putdown', [SNDR,NULL])"
        self.vars["wbVCRY"] = "wbByteArray(VCRY, 'Voice Category', 0)"
        self.vars["wbICON"] = "wbString(ICON, 'Icon Image')"
        self.vars["wbMICO"] = "wbString(MICO, 'Message Icon')"
        self.vars["wbINRD"] = "wbFormIDCk(INRD, 'Instance Naming', [INNR])"
        self.vars["wbEILV"] = "wbByteArray(EILV, 'EILV', 0)"
        self.vars["wbIBSD"] = "wbByteArray(IBSD, 'IBSD', 0)"
        self.vars["wbAPPR"] = "wbByteArray(APPR, 'Appearance', 0)"
        self.vars["wbMDOB"] = "wbByteArray(MDOB, 'Menu Display Object', 0)"
        self.vars["wbMIID"] = "wbByteArray(MIID, 'Menu Item ID', 0)"
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
        # Comparison value and parameters use bytes (no per-function typing).
        # ----------------------------------------------------------------
        self.vars["wbConditions"] = (
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
        )

        # ----------------------------------------------------------------
        # Effects — rstruct wrapping EFID + EFIT (bytes) + optional fields.
        # wbConditions is already modeled above.
        # ----------------------------------------------------------------
        self.vars.setdefault("wbEFID", "wbFormIDCk(EFID,'Base Effect',[MGEF])")
        # EFIT layout is version-conditional; use bytes for structural fidelity.
        self.vars.setdefault("wbEFIT", "wbByteArray(EFIT,'Effect Item Data',0)")
        self.vars["wbEffect"] = (
            "wbRStruct('Effect',["
            "wbFormIDCk(EFID,'Base Effect',[MGEF]),"
            "wbByteArray(EFIT,'Effect Item Data',0),"
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

        # ----------------------------------------------------------------
        # Object Template — rstruct; OBTS internals modeled as bytes to
        # avoid the count-path complexity inside the struct's arrays.
        # ----------------------------------------------------------------
        self.vars["wbOBTSReq"] = "wbByteArray(OBTS,'Object Mod Template Item',0)"
        self.vars["wbObjectTemplate"] = (
            "wbRStruct('Object Template',["
            "wbInteger(OBTE,'Count',itU32),"
            "wbRArray('Combinations',wbRStruct('Combination',["
            "wbEmpty(OBTF,'Editor Only'),"
            "wbLStringKC(FULL,'Name',0,cpTranslate),"
            "wbByteArray(OBTS,'Object Mod Template Item',0)"
            "])),"
            "wbEmpty(STOP,'Marker')"
            "])"
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
        bare_m = re.match(r"^(wb[A-Za-z0-9_]+)\.", expr)
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
                return (
                    f"wbArrayS(DAMA, '{name}s', wbStructSK([0], '{name}', ["
                    f"wbFormIDCk('Type', [DMGT]), wbInteger('Amount', itU32)]))"
                )
            if fn == "wbModelInfo":
                parts = split_top_level(args)
                sig = parts[0].strip() if parts else "MODT"
                return f"wbByteArray({sig}, 'Model Information', 0)"
            # wbVec3PosRot(SIG) → bytes (24 bytes = pos xyz + rot xyz)
            if fn == "wbVec3PosRot":
                parts = split_top_level(args)
                sig2 = parts[0].strip() if parts else "DATA"
                return f"wbByteArray({sig2}, 'Position/Rotation', 24)"
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
            if fn in self.vars:
                return self.vars[fn]
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

    def parse_member(self, expr: str) -> dict | None:
        expr = self.expand_call(expr)
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
                child = self.parse_member(m.group(2))
                if child:
                    child["from_version"] = int(m.group(1))
                return child
        if expr.startswith("__below_version__"):
            m = re.match(r"__below_version__\((\d+),\s*(.+)\)\s*$", expr, re.DOTALL)
            if m:
                child = self.parse_member(m.group(2))
                if child:
                    child["below_version"] = int(m.group(1))
                return child

        # wbStruct / wbStructSK / wbStructExSK (ExSK has extra leading [exclude] arg)
        for prefix in ("wbStructExSK", "wbStructSK", "wbStruct"):
            if expr.startswith(prefix + "("):
                return self._parse_struct(expr)
        if expr.startswith("wbRStruct") or expr.startswith("wbRStructSK"):
            return self._parse_rstruct(expr)
        if expr.startswith("wbRArray") or expr.startswith("wbRArrayS"):
            return self._parse_rarray(expr)
        if expr.startswith("wbArrayS") or expr.startswith("wbArray"):
            result = self._parse_array(expr)
            if count_path and isinstance(result, dict) and result.get("kind") == "array":
                result["count"] = {"count_path": count_path}
            return result
        if expr.startswith("wbUnion"):
            return self._parse_union(expr)
        if expr.startswith("wbInteger"):
            return self._parse_integer(expr)
        if expr.startswith("wbFloat") and "(" in expr:
            return self._parse_float(expr)
        if expr.startswith("wbFormIDCk") or expr.startswith("wbFormIDCK"):
            return self._parse_formid(expr)
        if expr.startswith("wbFormID"):
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
        return None

    def _parse_struct(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        args = expr[lparen + 1 : rparen]
        parts = split_top_level(args)
        sig = None
        name = ""
        fields_part = parts[-1]
        if not fields_part.strip().startswith("["):
            for p in parts:
                if p.strip().startswith("["):
                    fields_part = p
                    break
        # Determine sig/name: skip leading [bracket] args (SK sort keys, ExSK exclude keys).
        # Then the first sig-like token is the sig, and the next quoted string is the name.
        idx = 0
        while idx < len(parts) and parts[idx].strip().startswith("[") and not sig_id(parts[idx].strip()):
            idx += 1
        if idx < len(parts) and sig_id(parts[idx].strip()):
            sig = parts[idx].strip()
            if idx + 1 < len(parts) and parts[idx + 1].strip().startswith("'"):
                name = unquote(parts[idx + 1])
        elif idx < len(parts) and parts[idx].strip().startswith("'"):
            name = unquote(parts[idx])
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
        # skip summary keys for SK variant
        idx = 0
        if parts[0].strip().startswith("["):
            idx = 1
        name = unquote(parts[idx]) if parts[idx].strip().startswith("'") else parts[idx]
        # wbRStruct/wbRStructSK can have trailing args after the member list
        # (e.g., [], cpNormal, False, nil, True).  Mirror _parse_struct: take the
        # LAST arg as a first guess and fall back to the first arg that contains '['.
        members_expr = parts[-1]
        if not members_expr.strip().startswith("["):
            for p in parts:
                if p.strip().startswith("["):
                    members_expr = p
                    break
        if "[" not in members_expr:
            return {"kind": "raw_fallback", "name": name or "rstruct", "reason": "rstruct variable ref"}
        start = members_expr.index("[")
        fe = find_matching_bracket(members_expr, start)
        members = self._parse_member_list(members_expr[start + 1 : fe])
        return {"kind": "rstruct", "name": name, "members": members}

    def _parse_rarray(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        args = split_top_level(expr[lparen + 1 : rparen])
        name = unquote(args[0]) if args[0].strip().startswith("'") else args[0]
        # FIX: element is always args[1] (name is args[0]).
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
            # FIX: element is at index 2 when leading sig is present.
            elem_idx = 2
        else:
            name = unquote(parts[0])
            # FIX: element is at index 1 when no leading sig.
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
        # Capture count argument: -1 → count_prefix (4-byte inline prefix in data).
        count_arg_idx = elem_idx + 1
        if count_arg_idx < len(parts):
            count_str = parts[count_arg_idx].strip()
            if count_str == "-1":
                out["count"] = "count_prefix"
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
                    else:
                        # Decider-only: use the subst dict AS the decider,
                        # and fall through to parse variants from Pascal.
                        decider = dict(subst)
                    break
        else:
            decider = {"raw": True}
        if decider.get("raw"):
            return {
                "kind": "raw_fallback",
                "name": name or sig or "union",
                "reason": "closure union decider",
            }
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
        itype = parts[idx + 1].strip() if len(parts) > idx + 1 else "itU32"
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
        if ck and len(parts) > idx + 1 and "[" in parts[idx + 1]:
            refs = re.findall(r"[A-Z0-9_]{4}", parts[idx + 1])
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
        sig = parts[0].strip()
        name = unquote(parts[1]) if len(parts) > 1 else sig
        return {"kind": "empty", "sig": sig, "name": name}

    def _parse_unknown(self, expr: str) -> dict:
        m = re.match(r"wbUnknown\((.*)\)", expr)
        sig = None
        name = "Unknown"
        if m:
            tok = m.group(1).strip()
            if sig_id(tok):
                sig = tok
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
        """Return the first 4-char subrecord signature found in a parsed member dict.

        Used by `_parse_runion` to determine the discriminant signature for each
        wbRUnion variant (PresentSignature decider).
        """
        if member is None:
            return None
        sig = member.get("sig")
        if sig and re.fullmatch(r"[A-Z0-9_]{4}", sig):
            return sig
        # Recurse into rstruct/struct members, union variants, and array elements.
        for child in member.get("members", []):
            s = self._extract_anchor_sig(child)
            if s:
                return s
        for child in member.get("fields", []):
            s = self._extract_anchor_sig(child)
            if s:
                return s
        for child in member.get("variants", []):
            s = self._extract_anchor_sig(child)
            if s:
                return s
        elem = member.get("element")
        if elem:
            s = self._extract_anchor_sig(elem)
            if s:
                return s
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
            # Build PresentSignature from the first sig-bearing member of each variant.
            anchors = [self._extract_anchor_sig(v) or "" for v in variants]
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
            if m and m.group(1) in self.vars:
                item = self.vars[m.group(1)]
            parsed = self.parse_member(item)
            if parsed:
                out.append(parsed)
        return out

    def extract_record(self, sig: str) -> dict | None:
        pattern = rf"\bwbRecord\s*\(\s*{sig}\s*,"
        m = re.search(pattern, self.fo76)
        if not m:
            return None
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
                print(f"WARNING: failed {sig}: {e}", file=sys.stderr)
                rec = None
            if rec:
                records[sig] = rec
                print(f"extracted {sig}: {len(rec['members'])} members", file=sys.stderr)
            else:
                print(f"WARNING: missing {sig}", file=sys.stderr)
        return {"records": records}


def main() -> None:
    if not FO76_PAS.exists():
        print(f"Missing {FO76_PAS}", file=sys.stderr)
        sys.exit(1)
    ex = Extractor(read_text(FO76_PAS), read_text(COMMON_PAS) if COMMON_PAS.exists() else "")
    schema = ex.run()

    # Merge overrides — hand-authored fixes that survive regeneration.
    # Override wins at record-key level: the entire record entry is replaced.
    if OVERRIDES.exists():
        try:
            overrides = json.loads(OVERRIDES.read_text(encoding="utf-8"))
            merged = 0
            for sig, rec in overrides.get("records", {}).items():
                schema["records"][sig] = rec
                merged += 1
            print(f"merged {merged} override(s) from {OVERRIDES.name}", file=sys.stderr)
        except Exception as e:
            print(f"WARNING: failed to load overrides: {e}", file=sys.stderr)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(schema, indent=2), encoding="utf-8")
    print(f"wrote {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
