"""Spacebar-capture listener: optional pynput hook for hands-free capture.

When the toggle in the scan overlay is enabled, a pynput keyboard listener
observes the space key. On a press-edge (auto-repeat suppressed via
on_release tracking) it dispatches ``capture_current_page`` on the skill
scan if it is currently in the ``capturing`` phase. Idle: no-op.

Listening is pass-through (the press is not consumed), so the EU client
still receives the keystroke as normal. Scope is the capture-listener
toggle: the listener thread is registered only while enabled, and torn
down at shutdown or on disable. The hook is gated to the capture surface
that consumes it; see https://entropiaorme.com/privacy for the
input-listening privacy posture.
"""

from __future__ import annotations

import logging
import threading
from typing import Any

log = logging.getLogger(__name__)


class SpacebarCaptureListener:
    """Owns the optional space-key listener that fires manual-scan capture."""

    def __init__(self, skill_scan_manual: Any) -> None:
        self._skill = skill_scan_manual

        self._enabled = False
        self._key_listener = None
        self._space_down = False

        # Optional keystroke observer. None in normal operation; set by the
        # recording controller to copy space press/release edges into a bundle.
        # Called as tap(key: str, kind: str).
        self._key_tap = None

    @property
    def is_running(self) -> bool:
        return self._key_listener is not None

    def set_key_tap(self, tap) -> None:
        """Install a keystroke observer (called for each space press/release edge)."""
        self._key_tap = tap

    def clear_key_tap(self) -> None:
        """Remove the keystroke observer."""
        self._key_tap = None

    @property
    def is_enabled(self) -> bool:
        return self._enabled

    def set_enabled(self, enabled: bool) -> None:
        """Toggle the listener; idempotent."""
        if self._enabled == enabled:
            return
        self._enabled = enabled
        if enabled:
            self._start_key_listener()
        else:
            self._stop_key_listener()

    def stop(self) -> None:
        """Tear down; used at shutdown."""
        self._enabled = False
        self._stop_key_listener()

    # ------------------------------------------------------------------

    def _is_capturing(self) -> bool:
        return self._skill.get_status().get("phase") == "capturing"

    def _start_key_listener(self) -> None:
        if self._key_listener is not None:
            return
        try:
            from pynput import keyboard

            def on_press(key):
                if key != keyboard.Key.space:
                    return
                # Auto-repeat suppression; only the first press-edge fires.
                if self._space_down:
                    return
                self._space_down = True
                tap = self._key_tap
                if tap is not None:
                    try:
                        tap("space", "press")
                    except Exception:
                        log.exception("Keystroke tap failed")
                if not self._is_capturing():
                    return
                # Off-thread to keep the hook callback cheap.
                threading.Thread(
                    target=self._skill.capture_current_page,
                    name="spacebar-capture",
                    daemon=True,
                ).start()

            def on_release(key):
                if key == keyboard.Key.space:
                    self._space_down = False
                    tap = self._key_tap
                    if tap is not None:
                        try:
                            tap("space", "release")
                        except Exception:
                            log.exception("Keystroke tap failed")

            self._key_listener = keyboard.Listener(
                on_press=on_press,
                on_release=on_release,
            )
            self._key_listener.daemon = True
            self._key_listener.start()
            log.info("Spacebar capture listener: started")
        except ImportError:
            log.warning(
                "pynput not installed; spacebar capture disabled. "
                "Install with: pip install pynput"
            )
        except Exception as exc:
            log.warning("Failed to start spacebar capture listener: %s", exc)

    def _stop_key_listener(self) -> None:
        if self._key_listener:
            self._key_listener.stop()
            self._key_listener = None
            self._space_down = False
            log.info("Spacebar capture listener: stopped")
