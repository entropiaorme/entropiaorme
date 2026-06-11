"""Per-scenario clock plans: the committed time schedule a replay runs on.

Every corpus scenario carries a ``clock:`` block in its ``metadata.yaml``::

    clock:
      start: 2026-01-01T00:00:00
      step_seconds: 1.0

The plan defines a FROZEN, DRIVER-ADVANCED clock:

- the scenario clock starts frozen at ``start`` (a naive ISO-8601 instant,
  interpreted in the host timezone exactly as chat-log line instants are);
- only the replay DRIVER advances it, by ``step_seconds`` at scenario-defined
  points (canonically: once after the replay has fully drained, before the
  session stops, so the session's start/stop boundaries are distinct
  instants); production code under test only ever READS the clock.

Reads never advance the clock, so the instants a scenario produces are
independent of HOW MANY times the implementation reads time. That is the
load-bearing property: two implementations replaying the same scenario
produce identical timestamps even when their internal read counts differ,
which is what makes timestamp-bearing outputs comparable across them. A
per-read schedule (each read consuming the next instant) was rejected for
exactly that reason: it would couple the comparison to an implementation's
internal read count, which is interior detail no output contract pins.

The plan is deliberately trivial to consume from any implementation: two
scalar fields in the scenario's committed ``metadata.yaml``, no runtime
state. ``start`` values are chosen per scenario to avoid colliding with the
scenario's chat-log instants, so a plan-stamped timestamp can never merge
with a domain timestamp under encounter-order symbol assignment.

The watcher's drain-deadline monotonic reads stay on the REAL clock even in
plan-driven runs (see ``ChatlogWatcher``): they are timeout mechanics that
never reach an output, and freezing them would turn a failing drain into a
hang instead of a ``TimeoutError``.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

import yaml

from backend.testing.clock import MockClock


@dataclass(frozen=True)
class ClockPlan:
    """A scenario's committed clock schedule."""

    start: datetime
    step_seconds: float

    def build_clock(self) -> MockClock:
        """Construct the frozen scenario clock positioned at ``start``."""
        return MockClock(start=self.start)


def load_clock_plan(scenario_dir: Path) -> ClockPlan:
    """Read the scenario's ``clock:`` block from its ``metadata.yaml``.

    Fails loudly when the block (or the file) is absent or malformed: a
    scenario without a committed clock plan would silently ride the wall
    clock, which is exactly the nondeterminism the plan exists to remove.
    """
    metadata_path = scenario_dir / "metadata.yaml"
    if not metadata_path.is_file():
        raise FileNotFoundError(
            f"Scenario {scenario_dir.name!r} has no metadata.yaml; every corpus "
            "scenario must commit a clock plan (see backend/testing/clock_plan.py)."
        )
    doc = yaml.safe_load(metadata_path.read_text(encoding="utf-8")) or {}
    if not isinstance(doc, dict):
        raise ValueError(
            f"Scenario {scenario_dir.name!r} metadata.yaml must be a mapping at "
            f"the root, got {type(doc).__name__}."
        )
    block = doc.get("clock")
    if not isinstance(block, dict):
        raise ValueError(
            f"Scenario {scenario_dir.name!r} metadata.yaml carries no 'clock:' "
            "block; every corpus scenario must commit one (see "
            "backend/testing/clock_plan.py)."
        )
    raw_start = block.get("start")
    if isinstance(raw_start, datetime):
        start = raw_start  # yaml parses bare ISO timestamps natively
    elif isinstance(raw_start, str):
        try:
            start = datetime.fromisoformat(raw_start)
        except ValueError as exc:
            raise ValueError(
                f"Scenario {scenario_dir.name!r} clock.start must be a valid "
                f"ISO-8601 instant, got {raw_start!r}."
            ) from exc
    else:
        raise ValueError(
            f"Scenario {scenario_dir.name!r} clock.start must be an ISO-8601 "
            f"instant, got {raw_start!r}."
        )
    if start.tzinfo is not None:
        raise ValueError(
            f"Scenario {scenario_dir.name!r} clock.start must be naive (host-"
            "timezone interpreted, matching chat-log instants), got an aware "
            "datetime."
        )
    raw_step = block.get("step_seconds")
    if not isinstance(raw_step, (int, float)) or isinstance(raw_step, bool):
        raise ValueError(
            f"Scenario {scenario_dir.name!r} clock.step_seconds must be a "
            f"number, got {raw_step!r}."
        )
    step = float(raw_step)
    if step <= 0:
        raise ValueError(
            f"Scenario {scenario_dir.name!r} clock.step_seconds must be "
            f"positive, got {step!r}."
        )
    return ClockPlan(start=start, step_seconds=step)
