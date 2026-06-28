"""DSL build script for ``hof_item_drop``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.hof_item_drop.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="hof_item_drop")

    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(28.0)

    s.tick()
    s.combat.damage_dealt(33.0)

    s.tick()
    s.loot.received("Shrapnel", value_ped=12.00, quantity=1200)

    # Hall-of-Fame rare-item global within the 5s staleness window
    # of the kill above. Player name must match the tracker's
    # configured player_name for correlation to fire.
    s.tick()
    s.globals.item(
        player="TestPlayer",
        item="Modified Mercenary Armor",
        value_ped=4800.00,
        hof=True,
    )

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
