"""Stateful and focused tests for the hunt tracker.

A ``RuleBasedStateMachine`` drives ``backend.tracking.tracker.HuntTracker``
through the production event-bus seam and asserts the accumulator / kill-model
invariants after every step. Focused tests pin the loot-dedup and global /
HoF correlation windows, which are time-sensitive and easier to assert directly.
"""

import sqlite3
from datetime import datetime, timedelta

import pytest
from hypothesis import settings
from hypothesis import strategies as st
from hypothesis.stateful import RuleBasedStateMachine, invariant, precondition, rule

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_COMBAT,
    EVENT_GLOBAL,
    EVENT_LOOT_GROUP,
)
from backend.tracking.tracker import HuntTracker

_AMOUNT = st.floats(
    min_value=0.0, max_value=1000.0, allow_nan=False, allow_infinity=False
)


def _tracker():
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db, player_name="Me")
    return bus, tracker, db


def _loot_group(value, item, ts):
    return {
        "items": [
            {
                "item_name": item,
                "quantity": 1,
                "value_ped": value,
                "is_enhancer_shrapnel": False,
            }
        ],
        "total_ped": value,
        "timestamp": ts,
    }


class HuntTrackerMachine(RuleBasedStateMachine):
    def __init__(self):
        super().__init__()
        self.db = sqlite3.connect(":memory:", check_same_thread=False)
        self.bus = EventBus()
        self.tracker = HuntTracker(self.bus, self.db, player_name="Me")
        self.clock = datetime(2025, 1, 1, 12, 0, 0)
        self.tracking = False
        self.exp_shots = 0
        self.exp_damage = 0.0
        self.exp_crits = 0
        self.exp_taken = 0.0
        self.exp_kills = 0
        self.exp_mob = "Unknown"

    def _reset_accumulator_model(self):
        self.exp_shots = 0
        self.exp_damage = 0.0
        self.exp_crits = 0
        self.exp_taken = 0.0

    def _advance(self, seconds):
        self.clock += timedelta(seconds=seconds)

    @precondition(lambda self: not self.tracking)
    @rule()
    def start(self):
        self.tracker.start_session()
        self.tracking = True
        self._reset_accumulator_model()
        self.exp_kills = 0
        self.exp_mob = "Unknown"

    @precondition(lambda self: self.tracking)
    @rule(amount=_AMOUNT)
    def fire_damage(self, amount):
        self._advance(0.1)
        self.bus.publish(
            EVENT_COMBAT,
            {"type": "damage_dealt", "amount": amount, "timestamp": self.clock},
        )
        self.exp_shots += 1
        self.exp_damage += amount

    @precondition(lambda self: self.tracking)
    @rule(amount=_AMOUNT)
    def fire_crit(self, amount):
        self._advance(0.1)
        self.bus.publish(
            EVENT_COMBAT,
            {"type": "critical_hit", "amount": amount, "timestamp": self.clock},
        )
        self.exp_shots += 1
        self.exp_damage += amount
        self.exp_crits += 1

    @precondition(lambda self: self.tracking)
    @rule(kind=st.sampled_from(["target_dodge", "target_evade", "target_jam"]))
    def miss(self, kind):
        self._advance(0.1)
        self.bus.publish(
            EVENT_COMBAT, {"type": kind, "amount": 0.0, "timestamp": self.clock}
        )
        self.exp_shots += 1

    @precondition(lambda self: self.tracking)
    @rule(amount=_AMOUNT)
    def take_damage(self, amount):
        self._advance(0.1)
        self.bus.publish(
            EVENT_COMBAT,
            {"type": "damage_received", "amount": amount, "timestamp": self.clock},
        )
        self.exp_taken += amount

    @precondition(lambda self: self.tracking)
    @rule(tool=st.sampled_from(["Weapon A", "Weapon B"]))
    def change_tool(self, tool):
        # Re-keys the accumulator's tool stats (merging "Unknown"); the aggregate
        # totals the invariants check must not move.
        self.bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": tool})

    @precondition(lambda self: self.tracking)
    @rule(mob=st.sampled_from(["Atrox", "Daikiba", "Combibo"]))
    def set_mob(self, mob):
        self.tracker.set_manual_mob(mob, mob, "")
        self.exp_mob = mob

    @precondition(lambda self: self.tracking)
    @rule(
        value=st.floats(
            min_value=0.0, max_value=500.0, allow_nan=False, allow_infinity=False
        )
    )
    def loot(self, value):
        # Advance past the dedup window and use a unique item name, so every loot
        # is a fresh kill (dedup is exercised separately, below).
        self._advance(3.0)
        item = f"Loot {self.exp_kills}"
        self.bus.publish(EVENT_LOOT_GROUP, _loot_group(value, item, self.clock))
        self.exp_kills += 1
        self._reset_accumulator_model()

        session = self.tracker.session
        assert session is not None
        kill = session.kills[-1]
        assert kill.mob_name == self.exp_mob
        assert kill.loot_total_ped == pytest.approx(round(value, 4))

    @precondition(lambda self: self.tracking)
    @rule()
    def stop(self):
        session = self.tracker.stop_session()
        self.tracking = False
        assert session is not None
        db_kills = self.db.execute(
            "SELECT COUNT(*) FROM kills WHERE session_id = ?", (session.id,)
        ).fetchone()[0]
        assert db_kills == len(session.kills) == self.exp_kills
        for kill in session.kills:
            shots = self.db.execute(
                "SELECT COALESCE(SUM(shots_fired), 0) FROM kill_tool_stats WHERE kill_id = ?",
                (kill.id,),
            ).fetchone()[0]
            assert shots == kill.shots_fired

    @invariant()
    def accumulator_is_consistent(self):
        if not self.tracking:
            return
        acc = self.tracker.current_accumulator
        session = self.tracker.session
        assert acc is not None and session is not None
        assert acc.shots_fired >= 0
        assert acc.critical_hits <= acc.shots_fired
        assert acc.shots_fired == sum(ts.shots_fired for ts in acc.tool_stats.values())
        assert acc.damage_dealt == pytest.approx(
            sum(ts.damage_dealt for ts in acc.tool_stats.values())
        )
        assert acc.shots_fired == self.exp_shots
        assert acc.critical_hits == self.exp_crits
        assert acc.damage_dealt == pytest.approx(self.exp_damage)
        assert acc.damage_taken == pytest.approx(self.exp_taken)
        assert len(session.kills) == self.exp_kills

    def teardown(self):
        try:
            if self.tracking:
                self.tracker.stop_session()
        finally:
            self.db.close()


TestHuntTracker = HuntTrackerMachine.TestCase
TestHuntTracker.settings = settings(
    max_examples=25, stateful_step_count=30, deadline=None
)


# --- focused: loot dedup window ---


def test_identical_loot_within_window_is_deduplicated():
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    bus.publish(EVENT_LOOT_GROUP, _loot_group(5.0, "Hide", t0))
    bus.publish(EVENT_LOOT_GROUP, _loot_group(5.0, "Hide", t0 + timedelta(seconds=1)))
    session = tracker.session
    assert session is not None
    assert len(session.kills) == 1
    bus.publish(EVENT_LOOT_GROUP, _loot_group(5.0, "Hide", t0 + timedelta(seconds=3)))
    assert len(session.kills) == 2
    tracker.stop_session()
    db.close()


def test_different_loot_within_window_is_not_deduplicated():
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    bus.publish(EVENT_LOOT_GROUP, _loot_group(5.0, "Hide", t0))
    bus.publish(EVENT_LOOT_GROUP, _loot_group(5.0, "Bone", t0 + timedelta(seconds=1)))
    session = tracker.session
    assert session is not None
    assert len(session.kills) == 2
    tracker.stop_session()
    db.close()


# --- focused: global / HoF correlation window ---


def _kill_with_loot(bus, tracker, t0):
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": t0})
    bus.publish(EVENT_LOOT_GROUP, _loot_group(50.0, "Hide", t0))


def test_global_within_five_seconds_tags_the_kill():
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _kill_with_loot(bus, tracker, t0)
    bus.publish(
        EVENT_GLOBAL,
        {
            "player": "Me",
            "type": "global_kill",
            "creature": "Atrox",
            "value": 50.0,
            "timestamp": t0 + timedelta(seconds=2),
        },
    )
    session = tracker.session
    assert session is not None
    assert session.kills[-1].is_global is True
    tracker.stop_session()
    db.close()


def test_global_outside_five_seconds_does_not_tag():
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _kill_with_loot(bus, tracker, t0)
    bus.publish(
        EVENT_GLOBAL,
        {
            "player": "Me",
            "type": "global_kill",
            "creature": "Atrox",
            "value": 50.0,
            "timestamp": t0 + timedelta(seconds=10),
        },
    )
    session = tracker.session
    assert session is not None
    assert session.kills[-1].is_global is False
    tracker.stop_session()
    db.close()


def test_global_from_another_player_is_ignored():
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _kill_with_loot(bus, tracker, t0)
    bus.publish(
        EVENT_GLOBAL,
        {
            "player": "SomeoneElse",
            "type": "global_kill",
            "creature": "Atrox",
            "value": 50.0,
            "timestamp": t0 + timedelta(seconds=2),
        },
    )
    session = tracker.session
    assert session is not None
    assert session.kills[-1].is_global is False
    tracker.stop_session()
    db.close()
