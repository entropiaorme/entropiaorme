"""Acceptance test for ``hof_kill_correlated``.

A local kill is followed by a Hall-of-Fame kill global announcement
naming the test's configured player_name. Pins the HOF_KILL parser
rule path plus the tracker's _on_global correlation: the HoF global
lands within the 5-second staleness window of the most recent kill
and tags that kill as is_global and is_hof. Uses ``make_e2e_pipeline``
to set ``player_name`` explicitly since the default pipeline drops
globals on the floor.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_hof_kill_correlates_to_recent_kill(
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    scenario = corpus_root / "scripted" / "hof_kill_correlated"
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(
        player_name="TestPlayer", clock=clock
    )
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    result = tracker.stop_session()

    # Single kill: 40.0 + 35.0 = 75.0 dmg across two shots, loot 15.00.
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert kill.shots_fired == 2
    assert kill.damage_dealt == pytest.approx(75.0)
    assert kill.loot_total_ped == pytest.approx(15.00)
    # A Hall-of-Fame kill global tags the kill as global AND HoF.
    assert kill.is_global is True
    assert kill.is_hof is True

    goldens.assert_matches(in_memory_db)
