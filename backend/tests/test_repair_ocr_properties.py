"""Property-based tests for the repair-cost OCR service.

Covers ``backend.services.repair_ocr.RepairOcrService`` on two surfaces:

* ``_parse_cost``: a pure ``str -> float`` post-processor that normalises a
  comma decimal separator and stray ASCII spaces, then reads the first numeric
  run from arbitrary recogniser text. The properties below pin its safety
  envelope (never negative, never raises) and its comma-normalisation symmetry
  over arbitrary generated strings.
* ``scan_repair_cost``: the one-shot capture/OCR orchestration. Driven through
  its injected seams (``repair_region``, ``ScreenCapturer``, the ``local_ocr``
  engine) with lightweight fakes, so the region-dimension and capture-tap
  isolation guarantees are exercised without a live game client or the real
  recogniser. The region values are generated over the strictly-positive
  rectangles ``repair_region`` is documented to return (None or a positive-area
  ``(tl, br)`` pair), which is the only domain that reaches the capture.
"""

from __future__ import annotations

import math

import pytest
from hypothesis import given
from hypothesis import strategies as st

from backend.services.repair_ocr import RepairOcrService

parse = RepairOcrService._parse_cost


# ── _parse_cost: pure post-processor over arbitrary recogniser text ───────────

# Arbitrary text, biased towards the characters that actually shape the parse
# (digits, the comma/dot separators, ASCII spaces, sign and label noise) while
# still admitting the full unicode range so non-ASCII digits and separators are
# in scope.
_OCR_TEXT = st.one_of(
    st.text(max_size=40),
    st.text(alphabet="0123456789., -PEDcost", max_size=20),
)


@given(_OCR_TEXT)
def test_parse_cost_is_non_negative(text):
    """The parsed cost is never negative: the regex captures only an unsigned
    digit run, so a leading sign can never enter the converted value."""
    assert parse(text) >= 0.0


@given(_OCR_TEXT)
def test_parse_cost_never_raises(text):
    """``_parse_cost`` is total over ``str``: every input returns a float and
    none raises. The result may be non-finite for a pathological digit run, so
    this asserts only the no-raise / float-typed contract, not finiteness."""
    result = parse(text)
    assert isinstance(result, float)


@given(_OCR_TEXT)
def test_parse_cost_comma_normalisation_is_idempotent(text):
    """Pre-substituting ASCII commas for dots leaves the result unchanged: the
    service normalises ``,`` to ``.`` internally, so an already-substituted
    string reads identically to the original."""
    assert parse(text) == pytest.approx(parse(text.replace(",", ".")), nan_ok=True)


@given(st.text(alphabet="0123456789", min_size=1, max_size=9))
def test_parse_cost_reads_a_bare_digit_run_exactly(digits):
    """A pure ASCII-digit run (bounded so the float is exact) reads back as its
    own integer value: a concrete pin on the unsigned, finite happy path."""
    assert parse(digits) == pytest.approx(float(digits))


# ── scan_repair_cost: capture orchestration driven through its seams ──────────


class _SpyCapturer:
    """Stands in for ``ScreenCapturer``; records the dimensions it is asked to
    capture and returns a sentinel frame so the pipeline proceeds to OCR."""

    def __init__(self, calls):
        self._calls = calls

    def capture_region(self, x, y, width, height):
        self._calls.append((x, y, width, height))
        return object()  # opaque, non-None frame; the fake engine ignores it


class _FakeEngine:
    """Minimal stand-in for the bundled recogniser."""

    def read_text(self, frame):
        return "0.05", 0.99


def _install_seams(monkeypatch, region, calls, engine=None):
    """Wire the three seams ``scan_repair_cost`` reaches into: the region
    source, the screen capturer, and the local OCR engine."""
    monkeypatch.setattr("backend.services.repair_ocr.repair_region", lambda: region)
    monkeypatch.setattr(
        "backend.ocr.capturer.ScreenCapturer", lambda: _SpyCapturer(calls)
    )
    monkeypatch.setattr(
        "backend.services.local_ocr.get_engine",
        lambda: engine if engine is not None else _FakeEngine(),
    )


# A strictly-positive-area rectangle expressed as the ``(tl, br)`` pair
# ``repair_region`` returns; this is the only non-None shape it can produce
# (its own guard collapses any degenerate rect to None).
_COORD = st.integers(min_value=-5000, max_value=5000)
_EXTENT = st.integers(min_value=1, max_value=4000)


@st.composite
def _positive_regions(draw):
    tl_x = draw(_COORD)
    tl_y = draw(_COORD)
    w = draw(_EXTENT)
    h = draw(_EXTENT)
    return [tl_x, tl_y], [tl_x + w, tl_y + h]


@given(_positive_regions())
def test_scan_captures_only_positive_dimensions(region):
    """``scan_repair_cost`` never asks the capturer for a non-positive region:
    a positive-area region from ``repair_region`` reaches ``capture_region``
    with strictly positive width and height."""
    # A context-managed MonkeyPatch (rather than the function-scoped fixture)
    # so each generated input applies and undoes the seam patches cleanly.
    calls: list[tuple[int, int, int, int]] = []
    with pytest.MonkeyPatch.context() as mp:
        _install_seams(mp, region, calls)
        result = RepairOcrService(config_service=None).scan_repair_cost()

    assert "error" not in result
    assert len(calls) == 1
    _x, _y, w, h = calls[0]
    assert w > 0
    assert h > 0


@given(_positive_regions())
def test_capture_tap_failure_does_not_block_the_read(region):
    """A capture tap that raises is isolated: the failure is swallowed and the
    OCR read still runs, so a parsed cost is returned rather than an error."""
    calls: list[tuple[int, int, int, int]] = []
    with pytest.MonkeyPatch.context() as mp:
        _install_seams(mp, region, calls)
        service = RepairOcrService(config_service=None)

        def _boom(panel, region_dict, frame):
            raise RuntimeError("tap failure")

        service.set_capture_tap(_boom)
        result = service.scan_repair_cost()

    # The read proceeded past the failed tap: no error surfaced and the fake
    # engine's reading was parsed through.
    assert "error" not in result
    assert result["cost_ped"] == pytest.approx(0.05)
    assert math.isfinite(result["confidence"])


def test_injected_capturer_factory_wins_over_module_symbol():
    """A constructor-injected capturer factory supplies the frame source.

    The composition root wires a fixture-backed factory under test mode; the
    module ``ScreenCapturer`` symbol (patched here to the spy) must never be
    resolved, and the read proceeds off the injected capturer's frame.
    """
    spy_calls: list[tuple[int, int, int, int]] = []
    factory_served: list[tuple[int, int, int, int]] = []

    class _Injected:
        def capture_region(self, x, y, width, height):
            factory_served.append((x, y, width, height))
            return object()

    with pytest.MonkeyPatch.context() as mp:
        _install_seams(mp, ([0, 0], [10, 10]), spy_calls)
        service = RepairOcrService(config_service=None, capturer_factory=_Injected)
        result = service.scan_repair_cost()

    assert "error" not in result
    assert result["cost_ped"] == pytest.approx(0.05)
    assert factory_served == [(0, 0, 10, 10)]
    assert spy_calls == []  # the module-symbol spy was never used
