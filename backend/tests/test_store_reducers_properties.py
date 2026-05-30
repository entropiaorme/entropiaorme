"""Property-based tests for the hydration-state reducers.

Covers ``backend.testing.store_reducers``: the per-event-type folding
contract of ``TrackingReducer``, the key-set discipline of the base
``Reducer.hydrate``, the defensive-copy contract of ``Reducer.state`` on
the tracker projection, and the structural relations the live tracking
view (``tracking_view_state``) holds for any hunt sequence.

These are construction properties of the reducer surface itself, not the
end-to-end snapshot/event-stream consistency property (which lives in
``test_consistency_property``). The reducers are pure folds over bus
payloads, so most properties drive the reducer directly via ``on_event``;
the view relation drives a real ``HuntTracker`` over an in-memory database
and publishes the bus events the tracker subscribes to, then reads the
composed view.
"""

from __future__ import annotations

import sqlite3

from hypothesis import given
from hypothesis import strategies as st

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_LOOT_GROUP,
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

# Combat subtypes the reducer distinguishes: the two damage-bearing kinds
# advance damage, the defensive kinds advance shots only. ``mob_miss`` and
# ``damage_received`` ride EVENT_COMBAT in production but the reducer folds
# neither into any total, so including them strengthens the damage-source
# claim.
_DAMAGE_BEARING = ("damage_dealt", "critical_hit")
_DEFENSIVE = ("target_dodge", "target_evade", "target_jam")
_NON_DAMAGE = _DEFENSIVE + ("damage_received", "mob_miss", "self_heal")
_COMBAT_KINDS = _DAMAGE_BEARING + _NON_DAMAGE

# Two-decimal-clean positive amounts so the reducer's round(_, 4)
# accumulation has no float-tail the comparison must absorb.
_AMOUNTS = st.integers(min_value=1, max_value=500000).map(lambda cents: cents / 100.0)


@st.composite
def _combat_events(draw: st.DrawFn) -> list[dict]:
    """A sequence of combat-event payloads in the reducer's payload shape."""
    count = draw(st.integers(min_value=0, max_value=12))
    events: list[dict] = []
    for _ in range(count):
        kind = draw(st.sampled_from(_COMBAT_KINDS))
        amount = draw(_AMOUNTS)
        events.append({"type": kind, "amount": amount})
    return events


# --- damage only advances on damage-bearing types ---


@given(events=_combat_events())
def test_damage_total_equals_sum_of_damage_bearing_amounts(events):
    """``damage_dealt_total`` advances only on damage-bearing combat types.

    Folding any mix of combat payloads leaves the damage total equal to
    the sum of the amounts carried by ``damage_dealt`` / ``critical_hit``
    events alone; every defensive or incoming type contributes zero.
    """
    reducer = TrackingReducer()
    for payload in events:
        reducer.on_event(EVENT_COMBAT, payload)

    expected = round(
        sum(e["amount"] for e in events if e["type"] in _DAMAGE_BEARING), 4
    )
    assert reducer.state["damage_dealt_total"] == expected


@given(events=_combat_events())
def test_non_damage_combat_leaves_damage_total_zero(events):
    """A sequence with no damage-bearing types never moves the damage total."""
    non_damage = [e for e in events if e["type"] in _NON_DAMAGE]
    reducer = TrackingReducer()
    for payload in non_damage:
        reducer.on_event(EVENT_COMBAT, payload)
    assert reducer.state["damage_dealt_total"] == 0.0


# --- base hydrate keeps exactly initial_state()'s key set ---

# Field names the tracking and quests reducers project, plus stray names a
# snapshot might carry that the reducer does not. The snapshot strategy
# mixes both so the "snapshot-only keys are dropped" half is exercised.
_KNOWN_KEYS = (
    "status",
    "session_id",
    "kill_count",
    "shots_fired_total",
    "damage_dealt_total",
    "critical_hits_total",
    "returns",
    "mission_names_received",
)
_STRAY_KEYS = ("weapon_cost", "enhancer_cost", "is_global", "skill_points", "extra")


@st.composite
def _snapshots(draw: st.DrawFn) -> dict:
    """An arbitrary snapshot dict mixing known and stray keys."""
    keys = draw(
        st.lists(
            st.sampled_from(_KNOWN_KEYS + _STRAY_KEYS),
            unique=True,
            max_size=len(_KNOWN_KEYS) + len(_STRAY_KEYS),
        )
    )
    values = st.one_of(
        st.integers(),
        st.floats(allow_nan=False, allow_infinity=False),
        st.text(max_size=8),
        st.none(),
        st.lists(st.text(max_size=8), max_size=4),
    )
    return {key: draw(values) for key in keys}


@given(snapshot=_snapshots())
def test_tracking_hydrate_key_set_is_exactly_initial_state(snapshot):
    """``TrackingReducer.hydrate`` yields exactly ``initial_state``'s keys.

    No snapshot can add a key the reducer does not project, and no
    initial key is dropped: keys absent from the snapshot keep their
    defaults, snapshot-only keys never appear.
    """
    reducer = TrackingReducer()
    reducer.hydrate(snapshot)

    initial_keys = set(reducer.initial_state())
    assert set(reducer.state) == initial_keys
    # Known keys present in the snapshot are adopted; absent ones keep the
    # default. Stray keys never appear.
    for key in initial_keys:
        if key in snapshot:
            assert reducer.state[key] == snapshot[key]
    for stray in _STRAY_KEYS:
        assert stray not in reducer.state


@given(snapshot=_snapshots())
def test_quests_hydrate_key_set_is_exactly_initial_state(snapshot):
    """The same key-set discipline holds for ``QuestsReducer.hydrate``."""
    reducer = QuestsReducer()
    reducer.hydrate(snapshot)
    assert set(reducer.state) == set(reducer.initial_state())


# --- tracker reducer .state is a defensive copy ---


@st.composite
def _tracking_event_stream(draw: st.DrawFn) -> list[tuple[str, dict]]:
    """A lifecycle-bracketed stream of (topic, payload) for the tracker reducer."""
    stream: list[tuple[str, dict]] = [
        (EVENT_SESSION_STARTED, {"session_id": draw(st.uuids()).hex})
    ]
    for _ in range(draw(st.integers(min_value=0, max_value=8))):
        choice = draw(st.integers(min_value=0, max_value=2))
        if choice == 0:
            stream.append(
                (
                    EVENT_COMBAT,
                    {
                        "type": draw(st.sampled_from(_COMBAT_KINDS)),
                        "amount": draw(_AMOUNTS),
                    },
                )
            )
        elif choice == 1:
            stream.append(
                (EVENT_LOOT_GROUP, {"items": [{"value_ped": draw(_AMOUNTS)}]})
            )
        else:
            stream.append((EVENT_SESSION_STOPPED, {}))
    return stream


@given(stream=_tracking_event_stream())
def test_state_is_defensive_copy_on_tracker_projection(stream):
    """Mutating the dict returned by ``.state`` never reaches ``_state``.

    The tracker projection holds only scalars, so the shallow copy
    ``Reducer.state`` returns is a complete defensive copy: reassigning,
    popping, or adding keys on the returned dict leaves a subsequent
    ``.state`` read identical to the first.
    """
    reducer = TrackingReducer()
    for topic, payload in stream:
        reducer.on_event(topic, payload)

    before = reducer.state
    leaked = reducer.state
    leaked["kill_count"] = 999_999
    leaked["damage_dealt_total"] = -1.0
    leaked["injected_key"] = "noise"
    leaked.pop("status", None)

    assert reducer.state == before


# --- view: crits never exceed shots, kill_count equals resolved kills ---


@st.composite
def _view_drive(draw: st.DrawFn) -> tuple[list[dict], int]:
    """A combat-event list plus a loot-group count that closes that many kills.

    Each loot group is given a distinct ``item_name`` so the tracker's
    same-fingerprint dedup window never collapses two loot groups into
    one, keeping the resolved kill count equal to the number of loot
    groups published.
    """
    combat = [
        {"type": draw(st.sampled_from(_COMBAT_KINDS)), "amount": draw(_AMOUNTS)}
        for _ in range(draw(st.integers(min_value=0, max_value=10)))
    ]
    loot_count = draw(st.integers(min_value=0, max_value=5))
    return combat, loot_count


@given(drive=_view_drive())
def test_view_shots_ge_crits_and_kill_count_equals_session_kills(drive):
    """``tracking_view_state`` holds crits <= shots and kills == len(kills).

    Every crit the tracker records carries its shot, so the view's
    critical-hit total can never exceed its shot total. The view's
    ``kill_count`` is, by definition, the length of the in-memory
    session kill list; the distinct-item loot groups also resolve one
    kill apiece, so the published count tracks it.
    """
    combat, loot_count = drive
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db)
    try:
        tracker.start_session()
        for payload in combat:
            bus.publish(EVENT_COMBAT, payload)
        for index in range(loot_count):
            # Distinct item names keep loot fingerprints apart so the
            # 2-second dedup window never collapses two loot groups.
            bus.publish(
                EVENT_LOOT_GROUP,
                {"items": [{"item_name": f"Drop {index}", "value_ped": 1.0}]},
            )

        view = tracking_view_state(TrackingViewContext(tracker=tracker))
        assert view["critical_hits_total"] <= view["shots_fired_total"]
        assert tracker.session is not None
        assert view["kill_count"] == len(tracker.session.kills)
        assert view["kill_count"] == loot_count
        assert view["shots_fired_total"] >= 0
        assert view["critical_hits_total"] >= 0
    finally:
        db.close()
