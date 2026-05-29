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
    try:
        yield HuntTracker(bus, conn)
    finally:
        conn.close()


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

    # Stopping a session also runs the prospect-summary write through
    # write_session_summary. This session has no kills and no skill gains,
    # so it fails the qualifying filters: the decline branch must leave no
    # session_summaries row behind (rather than persisting an empty one).
    summary_row = tracker._db.execute(
        "SELECT 1 FROM session_summaries WHERE session_id = ?",
        (started.id,),
    ).fetchone()
    assert summary_row is None


def test_stop_tracking_without_active_session_raises_409(tracker, monkeypatch):
    monkeypatch.setattr(
        tracking, "get_services", lambda: SimpleNamespace(tracker=tracker)
    )

    with pytest.raises(HTTPException) as exc:
        tracking.stop_tracking()

    assert exc.value.status_code == 409


def test_stop_tracking_raises_500_when_session_unexpectedly_missing(monkeypatch):
    """The post-`is_tracking` guard surfaces a clean 500 if the tracker reports
    active but yields no session (an invariant breach), rather than an
    AttributeError on the None session."""
    fake = SimpleNamespace(is_tracking=True, stop_session=lambda: None)
    monkeypatch.setattr(tracking, "get_services", lambda: SimpleNamespace(tracker=fake))

    with pytest.raises(HTTPException) as exc:
        tracking.stop_tracking()

    assert exc.value.status_code == 500
