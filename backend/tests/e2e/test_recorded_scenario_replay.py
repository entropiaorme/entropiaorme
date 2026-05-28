"""Acceptance test for a recorded-flavour scenario.

Replays a scenario from ``corpus/recorded/`` through the real watcher → parser
→ tick-buffer → bus → tracker → SQLite path and asserts the full event/DB
fingerprint matches its goldens, exactly as the scripted-scenario tests do.

The recorded corpus is grown by recording mode during live gameplay, but real
recorded bundles are local-by-default and are not committed to the public repo
(see ``backend/testing/RECORDING.md``). This public test therefore runs
permanently against ``placeholder_recorded_hunt`` (a synthetic DSL-authored
stand-in): it keeps the recorded-scenario replay path green for any reader or
CI without exposing real gameplay. A real bundle, once seeded locally into
``corpus/recorded/``, can be pinned by an opt-in/local sibling test pointed at
it.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_placeholder_recorded_hunt_replays_against_goldens(
    e2e_pipeline,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    """Replay the placeholder recorded hunt; assert two kills and the full
    event/DB state matches the goldens."""
    bus, tracker, watcher, chatlog = e2e_pipeline

    scenario = corpus_root / "recorded" / "placeholder_recorded_hunt"
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    result = tracker.stop_session()

    assert len(result.kills) == 2, (
        f"Expected 2 kills, got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )
    second = result.kills[1]
    assert second.critical_hits == 1
    assert second.damage_dealt == pytest.approx(55.0)

    goldens.assert_matches(in_memory_db)
