"""DSL build script for the ``spacebar_scan_capture`` scenario.

Regenerate the scenario's ``chat_replay.log`` (intentionally empty)
and ``keystrokes.jsonl`` via::

    python -m backend.tests.e2e.corpus.scripted.spacebar_scan_capture.build

Two space-key edges: a press-release pair while the skill scan is
``capturing`` (the first press fires the capture; the release lets
the next press be a fresh edge), then a second press while the skill
scan is idle (no capture should fire).
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log + keystrokes.jsonl."""

    s = Scenario(name="spacebar_scan_capture")

    s.at("2026-05-19 10:00:00")

    # Press-release while the scan is capturing — first edge fires.
    s.keystroke.press("space")
    s.tick()
    s.keystroke.release("space")

    # Idle-phase press — tap records, capture does not fire.
    s.tick()
    s.keystroke.press("space")
    s.tick()
    s.keystroke.release("space")

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
