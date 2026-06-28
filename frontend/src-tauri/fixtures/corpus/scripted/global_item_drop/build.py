"""DSL build script for ``global_item_drop``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.global_item_drop.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="global_item_drop")

    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(28.0)

    s.tick()
    s.combat.damage_dealt(33.0)

    s.tick()
    s.loot.received("Shrapnel", value_ped=12.00, quantity=1200)

    # A plain (non-Hall-of-Fame) rare-item global within the 5s
    # correlation window of the kill above. Player name must match the
    # tracker's configured player_name for correlation to fire; hof=False
    # keeps the GLOBAL_ITEM parse (no "A record" suffix), so the kill is
    # tagged is_global WITHOUT is_hof.
    s.tick()
    s.globals.item(
        player="TestPlayer",
        item="Improved A104 Restoration Chip",
        value_ped=850.00,
        hof=False,
    )

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
