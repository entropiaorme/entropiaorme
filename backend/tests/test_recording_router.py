"""Tests for the developer-only recording router.

Mounts just the recording router on a minimal app and injects a fake services
container so the gate and error-mapping logic is covered without booting the
full backend lifespan (that integration path is exercised separately).
"""

from __future__ import annotations

from types import SimpleNamespace

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

import backend.dependencies as deps
from backend.routers import recording
from backend.testing.recording_controller import (
    RecordingStateError,
    RecordingValidationError,
)


class _FakeController:
    def __init__(self, *, stop_raises: Exception | None = None):
        self.calls: list = []
        self._stop_raises = stop_raises

    def start(self):
        self.calls.append("start")
        return {"state": "recording", "started_at": "t", "lines": 0, "captures": 0, "keystrokes": 0}

    def status(self):
        return {"state": "idle", "started_at": None, "lines": 0, "captures": 0, "keystrokes": 0}

    def stop(self, meta):
        if self._stop_raises is not None:
            raise self._stop_raises
        self.calls.append(("stop", meta))
        return {"finalized_path": "/x/y", "determinism": "ok"}

    def abort(self):
        self.calls.append("abort")
        return {"state": "idle"}


@pytest.fixture
def restore_services():
    saved = deps._services
    yield
    deps._services = saved


def _client(developer_mode: bool, controller) -> TestClient:
    app = FastAPI()
    app.include_router(recording.router, prefix="/api")
    config = SimpleNamespace(developer_mode_enabled=developer_mode)
    deps.set_services(
        SimpleNamespace(
            config_service=SimpleNamespace(get=lambda: config),
            recording_controller=controller,
        )
    )
    return TestClient(app)


def test_start_forbidden_when_developer_mode_off(restore_services):
    fake = _FakeController()
    client = _client(False, fake)
    resp = client.post("/api/recording/start")
    assert resp.status_code == 403
    assert fake.calls == []


def test_start_ok_when_developer_mode_on(restore_services):
    fake = _FakeController()
    client = _client(True, fake)
    resp = client.post("/api/recording/start")
    assert resp.status_code == 200
    assert resp.json()["state"] == "recording"
    assert "start" in fake.calls


def test_status_ok_when_developer_mode_on(restore_services):
    client = _client(True, _FakeController())
    resp = client.get("/api/recording/status")
    assert resp.status_code == 200
    assert resp.json()["state"] == "idle"


def test_status_forbidden_when_off(restore_services):
    resp = _client(False, _FakeController()).get("/api/recording/status")
    assert resp.status_code == 403


def test_stop_passes_body_through(restore_services):
    fake = _FakeController()
    client = _client(True, fake)
    resp = client.post("/api/recording/stop", json={"scenario_name": "rec_one", "surfaces": ["tracking"]})
    assert resp.status_code == 200
    assert resp.json()["determinism"] == "ok"
    (_tag, meta) = fake.calls[0]
    assert meta["scenario_name"] == "rec_one"
    assert meta["surfaces"] == ["tracking"]


def test_stop_validation_error_maps_to_400(restore_services):
    fake = _FakeController(stop_raises=RecordingValidationError("bad slug"))
    resp = _client(True, fake).post("/api/recording/stop", json={"scenario_name": "X"})
    assert resp.status_code == 400


def test_stop_state_error_maps_to_409(restore_services):
    fake = _FakeController(stop_raises=RecordingStateError("not recording"))
    resp = _client(True, fake).post("/api/recording/stop", json={"scenario_name": "x"})
    assert resp.status_code == 409


def test_abort_ok_when_developer_mode_on(restore_services):
    fake = _FakeController()
    resp = _client(True, fake).post("/api/recording/abort")
    assert resp.status_code == 200
    assert "abort" in fake.calls
