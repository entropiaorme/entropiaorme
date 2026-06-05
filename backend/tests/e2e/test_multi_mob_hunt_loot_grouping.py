"""Acceptance test for the ``multi_mob_hunt_loot_grouping`` scenario.

Three sequential combat-then-loot cycles produce three independent
kills with distinct loot composition. Pins the multi-kill flush
path at a small enough event count that the kill-by-kill assertions
still read cleanly above the golden coverage.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_multi_mob_hunt_produces_three_kills(
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    scenario = corpus_root / "scripted" / "multi_mob_hunt_loot_grouping"
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    result = tracker.stop_session()

    assert len(result.kills) == 3, (
        f"Expected 3 kills, got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    # Kill 1: 15.0 + 20.0 = 35.0 dmg across two shots, loot 3.50.
    assert result.kills[0].shots_fired == 2
    assert result.kills[0].damage_dealt == pytest.approx(35.0)
    assert result.kills[0].loot_total_ped == pytest.approx(3.50)

    # Kill 2: 11.0 + 14.0 + crit 28.0 = 53.0 dmg, loot 0.65 + 1.20 = 1.85.
    assert result.kills[1].shots_fired == 3
    assert result.kills[1].damage_dealt == pytest.approx(53.0)
    assert result.kills[1].loot_total_ped == pytest.approx(1.85)

    # Kill 3: 18.0 dmg one shot, loot 6.70.
    assert result.kills[2].shots_fired == 1
    assert result.kills[2].damage_dealt == pytest.approx(18.0)
    assert result.kills[2].loot_total_ped == pytest.approx(6.70)

    goldens.assert_matches(in_memory_db)
