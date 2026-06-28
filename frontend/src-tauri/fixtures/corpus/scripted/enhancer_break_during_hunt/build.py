"""DSL build script for ``enhancer_break_during_hunt``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.enhancer_break_during_hunt.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="enhancer_break_during_hunt")

    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(13.0)

    s.tick()
    s.combat.damage_dealt(17.0)

    # Enhancer pops between combat shots.
    s.tick()
    s.enhancer.broken(
        enhancer_name="Weapon Damage Enhancer 1",
        item_name="ArMatrix LR-5",
        shrapnel_ped=0.83,
        remaining=2,
    )

    s.tick()
    s.combat.damage_dealt(21.0)

    s.tick()
    s.loot.received("Shrapnel", value_ped=4.10, quantity=410)
    s.loot.received("Animal Eye Oil", value_ped=0.55)

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
