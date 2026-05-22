"""Acceptance test for ``hof_item_drop``.

A local kill is followed by a Hall-of-Fame rare-item global
announcement naming the test's configured player_name. Pins the
HOF_ITEM parser rule path plus the tracker's _on_global
correlation: the HoF event lands within the 5-second staleness
window of the most recent kill and tags that kill as is_global
and is_hof. Uses ``make_e2e_pipeline`` to set ``player_name``
explicitly since the default pipeline drops globals on the
floor.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_hof_item_drop_correlates_to_recent_kill(
    make_e2e_pipeline,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    bus, tracker, _watcher, chatlog = make_e2e_pipeline(player_name="TestPlayer")

    scenario = corpus_root / "scripted" / "hof_item_drop"
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain()
    result = tracker.stop_session()

    # Single kill: 28.0 + 33.0 = 61.0 dmg across two shots, loot 12.00.
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert kill.shots_fired == 2
    assert kill.damage_dealt == pytest.approx(61.0)
    assert kill.loot_total_ped == pytest.approx(12.00)
    assert kill.is_global is True
    assert kill.is_hof is True

    goldens.assert_matches(in_memory_db)
