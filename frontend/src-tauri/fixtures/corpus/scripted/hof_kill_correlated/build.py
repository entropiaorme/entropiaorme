"""DSL build script for ``hof_kill_correlated``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.hof_kill_correlated.build
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="hof_kill_correlated")

    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(40.0)

    s.tick()
    s.combat.damage_dealt(35.0)

    s.tick()
    s.loot.received("Shrapnel", value_ped=15.00, quantity=1500)

    # A Hall-of-Fame kill global within the 5s correlation window of the
    # kill above. Player name must match the tracker's configured
    # player_name for correlation to fire; hof=True appends the
    # Hall-of-Fame suffix that promotes the parse to HOF_KILL, so the kill
    # is tagged is_global AND is_hof.
    s.tick()
    s.globals.kill(
        player="TestPlayer",
        creature="Argonaut Stalker",
        value_ped=2200.00,
        hof=True,
    )

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
