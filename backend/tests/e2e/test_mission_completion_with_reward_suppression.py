"""Acceptance test for ``mission_completion_with_reward_suppression``.

The chatlog lifecycle is: mission_received -> brief hunt
(one kill) -> mission_completed -> the would-be skill-gain
reward line. The kill totals, the mission name and the reward
line's concrete skill + amount are pinned against the recorded
event stream alongside the golden DB snapshot. Active
reward-suppression (the gain being dropped from the stream)
needs ``QuestService.quest_reward_filter`` threaded into the
watcher and is exercised by the playlist-match acceptance test;
this pipeline boots the tracker without that filter, so the
gain here is emitted unchanged.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.core.events import EVENT_MISSION_RECEIVED, EVENT_SKILL_GAIN
from backend.testing.replay import replay_scenario, wait_for_drain

MISSION_NAME = "Codex Argonaut Stage 1"


def test_mission_lifecycle_with_skill_gain(
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    scenario = corpus_root / "scripted" / "mission_completion_with_reward_suppression"
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    result = tracker.stop_session()

    # Single kill in the middle of the mission lifecycle.
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert kill.shots_fired == 2
    assert kill.damage_dealt == pytest.approx(35.0)
    assert kill.loot_total_ped == pytest.approx(2.50)

    # Pin the value-level shape of the recorded event stream, not just
    # its row counts: a mutation that corrupted the mission name or the
    # reward line's skill/amount while leaving every event count intact
    # would otherwise slip past a status-only check.
    events = goldens.recorder.events
    by_topic: dict[str, list] = {}
    for topic, payload in events:
        by_topic.setdefault(topic, []).append(payload)

    mission_lines = by_topic.get(EVENT_MISSION_RECEIVED, [])
    assert [p["mission_name"] for p in mission_lines] == [MISSION_NAME]

    # The skill-gain line that follows the mission completion is the
    # would-be quest reward. This pipeline boots the tracker without a
    # ``quest_reward_filter`` wired in (the shared ``e2e_pipeline``
    # fixture passes none), so the watcher's tick-time suppression path
    # never fires and the gain is emitted unchanged. Pinning the
    # concrete skill_name + amount here catches a mutant that corrupts
    # the reward value; the absent-from-stream form of suppression is
    # exercised where ``QuestService.quest_reward_filter`` is actually
    # threaded into the watcher (the playlist-match acceptance test).
    skill_lines = by_topic.get(EVENT_SKILL_GAIN, [])
    assert len(skill_lines) == 1
    reward = skill_lines[0]
    assert reward["skill_name"] == "Bioregenesis"
    assert reward["amount"] == pytest.approx(0.05)

    goldens.assert_matches(in_memory_db)
