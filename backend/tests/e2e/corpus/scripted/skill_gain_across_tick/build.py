"""DSL build script for ``skill_gain_across_tick``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.skill_gain_across_tick.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="skill_gain_across_tick")

    # Kill 1: combat + skill within same tick, loot closes.
    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(14.0)
    s.skill.gained(0.0500, "Bioregenesis")
    s.tick()
    s.combat.damage_dealt(18.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=2.00, quantity=200)
    s.skill.gained(0.0200, "Combat Sense")

    # Between-kill skill-gain (no combat accumulator active).
    s.at("2026-05-19 10:00:05")
    s.skill.gained(0.0100, "Anatomy")

    # Kill 2: combat + loot.
    s.tick()
    s.combat.damage_dealt(20.0)
    s.tick()
    s.combat.damage_dealt(11.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=3.30, quantity=330)

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
