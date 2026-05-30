"""DSL build script for ``multi_mob_hunt_loot_grouping``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.multi_mob_hunt_loot_grouping.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="multi_mob_hunt_loot_grouping")

    # Kill 1: two shots, one-item loot tick.
    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(15.0)
    s.tick()
    s.combat.damage_dealt(20.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=3.50, quantity=350)

    # Gap before kill 2 (5 seconds, well outside any tick window).
    s.at("2026-05-19 10:00:08")
    s.combat.damage_dealt(11.0)
    s.tick()
    s.combat.damage_dealt(14.0)
    s.combat.critical_hit(28.0)
    s.tick()
    s.loot.received("Animal Muscle Oil", value_ped=0.65)
    s.loot.received("Wool", value_ped=1.20, quantity=3)

    # Kill 3: single combat shot closed by a Shrapnel loot tick.
    s.at("2026-05-19 10:00:15")
    s.combat.damage_dealt(18.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=6.70, quantity=670)

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
