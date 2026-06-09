"""Whole-process external equivalence against the committed goldens.

Boots the backend as a real subprocess under test mode, drives the
``basic_hunt_10_events`` scenario purely through env vars and HTTP (no
in-process pokes anywhere), captures the three equivalence surfaces (the
``events.jsonl`` publish stream, the data-dir SQLite file, the curated
hydration GET set) and proves them byte-identical to the scenario's
committed goldens through the same Python emitters the committed
raw-capture fixtures are proven through.

This is the whole-process control leg of the cross-language equivalence
runner: it demonstrates an externally-driven backend process reaches a
drained, fingerprint-comparable state that reproduces the in-process
goldens exactly, so a second backend implementation driven the same way
is graded against the same bytes with this run as the known-good
reference. The process lifecycle lives in
``backend.testing.external_process``, shared with the dual-process
smoke (``test_dual_process_equivalence.py``).
"""

from __future__ import annotations

from contextlib import ExitStack
from pathlib import Path

from backend.testing.external_process import (
    ExternalBackendLeg,
    expected_surfaces,
    free_ports,
)

SCENARIO = Path(__file__).parent / "corpus" / "scripted" / "basic_hunt_10_events"


def test_external_process_run_reproduces_committed_goldens(tmp_path):
    """Boot, replay, capture, byte-compare: the full external contract."""
    (port,) = free_ports()
    leg = ExternalBackendLeg(SCENARIO, tmp_path, port=port)
    with ExitStack() as stack:
        leg.start()
        stack.callback(leg.shutdown)
        leg.wait_ready()
        leg.replay()
        leg.capture_http()

    actual = leg.surfaces()
    expected = expected_surfaces(SCENARIO)
    assert actual.fingerprint == expected.fingerprint
    assert actual.db_state == expected.db_state
    assert set(actual.http_responses) == set(expected.http_responses)
    for endpoint_id, expected_text in expected.http_responses.items():
        assert actual.http_responses[endpoint_id] == expected_text, (
            f"HTTP golden diverged for {endpoint_id}"
        )
