"""Keystroke source abstraction for harness tests.

Production listeners (``HotbarListener``, ``SpacebarCaptureListener``)
currently call into ``pynput`` directly. The R6 round extracts a
``KeystrokeSource`` interface that those listeners depend on; tests
inject a ``MockKeystrokeSource`` and dispatch synthetic key events.

R1 lands the interface and the mock so downstream rounds can build
scenarios against a stable shape. Production wire-through follows in
the round that extracts the seam from the listeners.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from datetime import datetime
from typing import Callable, Literal


KeystrokeKind = Literal["press", "release"]


@dataclass(frozen=True)
class KeystrokeEvent:
    """One observed keystroke.

    Attributes
    ----------
    key:
        Human-readable key identifier ("1", "F1", "space"); shape
        matches the hotbar / F-Spam vocabulary the listeners speak.
    timestamp:
        When the event occurred. In production this is wall-clock; in
        tests it is whatever the scenario chose.
    kind:
        ``"press"`` or ``"release"``.
    """

    key: str
    timestamp: datetime
    kind: KeystrokeKind = "press"


KeystrokeCallback = Callable[[KeystrokeEvent], None]


class KeystrokeSource(ABC):
    """Abstract source of keystroke events.

    Subscribers register a callback; ``start()`` begins delivering
    events; ``stop()`` halts delivery. Production wraps ``pynput``;
    tests use ``MockKeystrokeSource`` and call ``inject()`` directly.
    """

    @abstractmethod
    def subscribe(self, callback: KeystrokeCallback) -> None: ...

    @abstractmethod
    def start(self) -> None: ...

    @abstractmethod
    def stop(self) -> None: ...


class MockKeystrokeSource(KeystrokeSource):
    """Test-mode keystroke source — dispatches injected events to
    subscribers.

    Events injected before ``start()`` (or after ``stop()``) are
    silently dropped so scenario-script ordering matches the
    production listener's "events only flow while running" contract.
    """

    def __init__(self) -> None:
        self._callbacks: list[KeystrokeCallback] = []
        self._running = False

    def subscribe(self, callback: KeystrokeCallback) -> None:
        self._callbacks.append(callback)

    def start(self) -> None:
        self._running = True

    def stop(self) -> None:
        self._running = False

    def inject(
        self,
        key: str,
        timestamp: datetime,
        kind: KeystrokeKind = "press",
    ) -> None:
        """Dispatch a synthetic keystroke to all subscribers."""
        if not self._running:
            return
        event = KeystrokeEvent(key=key, timestamp=timestamp, kind=kind)
        for callback in self._callbacks:
            callback(event)
