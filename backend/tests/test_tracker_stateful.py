"""Stateful and focused tests for the hunt tracker.

A ``RuleBasedStateMachine`` drives ``backend.tracking.tracker.HuntTracker``
through the production event-bus seam and asserts the accumulator / kill-model
invariants after every step. Focused tests pin the loot-dedup and global /
HoF correlation windows, which are time-sensitive and easier to assert directly.
"""

import sqlite3
from datetime import datetime, timedelta

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st
from hypothesis.stateful import RuleBasedStateMachine, invariant, precondition, rule

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_GLOBAL,
    EVENT_LOOT_GROUP,
    EVENT_SKILL_GAIN,
)
from backend.tracking.tracker import HuntTracker, _DamageEnhancerState

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
        # Highest accumulator.shots_fired observed in the current between-kills
        # window; resets to 0 on every kill / session boundary.
        self._window_shots_high_water = 0

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
        self._window_shots_high_water = 0

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
    @rule(
        kind=st.sampled_from(["player_dodge", "player_evade", "player_jam", "deflect"])
    )
    def player_defends(self, kind):
        # Defensive events the player wins: no shot is fired and no damage is
        # dealt or taken, so every tracker counter must stay put.
        self._advance(0.1)
        self.bus.publish(
            EVENT_COMBAT, {"type": kind, "amount": 0.0, "timestamp": self.clock}
        )

    @precondition(lambda self: self.tracking)
    @rule(amount=_AMOUNT)
    def heal(self, amount):
        # Self-heal touches no accumulator field the kill model snapshots.
        self._advance(0.1)
        self.bus.publish(
            EVENT_COMBAT,
            {"type": "self_heal", "amount": amount, "timestamp": self.clock},
        )

    @precondition(lambda self: self.tracking)
    @rule(
        amount=st.floats(
            min_value=0.0, max_value=50.0, allow_nan=False, allow_infinity=False
        )
    )
    def enhancer_break(self, amount):
        # No damage enhancer is configured on the tools driven here, so the
        # break is a no-op for the accumulator; it exercises the robustness of
        # the offensive/loot counters under an unrelated equipment event.
        self._advance(0.1)
        self.bus.publish(
            EVENT_ENHANCER_BREAK,
            {
                "type": "enhancer_break",
                "enhancer_name": "Weapon Damage Enhancer 1",
                "item_name": "Weapon A",
                "shrapnel_ped": amount,
                "remaining": 0,
                "timestamp": self.clock,
            },
        )

    @precondition(lambda self: self.tracking)
    @rule(
        amount=st.floats(
            min_value=0.0001, max_value=10.0, allow_nan=False, allow_infinity=False
        ),
        skill=st.sampled_from(
            ["Laser Weaponry Technology", "Anatomy", "Combat Reflexes"]
        ),
    )
    def skill_gain(self, amount, skill):
        # The hunt tracker does not subscribe to skill gains (a separate skill
        # tracker owns them); publishing one must leave every tracker counter
        # untouched.
        self._advance(0.1)
        self.bus.publish(
            EVENT_SKILL_GAIN,
            {
                "type": "skill_gain",
                "skill_name": skill,
                "amount": amount,
                "timestamp": self.clock,
            },
        )

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
        self._window_shots_high_water = 0

        session = self.tracker.session
        assert session is not None
        kill = session.kills[-1]
        assert kill.mob_name == self.exp_mob
        assert kill.loot_total_ped == pytest.approx(round(value, 4))

        # accumulator_resets_to_zero_on_kill: the post-loot accumulator is
        # zeroed before any further combat is processed.
        acc = self.tracker.current_accumulator
        assert acc is not None
        assert acc.shots_fired == 0
        assert acc.critical_hits == 0
        assert acc.damage_dealt == 0.0
        assert acc.damage_taken == 0.0
        assert acc.tool_stats == {}

        # kill_shots_fired_equals_tool_shots_sum + crits-bounded, on the
        # finalised kill snapshot.
        assert kill.shots_fired == sum(
            ts.shots_fired for ts in kill.tool_stats.values()
        )
        assert kill.critical_hits == sum(
            ts.critical_hits for ts in kill.tool_stats.values()
        )
        assert kill.critical_hits <= kill.shots_fired
        assert kill.damage_dealt == pytest.approx(
            sum(ts.damage_dealt for ts in kill.tool_stats.values())
        )

    @precondition(lambda self: self.tracking)
    @rule()
    def stop(self):
        dangling_before_stop = (
            self.tracker.current_accumulator.total_cost
            if self.tracker.current_accumulator is not None
            else 0.0
        )
        session = self.tracker.stop_session()
        self.tracking = False
        assert session is not None
        db_kills = self.db.execute(
            "SELECT COUNT(*) FROM kills WHERE session_id = ?", (session.id,)
        ).fetchone()[0]
        assert db_kills == len(session.kills) == self.exp_kills

        # The stopped session's persisted row reflects the close: deactivated,
        # ended, and carrying the dangling cost captured from the accumulator.
        row = self.db.execute(
            "SELECT is_active, ended_at, dangling_cost FROM tracking_sessions "
            "WHERE id = ?",
            (session.id,),
        ).fetchone()
        assert row is not None
        is_active, ended_at, db_dangling = row
        assert is_active == 0
        assert ended_at is not None
        assert db_dangling == pytest.approx(dangling_before_stop)
        assert db_dangling == pytest.approx(session.dangling_cost)

        for kill in session.kills:
            shots = self.db.execute(
                "SELECT COALESCE(SUM(shots_fired), 0) FROM kill_tool_stats WHERE kill_id = ?",
                (kill.id,),
            ).fetchone()[0]
            assert shots == kill.shots_fired
            # Deepen beyond count-only: the persisted kill row round-trips the
            # snapshotted damage / crit / loot values, not merely its shots.
            persisted = self.db.execute(
                "SELECT shots_fired, damage_dealt, critical_hits, loot_total_ped, "
                "mob_name FROM kills WHERE id = ?",
                (kill.id,),
            ).fetchone()
            assert persisted is not None
            p_shots, p_damage, p_crits, p_loot, p_mob = persisted
            assert p_shots == kill.shots_fired
            assert p_damage == pytest.approx(kill.damage_dealt)
            assert p_crits == kill.critical_hits
            assert p_loot == pytest.approx(kill.loot_total_ped)
            assert p_mob == kill.mob_name

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

    @invariant()
    def per_tool_crits_bounded_by_shots(self):
        # critical_hits_bounded_by_shots holds at per-tool granularity too:
        # a crit increment always rides on a shot increment for that tool.
        if not self.tracking:
            return
        acc = self.tracker.current_accumulator
        assert acc is not None
        for ts in acc.tool_stats.values():
            assert ts.critical_hits <= ts.shots_fired

    @invariant()
    def weapon_cost_is_sum_of_tool_costs(self):
        # weapon_cost_definition + total_cost decomposition: both are computed
        # properties, so they must equal their definitions exactly.
        if not self.tracking:
            return
        acc = self.tracker.current_accumulator
        assert acc is not None
        assert acc.weapon_cost == pytest.approx(
            sum(ts.cost_per_shot * ts.shots_fired for ts in acc.tool_stats.values())
        )
        assert acc.total_cost == pytest.approx(acc.weapon_cost + acc.enhancer_cost)

    @invariant()
    def counters_and_values_non_negative(self):
        # non_negative_counters_and_values across the live accumulator and every
        # finalised kill / tool-stat / loot-item the session holds.
        if not self.tracking:
            return
        acc = self.tracker.current_accumulator
        session = self.tracker.session
        assert acc is not None and session is not None
        assert acc.shots_fired >= 0
        assert acc.critical_hits >= 0
        assert acc.damage_dealt >= 0.0
        assert acc.damage_taken >= 0.0
        assert acc.enhancer_cost >= 0.0
        assert acc.weapon_cost >= 0.0
        for ts in acc.tool_stats.values():
            assert ts.shots_fired >= 0
            assert ts.critical_hits >= 0
            assert ts.damage_dealt >= 0.0
            assert ts.cost_per_shot >= 0.0
        for kill in session.kills:
            assert kill.shots_fired >= 0
            assert kill.critical_hits >= 0
            assert kill.damage_dealt >= 0.0
            assert kill.damage_taken >= 0.0
            assert kill.cost_ped >= 0.0
            assert kill.enhancer_cost >= 0.0
            assert kill.loot_total_ped >= 0.0
            for item in kill.loot_items:
                assert item.quantity >= 0
                assert item.value_ped >= 0.0

    @invariant()
    def shots_non_decreasing_between_kills(self):
        # shots_monotonic_non_decreasing_between_kills: within a single window
        # (loot and session boundaries reset the high-water mark) the
        # accumulator's shot count never moves backwards.
        if not self.tracking:
            return
        acc = self.tracker.current_accumulator
        assert acc is not None
        assert acc.shots_fired >= self._window_shots_high_water
        self._window_shots_high_water = acc.shots_fired

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


def _persisted_session_snapshot(db, session_id):
    """Read the persisted totals a stopped session should never change."""
    kill_count = db.execute(
        "SELECT COUNT(*) FROM kills WHERE session_id = ?", (session_id,)
    ).fetchone()[0]
    kill_rows = db.execute(
        "SELECT id, shots_fired, damage_dealt, critical_hits, loot_total_ped "
        "FROM kills WHERE session_id = ? ORDER BY id",
        (session_id,),
    ).fetchall()
    return kill_count, kill_rows


# --- focused: a stopped session ignores further combat and loot ---


def test_stopped_session_totals_are_immutable_to_replayed_events():
    # stopped_session_totals_immutable: assert on the observable outcome
    # (persisted kill count + per-kill totals) rather than subscription state,
    # so the property holds across any unsubscribe / null-guard refactor.
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _kill_with_loot(bus, tracker, t0)
    session = tracker.stop_session()
    assert session is not None

    before = _persisted_session_snapshot(db, session.id)

    # Replay a full spread of combat and loot after the stop; none of it may
    # reach the tracker.
    later = t0 + timedelta(seconds=30)
    bus.publish(
        EVENT_COMBAT, {"type": "damage_dealt", "amount": 99.0, "timestamp": later}
    )
    bus.publish(
        EVENT_COMBAT, {"type": "critical_hit", "amount": 99.0, "timestamp": later}
    )
    bus.publish(
        EVENT_COMBAT, {"type": "damage_received", "amount": 99.0, "timestamp": later}
    )
    bus.publish(EVENT_LOOT_GROUP, _loot_group(123.0, "Skin", later))

    after = _persisted_session_snapshot(db, session.id)
    assert after == before
    assert tracker.session is None
    db.close()


def test_post_stop_replay_is_idempotent():
    # Metamorphic: replaying the same post-stop event stream twice equals
    # replaying it once (the tracker is unsubscribed, so both are no-ops).
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _kill_with_loot(bus, tracker, t0)
    session = tracker.stop_session()
    assert session is not None

    once = t0 + timedelta(seconds=30)
    bus.publish(
        EVENT_COMBAT, {"type": "damage_dealt", "amount": 5.0, "timestamp": once}
    )
    bus.publish(EVENT_LOOT_GROUP, _loot_group(7.0, "Skin", once))
    snap_once = _persisted_session_snapshot(db, session.id)

    twice = t0 + timedelta(seconds=60)
    bus.publish(
        EVENT_COMBAT, {"type": "damage_dealt", "amount": 5.0, "timestamp": twice}
    )
    bus.publish(EVENT_LOOT_GROUP, _loot_group(7.0, "Skin", twice))
    snap_twice = _persisted_session_snapshot(db, session.id)

    assert snap_twice == snap_once
    db.close()


# --- focused: enhancer-stack redistribution conserves the total ---


@given(
    configured=st.integers(min_value=1, max_value=10),
    total=st.integers(min_value=-50, max_value=500),
)
@settings(max_examples=100, deadline=None)
def test_set_total_conserves_total(configured, total):
    # set_total_conserves_total: divmod redistribution preserves the clamped
    # total across the stacks; a negative request clamps to zero.
    state = _DamageEnhancerState(
        tool_name="Weapon A",
        props={"damage_enhancers": configured},
        stacks=[100] * configured,
    )
    state.set_total(total)
    expected = max(0, total)
    assert sum(state.stacks) == expected
    assert all(stack >= 0 for stack in state.stacks)


def test_set_total_is_a_no_op_without_slots():
    # set_total_conserves_total, zero-slot clause: no stacks means a genuine
    # no-op, no exception and no spurious slot.
    state = _DamageEnhancerState(tool_name="Weapon A", props={}, stacks=[])
    state.set_total(5)
    assert state.stacks == []
    assert state.active_slots == 0


# --- metamorphic: combat ordering within a kill window commutes ---


def _replay_combat(bus, tracker, t0, events):
    """Publish a sequence of (type, amount, dt) combat tuples, then loot."""
    for kind, amount, dt in events:
        bus.publish(
            EVENT_COMBAT,
            {"type": kind, "amount": amount, "timestamp": t0 + timedelta(seconds=dt)},
        )


_COMBAT_EVENT = st.tuples(
    st.sampled_from(
        ["damage_dealt", "critical_hit", "target_dodge", "damage_received"]
    ),
    st.floats(min_value=0.0, max_value=100.0, allow_nan=False, allow_infinity=False),
)


@given(events=st.lists(_COMBAT_EVENT, min_size=0, max_size=12))
@settings(max_examples=50, deadline=None)
def test_combat_order_within_window_is_commutative(events):
    # Reordering independent combat events within one kill window yields the
    # same accumulator totals (additive accumulation is order-independent).
    indexed = [(kind, amount, idx) for idx, (kind, amount) in enumerate(events)]

    def run(ordered):
        bus, tracker, db = _tracker()
        tracker.start_session()
        t0 = datetime(2025, 1, 1, 12, 0, 0)
        _replay_combat(bus, tracker, t0, ordered)
        acc = tracker.current_accumulator
        assert acc is not None
        out = (acc.shots_fired, acc.critical_hits, acc.damage_dealt, acc.damage_taken)
        tracker.stop_session()
        db.close()
        return out

    forward = run(indexed)
    reversed_out = run(list(reversed(indexed)))
    assert forward[0] == reversed_out[0]  # shots_fired
    assert forward[1] == reversed_out[1]  # critical_hits
    assert forward[2] == pytest.approx(reversed_out[2])  # damage_dealt
    assert forward[3] == pytest.approx(reversed_out[3])  # damage_taken


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
