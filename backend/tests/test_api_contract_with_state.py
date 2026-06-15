"""API contract under replayed scenario state.

Sibling to ``test_api_contract.py``. The foundation contract suite
exercises every GET against a freshly-booted app, which catches the
many regressions that depend only on the spec and the empty-state
shape. The remaining failure mode is a regression that only manifests
once the backend holds non-empty state: a session-scoped field that
the spec declares optional but the runtime always emits with
non-empty data, an analytics row that fails to satisfy the spec
because a join surfaces an unexpected null, and so on.

This module boots the same lifespan, then drives a scripted scenario
through the production tracker before parametrise runs, so every
generated case lands against an app that has lived through a real
hunt: one closed session, three kills, a populated ledger, the codex
service holding the snapshot tables it always does. The schemathesis
checks are identical to the foundation suite (``not_a_server_error``
on every response; ``response_schema_conformance`` on 2xx); only the
state under them changes.

Scenario choice: ``basic_hunt_10_events`` is the smallest scenario
that exercises both the tracking surface (three kills + loot ticks)
and the post-session shape (session_summaries cache populated, kills
attributed). Adding more scenarios is a one-line change to
``REPLAYED_SCENARIOS``; the fixture loops through every entry.
"""

from __future__ import annotations

import io
import json
import os
import shutil
from contextlib import redirect_stdout
from pathlib import Path

import pytest
import schemathesis
from fastapi.testclient import TestClient
from hypothesis import settings
from schemathesis import GenerationMode
from schemathesis.checks import not_a_server_error
from schemathesis.specs.openapi.checks import response_schema_conformance

import backend.routers.demo as demo_module
from backend.dependencies import get_services
from backend.main import BACKEND_PORT, app
from backend.testing.replay import wait_for_drain

pytestmark = pytest.mark.contract

BASE_URL = f"http://localhost:{BACKEND_PORT}"
ALLOWED_ORIGIN = "tauri://localhost"
REQUEST_HEADERS = {"Origin": ALLOWED_ORIGIN}

REPLAYED_SCENARIOS: tuple[str, ...] = ("basic_hunt_10_events",)

schema = schemathesis.openapi.from_asgi("/openapi.json", app)
schema.config.update(base_url=BASE_URL, headers=REQUEST_HEADERS)
schema.config.generation.update(
    modes=[GenerationMode.POSITIVE], max_examples=8, deterministic=True
)
schema.config.phases.update(phases=["examples", "fuzzing"])

_INT64_MIN = -(2**63)
_INT64_MAX = 2**63 - 1


def _clamp_int64_params(params) -> None:
    if not params:
        return
    for key, value in list(params.items()):
        if isinstance(value, bool):
            continue
        if isinstance(value, int):
            params[key] = max(_INT64_MIN, min(_INT64_MAX, value))


def _stream_segment(source: Path, destination: Path) -> None:
    """Append ``source`` to ``destination`` line-by-line, flushing each."""
    lines = source.read_text(encoding="utf-8").splitlines(keepends=True)
    with destination.open("a", encoding="utf-8") as sink:
        for line in lines:
            sink.write(line)
            sink.flush()


@pytest.fixture(scope="module")
def contract_state_env(tmp_path_factory: pytest.TempPathFactory):
    """Boot the lifespan with a pre-seeded chatlog, then replay scenarios.

    Module-scoped: scenario replay + lifespan boot pay their cost once
    per contract-suite-with-state run, not per generated case. Tears
    down by stopping the lifespan, restoring demo + env state.
    """
    data_dir_str = str(tmp_path_factory.mktemp("contract_state_data"))
    demo_dir_str = str(tmp_path_factory.mktemp("contract_state_demo"))
    data_dir = Path(data_dir_str)
    demo_dir = Path(demo_dir_str)
    chatlog = data_dir / "chat_testing.log"
    chatlog.touch()

    (data_dir / "settings.json").write_text(
        json.dumps(
            {
                "chatlog_path": str(chatlog),
                "developer_mode_enabled": True,
            }
        ),
        encoding="utf-8",
    )

    from backend.scripts.demo_seed.__main__ import main as seed_demo

    with redirect_stdout(io.StringIO()):
        seed_demo(["--reseed", "--out", demo_dir_str])
    demo_db = demo_dir / "entropia_orme.db"

    original_resolver = demo_module._resolve_demo_db_path
    demo_module._resolve_demo_db_path = lambda: demo_db
    demo_module._state["conn"] = None
    demo_module._state["svc"] = None

    original_data_dir = os.environ.get("ENTROPIAORME_DATA_DIR")
    os.environ["ENTROPIAORME_DATA_DIR"] = data_dir_str

    corpus_root = Path(__file__).resolve().parent / "e2e" / "corpus" / "scripted"

    try:
        with TestClient(app, base_url=BASE_URL):
            tracker = get_services().tracker
            watcher = get_services().chatlog_watcher
            for scenario_name in REPLAYED_SCENARIOS:
                scenario_dir = corpus_root / scenario_name
                tracker.start_session()
                _stream_segment(scenario_dir / "chat_replay.log", chatlog)
                wait_for_drain(watcher, chatlog)
                secondary = scenario_dir / "chat_replay_after.log"
                if secondary.exists():
                    _stream_segment(secondary, chatlog)
                    wait_for_drain(watcher, chatlog)
                tracker.stop_session()
            yield
    finally:
        demo_module._resolve_demo_db_path = original_resolver
        demo_module._state["conn"] = None
        demo_module._state["svc"] = None
        if original_data_dir is None:
            os.environ.pop("ENTROPIAORME_DATA_DIR", None)
        else:
            os.environ["ENTROPIAORME_DATA_DIR"] = original_data_dir
        # ignore_errors: Windows may briefly hold the SQLite file open
        # past lifespan shutdown via the per-thread connection pool; a
        # leftover temp dir on a stuck handle is preferable to a teardown
        # crash that masks a real test failure.
        shutil.rmtree(data_dir, ignore_errors=True)
        shutil.rmtree(demo_dir, ignore_errors=True)


@schema.include(method="GET").parametrize()
@settings(deadline=None)
def test_get_endpoints_conform_with_state(case, contract_state_env):
    """Every GET response (under populated state) clears the same checks
    as the empty-state contract suite. The check set is identical so a
    spec-conformance regression that only appears with state surfaces
    here without dragging the empty-state contract surface."""
    if case.method.upper() != "GET":
        pytest.skip("contract suite covers the GET read surface only")

    _clamp_int64_params(case.path_parameters)
    _clamp_int64_params(case.query)

    response = case.call()
    assert response.status_code != 403, response.text

    checks = [not_a_server_error]
    if 200 <= response.status_code < 300:
        checks.append(response_schema_conformance)
    case.validate_response(response, checks=tuple(checks))
