"""Acceptance test for ``defensive_combat_round``.

Pure defensive-combat surface: no offensive shots, no loot, no
kills. Pins the no-kill path while exercising every defensive
parser line (damage_received twice, player_dodge / player_evade
/ player_jam / mob_miss / deflect, plus self_heal). Profession
panel coverage is unrelated to the chatlog parser and lands via
the screen-capture harness layer instead.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.replay import replay_scenario, wait_for_drain


def test_defensive_only_produces_no_kills(
    e2e_pipeline,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    bus, tracker, _watcher, chatlog = e2e_pipeline

    scenario = corpus_root / "scripted" / "defensive_combat_round"
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain()
    result = tracker.stop_session()

    assert len(result.kills) == 0, (
        f"Expected 0 kills (defensive only), got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    goldens.assert_matches(in_memory_db)
