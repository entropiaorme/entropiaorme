"""Acceptance test for the basic 10-event hunt scenario.

The scenario lives at
``backend/tests/e2e/corpus/scripted/basic_hunt_10_events/`` and
contains ten chat.log lines spanning three loot ticks. This test
boots the real ``ChatlogWatcher`` against a temp file, streams the
scenario's lines through it, and asserts the resulting ``HuntTracker``
state matches the scenario's documented expectation. The path under
test (file write -> watcher tail loop -> parser -> tick buffer -> bus
-> tracker -> SQLite) is the same one the live game drives in
production; this is the first test in the suite to exercise it
end-to-end rather than via direct ``_process_line(...)`` calls.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_basic_hunt_produces_three_kills_via_real_tail_loop(
    e2e_pipeline,
    corpus_root: Path,
) -> None:
    """Stream the 10-event scenario through the real watcher and
    assert the tracker captured three kills with the documented stats."""
    _bus, tracker, _watcher, chatlog = e2e_pipeline

    scenario = corpus_root / "scripted" / "basic_hunt_10_events"

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain()
    result = tracker.stop_session()

    # Three loot ticks (10:00:02, 10:00:06, 10:00:10) each close an
    # accumulated combat window into its own kill record.
    assert len(result.kills) == 3, (
        f"Expected 3 kills, got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    # Kill 1: 10:00:00 (10.5 + 15.0) + 10:00:01 (30.0)
    #         flushed by loot tick at 10:00:02 (Shrapnel 5.00 + Animal Muscle Oil 0.12).
    k1 = result.kills[0]
    assert k1.shots_fired == 3
    assert k1.damage_dealt == pytest.approx(55.5)
    assert k1.loot_total_ped == pytest.approx(5.12)

    # Kill 2: 10:00:05 (20.0) flushed by loot tick at 10:00:06.
    k2 = result.kills[1]
    assert k2.shots_fired == 1
    assert k2.damage_dealt == pytest.approx(20.0)
    assert k2.loot_total_ped == pytest.approx(2.00)

    # Kill 3: 10:00:08 (25.0) flushed by loot tick at 10:00:10.
    # The skill-gain tick at 10:00:09 sits between the combat and loot
    # ticks and must not partition the accumulator.
    k3 = result.kills[2]
    assert k3.shots_fired == 1
    assert k3.damage_dealt == pytest.approx(25.0)
    assert k3.loot_total_ped == pytest.approx(1.50)

    # Session-level totals: 5 shots, 100.5 damage, 8.62 PED loot.
    assert sum(k.shots_fired for k in result.kills) == 5
    assert sum(k.damage_dealt for k in result.kills) == pytest.approx(100.5)
    assert sum(k.loot_total_ped for k in result.kills) == pytest.approx(8.62)
