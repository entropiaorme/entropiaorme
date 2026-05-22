"""Acceptance test for the ``empty_session`` scenario.

The scenario carries an empty ``chat_replay.log``; this test pins
the smallest-possible session shape: start, drain nothing, stop.
Together with ``single_mob_hunt`` it forms the corpus's first
two DSL-authored scenarios, validating the DSL substrate against
a real ``ChatlogWatcher`` tail loop and an in-memory tracker.

The goldens capture only the session-lifecycle events
(``session_started`` -> ``session_stopped``) and the empty
tracker/ledger state at session-stop time; any future change that
emits an event during an empty session, or that fails to write
the session-stop ledger row, surfaces as a golden diff.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.replay import replay_scenario, wait_for_drain


def test_empty_session_produces_no_kills_via_real_tail_loop(
    e2e_pipeline,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    """Stream zero events through the pipeline; assert the session
    closes with an empty kill list and matches the empty-session
    goldens.
    """

    bus, tracker, _watcher, chatlog = e2e_pipeline

    scenario = corpus_root / "scripted" / "empty_session"
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain()
    result = tracker.stop_session()

    assert len(result.kills) == 0, (
        f"Expected 0 kills for an empty session, got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    goldens.assert_matches(in_memory_db)
