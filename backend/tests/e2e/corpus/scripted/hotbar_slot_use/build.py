"""DSL build script for the ``hotbar_slot_use`` scenario.

Regenerate the scenario's ``chat_replay.log`` and
``keystrokes.jsonl`` via::

    python -m backend.tests.e2e.corpus.scripted.hotbar_slot_use.build

A minimal one-mob hunt provides tracking-session context; three
hotbar slot presses (weapon / heal / consumable) drive the
HotbarListener seam under test.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log + keystrokes.jsonl."""

    s = Scenario(name="hotbar_slot_use")

    # Anchor + one combat tick to establish a non-empty hunt before
    # the hotbar presses fire. (The hotbar listener gates on session
    # active + capability on; both are arranged by the test fixture.)
    s.at("2026-05-19 10:00:00")
    s.combat.damage_dealt(20.0)

    # Slot "1" press: equip the weapon. The listener resolves it
    # and publishes ACTIVE_TOOL_CHANGED.
    s.tick()
    s.keystroke.press("1")

    # Slot "2" press: switch to heal tool. Publishes
    # ACTIVE_HEAL_TOOL_CHANGED.
    s.tick()
    s.keystroke.press("2")

    # Slot "3" press: consumable. The listener resolves it but
    # deliberately publishes nothing (consumables are one-off, do
    # not switch the active weapon).
    s.tick()
    s.keystroke.press("3")

    # A second combat shot + loot tick closes the kill.
    s.tick()
    s.combat.damage_dealt(15.0)

    s.tick()
    s.loot.received("Shrapnel", value_ped=5.00, quantity=500)

    return s.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
