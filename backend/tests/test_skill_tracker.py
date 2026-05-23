"""Tests for skill gain tracking during sessions."""

import tempfile
import os
from datetime import datetime

from backend.core.event_bus import EventBus
from backend.db.app_database import AppDatabase
from backend.services.skill_tracker import SkillTracker


def _make_tracker():
    """Create a SkillTracker with a fresh in-memory DB."""
    td = tempfile.mkdtemp()
    db = AppDatabase(os.path.join(td, "test.db"))
    bus = EventBus()
    tracker = SkillTracker(bus, db)
    return tracker, bus, db


def test_gain_ignored_without_session():
    tracker, bus, db = _make_tracker()
    bus.publish("skill_gain", {
        "skill_name": "Laser Weaponry Technology",
        "amount": 0.1234,
        "timestamp": datetime(2026, 3, 25, 12, 0, 0),
    })
    # No session active; gain should be ignored
    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 0
    db.close()


def test_gain_recorded_during_session():
    tracker, bus, db = _make_tracker()
    bus.publish("session_started", {"session_id": "test-session-1"})
    bus.publish("skill_gain", {
        "skill_name": "Laser Weaponry Technology",
        "amount": 0.1234,
        "timestamp": datetime(2026, 3, 25, 12, 0, 0),
    })
    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 1
    assert rows[0][2] is not None  # timestamp
    assert rows[0][3] == "Laser Weaponry Technology"
    assert abs(rows[0][4] - 0.1234) < 0.0001  # amount
    assert rows[0][5] is None  # ped_value is null (not calibrated)
    db.close()


def test_tt_value_computed_when_calibrated():
    tracker, bus, db = _make_tracker()
    # Insert calibration point first
    db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        ("Laser Weaponry Technology", 1000.0, 1711000000.0),
    )
    db.conn.commit()

    bus.publish("session_started", {"session_id": "test-session-2"})
    bus.publish("skill_gain", {
        "skill_name": "Laser Weaponry Technology",
        "amount": 100.0,
        "timestamp": datetime(2026, 3, 25, 12, 0, 0),
    })

    rows = db.conn.execute("SELECT ped_value FROM skill_gains").fetchall()
    assert len(rows) == 1
    assert rows[0][0] is not None
    assert rows[0][0] > 0  # TT value between 1000 and 1100 should be positive
    db.close()


def test_calibration_level_incremented():
    tracker, bus, db = _make_tracker()
    db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        ("Anatomy", 500.0, 1711000000.0),
    )
    db.conn.commit()

    bus.publish("session_started", {"session_id": "test-session-3"})
    bus.publish("skill_gain", {
        "skill_name": "Anatomy",
        "amount": 0.5,
        "timestamp": datetime(2026, 3, 25, 12, 0, 0),
    })

    # Should have a new calibration row with incremented level
    rows = db.conn.execute(
        "SELECT level, source FROM skill_calibrations WHERE skill_name = ? ORDER BY scanned_at DESC",
        ("Anatomy",),
    ).fetchall()
    assert len(rows) == 2  # original scan + chatlog increment
    assert abs(rows[0][0] - 500.5) < 0.001
    assert rows[0][1] == "chatlog"
    db.close()


def test_gains_ignored_after_session_stop():
    tracker, bus, db = _make_tracker()
    bus.publish("session_started", {"session_id": "test-session-5"})
    bus.publish("skill_gain", {
        "skill_name": "Anatomy",
        "amount": 0.1,
        "timestamp": datetime(2026, 3, 25, 12, 0, 0),
    })
    bus.publish("session_stopped", {"session_id": "test-session-5"})
    bus.publish("skill_gain", {
        "skill_name": "Anatomy",
        "amount": 0.2,
        "timestamp": datetime(2026, 3, 25, 12, 1, 0),
    })

    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 1  # only the gain during session
    db.close()


# ── Codex suppression tests ────────────────────────────────────────────────


def test_suppress_next_consumes_matching_gain():
    """Suppressed skill gain should be silently dropped."""
    tracker, bus, db = _make_tracker()
    bus.publish("session_started", {"session_id": "test-suppress-1"})

    tracker.suppress_next("Aim", timeout=30)
    bus.publish("skill_gain", {
        "skill_name": "Aim",
        "amount": 0.1,
        "timestamp": datetime(2026, 3, 26, 12, 0, 0),
    })

    # The gain should have been suppressed
    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 0
    # Suppression entry should be consumed
    assert "Aim" not in tracker._suppressed_claims
    db.close()


def test_suppress_does_not_affect_other_skills():
    """Only the suppressed skill should be dropped; others pass through."""
    tracker, bus, db = _make_tracker()
    bus.publish("session_started", {"session_id": "test-suppress-2"})

    tracker.suppress_next("Aim", timeout=30)
    bus.publish("skill_gain", {
        "skill_name": "Rifle",
        "amount": 0.2,
        "timestamp": datetime(2026, 3, 26, 12, 0, 0),
    })

    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 1
    assert rows[0]["skill_name"] == "Rifle"
    # Aim suppression still pending
    assert "Aim" in tracker._suppressed_claims
    db.close()


def test_suppress_expired_processes_normally():
    """Expired suppression should not block the gain."""
    import time as _t
    tracker, bus, db = _make_tracker()
    bus.publish("session_started", {"session_id": "test-suppress-3"})

    # Set suppression that already expired
    tracker._suppressed_claims["Aim"] = _t.time() - 10

    bus.publish("skill_gain", {
        "skill_name": "Aim",
        "amount": 0.1,
        "timestamp": datetime(2026, 3, 26, 12, 0, 0),
    })

    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 1  # processed normally
    assert "Aim" not in tracker._suppressed_claims
    db.close()


def test_suppress_only_consumes_once():
    """After one suppressed gain, the next gain for the same skill is recorded."""
    tracker, bus, db = _make_tracker()
    bus.publish("session_started", {"session_id": "test-suppress-4"})

    tracker.suppress_next("Aim", timeout=30)

    # First gain: suppressed
    bus.publish("skill_gain", {
        "skill_name": "Aim",
        "amount": 0.1,
        "timestamp": datetime(2026, 3, 26, 12, 0, 0),
    })
    # Second gain: should be recorded
    bus.publish("skill_gain", {
        "skill_name": "Aim",
        "amount": 0.1,
        "timestamp": datetime(2026, 3, 26, 12, 0, 1),
    })

    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 1  # only the second gain
    db.close()


def test_suppression_does_not_leak_across_sessions():
    """A suppression armed in one session must not carry into the next.

    A codex claim armed just before tracking stops would otherwise suppress
    the first matching gain of the next session (until its timeout), dropping
    a valid data point. Session start and stop both clear the cache.
    """
    tracker, bus, db = _make_tracker()
    bus.publish("session_started", {"session_id": "test-leak-1"})

    # Arm a long-lived suppression, then end the session before the gain lands.
    tracker.suppress_next("Aim", timeout=300)
    bus.publish("session_stopped", {})
    assert "Aim" not in tracker._suppressed_claims

    # New session: a genuine Aim gain must be recorded, not suppressed.
    bus.publish("session_started", {"session_id": "test-leak-2"})
    bus.publish("skill_gain", {
        "skill_name": "Aim",
        "amount": 0.1,
        "timestamp": datetime(2026, 3, 26, 12, 0, 0),
    })

    rows = db.conn.execute("SELECT * FROM skill_gains").fetchall()
    assert len(rows) == 1
    assert rows[0]["skill_name"] == "Aim"
    db.close()
