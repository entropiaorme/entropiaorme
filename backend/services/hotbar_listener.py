"""Hotbar key listener — observes hotbar slot keypresses for active-tool attribution.

Resolves number-key presses against the configured hotbar to publish
active-tool / heal-tool / consumable events.

The listener runs only while the capability toggle is on and a tracking
session is active; otherwise the listener thread is torn down.
"""

import logging
from typing import Any

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)

log = logging.getLogger(__name__)

# Hotbar slot keys: number row 1-9 and 0.
HOTBAR_SLOT_KEYS = {str(i) for i in list(range(1, 10)) + [0]}


class HotbarListener:
    """Owns the hotbar slot-key listener."""

    def __init__(
        self,
        event_bus: EventBus,
        hotbar_resolver=None,
    ):
        self._event_bus = event_bus
        self._hotbar_hooks_enabled = False
        self._session_active = False

        # Hotbar resolver: callable(slot_key: str) -> (name, cost, item_type, reload_s) | None
        self._hotbar_resolver = hotbar_resolver

        # pynput's keyboard.Listener (untyped C-extension), or None when stopped.
        self._key_listener: Any = None

        # Optional keystroke observer. None in normal operation; set by the
        # recording controller to copy hotbar-slot presses into a bundle.
        # Called as tap(key: str, kind: str).
        self._key_tap = None

        event_bus.subscribe(EVENT_SESSION_STARTED, self._on_session_started)
        event_bus.subscribe(EVENT_SESSION_STOPPED, self._on_session_stopped)

    @property
    def is_running(self) -> bool:
        """True if the pynput listener is currently active."""
        return self._key_listener is not None

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
        """Enable or disable the pynput hotbar-slot listener.

        The listener still only runs when a tracking session is active.
        """
        self._hotbar_hooks_enabled = enabled
        self._reconcile()

    def stop(self) -> None:
        """Tear down — used at shutdown."""
        self._event_bus.unsubscribe(EVENT_SESSION_STARTED, self._on_session_started)
        self._event_bus.unsubscribe(EVENT_SESSION_STOPPED, self._on_session_stopped)
        self._stop_key_listener()
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
        """Start or stop the listener based on the current gate state."""
        if self._hotbar_hooks_enabled and self._session_active:
            self._start_key_listener()
        else:
            self._stop_key_listener()

    # ------------------------------------------------------------------
    # Hotbar-slot key listener
    # ------------------------------------------------------------------

    def _start_key_listener(self):
        """Listen for hotbar slot keypresses and resolve via hotbar."""
        if self._key_listener is not None:
            return
        try:
            from pynput import keyboard

            def on_press(key):
                try:
                    ch = key.char
                except AttributeError:
                    return
                if ch in HOTBAR_SLOT_KEYS:
                    tap = self._key_tap
                    if tap is not None:
                        try:
                            tap(ch, "press")
                        except Exception:
                            log.exception("Keystroke tap failed")
                    if self._hotbar_resolver:
                        # Keep on_press short — do all work off the listener thread.
                        import threading

                        threading.Thread(
                            target=self._resolve_hotbar_slot,
                            args=(ch,),
                            daemon=True,
                        ).start()

            self._key_listener = keyboard.Listener(on_press=on_press)
            self._key_listener.daemon = True
            self._key_listener.start()
        except ImportError:
            log.warning(
                "pynput not installed — hotbar detection disabled. "
                "Install with: pip install pynput"
            )
        except Exception as e:
            log.warning("Failed to start key listener: %s", e)

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

    def _stop_key_listener(self):
        if self._key_listener:
            self._key_listener.stop()
            self._key_listener = None
