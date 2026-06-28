"""DSL build script for ``quest_automation_with_playlist_match``.

Regenerate via::

    python -m backend.tests.e2e.corpus.scripted.quest_automation_with_playlist_match.build

Two missions back-to-back in a single session. Each mission opens
with a received line, carries a brief hunt window, and closes with a
completed line. The chat-line count (events: 8) matches what the
metadata declares.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    s = Scenario(name="quest_automation_with_playlist_match")

    # Mission 1.
    s.at("2026-05-19 10:00:00")
    s.mission.received("Alpha Hunt")

    s.tick()
    s.combat.damage_dealt(18.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=2.50, quantity=250)

    s.tick()
    s.mission.completed("Alpha Hunt")

    # Mission 2 (same session).
    s.tick()
    s.mission.received("Beta Hunt")

    s.tick()
    s.combat.damage_dealt(22.0)
    s.tick()
    s.loot.received("Shrapnel", value_ped=2.50, quantity=250)

    s.tick()
    s.mission.completed("Beta Hunt")

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
