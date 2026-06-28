"""DSL build script for ``crit_dodge_evade_jam``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.crit_dodge_evade_jam.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="crit_dodge_evade_jam")

    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(14.0)
    s.combat.target_dodge()

    s.tick()
    s.combat.target_evade()
    s.combat.damage_dealt(17.0)
    s.combat.critical_hit(30.0)

    s.tick()
    s.combat.target_jam()
    s.combat.damage_dealt(12.0)

    s.tick()
    s.loot.received("Shrapnel", value_ped=5.40, quantity=540)

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
