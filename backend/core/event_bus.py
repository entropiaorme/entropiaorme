"""Synchronous in-process event dispatch."""

from __future__ import annotations

import logging
import threading
from collections.abc import Callable
from typing import Any

log = logging.getLogger(__name__)


class EventBus:
    """Thread-safe pub/sub for app services that share process memory."""

    def __init__(self):
        self._subscribers: dict[str, list[Callable[[Any], None]]] = {}
        # Full-stream observers: called with every (event_type, data) pair
        # that crosses publish(), regardless of topic. Subscription is
        # per-topic by design, so a tap is the only supported way to observe
        # the complete publish stream (new topics included) without
        # monkeypatching publish itself.
        self._taps: list[Callable[[str, Any], None]] = []
        self._lock = threading.RLock()

    def subscribe(self, event_type: str, callback: Callable[[Any], None]) -> None:
        with self._lock:
            callbacks = self._subscribers.setdefault(event_type, [])
            if callback not in callbacks:
                callbacks.append(callback)

    def unsubscribe(self, event_type: str, callback: Callable[[Any], None]) -> None:
        with self._lock:
            callbacks = self._subscribers.get(event_type)
            if not callbacks:
                return
            try:
                callbacks.remove(callback)
            except ValueError:
                return
            if not callbacks:
                self._subscribers.pop(event_type, None)

    def has_subscribers(self, event_type: str) -> bool:
        with self._lock:
            return bool(self._subscribers.get(event_type))

    def add_tap(self, tap: Callable[[str, Any], None]) -> None:
        """Install a full-stream observer.

        The tap runs synchronously on the publisher's thread for every
        publish, before subscriber dispatch, and sees the payload object
        unchanged. Taps must be fast; the bus contains their exceptions the
        same way it contains subscriber exceptions, so a failing tap cannot
        break dispatch.
        """
        with self._lock:
            if tap not in self._taps:
                self._taps.append(tap)

    def remove_tap(self, tap: Callable[[str, Any], None]) -> None:
        """Remove a previously installed full-stream observer."""
        with self._lock:
            try:
                self._taps.remove(tap)
            except ValueError:
                return

    def publish(self, event_type: str, data: Any = None) -> None:
        with self._lock:
            taps = tuple(self._taps)
            callbacks = tuple(self._subscribers.get(event_type, ()))

        for tap in taps:
            try:
                tap(event_type, data)
            except Exception:
                log.exception("Event tap failed for %s", event_type)

        for callback in callbacks:
            try:
                callback(data)
            except Exception:
                log.exception("Event subscriber failed for %s", event_type)
