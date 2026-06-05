"""Acceptance test for the ``single_mob_hunt`` scenario.

One mob, four combat shots (including a critical) across three
tick boundaries, closed by a two-item loot tick that flushes the
accumulator into a single kill row. Pins the same tick-buffer
flush path that ``basic_hunt_10_events`` exercises but at a
smaller event count so the golden diff surface stays tight and
the test reads as a worked DSL-authored example.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_single_mob_hunt_produces_one_kill_via_real_tail_loop(
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    """Stream the 6-event scenario through the real watcher; assert
    one kill emerges with the documented shot count, damage total,
    and loot value, and the full event/DB state matches the
    goldens.
    """

    scenario = corpus_root / "scripted" / "single_mob_hunt"
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    result = tracker.stop_session()

    # Combat at 10:00:00 (12.0) + 10:00:01 (18.0 + crit 35.0) +
    # 10:00:02 (22.0) flushed by loot tick at 10:00:03.
    assert len(result.kills) == 1, (
        f"Expected 1 kill, got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    kill = result.kills[0]
    assert kill.shots_fired == 4
    assert kill.damage_dealt == pytest.approx(87.0)
    assert kill.loot_total_ped == pytest.approx(8.40)

    goldens.assert_matches(in_memory_db)
