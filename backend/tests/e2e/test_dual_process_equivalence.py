"""Dual-process equivalence smoke over the full-golden scenario set.

Boots TWO independent backend processes per scenario, drives both
through the same externally-applied replay, and proves the three
equivalence surfaces (the ``events.jsonl`` publish stream, the data-dir
SQLite file, the curated hydration GET set) byte-identical between the
legs and to the committed goldens.

With both legs on the Python backend this is the known-good negative
control for cross-implementation comparison: process identity, ports,
session ids and data directories all differ between the legs, so the
byte-identity proven here is identity of *behaviour* under the
normalised capture, not identity of process state. A second
implementation of the same HTTP surface is graded by pointing one leg's
launch command at it (``ExternalBackendLeg(command=...)``) while every
assertion below stays untouched.

The scenario set is exactly the scripted scenarios that carry all three
golden surfaces, spanning the tracker's kill accounting, the quest
lifecycle path, and multi-kill loot grouping.
"""

from __future__ import annotations

from contextlib import ExitStack
from pathlib import Path

import pytest

from backend.testing.external_process import (
    ExternalBackendLeg,
    expected_surfaces,
    free_ports,
)

CORPUS = Path(__file__).parent / "corpus" / "scripted"

SCENARIOS = [
    "basic_hunt_10_events",
    "mission_completion_with_reward_suppression",
    "multi_mob_hunt_loot_grouping",
]


def test_second_replay_in_one_boot_is_refused(tmp_path):
    """The replay route is one-shot per boot; the clock guard enforces it.

    After a settled replay the frozen clock has advanced past the
    scenario plan's start instant, so a second replay in the same
    process is refused (409) rather than silently double-driving the
    pipeline. Pinned here through the external surface because the
    cross-implementation harness relies on it: one boot is one run.
    """
    (port,) = free_ports()
    leg = ExternalBackendLeg(CORPUS / SCENARIOS[0], tmp_path, port=port)
    with ExitStack() as stack:
        leg.start()
        stack.callback(leg.shutdown)
        leg.wait_ready()
        leg.replay()
        with pytest.raises(RuntimeError, match="replay returned 409"):
            leg.replay()


@pytest.mark.parametrize("scenario_name", SCENARIOS)
def test_dual_process_runs_are_byte_identical(tmp_path, scenario_name):
    """Two processes, one scenario: every surface byte-equal across legs."""
    scenario = CORPUS / scenario_name
    port_a, port_b = free_ports(2)
    leg_a = ExternalBackendLeg(scenario, tmp_path / "leg_a", port=port_a)
    leg_b = ExternalBackendLeg(scenario, tmp_path / "leg_b", port=port_b)

    with ExitStack() as stack:
        # Overlap the boots (interpreter start-up and imports dominate the
        # cost), then drive each leg's synchronous replay in turn; the
        # replay response itself is each leg's drain barrier.
        leg_a.start()
        stack.callback(leg_a.shutdown)
        leg_b.start()
        stack.callback(leg_b.shutdown)
        leg_a.wait_ready()
        leg_b.wait_ready()
        leg_a.replay()
        leg_a.capture_http()
        leg_b.replay()
        leg_b.capture_http()

    surfaces_a = leg_a.surfaces()
    surfaces_b = leg_b.surfaces()

    # Anchor leg A to the committed goldens first: the cross-leg identity
    # below is only meaningful as a control when the reference leg is
    # provably the known-good behaviour.
    expected = expected_surfaces(scenario)
    assert surfaces_a.fingerprint == expected.fingerprint
    assert surfaces_a.db_state == expected.db_state
    assert set(surfaces_a.http_responses) == set(expected.http_responses)
    for endpoint_id, expected_text in expected.http_responses.items():
        assert surfaces_a.http_responses[endpoint_id] == expected_text, (
            f"leg A diverged from the committed {endpoint_id} golden"
        )

    # The dual-process contract: leg B byte-identical to leg A on every
    # surface. This is the comparison a second backend implementation
    # plugs into.
    assert surfaces_b.fingerprint == surfaces_a.fingerprint
    assert surfaces_b.db_state == surfaces_a.db_state
    assert set(surfaces_b.http_responses) == set(surfaces_a.http_responses)
    for endpoint_id, leg_a_text in surfaces_a.http_responses.items():
        assert surfaces_b.http_responses[endpoint_id] == leg_a_text, (
            f"legs diverged on {endpoint_id}"
        )
