"""Keystroke source abstraction for harness tests.

Production listeners (``HotbarListener``, ``SpacebarCaptureListener``)
currently call into ``pynput`` directly. A ``KeystrokeSource`` interface
that those listeners depend on is extracted as that surface is built
out; tests inject a ``MockKeystrokeSource`` and dispatch synthetic key
events.

This lands the interface and the mock so scenarios can be built against
a stable shape. The production wire-through follows when the seam is
extracted from the listeners.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime
from typing import Literal

KeystrokeKind = Literal["press", "release"]


@dataclass(frozen=True)
class KeystrokeEvent:
    """One observed keystroke.

    Attributes
    ----------
    key:
        Human-readable key identifier (``"1"``, ``"F1"``, ``"space"``);
        shape matches the hotbar / F-Spam vocabulary the listeners
        speak.
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
    def subscribe(self, callback: KeystrokeCallback) -> None:
        """Register ``callback`` to receive every dispatched event."""

    @abstractmethod
    def start(self) -> None:
        """Begin delivering events to subscribers."""

    @abstractmethod
    def stop(self) -> None:
        """Halt delivery; subscribers remain registered for the next
        ``start()`` call."""


class MockKeystrokeSource(KeystrokeSource):
    """Test-mode keystroke source: dispatches injected events to
    subscribers.

    Events injected before ``start()`` (or after ``stop()``) are
    silently dropped so scenario-script ordering matches the
    production listener's "events only flow while running" contract.
    """

    def __init__(self) -> None:
        """Build an idle mock with no subscribers and no pending events."""
        self._callbacks: list[KeystrokeCallback] = []
        self._running = False

    def subscribe(self, callback: KeystrokeCallback) -> None:
        """Append ``callback`` to the dispatch list."""
        self._callbacks.append(callback)

    def start(self) -> None:
        """Mark the source as running so injected events propagate."""
        self._running = True

    def stop(self) -> None:
        """Mark the source as halted; subsequent ``inject()`` calls
        are dropped silently."""
        self._running = False

    def inject(
        self,
        key: str,
        timestamp: datetime,
        kind: KeystrokeKind = "press",
    ) -> None:
        """Dispatch a synthetic keystroke to all subscribers.

        No-op when the source is halted. Constructs a frozen
        ``KeystrokeEvent`` and delivers it to every registered callback
        in registration order.
        """
        if not self._running:
            return
        event = KeystrokeEvent(key=key, timestamp=timestamp, kind=kind)
        for callback in self._callbacks:
            callback(event)
