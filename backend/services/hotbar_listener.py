"""Hotbar key listener — observes hotbar slot keypresses for active-tool attribution.

Resolves number-key presses against the configured hotbar to publish
active-tool / heal-tool / consumable events.

The listener consumes a :class:`backend.testing.keystroke_source.KeystrokeSource`:
production wires in :class:`PynputKeystrokeSource`, tests inject
:class:`MockKeystrokeSource`. The listener gates the source's lifecycle on
the capability toggle and an active tracking session, exactly as before.
"""

import logging
import threading

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.testing.keystroke_source import KeystrokeEvent, KeystrokeSource

log = logging.getLogger(__name__)

# Hotbar slot keys: number row 1-9 and 0.
HOTBAR_SLOT_KEYS = {str(i) for i in list(range(1, 10)) + [0]}


class HotbarListener:
    """Owns the hotbar slot-key listener."""

    def __init__(
        self,
        event_bus: EventBus,
        keystroke_source: KeystrokeSource | None = None,
        hotbar_resolver=None,
    ):
        self._event_bus = event_bus
        self._hotbar_hooks_enabled = False
        self._session_active = False

        # Hotbar resolver: callable(slot_key: str) -> (name, cost, item_type, reload_s) | None
        self._hotbar_resolver = hotbar_resolver

        # Keystroke source. Production: a PynputKeystrokeSource filtered to
        # HOTBAR_SLOT_KEYS at the OS-hook boundary. Tests: a MockKeystrokeSource.
        # None leaves the listener inert (matches the pre-seam ImportError path).
        self._keystroke_source = keystroke_source
        if keystroke_source is not None:
            keystroke_source.subscribe(self._on_keystroke)
        self._source_running = False

        # Optional keystroke observer. None in normal operation; set by the
        # recording controller to copy hotbar-slot presses into a bundle.
        # Called as tap(key: str, kind: str).
        self._key_tap = None

        event_bus.subscribe(EVENT_SESSION_STARTED, self._on_session_started)
        event_bus.subscribe(EVENT_SESSION_STOPPED, self._on_session_stopped)

    @property
    def is_running(self) -> bool:
        """True when the keystroke source is currently delivering events."""
        return self._source_running

    def set_key_tap(self, tap) -> None:
        """Install a keystroke observer (called for each hotbar-slot press)."""
        self._key_tap = tap

    def clear_key_tap(self) -> None:
        """Remove the keystroke observer."""
        self._key_tap = None

    # ------------------------------------------------------------------
    # Capability toggle
    # ------------------------------------------------------------------

    def apply_config(self, *, hotbar_hooks_enabled: bool) -> None:
        """Apply the hotbar capability toggle."""
        self.set_hotbar_hooks_enabled(hotbar_hooks_enabled)

    def set_hotbar_hooks_enabled(self, enabled: bool) -> None:
        """Enable or disable hotbar-slot listening.

        The keystroke source still only runs when a tracking session is active.
        """
        self._hotbar_hooks_enabled = enabled
        self._reconcile()

    def stop(self) -> None:
        """Tear down — used at shutdown."""
        self._event_bus.unsubscribe(EVENT_SESSION_STARTED, self._on_session_started)
        self._event_bus.unsubscribe(EVENT_SESSION_STOPPED, self._on_session_stopped)
        self._stop_source()
        self._hotbar_hooks_enabled = False
        self._session_active = False

    # ------------------------------------------------------------------
    # Session lifecycle
    # ------------------------------------------------------------------

    def _on_session_started(self, _payload):
        self._session_active = True
        self._reconcile()

    def _on_session_stopped(self, _payload):
        self._session_active = False
        self._reconcile()

    def _reconcile(self):
        """Start or stop the keystroke source based on the current gate state."""
        if self._hotbar_hooks_enabled and self._session_active:
            self._start_source()
        else:
            self._stop_source()

    # ------------------------------------------------------------------
    # Keystroke source lifecycle + dispatch
    # ------------------------------------------------------------------

    def _start_source(self) -> None:
        if self._keystroke_source is None or self._source_running:
            return
        try:
            self._keystroke_source.start()
            self._source_running = True
        except Exception:
            log.exception("Failed to start hotbar keystroke source")

    def _stop_source(self) -> None:
        if self._keystroke_source is None or not self._source_running:
            return
        try:
            self._keystroke_source.stop()
        finally:
            self._source_running = False

    def _on_keystroke(self, event: KeystrokeEvent) -> None:
        """Handle one keystroke from the source. No-op when paused or filtered."""
        if not self._source_running:
            return
        if event.kind != "press":
            return
        ch = event.key
        if ch not in HOTBAR_SLOT_KEYS:
            return
        tap = self._key_tap
        if tap is not None:
            try:
                tap(ch, "press")
            except Exception:
                log.exception("Keystroke tap failed")
        if self._hotbar_resolver:
            # Keep dispatch short — do all work off the listener thread.
            threading.Thread(
                target=self._resolve_hotbar_slot,
                args=(ch,),
                daemon=True,
            ).start()

    def _resolve_hotbar_slot(self, slot: str):
        """Resolve a hotbar slot and publish tool change. Runs off the hook thread."""
        result = self._hotbar_resolver(slot)
        if result:
            name, cost, item_type, reload_s = result
            log.debug("Hotbar slot %s: %r (%s)", slot, name, item_type)
            if item_type == "healing":
                self._event_bus.publish(
                    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
                    {
                        "tool_name": name,
                        "cost_per_use_ped": cost,
                        "reload_seconds": reload_s,
                        "source": f"hotbar:{slot}",
                    },
                )
            elif item_type == "consumable":
                # Consumables are one-off actions — do NOT switch the active
                # weapon in cost tracking.
                pass
            else:
                self._event_bus.publish(
                    EVENT_ACTIVE_TOOL_CHANGED,
                    {
                        "tool_name": name,
                        "source": f"hotbar:{slot}",
                    },
                )
