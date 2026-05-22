"""Injectable clock for harness tests.

Production services that read wall-clock or monotonic time go through
a ``Clock`` so test scenarios can freeze and advance time
deterministically. R1 lands the interface and the two concrete
implementations; subsequent rounds wire individual services to take a
``Clock`` at construction.
"""

from __future__ import annotations

import time as _time
from abc import ABC, abstractmethod
from datetime import datetime, timedelta, timezone


class Clock(ABC):
    """Abstract time source — production uses ``RealClock``, tests
    use ``MockClock``."""

    @abstractmethod
    def now(self) -> datetime:
        """Current wall-clock instant. Naive (matches the existing
        ``datetime.now()`` callers in services); rounds that want
        timezone-awareness opt in by passing ``tz`` at construction."""

    @abstractmethod
    def monotonic(self) -> float:
        """Monotonic seconds since an arbitrary epoch (matches
        ``time.monotonic`` semantics)."""


class RealClock(Clock):
    """Production clock — delegates to the stdlib."""

    def __init__(self, tz: timezone | None = None):
        self._tz = tz

    def now(self) -> datetime:
        return datetime.now(tz=self._tz)

    def monotonic(self) -> float:
        return _time.monotonic()


class MockClock(Clock):
    """Test clock — frozen by default, advanced explicitly by the
    scenario.

    The wall-clock and monotonic streams advance in lockstep when
    ``advance()`` is called; ``freeze_at()`` only resets the wall-clock
    stream so monotonic counts are preserved as the scenario walks
    through several "frozen" instants.
    """

    def __init__(
        self,
        start: datetime | None = None,
        monotonic_start: float = 0.0,
    ):
        self._now = start if start is not None else datetime(2026, 1, 1, 0, 0, 0)
        self._monotonic = monotonic_start

    def now(self) -> datetime:
        return self._now

    def monotonic(self) -> float:
        return self._monotonic

    def advance(self, seconds: float) -> None:
        """Advance both wall-clock and monotonic streams by ``seconds``."""
        self._now = self._now + timedelta(seconds=seconds)
        self._monotonic += seconds

    def freeze_at(self, ts: datetime) -> None:
        """Reset the wall-clock stream to ``ts``; monotonic stream is
        preserved (use ``advance()`` to bump both)."""
        self._now = ts
