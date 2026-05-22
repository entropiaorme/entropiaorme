"""Acceptance test for ``mission_completion_with_reward_suppression``.

The chatlog lifecycle is: mission_received -> brief hunt
(one kill) -> mission_completed -> the would-be skill-gain
reward line. The golden pins whatever the tracker +
quest_service currently do with this surface; fuller
reward-suppression coverage involving the junction table and
playlist matching lands with the keystroke / quest-automation
harness layer.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_mission_lifecycle_with_skill_gain(
    e2e_pipeline,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    bus, tracker, _watcher, chatlog = e2e_pipeline

    scenario = corpus_root / "scripted" / "mission_completion_with_reward_suppression"
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain()
    result = tracker.stop_session()

    # Single kill in the middle of the mission lifecycle.
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert kill.shots_fired == 2
    assert kill.damage_dealt == pytest.approx(35.0)
    assert kill.loot_total_ped == pytest.approx(2.50)

    goldens.assert_matches(in_memory_db)
