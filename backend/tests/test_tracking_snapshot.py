"""Owner-side tracking snapshot.

Two properties:

- The consolidated readout carries the full tracking union directly: the status
  superset, the live-only extras (elapsed / net / currentTool /
  trifectaAttribution), the identified recent-events activity feed, and the
  warnings sibling, with the camelCase id/count duplicates dropped and the feed
  cleared on idle. (The legacy status / live / recent-events readouts it
  consolidated have since been removed; this asserts the surviving snapshot shape
  directly rather than against those readouts.)
- ``HuntTracker.snapshot`` assembles the active readout in a single tight read
  sequence: two session-scoped statements, no per-field query fan-out.
"""

import sqlite3
from datetime import datetime, timedelta
from types import SimpleNamespace

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_COMBAT, EVENT_LOOT_GROUP
from backend.routers.tracking import tracking_snapshot_impl
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
    # A per-shot cost for the active tool only, so kills fired with it carry
    # non-zero weapon cost while the no-tool kill stays at zero. This exercises
    # both the snapshot's non-zero financial paths (weapon cost, the multiplier
    # division and its capped history, the pro-rata heal share) AND the
    # zero-cost guard branch (a kill excluded from the multiplier set).
    tracker = HuntTracker(
        bus, db, equipment_cost_lookup=lambda name: 0.5 if name == "Opalo" else 0.0
    )
    session = tracker.start_session()

    base = datetime.now(tz=None)

    def _kill(offset_s, combats, loot):
        ts = base + timedelta(seconds=offset_s)
        for kind, amount in combats:
            bus.publish(EVENT_COMBAT, {"type": kind, "amount": amount, "timestamp": ts})
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": loot}],
                "total_ped": loot,
                "timestamp": ts + timedelta(milliseconds=500),
            },
        )

    # Kill 1 resolves with no active tool, so its weapon cost is zero (the
    # multiplier guard's None branch). Kills 2 and 3 fire with a tool active, so
    # their shots carry the per-shot cost; the critical hit and the loot spread
    # give a non-trivial multiplier range and a populated history.
    _kill(0, [("damage_dealt", 10.0)], 1.50)
    tracker._active_hotbar_tool_name = "Opalo"
    _kill(5, [("damage_dealt", 10.0), ("damage_dealt", 12.0)], 2.50)
    _kill(10, [("damage_dealt", 20.0), ("critical_hit", 30.0)], 5.00)
    # A session-level heal cost so the pro-rata heal-share folded into the
    # cumulative-net history is non-zero.
    tracker._session_heal_cost = 1.25

    # A session-scoped skill gain (drives pes) and a notable event (the feed).
    db.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value)"
        " VALUES (?, ?, ?, ?, ?)",
        (session.id, base.timestamp(), "Laser Weaponry Technology", 0.5, 0.05),
    )
    db.execute(
        "INSERT INTO notable_events "
        "(session_id, event_type, mob_or_item, value_ped, timestamp)"
        " VALUES (?, ?, ?, ?, ?)",
        (session.id, "global_kill", "Atrox Old", 55.0, base.timestamp()),
    )
    db.commit()
    # A tracker warning so the warnings sibling is exercised.
    tracker._session_warnings.append("Heal tool not equipped")

    return SimpleNamespace(
        bus=bus, tracker=tracker, db=db, svc=_make_svc(tracker, db), session=session
    )


def test_snapshot_active_carries_the_full_tracking_union(active_pipeline):
    svc = active_pipeline.svc
    snap = tracking_snapshot_impl(svc)

    assert snap["status"] == "active"

    # The active snapshot carries the full union directly: the status superset,
    # the live-only extras, and the consolidated activity feed + warnings sibling.
    # The legacy readouts it consolidated are gone, so coverage is asserted as
    # field presence on the surviving shape rather than against those readouts.
    status_fields = {
        "session_id",
        "started_at",
        "kill_count",
        "cost",
        "returns",
        "pes",
        "returnRate",
        "damageDealtTotal",
        "weaponDamageDealt",
        "weaponCost",
        "shotsFiredTotal",
        "criticalHitsTotal",
        "maxDamage",
        "globalsCount",
        "hofsCount",
        "latestKillLoot",
        "multiplierLast",
        "multiplierAvg",
        "multiplierMax",
        "multiplierHistory",
        "cumulativeNetHistory",
        "hotbarListenerActive",
        "weaponAttribution",
        "repairOcrEnabled",
        "endOfSessionArmourReminderEnabled",
        "mobEntryMode",
        "currentMob",
        "mobSource",
    }
    live_only_fields = {"elapsed", "net", "currentTool", "trifectaAttribution"}
    consolidated_fields = {"recentEvents", "warnings"}
    missing = (status_fields | live_only_fields | consolidated_fields) - snap.keys()
    assert not missing, f"snapshot dropped union fields: {sorted(missing)}"

    # elapsed is wall-clock; assert its shape.
    assert isinstance(snap["elapsed"], int) and snap["elapsed"] >= 0

    # The dropped camelCase duplicates must not reappear on the wire.
    assert "sessionId" not in snap
    assert "killCount" not in snap
    assert "kills" not in snap
    # The notable event landed in the feed and the warning in its sibling.
    assert [e["eventType"] for e in snap["recentEvents"]] == ["global_kill"]
    assert snap["warnings"] == [
        {"type": "warning", "description": "Heal tool not equipped", "value": 0}
    ]

    # Guard that the fixture genuinely drove the non-zero-cost financial paths
    # the snapshot duplicates from the legacy readouts: weapon cost, the
    # multiplier division and its capped history, and the cumulative-net curve
    # with a non-zero pro-rata heal share. Without these a regression to a
    # zero-cost fixture would leave those formulas unexercised while the
    # equivalence assertion still passed trivially.
    assert snap["kill_count"] == 3
    assert snap["weaponCost"] > 0
    assert len(snap["multiplierHistory"]) == 2  # the two tool-active kills
    assert snap["multiplierMax"] is not None and snap["multiplierMax"] > 0
    assert snap["criticalHitsTotal"] >= 1
    assert len(snap["cumulativeNetHistory"]) == 3  # every kill, heal-share folded
    # Value-pin the active-only flat pass-throughs the fixture deterministically
    # drives, so a mis-wired field on the snapshot impl is caught rather than only
    # a missing key: the seeded skill gain drives pes, the three kills drive the
    # damage totals, and the critical hit sets the max single-hit damage.
    assert snap["pes"] > 0
    assert snap["damageDealtTotal"] > 0
    assert snap["weaponDamageDealt"] > 0  # kills 2 and 3 fired with a tool active
    assert snap["maxDamage"] > 0  # the critical hit


def test_snapshot_idle_clears_the_feed_and_unions_the_config_envelope(active_pipeline):
    svc = active_pipeline.svc
    active_pipeline.tracker.stop_session()

    snap = tracking_snapshot_impl(svc)
    assert snap["status"] == "idle"
    # The activity feed clears on idle (the dashboard's chosen idle behaviour).
    assert snap["recentEvents"] == []

    # The idle shape is the config + runtime envelope only, carrying the same
    # envelope fields the active shape does, with no session-derived numbers.
    idle_fields = {
        "status",
        "hotbarListenerActive",
        "weaponAttribution",
        "repairOcrEnabled",
        "endOfSessionArmourReminderEnabled",
        "currentTool",
        "trifectaAttribution",
        "mobEntryMode",
        "currentMob",
        "mobSource",
        "recentEvents",
    }
    missing = idle_fields - snap.keys()
    assert not missing, f"idle snapshot dropped envelope fields: {sorted(missing)}"
    # No active-only session numbers leak into the idle shape.
    for active_only in ("session_id", "started_at", "kill_count", "elapsed", "net"):
        assert active_only not in snap


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
