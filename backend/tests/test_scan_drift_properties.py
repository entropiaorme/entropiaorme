"""Property-based tests for scan-vs-tracked level drift.

Covers ``backend.services.scan_drift.summarize_level_drift``: a pure function
that compares the app's tracked skill levels against a fresh scan and
summarises the divergence. Every property below restricts levels to finite,
bounded floats, mirroring the production ingestion paths (OCR-parsed scan
levels and ``skill_calibrations.level`` REAL rows), which never persist NaN or
infinite values; the metrics are only well behaved over that finite domain.
"""

import math

import pytest
from hypothesis import assume, given
from hypothesis import strategies as st

from backend.services.scan_drift import summarize_level_drift

# A small key space so the two maps frequently share names, with finite,
# bounded levels so the accumulated metrics never overflow or go NaN.
_NAMES = st.text(alphabet="abc", min_size=1, max_size=2)
_LEVELS = st.floats(
    min_value=-1e6, max_value=1e6, allow_nan=False, allow_infinity=False
)
_LEVEL_MAPS = st.dictionaries(_NAMES, _LEVELS, max_size=6)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_none_iff_no_shared_keys(tracked, scanned):
    """A summary exists exactly when the two maps share at least one name."""
    shared = set(tracked) & set(scanned)
    assert (summarize_level_drift(tracked, scanned) is None) == (not shared)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_count_partition_is_conserved(tracked, scanned):
    """Compared + per-side-only counts partition each input's key set."""
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    shared = set(tracked) & set(scanned)
    assert result["compared_count"] == len(shared)
    assert result["compared_count"] + result["tracked_only_count"] == len(tracked)
    assert result["compared_count"] + result["scan_only_count"] == len(scanned)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_absolute_metrics_are_non_negative(tracked, scanned):
    """All magnitude-derived metrics stay non-negative for finite inputs."""
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    assert result["total_abs_diff"] >= 0.0
    assert result["avg_abs_diff"] >= 0.0
    assert result["avg_abs_pct"] >= 0.0
    assert result["worst_abs_diff"] >= 0.0


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_signed_total_within_absolute_total(tracked, scanned):
    """Triangle inequality: net signed drift cannot exceed summed magnitude."""
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    assert abs(result["total_signed_diff"]) <= result["total_abs_diff"] + 1e-6


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_worst_is_the_maximum_absolute_diff(tracked, scanned):
    """The reported worst row is the argmax over absolute drift, and its
    fields reconstruct exactly from the stored tracked/scanned operands."""
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    shared = set(tracked) & set(scanned)
    max_abs = max(abs(float(scanned[n]) - float(tracked[n])) for n in shared)
    assert result["worst_abs_diff"] == pytest.approx(max_abs)
    # Clauses below are construction-exact (same operands, recomputed), so
    # require bitwise equality rather than approximate agreement.
    assert result["worst_abs_diff"] == abs(result["worst_signed_diff"])
    assert (
        result["worst_signed_diff"] == result["worst_scanned"] - result["worst_tracked"]
    )


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_abs_pct_uses_scanned_denominator_floored_at_one(tracked, scanned):
    """``avg_abs_pct`` is the mean of abs(diff) / max(|scanned|, 1) * 100 over
    the shared names, using the SCANNED value as the denominator."""
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    shared = set(tracked) & set(scanned)
    expected = sum(
        abs(float(scanned[n]) - float(tracked[n]))
        / max(abs(float(scanned[n])), 1.0)
        * 100.0
        for n in shared
    ) / len(shared)
    assert result["avg_abs_pct"] == pytest.approx(expected)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_symmetric_metrics_are_swap_invariant(tracked, scanned):
    """Swapping the two maps leaves the symmetric magnitude metrics unchanged
    (total/avg absolute diff, worst absolute diff), even though the signed
    total and percentage metrics are not symmetric."""
    forward = summarize_level_drift(tracked, scanned)
    backward = summarize_level_drift(scanned, tracked)
    assume(forward is not None)
    assert forward is not None
    assert backward is not None
    assert forward["total_abs_diff"] == pytest.approx(backward["total_abs_diff"])
    assert forward["avg_abs_diff"] == pytest.approx(backward["avg_abs_diff"])
    assert forward["worst_abs_diff"] == pytest.approx(backward["worst_abs_diff"])


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_averages_are_the_means_of_their_totals(tracked, scanned):
    """Each average equals its total divided by the compared count, which is
    a positive integer on every non-None path."""
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    n = result["compared_count"]
    assert n >= 1
    assert result["avg_abs_diff"] == pytest.approx(result["total_abs_diff"] / n)


@given(_LEVEL_MAPS, _LEVEL_MAPS)
def test_all_reported_metrics_are_finite(tracked, scanned):
    """Finite level inputs yield finite metrics throughout the summary."""
    result = summarize_level_drift(tracked, scanned)
    assume(result is not None)
    assert result is not None
    for key in (
        "total_abs_diff",
        "avg_abs_diff",
        "total_signed_diff",
        "avg_abs_pct",
        "worst_tracked",
        "worst_scanned",
        "worst_signed_diff",
        "worst_abs_diff",
    ):
        assert math.isfinite(result[key])
