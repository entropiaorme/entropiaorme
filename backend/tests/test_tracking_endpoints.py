"""Unit cover for the tracking stop endpoint.

Drives ``stop_tracking`` against an in-memory ``HuntTracker`` through the
service-locator seam, covering both the active-session stop path and the
no-active-session guard.
"""

import sqlite3
from types import SimpleNamespace

import pytest
from fastapi import HTTPException

from backend.core.event_bus import EventBus
from backend.routers import tracking
from backend.tracking.tracker import HuntTracker


@pytest.fixture
def tracker():
    bus = EventBus()
    conn = sqlite3.connect(":memory:", check_same_thread=False)
    return HuntTracker(bus, conn)


def test_stop_tracking_returns_session_summary(tracker, monkeypatch):
    monkeypatch.setattr(
        tracking, "get_services", lambda: SimpleNamespace(tracker=tracker)
    )
    started = tracker.start_session()

    result = tracking.stop_tracking()

    assert result["session_id"] == started.id
    assert result["kill_count"] == 0
    assert result["started_at"]  # non-empty ISO timestamp
    assert result["ended_at"] is not None
    assert not tracker.is_tracking


def test_stop_tracking_without_active_session_raises_409(tracker, monkeypatch):
    monkeypatch.setattr(
        tracking, "get_services", lambda: SimpleNamespace(tracker=tracker)
    )

    with pytest.raises(HTTPException) as exc:
        tracking.stop_tracking()

    assert exc.value.status_code == 409
