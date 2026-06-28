"""DSL build script for ``defensive_combat_round``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.defensive_combat_round.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="defensive_combat_round")

    s.at("2026-05-19 10:00:00")
    s.combat.damage_received(15.0)
    s.tick()
    s.combat.player_dodge()
    s.tick()
    s.combat.player_evade()
    s.tick()
    s.combat.player_jam()
    s.tick()
    s.combat.mob_miss()
    s.tick()
    s.combat.deflect()
    s.tick()
    s.combat.damage_received(8.0)
    s.tick()
    s.combat.self_heal(12.0)

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
