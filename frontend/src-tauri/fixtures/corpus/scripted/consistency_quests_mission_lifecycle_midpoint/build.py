"""DSL build script for the consistency_quests_mission_lifecycle_midpoint scenario.

Regenerate the scenario's two segment files via::

    python -m backend.tests.e2e.corpus.scripted.consistency_quests_mission_lifecycle_midpoint.build

Each segment fires one mission_received event. The mission names
match quests the test pre-populates so QuestService auto-starts the
quest and records a notable_events row the view reads.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def _write_segment(lines: list[str], target: Path) -> None:
    """Write a segment's chat lines into the scenario directory."""
    target.write_text("".join(lines), encoding="utf-8")


def build() -> Path:
    """Build both segment files into the scenario's own directory."""

    target_dir = Path(__file__).parent

    pre = Scenario(name="consistency_quests_mission_lifecycle_midpoint_pre")
    pre.at("2026-05-19 11:00:00")
    pre.mission.received("Alpha Hunt")

    _write_segment(pre.lines(), target_dir / "chat_replay.log")

    post = Scenario(name="consistency_quests_mission_lifecycle_midpoint_post")
    post.at("2026-05-19 11:00:30")
    post.mission.received("Beta Hunt")

    _write_segment(post.lines(), target_dir / "chat_replay_after.log")

    return target_dir


if __name__ == "__main__":
    build()
