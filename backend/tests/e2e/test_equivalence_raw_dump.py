"""On-demand dumper for the equivalence runner's raw-capture fixtures.

The cross-language equivalence runner proves the Rust DB-snapshot and HTTP
fingerprint emitters byte-identical to the committed Python goldens by feeding
both legs the SAME raw inputs (the pre-normalisation bus events, DB rows, and
HTTP responses a replay of ``basic_hunt_10_events`` produces). Those raw inputs
are committed under the scenario's ``raw_captures/`` directory so the Rust
proof and the Python faithfulness check are hermetic (no replay at test time).

This module regenerates those committed fixtures from a live replay. It is
gated behind ``EO_DUMP_RAW`` so it never runs in the normal suite (it writes
committed files); regenerate with::

    EO_DUMP_RAW=1 .venv/Scripts/python.exe -m pytest \
        backend/tests/e2e/test_equivalence_raw_dump.py -q

The replays here mirror ``test_basic_hunt_10_events`` (fingerprint + DB) and
``test_http_fingerprint_scenarios`` (HTTP) exactly, so the dumped raw inputs are
the same ones the committed goldens were generated from. The faithfulness test
(``test_equivalence_emitters.py``) then asserts the Python emitter over these
committed raw inputs still reproduces the committed goldens, so a stale dump
cannot pass silently.
"""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
from typing import Any

import pytest

from backend.dependencies import get_services
from backend.testing import db_snapshot
from backend.testing.clock import MockClock
from backend.testing.clock_plan import load_clock_plan
from backend.testing.http_fingerprint import HYDRATION_ENDPOINTS
from backend.testing.replay import replay_scenario, wait_for_drain
from backend.testing.wire import wire

pytestmark = pytest.mark.skipif(
    not os.environ.get("EO_DUMP_RAW"),
    reason="raw-capture dumper; set EO_DUMP_RAW=1 to regenerate the committed fixtures",
)

SCENARIO_NAME = "basic_hunt_10_events"


def _raw_captures_dir(scenario: Path) -> Path:
    out = scenario / "raw_captures"
    out.mkdir(parents=True, exist_ok=True)
    return out


def _write_json(path: Path, payload: Any) -> None:
    path.write_text(
        json.dumps(payload, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def test_dump_fingerprint_and_db(
    make_e2e_pipeline,
    scenario_clock,
    corpus_root: Path,
    golden_set,
    in_memory_db,
) -> None:
    """Replay the scenario and dump raw bus events + raw catalogue rows."""
    scenario = corpus_root / "scripted" / SCENARIO_NAME
    clock, plan = scenario_clock(scenario)
    bus, tracker, watcher, chatlog = make_e2e_pipeline(clock=clock)
    goldens = golden_set(scenario)
    goldens.recorder.install(bus)

    tracker.start_session()
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    clock.advance(plan.step_seconds)
    tracker.stop_session()

    raw_events = [
        {"topic": topic, "payload": wire(payload)}
        for topic, payload in goldens.recorder.events
    ]
    raw_db_rows = {
        spec.name: db_snapshot._fetch_rows(in_memory_db, spec)
        for spec in db_snapshot.CATALOGUE
    }

    out = _raw_captures_dir(scenario)
    _write_json(out / "events.json", raw_events)
    _write_json(out / "db_rows.json", raw_db_rows)


def test_dump_http_responses(
    make_e2e_http_pipeline,
    corpus_root: Path,
) -> None:
    """Replay the scenario through the lifespan app and dump raw responses."""
    scenario = corpus_root / "scripted" / SCENARIO_NAME
    plan = load_clock_plan(scenario)
    captured: list[dict[str, Any]] = []

    with make_e2e_http_pipeline(scenario) as (client, chatlog, watcher):
        services = get_services()
        app_clock = services.clock
        assert isinstance(app_clock, MockClock)
        tracker = services.tracker
        session = tracker.start_session()
        try:
            replay_scenario(scenario, chatlog)
            wait_for_drain(watcher, chatlog)
            app_clock.advance(plan.step_seconds)
            tracker.stop_session()
        finally:
            if tracker.is_tracking:
                tracker.stop_session()

        for endpoint_id, method, path_template in HYDRATION_ENDPOINTS:
            path = path_template.format(session_id=session.id)
            response = client.get(path)
            assert response.status_code == 200, (
                f"{endpoint_id} returned {response.status_code}: {response.text!r}"
            )
            captured.append(
                {
                    "endpoint_id": endpoint_id,
                    "method": method,
                    "path": path,
                    "query": {},
                    "status_code": response.status_code,
                    "headers": dict(response.headers),
                    "body_b64": base64.b64encode(response.content).decode("ascii"),
                }
            )

    out = _raw_captures_dir(scenario)
    _write_json(out / "http_responses.json", captured)
