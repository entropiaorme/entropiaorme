"""Injectable clock for harness tests.

Production services that read wall-clock or monotonic time go through
a ``Clock`` so test scenarios can freeze and advance time
deterministically. The first round lands the interface and the two
concrete implementations; subsequent rounds wire individual services
to take a ``Clock`` at construction.
"""

from __future__ import annotations

import time as _time
from abc import ABC, abstractmethod
from datetime import datetime, timedelta, timezone


class Clock(ABC):
    """Abstract time source. Production uses ``RealClock``; tests use
    ``MockClock``."""

    @abstractmethod
    def now(self) -> datetime:
        """Return the current wall-clock instant.

        Naive by default (matches the existing ``datetime.now()``
        callers in services); rounds that want timezone-awareness opt
        in by passing ``tz`` at construction.
        """

    @abstractmethod
    def monotonic(self) -> float:
        """Return monotonic seconds since an arbitrary epoch.

        Matches ``time.monotonic`` semantics: only deltas are
        meaningful; the absolute value is opaque.
        """


class RealClock(Clock):
    """Production clock. Delegates directly to the stdlib."""

    def __init__(self, tz: timezone | None = None):
        """Build a production clock with an optional fixed timezone."""
        self._tz = tz

    def now(self) -> datetime:
        """Return ``datetime.now(tz=self._tz)`` verbatim."""
        return datetime.now(tz=self._tz)

    def monotonic(self) -> float:
        """Return ``time.monotonic()`` verbatim."""
        return _time.monotonic()


class MockClock(Clock):
    """Test clock. Frozen by default; advanced explicitly by the
    scenario.

    The wall-clock and monotonic streams advance in lockstep when
    ``advance()`` is called; ``freeze_at()`` resets only the
    wall-clock stream so monotonic counts are preserved as the
    scenario walks through several frozen instants.
    """

    def __init__(
        self,
        start: datetime | None = None,
        monotonic_start: float = 0.0,
    ):
        """Initialise the clock at ``start`` (defaults to 2026-01-01)
        with the monotonic stream at ``monotonic_start``."""
        self._now = start if start is not None else datetime(2026, 1, 1, 0, 0, 0)
        self._monotonic = monotonic_start

    def now(self) -> datetime:
        """Return the current frozen wall-clock instant."""
        return self._now

    def monotonic(self) -> float:
        """Return the current monotonic stream value."""
        return self._monotonic

    def advance(self, seconds: float) -> None:
        """Advance both wall-clock and monotonic streams by ``seconds``.

        Negative values are rejected with ``ValueError`` to preserve
        the monotonic-stream invariant; use ``freeze_at()`` if the
        scenario needs to set the wall-clock stream to an arbitrary
        instant without touching the monotonic stream.
        """
        if seconds < 0:
            raise ValueError(
                f"MockClock.advance() rejects negative deltas (got {seconds}); "
                "use freeze_at() to set the wall-clock stream without "
                "moving the monotonic stream backwards."
            )
        self._now = self._now + timedelta(seconds=seconds)
        self._monotonic += seconds

    def freeze_at(self, ts: datetime) -> None:
        """Reset the wall-clock stream to ``ts``.

        The monotonic stream is preserved; use ``advance()`` to bump
        both together.
        """
        self._now = ts
