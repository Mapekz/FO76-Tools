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

WHITELIST = ["AMMO", "ARMO", "PROJ", "EXPL", "WEAP", "SPEL", "MGEF", "PERK", "GLOB"]

# Vars that use runtime Pascal deciders — emit raw fallback at subrecord level.
HARD_RAW_VARS = {
    "wbConditions",
    "wbEffectsReq",
    "wbEffect",
    "wbPerkEffect",
    "wbPERKData",
    "wbVMAD",
    "wbVMADFragmentedPERK",
    "wbVMADFragmentedPACK",
    "wbVMADFragmentedQUST",
    "wbVMADFragmentedSCEN",
    "wbVMADFragmentedINFO",
    "wbObjectTemplate",
    "wbMagicEffectSounds",
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
            if c == quote and (i == 0 or s[i - 1] != "\\"):
                in_str = False
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
            if c == quote and (i == 0 or s[i - 1] != "\\"):
                in_str = False
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
    for c in text:
        if in_str:
            cur.append(c)
            if c == quote:
                in_str = False
            continue
        if c in ("'", '"'):
            in_str = True
            quote = c
            cur.append(c)
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
            continue
        cur.append(c)
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
    for m in re.finditer(r"(Int64\((\d+)\)|\$[0-9A-Fa-f]+|0x[0-9A-Fa-f]+|\d+)\s*,\s*'([^']*)'", text):
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
        inner = arg[len("wbFlags(") : find_matching_paren(arg, len("wbFlags("))]
        return {"flags": parse_flags_list(inner)}
    if arg.startswith("wbEnum("):
        inner = arg[len("wbEnum(") : find_matching_paren(arg, len("wbEnum("))]
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
            "wbByteArray(MODF, 'Model Flags', 0)])"
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
            if fn in self.vars:
                return self.vars[fn]
            return expr
        return expr

    def parse_member(self, expr: str) -> dict | None:
        expr = self.expand_call(expr)
        if expr in HARD_RAW_VARS:
            return {
                "kind": "raw_fallback",
                "name": expr,
                "reason": "runtime Pascal decider",
            }
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

        # wbStruct / wbStructSK
        for prefix in ("wbStructSK", "wbStruct"):
            if expr.startswith(prefix + "("):
                return self._parse_struct(expr, prefix)
        if expr.startswith("wbRStruct") or expr.startswith("wbRStructSK"):
            return self._parse_rstruct(expr)
        if expr.startswith("wbRArray") or expr.startswith("wbRArrayS"):
            return self._parse_rarray(expr)
        if expr.startswith("wbArrayS") or expr.startswith("wbArray"):
            return self._parse_array(expr)
        if expr.startswith("wbUnion"):
            return self._parse_union(expr)
        if expr.startswith("wbInteger"):
            return self._parse_integer(expr)
        if expr.startswith("wbFloat"):
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
        if re.fullmatch(r"[A-Z0-9_]{4}", expr):
            return self._parse_sig_ref(expr)
        return None

    def _parse_struct(self, expr: str, prefix: str) -> dict:
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
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            if len(parts) > 1 and parts[1].strip().startswith("'"):
                name = unquote(parts[1])
        elif parts[0].strip().startswith("'"):
            name = unquote(parts[0])
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
        members_expr = parts[-1]
        if members_expr.strip().startswith("["):
            fb = 0
        else:
            fb = members_expr.index("[")
        fe = find_matching_bracket(members_expr, fb if members_expr[fb] == "[" else members_expr.index("["))
        start = members_expr.index("[")
        fe = find_matching_bracket(members_expr, start)
        members = self._parse_member_list(members_expr[start + 1 : fe])
        return {"kind": "rstruct", "name": name, "members": members}

    def _parse_rarray(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        args = split_top_level(expr[lparen + 1 : rparen])
        name = unquote(args[0]) if args[0].strip().startswith("'") else args[0]
        elem = self.parse_member(args[-1])
        return {"kind": "rarray", "name": name, "element": elem or {"kind": "unknown", "name": "element"}}

    def _parse_array(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = None
        name = ""
        elem_expr = parts[-1]
        if sig_id(parts[0].strip()):
            sig = parts[0].strip()
            name = unquote(parts[1]) if len(parts) > 2 else ""
        else:
            name = unquote(parts[0])
        elem = self.parse_member(elem_expr)
        out: dict = {
            "kind": "array",
            "name": name,
            "element": elem or {"kind": "unknown", "name": "element"},
        }
        if sig:
            out["sig"] = sig
        return out

    def _parse_union(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        name = unquote(parts[0]) if parts[0].strip().startswith("'") else parts[0]
        decider_expr = parts[1]
        decider: dict
        if "wbFormVersionDecider" in decider_expr:
            m = re.search(r"wbFormVersionDecider\((\d+)(?:\s*,\s*(\d+))?\)", decider_expr)
            if m:
                decider = {
                    "form_version": {
                        "min": int(m.group(1)),
                        "max": int(m.group(2)) if m.group(2) else None,
                    }
                }
            else:
                decider = {"raw": True}
        elif "Decider" in decider_expr or "wbCondition" in decider_expr:
            decider = {"raw": True}
        else:
            decider = {"raw": True}
        variants_expr = parts[2]
        vb = variants_expr.index("[")
        ve = find_matching_bracket(variants_expr, vb)
        variants = self._parse_member_list(variants_expr[vb + 1 : ve])
        if decider.get("raw"):
            return {
                "kind": "raw_fallback",
                "name": name or "union",
                "reason": "closure union decider",
            }
        return {"kind": "union", "name": name, "decider": decider, "variants": variants}

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
        itype = parts[idx + 1].strip()
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
            fmt = parse_format_arg(parts[idx + 2])
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
        name = unquote(parts[idx])
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
        name = unquote(parts[1])
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
        name = unquote(parts[1])
        return {"kind": "byte_rgba", "sig": sig, "name": name}

    def _parse_empty(self, expr: str) -> dict:
        lparen = expr.index("(")
        rparen = find_matching_paren(expr, lparen)
        parts = split_top_level(expr[lparen + 1 : rparen])
        sig = parts[0].strip()
        name = unquote(parts[1])
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

    def _parse_sig_ref(self, sig: str) -> dict:
        return {"kind": "unknown", "sig": sig, "name": sig}

    def _parse_member_list(self, text: str) -> list[dict]:
        items = split_top_level(text)
        out: list[dict] = []
        for item in items:
            item = item.strip()
            if not item or item.startswith("//"):
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
            rec = self.extract_record(sig)
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
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(schema, indent=2), encoding="utf-8")
    print(f"wrote {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
