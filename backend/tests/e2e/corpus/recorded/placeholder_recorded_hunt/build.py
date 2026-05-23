"""DSL build script for the placeholder_recorded_hunt scenario.

This is a SYNTHETIC placeholder standing in for a real recorded scenario
until one is captured during live gameplay via recording mode. It is authored
through the DSL (like a scripted scenario) but lives under ``corpus/recorded/``
so the recorded-scenario replay test has a bundle to assert against from the
moment recording mode lands. The first real recording supersedes it.

Two kills across four tick boundaries: a plain kill closed by a Shrapnel loot
tick, then a kill with a critical closed by an oil loot tick.
"""

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""
    s = Scenario(name="placeholder_recorded_hunt")
    s.at("2026-05-20 14:00:00")
    s.combat.damage_dealt(20.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=5.00, quantity=500)
    s.tick()
    s.combat.damage_dealt(15.0)
    s.combat.critical_hit(40.0)
    s.tick()
    s.loot.received("Animal Muscle Oil", value_ped=1.20)
    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
