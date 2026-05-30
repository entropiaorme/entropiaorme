"""Mutation-hardening tests for ``backend.services.scan_drift``.

These example-based tests pin the exact numeric outputs of
``summarize_level_drift`` so that the surviving accumulator-init, arithmetic,
and tie-break mutants in the ``scan_drift`` cluster are killed. Each test names
the behaviour it locks down; values are computed by hand from the documented
formula::

    signed = scanned - tracked
    abs_diff = abs(signed)
    abs_pct = abs_diff / max(abs(scanned), 1.0) * 100.0

and the per-name "worst" is the first name (in sorted order) whose ``abs_diff``
is strictly the largest seen so far.
"""

import pytest

from backend.services.scan_drift import summarize_level_drift


def test_single_pair_exact_metrics():
    """Pin every numeric field for one shared name.

    tracked=10, scanned=16 -> signed=+6, abs_diff=6,
    abs_pct = 6 / max(16, 1) * 100 = 37.5.

    Kills the percent-formula and avg mutants on the single-pair path:
      - mut_35  '* 100.0' -> '/ 100.0'   (avg_abs_pct would be 0.00375)
      - mut_36  '/ max(.)' -> '* max(.)'  (avg_abs_pct would be 9600.0)
      - mut_43  '* 100.0' -> '* 101.0'   (avg_abs_pct would be 37.875)
      - mut_72  'total / count' -> 'total * count' (avg_abs_pct would be 37.5*1
                                                     == same here, so the
                                                     two-pair test below pins it)
    """
    result = summarize_level_drift({"a": 10.0}, {"a": 16.0})
    assert result is not None
    assert result["compared_count"] == 1
    assert result["total_abs_diff"] == pytest.approx(6.0)
    assert result["avg_abs_diff"] == pytest.approx(6.0)
    assert result["total_signed_diff"] == pytest.approx(6.0)
    assert result["avg_abs_pct"] == pytest.approx(37.5)
    assert result["worst_name"] == "a"
    assert result["worst_tracked"] == pytest.approx(10.0)
    assert result["worst_scanned"] == pytest.approx(16.0)
    assert result["worst_signed_diff"] == pytest.approx(6.0)
    assert result["worst_abs_diff"] == pytest.approx(6.0)


def test_percent_uses_floor_of_one_not_two():
    """abs(scanned) in [1, 2) makes the max() floor observable.

    tracked=0.5, scanned=1.5 -> abs_diff=1.0,
    abs_pct = 1.0 / max(1.5, 1.0) * 100 = 66.666...

    mut_42 changes the floor 1.0 -> 2.0, giving 1.0 / max(1.5, 2.0) * 100 = 50.0.
    """
    result = summarize_level_drift({"a": 0.5}, {"a": 1.5})
    assert result is not None
    assert result["avg_abs_pct"] == pytest.approx(100.0 / 1.5)
    assert result["avg_abs_pct"] != pytest.approx(50.0)


def test_two_pairs_accumulate_totals_and_averages():
    """Two shared names so the running sums (not just the last term) matter.

    a: tracked=10, scanned=16 -> signed=+6, abs_diff=6, abs_pct=6/16*100=37.5
    b: tracked=10, scanned=14 -> signed=+4, abs_diff=4, abs_pct=4/14*100=28.5714...

    total_abs_diff = 10.0, avg_abs_diff = 5.0
    total_signed_diff = 10.0
    total_abs_pct = 66.0714..., avg_abs_pct = 33.0357...

    Kills:
      - mut_9   total_abs_diff init 0.0 -> 1.0  (total 11.0, avg 5.5)
      - mut_13  total_abs_pct  init 0.0 -> 1.0  (avg_abs_pct 33.5357...)
      - mut_46  'total_signed_diff +=' -> '='   (total_signed_diff 4.0)
      - mut_47  'total_signed_diff +=' -> '-='  (total_signed_diff -10.0)
      - mut_48  'total_abs_pct +=' -> '='       (avg_abs_pct 14.2857...)
      - mut_72  'avg = total / count' -> 'total * count' (avg_abs_pct 132.14...)
    """
    result = summarize_level_drift({"a": 10.0, "b": 10.0}, {"a": 16.0, "b": 14.0})
    assert result is not None
    assert result["compared_count"] == 2
    assert result["total_abs_diff"] == pytest.approx(10.0)
    assert result["avg_abs_diff"] == pytest.approx(5.0)
    assert result["total_signed_diff"] == pytest.approx(10.0)
    expected_total_pct = 6.0 / 16.0 * 100.0 + 4.0 / 14.0 * 100.0
    assert result["avg_abs_pct"] == pytest.approx(expected_total_pct / 2.0)


def test_strict_tie_break_keeps_first_sorted_worst():
    """Equal abs_diff between names: '>' keeps the first, '>=' takes the last.

    Sorted order is ['a', 'b']. Both have abs_diff == 5.0 but opposite signs:
      a: tracked=10, scanned=15 -> signed=+5
      b: tracked=30, scanned=25 -> signed=-5

    With the original strict '>' the worst stays 'a' (the first seen with the
    max), so worst_signed_diff == +5.0. mut_50 relaxes '>' to '>=', which lets
    'b' overwrite on the tie, flipping worst to 'b' (worst_signed_diff == -5.0).
    """
    result = summarize_level_drift({"a": 10.0, "b": 30.0}, {"a": 15.0, "b": 25.0})
    assert result is not None
    assert result["worst_abs_diff"] == pytest.approx(5.0)
    assert result["worst_name"] == "a"
    assert result["worst_tracked"] == pytest.approx(10.0)
    assert result["worst_scanned"] == pytest.approx(15.0)
    assert result["worst_signed_diff"] == pytest.approx(5.0)


def test_worst_tracked_and_scanned_carry_real_values():
    """The winning name's tracked/scanned are copied through unchanged.

    Single shared name with a unique large drift so the if-branch fires:
      tracked=42.0, scanned=7.0 -> worst_tracked=42.0, worst_scanned=7.0.

    Kills the in-branch assignment mutants:
      - mut_52  'worst_tracked = tracked' -> 'worst_tracked = None'
      - mut_53  'worst_scanned = scanned' -> 'worst_scanned = None'
    """
    result = summarize_level_drift({"x": 42.0}, {"x": 7.0})
    assert result is not None
    assert result["worst_tracked"] == pytest.approx(42.0)
    assert result["worst_scanned"] == pytest.approx(7.0)
    assert result["worst_signed_diff"] == pytest.approx(-35.0)
