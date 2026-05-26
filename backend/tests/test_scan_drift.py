"""Property-based and unit tests for scan-vs-tracked level drift.

Covers ``backend.services.scan_drift.summarize_level_drift``, which compares the
app's tracked skill levels against a fresh scan and summarises the divergence.
"""

import pytest
from hypothesis import assume, given
from hypothesis import strategies as st

from backend.services.scan_drift import summarize_level_drift

# A small key space so the two maps frequently share names (few discards), and
# finite, bounded levels so the accumulated metrics never overflow or go NaN.
_NAMES = st.text(alphabet="abc", min_size=1, max_size=2)
_LEVELS = st.floats(
    min_value=-1e6, max_value=1e6, allow_nan=False, allow_infinity=False
)
_LEVEL_MAPS = st.dictionaries(_NAMES, _LEVELS, max_size=6)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_none_iff_no_shared_keys(tracked, scanned):
    shared = set(tracked) & set(scanned)
    assert (summarize_level_drift(tracked, scanned) is None) == (not shared)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_count_partition_is_conserved(tracked, scanned):
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None  # narrow for the type checker; assume guarantees it
    assert result["compared_count"] + result["tracked_only_count"] == len(tracked)
    assert result["compared_count"] + result["scan_only_count"] == len(scanned)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_absolute_metrics_are_non_negative(tracked, scanned):
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    assert result["total_abs_diff"] >= 0.0
    assert result["avg_abs_diff"] >= 0.0
    assert result["avg_abs_pct"] >= 0.0
    assert result["worst_abs_diff"] >= 0.0


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_averages_match_their_totals(tracked, scanned):
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    n = result["compared_count"]
    assert result["avg_abs_diff"] == pytest.approx(result["total_abs_diff"] / n)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_signed_total_within_absolute_total(tracked, scanned):
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    # Triangle inequality: the net signed drift cannot exceed the summed magnitude.
    assert abs(result["total_signed_diff"]) <= result["total_abs_diff"] + 1e-6


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_worst_is_the_maximum_absolute_diff(tracked, scanned):
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    shared = set(tracked) & set(scanned)
    max_abs = max(abs(float(scanned[n]) - float(tracked[n])) for n in shared)
    assert result["worst_abs_diff"] == pytest.approx(max_abs)
    assert result["worst_abs_diff"] == pytest.approx(abs(result["worst_signed_diff"]))


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_total_abs_diff_is_swap_invariant(tracked, scanned):
    forward = summarize_level_drift(tracked, scanned)
    backward = summarize_level_drift(scanned, tracked)
    assume(forward is not None)
    assert forward is not None
    assert backward is not None
    assert forward["total_abs_diff"] == pytest.approx(backward["total_abs_diff"])


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_result_is_independent_of_key_insertion_order(tracked, scanned):
    forward = summarize_level_drift(tracked, scanned)
    reordered = summarize_level_drift(
        dict(reversed(list(tracked.items()))),
        dict(reversed(list(scanned.items()))),
    )
    assert forward == reordered


# --- plain units ---


def test_returns_none_without_overlap():
    assert summarize_level_drift({"Handgun": 1.0}, {"Rifle": 2.0}) is None


def test_drift_shape_and_worst_selection():
    result = summarize_level_drift({"Handgun": 10.0, "Rifle": 20.0}, {"Handgun": 12.0})
    assert result is not None
    assert result["compared_count"] == 1
    assert result["tracked_only_count"] == 1
    assert result["scan_only_count"] == 0
    assert result["worst_name"] == "Handgun"
    assert result["worst_signed_diff"] == pytest.approx(2.0)
    assert result["worst_abs_diff"] == pytest.approx(2.0)
