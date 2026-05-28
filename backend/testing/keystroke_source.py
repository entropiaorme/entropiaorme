"""Keystroke source abstraction.

``HotbarListener`` and ``SpacebarCaptureListener`` consume a
``KeystrokeSource`` rather than calling ``pynput`` themselves;
production wires in ``PynputKeystrokeSource``, tests inject
``MockKeystrokeSource``.

The source also enforces the input-listening minimisation policy
structurally: a constructor-passed ``key_allowlist`` filters at the
OS-hook boundary so out-of-scope keystrokes never enter the
application's event stream in the first place.
"""

from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from collections.abc import Callable
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any, Literal

log = logging.getLogger(__name__)

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


def _pynput_key_name(key: Any) -> str | None:
    """Normalise a ``pynput`` key object to the source's string vocabulary.

    Alphanumerics return ``key.char`` (``"1"``, ``"a"``); named keys return
    ``key.name`` (``"space"``, ``"f1"``). Returns ``None`` when the key has
    neither attribute (an unmappable virtual key).
    """
    char = getattr(key, "char", None)
    if isinstance(char, str) and char:
        return char
    name = getattr(key, "name", None)
    if isinstance(name, str) and name:
        return name
    return None


class PynputKeystrokeSource(KeystrokeSource):
    """Production keystroke source backed by ``pynput``.

    An optional ``key_allowlist`` filters at the OS-hook boundary so
    only keys named in the set ever enter the dispatch path. This is
    how the input-listening minimisation policy is enforced structurally
    rather than each consumer trusting itself to filter after the fact.

    ``start()`` is idempotent (no-op when already running); ``stop()`` is
    likewise. ``ImportError`` on ``pynput`` (e.g. headless CI) leaves
    the source inert with a single warning; this matches the prior
    listener behaviour where missing ``pynput`` disabled the feature
    without crashing the app.
    """

    def __init__(self, key_allowlist: set[str] | None = None) -> None:
        """Build an idle source. ``key_allowlist=None`` admits every key."""
        self._allowlist: set[str] | None = (
            set(key_allowlist) if key_allowlist is not None else None
        )
        self._callbacks: list[KeystrokeCallback] = []
        # pynput's keyboard.Listener (untyped C-extension), or None when stopped.
        self._listener: Any = None

    def subscribe(self, callback: KeystrokeCallback) -> None:
        """Append ``callback`` to the dispatch list."""
        self._callbacks.append(callback)

    def start(self) -> None:
        """Begin observing the OS keyboard hook; idempotent.

        Allow-list filtering applies inside the ``pynput`` callbacks so
        non-admitted keys never reach a subscriber. ``ImportError`` is
        logged at WARNING and the source stays inert (legacy behaviour
        preserved from the pre-seam listeners).
        """
        if self._listener is not None:
            return
        try:
            from pynput import keyboard
        except ImportError:
            log.warning(
                "pynput not installed; keystroke source inert. "
                "Install with: pip install pynput"
            )
            return

        def on_press(key: Any) -> None:
            self._dispatch(key, "press")

        def on_release(key: Any) -> None:
            self._dispatch(key, "release")

        listener = keyboard.Listener(on_press=on_press, on_release=on_release)
        listener.daemon = True
        listener.start()
        self._listener = listener

    def stop(self) -> None:
        """Halt OS-hook observation; subscribers remain registered."""
        if self._listener is None:
            return
        self._listener.stop()
        self._listener = None

    def _dispatch(self, key: Any, kind: KeystrokeKind) -> None:
        """Filter against the allow-list and deliver to subscribers."""
        name = _pynput_key_name(key)
        if name is None:
            return
        if self._allowlist is not None and name not in self._allowlist:
            return
        event = KeystrokeEvent(
            key=name,
            timestamp=datetime.now(UTC),
            kind=kind,
        )
        for callback in self._callbacks:
            try:
                callback(event)
            except Exception:
                log.exception("Keystroke callback failed")
