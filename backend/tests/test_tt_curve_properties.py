"""Property-based tests for the TT value curve.

Covers ``backend.data.tt_value_curve``. The curve is **non-strictly** monotone
(it has long flat regions), so the properties encode bounds and non-strict
inequalities, never a clean two-way inverse.
"""

import pytest
from hypothesis import given
from hypothesis import strategies as st

from backend.data.tt_value_curve import (
    levels_for_tt_value,
    max_tt_curve_level,
    tt_value_at,
    tt_value_of_gain,
)

_MAX = float(max_tt_curve_level())
_TOP = tt_value_at(_MAX)
_LEVEL = st.floats(
    min_value=-100.0, max_value=_MAX + 100.0, allow_nan=False, allow_infinity=False
)
_IN_RANGE = st.floats(
    min_value=0.0, max_value=_MAX, allow_nan=False, allow_infinity=False
)
_PED = st.floats(min_value=0.0, max_value=1e5, allow_nan=False, allow_infinity=False)


@given(_LEVEL)
def test_value_is_bounded(level):
    assert 0.0 <= tt_value_at(level) <= _TOP + 1e-6


def test_clamping_at_the_ends():
    assert tt_value_at(-5.0) == 0.0
    assert tt_value_at(0.0) == 0.0
    assert tt_value_at(_MAX) == _TOP
    assert tt_value_at(_MAX + 100.0) == _TOP


@given(_LEVEL, _LEVEL)
def test_non_strictly_monotone(a, b):
    lo, hi = sorted((a, b))
    assert tt_value_at(lo) <= tt_value_at(hi) + 1e-6


@given(_LEVEL)
def test_gain_of_zero_span_is_zero(level):
    assert tt_value_of_gain(level, level) == 0.0


@given(_LEVEL, _LEVEL)
def test_gain_is_antisymmetric(a, b):
    assert tt_value_of_gain(a, b) == pytest.approx(-tt_value_of_gain(b, a), abs=1e-4)


@given(_LEVEL, _LEVEL)
def test_gain_sign_matches_value_ordering(a, b):
    gain = tt_value_of_gain(a, b)
    if tt_value_at(b) > tt_value_at(a):
        assert gain >= -1e-9
    elif tt_value_at(b) < tt_value_at(a):
        assert gain <= 1e-9


@given(_LEVEL, _LEVEL, _LEVEL)
def test_gain_telescopes(a, b, c):
    assert tt_value_of_gain(a, c) == pytest.approx(
        tt_value_of_gain(a, b) + tt_value_of_gain(b, c), abs=1e-3
    )


def test_levels_for_non_positive_ped_is_zero():
    assert levels_for_tt_value(100.0, 0.0) == 0.0
    assert levels_for_tt_value(100.0, -5.0) == 0.0


@given(
    _IN_RANGE,
    st.floats(min_value=0.0, max_value=1e6, allow_nan=False, allow_infinity=False),
)
def test_levels_for_tt_value_never_overspends(frm, ped):
    result = levels_for_tt_value(frm, ped)
    assert result >= 0.0
    spent = tt_value_at(frm + result)
    budget = tt_value_at(frm) + ped
    # The result is rounded to 1e-4 levels; allow one such step of slack.
    slack = abs(tt_value_at(frm + result + 1e-4) - spent) + 1e-4
    assert spent <= budget + slack


@given(_IN_RANGE, _PED, _PED)
def test_levels_for_tt_value_is_monotone_in_ped(frm, p1, p2):
    lo, hi = sorted((p1, p2))
    assert levels_for_tt_value(frm, hi) + 1e-6 >= levels_for_tt_value(frm, lo)


def test_levels_saturates_at_the_curve_ceiling():
    frm = 100.0
    assert levels_for_tt_value(frm, 1e12) == pytest.approx(_MAX - frm)
