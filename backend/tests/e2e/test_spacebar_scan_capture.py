"""Acceptance test for the ``spacebar_scan_capture`` scenario.

Wires a ``SpacebarCaptureListener`` with a ``MockKeystrokeSource`` and
a fake skill-scan-manual that records ``capture_current_page``
invocations, then replays the scenario's space-key edges across a
capture-phase boundary. Pins the listener's gating contract:

- Press while phase == ``capturing`` → capture fires + tap records.
- Press while phase == ``idle`` → tap records, capture does not fire.

The listener's auto-repeat suppression is covered by unit tests at
``backend/tests/test_spacebar_capture_listener.py``; this scenario
is the end-to-end seam check.
"""

from __future__ import annotations

import json
import threading
from datetime import datetime
from pathlib import Path

from backend.services.spacebar_capture_listener import SpacebarCaptureListener
from backend.testing.keystroke_source import MockKeystrokeSource


class _RecordingSkillScan:
    """Test double for SkillScanManual.

    Phase is mutable so the test can toggle between ``capturing`` and
    ``idle`` over the scenario's timeline; ``capture_current_page``
    records every invocation and signals an Event for synchronous
    waits.
    """

    def __init__(self, phase: str = "idle") -> None:
        self._phase = phase
        self.capture_calls = 0
        self._capture_done = threading.Event()

    def get_status(self) -> dict:
        return {"phase": self._phase}

    def set_phase(self, phase: str) -> None:
        self._phase = phase

    def capture_current_page(self) -> None:
        self.capture_calls += 1
        self._capture_done.set()

    def wait_for_capture(self, timeout_s: float = 2.0) -> bool:
        result = self._capture_done.wait(timeout=timeout_s)
        self._capture_done.clear()
        return result


def _load_keystrokes(scenario: Path) -> list[dict]:
    """Read the scenario's keystrokes.jsonl."""
    return [
        json.loads(line)
        for line in (scenario / "keystrokes.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
        if line.strip()
    ]


def test_spacebar_scan_capture_drives_listener_via_keystroke_source(
    corpus_root: Path,
    data_regression,
) -> None:
    """The two recorded press-release pairs produce one capture
    (capturing-phase press) and zero captures (idle-phase press).
    """

    scenario = corpus_root / "scripted" / "spacebar_scan_capture"
    keystrokes = _load_keystrokes(scenario)
    assert len(keystrokes) == 4, "scenario should carry two press-release pairs"

    skill = _RecordingSkillScan(phase="capturing")
    source = MockKeystrokeSource()
    listener = SpacebarCaptureListener(
        skill_scan_manual=skill,
        keystroke_source=source,
    )
    listener.set_enabled(True)

    taps: list[dict] = []
    listener.set_key_tap(lambda key, kind: taps.append({"key": key, "kind": kind}))

    # First press-release pair: phase=capturing, capture should fire.
    _inject(source, keystrokes[0])  # press
    assert skill.wait_for_capture(), "capture_current_page should have fired"
    _inject(source, keystrokes[1])  # release

    # Flip the phase to idle for the second pair.
    skill.set_phase("idle")
    _inject(source, keystrokes[2])  # press
    _inject(source, keystrokes[3])  # release
    # Negative assertion: brief settle window, then confirm count unchanged.
    import time

    time.sleep(0.05)

    # Value-level pins so a mutant that preserves the capture count but
    # corrupts the dispatched edges (e.g. drops a release, mislabels a
    # press, or fires capture on the idle-phase press) is still caught.
    assert skill.capture_calls == 1
    assert skill.get_status() == {"phase": "idle"}, "phase flips before the idle pair"
    # Both press-release pairs flow through the tap observer regardless of
    # phase; gating only governs capture, not the pass-through tap.
    assert taps == [
        {"key": "space", "kind": "press"},
        {"key": "space", "kind": "release"},
        {"key": "space", "kind": "press"},
        {"key": "space", "kind": "release"},
    ]
    data_regression.check(
        {
            "capture_calls": skill.capture_calls,
            "tap_edges": taps,
        }
    )


def _inject(source: MockKeystrokeSource, record: dict) -> None:
    """Inject one keystroke record into the mock source."""
    source.inject(
        record["key"],
        datetime.fromisoformat(record["wall"]),
        record["kind"],
    )
