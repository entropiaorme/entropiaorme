"""Mutation-killing tests for ``backend.data.tt_value_curve``.

Targets the surviving mutants in the ``tt_value_curve`` cluster of the
EntropiaOrme mutation campaign. Each test imports the real module and
exercises the exact mutated line/behaviour, asserting the precise result
the mutation breaks.

The ``_load_curve`` mutants are driven by calling ``_load_curve()``
*explicitly* (rather than relying on the import-time ``_LEVELS``/``_TT_VALUES``
binding): mutmut's trampoline routes the call to the mutant at call time, so a
direct invocation deterministically exercises the mutated body regardless of
module-import caching during the run.
"""

import pytest

import backend.data.tt_value_curve as ttc


# ── _load_curve: exact CSV parse ─────────────────────────────────────────────
#
# Kills the mutants that either raise (None list / None open args / bad key /
# bad cast / wrong column name) or load wrong data (None elements). A direct
# call to _load_curve() is routed to the mutant by the trampoline; we assert the
# parse produced the exact, well-typed anchors from the CSV.


def test_load_curve_returns_exact_anchors():
    levels, tt_values = ttc._load_curve()

    # Two parallel, equal-length lists (catches None-initialised lists and
    # any cast/key error which would raise before returning).
    assert isinstance(levels, list) and isinstance(tt_values, list)
    assert len(levels) == len(tt_values)
    assert len(levels) == 20001

    # Every element is the correctly-typed parsed value (catches append(None),
    # int(None)/float(None), and wrong/upper-cased column keys which would have
    # raised KeyError before reaching here).
    assert all(isinstance(x, int) for x in levels)
    assert all(isinstance(x, float) for x in tt_values)

    # Exact known anchors from tt_value_curve.csv.
    assert levels[0] == 0
    assert levels[-1] == 20000
    assert levels[:8] == [0, 1, 2, 3, 4, 5, 6, 7]
    assert tt_values[0] == 0.0
    assert tt_values[1] == 0.0
    assert tt_values[2] == 0.01
    assert tt_values[7] == 0.02
    assert tt_values[-1] == 13381.54
    assert tt_values[-2] == 13380.62

    # The level column is the strictly-increasing 0..20000 sequence (a wrong
    # column key would not produce this).
    assert levels == list(range(20001))


def test_module_level_curve_matches_load_curve():
    # The import-time binding must equal a fresh parse (guards the wrong-data
    # mutants that still return a list, e.g. append(None)).
    levels, tt_values = ttc._load_curve()
    assert ttc._LEVELS == levels
    assert ttc._TT_VALUES == tt_values


# ── tt_value_at ──────────────────────────────────────────────────────────────


def test_tt_value_at_interpolates_just_below_the_ceiling():
    # mutmut_6: clamp guard ``level >= _LEVELS[-1]`` -> ``_LEVELS[-2]`` would
    # return the ceiling V[-1]=13381.54 for any level >= 19999. The real curve
    # interpolates between V[-2] and V[-1] at 19999.5.
    assert ttc.tt_value_at(19999.5) == 13381.08
    assert ttc.tt_value_at(19999.5) != ttc._TT_VALUES[-1]


def test_tt_value_at_rounds_to_four_decimals():
    # mutmut_32: round(..., 4) -> round(..., 5). At level 6.001 the interpolated
    # value is 0.01001; rounded to 4 dp it is 0.01, to 5 dp it is 0.01001.
    assert ttc.tt_value_at(6.001) == 0.01


def test_tt_value_at_ceiling_clamp_holds():
    # Anchors the ceiling clamp and the interpolation denominator together.
    assert ttc.tt_value_at(20000.0) == 13381.54
    # mutmut_20 is an equivalent (spacing == 1), but a finite interpolated value
    # mid-curve still pins the interpolation arithmetic against gross breakage.
    assert ttc.tt_value_at(100.5) == pytest.approx(0.125, abs=1e-9)


# ── tt_value_of_gain ─────────────────────────────────────────────────────────


def test_tt_value_of_gain_is_a_difference():
    # Pins the difference (kills any add/arg-swap had they survived) and the
    # 4-dp rounding contract of the public result.
    g = ttc.tt_value_of_gain(100.0, 200.0)
    assert g == pytest.approx(
        ttc.tt_value_at(200.0) - ttc.tt_value_at(100.0), abs=1e-9
    )
    assert g > 0.0


# ── levels_for_tt_value ──────────────────────────────────────────────────────


def test_levels_saturates_when_target_equals_ceiling():
    # mutmut_11: ``target_tt >= _TT_VALUES[-1]`` -> ``>``. At target exactly
    # equal to the ceiling the real function returns the full span to the top
    # (20000.0); the mutant falls into the binary search and returns 19999.9999.
    top = ttc._TT_VALUES[-1]
    assert ttc.levels_for_tt_value(0.0, top) == 20000.0


def test_levels_does_not_saturate_just_below_ceiling():
    # mutmut_13: ``_TT_VALUES[-1]`` -> ``_TT_VALUES[-2]``. A target strictly
    # between V[-2] and V[-1] must NOT shortcut to the top; the real binary
    # search returns 19999.5001 while the mutant returns 20000.0.
    ped = (ttc._TT_VALUES[-2] + ttc._TT_VALUES[-1]) / 2
    result = ttc.levels_for_tt_value(0.0, ped)
    assert result == 19999.5001
    assert result < 20000.0


def test_levels_for_tt_value_rounds_to_four_decimals():
    # mutmut_30: round(..., 4) -> round(..., 5). levels_for_tt_value(0.0, 17.68)
    # rounds to 2446.3317 at 4 dp but 2446.33167 at 5 dp.
    assert ttc.levels_for_tt_value(0.0, 17.68) == 2446.3317
