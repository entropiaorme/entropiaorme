"""Unit tests for ``SpacebarCaptureListener`` driven by ``MockKeystrokeSource``.

The listener's contract under the seam:

1. Source lifecycle follows the ``set_enabled`` toggle.
2. A ``space`` press while the skill scan is ``capturing`` dispatches
   ``capture_current_page`` (off-thread).
3. Auto-repeat is suppressed: only the first press-edge fires.
4. The recorder tap receives both press and release edges.

Tests drive the listener with a ``MockKeystrokeSource`` and a fake
skill-scan-manual that records its capture invocations.
"""

from __future__ import annotations

import threading
from datetime import datetime

from backend.services.spacebar_capture_listener import SpacebarCaptureListener
from backend.testing.keystroke_source import MockKeystrokeSource

_TS = datetime(2026, 5, 28, 12, 0, 0)


class _FakeSkillScan:
    """Test double for SkillScanManual.

    Records each ``capture_current_page`` call. ``set_phase`` toggles
    the status the listener observes through ``get_status()``.
    """

    def __init__(self, phase: str = "idle") -> None:
        self.phase = phase
        self.capture_calls = 0
        self._capture_done = threading.Event()

    def get_status(self) -> dict:
        return {"phase": self.phase}

    def set_phase(self, phase: str) -> None:
        self.phase = phase

    def capture_current_page(self) -> None:
        self.capture_calls += 1
        self._capture_done.set()

    def wait_for_capture(self, timeout_s: float = 2.0) -> bool:
        return self._capture_done.wait(timeout=timeout_s)


def test_source_lifecycle_follows_enabled_toggle() -> None:
    """``set_enabled(True)`` starts the source; ``set_enabled(False)`` stops it."""
    skill = _FakeSkillScan()
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(skill_scan_manual=skill, keystroke_source=source)

    assert not listener.is_running
    listener.set_enabled(True)
    assert listener.is_running
    listener.set_enabled(False)
    assert not listener.is_running


def test_space_press_while_capturing_fires_capture_current_page() -> None:
    """The press dispatches the capture on the skill-scan service."""
    skill = _FakeSkillScan(phase="capturing")
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(skill_scan_manual=skill, keystroke_source=source)
    listener.set_enabled(True)

    source.inject("space", _TS, "press")
    assert skill.wait_for_capture(), "capture_current_page should have fired"
    assert skill.capture_calls == 1


def test_space_press_while_idle_is_a_no_op_for_capture() -> None:
    """When the skill scan is not ``capturing``, the press still records the
    tap but does not dispatch a capture."""
    skill = _FakeSkillScan(phase="idle")
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(skill_scan_manual=skill, keystroke_source=source)
    listener.set_enabled(True)

    source.inject("space", _TS, "press")
    # Negative assertion: give the off-thread path a moment, then confirm zero.
    _spin_briefly()
    assert skill.capture_calls == 0


def test_auto_repeat_is_suppressed_until_release() -> None:
    """A second press with no release between edges does not refire capture."""
    skill = _FakeSkillScan(phase="capturing")
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(skill_scan_manual=skill, keystroke_source=source)
    listener.set_enabled(True)

    source.inject("space", _TS, "press")
    assert skill.wait_for_capture()
    assert skill.capture_calls == 1

    # Second press with no release in between — should be suppressed.
    skill._capture_done.clear()
    source.inject("space", _TS, "press")
    _spin_briefly()
    assert skill.capture_calls == 1

    # Release then re-press: second capture fires.
    source.inject("space", _TS, "release")
    source.inject("space", _TS, "press")
    assert skill.wait_for_capture()
    assert skill.capture_calls == 2


def test_non_space_keys_are_ignored() -> None:
    """Keys other than ``space`` never dispatch capture or hit the tap."""
    skill = _FakeSkillScan(phase="capturing")
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(skill_scan_manual=skill, keystroke_source=source)
    listener.set_enabled(True)

    taps: list[tuple[str, str]] = []
    listener.set_key_tap(lambda key, kind: taps.append((key, kind)))

    source.inject("1", _TS, "press")
    source.inject("enter", _TS, "press")
    _spin_briefly()

    assert skill.capture_calls == 0
    assert taps == []


def test_recorder_tap_receives_press_and_release_edges() -> None:
    """The recorder-tap surface still copies both space press and release edges."""
    skill = _FakeSkillScan(phase="idle")
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(skill_scan_manual=skill, keystroke_source=source)
    listener.set_enabled(True)

    taps: list[tuple[str, str]] = []
    listener.set_key_tap(lambda key, kind: taps.append((key, kind)))

    source.inject("space", _TS, "press")
    source.inject("space", _TS, "release")
    source.inject("space", _TS, "press")
    _spin_briefly()

    assert taps == [("space", "press"), ("space", "release"), ("space", "press")]


def test_stop_clears_source_and_state() -> None:
    """``stop()`` halts the source and forgets any pending press state."""
    skill = _FakeSkillScan(phase="capturing")
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(skill_scan_manual=skill, keystroke_source=source)
    listener.set_enabled(True)

    source.inject("space", _TS, "press")
    assert skill.wait_for_capture()

    listener.stop()
    assert not listener.is_running

    # Further injects are silently dropped by the stopped source.
    source.inject("space", _TS, "press")
    _spin_briefly()
    assert skill.capture_calls == 1


# ----------------------------------------------------------------------


def _spin_briefly() -> None:
    """Give off-thread dispatch a small window to either fire or not."""
    import time

    time.sleep(0.05)
