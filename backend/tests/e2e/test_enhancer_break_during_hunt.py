"""Acceptance test for ``enhancer_break_during_hunt``.

A weapon enhancer breaks between combat shots; the surrounding
kill closes normally on the subsequent loot tick. Pins the
ENHANCER_BREAK parse + tracker event surface alongside the
ordinary kill-creation path.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_enhancer_break_does_not_disrupt_kill(
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    scenario = corpus_root / "scripted" / "enhancer_break_during_hunt"
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    result = tracker.stop_session()

    # Single kill: 13.0 + 17.0 + 21.0 = 51.0 dmg across three shots,
    # loot Shrapnel 4.10 + Animal Eye Oil 0.55 = 4.65.
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert kill.shots_fired == 3
    assert kill.damage_dealt == pytest.approx(51.0)
    assert kill.loot_total_ped == pytest.approx(4.65)

    goldens.assert_matches(in_memory_db)
