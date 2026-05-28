"""Spacebar-capture listener: optional hook for hands-free capture.

When the toggle in the scan overlay is enabled, the listener consumes a
:class:`backend.testing.keystroke_source.KeystrokeSource` (production:
``PynputKeystrokeSource`` filtered to ``{"space"}`` at the OS-hook
boundary; tests: ``MockKeystrokeSource``). On a press-edge (auto-repeat
suppressed via on_release tracking) it dispatches ``capture_current_page``
on the skill scan if it is currently in the ``capturing`` phase. Idle:
no-op.

Listening is pass-through (the press is not consumed), so the EU client
still receives the keystroke as normal. Scope is the capture-listener
toggle: the source is started only while enabled, and torn down at
shutdown or on disable. The OS-hook-boundary filter enforces the input
minimisation policy structurally; see https://entropiaorme.com/privacy.
"""

from __future__ import annotations

import logging
import threading
from typing import Any

from backend.testing.keystroke_source import KeystrokeEvent, KeystrokeSource

log = logging.getLogger(__name__)


class SpacebarCaptureListener:
    """Owns the optional space-key listener that fires manual-scan capture."""

    def __init__(
        self,
        skill_scan_manual: Any,
        keystroke_source: KeystrokeSource | None = None,
    ) -> None:
        self._skill = skill_scan_manual

        self._enabled = False
        self._source_running = False
        self._space_down = False

        # Keystroke source. Production: PynputKeystrokeSource(key_allowlist={"space"}).
        # Tests: MockKeystrokeSource. None leaves the listener inert.
        self._keystroke_source = keystroke_source
        if keystroke_source is not None:
            keystroke_source.subscribe(self._on_keystroke)

        # Optional keystroke observer. None in normal operation; set by the
        # recording controller to copy space press/release edges into a bundle.
        # Called as tap(key: str, kind: str).
        self._key_tap = None

    @property
    def is_running(self) -> bool:
        return self._source_running

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
            self._start_source()
        else:
            self._stop_source()

    def stop(self) -> None:
        """Tear down; used at shutdown."""
        self._enabled = False
        self._stop_source()

    # ------------------------------------------------------------------

    def _is_capturing(self) -> bool:
        return self._skill.get_status().get("phase") == "capturing"

    def _start_source(self) -> None:
        if self._keystroke_source is None or self._source_running:
            return
        try:
            self._keystroke_source.start()
            self._source_running = True
            log.info("Spacebar capture listener: started")
        except Exception:
            log.exception("Failed to start spacebar keystroke source")

    def _stop_source(self) -> None:
        if self._keystroke_source is None or not self._source_running:
            return
        try:
            self._keystroke_source.stop()
        finally:
            self._source_running = False
            self._space_down = False
            log.info("Spacebar capture listener: stopped")

    def _on_keystroke(self, event: KeystrokeEvent) -> None:
        """Handle one space-key event from the source."""
        if not self._source_running:
            return
        if event.key != "space":
            return
        if event.kind == "press":
            self._on_space_press()
        elif event.kind == "release":
            self._on_space_release()

    def _on_space_press(self) -> None:
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
        # Off-thread to keep the dispatch callback cheap.
        threading.Thread(
            target=self._skill.capture_current_page,
            name="spacebar-capture",
            daemon=True,
        ).start()

    def _on_space_release(self) -> None:
        self._space_down = False
        tap = self._key_tap
        if tap is not None:
            try:
                tap("space", "release")
            except Exception:
                log.exception("Keystroke tap failed")
