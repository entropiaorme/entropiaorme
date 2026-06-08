"""Tests for the per-scenario clock-plan loader.

Pins ``load_clock_plan``'s contract: a valid ``clock:`` block yields a frozen
start and a positive step, and malformed metadata fails loudly with scenario
context rather than a bare parser error or an ``AttributeError``.
"""

from __future__ import annotations

from datetime import datetime

import pytest

from backend.testing.clock_plan import ClockPlan, load_clock_plan


def _write(scenario_dir, text: str) -> None:
    (scenario_dir / "metadata.yaml").write_text(text, encoding="utf-8")


def test_loads_a_valid_clock_block(tmp_path):
    _write(tmp_path, "clock:\n  start: 2026-01-01T00:00:00\n  step_seconds: 1.5\n")
    plan = load_clock_plan(tmp_path)
    assert isinstance(plan, ClockPlan)
    assert plan.start == datetime(2026, 1, 1, 0, 0, 0)
    assert plan.step_seconds == 1.5


def test_non_mapping_root_is_rejected_with_scenario_context(tmp_path):
    # A YAML list at the root would make ``doc.get`` raise AttributeError; the
    # loader must reject it with the scenario name instead.
    _write(tmp_path, "- not\n- a\n- mapping\n")
    with pytest.raises(ValueError, match="must be a mapping at") as exc:
        load_clock_plan(tmp_path)
    assert tmp_path.name in str(exc.value)


def test_malformed_iso_start_reports_scenario_context(tmp_path):
    _write(tmp_path, 'clock:\n  start: "not-a-date"\n  step_seconds: 1.0\n')
    with pytest.raises(ValueError, match="valid ISO-8601 instant") as exc:
        load_clock_plan(tmp_path)
    assert tmp_path.name in str(exc.value)


def test_timezone_aware_start_is_rejected(tmp_path):
    _write(
        tmp_path,
        'clock:\n  start: "2026-01-01T00:00:00+00:00"\n  step_seconds: 1.0\n',
    )
    with pytest.raises(ValueError, match="must be naive"):
        load_clock_plan(tmp_path)
