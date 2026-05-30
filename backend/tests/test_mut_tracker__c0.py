"""Mutation-hardening tests for the HuntTracker construction / recovery /
weapon-state-reset surface (cluster tracker__c0).

Scope: HuntTracker.__init__, _recover_orphaned_sessions,
_reset_weapon_runtime_state, _active_weapon_state.

These tests drive the real backend.tracking.tracker against an in-memory
SQLite DB and the in-process event bus. Each test asserts the exact behaviour
a surviving mutant would break, observed through public APIs (properties,
start/stop session, kill stamping, DB rows) or, where the only output is a log
line / a raised exception, through caplog / a direct handler call.
"""

from __future__ import annotations

import logging
import re
import sqlite3
import uuid
from datetime import datetime, timedelta
from time import monotonic as _monotonic

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_GLOBAL,
    EVENT_LOOT_GROUP,
)
from backend.tracking.schema import init_tracking_tables
from backend.tracking.tracker import HuntTracker, _DamageEnhancerState

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _db() -> sqlite3.Connection:
    conn = sqlite3.connect(":memory:", check_same_thread=False)
    return conn


def _make(**kwargs):
    """Construct a HuntTracker on a fresh in-memory DB."""
    db = kwargs.pop("db", None) or _db()
    bus = EventBus()
    tracker = HuntTracker(bus, db, **kwargs)
    return bus, tracker, db


def _add_skill_gains_table(db: sqlite3.Connection) -> None:
    db.executescript(
        """
        CREATE TABLE IF NOT EXISTS skill_gains (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id    TEXT NOT NULL,
            timestamp     REAL NOT NULL,
            skill_name    TEXT NOT NULL,
            amount        REAL NOT NULL,
            ped_value     REAL
        );
        """
    )
    db.commit()


# ---------------------------------------------------------------------------
# __init__: session-state defaults observable via public properties
# ---------------------------------------------------------------------------


def test_fresh_tracker_is_not_tracking_and_has_no_accumulator():
    """_session and _accumulator default to None (not "" or other truthy).

    Kills __init__ mutants that seed self._session / self._accumulator to "".
    """
    _bus, tracker, _db_ = _make()
    # _session is None -> is_tracking is False (mutant "" -> '' is not None -> True)
    assert tracker.is_tracking is False
    assert tracker.session is None
    # _accumulator is None (mutant "" -> current_accumulator returns "")
    assert tracker.current_accumulator is None


# ---------------------------------------------------------------------------
# __init__: default player_name "" -> globals are never filtered to a player
# ---------------------------------------------------------------------------


def test_default_player_name_rejects_all_globals():
    """player_name defaults to "" so `not self._player_name` short-circuits and
    every global is rejected - no kill is tagged global.

    Behavioural coverage of the default player-name filter. (The signature
    default literal itself, __init__ mutmut_1 "" -> "XXXX", is equivalent: the
    mutmut trampoline wrapper forwards its own unmutated "" default explicitly,
    so the inner default is never used.)
    """
    bus, tracker, db = _make()  # default player_name
    tracker.start_session()

    now = datetime.now(tz=None)
    # Create a kill via loot.
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 5.0, "timestamp": now})
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "items": [{"item_name": "Animal Oil", "value_ped": 1.0, "quantity": 1}],
            "total_ped": 1.0,
            "timestamp": now,
        },
    )
    # A HoF global whose player matches the *mutant* default "XXXX".
    bus.publish(
        EVENT_GLOBAL,
        {
            "type": "hof_kill",
            "player": "XXXX",
            "creature": "Atrox",
            "value": 100.0,
            "timestamp": now,
        },
    )
    # Original: player_name == "" -> global rejected -> kill not global.
    row = db.execute("SELECT is_global FROM kills").fetchone()
    assert row is not None
    assert row[0] == 0


def test_explicit_player_name_is_stored_stripped():
    """An explicit player_name is honoured (and stripped); a matching global
    tags the kill. Reinforces the player-name filter wiring."""
    bus, tracker, db = _make(player_name="  Hunter  ")
    tracker.start_session()
    now = datetime.now(tz=None)
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 5.0, "timestamp": now})
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "items": [{"item_name": "Animal Oil", "value_ped": 1.0, "quantity": 1}],
            "total_ped": 1.0,
            "timestamp": now,
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
    row = db.execute("SELECT is_global, is_hof FROM kills").fetchone()
    assert row == (1, 1)


# ---------------------------------------------------------------------------
# __init__: loot-filter blacklist param must be retained for _refresh_loot_filter
# ---------------------------------------------------------------------------


def test_constructor_loot_blacklist_is_retained_and_filters_loot():
    """A blacklist passed at construction (no provider) must survive into
    _loot_filter_blacklist so _refresh_loot_filter applies it.

    Kills __init__ mutant `_loot_filter_blacklist = list(... or []) -> None`:
    with None the custom blacklist is dropped and the item is NOT filtered.
    """
    bus, tracker, db = _make(loot_filter_blacklist=["Forbidden Ore"])
    tracker.start_session()
    now = datetime.now(tz=None)
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 5.0, "timestamp": now})
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "items": [
                {"item_name": "Forbidden Ore", "value_ped": 9.0, "quantity": 1},
                {"item_name": "Animal Oil", "value_ped": 1.0, "quantity": 1},
            ],
            "total_ped": 10.0,
            "timestamp": now,
        },
    )
    # Forbidden Ore is blacklisted -> only Animal Oil persisted.
    names = [
        r[0] for r in db.execute("SELECT item_name FROM kill_loot_items").fetchall()
    ]
    assert "Forbidden Ore" not in names
    assert names == ["Animal Oil"]


# ---------------------------------------------------------------------------
# __init__: default mob-tracking-mode provider -> persisted to DB session row
# ---------------------------------------------------------------------------


def test_default_mob_tracking_mode_persisted_as_mob():
    """The default mob_tracking_mode provider returns exactly "mob", which is
    persisted into tracking_sessions.mob_tracking_mode.

    Kills the "mob" -> "XXmobXX" / "MOB" default-provider literal mutants.
    """
    _bus, tracker, db = _make()
    session = tracker.start_session()
    row = db.execute(
        "SELECT mob_tracking_mode FROM tracking_sessions WHERE id = ?",
        (session.id,),
    ).fetchone()
    assert row[0] == "mob"


def test_default_tag_provider_is_empty_so_tag_mode_sets_no_mob():
    """In tag mode the default tag provider yields "" (falsy) so no session tag
    is applied; a kill stamps mob_name "Unknown".

    Kills the default tag-provider literal `"" -> "XXXX"`: with "XXXX" the tag
    would be applied and the kill would stamp mob_name "XXXX".
    """
    bus, tracker, db = _make(mob_tracking_mode_provider=lambda: "tag")
    tracker.start_session()
    now = datetime.now(tz=None)
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 5.0, "timestamp": now})
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "items": [{"item_name": "Animal Oil", "value_ped": 1.0, "quantity": 1}],
            "total_ped": 1.0,
            "timestamp": now,
        },
    )
    row = db.execute("SELECT mob_name FROM kills").fetchone()
    assert row[0] == "Unknown"


# ---------------------------------------------------------------------------
# __init__: pre-session confirmed/current mob names default to "" (falsy)
# ---------------------------------------------------------------------------


def test_release_current_mob_is_none_on_fresh_tracker():
    """confirmed/current mob names default to "" so release_current_mob() returns
    None on a fresh tracker.

    Kills the __init__ mutants seeding _confirmed_mob_name / _current_mob_name to
    a non-empty literal ("XXXX"), which would make release return that literal.
    """
    _bus, tracker, _db_ = _make()
    assert tracker.release_current_mob() is None


# ---------------------------------------------------------------------------
# __init__: heal defaults observable via heal handling
# ---------------------------------------------------------------------------


def test_self_heal_without_tool_emits_warning():
    """_active_heal_tool_name defaults to None, so a self-heal with no equipped
    heal tool records the "no heal tool" warning.

    Kills the __init__ mutant `_active_heal_tool_name = None -> ""`: with "" the
    `is None` guard is False and no warning is recorded.
    """
    bus, tracker, _db_ = _make()
    tracker.start_session()
    bus.publish(
        EVENT_COMBAT,
        {"type": "self_heal", "amount": 10.0, "timestamp": datetime.now(tz=None)},
    )
    assert any("heal tool" in w.lower() for w in tracker._session_warnings)


def test_self_heal_cost_zero_by_default_does_not_raise_or_charge():
    """_heal_cost_per_use_ped defaults to 0.0 (a float, not None) so the
    `> 0` guard is safe and no heal cost is charged without an equipped tool.

    Kills `_heal_cost_per_use_ped = 0.0 -> None` (None > 0 raises TypeError) and
    `0.0 -> 1.0` (would charge 1.0 PED per heal, persisted to heal_cost).
    """
    bus, tracker, db = _make()
    session = tracker.start_session()
    # Direct handler call so a None default would surface its TypeError here.
    tracker._on_combat(
        {"type": "self_heal", "amount": 10.0, "timestamp": datetime.now(tz=None)}
    )
    tracker.stop_session()
    row = db.execute(
        "SELECT heal_cost FROM tracking_sessions WHERE id = ?", (session.id,)
    ).fetchone()
    assert row[0] == 0.0


def test_default_heal_reload_window_is_two_point_five_seconds():
    """The default heal-reload dedup window is exactly 2.5s. Two heals 3s apart
    are therefore two distinct activations (both charged).

    Kills `_heal_reload_seconds = 2.5 -> 3.5`: a 3.5s window would treat the
    second heal as a duplicate and charge only once.
    """
    bus, tracker, db = _make()
    session = tracker.start_session()
    # Establish a per-use heal cost without equipping a tool (which would reset
    # the reload window we are testing).
    tracker._heal_cost_per_use_ped = 0.5
    t0 = datetime.now(tz=None)
    tracker._on_combat({"type": "self_heal", "amount": 10.0, "timestamp": t0})
    tracker._on_combat(
        {"type": "self_heal", "amount": 10.0, "timestamp": t0 + timedelta(seconds=3)}
    )
    tracker.stop_session()
    heal_cost = db.execute(
        "SELECT heal_cost FROM tracking_sessions WHERE id = ?", (session.id,)
    ).fetchone()[0]
    # 2.5s window: 3s gap -> two activations -> 2 x 0.5 PED charged.
    assert heal_cost == pytest.approx(1.0)


def test_default_heal_reload_window_dedups_via_none_safe_comparison():
    """_heal_reload_seconds defaults to a real float (2.5), so the second
    self-heal's dedup comparison `(dt) >= self._heal_reload_seconds` is safe.

    Kills `_heal_reload_seconds = 2.5 -> None`: the second heal's comparison
    against None raises TypeError. Calling the handler directly surfaces it.
    """
    bus, tracker, _db_ = _make()
    tracker.start_session()
    t0 = datetime.now(tz=None)
    tracker._on_combat({"type": "self_heal", "amount": 10.0, "timestamp": t0})
    # Second heal within the window exercises the `>= reload_seconds` compare.
    tracker._on_combat(
        {"type": "self_heal", "amount": 10.0, "timestamp": t0 + timedelta(seconds=1)}
    )


# ---------------------------------------------------------------------------
# __init__: perf counters must be real numbers, not None (DEBUG-gated path)
# ---------------------------------------------------------------------------


def test_combat_shot_with_debug_logging_does_not_raise(caplog):
    """The perf-accounting path (DEBUG-gated) starts from numeric counters
    (_perf_window_started float, _perf_shot_count/_seconds 0). A shot must not
    raise.

    Kills `_perf_window_started = None` (now - None), `_perf_shot_count = None`
    (None += 1), `_perf_shot_seconds = None` (None += float),
    `_perf_unknown_tool_shots = None` (None += 1 on an unknown-tool shot).
    Calling the handler directly with DEBUG on surfaces any TypeError.
    """
    bus, tracker, _db_ = _make()
    tracker.start_session()
    logger = logging.getLogger("backend.tracking.tracker")
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        assert logger.isEnabledFor(logging.DEBUG)
        # Unknown-tool offensive shot drives _perf_shot_count, _perf_shot_seconds
        # and _perf_unknown_tool_shots (no hotbar tool -> unknown_tool True).
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 7.0, "timestamp": datetime.now(tz=None)}
        )
    # The shot was still accumulated (perf path runs after accumulation).
    assert tracker.current_accumulator.shots_fired == 1


def test_trifecta_inference_miss_perf_counter_is_numeric(caplog):
    """_perf_inference_misses defaults to 0; a trifecta-mode damage shot that
    matches no weapon profile is an inference miss and increments it under DEBUG.

    Kills `_perf_inference_misses = None` (None += 1 on the miss).
    """
    bus, tracker, _db_ = _make(
        weapon_attribution_trifecta_provider=lambda: True,
        # default trifecta_resolver returns None -> no weapon profiles loaded,
        # so every damage shot fails attribution (an inference miss).
    )
    tracker.start_session()
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 7.0, "timestamp": datetime.now(tz=None)}
        )
    assert tracker.current_accumulator.shots_fired == 1


def test_combat_cost_lookup_perf_counter_is_numeric(caplog):
    """_perf_cost_lookup_seconds defaults to 0.0; a known-tool shot under DEBUG
    accumulates into it. Kills `_perf_cost_lookup_seconds = None` (None += dt).
    """
    bus, tracker, _db_ = _make(equipment_cost_lookup=lambda _: 0.5)
    tracker.start_session()
    # Equip a hotbar tool so the cost-lookup branch (tool is not None) runs.
    tracker._on_tool_changed({"tool_name": "Korss H400"})
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 7.0, "timestamp": datetime.now(tz=None)}
        )
    assert tracker.current_accumulator.shots_fired == 1


def _perf_log_fields(caplog) -> dict:
    """Parse the 'Tracker combat perf:' debug line into its numeric fields."""
    line = next(
        r.getMessage()
        for r in caplog.records
        if r.getMessage().startswith("Tracker combat perf:")
    )
    m = re.search(
        r"shots=(\d+) unknown=(\d+) inference_misses=(\d+) "
        r"avg_shot_ms=([\d.]+) avg_cost_lookup_ms=([\d.]+)",
        line,
    )
    assert m, f"unparseable perf line: {line!r}"
    return {
        "shots": int(m.group(1)),
        "unknown": int(m.group(2)),
        "inference_misses": int(m.group(3)),
        "avg_shot_ms": float(m.group(4)),
        "avg_cost_lookup_ms": float(m.group(5)),
    }


def test_perf_window_flush_reports_exact_counter_values(caplog):
    """Forcing the 15s perf window to have elapsed flushes the debug log; the
    counters it reports start from their zero seeds.

    Kills the perf-counter seed *value* mutants (only observable in this log):
      - _perf_shot_count = 1   -> shots one too high
      - _perf_unknown_tool_shots = 1 -> unknown one too high
      - _perf_shot_seconds = 1.0     -> avg_shot_ms ~1000ms too high
    """
    bus, tracker, _db_ = _make()
    tracker.start_session()
    # Pretend the perf window opened > 15s ago so the next shot flushes the log.
    tracker._perf_window_started = _monotonic() - 100.0
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        # One unknown-tool shot: shots=1, unknown=1, inference_misses=0.
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 7.0, "timestamp": datetime.now(tz=None)}
        )
    fields = _perf_log_fields(caplog)
    assert fields["shots"] == 1
    assert fields["unknown"] == 1
    assert fields["inference_misses"] == 0
    # A single just-measured shot is far below a millisecond; the =1.0s seed
    # would push the average to ~1000ms.
    assert fields["avg_shot_ms"] < 100.0


def test_perf_window_flush_reports_inference_miss_and_cost_lookup(caplog):
    """Flush the perf log under trifecta inference-miss + known-tool conditions.

    Kills:
      - _perf_inference_misses = 1   -> inference_misses one too high
      - _perf_cost_lookup_seconds = 1.0 -> avg_cost_lookup_ms ~1000ms too high
    """
    bus, tracker, _db_ = _make(
        weapon_attribution_trifecta_provider=lambda: True,
        equipment_cost_lookup=lambda _: 0.5,
    )
    tracker.start_session()
    # A known last-offensive tool so the dodge/known path performs a cost lookup
    # while still being an inference miss is not possible together; instead use
    # a damage shot that misses attribution (inference miss) - its tool is None
    # so no cost lookup runs. To exercise cost-lookup too, prime a last tool.
    tracker._last_offensive_tool_name = "Korss H400"
    tracker._perf_window_started = _monotonic() - 100.0
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        # damage_dealt in trifecta with no profiles -> attribution miss.
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 7.0, "timestamp": datetime.now(tz=None)}
        )
    fields = _perf_log_fields(caplog)
    # One inference miss recorded (seed 0 + 1), not 2 (seed 1 + 1).
    assert fields["inference_misses"] == 1
    assert fields["avg_cost_lookup_ms"] < 100.0


def test_perf_window_flush_cost_lookup_known_tool(caplog):
    """A known hotbar-tool shot performs a cost lookup; the flushed log's
    avg_cost_lookup_ms reflects only the just-measured tiny duration.

    Kills `_perf_cost_lookup_seconds = 1.0` (would inflate avg by ~1000ms).
    """
    bus, tracker, _db_ = _make(equipment_cost_lookup=lambda _: 0.5)
    tracker.start_session()
    tracker._on_tool_changed({"tool_name": "Korss H400"})
    tracker._perf_window_started = _monotonic() - 100.0
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 7.0, "timestamp": datetime.now(tz=None)}
        )
    fields = _perf_log_fields(caplog)
    assert fields["shots"] == 1
    assert fields["avg_cost_lookup_ms"] < 100.0


# ---------------------------------------------------------------------------
# __init__: demo-seed priming hook (frozen guard + maybe_prime(self))
# ---------------------------------------------------------------------------


def test_demo_scenario_env_primes_tracker_in_non_frozen_build(monkeypatch):
    """With ENTROPIAORME_DEMO_SCENARIO=mid_hunt set and sys.frozen falsy, the
    dev-only priming hook runs and primes the tracker (a session + kills).

    Kills:
      - frozen guard inversion `if not getattr(sys,"frozen",False)` -> `if ...`
      - default flip `getattr(sys,"frozen",False)` -> `getattr(sys,"frozen",True)`
      - `maybe_prime_tracker_from_env(self)` -> `(None)`
    all of which skip / misdirect the priming so the tracker stays unprimed.
    """
    monkeypatch.setenv("ENTROPIAORME_DEMO_SCENARIO", "mid_hunt")
    # mid_hunt priming writes skill gains; the table lives in app_database in
    # production, so create it here before construction triggers the hook.
    db = _db()
    init_tracking_tables(db)
    _add_skill_gains_table(db)
    _bus, tracker, db = _make(db=db)
    # Priming installs an in-memory session and writes kills to the DB.
    assert tracker.is_tracking is True
    kill_count = db.execute("SELECT COUNT(*) FROM kills").fetchone()[0]
    assert kill_count > 0


def test_no_demo_env_leaves_tracker_unprimed(monkeypatch):
    """Sanity baseline: without the env var the hook is a no-op."""
    monkeypatch.delenv("ENTROPIAORME_DEMO_SCENARIO", raising=False)
    _bus, tracker, db = _make()
    assert tracker.is_tracking is False
    assert db.execute("SELECT COUNT(*) FROM kills").fetchone()[0] == 0


# ---------------------------------------------------------------------------
# _recover_orphaned_sessions
# ---------------------------------------------------------------------------


def _seed_orphan(
    db: sqlite3.Connection,
    *,
    session_id: str,
    started_at: float,
    kill_ts: float | None,
    loot_ped: float = 0.0,
) -> None:
    """Insert an active (orphaned) session with one optional kill."""
    init_tracking_tables(db)
    db.execute(
        "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) "
        "VALUES (?, ?, 1, 'mob')",
        (session_id, started_at),
    )
    if kill_ts is not None:
        kid = str(uuid.uuid4())
        db.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, loot_total_ped) "
            "VALUES (?, ?, 'Atrox', ?, ?)",
            (kid, session_id, kill_ts, loot_ped),
        )
    db.commit()


def test_recover_closes_orphaned_session_and_sets_ended_at_to_latest_kill():
    """Recovery marks is_active=0 and sets ended_at to the latest kill ts.

    Exercises the SELECT/UPDATE statements (which the SQL-case mutants leave
    behaviourally identical) and confirms the recovery actually closes the row.
    """
    db = _db()
    sid = str(uuid.uuid4())
    _seed_orphan(db, session_id=sid, started_at=1000.0, kill_ts=2500.0)
    HuntTracker(EventBus(), db)  # __init__ runs recovery
    row = db.execute(
        "SELECT is_active, ended_at FROM tracking_sessions WHERE id = ?", (sid,)
    ).fetchone()
    assert row[0] == 0
    assert row[1] == 2500.0


def test_recover_uses_started_at_when_no_kills():
    """With no kills, ended_at falls back to started_at."""
    db = _db()
    sid = str(uuid.uuid4())
    _seed_orphan(db, session_id=sid, started_at=1234.0, kill_ts=None)
    HuntTracker(EventBus(), db)
    row = db.execute(
        "SELECT is_active, ended_at FROM tracking_sessions WHERE id = ?", (sid,)
    ).fetchone()
    assert row[0] == 0
    assert row[1] == 1234.0


def test_recover_warning_reports_session_prefix_and_kill_count(caplog):
    """The recovery warning logs the 8-char session prefix and the real kill
    count.

    Kills:
      - `count_row = ...fetchone() -> None` (kill_count forced to 0)
      - `session_id[:8] -> None` and `[:8] -> [:9]`
      - the warning-message string literals (text / case mutants)
    """
    db = _db()
    sid = str(uuid.uuid4())
    # Two kills so the real count (2) differs from the count_row=None fallback (0).
    _seed_orphan(db, session_id=sid, started_at=1000.0, kill_ts=2000.0)
    db.execute(
        "INSERT INTO kills (id, session_id, mob_name, timestamp) VALUES (?, ?, 'A', ?)",
        (str(uuid.uuid4()), sid, 2100.0),
    )
    db.commit()
    with caplog.at_level(logging.WARNING, logger="backend.tracking.tracker"):
        HuntTracker(EventBus(), db)
    msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.WARNING]
    # Assert the EXACT rendered message - this pins every literal in the two
    # implicitly-concatenated format strings (text/case/XX-wrap mutants), the
    # 8-char (not 9-char) session prefix, and the real kill count (not the
    # count_row=None fallback of 0).
    expected = (
        f"Recovered orphaned session {sid[:8]}: 2 kills preserved, "
        "in-progress accumulator at crash time was lost"
    )
    assert expected in msgs, f"expected exact recovery warning, got {msgs!r}"


def test_recover_writes_session_summary_for_recovered_session():
    """Recovery calls write_session_summary(db, session_id) for the recovered
    session - a qualifying session gets a session_summaries row.

    Kills `write_session_summary(self._db, session_id) -> (..., None)`: with
    None the real session's summary is never written (None -> no/other row).
    """
    db = _db()
    _add_skill_gains_table(db)
    sid = str(uuid.uuid4())
    # Orphaned session with a kill (gives ended_at > started_at and weapon cost).
    init_tracking_tables(db)
    db.execute(
        "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) "
        "VALUES (?, 1000.0, 1, 'mob')",
        (sid,),
    )
    kid = str(uuid.uuid4())
    db.execute(
        "INSERT INTO kills (id, session_id, mob_name, timestamp, loot_total_ped) "
        "VALUES (?, ?, 'Atrox', 4600.0, 5.0)",
        (kid, sid),
    )
    # Weapon cost so cycled_ped > 0.
    db.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) "
        "VALUES (?, 'Korss H400', 100, 0.01)",
        (kid,),
    )
    # A skill gain so the session qualifies for a summary.
    db.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES (?, 4600.0, 'Rifle', 1.0, 0.5)",
        (sid,),
    )
    db.commit()

    HuntTracker(EventBus(), db)
    n = db.execute(
        "SELECT COUNT(*) FROM session_summaries WHERE session_id = ?", (sid,)
    ).fetchone()[0]
    assert n == 1


# ---------------------------------------------------------------------------
# _reset_weapon_runtime_state  (via start_session -> reset to None)
# ---------------------------------------------------------------------------


def test_reset_weapon_runtime_state_clears_active_key_to_none():
    """_reset_weapon_runtime_state sets _active_weapon_state_key back to None so
    _active_weapon_state() returns None after a reset.

    Kills `_active_weapon_state_key = None -> ""` would leave a falsy-but-not-None
    key; here we assert the key is exactly None (the `is None` fast-path) AND
    that observed/last-tool fields are cleared to None.
    """
    bus, tracker, _db_ = _make()
    # Dirty the runtime state, then reset.
    tracker._active_weapon_state_key = "Korss H400"
    tracker._active_weapon_observed_name = "Korss H400"
    tracker._last_offensive_tool_name = "Korss H400"
    tracker._reset_weapon_runtime_state()
    assert tracker._active_weapon_state_key is None
    assert tracker._active_weapon_observed_name is None
    assert tracker._last_offensive_tool_name is None
    # And the fast-path returns None.
    assert tracker._active_weapon_state() is None


# ---------------------------------------------------------------------------
# _active_weapon_state
# ---------------------------------------------------------------------------


def _enhancer_props(slots: int) -> dict:
    return {
        "weapon_entity": {
            "name": "Korss H400",
            "damage": {"impact": 10.0},
            "economy": {"decay": 1.0, "ammo_burn": 50},
        },
        "weapon_markup": 100,
        "damage_enhancers": slots,
    }


def test_active_weapon_state_returns_the_keyed_state():
    """When a weapon key is set, _active_weapon_state() returns exactly the
    matching _DamageEnhancerState.

    Kills:
      - `if key is None:` -> `if key is not None:` (would return None when a
        weapon IS active)
      - `.get(self._active_weapon_state_key)` -> `.get(None)` (would miss the key)
    """
    _bus, tracker, _db_ = _make()
    state = _DamageEnhancerState.from_props("Korss H400", _enhancer_props(2))
    tracker._weapon_enhancer_states = {"Korss H400": state}
    tracker._active_weapon_state_key = "Korss H400"
    assert tracker._active_weapon_state() is state


def test_active_weapon_state_returns_none_when_no_key():
    """With no active key the method returns None (baseline for the `is None`
    fast-path)."""
    _bus, tracker, _db_ = _make()
    tracker._weapon_enhancer_states = {
        "Korss H400": _DamageEnhancerState.from_props("Korss H400", _enhancer_props(1))
    }
    tracker._active_weapon_state_key = None
    assert tracker._active_weapon_state() is None
