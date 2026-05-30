"""Mutation-hardening tests for backend.services.skill_panel_parse.

Targets the surviving mutants in the skill_panel_parse cluster:

  - x_parse_bar_fill__mutmut_29: `col_mean >= threshold` -> `col_mean > threshold`.
    The boundary (a column whose mean is *exactly* the midpoint threshold) is the
    only place the two comparators differ, and it must be the *rightmost* bright
    column for the difference to reach the reported fill. The existing suite never
    lands a column exactly on the threshold, so the mutant survived.

  - x_fuzzy_resolve__mutmut_22: drops the explicit `scorer=_FUZZ_SCORER` kwarg from
    `process.extract`. rapidfuzz's `process.extract` already defaults `scorer` to
    `fuzz.WRatio` (the same object bound to `_FUZZ_SCORER`), so the call is
    behaviour-identical today. Recorded as an equivalent mutant, not killed here.
"""

from __future__ import annotations

import numpy as np
import pytest

from backend.services import skill_panel_parse as sp


def _bar_from_cols(values: list[int], height: int = 4) -> np.ndarray:
    """A BGR crop whose column j is the flat grey value ``values[j]``.

    Equal channels mean the BGR->GRAY conversion returns the value itself, so
    the per-column luminance the estimator reads is exactly ``values``.
    """
    crop = np.zeros((height, len(values), 3), dtype=np.uint8)
    for j, v in enumerate(values):
        crop[:, j, :] = v
    return crop


def test_parse_bar_fill_threshold_column_is_inclusive():
    """A column whose mean equals the midpoint threshold counts as bright.

    Columns [0, 200, 100, 50, 10]: lo=0, hi=200, threshold=(0+200)/2=100.
    Column index 2 has mean *exactly* 100, equal to the threshold, and is the
    rightmost column at or above it. The inclusive `col_mean >= threshold`
    keeps it bright, so rightmost=2 and fill=(2+1)/5=0.6. A strict
    `col_mean > threshold` would drop that boundary column, leaving rightmost=1
    and fill=(1+1)/5=0.4. Asserting 0.6 (not 0.4) kills the `>=`->`>` mutant.
    """
    crop = _bar_from_cols([0, 200, 100, 50, 10])
    # Guard the construction: column means are exactly the requested values and
    # the index-2 column sits precisely on the midpoint threshold.
    import cv2

    col_mean = cv2.cvtColor(crop, cv2.COLOR_BGR2GRAY).mean(axis=0)
    assert list(col_mean) == [0.0, 200.0, 100.0, 50.0, 10.0]
    assert col_mean[2] == (col_mean.min() + col_mean.max()) / 2.0

    assert sp.parse_bar_fill(crop) == pytest.approx(0.6)
    # And explicitly distinguish from the strict-comparator outcome.
    assert sp.parse_bar_fill(crop) != pytest.approx(0.4)
