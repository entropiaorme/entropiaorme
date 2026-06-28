"""DSL build script for the consistency_tracking_hunt_midpoint scenario.

Regenerate the scenario's two segment files via::

    python -m backend.tests.e2e.corpus.scripted.consistency_tracking_hunt_midpoint.build

The pre-segment lays down one kill; the post-segment continues the
same session with another kill, more shots, and a critical. Both
segments are written into the scenario directory next to
``metadata.yaml`` as ``chat_replay.log`` (pre) and
``chat_replay_after.log`` (post); the filesystem split is the midpoint
marker the consistency harness keys on.
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

    # Pre-midpoint segment: one kill (closed by a loot tick). The kill
    # closes at 10:00:02 so the T0 snapshot captures kill_count=1,
    # returns=5.00, and shots / damage from the resolved kill.
    pre = Scenario(name="consistency_tracking_hunt_midpoint_pre")
    pre.at("2026-05-19 10:00:00")
    pre.combat.damage_dealt(20.0)
    pre.tick()
    pre.combat.damage_dealt(15.0)
    pre.tick()
    pre.loot.received("Shrapnel", value_ped=5.00, quantity=500)

    _write_segment(pre.lines(), target_dir / "chat_replay.log")

    # Post-midpoint segment: continues the session with combat, a
    # critical, and one final loot tick. T1 should carry kill_count=2,
    # returns=12.50, and advanced shots / damage / crits totals.
    post = Scenario(name="consistency_tracking_hunt_midpoint_post")
    post.at("2026-05-19 10:00:10")
    post.combat.damage_dealt(18.0)
    post.tick()
    post.combat.critical_hit(40.0)
    post.tick()
    post.combat.damage_dealt(12.0)
    post.tick()
    post.loot.received("Animal Muscle Oil", value_ped=7.50)

    _write_segment(post.lines(), target_dir / "chat_replay_after.log")

    return target_dir


if __name__ == "__main__":
    build()
