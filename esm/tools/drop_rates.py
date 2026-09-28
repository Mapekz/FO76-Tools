#!/usr/bin/env python3
"""
Calculate exact on-paper drop rates for Fallout 76 leveled lists (LVLI).

Usage: python drop_rates.py <form_id> [--level N] [--esm PATH]

Form ID formats: 0x008CD162, 008CD162, 8993138 (decimal)
"""

import json
import os
import shutil
import subprocess
import sys
from collections import OrderedDict, defaultdict
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent


def _resolve_esm_exe():
    name = "esm.exe" if os.name == "nt" else "esm"
    release = WORKSPACE_ROOT / "target" / "release" / name
    if release.is_file() and os.access(release, os.X_OK):
        return str(release)
    found = shutil.which("esm")
    if found:
        return found
    raise SystemExit("Cannot find esm binary. Build it or add it to PATH.")


def _resolve_esm_path():
    p = os.environ.get("FO76_ESM_PATH")
    if not p:
        raise SystemExit("FO76_ESM_PATH not set.")
    path = Path(p)
    if path.is_file():
        return str(path)
    if path.is_dir():
        candidate = path / "SeventySix.esm"
        if candidate.is_file():
            return str(candidate)
    raise SystemExit(f"FO76_ESM_PATH: {p} is not a valid ESM file or data directory.")


ESM_EXE = _resolve_esm_exe()
DEFAULT_ESM_PATH = _resolve_esm_path()
DEFAULT_LEVEL = 50


def esm_get(form_ids, esm_path):
    if not form_ids:
        return []
    cmd = [ESM_EXE, "-p", "--esm", esm_path, "get"]
    cmd += [f"0x{fid:08X}" for fid in form_ids]
    cmd += ["--json"]
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if result.returncode != 0:
        print(f"  esm error: {result.stderr.strip()}", file=sys.stderr)
        return []
    try:
        data = json.loads(result.stdout)
    except json.JSONDecodeError as e:
        print(f"  JSON parse error: {e}", file=sys.stderr)
        return []
    return data if isinstance(data, list) else [data]


def parse_record(record):
    sig = record["header"]["signature"]
    form_id = int(record["header"]["form_id"], 16)
    editor_id = record["editor_id"]
    fields = record.get("fields", {})

    if sig == "LVLI":
        flags = fields.get("Flags", {}).get("flags", [])
        entries = []
        for ew in fields.get("Leveled List Entries", []):
            e = ew["Leveled List Entry"]
            base_data = e.get("Base Data")

            if base_data:
                ref_str = base_data.get("Item")
                cnone_global_str = None
                cnone_value = base_data.get("Chance None", 0)
                quantity = base_data.get("Count", 1)
                min_level = base_data.get("Level", 1)
                cnone_curve = None
                min_level_global_str = None
            else:
                ref_str = e.get("Reference")
                cnone_global_str = e.get("Chance None Global")
                if cnone_global_str and cnone_global_str != "0x00000000":
                    pass
                else:
                    cnone_global_str = None
                cnone_value = e.get("Chance None Value", 0)
                quantity = e.get("Quantity", 1)
                min_level = e.get("Minimum Level", 1)
                cnone_curve_field = e.get("Chance None Curve Table")
                cnone_curve = None
                if isinstance(cnone_curve_field, dict):
                    cnone_curve = cnone_curve_field.get("curve", [])
                min_level_global_str = e.get("Minimum Level Global")
                if min_level_global_str and min_level_global_str != "0x00000000":
                    pass
                else:
                    min_level_global_str = None

            if ref_str is None:
                continue

            ref = int(ref_str, 16) if isinstance(ref_str, str) else int(ref_str)

            cnone_global = None
            if cnone_global_str:
                cnone_global = int(cnone_global_str, 16)

            min_level_global = None
            if min_level_global_str:
                min_level_global = int(min_level_global_str, 16)

            conditions = []
            conds_data = e.get("Conditions", {})
            if conds_data:
                for cd in conds_data.get("Conditions", []):
                    cond = cd.get("Condition", {}).get("Condition Data", {})
                    func = cond.get("Function", "")
                    operator = cond.get("Operator", "")
                    comp_val = cond.get("Comparison Value")
                    param1 = cond.get("Parameter 1")
                    conditions.append({
                        "function": func,
                        "operator": operator,
                        "comparison_value": comp_val,
                        "parameter1": param1,
                    })

            entries.append({
                "ref": ref,
                "cnone_value": cnone_value,
                "cnone_global": cnone_global,
                "cnone_curve": cnone_curve,
                "quantity": quantity,
                "min_level": min_level,
                "min_level_global": min_level_global,
                "conditions": conditions,
            })

        list_cnone_global_str = fields.get("Chance None Global")
        list_cnone_global = None
        if list_cnone_global_str and list_cnone_global_str != "0x00000000":
            list_cnone_global = int(list_cnone_global_str, 16)

        max_global_str = fields.get("Max Global")
        max_global = None
        if max_global_str and max_global_str != "0x00000000":
            max_global = int(max_global_str, 16)

        max_curve = None
        max_curve_field = fields.get("Max Curve Table")
        if isinstance(max_curve_field, dict):
            max_curve = max_curve_field.get("curve", [])

        list_cnone_curve = None
        list_cnone_curve_field = fields.get("Chance None Curve Table")
        if isinstance(list_cnone_curve_field, dict):
            list_cnone_curve = list_cnone_curve_field.get("curve", [])

        return {
            "type": "lvli",
            "form_id": form_id,
            "editor_id": editor_id,
            "use_all": "Use All" in flags,
            "calc_each": "Calculate for each item in count" in flags,
            "use_first_match": "Use First Object That Matches All Conditions" in flags,
            "bit0_set": "Calculate from all levels <= player's level" in flags,
            "max_value": fields.get("Max Value", 1),
            "max_global": max_global,
            "max_curve": max_curve,
            "count": fields.get("Count", 1),
            "list_cnone_value": fields.get("Chance None Value", 0),
            "list_cnone_global": list_cnone_global,
            "list_cnone_curve": list_cnone_curve,
            "entries": entries,
        }
    elif sig == "GLOB":
        return {
            "type": "glob",
            "form_id": form_id,
            "editor_id": editor_id,
            "value": fields.get("Value", 0),
        }
    else:
        weight = fields.get("Weight")
        if weight is None:
            data_block = fields.get("Data", {})
            if isinstance(data_block, dict):
                weight = data_block.get("Weight")
        if weight is None:
            armor_data = fields.get("Armor Data", {})
            if isinstance(armor_data, dict):
                weight = armor_data.get("Weight")
        return {
            "type": "item",
            "form_id": form_id,
            "editor_id": editor_id,
            "name": fields.get("Name", editor_id),
            "record_type": sig,
            "weight": weight if weight is not None else 0.0,
        }


def fetch_lvli_tree(root_form_id, esm_path):
    cache = {}
    to_fetch = {root_form_id}
    fetched = set()
    while to_fetch:
        batch = [fid for fid in sorted(to_fetch) if fid not in fetched]
        if not batch:
            break
        records = esm_get(batch, esm_path)
        to_fetch.clear()
        for record in records:
            fid = int(record["header"]["form_id"], 16)
            if fid in fetched:
                continue
            fetched.add(fid)
            parsed = parse_record(record)
            cache[fid] = parsed
            if parsed["type"] == "lvli":
                if parsed.get("list_cnone_global") and parsed["list_cnone_global"] not in fetched:
                    to_fetch.add(parsed["list_cnone_global"])
                if parsed.get("max_global") and parsed["max_global"] not in fetched:
                    to_fetch.add(parsed["max_global"])
                for entry in parsed["entries"]:
                    if entry["ref"] not in fetched:
                        to_fetch.add(entry["ref"])
                    if entry["cnone_global"] and entry["cnone_global"] not in fetched:
                        to_fetch.add(entry["cnone_global"])
                    if entry.get("min_level_global") and entry["min_level_global"] not in fetched:
                        to_fetch.add(entry["min_level_global"])
                    for cond in entry["conditions"]:
                        cv = cond["comparison_value"]
                        if isinstance(cv, str) and cv.startswith("0x"):
                            gid = int(cv, 16)
                            if gid not in fetched:
                                to_fetch.add(gid)
                        p1 = cond["parameter1"]
                        if isinstance(p1, str) and p1.startswith("0x"):
                            gid = int(p1, 16)
                            if gid not in fetched:
                                to_fetch.add(gid)
    return cache


def interpolate_curve(curve_points, x):
    if not curve_points:
        return 0
    first = curve_points[0]
    if x <= first["x"]:
        return first["y"]
    last = curve_points[-1]
    if x >= last["x"]:
        return last["y"]
    for i in range(len(curve_points) - 1):
        curr = curve_points[i]
        nxt = curve_points[i + 1]
        if curr["x"] <= x < nxt["x"]:
            step = nxt["x"] - curr["x"]
            if step == 0:
                return curr["y"]
            return curr["y"] + (nxt["y"] - curr["y"]) * ((x - curr["x"]) / step)
    return last["y"]


def get_entry_cnone(entry, cache, player_level):
    cnone_curve = entry.get("cnone_curve")
    if cnone_curve:
        cnone_global = entry.get("cnone_global")
        if cnone_global is not None and cnone_global in cache:
            glob = cache[cnone_global]
            if glob.get("type") == "glob":
                return interpolate_curve(cnone_curve, glob["value"])
        return interpolate_curve(cnone_curve, player_level)

    if entry["cnone_global"] is not None and entry["cnone_global"] in cache:
        glob = cache[entry["cnone_global"]]
        if glob.get("type") == "glob":
            return glob["value"]
    return entry["cnone_value"]


def get_entry_min_level(entry, cache):
    ml_global = entry.get("min_level_global")
    if ml_global is not None and ml_global in cache:
        glob = cache[ml_global]
        if glob.get("type") == "glob":
            return glob["value"]
    return entry["min_level"]


def get_list_cnone(lvli_data, cache, player_level):
    list_cnone_curve = lvli_data.get("list_cnone_curve")
    if list_cnone_curve:
        cnone_global = lvli_data.get("list_cnone_global")
        if cnone_global is not None and cnone_global in cache:
            glob = cache[cnone_global]
            if glob.get("type") == "glob":
                return interpolate_curve(list_cnone_curve, glob["value"])
        return interpolate_curve(list_cnone_curve, player_level)

    cnone_global = lvli_data.get("list_cnone_global")
    if cnone_global is not None and cnone_global in cache:
        glob = cache[cnone_global]
        if glob.get("type") == "glob":
            return glob["value"]
    return lvli_data.get("list_cnone_value", 0)


def resolve_max_value(lvli_data, cache):
    max_curve = lvli_data.get("max_curve")
    max_global = lvli_data.get("max_global")
    if max_curve and max_global is not None and max_global in cache:
        glob = cache[max_global]
        if glob.get("type") == "glob":
            return int(interpolate_curve(max_curve, glob["value"]))
    return int(lvli_data["max_value"])


def resolve_name(ref, cache):
    record = cache.get(ref)
    if record is None:
        return f"0x{ref:08X}"
    if record["type"] == "item":
        return record.get("name", record["editor_id"])
    return record.get("editor_id", f"0x{ref:08X}")


def get_item_weight(ref, cache):
    record = cache.get(ref)
    if record and record["type"] == "item":
        return record.get("weight", 0.0)
    return 0.0


def resolve_type_tag(ref, cache):
    record = cache.get(ref)
    if record is None:
        return "?"
    if record["type"] == "lvli":
        return "LVLI"
    if record["type"] == "item":
        return record.get("record_type", "?")
    return record.get("type", "?").upper()


def resolve_comparison_value(cv, cache):
    if isinstance(cv, str) and cv.startswith("0x"):
        gid = int(cv, 16)
        glob = cache.get(gid)
        if glob and glob.get("type") == "glob":
            return glob["value"], glob["editor_id"]
        return 0.0, cv
    return float(cv), str(cv)


def cond_probability(cond, cache):
    func = cond["function"]
    operator = cond["operator"]
    cv_raw = cond["comparison_value"]
    param1 = cond.get("parameter1")

    if func == "GetRandomPercent":
        val, label = resolve_comparison_value(cv_raw, cache)
        if operator in ("Greater Than Or Equal To", ">="):
            return max(0.0, (100.0 - val) / 100.0), f">={val:.0f}"
        elif operator in ("Less Than Or Equal To", "<="):
            return max(0.0, (val + 1.0) / 100.0), f"<={val:.0f}"
        return 0.0, f"?{operator}?"

    if func == "GetGlobalValue":
        if isinstance(param1, str) and param1.startswith("0x"):
            gid = int(param1, 16)
            glob = cache.get(gid)
            if glob and glob.get("type") == "glob":
                glob_val = glob["value"]
                target_val = float(cv_raw) if not isinstance(cv_raw, str) else 0.0
                if operator in ("Equal To", "=="):
                    match = abs(glob_val - target_val) < 0.001
                    return 1.0 if match else 0.0, \
                        f"{glob['editor_id']}={glob_val:.0f} {'==' if match else '!='} {target_val:.0f}"
        return 0.0, "GLOB?"

    return 1.0, func


def entry_match_prob(entry, cache):
    conds = entry["conditions"]
    if not conds:
        return 1.0, []
    total = 1.0
    labels = []
    for c in conds:
        p, label = cond_probability(c, cache)
        total *= p
        labels.append(label)
    return total, labels


def filter_entries_by_level(entries, cache, player_level, bit0_set):
    if bit0_set:
        return [(i, e) for i, e in enumerate(entries)
                if get_entry_min_level(e, cache) <= player_level]

    best_level = -1
    for e in entries:
        ml = get_entry_min_level(e, cache)
        if ml <= player_level and ml > best_level:
            best_level = ml
    if best_level < 0:
        return []
    return [(i, e) for i, e in enumerate(entries)
            if get_entry_min_level(e, cache) == best_level]


def calc_tier_probs(lvli_data, cache, player_level):
    entries = lvli_data["entries"]
    max_val = resolve_max_value(lvli_data, cache)
    bit0_set = lvli_data.get("bit0_set", True)

    eligible = filter_entries_by_level(entries, cache, player_level, bit0_set)
    if not eligible:
        return []

    p_pass = [max(0.0, (100.0 - get_entry_cnone(e, cache, player_level)) / 100.0)
              for _, e in eligible]
    p_fail = [1.0 - p for p in p_pass]

    if max_val == 0:
        result = []
        for idx, (orig_i, entry) in enumerate(eligible):
            result.append((orig_i, entry, p_pass[idx]))
        return result

    dp = [0.0] * (max_val + 1)
    dp[0] = 1.0

    result = []
    for idx, (orig_i, entry) in enumerate(eligible):
        prob_selected = p_pass[idx] * sum(dp[:max_val])
        result.append((orig_i, entry, prob_selected))
        new_dp = [0.0] * (max_val + 1)
        for k in range(max_val + 1):
            if dp[k] > 0:
                new_dp[k] += dp[k] * p_fail[idx]
                if k + 1 <= max_val:
                    new_dp[k + 1] += dp[k] * p_pass[idx]
        dp = new_dp

    return result


def calc_lvli(lvli_data, cache, player_level, depth=0, bit0_set=None):
    if depth > 10:
        return OrderedDict(), {}, {}, 1.0

    entries = lvli_data["entries"]
    use_all = lvli_data["use_all"]
    calc_each = lvli_data["calc_each"]
    use_first = lvli_data.get("use_first_match", False)
    count = int(lvli_data["count"])

    if bit0_set is None:
        bit0_set = lvli_data.get("bit0_set", True)

    list_cnone = get_list_cnone(lvli_data, cache, player_level)
    list_self_chance = max(0.0, (100.0 - list_cnone) / 100.0)

    result = defaultdict(float)
    item_weights = {}
    item_fids = {}

    def _eval_sublist(ref, entry_chance, qty=1):
        record = cache.get(ref)
        if record and record["type"] == "lvli":
            sub, sub_w, sub_fids, sub_empty = calc_lvli(record, cache, player_level, depth + 1)
            sub_count = int(record["count"])
            scale = qty if sub_count == 1 else 1
            for (sub_name, sub_qty), sub_expected in sub.items():
                item_key = (sub_name, sub_qty * scale)
                result[item_key] += entry_chance * sub_expected
                item_weights[item_key] = sub_w.get((sub_name, sub_qty), 0.0)
                item_fids[item_key] = sub_fids.get((sub_name, sub_qty), ref)
            return sub_empty
        else:
            name = resolve_name(ref, cache)
            key = (name, qty)
            result[key] += entry_chance
            item_weights[key] = get_item_weight(ref, cache)
            item_fids[key] = ref
            return 0.0

    def _eval_entry_chance(entry):
        conds_match, _ = entry_match_prob(entry, cache)
        entry_cnone = get_entry_cnone(entry, cache, player_level)
        self_chance = (1.0 - entry_cnone / 100.0) * conds_match
        return self_chance

    if use_first:
        eligible = filter_entries_by_level(entries, cache, player_level, bit0_set)
        cum_no_match = 1.0
        for orig_i, entry in eligible:
            p_match, _ = entry_match_prob(entry, cache)
            entry_cnone = get_entry_cnone(entry, cache, player_level)
            p_self = (1.0 - entry_cnone / 100.0) * p_match
            p_selected = p_self * cum_no_match
            cum_no_match *= (1.0 - p_match)

            qty = entry["quantity"]
            _eval_sublist(entry["ref"], p_selected, qty)

    elif use_all:
        eligible = filter_entries_by_level(entries, cache, player_level, bit0_set)
        max_val = resolve_max_value(lvli_data, cache)

        entry_self = [_eval_entry_chance(e) for _, e in eligible]
        sub_empty = [0.0] * len(eligible)
        sub_cache = [None] * len(eligible)
        for idx, (_, entry) in enumerate(eligible):
            record = cache.get(entry["ref"])
            if record and record["type"] == "lvli":
                sub, sub_w, sub_fids, se = calc_lvli(record, cache, player_level, depth + 1)
                sub_empty[idx] = se
                sub_cache[idx] = (sub, sub_w, sub_fids)

        if max_val == 0:
            for idx, (_, entry) in enumerate(eligible):
                qty = entry["quantity"]
                if sub_cache[idx] is not None:
                    sub, sub_w, sub_fids = sub_cache[idx]
                    entry_record = cache.get(entry["ref"])
                    sub_count = int(entry_record["count"]) if entry_record else 1
                    if qty > 1:
                        adj_empty = sub_empty[idx] ** qty
                        eff = entry_self[idx] * (1.0 - adj_empty)
                        for (sub_name, sub_qty), sub_expected in sub.items():
                            display_qty = sub_qty * qty if sub_count == 1 else sub_qty
                            item_key = (sub_name, display_qty)
                            adjusted = 1.0 - (1.0 - sub_expected) ** qty
                            result[item_key] += eff * adjusted
                            item_weights[item_key] = sub_w.get((sub_name, sub_qty), 0.0)
                            item_fids[item_key] = sub_fids.get((sub_name, sub_qty), entry["ref"])
                    else:
                        eff = entry_self[idx] * (1.0 - sub_empty[idx])
                        for item_key, sub_expected in sub.items():
                            result[item_key] += eff * sub_expected
                        item_weights.update(sub_w)
                        item_fids.update(sub_fids)
                else:
                    name = resolve_name(entry["ref"], cache)
                    eff = entry_self[idx]
                    key = (name, qty)
                    result[key] += eff
                    item_weights[key] = get_item_weight(entry["ref"], cache)
                    item_fids[key] = entry["ref"]
        else:
            dp = [0.0] * (max_val + 1)
            dp[0] = 1.0
            for idx, (_, entry) in enumerate(eligible):
                qty = entry["quantity"]
                if sub_cache[idx] is not None:
                    sub, sub_w, sub_fids = sub_cache[idx]
                    entry_record = cache.get(entry["ref"])
                    sub_count = int(entry_record["count"]) if entry_record else 1
                    if qty > 1:
                        adj_empty = sub_empty[idx] ** qty
                        eff = entry_self[idx] * (1.0 - adj_empty)
                        prob_selected = eff * sum(dp[:max_val])
                        for (sub_name, sub_qty), sub_expected in sub.items():
                            display_qty = sub_qty * qty if sub_count == 1 else sub_qty
                            item_key = (sub_name, display_qty)
                            adjusted = 1.0 - (1.0 - sub_expected) ** qty
                            result[item_key] += prob_selected * adjusted
                            item_weights[item_key] = sub_w.get((sub_name, sub_qty), 0.0)
                            item_fids[item_key] = sub_fids.get((sub_name, sub_qty), entry["ref"])
                    else:
                        eff = entry_self[idx] * (1.0 - sub_empty[idx])
                        prob_selected = eff * sum(dp[:max_val])
                        for item_key, sub_expected in sub.items():
                            result[item_key] += prob_selected * sub_expected
                        item_weights.update(sub_w)
                        item_fids.update(sub_fids)
                else:
                    name = resolve_name(entry["ref"], cache)
                    eff = entry_self[idx]
                    prob_selected = eff * sum(dp[:max_val])
                    key = (name, qty)
                    result[key] += prob_selected
                    item_weights[key] = get_item_weight(entry["ref"], cache)
                    item_fids[key] = entry["ref"]
                fail = 1.0 - eff
                new_dp = [0.0] * (max_val + 1)
                for k in range(max_val + 1):
                    if dp[k] > 0:
                        new_dp[k] += dp[k] * fail
                        if k + 1 <= max_val:
                            new_dp[k + 1] += dp[k] * eff
                dp = new_dp

    else:
        eligible = filter_entries_by_level(entries, cache, player_level, bit0_set)
        if not eligible:
            return OrderedDict(), {}, {}, 1.0

        n = len(eligible)
        entry_self = [_eval_entry_chance(e) for _, e in eligible]

        sub_empty = [0.0] * n
        for idx, (_, entry) in enumerate(eligible):
            record = cache.get(entry["ref"])
            if record and record["type"] == "lvli":
                _, _, _, se = calc_lvli(record, cache, player_level, depth + 1)
                sub_empty[idx] = se

        entry_chances = [(1.0 / n) * entry_self[i] * (1.0 - sub_empty[i]) for i in range(n)]
        total_single = sum(entry_chances)

        if calc_each and count > 1:
            from collections import defaultdict as dd
            grouped = dd(lambda: [0.0, 1])
            for idx, (_, entry) in enumerate(eligible):
                key = (entry["ref"], entry["quantity"])
                grouped[key][0] += entry_chances[idx]
                grouped[key][1] = entry["quantity"]
            for (ref, qty), (combined_chance, _) in grouped.items():
                _eval_sublist(ref, combined_chance, qty)
        else:
            for idx, (_, entry) in enumerate(eligible):
                qty = entry["quantity"]
                _eval_sublist(entry["ref"], entry_chances[idx], qty)

        empty_single = max(0.0, 1.0 - total_single)

        raw_sum = sum(result.values())
        for item in result:
            result[item] *= list_self_chance

        full_empty = (1.0 - list_self_chance) + list_self_chance * empty_single

        return (OrderedDict(sorted(result.items(), key=lambda x: -x[1])),
                item_weights, item_fids, full_empty)

    raw_sum = sum(result.values())
    for item in result:
        result[item] *= list_self_chance

    empty = (1.0 - list_self_chance) + list_self_chance * max(0.0, 1.0 - raw_sum)

    return (OrderedDict(sorted(result.items(), key=lambda x: -x[1])),
            item_weights, item_fids, empty)


def print_tree(lvli_data, cache, player_level, indent=0):
    prefix = "  " * indent
    name = lvli_data["editor_id"]
    fid = lvli_data["form_id"]
    print(f"{prefix}{name} (0x{fid:08X})")

    flags = []
    if lvli_data.get("bit0_set", True):
        flags.append("AllLevels")
    else:
        flags.append("ClosestLevel")
    if lvli_data.get("use_first_match"):
        flags.append("FirstMatch")
    if lvli_data["use_all"]:
        flags.append("Use All")
    if lvli_data["calc_each"]:
        flags.append("CalcEach")
    max_val = resolve_max_value(lvli_data, cache)
    list_cnone = get_list_cnone(lvli_data, cache, player_level)
    cnone_str = f" list_cnone={list_cnone:.0f}%" if list_cnone > 0 else ""
    print(f"{prefix}  flags=[{', '.join(flags) if flags else 'none'}] "
          f"max_value={max_val} count={lvli_data['count']:.0f}{cnone_str}")

    for entry in lvli_data["entries"]:
        ref = entry["ref"]
        record = cache.get(ref)
        tag = resolve_type_tag(ref, cache)
        min_lvl = get_entry_min_level(entry, cache)
        entry_cnone = get_entry_cnone(entry, cache, player_level)

        conds = entry["conditions"]
        if conds:
            cond_strs = []
            for c in conds:
                _, label = cond_probability(c, cache)
                cond_strs.append(label)
            cond_tag = " AND ".join(cond_strs)
        else:
            cond_tag = None

        lvl_tag = f" [lvl>={min_lvl:.0f}]" if min_lvl > 1 else ""
        cnone_tag = f" cnone={entry_cnone:.0f}%" if entry_cnone > 0 else ""

        qty = entry["quantity"]
        qty_str = f" x{qty:.0f}" if qty != 1 else ""

        if record and record["type"] == "lvli":
            print(f"{prefix}  [{cond_tag or 'always'}]{lvl_tag}{cnone_tag}{qty_str} -> {tag} (0x{ref:08X}):")
            print_tree(record, cache, player_level, indent + 2)
        else:
            item_name = resolve_name(ref, cache)
            w = record.get("weight", 0) if record else 0
            print(f"{prefix}  [{cond_tag or 'always'}]{lvl_tag}{cnone_tag} {item_name} [{tag}] 0x{ref:08X} wt={w:.1f}{qty_str}")


def main():
    import argparse
    parser = argparse.ArgumentParser(
        description="Calculate exact drop rates for FO76 leveled lists")
    parser.add_argument("form_id",
        help="LVLI form ID (0x008CD162, 008CD162, or 8993138)")
    parser.add_argument("--level", "-l", type=int, default=DEFAULT_LEVEL,
        help=f"Player level for min-level checks (default: {DEFAULT_LEVEL})")
    parser.add_argument("--esm", default=DEFAULT_ESM_PATH,
        help="Path to SeventySix.esm (default: $FO76_ESM_PATH)")
    args = parser.parse_args()

    form_id_str = args.form_id.strip()
    if form_id_str.lower().startswith("0x"):
        form_id = int(form_id_str, 16)
    elif len(form_id_str) == 8 and all(c in "0123456789abcdefABCDEF" for c in form_id_str):
        form_id = int(form_id_str, 16)
    else:
        form_id = int(form_id_str)

    print(f"Fetching record tree for 0x{form_id:08X} ...")
    cache = fetch_lvli_tree(form_id, args.esm)

    if form_id not in cache:
        print(f"Error: record 0x{form_id:08X} not found")
        sys.exit(1)

    lvli = cache[form_id]
    if lvli["type"] != "lvli":
        print(f"Error: 0x{form_id:08X} is {lvli['record_type']}, not LVLI")
        sys.exit(1)

    print()
    print_tree(lvli, cache, args.level)
    print()

    use_first = lvli.get("use_first_match", False)
    if use_first:
        bit0 = lvli.get("bit0_set", True)
        level_mode = "all levels" if bit0 else "closest level"
        print(f"Conditional tier evaluation (first match wins, {level_mode}):")
        eligible = filter_entries_by_level(lvli["entries"], cache, args.level, lvli.get("bit0_set", True))
        cum = 1.0
        for orig_i, entry in eligible:
            p_match, labels = entry_match_prob(entry, cache)
            entry_cnone = get_entry_cnone(entry, cache, args.level)
            p_self = (1.0 - entry_cnone / 100.0) * p_match
            p_selected = p_self * cum
            cum *= (1.0 - p_match)
            name = resolve_name(entry["ref"], cache)
            conds_strs = []
            if entry_cnone > 0:
                conds_strs.append(f"cnone={entry_cnone:.0f}%")
            conds_strs.extend(labels if labels else ["always"])
            conds = " AND ".join(conds_strs)
            print(f"  {orig_i+1}. {name}: {p_selected*100:.1f}%  [{conds}]")
        print()
    elif lvli["use_all"]:
        tier_probs = calc_tier_probs(lvli, cache, args.level)
        max_val = resolve_max_value(lvli, cache)
        bit0 = lvli.get("bit0_set", True)
        level_mode = "all levels" if bit0 else "closest level"
        print(f"Tier activation (Use All, top-to-bottom, Max Value={max_val}, {level_mode}):")
        for orig_i, entry, prob in tier_probs:
            name = resolve_name(entry["ref"], cache)
            print(f"  {orig_i+1}. {name}: {prob*100:.1f}%")
        print()

    result, item_weights, item_fids, empty = calc_lvli(lvli, cache, args.level)

    print("Drop rates:")
    print("-" * 74)
    print(f"  {'Item':<30s} {'FormID':>10s} {'wt':>5s} {'%':>6s}  ")
    print("-" * 74)
    for (name, qty), expected in result.items():
        pct = expected * 100
        w = item_weights.get((name, qty), 0.0)
        fid = item_fids.get((name, qty), 0)
        bar = "#" * max(1, min(50, int(pct / 2))) if expected > 0 else ""
        qty_str = f" x{int(qty)}" if qty != 1 else ""
        print(f"  {name + qty_str:<30s} 0x{fid:08X} {w:>5.1f} {pct:>5.1f}%  {bar}")
    print("-" * 74)
    if empty > 0.001:
        print(f"  {'NOTHING':<30s} {'':>10s} {'':>5s} {empty*100:>5.1f}%")


if __name__ == "__main__":
    main()
