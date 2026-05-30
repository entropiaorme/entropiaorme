"""Owner-side tracking snapshot.

Two properties:

- The consolidated readout reproduces the union of the legacy status, live, and
  recent-events shapes (the A/B equivalence the consolidation rests on), with
  the documented reshape (camelCase id/count duplicates dropped, the activity
  feed taken from the identified recent-events projection, warnings split into a
  sibling array, and the feed cleared on idle).
- ``HuntTracker.snapshot`` assembles the active readout in a single tight read
  sequence: two session-scoped statements, no per-field query fan-out.
"""

import sqlite3
from datetime import datetime
from types import SimpleNamespace

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_COMBAT, EVENT_LOOT_GROUP
from backend.routers.tracking import (
    recent_events_impl,
    tracking_live_impl,
    tracking_snapshot_impl,
    tracking_status_impl,
)
from backend.tracking.tracker import HuntTracker


def _make_svc(tracker, db):
    """A services-shaped stub exposing the surface the read impls consume.

    Hotbar attribution mode keeps the trifecta-summary path (and its extra
    equipment-library reads) out, so the snapshot's read budget is the
    session-scoped pair under test.
    """
    config = SimpleNamespace(
        hotbar_hooks_enabled=True,
        mob_tracking_mode="mob",
        mob_tracking_tag="",
        repair_ocr_enabled=False,
        end_of_session_armour_reminder_enabled=False,
        manual_mob_species="",
        manual_mob_maturity="",
    )
    return SimpleNamespace(
        tracker=tracker,
        app_db=SimpleNamespace(conn=db),
        config_service=SimpleNamespace(get=lambda: config),
        hotbar_listener=SimpleNamespace(is_running=False),
    )


@pytest.fixture
def active_pipeline():
    db = sqlite3.connect(":memory:", check_same_thread=False)
    # skill_gains lives on the app database, not init_tracking_tables; the
    # snapshot (and the legacy readouts) read it, so create it here.
    db.execute(
        """CREATE TABLE IF NOT EXISTS skill_gains (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT NOT NULL, timestamp REAL NOT NULL,
               skill_name TEXT NOT NULL, amount REAL NOT NULL, ped_value REAL)"""
    )
    bus = EventBus()
    tracker = HuntTracker(bus, db)
    session = tracker.start_session()

    now = datetime.now(tz=None)
    # Two kills with combat + loot so the readout has non-trivial aggregates.
    for dmg, loot in ((10.0, 1.50), (20.0, 2.50)):
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": dmg, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": loot}],
                "total_ped": loot,
                "timestamp": now,
            },
        )

    # A session-scoped skill gain (drives pes) and a notable event (the feed).
    db.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value)"
        " VALUES (?, ?, ?, ?, ?)",
        (session.id, now.timestamp(), "Laser Weaponry Technology", 0.5, 0.05),
    )
    db.execute(
        "INSERT INTO notable_events "
        "(session_id, event_type, mob_or_item, value_ped, timestamp)"
        " VALUES (?, ?, ?, ?, ?)",
        (session.id, "global_kill", "Atrox Old", 55.0, now.timestamp()),
    )
    db.commit()
    # A tracker warning so the warnings sibling is exercised.
    tracker._session_warnings.append("Heal tool not equipped")

    return SimpleNamespace(
        bus=bus, tracker=tracker, db=db, svc=_make_svc(tracker, db), session=session
    )


def test_snapshot_active_reproduces_union_of_legacy_readouts(active_pipeline):
    svc = active_pipeline.svc
    status = dict(tracking_status_impl(svc))
    live = dict(tracking_live_impl(svc))
    recent = recent_events_impl(svc)

    # The snapshot is the documented reshape of the three legacy readouts:
    # the status superset plus the live-only extras, the camelCase session-id /
    # kill-count duplicates and the bare kills count dropped, the activity feed
    # taken from the identified recent-events projection, and tracker warnings
    # split into a sibling array.
    expected = dict(status)
    for key in ("elapsed", "net", "currentTool", "trifectaAttribution"):
        expected[key] = live[key]
    expected["recentEvents"] = recent
    expected["warnings"] = [
        {"type": "warning", "description": e["description"], "value": e["value"]}
        for e in live["recentEvents"]
        if e.get("type") == "warning"
    ]

    snap = tracking_snapshot_impl(svc)

    # elapsed is wall-clock; assert its shape, compare everything else exactly.
    assert isinstance(snap["elapsed"], int) and snap["elapsed"] >= 0
    expected.pop("elapsed", None)
    assert {k: v for k, v in snap.items() if k != "elapsed"} == expected

    # The dropped duplicates must not reappear on the wire.
    assert "sessionId" not in snap
    assert "killCount" not in snap
    assert "kills" not in snap
    # The notable event landed in the feed and the warning in its sibling.
    assert [e["eventType"] for e in snap["recentEvents"]] == ["global_kill"]
    assert snap["warnings"] == [
        {"type": "warning", "description": "Heal tool not equipped", "value": 0}
    ]


def test_snapshot_idle_clears_the_feed_and_unions_the_config_envelope(active_pipeline):
    svc = active_pipeline.svc
    active_pipeline.tracker.stop_session()

    status = dict(tracking_status_impl(svc))
    live = dict(tracking_live_impl(svc))

    # Idle union: the status idle shape plus the live-only idle fields, with the
    # activity feed cleared (the dashboard's chosen idle behaviour).
    expected = dict(status)
    for key, value in live.items():
        expected.setdefault(key, value)
    expected["recentEvents"] = []

    snap = tracking_snapshot_impl(svc)
    assert snap == expected
    assert snap["status"] == "idle"
    assert snap["recentEvents"] == []


def test_snapshot_issues_two_session_scoped_reads(active_pipeline):
    db = active_pipeline.db
    statements: list[str] = []
    db.set_trace_callback(statements.append)
    try:
        active_pipeline.tracker.snapshot()
    finally:
        db.set_trace_callback(None)
    selects = [s for s in statements if s.lstrip().upper().startswith("SELECT")]
    assert len(selects) == 2, selects
