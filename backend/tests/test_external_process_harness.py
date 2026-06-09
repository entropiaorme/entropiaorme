"""Unit guards for the external-process harness's edges.

The happy path of ``backend.testing.external_process`` is exercised
end-to-end by the e2e equivalence tests; these guards pin the cheap
edges that need no real backend boot: port allocation, lifecycle misuse,
a child that dies during boot, and diagnostics when no log exists.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

from backend.testing.external_process import ExternalBackendLeg, free_ports

SCENARIO = (
    Path(__file__).parent / "e2e" / "corpus" / "scripted" / "basic_hunt_10_events"
)


def test_free_ports_are_distinct():
    ports = free_ports(3)
    assert len(ports) == len(set(ports)) == 3
    assert all(1 <= port <= 65535 for port in ports)


def test_unstarted_leg_refuses_lifecycle_calls(tmp_path):
    leg = ExternalBackendLeg(SCENARIO, tmp_path, port=1)
    with pytest.raises(RuntimeError, match="not running"):
        leg.wait_ready()
    with pytest.raises(RuntimeError, match="not running"):
        leg.replay()
    with pytest.raises(RuntimeError, match="not running"):
        leg.capture_http()


def test_shutdown_is_idempotent_on_an_unstarted_leg(tmp_path):
    leg = ExternalBackendLeg(SCENARIO, tmp_path, port=1)
    leg.shutdown()
    leg.shutdown()


def test_tail_without_a_log_reports_so(tmp_path):
    leg = ExternalBackendLeg(SCENARIO, tmp_path, port=1)
    assert leg.tail() == "<no child log captured>"


def test_child_exiting_during_boot_surfaces_its_exit_code(tmp_path):
    (port,) = free_ports()
    leg = ExternalBackendLeg(
        SCENARIO,
        tmp_path,
        port=port,
        command=(sys.executable, "-c", "import sys; sys.exit(3)"),
    )
    leg.start()
    try:
        with pytest.raises(RuntimeError, match=r"exited during boot \(rc=3\)"):
            leg.wait_ready()
    finally:
        leg.shutdown()
