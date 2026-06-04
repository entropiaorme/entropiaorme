"""Mutation-hardening tests for HuntTracker.start_session and
HuntTracker._refresh_loot_filter (campaign cluster tracker__c4).

Each test drives the real backend.tracking.tracker against an in-memory
SQLite DB and the in-process event bus, asserting the exact behaviour a
surviving mutant would break:

  * the per-session state reset block (heal cost, warning flags, mob
    stamping, dedup state),
  * the ``and`` guard on the tag-mode entry branch,
  * the six event-bus subscriptions wired at session start,
  * the persisted INSERT and the "Session started" log line / published
    EVENT_SESSION_STARTED payload,
  * the loot-filter refresh that seeds the active blacklist.

The tracker logger name used for caplog assertions.
"""

import logging
import sqlite3
from datetime import datetime

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_LOOT_GROUP,
    EVENT_SESSION_STARTED,
)
from backend.tracking.loot_filter import normalize_blacklist
from backend.tracking.tracker import HuntTracker

TRACKER_LOGGER = "backend.tracking.tracker"


def _enhancer_props(slots: int) -> dict:
    """A weapon props payload carrying ``slots`` damage enhancers."""
    return {
        "weapon_entity": {
            "damage": {"impact": 10.0},
            "economy": {"decay": 1.0, "ammo_burn": 50},
        },
        "weapon_markup": 100,
        "damage_enhancers": slots,
    }


def _new_tracker(bus: EventBus, db: sqlite3.Connection, **kwargs) -> HuntTracker:
    return HuntTracker(bus, db, **kwargs)


def _fresh_db() -> sqlite3.Connection:
    return sqlite3.connect(":memory:", check_same_thread=False)


def _one_shot_one_loot(bus: EventBus, items, total_ped, when=None):
    """Drive one offensive shot then one loot group (one Kill)."""
    when = when or datetime.now(tz=None)
    bus.publish(
        EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": when}
    )
    bus.publish(
        EVENT_LOOT_GROUP,
        {"items": items, "total_ped": total_ped, "timestamp": when},
    )


# ---------------------------------------------------------------------------
# Heal-cost reset: _session_heal_cost = 0.0  (mutmut_10 -> None, mutmut_11 -> 1.0)
# ---------------------------------------------------------------------------
class TestSessionHealCostReset:
    def test_session_heal_cost_persists_as_zero_with_no_heals(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db)
        session = tracker.start_session()
        # No heal events occur, so the start-of-session reset value is what is
        # persisted on stop. mutmut_10 makes it None (NULL), mutmut_11 -> 1.0.
        assert tracker._session_heal_cost == 0.0
        tracker.stop_session()
        row = db.execute(
            "SELECT heal_cost FROM tracking_sessions WHERE id = ?",
            (session.id,),
        ).fetchone()
        assert row[0] == 0.0


# ---------------------------------------------------------------------------
# Heal-warning emission: _heal_warning_emitted = False (mutmut_13 -> True),
# _session_warnings = [] (mutmut_14 -> None), _last_heal_time = None
# (mutmut_9 -> ""). A self_heal with no heal tool equipped must emit exactly
# one warning into the list.
# ---------------------------------------------------------------------------
class TestHealWarningEmission:
    def test_self_heal_without_tool_emits_warning(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db)
        tracker.start_session()
        bus.publish(
            EVENT_COMBAT,
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime.now(tz=None)},
        )
        # mutmut_14 (None list) -> append swallowed, stays None.
        # mutmut_9 ("" heal time) -> subtraction crashes before the warning.
        # mutmut_13 (already-emitted True) -> `not True` suppresses the warning.
        assert tracker._session_warnings == [
            "Healing detected: no heal tool equipped via hotbar"
        ]
        assert tracker._heal_warning_emitted is True


# ---------------------------------------------------------------------------
# Trifecta unmatched warning: _trifecta_unmatched_warning_emitted = False
# (mutmut_28 -> True). With trifecta attribution on and no weapon profiles
# loaded, a damage shot fails to match and must record the warning.
# ---------------------------------------------------------------------------
class TestTrifectaUnmatchedWarning:
    def test_unmatched_trifecta_damage_emits_warning(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(
            bus,
            db,
            weapon_attribution_trifecta_provider=lambda: True,
            trifecta_resolver=lambda: None,
        )
        tracker.start_session()
        bus.publish(
            EVENT_COMBAT,
            {
                "type": "damage_dealt",
                "amount": 12.3,
                "timestamp": datetime.now(tz=None),
            },
        )
        assert tracker._session_warnings == [
            "Trifecta attribution: damage fell outside both weapon ranges"
        ]


# ---------------------------------------------------------------------------
# Hotbar reset: _active_hotbar_tool_name = None (mutmut_8 -> "").
# With no tool-changed event, the first shot's phase key must be "Unknown",
# not the empty string a "" reset would produce.
# ---------------------------------------------------------------------------
class TestActiveHotbarToolReset:
    def test_first_shot_phase_key_is_unknown_without_tool(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db, equipment_cost_lookup=lambda _n: 1.23)
        tracker.start_session()
        _one_shot_one_loot(
            bus,
            items=[{"item_name": "Animal Hide", "quantity": 1, "value_ped": 1.0}],
            total_ped=1.0,
        )
        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert "Unknown" in kill.tool_stats
        assert "" not in kill.tool_stats


# NOTE: the _confirmed_mob_name / _confirmed_mob_species / _confirmed_mob_maturity
# / _mob_source reset lines (mutmut_18..24) are DEAD STORES - start_session calls
# self._clear_mob_state() two lines later, which unconditionally resets all four
# to ""/None before any kill can read them. Tag/manual mode then overwrites them
# from the providers. So those mutants are behaviourally equivalent and are
# RECORDED as such rather than killed (verified: a kill stamps "Unknown"/"" in
# the default config regardless of the injected value).


# ---------------------------------------------------------------------------
# Tag-mode entry guard: `is_session_tag_mode() and _session_mob_tracking_tag`
# (mutmut_29 -> `or`). In mob mode with a non-empty configured tag the guard
# is False, so the tag must NOT be locked as the mob; `or` would lock it.
# ---------------------------------------------------------------------------
class TestTagModeEntryGuard:
    def test_mob_mode_with_tag_does_not_lock_tag(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(
            bus,
            db,
            mob_tracking_mode_provider=lambda: "mob",
            mob_tracking_tag_provider=lambda: "MyTag",
            manual_mob_entry_enabled_provider=lambda: True,
            manual_mob_provider=lambda: None,
        )
        tracker.start_session()
        assert tracker._confirmed_mob_name == ""
        assert tracker._mob_source is None
        _one_shot_one_loot(
            bus,
            items=[{"item_name": "Animal Hide", "quantity": 1, "value_ped": 1.0}],
            total_ped=1.0,
        )
        result = tracker.stop_session()
        assert result is not None
        # `or` would have stamped "MyTag" via _set_session_tag.
        assert result.kills[0].mob_name == "Unknown"


# ---------------------------------------------------------------------------
# Heal-tool subscription: subscribe(EVENT_ACTIVE_HEAL_TOOL_CHANGED,
# _on_heal_tool_changed). mutmut_53 subscribes under None (event never
# delivered); mutmut_54 subscribes a None callback (delivery raises and is
# swallowed). Either way the handler never runs, so equipping a heal tool
# leaves _active_heal_tool_name unset.
# ---------------------------------------------------------------------------
class TestHealToolSubscription:
    def test_heal_tool_changed_updates_active_heal_tool(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db)
        tracker.start_session()
        assert tracker._active_heal_tool_name is None
        bus.publish(
            EVENT_ACTIVE_HEAL_TOOL_CHANGED,
            {"tool_name": "FAP-5", "cost_per_use_ped": 0.3, "reload_seconds": 1.5},
        )
        assert tracker._active_heal_tool_name == "FAP-5"
        assert tracker._heal_cost_per_use_ped == 0.3


# ---------------------------------------------------------------------------
# Enhancer-break subscription: subscribe(EVENT_ENHANCER_BREAK,
# _on_enhancer_break). mutmut_61 subscribes under None; mutmut_62 subscribes
# a None callback. Either way a break event must NOT deplete the active
# weapon's enhancer slots.
# ---------------------------------------------------------------------------
class TestEnhancerBreakSubscription:
    def test_enhancer_break_depletes_active_slot(self):
        db = _fresh_db()
        bus = EventBus()
        profile = _enhancer_props(2)
        tracker = _new_tracker(
            bus,
            db,
            equipment_profile_lookup=lambda n: profile if n == "MyGun" else None,
            equipment_cost_lookup=lambda _n: 0.5,
        )
        tracker.start_session()
        now = datetime.now(tz=None)
        bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": "MyGun"})
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        state = tracker._active_weapon_state()
        assert state is not None
        assert state.active_slots == 2
        bus.publish(
            EVENT_ENHANCER_BREAK,
            {
                "enhancer_name": "Weapon Damage Enhancer",
                "item_name": "MyGun",
                "remaining": 1,
                "shrapnel_ped": 0.5,
            },
        )
        # The break handler must have run and applied the depletion.
        state = tracker._active_weapon_state()
        assert state is not None
        assert state.active_slots == 1


# ---------------------------------------------------------------------------
# Persisted INSERT into tracking_sessions (mutmut_70/_71/_73/_75 only change
# SQL keyword/identifier case, which SQLite ignores - recorded equivalent).
# We still pin the row the INSERT writes so the persistence intent is covered.
# ---------------------------------------------------------------------------
class TestSessionInsert:
    def test_start_session_persists_active_row_with_mode(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db, mob_tracking_mode_provider=lambda: "tag")
        session = tracker.start_session()
        row = db.execute(
            "SELECT id, is_active, mob_tracking_mode "
            "FROM tracking_sessions WHERE id = ?",
            (session.id,),
        ).fetchone()
        assert row is not None
        assert row[0] == session.id
        assert row[1] == 1
        assert row[2] == "tag"


# ---------------------------------------------------------------------------
# Start log line: log.info("Session started: %s", session_id[:8]).
# mutmut_76..83 alter the format string, the args, or the slice. caplog pins
# the exact INFO record.
# ---------------------------------------------------------------------------
class TestStartSessionLog:
    def test_logs_exact_session_started_message(self, caplog):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db)
        with caplog.at_level(logging.INFO, logger=TRACKER_LOGGER):
            session = tracker.start_session()
        expected = f"Session started: {session.id[:8]}"
        # getMessage() raises for the malformed-format mutants (None fmt / %S);
        # building this list at all kills them, and the exact-match kills the
        # rest. Filter to the tracker logger to ignore unrelated records.
        messages = [
            r.getMessage()
            for r in caplog.records
            if r.name == TRACKER_LOGGER and r.levelno == logging.INFO
        ]
        assert expected in messages
        # The slice is exactly 8 chars: the 9-char slice mutant (mutmut_83)
        # would emit a longer id and fail the exact equality above, but guard
        # against an accidental 9-char match by pinning the length.
        assert len(session.id[:8]) == 8


# ---------------------------------------------------------------------------
# Published start event: publish(EVENT_SESSION_STARTED,
# {"session_id": session_id}). mutmut_84 publishes under None; mutmut_85/_87
# publish a None payload; mutmut_88/_89 mangle the dict key.
# ---------------------------------------------------------------------------
class TestStartSessionEvent:
    def test_publishes_session_started_with_id_payload(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db)
        received: list = []
        bus.subscribe(EVENT_SESSION_STARTED, lambda data: received.append(data))
        session = tracker.start_session()
        # mutmut_84 (wrong event) -> handler never called -> empty list.
        assert len(received) == 1
        payload = received[0]
        # mutmut_85/_87 (None payload) -> subscript/`.get` would fail.
        assert payload is not None
        # mutmut_88/_89 (mangled key) -> "session_id" key absent.
        assert payload == {"session_id": session.id}


# ---------------------------------------------------------------------------
# _refresh_loot_filter: blacklist = self._loot_filter_blacklist
# (mutmut_1 -> None). With a configured blacklist and no provider, the active
# filter must reflect the configured names, so a blacklisted item is dropped
# from a kill's loot. The None mutant falls back to the default blacklist
# (which does not contain the configured name), keeping the item.
# ---------------------------------------------------------------------------
class TestRefreshLootFilter:
    def test_configured_blacklist_filters_named_item(self):
        db = _fresh_db()
        bus = EventBus()
        tracker = _new_tracker(bus, db, loot_filter_blacklist=["Shrapnel"])
        tracker.start_session()
        # Sanity: the active blacklist came from the configured list, not the
        # default universal-ammo-only set.
        assert tracker._loot_blacklist == normalize_blacklist(["Shrapnel"])
        assert tracker._loot_blacklist != normalize_blacklist(None)
        _one_shot_one_loot(
            bus,
            items=[
                {"item_name": "Shrapnel", "quantity": 1, "value_ped": 1.0},
                {"item_name": "Animal Hide", "quantity": 2, "value_ped": 3.0},
            ],
            total_ped=4.0,
        )
        result = tracker.stop_session()
        assert result is not None
        names = [li.item_name for li in result.kills[0].loot_items]
        assert names == ["Animal Hide"]
        assert "Shrapnel" not in names
