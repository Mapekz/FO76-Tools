#!/usr/bin/env python3
"""curvelib.py — dependency-free linear-interpolation helpers. Consumer:
`curvelookup.py` (hardcoded tiered curve files). Any-CURV-record lookup is
the native `esm curve` subcommand (`src/curves.rs`)."""

from __future__ import annotations


def interpolate(curve: list[dict], x: float) -> float:
    """Linear interpolation with clamping at the curve's min/max x values.

    Mirrors `eval` in `src/curves.rs` exactly: any change to clamping/
    edge-case semantics there should be ported here too."""
    if not curve:
        raise ValueError("interpolate: curve has no points")

    curve = sorted(curve, key=lambda p: p["x"])
    xs = [p["x"] for p in curve]
    ys = [p["y"] for p in curve]

    if x <= xs[0]:
        return ys[0]
    if x >= xs[-1]:
        return ys[-1]

    for i in range(len(xs) - 1):
        if xs[i] <= x <= xs[i + 1]:
            t = (x - xs[i]) / (xs[i + 1] - xs[i])
            return ys[i] + t * (ys[i + 1] - ys[i])

    return ys[-1]  # unreachable, but safe fallback


def fmt_value(v: float) -> str:
    """Format value: integer if it's whole, else 2 decimal places."""
    return f"{v:.0f}" if v == int(v) else f"{v:.2f}"
