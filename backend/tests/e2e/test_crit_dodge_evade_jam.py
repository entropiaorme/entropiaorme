"""Acceptance test for ``crit_dodge_evade_jam``.

Full offensive-combat-line coverage in a single kill window:
plain damage, critical hit, and the three countered-shot lines
(target_dodge, target_evade, target_jam). Pins regex precedence
and prefix-anchored matching across the offensive surface.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.testing.replay import replay_scenario, wait_for_drain


def test_full_offensive_combat_surface_yields_one_kill(
    e2e_pipeline,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    bus, tracker, watcher, chatlog = e2e_pipeline

    scenario = corpus_root / "scripted" / "crit_dodge_evade_jam"
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    result = tracker.stop_session()

    # Damage shots: 14.0 + 17.0 + crit 30.0 + 12.0 = 73.0 dmg.
    # Three dodged/evaded/jammed shots don't add to damage but
    # are still shots fired (they're attempts).
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert kill.damage_dealt == pytest.approx(73.0)
    assert kill.loot_total_ped == pytest.approx(5.40)
    assert kill.critical_hits == 1

    goldens.assert_matches(in_memory_db)
