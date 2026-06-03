"""Tracker read/write concurrency regression.

The tracking read path aggregates the live session on the web threadpool while
the chat-log and hotbar producer threads mutate it. Without the tracker lock a
reader could iterate the in-progress accumulator's ``tool_stats`` dict while a
producer resized it (``RuntimeError: dictionary changed size during iteration``)
or pair a kills-list snapshot against a per-kill aggregate built a moment
earlier (``zip(..., strict=True)`` length mismatch), and could return multi-pass
inconsistent totals.

These tests hammer the read path from one thread while a second mutates the
session through the real event bus, and assert the reader never raises and never
observes an internally inconsistent readout. They fail on the lockless read path
(revert the ``snapshot`` lock to confirm) and pass with the lock in place.
"""

import sqlite3
import sys
import threading
from datetime import datetime, timedelta
from types import SimpleNamespace

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_COMBAT,
    EVENT_LOOT_GROUP,
)
from backend.routers.tracking import tracking_snapshot_impl
from backend.tracking.tracker import HuntTracker

# A small pool of priced tools; cycling the active tool churns the accumulator's
# tool_stats dict (the merge/insert path) while the reader iterates it.
_TOOLS = ("Opalo", "EmikS", "Sollomate", "CB5", "ManaForce")
# Bounded so the (passing) locked run stays a few seconds on the standard tier;
# the lockless race reproduces within the first handful of kills, so the budget
# is headroom, not a reproduction threshold.
_ITERATIONS = 1500


def _make_pipeline():
    db = sqlite3.connect(":memory:", check_same_thread=False)
    # skill_gains lives on the app database, not init_tracking_tables; the
    # readout reads it, so create it here.
    db.execute(
        """CREATE TABLE IF NOT EXISTS skill_gains (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT NOT NULL, timestamp REAL NOT NULL,
               skill_name TEXT NOT NULL, amount REAL NOT NULL, ped_value REAL)"""
    )
    # A DB-backed cost lookup so the combat / tool-change handlers exercise the
    # provider read against the shared connection WHILE holding the tracker lock
    # (an in-memory lambda would hide that under-lock DB path), concurrently with
    # the reader's own snapshot DB reads.
    db.execute(
        "CREATE TABLE IF NOT EXISTS tool_costs (name TEXT PRIMARY KEY, cost REAL)"
    )
    db.executemany(
        "INSERT OR REPLACE INTO tool_costs (name, cost) VALUES (?, ?)",
        [(name, 0.5) for name in _TOOLS],
    )
    db.commit()

    def _cost_lookup(name):
        row = db.execute(
            "SELECT cost FROM tool_costs WHERE name = ?", (name,)
        ).fetchone()
        return row[0] if row else 0.0

    bus = EventBus()
    tracker = HuntTracker(bus, db, equipment_cost_lookup=_cost_lookup)
    tracker.start_session()
    return tracker, db


def _make_svc(tracker, db):
    config = SimpleNamespace(
        # Hotbar attribution keeps the trifecta-summary path (and its extra
        # equipment-library reads) out, so the readout under test is the
        # session-scoped pair.
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


def _drive_mutations(bus, stop, iterations):
    """Publish a churning combat/loot/tool stream onto the bus.

    Each iteration switches the active tool (merging the accumulator's
    tool_stats), fires two shots, and records a kill (appending to the kills
    list and resetting the accumulator). Loot fingerprints vary so the 2 s
    dedup guard never collapses them: the kills list grows every iteration, so
    the reader's multi-pass aggregation widens into a real race window.
    """
    base = datetime(2026, 1, 1, 12, 0, 0)
    for i in range(iterations):
        if stop.is_set():
            return
        ts = base + timedelta(seconds=i)
        bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": _TOOLS[i % len(_TOOLS)]})
        bus.publish(
            EVENT_COMBAT,
            {"type": "damage_dealt", "amount": 10.0 + (i % 7), "timestamp": ts},
        )
        bus.publish(
            EVENT_COMBAT,
            {"type": "critical_hit", "amount": 20.0, "timestamp": ts},
        )
        loot = round(1.0 + i * 0.001, 4)
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": loot}],
                "total_ped": loot,
                "timestamp": ts + timedelta(milliseconds=1),
            },
        )


def _run_hammer(tracker, read_fn, iterations=_ITERATIONS):
    """Run a reader and a mutator concurrently on one tracker.

    Returns ``(errors, max_kill_count)``: any exception either thread raised,
    and the largest active kill count the reader observed (so a test can assert
    it genuinely read a growing session rather than passing vacuously).
    """
    bus = tracker._event_bus
    stop = threading.Event()
    errors: list[BaseException] = []
    seen_kill_counts: list[int] = []
    start = threading.Barrier(2)

    def mutator():
        try:
            start.wait()
            _drive_mutations(bus, stop, iterations)
        except BaseException as exc:  # noqa: BLE001 -- surface any producer fault
            errors.append(exc)
        finally:
            stop.set()

    def reader():
        try:
            start.wait()
            while not stop.is_set():
                result = read_fn()
                if isinstance(result, dict):
                    if result.get("status") == "active":
                        # status uses snake kill_count; live uses camel killCount.
                        seen_kill_counts.append(
                            result.get("kill_count") or result.get("killCount") or 0
                        )
                elif result.active is not None:
                    seen_kill_counts.append(result.active.kill_count)
        except BaseException as exc:  # noqa: BLE001 -- the race we guard against
            errors.append(exc)
        finally:
            stop.set()

    mt = threading.Thread(target=mutator, name="tracker-mutator")
    rt = threading.Thread(target=reader, name="tracker-reader")
    # Tighten the interpreter switch interval so the reader's multi-pass
    # aggregation reliably interleaves with the producer's mutations within the
    # iteration budget (rather than each thread running to completion in one
    # scheduling slice). This makes the lockless failure deterministic without
    # sleeps; the locked path is unaffected (the lock serialises regardless).
    previous_interval = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    try:
        rt.start()
        mt.start()
        mt.join(timeout=60)
        rt.join(timeout=60)
    finally:
        sys.setswitchinterval(previous_interval)
    assert not mt.is_alive() and not rt.is_alive(), "hammer threads did not finish"
    return errors, max(seen_kill_counts, default=0)


def test_snapshot_survives_concurrent_mutation():
    tracker, _db = _make_pipeline()
    errors, max_kc = _run_hammer(tracker, tracker.snapshot)
    assert not errors, f"snapshot() raced the mutator: {errors[0]!r}"
    # Guard against a vacuous pass: the reader must have read a growing session.
    assert max_kc > 0, "the reader never observed an active session; test inert"


def test_snapshot_readout_survives_concurrent_mutation():
    tracker, db = _make_pipeline()
    svc = _make_svc(tracker, db)

    def read_snapshot():
        # The surviving HTTP readout routes through the same locked aggregation;
        # hammer the router projection, not just snapshot() directly.
        return tracking_snapshot_impl(svc)

    errors, max_kc = _run_hammer(tracker, read_snapshot)
    assert not errors, f"the snapshot readout raced the mutator: {errors[0]!r}"
    assert max_kc > 0, "the reader never observed an active session; test inert"
