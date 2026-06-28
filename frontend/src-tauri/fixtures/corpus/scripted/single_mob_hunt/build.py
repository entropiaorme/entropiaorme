"""DSL build script for the ``single_mob_hunt`` scenario.

Regenerate the scenario's ``chat_replay.log`` via::

    python -m backend.tests.e2e.corpus.scripted.single_mob_hunt.build

The script lays down four combat shots (one of them a critical)
across three tick boundaries, closed by a two-item loot tick that
flushes the accumulator into a single kill.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="single_mob_hunt")

    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(12.0)

    s.tick()
    s.combat.damage_dealt(18.0)
    s.combat.critical_hit(35.0)

    s.tick()
    s.combat.damage_dealt(22.0)

    s.tick()
    s.loot.received("Shrapnel", value_ped=8.00, quantity=800)
    s.loot.received("Animal Muscle Oil", value_ped=0.40)

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
