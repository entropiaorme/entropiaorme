"""Acceptance test for ``skill_gain_across_tick``.

Multiple skill-gain lines land within combat ticks, on loot
ticks, and between kills. Pins that skill-gain does not
partition the combat accumulator (a regression here would split
one kill into two, or merge two kills into one).
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_skill_gain_does_not_partition_kills(
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    scenario = corpus_root / "scripted" / "skill_gain_across_tick"
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    result = tracker.stop_session()

    assert len(result.kills) == 2, (
        f"Expected 2 kills, got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    # Kill 1: 14.0 + 18.0 = 32.0 dmg, loot 2.00.
    assert result.kills[0].shots_fired == 2
    assert result.kills[0].damage_dealt == pytest.approx(32.0)
    assert result.kills[0].loot_total_ped == pytest.approx(2.00)

    # Kill 2: 20.0 + 11.0 = 31.0 dmg, loot 3.30.
    assert result.kills[1].shots_fired == 2
    assert result.kills[1].damage_dealt == pytest.approx(31.0)
    assert result.kills[1].loot_total_ped == pytest.approx(3.30)

    goldens.assert_matches(in_memory_db)
