"""Mutation-hardening tests for backend.testing.store_reducers (cluster c0).

Targets the surviving / no-test mutants on Reducer.__init__, install,
uninstall, _make_callback, TrackingReducer.initial_state / on_event /
_on_combat / _on_loot_group, tracking_view_state, and QuestsReducer.on_event.

Each test exercises the real production code in
``backend.testing.store_reducers`` and asserts the exact behaviour a
mutation would break.
"""

from __future__ import annotations

import sqlite3

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_LOOT_GROUP,
    EVENT_MISSION_RECEIVED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.testing.store_reducers import (
    QuestsReducer,
    TrackingReducer,
    TrackingViewContext,
    tracking_view_state,
)
from backend.tracking.tracker import HuntTracker

# Idle-state dict the view and the reducer both project before any
# session is active. Pinned exactly so any key-rename or default-value
# mutation in either initial_state / the idle branch is caught.
_IDLE_STATE = {
    "status": "idle",
    "session_id": None,
    "kill_count": 0,
    "shots_fired_total": 0,
    "damage_dealt_total": 0.0,
    "critical_hits_total": 0,
    "returns": 0.0,
}


# ── Reducer.__init__ / install / uninstall / _make_callback ──────────────────


def test_reducer_init_bus_is_none() -> None:
    """``__init__`` leaves ``_bus`` strictly ``None`` (not ``""``).

    Kills ``__init____mutmut_2`` (``self._bus = ""``): the install
    idempotence/relocation logic below branches on ``is None`` /
    ``is bus`` identity, so a falsy-but-not-None sentinel would corrupt
    those branches. We assert the public consequence: a freshly built
    reducer installs cleanly on its first bus.
    """
    reducer = TrackingReducer()
    assert reducer._bus is None


def test_install_subscribes_each_topic_and_dispatches_with_topic() -> None:
    """``install`` subscribes the reducer's topics so bus events fold in.

    Kills ``install__mutmut_4/5/6/7/8`` (subscribe args broken /
    dropped / ``None``) and ``_make_callback__mutmut_1..4`` (the
    dispatcher must forward BOTH the bound topic and the payload):
    publishing a combat event then a loot group through the bus must
    move the reducer's state exactly as a direct ``on_event`` would.
    """
    reducer = TrackingReducer()
    bus = EventBus()
    reducer.install(bus)

    bus.publish(EVENT_SESSION_STARTED, {"session_id": "s1"})
    bus.publish(EVENT_COMBAT, {"type": "critical_hit", "amount": 5.0})
    bus.publish(EVENT_LOOT_GROUP, {"items": [{"value_ped": 2.0}]})

    state = reducer.state
    assert state["status"] == "active"
    assert state["session_id"] == "s1"
    assert state["shots_fired_total"] == 1
    assert state["critical_hits_total"] == 1
    assert state["damage_dealt_total"] == 5.0
    assert state["kill_count"] == 1
    assert state["returns"] == 2.0


def test_install_idempotent_on_same_bus_does_not_double_subscribe() -> None:
    """Re-installing on the SAME bus must not duplicate subscriptions.

    Kills ``install__mutmut_1`` (``is bus`` -> ``is not bus``): the
    guard returns early when already on this bus. With the guard
    inverted, the second install re-subscribes a second closure, so one
    published combat event folds twice. We assert a single fold.
    """
    reducer = TrackingReducer()
    bus = EventBus()
    reducer.install(bus)
    reducer.install(bus)  # idempotent: no second subscription

    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 1.0})
    assert reducer.state["shots_fired_total"] == 1


def test_install_records_bus_so_relocation_drops_old_subscriptions() -> None:
    """``install`` stores the new bus; relocating drops the old bus.

    Kills ``install__mutmut_3`` (``self._bus = bus`` -> ``= None``):
    after a successful install the reducer must remember its bus, so a
    follow-up install on the SAME bus is a no-op (idempotent) rather
    than re-subscribing. With ``_bus`` left None, the second install
    would re-subscribe and a single event would fold twice.
    """
    reducer = TrackingReducer()
    bus_a = EventBus()
    bus_b = EventBus()

    reducer.install(bus_a)
    assert reducer._bus is bus_a

    # Relocate to bus_b: the old bus_a subscription must be dropped.
    reducer.install(bus_b)
    assert reducer._bus is bus_b

    # Idempotent re-install on bus_b must not double-subscribe.
    reducer.install(bus_b)
    bus_b.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 1.0})
    assert reducer.state["shots_fired_total"] == 1


def test_uninstall_resets_bus_to_none() -> None:
    """``uninstall`` clears ``_bus`` to exactly ``None`` (not ``""``).

    Kills ``uninstall__mutmut_1`` (``self._bus = None`` -> ``= ""``).
    ``uninstall`` is a documented no-op stub on the bus side (it does
    not drop subscriptions), so its only contract is the ``_bus``
    identity it leaves behind: ``None``, the same pristine sentinel
    ``__init__`` sets and that ``install``'s ``is None`` first-install
    branch keys off. The ``""`` mutant leaves a falsy-but-not-None
    value, which the identity assertion below rejects.
    """
    reducer = TrackingReducer()
    bus = EventBus()
    reducer.install(bus)
    assert reducer._bus is bus
    reducer.uninstall()
    assert reducer._bus is None


# ── TrackingReducer.initial_state ────────────────────────────────────────────


def test_tracking_reducer_initial_state_exact_shape() -> None:
    """A fresh ``TrackingReducer`` projects the exact idle shape.

    Kills ``initial_state__mutmut_1..4`` (``status`` key rename /
    ``idle`` value case-mangling): an exact-dict comparison rejects any
    altered key name or value.
    """
    reducer = TrackingReducer()
    assert reducer.state == _IDLE_STATE


# ── TrackingReducer.on_event dispatch ────────────────────────────────────────


def test_on_event_session_stopped_flips_status_to_idle() -> None:
    """``on_event(EVENT_SESSION_STOPPED, payload)`` flips status to idle.

    Kills ``on_event__mutmut_4`` (``_on_session_stopped(payload)`` ->
    ``_on_session_stopped(None)``). ``_on_session_stopped`` discards its
    payload, so substituting ``None`` is behaviourally identical UNLESS
    the dispatch wiring is broken -- but the mutant only changes the
    argument, which the callee ignores. We still pin the observable
    transition (active -> idle) so the dispatch arm is exercised; the
    None-substitution itself is recorded as equivalent below.
    """
    reducer = TrackingReducer()
    reducer.on_event(EVENT_SESSION_STARTED, {"session_id": "s1"})
    assert reducer.state["status"] == "active"
    reducer.on_event(EVENT_SESSION_STOPPED, {"session_id": "s1"})
    assert reducer.state["status"] == "idle"


# ── TrackingReducer._on_combat ───────────────────────────────────────────────


def test_on_combat_amount_default_is_zero_not_one() -> None:
    """A damage event with no ``amount`` contributes zero damage.

    Kills ``_on_combat__mutmut_12`` (``payload.get("amount") or 0.0``
    -> ``or 1.0``): a ``damage_dealt`` event whose amount is missing /
    falsy must add 0.0 to the damage total, not 1.0.
    """
    reducer = TrackingReducer()
    reducer.on_event(EVENT_COMBAT, {"type": "damage_dealt"})
    assert reducer.state["damage_dealt_total"] == 0.0
    assert reducer.state["shots_fired_total"] == 1


def test_on_combat_damage_rounds_to_four_decimals() -> None:
    """Damage accumulation rounds to 4 decimals, not 5.

    Kills ``_on_combat__mutmut_33`` (``round(_, 4)`` -> ``round(_, 5)``):
    an amount with a non-zero 5th decimal rounds differently at 4 vs 5.
    """
    reducer = TrackingReducer()
    reducer.on_event(EVENT_COMBAT, {"type": "damage_dealt", "amount": 0.123456})
    assert reducer.state["damage_dealt_total"] == 0.1235


def test_on_combat_critical_hits_accumulate() -> None:
    """Each critical hit increments the crit total (``+= 1``, not ``= 1``).

    Kills ``_on_combat__mutmut_37`` (``critical_hits_total += 1`` ->
    ``= 1``): two crits must yield a total of 2.
    """
    reducer = TrackingReducer()
    reducer.on_event(EVENT_COMBAT, {"type": "critical_hit", "amount": 1.0})
    reducer.on_event(EVENT_COMBAT, {"type": "critical_hit", "amount": 1.0})
    assert reducer.state["critical_hits_total"] == 2
    assert reducer.state["shots_fired_total"] == 2


def test_on_combat_defensive_types_count_as_shots() -> None:
    """Each defensive combat type counts as exactly one shot.

    Kills ``_on_combat__mutmut_42`` (``in`` -> ``not in`` on the
    defensive-type tuple): with the membership flipped a damage event
    would be (wrongly) counted as a defensive shot too, and the
    defensive events would not. We assert each of dodge / evade / jam
    advances shots, and a non-defensive damage event does NOT advance
    shots a second time.
    """
    for kind in ("target_dodge", "target_evade", "target_jam"):
        reducer = TrackingReducer()
        reducer.on_event(EVENT_COMBAT, {"type": kind})
        assert reducer.state["shots_fired_total"] == 1
        assert reducer.state["damage_dealt_total"] == 0.0
        assert reducer.state["critical_hits_total"] == 0

    # A damage event advances shots exactly once (it is NOT in the
    # defensive tuple, so the second `if` must not fire for it).
    reducer = TrackingReducer()
    reducer.on_event(EVENT_COMBAT, {"type": "damage_dealt", "amount": 1.0})
    assert reducer.state["shots_fired_total"] == 1


def test_on_combat_target_jam_is_exact_string() -> None:
    """``target_jam`` (exact, lowercase) is the recognised defensive type.

    Kills ``_on_combat__mutmut_47`` (``"target_jam"`` ->
    ``"XXtarget_jamXX"``) and ``_on_combat__mutmut_48``
    (``"target_jam"`` -> ``"TARGET_JAM"``): the literal must match the
    real chatlog type string exactly, so a ``target_jam`` event counts
    one shot while a mangled-case ``TARGET_JAM`` does not.
    """
    reducer = TrackingReducer()
    reducer.on_event(EVENT_COMBAT, {"type": "target_jam"})
    assert reducer.state["shots_fired_total"] == 1

    # The mutated literals would only match the mangled strings; a real
    # `target_jam` event would then count zero shots.
    other = TrackingReducer()
    other.on_event(EVENT_COMBAT, {"type": "TARGET_JAM"})
    assert other.state["shots_fired_total"] == 0


# ── TrackingReducer._on_loot_group ───────────────────────────────────────────


def test_on_loot_group_kill_count_accumulates() -> None:
    """Each loot group increments the kill count (``+= 1``, not ``= 1``).

    Kills ``_on_loot_group__mutmut_2`` (``kill_count += 1`` -> ``= 1``):
    three loot groups must yield a kill count of 3.
    """
    reducer = TrackingReducer()
    for _ in range(3):
        reducer.on_event(EVENT_LOOT_GROUP, {"items": []})
    assert reducer.state["kill_count"] == 3


def test_on_loot_group_skips_non_dict_items_via_continue() -> None:
    """Non-dict items are skipped (``continue``), later items still add.

    Kills ``_on_loot_group__mutmut_15`` (``continue`` -> ``break``): a
    non-dict item appearing BEFORE a valid item must not abort the loop;
    the trailing valid item's value must still be folded into returns.
    """
    reducer = TrackingReducer()
    reducer.on_event(
        EVENT_LOOT_GROUP,
        {"items": ["not-a-dict", {"value_ped": 7.0}]},
    )
    assert reducer.state["returns"] == 7.0
    assert reducer.state["kill_count"] == 1


def test_on_loot_group_value_default_is_zero_not_one() -> None:
    """An item with no ``value_ped`` contributes zero to returns.

    Kills ``_on_loot_group__mutmut_23`` (``item.get("value_ped") or
    0.0`` -> ``or 1.0``): a valueless item must add 0.0, not 1.0.
    """
    reducer = TrackingReducer()
    reducer.on_event(EVENT_LOOT_GROUP, {"items": [{"item_name": "x"}]})
    assert reducer.state["returns"] == 0.0
    assert reducer.state["kill_count"] == 1


def test_on_loot_group_returns_round_to_four_decimals() -> None:
    """Returns accumulation rounds to 4 decimals, not 5.

    Kills ``_on_loot_group__mutmut_34`` (``round(_, 4)`` ->
    ``round(_, 5)``): an item value with a non-zero 5th decimal rounds
    differently at 4 vs 5.
    """
    reducer = TrackingReducer()
    reducer.on_event(EVENT_LOOT_GROUP, {"items": [{"value_ped": 0.123456}]})
    assert reducer.state["returns"] == 0.1235


# ── QuestsReducer.on_event dispatch ──────────────────────────────────────────


def test_quests_on_event_session_stopped_clears_session_id() -> None:
    """``on_event(EVENT_SESSION_STOPPED, payload)`` clears the session id.

    Kills ``QuestsReducerǁon_event__mutmut_4`` (the SESSION_STOPPED arm
    passing ``None`` instead of ``payload``). ``_on_session_stopped``
    discards its payload, so this arm's transition (session id ->
    None, mission log preserved) is what we pin; the None-substitution
    itself is recorded as equivalent.
    """
    reducer = QuestsReducer()
    reducer.on_event(EVENT_SESSION_STARTED, {"session_id": "s1"})
    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": "A"}
    )
    reducer.on_event(EVENT_SESSION_STOPPED, {"session_id": "s1"})
    assert reducer.state["session_id"] is None
    assert reducer.state["mission_names_received"] == ["A"]


# ── tracking_view_state ──────────────────────────────────────────────────────


def _fresh_tracker() -> tuple[HuntTracker, sqlite3.Connection, EventBus]:
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db)
    return tracker, db, bus


def test_view_idle_state_exact_shape() -> None:
    """An untracked tracker yields the exact idle projection.

    Kills the idle-branch key/value mutants
    ``tracking_view_state__mutmut_5..25`` (key renames such as
    ``XXstatusXX`` / ``STATUS``, value mangles ``idle`` -> ``IDLE`` /
    ``XXidleXX``, and int/float default bumps 0 -> 1 / 0.0 -> 1.0).

    Also kills ``mutmut_3`` (``not tracker.is_tracking`` ->
    ``tracker.is_tracking``) and ``mutmut_4`` (``session is None`` ->
    ``session is not None``): both flip the idle guard to False for an
    untracked tracker, so the function falls through to the
    session-None RuntimeError instead of returning the idle dict.
    """
    tracker, db, _bus = _fresh_tracker()
    try:
        assert not tracker.is_tracking
        assert tracker.session is None
        view = tracking_view_state(TrackingViewContext(tracker=tracker))
        assert view == _IDLE_STATE
    finally:
        db.close()


def test_view_active_in_flight_combat_included_exact_shape() -> None:
    """An active session with in-flight (un-looted) combat is projected.

    Drives a real tracker: start, then publish combat with no loot
    group, so the stats live only in ``current_accumulator``.

    Kills:
    - ``mutmut_35`` (``accumulator = tracker.current_accumulator`` ->
      ``None``) and ``mutmut_42`` (``accumulator is not None`` ->
      ``is None``): both drop the in-flight totals, leaving the view
      reporting zero shots / damage / crits.
    - the active-branch key/value mutants ``mutmut_51..73`` (key
      renames, ``active`` / ``idle`` value mangles, ``session_id`` key
      rename) via the exact-dict comparison.
    - ``mutmut_66/67/68`` (round-arg broken) and ``mutmut_69``
      (``round(_, 4)`` -> ``round(_, 5)``) on damage via a 5th-decimal
      amount.
    """
    tracker, db, bus = _fresh_tracker()
    try:
        session = tracker.start_session()
        bus.publish(EVENT_COMBAT, {"type": "critical_hit", "amount": 0.123456})
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 1.0})

        acc = tracker.current_accumulator
        assert acc is not None
        assert acc.shots_fired == 2
        assert acc.critical_hits == 1

        view = tracking_view_state(TrackingViewContext(tracker=tracker))
        assert view == {
            "status": "active",
            "session_id": session.id,
            "kill_count": 0,
            "shots_fired_total": 2,
            "damage_dealt_total": round(0.123456 + 1.0, 4),
            "critical_hits_total": 1,
            "returns": 0.0,
        }
        # The damage total must round to 4 places (1.1235), not 5
        # (1.12346): pins mutmut_69.
        assert view["damage_dealt_total"] == 1.1235
    finally:
        db.close()


def test_view_accumulator_adds_to_closed_kill_totals() -> None:
    """In-flight accumulator stats ADD to the closed-kill totals.

    Closes one kill (combat + loot group), then publishes more combat
    that stays in the accumulator. The view must report the SUM of the
    closed kill's stats and the in-flight accumulator's stats.

    Kills:
    - ``mutmut_43`` (``damage_total += accumulator.damage_dealt`` ->
      ``=``) and ``mutmut_44`` (-> ``-=``): replacing/subtracting would
      drop or negate the closed-kill damage.
    - ``mutmut_47`` (``crits_total += ...`` -> ``=``) and ``mutmut_48``
      (-> ``-=``): same for the critical-hit total.
    """
    tracker, db, bus = _fresh_tracker()
    try:
        tracker.start_session()
        # Close kill 1: 2 crits worth 3.0 + 4.0 damage, then a loot tick.
        bus.publish(EVENT_COMBAT, {"type": "critical_hit", "amount": 3.0})
        bus.publish(EVENT_COMBAT, {"type": "critical_hit", "amount": 4.0})
        bus.publish(
            EVENT_LOOT_GROUP,
            {"items": [{"item_name": "Drop 0", "value_ped": 1.0}]},
        )
        # In-flight (not yet looted): one more crit worth 5.0 damage.
        bus.publish(EVENT_COMBAT, {"type": "critical_hit", "amount": 5.0})

        assert tracker.session is not None
        assert len(tracker.session.kills) == 1
        kill = tracker.session.kills[0]
        acc = tracker.current_accumulator
        assert acc is not None

        view = tracking_view_state(TrackingViewContext(tracker=tracker))
        # Damage = closed kill + accumulator (the += contract).
        assert view["damage_dealt_total"] == round(
            kill.damage_dealt + acc.damage_dealt, 4
        )
        assert view["damage_dealt_total"] == round(3.0 + 4.0 + 5.0, 4)
        # Crits = closed kill + accumulator.
        assert view["critical_hits_total"] == kill.critical_hits + acc.critical_hits
        assert view["critical_hits_total"] == 3
        assert view["kill_count"] == 1
    finally:
        db.close()


def test_view_returns_is_a_rounded_float_not_an_int() -> None:
    """The view's ``returns`` is ``round(returns, 4)`` -- a fractional float.

    Drives a single closed kill whose loot value is fractional (1.5
    PED), so the projected ``returns`` is 1.5.

    Kills ``tracking_view_state__mutmut_75`` (``round(returns, 4)`` ->
    ``round(returns, None)``) and ``mutmut_77`` (-> ``round(returns,)``,
    i.e. ``round(returns)``): both drop the ndigits argument, so
    ``round`` returns the integer nearest 1.5 (== 2) rather than 1.5.
    An idle/zero returns would mask this (``round(0.0) == 0.0`` holds),
    so a non-integer value is required to surface the truncation.
    """
    tracker, db, bus = _fresh_tracker()
    try:
        tracker.start_session()
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 1.0})
        bus.publish(
            EVENT_LOOT_GROUP,
            {"items": [{"item_name": "Shiny", "value_ped": 1.5}]},
        )
        assert tracker.session is not None
        assert len(tracker.session.kills) == 1

        view = tracking_view_state(TrackingViewContext(tracker=tracker))
        assert view["returns"] == 1.5
        # round(1.5) == 2 (int); the float 1.5 must survive intact.
        assert view["returns"] != 2
    finally:
        db.close()
