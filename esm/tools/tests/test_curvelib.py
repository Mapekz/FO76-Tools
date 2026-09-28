#!/usr/bin/env python3
"""Tests for tools/curvelib.py.

Pure-function coverage only -- `interpolate`/`fmt_value` take plain data and
return plain values, no gateway/fixture dependency needed. Consumer:
`curvelookup.py`."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import curvelib  # noqa: E402


class TestInterpolate(unittest.TestCase):
    def setUp(self):
        self.curve = [
            {"x": 0.0, "y": 0.0},
            {"x": 10.0, "y": 100.0},
            {"x": 20.0, "y": 300.0},
        ]

    def test_midpoint_interpolation(self):
        # Midpoint between (0, 0) and (10, 100) -> 50.
        self.assertEqual(curvelib.interpolate(self.curve, 5.0), 50.0)

    def test_midpoint_interpolation_second_segment(self):
        # Midpoint between (10, 100) and (20, 300) -> 200.
        self.assertEqual(curvelib.interpolate(self.curve, 15.0), 200.0)

    def test_clamps_below_domain(self):
        self.assertEqual(curvelib.interpolate(self.curve, -5.0), 0.0)

    def test_clamps_above_domain(self):
        self.assertEqual(curvelib.interpolate(self.curve, 999.0), 300.0)

    def test_exact_point_hit(self):
        self.assertEqual(curvelib.interpolate(self.curve, 10.0), 100.0)
        self.assertEqual(curvelib.interpolate(self.curve, 0.0), 0.0)
        self.assertEqual(curvelib.interpolate(self.curve, 20.0), 300.0)

    def test_empty_curve_raises_value_error(self):
        with self.assertRaises(ValueError):
            curvelib.interpolate([], 5.0)

    def test_unsorted_input_is_sorted_defensively(self):
        # Same three points as `self.curve`, given out of x-order. Getting
        # the sort wrong (e.g. relying on input order for bracketing) would
        # evaluate x=5 against the wrong segment and return something other
        # than 50.0.
        unsorted = [
            {"x": 20.0, "y": 300.0},
            {"x": 0.0, "y": 0.0},
            {"x": 10.0, "y": 100.0},
        ]
        self.assertEqual(curvelib.interpolate(unsorted, 5.0), 50.0)


class TestFmtValue(unittest.TestCase):
    def test_whole_number(self):
        self.assertEqual(curvelib.fmt_value(5.0), "5")

    def test_fractional_number(self):
        self.assertEqual(curvelib.fmt_value(5.5), "5.50")


if __name__ == "__main__":
    unittest.main()
