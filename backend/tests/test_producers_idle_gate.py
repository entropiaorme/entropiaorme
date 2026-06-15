"""The producer-idle gate (``ENTROPIAORME_PRODUCERS_IDLE``).

When the native substrate owns production, the sidecar must stand its
producers down: it still serves every proxied HTTP route, but it starts
no chat-log tail thread and writes nothing into the database from a
producer. These tests boot the real FastAPI lifespan in-process (via
``TestClient``, never a subprocess, so no uvicorn child can leak) against
a temp data dir whose ``settings.json`` points the watcher at a temp
chat-log, and assert:

* idle on -> no watcher thread, a fed chat-log yields no producer DB
  writes, while a proxied read still serves;
* idle off (the negative control) -> the watcher tails and the same fed
  chat-log produces the expected kill rows.

The gate touches only producer *startup*, so neither arm changes any
HTTP or database-state response shape.
"""

from __future__ import annotations

import json
import os
import shutil
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

# A combat tick then one loot tick (both loot lines share a timestamp,
# so they group into a single loot event): exactly one kill closes.
SCENARIO_LINES = [
    "2026-05-19 10:00:01 [System] [] You inflicted 12.0 points of damage",
    "2026-05-19 10:00:02 [System] [] You received Shrapnel x (500) Value: 5.00 PED",
    "2026-05-19 10:00:02 [System] [] You received Wool Value: 1.50 PED",
]


def _append(chatlog: Path, lines: list[str]) -> None:
    with chatlog.open("a", encoding="utf-8") as handle:
        for line in lines:
            handle.write(line + "\n")
        handle.flush()


_tmp_factory: pytest.TempPathFactory


@pytest.fixture(autouse=True)
def _bind_tmp_factory(tmp_path_factory: pytest.TempPathFactory) -> None:
    """Root the module's temp dirs under pytest's auto-rotated basetemp.

    ``_lifespan`` is a plain contextmanager entered from test bodies, not
    through the fixture protocol, so it cannot request ``tmp_path_factory``
    itself. Binding it here keeps every helper-created dir under the tree
    pytest prunes, instead of the OS temp directory an interrupted run never
    cleans.
    """
    global _tmp_factory
    _tmp_factory = tmp_path_factory


@contextmanager
def _lifespan(*, idle: bool) -> Iterator[tuple[TestClient, Path]]:
    """Boot ``backend.main.app``'s lifespan against a temp data dir.

    Yields ``(client, chatlog_path)``. The data dir's ``settings.json``
    points the in-lifespan watcher at the temp chat-log. The idle flag is
    set in the environment before the lifespan reads it on TestClient
    enter and restored on exit.
    """
    data_dir = _tmp_factory.mktemp("idle_gate_data")
    chatlog = _tmp_factory.mktemp("idle_gate_log") / "chat_testing.log"
    chatlog.touch()
    (data_dir / "settings.json").write_text(
        json.dumps({"chatlog_path": str(chatlog)}),
        encoding="utf-8",
    )

    from backend.main import BACKEND_PORT, app

    saved = {
        "ENTROPIAORME_DATA_DIR": os.environ.get("ENTROPIAORME_DATA_DIR"),
        "ENTROPIAORME_PRODUCERS_IDLE": os.environ.get("ENTROPIAORME_PRODUCERS_IDLE"),
        "ENTROPIA_TEST_CLOCK_START": os.environ.get("ENTROPIA_TEST_CLOCK_START"),
    }
    os.environ["ENTROPIAORME_DATA_DIR"] = str(data_dir)
    if idle:
        os.environ["ENTROPIAORME_PRODUCERS_IDLE"] = "1"
    else:
        os.environ.pop("ENTROPIAORME_PRODUCERS_IDLE", None)
    # A frozen clock keeps the stamped instants deterministic.
    os.environ["ENTROPIA_TEST_CLOCK_START"] = "2026-05-19T10:00:00"

    try:
        with TestClient(app, base_url=f"http://localhost:{BACKEND_PORT}") as client:
            yield client, chatlog
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(data_dir, ignore_errors=True)
        shutil.rmtree(chatlog.parent, ignore_errors=True)


def _kill_count() -> int:
    from backend.dependencies import get_services

    row = get_services().app_db.conn.execute("SELECT COUNT(*) FROM kills").fetchone()
    return int(row[0])


def test_idle_gate_starts_no_watcher_and_writes_nothing_while_routes_serve() -> None:
    with _lifespan(idle=True) as (client, chatlog):
        from backend.dependencies import get_services

        services = get_services()

        # No producer machinery started: the watcher's tail thread is not
        # running even though the service object exists for the routes.
        assert not services.chatlog_watcher.is_running, (
            "idle mode must not start the chat-log tail thread"
        )

        # A proxied read still serves: the route surface is unaffected.
        response = client.get("/api/health")
        assert response.status_code == 200, "reads still serve in idle mode"

        # Start a session and feed the chat-log. With no watcher tailing,
        # the lines are never parsed into the bus, so no kill is recorded.
        services.tracker.start_session()
        _append(chatlog, SCENARIO_LINES)
        # Give a (hypothetical) tail loop several intervals to act; in idle
        # mode there is none, so this is pure slack before the assertion.
        time.sleep(0.6)
        assert _kill_count() == 0, "idle mode writes no producer rows"

        if services.tracker.is_tracking:
            services.tracker.stop_session()


def test_negative_control_unset_gate_tails_and_records_kills() -> None:
    with _lifespan(idle=False) as (client, chatlog):
        from backend.dependencies import get_services

        services = get_services()
        watcher = services.chatlog_watcher

        # The producer spine is live: the watcher tails its temp log.
        assert watcher.is_running, "the default path starts the watcher"

        response = client.get("/api/health")
        assert response.status_code == 200

        services.tracker.start_session()
        _append(chatlog, SCENARIO_LINES)
        watcher.wait_until_drained(len(SCENARIO_LINES), timeout=10.0)
        services.tracker.stop_session()

        assert _kill_count() == 1, "the watcher fed one loot group into one kill"
