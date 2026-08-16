#!/usr/bin/env python3
"""
curvelookup.py — Fallout 76 PTS curve table lookup tool.

Look up damage, HP, resist, or XP values for creatures or players
at a given level and tier, with linear interpolation between curve points.

Usage:
    python curvelookup.py [options]   # omit any arg to be prompted (wizard mode)

Examples:
    python curvelookup.py -t creature -s damage -l 100 -T 30
    python curvelookup.py --type player --stat damage --level 36 --tier 30
    python curvelookup.py --tier 30 --level 36 --type player --stat damage
    python curvelookup.py -t creature -s hp -l 50 -T 10 --all-tiers
    python curvelookup.py --patch 20260803 -t creature -s damage -l 50 -T 20
    python curvelookup.py           # full wizard: prompts for everything
"""

import argparse
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path

# ---------------------------------------------------------------------------
# Configuration: which stats are valid for each character type,
# and the directory / file-prefix / tier range for each combo.
# ---------------------------------------------------------------------------

SCRIPT_DIR = Path(__file__).parent


@dataclass(frozen=True)
class StatConfig:
    subdir: str
    prefix: str
    tier_min: int
    tier_max: int


STAT_CONFIG: dict[str, dict[str, StatConfig]] = {
    "creature": {
        "damage": StatConfig("creatures/weapon", "damage", tier_min=1, tier_max=50),
        "hp": StatConfig("creatures/health", "health", tier_min=1, tier_max=60),
        "resist": StatConfig("creatures/armor", "armor", tier_min=1, tier_max=50),
        # Creature XP uses named categories (boss/critter/large/medium/small),
        # not numbered tiers. Not supported in this tool (universal tiers only).
    },
    "player": {
        "damage": StatConfig("player/damage", "damage", tier_min=1, tier_max=100),
        "resist": StatConfig("player/armor", "armor", tier_min=1, tier_max=100),
        "xp": StatConfig("player/xp", "xp", tier_min=1, tier_max=100),
        # Player has no HP curve.
    },
}

VALID_STATS = {
    "creature": sorted(STAT_CONFIG["creature"].keys()),
    "player": sorted(STAT_CONFIG["player"].keys()),
}

TYPE_ALIASES = {
    "npc": "creature",
    "creature": "creature",
    "pc": "player",
    "player": "player",
}


# ---------------------------------------------------------------------------
# Core logic
# ---------------------------------------------------------------------------

def resolve_curve_path(base_path: Path, version: str, char_type: str, stat: str, tier: int) -> Path:
    cfg = STAT_CONFIG[char_type][stat]
    filename = f"{cfg.prefix}_universal_tier{tier}.json"
    return base_path / version / "misc" / "curvetables" / "json" / cfg.subdir / filename


def load_curve(path: Path) -> list[dict]:
    with open(path) as f:
        data = json.load(f)
    return data["curve"]


def interpolate(curve: list[dict], level: float) -> float:
    """Linear interpolation with clamping at the curve's min/max x values."""
    xs = [p["x"] for p in curve]
    ys = [p["y"] for p in curve]

    if level <= xs[0]:
        return ys[0]
    if level >= xs[-1]:
        return ys[-1]

    for i in range(len(xs) - 1):
        if xs[i] <= level <= xs[i + 1]:
            t = (level - xs[i]) / (xs[i + 1] - xs[i])
            return ys[i] + t * (ys[i + 1] - ys[i])

    return ys[-1]  # unreachable, but safe fallback


def get_level_range(curve: list[dict]) -> tuple[float, float]:
    xs = [p["x"] for p in curve]
    return xs[0], xs[-1]


# ---------------------------------------------------------------------------
# Wizard (interactive prompting for missing args)
# ---------------------------------------------------------------------------

def prompt_choice(prompt: str, choices: list[str]) -> str:
    """Show a numbered menu and return the selected value."""
    print(prompt)
    for i, c in enumerate(choices, 1):
        print(f"  [{i}] {c}")
    while True:
        raw = input("  Choice: ").strip()
        if raw.isdigit() and 1 <= int(raw) <= len(choices):
            return choices[int(raw) - 1]
        # Also accept typing the value directly
        if raw.lower() in [c.lower() for c in choices]:
            return next(c for c in choices if c.lower() == raw.lower())
        print(f"  Please enter a number 1–{len(choices)} or one of {choices}.")


def prompt_int(prompt: str, min_val: int, max_val: int) -> int:
    while True:
        raw = input(f"  {prompt} [{min_val}–{max_val}]: ").strip()
        try:
            val = int(raw)
            if min_val <= val <= max_val:
                return val
            print(f"  Must be between {min_val} and {max_val}.")
        except ValueError:
            print("  Please enter a whole number.")


def wizard_fill(base_path: Path, version: str,
                char_type, stat, level, tier) -> tuple[str, str, int, int]:
    """Interactively prompt for any None values."""
    print()
    if char_type is None:
        char_type = prompt_choice("Character type?", ["creature", "player"])

    available_stats = VALID_STATS[char_type]
    if stat is None:
        stat = prompt_choice(f"Stat for {char_type}?", available_stats)
    elif stat not in STAT_CONFIG[char_type]:
        print(f"Error: '{stat}' is not a valid stat for {char_type}.")
        print(f"  Valid stats: {', '.join(available_stats)}")
        sys.exit(1)

    cfg = STAT_CONFIG[char_type][stat]
    tier_min, tier_max = cfg.tier_min, cfg.tier_max

    if tier is None:
        tier = prompt_int("Tier", tier_min, tier_max)
    elif not (tier_min <= tier <= tier_max):
        print(f"Error: tier {tier} is out of range for {char_type} {stat} (valid: {tier_min}–{tier_max}).")
        sys.exit(1)

    if level is None:
        # Load the curve to show the real level range
        path = resolve_curve_path(base_path, version, char_type, stat, tier)
        curve = load_curve(path)
        lv_min, lv_max = get_level_range(curve)
        level = prompt_int("Level", int(lv_min), int(lv_max))

    print()
    return char_type, stat, level, tier


# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------

def fmt_value(v: float) -> str:
    """Format value: integer if it's whole, else 2 decimal places."""
    return f"{v:.0f}" if v == int(v) else f"{v:.2f}"


def apply_resist(damage: float, resist: float) -> float:
    """Apply FO76 resistance formula: damage × clamp(0.01, 0.99, (damage×0.15/resist)^0.365)."""
    factor = max(0.01, min(0.99, (damage * 0.15 / resist) ** 0.365))
    return damage * factor


def print_result(char_type: str, stat: str, level: int, tier: int, value: float,
                 resist: float | None = None, multiplier: float | None = None) -> None:
    pre = value * multiplier if multiplier is not None else value
    label = f"{char_type.capitalize()} | {stat} | level {level} | tier {tier}"
    if resist is not None:
        post = apply_resist(pre, resist)
        mult_note = f"  ×{multiplier}" if multiplier is not None else ""
        print(f"{label}:  {fmt_value(pre)} (pre-resist{mult_note})  →  {fmt_value(post)} (post-resist vs {fmt_value(resist)})")
    else:
        mult_note = f"  ×{multiplier}" if multiplier is not None else ""
        print(f"{label}:  {fmt_value(pre)}{mult_note}")


def print_all_tiers(base_path: Path, version: str, char_type: str, stat: str, level: int,
                    resist: float | None = None, multiplier: float | None = None) -> None:
    cfg = STAT_CONFIG[char_type][stat]
    tier_min, tier_max = cfg.tier_min, cfg.tier_max
    mult_note = f"  (×{multiplier} multiplier)" if multiplier is not None else ""
    print(f"\n{char_type.capitalize()} {stat} at level {level} — all tiers:{mult_note}")
    if resist is not None:
        print(f"  {'Tier':>6}  {'Pre-resist':>14}  {'Post-resist':>14}  (vs resist {fmt_value(resist)})")
        print(f"  {'-'*6}  {'-'*14}  {'-'*14}")
    else:
        print(f"  {'Tier':>6}  {'Value':>14}")
        print(f"  {'-'*6}  {'-'*14}")
    for tier in range(tier_min, tier_max + 1):
        path = resolve_curve_path(base_path, version, char_type, stat, tier)
        curve = load_curve(path)
        raw = interpolate(curve, level)
        pre = raw * multiplier if multiplier is not None else raw
        if resist is not None:
            post = apply_resist(pre, resist)
            print(f"  {tier:>6}  {fmt_value(pre):>14}  {fmt_value(post):>14}")
        else:
            print(f"  {tier:>6}  {fmt_value(pre):>14}")


# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

def parse_args():
    parser = argparse.ArgumentParser(
        description="Look up FO76 PTS curve table values (damage, HP, resist, XP).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "-t", "--type",
        dest="char_type",
        metavar="TYPE",
        help="creature/npc or player/pc (omit to be prompted)",
    )
    parser.add_argument(
        "-s", "--stat",
        dest="stat",
        metavar="STAT",
        help="damage, hp, resist, or xp (omit to be prompted)",
    )
    parser.add_argument(
        "-l", "--level",
        dest="level",
        type=int,
        metavar="LEVEL",
        help="Character level to look up (omit to be prompted)",
    )
    parser.add_argument(
        "-T", "--tier",
        dest="tier",
        type=int,
        metavar="TIER",
        help="Tier number (omit to be prompted)",
    )
    parser.add_argument(
        "--patch",
        default="latest",
        metavar="VERSION",
        help="Snapshot to use, e.g. 20260803 (default: latest dated snapshot)",
    )
    parser.add_argument(
        "--base-path",
        default=os.environ.get("FO76_DATA_DIR", str(SCRIPT_DIR)),
        metavar="PATH",
        help="Base directory containing version folders (default: $FO76_DATA_DIR)",
    )
    parser.add_argument(
        "--all-tiers",
        action="store_true",
        help="Print the value for every available tier at the given level",
    )
    parser.add_argument(
        "--resist",
        dest="resist",
        type=float,
        metavar="RESIST",
        help="Resistance value; shows post-resist damage alongside the raw value",
    )
    parser.add_argument(
        "--multiplier", "-m",
        dest="multiplier",
        type=float,
        metavar="MULT",
        help="Total multiplier applied to the pre-resist damage in all outputs",
    )
    return parser.parse_args()


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    args = parse_args()
    base_path = Path(args.base_path)
    version = args.patch

    # "latest" resolves to the newest dated snapshot dir (Data/<YYYYMMDD>/ layout)
    # unless a literal latest/ directory exists.
    if version == "latest" and not (base_path / "latest").exists():
        snapshots = sorted(p.name for p in base_path.glob("[0-9]" * 8) if p.is_dir())
        if not snapshots:
            print(f"Error: no dated snapshot directories under {base_path}")
            sys.exit(1)
        version = snapshots[-1]

    # Normalize type aliases
    char_type = None
    if args.char_type is not None:
        normalized = TYPE_ALIASES.get(args.char_type.lower())
        if normalized is None:
            print(f"Error: unknown type '{args.char_type}'. Use creature/npc or player/pc.")
            sys.exit(1)
        char_type = normalized

    stat = args.stat.lower() if args.stat else None
    level = args.level
    tier = args.tier
    resist = args.resist
    multiplier = args.multiplier

    # Wizard: fill in any missing required inputs interactively
    needs_wizard = any(v is None for v in [char_type, stat, level, tier])
    if needs_wizard:
        char_type, stat, level, tier = wizard_fill(base_path, version, char_type, stat, level, tier)
    else:
        # Validate stat for type even in non-wizard mode
        assert char_type is not None and stat is not None  # guaranteed by the needs_wizard check above
        if stat not in STAT_CONFIG.get(char_type, {}):
            available = VALID_STATS.get(char_type, [])
            print(f"Error: '{stat}' is not a valid stat for {char_type}.")
            if char_type == "player" and stat == "hp":
                print("  Players have no HP curve in these tables.")
            print(f"  Valid stats for {char_type}: {', '.join(available)}")
            sys.exit(1)
        cfg = STAT_CONFIG[char_type][stat]
        tier_min, tier_max = cfg.tier_min, cfg.tier_max
        if not (tier_min <= tier <= tier_max):
            print(f"Error: tier {tier} is out of range for {char_type} {stat} (valid: {tier_min}–{tier_max}).")
            sys.exit(1)

    # Validate that the version directory exists
    version_path = base_path / version
    if not version_path.exists():
        print(f"Error: version directory not found: {version_path}")
        sys.exit(1)

    # After wizard_fill or validation, all four are guaranteed non-None
    assert char_type is not None and stat is not None and level is not None and tier is not None

    # --all-tiers mode
    if args.all_tiers:
        print_all_tiers(base_path, version, char_type, stat, level, resist=resist, multiplier=multiplier)
        return

    # Single lookup
    path = resolve_curve_path(base_path, version, char_type, stat, tier)
    curve = load_curve(path)
    value = interpolate(curve, level)
    print_result(char_type, stat, level, tier, value, resist=resist, multiplier=multiplier)


if __name__ == "__main__":
    main()
