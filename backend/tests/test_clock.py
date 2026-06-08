"""Tests for the injectable harness clock.

Pins the two ``Clock`` implementations: ``RealClock`` delegates to the
stdlib, and ``MockClock`` freezes and advances deterministically so a
scenario can walk wall-clock and monotonic streams without real time.
"""

from __future__ import annotations

from datetime import UTC, datetime

import pytest

from backend.testing.clock import Clock, MockClock, RealClock


class TestRealClock:
    """The production clock delegates straight to the stdlib."""

    def test_now_is_naive_by_default(self):
        clock = RealClock()
        assert clock.now().tzinfo is None

    def test_now_honours_a_fixed_timezone(self):
        clock = RealClock(tz=UTC)
        assert clock.now().tzinfo is UTC

    def test_monotonic_is_non_decreasing(self):
        clock = RealClock()
        first = clock.monotonic()
        second = clock.monotonic()
        assert isinstance(first, float)
        assert second >= first

    def test_is_a_clock(self):
        assert isinstance(RealClock(), Clock)


class TestMockClock:
    """The test clock is frozen by default and advanced explicitly."""

    def test_default_start_is_fixed_and_frozen(self):
        clock = MockClock()
        assert clock.now() == datetime(2026, 1, 1, 0, 0, 0)
        # Reading twice without advancing returns the same frozen instant.
        assert clock.now() == clock.now()
        assert clock.monotonic() == 0.0

    def test_custom_start_and_monotonic(self):
        start = datetime(2030, 6, 15, 12, 30, 0)
        clock = MockClock(start=start, monotonic_start=100.0)
        assert clock.now() == start
        assert clock.monotonic() == 100.0

    def test_advance_moves_both_streams_in_lockstep(self):
        clock = MockClock(start=datetime(2026, 1, 1), monotonic_start=10.0)
        clock.advance(2.5)
        assert clock.now() == datetime(2026, 1, 1, 0, 0, 2, 500000)
        assert clock.monotonic() == 12.5

    def test_advance_rejects_negative_delta(self):
        clock = MockClock()
        with pytest.raises(ValueError, match="negative deltas"):
            clock.advance(-1.0)

    def test_freeze_at_resets_wallclock_preserving_monotonic(self):
        clock = MockClock(start=datetime(2026, 1, 1), monotonic_start=5.0)
        clock.advance(3.0)
        assert clock.monotonic() == 8.0

        clock.freeze_at(datetime(2027, 12, 31, 23, 59, 59))
        # Wall-clock jumps; monotonic stream is untouched.
        assert clock.now() == datetime(2027, 12, 31, 23, 59, 59)
        assert clock.monotonic() == 8.0

    def test_is_a_clock(self):
        assert isinstance(MockClock(), Clock)


class TestBuildClock:
    """The whole-process clock seam parses ``ENTROPIA_TEST_CLOCK_START``."""

    def test_rejects_timezone_aware_start(self, monkeypatch):
        """An aware instant is refused.

        ``MockClock``'s later ``.timestamp()`` conversion would reinterpret an
        aware value (UTC vs host-local), silently shifting replay semantics
        away from the naive plan, so the seam rejects it (matching the
        naive-only guard in ``clock_plan``).
        """
        from backend.main import _build_clock

        monkeypatch.setenv("ENTROPIA_TEST_CLOCK_START", "2026-01-01T00:00:00+00:00")
        with pytest.raises(RuntimeError, match="must be a naive ISO-8601 instant"):
            _build_clock()

    def test_accepts_naive_start_and_freezes(self, monkeypatch):
        """A naive instant builds a ``MockClock`` frozen at that instant."""
        from backend.main import _build_clock

        monkeypatch.setenv("ENTROPIA_TEST_CLOCK_START", "2030-06-15T12:30:00")
        clock = _build_clock()
        assert isinstance(clock, MockClock)
        assert clock.now() == datetime(2030, 6, 15, 12, 30, 0)

    def test_unset_returns_real_clock(self, monkeypatch):
        """No override: production runs on the real clock."""
        from backend.main import _build_clock

        monkeypatch.delenv("ENTROPIA_TEST_CLOCK_START", raising=False)
        assert isinstance(_build_clock(), RealClock)
