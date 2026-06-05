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

from backend.testing.clock import MockClock
from backend.testing.replay import replay_scenario, wait_for_drain


def test_empty_session_produces_no_kills_via_real_tail_loop(
    make_e2e_pipeline,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    """Stream zero events through the pipeline; assert the session
    closes with an empty kill list and matches the empty-session
    goldens.
    """

    # A zero-event session reads the clock only twice (session start and
    # session stop) with no intervening events to separate them. Under the
    # real wall clock those two reads can land on the same instant, leaving
    # the start/stop timestamps equal or distinct depending on timing, which
    # makes the golden order-dependent. Drive a deterministic clock and step
    # it forward before the stop so the session boundaries are always
    # distinct and the golden is reproducible regardless of test order.
    clock = MockClock()
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)

    scenario = corpus_root / "scripted" / "empty_session"
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(1.0)
    result = tracker.stop_session()

    assert len(result.kills) == 0, (
        f"Expected 0 kills for an empty session, got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    goldens.assert_matches(in_memory_db)
