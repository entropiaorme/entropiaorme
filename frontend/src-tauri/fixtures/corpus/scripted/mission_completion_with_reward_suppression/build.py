"""DSL build script for ``mission_completion_with_reward_suppression``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.mission_completion_with_reward_suppression.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="mission_completion_with_reward_suppression")

    s.at("2026-05-19 10:00:00")
    s.mission.received("Codex Argonaut Stage 1")

    # Brief hunt window: one kill.
    s.tick()
    s.combat.damage_dealt(16.0)
    s.tick()
    s.combat.damage_dealt(19.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=2.50, quantity=250)

    s.tick()
    s.mission.completed("Codex Argonaut Stage 1")

    # The reward-flavoured skill_gain immediately after the
    # completion line. Production behaviour here may evolve as
    # quest infrastructure lands; the golden pins today's reality.
    s.tick()
    s.skill.gained(0.0500, "Bioregenesis")

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
