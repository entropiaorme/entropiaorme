"""Composition-root test-mode wiring and the test-only API surface.

Boots fresh apps (never the shared module-level one) with the test-mode
env seams set and unset, and pins both sides of the wiring contract:
production wiring is byte-for-byte what it was (real input sources, the
configured chatlog, no event sink, no test routes), while test mode
selects the mock keystroke sources, the redirected chatlog (created if
missing), the fixture capturers, and the events.jsonl sink, and exposes
the external drive surface (drain state + the synchronous replay
command an external process uses where the in-process suite pokes
objects directly).

Avoids the corpus-side helpers deliberately: everything here drives the
app the way an external harness would, through env and HTTP.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from fastapi.testclient import TestClient

from backend.testing.config import TestModeConfig
from backend.testing.keystroke_source import (
    MockKeystrokeSource,
    PynputKeystrokeSource,
)

_SCENARIO = Path(__file__).parent / "e2e" / "corpus" / "scripted" / "single_mob_hunt"
_ORIGIN = {"Origin": "tauri://localhost"}


def _fresh_client() -> TestClient:
    """A TestClient over a freshly-built app, reading env at build time.

    The module-level app is built at import with whatever env the test
    process started with; these tests need router registration to see the
    per-test env, so each builds its own app. The base URL keeps the Host
    header inside the origin-guard allowlist.
    """
    from backend.main import BACKEND_PORT, create_app

    return TestClient(create_app(), base_url=f"http://localhost:{BACKEND_PORT}")


def _set_test_env(
    monkeypatch,
    tmp_path: Path,
    *,
    scenario: Path | None = _SCENARIO,
    chatlog: str | Path | None = None,
    clock_start: str | None = "2026-01-01T00:00:00",
) -> Path:
    """Set the standard external-boot env; returns the data dir."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(exist_ok=True)
    monkeypatch.setenv("ENTROPIAORME_DATA_DIR", str(data_dir))
    monkeypatch.setenv("ENTROPIA_TEST_MODE", "1")
    if scenario is not None:
        monkeypatch.setenv("ENTROPIA_TEST_SCENARIO_DIR", str(scenario))
    else:
        monkeypatch.delenv("ENTROPIA_TEST_SCENARIO_DIR", raising=False)
    if chatlog is not None:
        monkeypatch.setenv("ENTROPIA_TEST_CHATLOG", str(chatlog))
    else:
        monkeypatch.delenv("ENTROPIA_TEST_CHATLOG", raising=False)
    if clock_start is not None:
        monkeypatch.setenv("ENTROPIA_TEST_CLOCK_START", clock_start)
    else:
        monkeypatch.delenv("ENTROPIA_TEST_CLOCK_START", raising=False)
    return data_dir


def test_production_wiring_is_unchanged_without_test_mode(tmp_path, monkeypatch):
    """No test-mode env: real input sources, no sink, no test routes."""
    data_dir = tmp_path / "data"
    data_dir.mkdir()
    monkeypatch.setenv("ENTROPIAORME_DATA_DIR", str(data_dir))
    for name in (
        "ENTROPIA_TEST_MODE",
        "ENTROPIA_TEST_SCENARIO_DIR",
        "ENTROPIA_TEST_CHATLOG",
        "ENTROPIA_TEST_FIXTURE_DIR",
        "ENTROPIA_TEST_CLOCK_START",
    ):
        monkeypatch.delenv(name, raising=False)

    from backend.main import BACKEND_PORT, create_app

    app = create_app()
    with TestClient(app, base_url=f"http://localhost:{BACKEND_PORT}") as client:
        from backend.dependencies import get_services

        svc = get_services()
        assert not svc.test_mode.enabled
        assert isinstance(svc.hotbar_keystroke_source, PynputKeystrokeSource)
        assert isinstance(svc.spacebar_keystroke_source, PynputKeystrokeSource)
        # The test-only surface does not exist: hard 404, not a 403 gate.
        assert client.get("/api/testing/drain").status_code == 404
        # And it is absent from the schema the snapshot/contract suites see.
        assert not any(
            path.startswith("/api/testing") for path in app.openapi()["paths"]
        )

    assert not (data_dir / "events.jsonl").exists()


def test_test_mode_selects_the_seams(tmp_path, monkeypatch):
    """Test mode: mock sources, redirected chatlog (created), sink installed."""
    sink_log = tmp_path / "replay_sink.log"  # deliberately not pre-created
    data_dir = _set_test_env(monkeypatch, tmp_path, chatlog=sink_log)

    with _fresh_client() as client:
        from backend.dependencies import get_services

        svc = get_services()
        assert svc.test_mode.enabled
        assert isinstance(svc.hotbar_keystroke_source, MockKeystrokeSource)
        assert isinstance(svc.spacebar_keystroke_source, MockKeystrokeSource)
        # The chatlog was redirected to the harness file and created, so the
        # watcher is actually tailing (a missing file would silently no-op).
        watcher = svc.chatlog_watcher
        assert watcher.path == sink_log
        assert sink_log.exists()
        assert watcher.is_running
        # The sink is live from the first publish.
        assert (data_dir / "events.jsonl").exists()
        assert client.get("/api/testing/drain", headers=_ORIGIN).status_code == 200


def test_replay_runs_the_scenario_to_drained(tmp_path, monkeypatch):
    """The synchronous replay command externalises the whole driver sequence."""
    data_dir = _set_test_env(
        monkeypatch, tmp_path, chatlog=tmp_path / "replay_sink.log"
    )

    with _fresh_client() as client:
        response = client.post("/api/testing/replay", headers=_ORIGIN)
        assert response.status_code == 200, response.text
        body = response.json()
        assert body["drained"] is True
        assert body["lines_streamed"] == 6  # the committed scenario's line count
        assert body["lines_seen"] == body["lines_streamed"]
        assert body["session_id"]

        drain = client.get("/api/testing/drain", headers=_ORIGIN).json()
        assert drain == {"lines_seen": 6, "has_pending_tick": False}

        # The clock-instant precondition makes a second replay in the same
        # process a refused conflict, not a silently-diverging rerun.
        second = client.post("/api/testing/replay", headers=_ORIGIN)
        assert second.status_code == 409
        assert "does not match the scenario clock plan start" in second.text

    # After shutdown the sink carries the complete publish-order stream:
    # session lifecycle, per-event topics, tick boundaries, and the typed
    # domain envelope (the proof it is a superset of the SSE surface).
    lines = [
        json.loads(line)
        for line in (data_dir / "events.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    topics = [line["topic"] for line in lines]
    assert topics[0] == "session_started"
    assert "combat" in topics
    assert "loot_group" in topics
    assert "tick_flushed" in topics
    assert "tracking.session.updated" in topics
    assert "session_stopped" in topics
    # Wire form, not normalised: instants are ISO strings (no <TS_N>
    # symbols, matching the committed raw-capture convention) and ids are
    # live UUIDs shared across the stream (no <UUID_N> symbols).
    first_tick = lines[topics.index("tick_flushed")]["payload"]
    assert first_tick["timestamp"] == "2026-05-19T10:00:00"
    started = lines[topics.index("session_started")]["payload"]
    stopped = lines[topics.index("session_stopped")]["payload"]
    assert "<UUID" not in started["session_id"]
    assert stopped["session_id"] == started["session_id"]


def test_replay_refuses_without_a_scenario(tmp_path, monkeypatch):
    _set_test_env(monkeypatch, tmp_path, scenario=None, clock_start=None)

    with _fresh_client() as client:
        response = client.post("/api/testing/replay", headers=_ORIGIN)
        assert response.status_code == 409
        assert "No scenario loaded" in response.text


def test_replay_refuses_on_a_real_clock(tmp_path, monkeypatch):
    _set_test_env(
        monkeypatch, tmp_path, chatlog=tmp_path / "replay_sink.log", clock_start=None
    )

    with _fresh_client() as client:
        response = client.post("/api/testing/replay", headers=_ORIGIN)
        assert response.status_code == 409
        assert "Deterministic clock required" in response.text


def test_replay_refuses_to_stream_the_source_into_itself(tmp_path, monkeypatch):
    """Tailing the committed scenario file is refused, not silently no-opped.

    The watcher seeks to end-of-file at start, so tailing the source could
    never replay it; worse, the replay command would append the committed
    file onto itself. The command refuses with the corrective env hint.
    """
    _set_test_env(monkeypatch, tmp_path, chatlog=_SCENARIO / "chat_replay.log")

    with _fresh_client() as client:
        response = client.post("/api/testing/replay", headers=_ORIGIN)
        assert response.status_code == 409
        assert "ENTROPIA_TEST_CHATLOG" in response.text


def test_handlers_re_check_the_gate(tmp_path, monkeypatch):
    """Defence in depth: even a registered route is inert when the gate is off."""
    _set_test_env(monkeypatch, tmp_path, chatlog=tmp_path / "replay_sink.log")

    with _fresh_client() as client:
        from backend.dependencies import get_services

        svc = get_services()
        live_overlay = svc.test_mode
        svc.test_mode = TestModeConfig()  # simulate a mis-registered surface
        try:
            assert client.get("/api/testing/drain", headers=_ORIGIN).status_code == 403
            assert (
                client.post("/api/testing/replay", headers=_ORIGIN).status_code == 403
            )
        finally:
            svc.test_mode = live_overlay


def test_frozen_builds_refuse_test_mode(monkeypatch):
    """A packaged install never exposes the harness, whatever the env says."""
    monkeypatch.setenv("ENTROPIA_TEST_MODE", "1")
    monkeypatch.setattr(sys, "frozen", True, raising=False)

    from backend.main import _build_test_mode, create_app

    assert not _build_test_mode().enabled
    app = create_app()
    assert not any(
        getattr(route, "path", "").startswith("/api/testing") for route in app.routes
    )
