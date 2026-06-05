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
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    scenario = corpus_root / "scripted" / "defensive_combat_round"
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    result = tracker.stop_session()

    assert len(result.kills) == 0, (
        f"Expected 0 kills (defensive only), got {len(result.kills)}: "
        f"{[(k.shots_fired, k.loot_total_ped) for k in result.kills]}"
    )

    goldens.assert_matches(in_memory_db)
