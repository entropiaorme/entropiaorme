"""TT value curve from the official Entropia Universe wiki chip-in optimizer.

Data lives in tt_value_curve.csv (same directory), loaded once at import time.
Linear interpolation between monotonic non-decreasing anchors, with level 0
anchored to 0.0 PED.
"""

import bisect
import csv
from pathlib import Path

# ── Load curve data from CSV ─────────────────────────────────────────────────

_CSV_PATH = Path(__file__).parent / "tt_value_curve.csv"


def _load_curve() -> tuple[list[int], list[float]]:
    levels: list[int] = []
    tt_values: list[float] = []
    with open(_CSV_PATH, encoding="utf-8") as f:
        for row in csv.DictReader(f):
            levels.append(int(row["level"]))
            tt_values.append(float(row["tt_value"]))
    return levels, tt_values


_LEVELS, _TT_VALUES = _load_curve()

# ── Public API (unchanged) ───────────────────────────────────────────────────


def tt_value_at(level: float) -> float:
    """Return cumulative TT value (PED) at a skill level. Linear interpolation between anchors."""
    if level <= 0:
        return 0.0
    if level >= _LEVELS[-1]:
        return _TT_VALUES[-1]
    i = bisect.bisect_right(_LEVELS, level) - 1
    lo, hi = _LEVELS[i], _LEVELS[i + 1]
    t = (level - lo) / (hi - lo)
    return round(_TT_VALUES[i] + t * (_TT_VALUES[i + 1] - _TT_VALUES[i]), 4)


def tt_value_of_gain(from_level: float, to_level: float) -> float:
    """TT value of a skill gain from from_level to to_level."""
    return round(tt_value_at(to_level) - tt_value_at(from_level), 4)


def levels_for_tt_value(from_level: float, ped_value: float) -> float:
    """How many skill levels does ped_value PED of TT buy starting from from_level?

    Uses binary search on the TT curve. Returns fractional levels gained,
    never negative: a from_level past the curve ceiling buys zero levels,
    not a negative span (skill progress is monotonic non-decreasing).
    """
    if ped_value <= 0:
        return 0.0
    target_tt = tt_value_at(from_level) + ped_value
    lo, hi = from_level, float(_LEVELS[-1])
    if target_tt >= _TT_VALUES[-1]:
        # Floor at zero: a from_level above the curve ceiling would
        # otherwise yield a negative buy, which a codex reward applies
        # as a calibration decrease. A non-positive buy is zero.
        return max(0.0, hi - from_level)
    for _ in range(64):
        mid = (lo + hi) / 2
        if tt_value_at(mid) < target_tt:
            lo = mid
        else:
            hi = mid
    return round(lo - from_level, 4)


def max_tt_curve_level() -> int:
    """Return the highest level represented by the loaded TT curve data."""
    return _LEVELS[-1]
