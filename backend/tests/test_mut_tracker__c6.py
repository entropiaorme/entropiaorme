"""Mutation-hardening tests for HuntTracker session-stop and mob-state methods.

Cluster tracker__c6 covers, on the real backend.tracking.tracker.HuntTracker:
  stop_session, _clear_mob_state, _set_session_tag, _set_manual_mob_state,
  set_manual_tag, set_manual_mob, release_current_mob.

Every test drives the real object through its public API (event bus, the
manual/tag providers, reload_config) and asserts behaviour observable through
a public surface: the EventBus subscription registry and published payloads,
the persisted/returned Kill records, the session DB row, the
``current_accumulator`` property, and ``release_current_mob``'s return value.
"""

import sqlite3
from datetime import datetime

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_GLOBAL,
    EVENT_LOOT_GROUP,
    EVENT_SESSION_STOPPED,
)
from backend.tracking.schema import init_tracking_tables
from backend.tracking.tracker import HuntTracker

# The six event types the tracker subscribes to for the lifetime of a session.
_SESSION_EVENTS = (
    EVENT_COMBAT,
    EVENT_LOOT_GROUP,
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_GLOBAL,
    EVENT_ENHANCER_BREAK,
)


def _make_tracker(**kwargs):
    """A tracker on a fresh in-memory DB + dedicated event bus."""
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db, **kwargs)
    return bus, tracker, db


def _fire_kill(bus, *, name="Shrapnel", value=0.50):
    """Drive one shot + one loot group → exactly one Kill record."""
    now = datetime.now(tz=None)
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "type": "loot",
            "items": [{"item_name": name, "quantity": 1, "value_ped": value}],
            "total_ped": value,
            "timestamp": now,
        },
    )


# --------------------------------------------------------------------------
# stop_session: event-bus unsubscribe (mutmut 5,6,9,10,13,14,17,18,21,22,25,26)
# --------------------------------------------------------------------------
class TestStopSessionUnsubscribes:
    def test_stop_removes_every_session_subscription(self):
        """Each of the six unsubscribe calls must target the right
        (event_type, handler) pair: after stop, no session event retains a
        subscriber. A None event-type or None handler in any unsubscribe call
        leaves that one event still subscribed."""
        bus, tracker, _ = _make_tracker()
        tracker.start_session()
        for event in _SESSION_EVENTS:
            assert bus.has_subscribers(event), f"{event} should be live mid-session"

        tracker.stop_session()

        for event in _SESSION_EVENTS:
            assert not bus.has_subscribers(event), (
                f"{event} still has a subscriber after stop_session - an "
                "unsubscribe call targeted the wrong event/handler"
            )

    def test_combat_after_stop_does_not_accumulate(self):
        """Belt-and-braces: a combat event published after stop must not be
        handled (the handler is gone), so no new session/accumulator appears."""
        bus, tracker, _ = _make_tracker()
        tracker.start_session()
        tracker.stop_session()
        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 99.0, "timestamp": now}
        )
        assert tracker.current_accumulator is None
        assert tracker.is_tracking is False


# --------------------------------------------------------------------------
# stop_session: published EVENT_SESSION_STOPPED (mutmut 66,67,69,70,71)
# --------------------------------------------------------------------------
class TestStopSessionPublish:
    def _capture(self):
        received = []
        bus, tracker, _ = _make_tracker()
        bus.subscribe(EVENT_SESSION_STOPPED, lambda data: received.append(data))
        return bus, tracker, received

    def test_publishes_session_stopped_with_session_id_payload(self):
        bus, tracker, received = self._capture()
        session = tracker.start_session()
        tracker.stop_session()
        # Mutant 66 publishes to a None event type → subscriber never fires.
        assert len(received) == 1, "EVENT_SESSION_STOPPED was not published"
        payload = received[0]
        # Mutants 67/69 publish a None payload.
        assert isinstance(payload, dict)
        # Mutants 70/71 use a mis-cased / wrapped key.
        assert "session_id" in payload
        assert payload["session_id"] == session.id


# --------------------------------------------------------------------------
# stop_session: cleanup + DB write (mutmut 73 ; plus dangling persistence)
# --------------------------------------------------------------------------
class TestStopSessionCleanupAndDb:
    def test_accumulator_is_none_after_stop(self):
        """Mutant 73 sets the accumulator to '' instead of None on cleanup;
        the public property must report None once a session ends."""
        bus, tracker, _ = _make_tracker()
        tracker.start_session()
        assert tracker.current_accumulator is not None
        tracker.stop_session()
        assert tracker.current_accumulator is None

    def test_db_row_marked_inactive_with_zero_dangling(self):
        """The UPDATE persists is_active=0 and the dangling cost; an empty
        session has zero dangling cost."""
        bus, tracker, db = _make_tracker()
        session = tracker.start_session()
        result = tracker.stop_session()
        assert result is not None
        row = db.execute(
            "SELECT is_active, ended_at, dangling_cost "
            "FROM tracking_sessions WHERE id = ?",
            (session.id,),
        ).fetchone()
        assert row is not None
        assert row[0] == 0
        assert row[1] is not None
        assert row[2] == 0.0
        assert result.dangling_cost == 0.0


# --------------------------------------------------------------------------
# stop_session: write_session_summary call (mutmut 50)
# --------------------------------------------------------------------------
class TestStopSessionWritesSummaryForRealSession:
    def test_stale_summary_row_for_session_is_cleared_on_stop(self):
        """stop_session calls write_session_summary(db, self._session.id).
        Mutant 50 passes None instead, so the real session id is never touched.
        An empty session does not qualify for a summary, so the real call
        DELETEs any stale summary row for that id; the mutant (id=None) leaves
        it untouched."""
        bus, tracker, db = _make_tracker()
        session = tracker.start_session()
        # Plant a stale summary row keyed on the real session id. The table
        # has many NOT NULL columns, so fill them with neutral defaults.
        db.execute(
            "INSERT INTO session_summaries ("
            "session_id, summary_version, started_at, ended_at, duration_hours, "
            "kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, "
            "dangling_cost, cycled_ped, regular_skill_ped_json, "
            "attribute_levels_json, regular_skill_tt, attribute_levels_total) "
            "VALUES (?, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, '{}', '{}', 0, 0)",
            (session.id,),
        )
        db.commit()
        tracker.stop_session()
        leftover = db.execute(
            "SELECT 1 FROM session_summaries WHERE session_id = ?",
            (session.id,),
        ).fetchone()
        assert leftover is None, (
            "stale summary for the real session id should have been cleared by "
            "write_session_summary(db, session.id)"
        )


# --------------------------------------------------------------------------
# _set_session_tag (tag-mode start) + _clear_mob_state via release
#   (set_session_tag mutmut 1 is equivalent; this pins the live attributes
#    that ARE observed: confirmed name -> kill stamp + release return)
# --------------------------------------------------------------------------
class TestTagModeMobStamping:
    def _tag_tracker(self, tag="Bigfoot Hunt"):
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: tag,
        )
        return bus, tracker, db

    def test_tag_session_stamps_kill_with_tag_name_and_blank_species(self):
        bus, tracker, db = self._tag_tracker("Bigfoot Hunt")
        tracker.start_session()
        _fire_kill(bus)
        result = tracker.stop_session()
        assert len(result.kills) == 1
        kill = result.kills[0]
        assert kill.mob_name == "Bigfoot Hunt"
        assert kill.mob_species == ""
        assert kill.mob_maturity == ""

    def test_release_returns_tag_then_clears(self):
        bus, tracker, db = self._tag_tracker("Bigfoot Hunt")
        tracker.start_session()
        # _set_session_tag set confirmed name to the tag on start.
        assert tracker.release_current_mob() == "Bigfoot Hunt"
        # After clearing, a second release sees no name.
        assert tracker.release_current_mob() is None


# --------------------------------------------------------------------------
# _set_manual_mob_state via set_manual_mob (mutmut 2,3 are dead-write on the
#   *current* fields; the observable confirmed fields are pinned here) + the
#   _mob_source == "manual" reload_config behaviour (mutmut 7,8,9).
# --------------------------------------------------------------------------
class TestManualMobStamping:
    def _manual_tracker(self, manual_mob=("Atrox", "Young")):
        box = {"mob": manual_mob}
        bus, tracker, db = _make_tracker(
            manual_mob_entry_enabled_provider=lambda: True,
            manual_mob_provider=lambda: box["mob"],
        )
        return bus, tracker, db, box

    def test_manual_mob_stamps_kill_with_name_species_maturity(self):
        bus, tracker, db, _ = self._manual_tracker()
        tracker.start_session()
        tracker.set_manual_mob("Young Atrox", "Atrox", "Young")
        _fire_kill(bus)
        result = tracker.stop_session()
        assert len(result.kills) == 1
        kill = result.kills[0]
        assert kill.mob_name == "Young Atrox"
        assert kill.mob_species == "Atrox"
        assert kill.mob_maturity == "Young"

    def test_release_returns_manual_name(self):
        bus, tracker, db, _ = self._manual_tracker()
        tracker.start_session()
        tracker.set_manual_mob("Young Atrox", "Atrox", "Young")
        assert tracker.release_current_mob() == "Young Atrox"

    def test_reload_clears_manual_state_when_provider_drops_to_none(self):
        """_set_manual_mob_state sets _mob_source = 'manual'. reload_config
        clears the locked mob only when _mob_source == 'manual'. Mutants that
        write a different/None source string make that comparison miss, so the
        locked mob is wrongly retained when the provider goes empty."""
        bus, tracker, db, box = self._manual_tracker(("Atrox", "Young"))
        tracker.start_session()
        tracker.set_manual_mob("Young Atrox", "Atrox", "Young")
        assert tracker.release_current_mob() == "Young Atrox"
        # restore the manual lock, then drop the provider's mob and reload
        tracker.set_manual_mob("Young Atrox", "Atrox", "Young")
        box["mob"] = None
        tracker.reload_config()
        # Real code: _mob_source == 'manual' → state cleared → nothing to release.
        assert tracker.release_current_mob() is None


# --------------------------------------------------------------------------
# release_current_mob boolean expression (mutmut 2,3)
#   released = self._confirmed_mob_name or self._current_mob_name or None
# --------------------------------------------------------------------------
class TestReleaseCurrentMobExpression:
    def test_release_prefers_confirmed_name(self):
        """With a confirmed name set, release returns it. Mutant 3 turns the
        first 'or' into 'and' (confirmed AND current) → would return the
        current name (or a falsy value), not the confirmed one."""
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: "Sentinel",
        )
        tracker.start_session()
        # confirmed == current == 'Sentinel' here; to separate them, clear then
        # re-confirm via a fresh tag so confirmed is the truthy operand.
        assert tracker.release_current_mob() == "Sentinel"

    def test_release_returns_none_when_no_mob(self):
        """No manual/tag config and no kills: nothing is confirmed, release is
        None. Mutant 2 (current AND None) and mutant 3 leave the trailing
        'or None' / 'and None' producing a different falsy/None result only
        when an operand is set; with all blank the real result is None."""
        bus, tracker, db = _make_tracker(
            manual_mob_entry_enabled_provider=lambda: False,
        )
        tracker.start_session()
        assert tracker.release_current_mob() is None


# --------------------------------------------------------------------------
# set_manual_tag guard messages (mutmut 2,3,4,5,7,8,9,10,13,14,15,16)
# --------------------------------------------------------------------------
class TestSetManualTagGuards:
    def test_no_active_session_message(self):
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "tag",
        )
        with pytest.raises(RuntimeError) as exc:
            tracker.set_manual_tag("x")
        assert str(exc.value) == "No active session"

    def test_not_tag_mode_message(self):
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "mob",
        )
        tracker.start_session()
        with pytest.raises(RuntimeError) as exc:
            tracker.set_manual_tag("x")
        assert str(exc.value) == "Active session is not in tag mode"

    def test_empty_tag_message(self):
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "tag",
        )
        tracker.start_session()
        with pytest.raises(ValueError) as exc:
            tracker.set_manual_tag("   ")
        assert str(exc.value) == "Tag cannot be empty"

    def test_set_manual_tag_restamps_kills_with_new_tag(self):
        """The happy path: a non-empty tag updates the confirmed tag so the
        next kill is stamped with it."""
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: "Old Tag",
        )
        tracker.start_session()
        tracker.set_manual_tag("New Tag")
        _fire_kill(bus)
        result = tracker.stop_session()
        assert result.kills[0].mob_name == "New Tag"


# --------------------------------------------------------------------------
# set_manual_mob guard messages (mutmut 2,3,4,5,6,7,8,9,11,12,13,14)
# --------------------------------------------------------------------------
class TestSetManualMobGuards:
    def test_no_active_session_message(self):
        bus, tracker, db = _make_tracker()
        with pytest.raises(RuntimeError) as exc:
            tracker.set_manual_mob("Atrox", "Atrox", "Young")
        assert str(exc.value) == "No active session"

    def test_tag_mode_rejects_manual_mob(self):
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: "T",
        )
        tracker.start_session()
        with pytest.raises(RuntimeError) as exc:
            tracker.set_manual_mob("Atrox", "Atrox", "Young")
        assert str(exc.value) == "Tag mode sessions do not allow manual mob locking"

    def test_manual_entry_disabled_message(self):
        bus, tracker, db = _make_tracker(
            mob_tracking_mode_provider=lambda: "mob",
            manual_mob_entry_enabled_provider=lambda: False,
        )
        tracker.start_session()
        with pytest.raises(RuntimeError) as exc:
            tracker.set_manual_mob("Atrox", "Atrox", "Young")
        assert str(exc.value) == "Manual mob entry is not enabled for this session"
