"""Mutation-hardening tests for HuntTracker.snapshot (cluster tracker__c11).

Scope: HuntTracker.snapshot (the owned, detached tracking readout) and its
financial / damage / multiplier / cumulative-net arithmetic, plus
HuntTracker._on_tick_flushed (the coalesced session-update emit and its
dirty-flag / timestamp-fallback branches).

Why this file exists: snapshot is exercised end-to-end only through the HTTP
read impl (test_tracking_snapshot.py drives tracking_snapshot_impl, which imports
the router), so it is absent from the Linux mutation campaign's pure-logic test
selection and every snapshot mutant scored no-test. These tests drive the real
tracker in-memory and call snapshot() DIRECTLY (no router import, so the file is
campaign-eligible), pinning each computed field to an independently-derived value
so an arithmetic / comparison mutant in the readout breaks an assertion.
"""

from __future__ import annotations

import sqlite3
from datetime import datetime, timedelta

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_COMBAT, EVENT_GLOBAL, EVENT_LOOT_GROUP
from backend.tracking.tracker import HuntTracker


def _db() -> sqlite3.Connection:
    conn = sqlite3.connect(":memory:", check_same_thread=False)
    # skill_gains lives on the app database (not init_tracking_tables); snapshot
    # reads it for the pes field, so create it here.
    conn.execute(
        """CREATE TABLE IF NOT EXISTS skill_gains (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT NOT NULL, timestamp REAL NOT NULL,
               skill_name TEXT NOT NULL, amount REAL NOT NULL, ped_value REAL)"""
    )
    return conn


def _make(**kwargs):
    db = kwargs.pop("db", None) or _db()
    bus = EventBus()
    # A per-shot cost for the "Opalo" tool only, so a no-tool kill stays at zero
    # weapon cost (the multiplier guard's excluded branch) while tool-active kills
    # carry a known cost.
    kwargs.setdefault(
        "equipment_cost_lookup", lambda name: 0.5 if name == "Opalo" else 0.0
    )
    tracker = HuntTracker(bus, db, **kwargs)
    return bus, tracker, db


def _kill(bus, base, offset_s, combats, loot):
    """Drive one kill: combat shots then a loot group that finalises it."""
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


# ---------------------------------------------------------------------------
# The rich active session: value-pin every computed financial / count field.
# ---------------------------------------------------------------------------


def _drive_rich_session():
    """Three kills with known values; returns (tracker, db, session, base).

    Kill 1: no tool -> weapon cost 0 (excluded from the multiplier set).
    Kill 2: tool active, 2 shots @ 0.5 -> weapon cost 1.0, loot 2.50.
    Kill 3: tool active, 2 shots @ 0.5 -> weapon cost 1.0, loot 5.00, one crit.
    """
    bus, tracker, db = _make()
    session = tracker.start_session()
    base = datetime(2026, 1, 2, 3, 4, 5)

    _kill(bus, base, 0, [("damage_dealt", 10.0)], 1.50)  # no tool yet
    tracker._active_hotbar_tool_name = "Opalo"
    _kill(bus, base, 5, [("damage_dealt", 10.0), ("damage_dealt", 12.0)], 2.50)
    _kill(bus, base, 10, [("damage_dealt", 20.0), ("critical_hit", 30.0)], 5.00)

    tracker._session_heal_cost = 1.25  # session-level heal cost

    db.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value)"
        " VALUES (?, ?, 'Laser Weaponry Technology', 0.5, 1.50)",
        (session.id, base.timestamp()),
    )
    db.execute(
        "INSERT INTO notable_events "
        "(session_id, event_type, mob_or_item, value_ped, timestamp)"
        " VALUES (?, 'global_kill', 'Atrox', 55.0, ?)",
        (session.id, base.timestamp()),
    )
    db.commit()
    return tracker, db, session, base


def test_snapshot_financial_totals_are_pinned():
    """Pin the headline financial aggregates so any +/-/*// mutant in the cost,
    returns, net, and return-rate arithmetic breaks an assertion."""
    tracker, _db_, _session, _base = _drive_rich_session()
    av = tracker.snapshot().active
    assert av is not None
    assert av.kill_count == 3
    # weapon_cost = 0 (kill1) + 1.0 (kill2) + 1.0 (kill3) = 2.0
    assert av.weapon_cost == pytest.approx(2.0)
    # cost = weapon 2.0 + heal 1.25 + enhancer 0.0
    assert av.cost == pytest.approx(3.25)
    # returns = 1.50 + 2.50 + 5.00
    assert av.returns == pytest.approx(9.0)
    # net = returns - cost
    assert av.net == pytest.approx(5.75)
    # return_rate = returns / cost (rounded to 4dp)
    assert av.return_rate == pytest.approx(9.0 / 3.25, abs=1e-3)
    # pes = SUM(skill_gains.ped_value) for the session
    assert av.pes == pytest.approx(1.5)
    assert av.latest_kill_loot == pytest.approx(5.0)


def test_snapshot_damage_and_shot_counts_are_pinned():
    """Pin the damage / shot / crit aggregates and the max-single-hit reducer."""
    tracker, _db_, _session, _base = _drive_rich_session()
    av = tracker.snapshot().active
    assert av is not None
    # damage = 10 (k1) + 22 (k2) + 50 (k3)
    assert av.damage_dealt_total == pytest.approx(82.0)
    assert av.weapon_damage_dealt == pytest.approx(82.0)
    # shots = 1 + 2 + 2
    assert av.shots_fired_total == 5
    # one critical hit, on kill 3
    assert av.critical_hits_total == 1
    # max single-kill damage is kill 3's 20 + 30
    assert av.max_damage == pytest.approx(50.0)


def test_snapshot_multipliers_exclude_zero_cost_kills():
    """The multiplier set uses loot / weapon-cost and excludes the zero-cost
    kill; pin avg / max / last / history so the division, the >0 guard, and the
    history slice all carry assertions."""
    tracker, _db_, _session, _base = _drive_rich_session()
    av = tracker.snapshot().active
    assert av is not None
    # kill1 has zero weapon cost -> excluded; kill2 = 2.50/1.0, kill3 = 5.00/1.0.
    assert av.multiplier_history == pytest.approx((2.5, 5.0))
    assert av.multiplier_avg == pytest.approx(3.75)
    assert av.multiplier_max == pytest.approx(5.0)
    assert av.multiplier_last == pytest.approx(5.0)


def test_snapshot_cumulative_net_curve_reconciles_with_net():
    """The per-kill cumulative-net curve folds a pro-rata heal share; pin each
    point and assert the final point reconciles with net (returns - cost)."""
    tracker, _db_, _session, _base = _drive_rich_session()
    av = tracker.snapshot().active
    assert av is not None
    # per-kill weapon cost [0, 1.0, 1.0]; total 2.0; heal 1.25 split pro-rata.
    #   k1: 1.50 - 0 - 0          = 1.50  -> 1.50
    #   k2: 2.50 - 1.0 - 0.625    = 0.875 -> 2.375
    #   k3: 5.00 - 1.0 - 0.625    = 3.375 -> 5.75
    assert av.cumulative_net_history == pytest.approx((1.5, 2.375, 5.75), abs=0.02)
    # The curve's final point reconciles with the displayed net.
    assert av.cumulative_net_history[-1] == pytest.approx(av.net)


def test_snapshot_passthrough_and_feed_fields():
    """Pin the flat pass-throughs and the session-scoped feed read so a mis-wired
    field (current tool, mob mode, the notable-event feed) is caught."""
    tracker, _db_, _session, base = _drive_rich_session()
    readout = tracker.snapshot()
    assert readout.current_tool == "Opalo"
    av = readout.active
    assert av is not None
    assert av.mob_entry_mode == "mob"
    assert av.current_mob is None
    assert av.mob_source is None
    # The seeded notable event surfaces in the feed verbatim.
    assert av.notable_event_rows == (("global_kill", "Atrox", 55.0, base.timestamp()),)


# ---------------------------------------------------------------------------
# Idle: no session -> active is None, current_tool still passes through.
# ---------------------------------------------------------------------------


def test_snapshot_idle_returns_no_active_view():
    """With no session the readout's active view is None and the early-return
    branch is taken; current_tool is still surfaced."""
    _bus, tracker, _db_ = _make()
    tracker._active_hotbar_tool_name = "Korss H400"
    readout = tracker.snapshot()
    assert readout.active is None
    assert readout.current_tool == "Korss H400"


# ---------------------------------------------------------------------------
# Mid-combat: an active accumulator folds into the live weapon cost / damage,
# and the no-kills defaults hold (None multipliers, empty curve).
# ---------------------------------------------------------------------------


def test_snapshot_folds_active_accumulator_and_holds_no_kill_defaults():
    """Snapshot taken mid-combat (before the loot finalises a kill): the active
    accumulator's weapon cost and damage fold in, while the no-kills default
    paths hold (None multipliers, empty curve, None latest loot)."""
    bus, tracker, _db_ = _make()
    tracker.start_session()
    tracker._active_hotbar_tool_name = "Opalo"
    now = datetime(2026, 1, 2, 3, 4, 5)
    # Two shots, no loot yet -> the accumulator is live, zero kills.
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 20.0, "timestamp": now})
    av = tracker.snapshot().active
    assert av is not None
    assert av.kill_count == 0
    # weapon_cost comes entirely from the accumulator: 2 shots @ 0.5.
    assert av.weapon_cost == pytest.approx(1.0)
    # live weapon damage is the accumulator's 10 + 20.
    assert av.weapon_damage_dealt == pytest.approx(30.0)
    assert av.shots_fired_total == 0  # no finalised kill yet
    # No-kills defaults.
    assert av.multiplier_avg is None
    assert av.multiplier_max is None
    assert av.multiplier_last is None
    assert av.multiplier_history == ()
    assert av.cumulative_net_history == ()
    assert av.latest_kill_loot is None
    assert av.max_damage == pytest.approx(0.0)


# ---------------------------------------------------------------------------
# Globals / HoFs counts come from kills tagged by a matching player global.
# ---------------------------------------------------------------------------


def test_snapshot_counts_globals_and_hofs():
    """A HoF global matching the player tags its kill is_global + is_hof, so both
    counts read 1; a non-tagged kill leaves them at 0."""
    bus, tracker, _db_ = _make(player_name="Hunter")
    tracker.start_session()
    now = datetime(2026, 1, 2, 3, 4, 5)
    # A kill that earns a matching HoF global.
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 100.0}],
            "total_ped": 100.0,
            "timestamp": now + timedelta(milliseconds=500),
        },
    )
    bus.publish(
        EVENT_GLOBAL,
        {
            "type": "hof_kill",
            "player": "hunter",
            "creature": "Atrox",
            "value": 100.0,
            "timestamp": now,
        },
    )
    av = tracker.snapshot().active
    assert av is not None
    assert av.globals_count == 1
    assert av.hofs_count == 1


# ---------------------------------------------------------------------------
# elapsed: wall-clock seconds since session start, pinned via a known start.
# ---------------------------------------------------------------------------


def test_snapshot_elapsed_is_seconds_since_session_start():
    """elapsed is int(now - start). Backdating the session start by a known
    delta pins it, so the subtraction and the int() cast carry an assertion."""
    _bus, tracker, _db_ = _make()
    session = tracker.start_session()
    # Backdate the start by 100s so elapsed is a known, non-trivial positive int.
    session.start_time = session.start_time - timedelta(seconds=100)
    av = tracker.snapshot().active
    assert av is not None
    assert isinstance(av.elapsed, int)
    assert av.elapsed == pytest.approx(100, abs=3)


# ---------------------------------------------------------------------------
# _on_tick_flushed: the coalesced session-update emit. A recording stub on
# _emit_session_event isolates the method's guards, dirty-flag reset, timestamp
# resolution, and the exact emitted arguments.
# ---------------------------------------------------------------------------


def _spy_emit(tracker):
    calls: list[tuple] = []
    tracker._emit_session_event = lambda *a: calls.append(a)
    return calls


def test_tick_flush_emits_update_with_event_timestamp_and_resets_dirty():
    """A dirty active session emits ("updated", "active", ts, session_id) with the
    tick's own datetime timestamp, and clears the dirty flag so a following no-op
    tick does not re-emit."""
    _bus, tracker, _db_ = _make()
    session = tracker.start_session()
    calls = _spy_emit(tracker)
    tracker._session_dirty = True
    ts = datetime(2026, 1, 2, 3, 4, 5)
    tracker._on_tick_flushed({"timestamp": ts})
    assert calls == [("updated", "active", ts.timestamp(), session.id)]
    assert tracker._session_dirty is False


def test_tick_flush_falls_back_to_clock_when_timestamp_absent():
    """With no timestamp on the tick, occurred_at falls back to the injected
    clock's now() so the emitted instant is still real."""
    import time

    _bus, tracker, _db_ = _make()
    tracker.start_session()
    calls = _spy_emit(tracker)
    tracker._session_dirty = True
    tracker._on_tick_flushed({})  # no "timestamp" key -> clock fallback
    assert len(calls) == 1
    assert calls[0][:2] == ("updated", "active")
    assert calls[0][2] == pytest.approx(time.time(), abs=10)


def test_tick_flush_coerces_a_non_datetime_timestamp_to_float():
    """A non-datetime, non-None timestamp is coerced via float()."""
    _bus, tracker, _db_ = _make()
    tracker.start_session()
    calls = _spy_emit(tracker)
    tracker._session_dirty = True
    tracker._on_tick_flushed({"timestamp": 123.5})
    assert calls[0][2] == pytest.approx(123.5)


def test_tick_flush_does_not_emit_when_session_not_dirty():
    """A clean tick (nothing changed the live readout) emits nothing, so idle
    chat traffic does not wake listeners."""
    _bus, tracker, _db_ = _make()
    tracker.start_session()
    calls = _spy_emit(tracker)
    tracker._session_dirty = False
    tracker._on_tick_flushed({"timestamp": datetime(2026, 1, 2, 3, 4, 5)})
    assert calls == []


def test_tick_flush_does_not_emit_when_no_session():
    """No active session -> no emit even if the dirty flag is somehow set."""
    _bus, tracker, _db_ = _make()
    calls = _spy_emit(tracker)
    tracker._session_dirty = True
    tracker._on_tick_flushed({"timestamp": datetime(2026, 1, 2, 3, 4, 5)})
    assert calls == []
