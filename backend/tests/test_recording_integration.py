"""End-to-end smoke for recording mode over HTTP.

Mounts the real recording router wired to a real ``RecordingController`` and
drives a full start -> capture -> stop -> finalise -> determinism-verify cycle
over a ``TestClient``. The five live services are faked (no live game) and the
chatlog fake's installed tap is driven with synthetic lines, so the controller
finalises a genuine bundle and runs the real chatlog determinism check against
a temp corpus.

This proves the request -> dev-gate -> controller -> finalise -> verify path
composes end to end, without booting the heavy full-app lifespan or writing
into the committed corpus.
"""

from __future__ import annotations

from types import SimpleNamespace

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

import backend.dependencies as deps
from backend.routers import recording
from backend.testing.recording_controller import RecordingController

CHAT_LINES = [
    "2026-05-21 09:00:00 [System] [] You inflicted 25.0 points of damage\n",
    "2026-05-21 09:00:01 [System] [] You received Shrapnel x (300) Value: 3.00 PED\n",
]


class _FakeChatlog:
    def __init__(self):
        self.tap = None

    def set_line_tap(self, tap):
        self.tap = tap

    def clear_line_tap(self):
        self.tap = None


class _FakeScan:
    def set_capture_tap(self, tap):
        pass

    def clear_capture_tap(self):
        pass


class _FakeKeys:
    def set_key_tap(self, tap):
        pass

    def clear_key_tap(self):
        pass


@pytest.fixture
def restore_services():
    saved = deps._services
    yield
    deps._services = saved


def test_recording_round_trip_over_http(tmp_path, restore_services):
    chatlog = _FakeChatlog()
    controller = RecordingController(
        chatlog_watcher=chatlog,  # type: ignore[arg-type]
        skill_scan_manual=_FakeScan(),
        repair_ocr=_FakeScan(),
        hotbar_listener=_FakeKeys(),
        spacebar_capture_listener=_FakeKeys(),
        corpus_root=tmp_path / "corpus",
    )
    config = SimpleNamespace(developer_mode_enabled=True)
    deps.set_services(
        SimpleNamespace(  # type: ignore[arg-type]
            config_service=SimpleNamespace(get=lambda: config),
            recording_controller=controller,
        )
    )

    app = FastAPI()
    app.include_router(recording.router, prefix="/api")
    client = TestClient(app)

    # Start.
    started = client.post("/api/recording/start")
    assert started.status_code == 200
    assert started.json()["state"] == "recording"

    # Feed synthetic chat lines through the tap the controller installed.
    assert chatlog.tap is not None
    for line in CHAT_LINES:
        chatlog.tap(line)

    # Live status reflects the captured count.
    status = client.get("/api/recording/status")
    assert status.status_code == 200
    assert status.json()["lines"] == len(CHAT_LINES)

    # Stop + finalise.
    stopped = client.post(
        "/api/recording/stop",
        json={
            "scenario_name": "smoke_capture",
            "description": "http round-trip smoke",
            "surfaces": ["tracking-kill-creation"],
        },
    )
    assert stopped.status_code == 200
    assert stopped.json()["determinism"] == "ok"

    target = tmp_path / "corpus" / "recorded" / "smoke_capture"
    assert (target / "chat_replay.log").read_text(encoding="utf-8") == "".join(
        CHAT_LINES
    )
    assert (target / "expected" / "fingerprint.jsonl").exists()
    assert (target / "expected" / "db_state.json").exists()
    assert (target / "metadata.yaml").exists()

    # Returned to idle.
    assert client.get("/api/recording/status").json()["state"] == "idle"
