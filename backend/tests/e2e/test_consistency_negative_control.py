"""Negative-control test for the consistency harness.

The main consistency test pins that a faithful reducer reproduces the
fresh-snapshot state from a hydrated T0 plus the post-midpoint event
stream. A passing assertion in that test is only meaningful if a
faulty reducer would have failed the same assertion: otherwise a
silent regression that broke the reducer would still see the test
pass (a vacuously-true property).

This control flips the proof. It wires a deliberately-broken reducer
(``_DropLootReducer`` ignores every ``loot_group`` event) onto the
same scenario and asserts the harness reports a non-empty divergence
list against the fresh T1 snapshot. The control is a regression test
for the harness itself: if a future refactor of ``ConsistencyHarness``
or the reducer protocol weakens the comparison so the property no
longer surfaces a real divergence, this control fires.
"""

from __future__ import annotations

from collections.abc import Iterable
from pathlib import Path
from typing import Any

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_COMBAT, EVENT_SESSION_STARTED
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.consistency import ConsistencyHarness, SurfaceAdapter
from backend.testing.store_reducers import (
    Reducer,
    TrackingViewContext,
    tracking_view_state,
)
from backend.tracking.tracker import HuntTracker


class _DropLootReducer(Reducer):
    """Reducer that ignores loot_group events (induced regression).

    Subscribes only to combat and session lifecycle topics, so the
    ``kill_count`` and ``returns`` fields stay at their initial-state
    values after hydration: a faithful T1 snapshot will diverge on at
    least those two keys, plus any field a loot tick would have
    advanced. Used by the negative-control test below to prove the
    consistency property catches a real regression.
    """

    def topics(self) -> Iterable[str]:
        """Deliberately omit ``EVENT_LOOT_GROUP`` to induce divergence."""
        return (EVENT_SESSION_STARTED, EVENT_COMBAT)

    def initial_state(self) -> dict[str, Any]:
        """Same shape as ``TrackingReducer`` so the diff is field-level."""
        return {
            "status": "idle",
            "session_id": None,
            "kill_count": 0,
            "shots_fired_total": 0,
            "damage_dealt_total": 0.0,
            "critical_hits_total": 0,
            "returns": 0.0,
        }

    def on_event(self, topic: str, payload: Any) -> None:
        """Only count shots; ignore everything else loot would advance."""
        if topic == EVENT_SESSION_STARTED and isinstance(payload, dict):
            sid = payload.get("session_id")
            if sid:
                self._state["session_id"] = sid
            self._state["status"] = "active"
        elif topic == EVENT_COMBAT and isinstance(payload, dict):
            combat_type = payload.get("type")
            if combat_type in ("damage_dealt", "critical_hit"):
                self._state["shots_fired_total"] += 1


def test_consistency_property_catches_a_broken_reducer(
    e2e_pipeline: tuple[EventBus, HuntTracker, ChatlogWatcher, Path],
    corpus_root: Path,
) -> None:
    """A reducer that drops loot events fails the consistency property."""

    bus, tracker, _watcher, chatlog = e2e_pipeline
    scenario_dir = corpus_root / "scripted" / "consistency_tracking_hunt_midpoint"

    tracker.start_session()
    try:
        harness = ConsistencyHarness(bus=bus, chatlog_path=chatlog)
        adapter = SurfaceAdapter(
            name="tracking_broken",
            view_fn=tracking_view_state,
            reducer_factory=_DropLootReducer,
        )
        result = harness.run(
            scenario_dir=scenario_dir,
            adapter=adapter,
            view_context=TrackingViewContext(tracker=tracker),
        )
    finally:
        if tracker.is_tracking:
            tracker.stop_session()

    # The broken reducer ignored every loot_group event, so the fresh
    # T1 snapshot and the hydrated-and-folded state must diverge on at
    # least ``kill_count`` (the loot tick is what increments it) and
    # ``returns`` (the loot tick is what credits it).
    assert not result.holds, (
        "Negative-control reducer drops loot events but the consistency "
        "harness reported no divergence; this means the property is no "
        "longer catching reducer regressions. hydrated_state="
        f"{result.hydrated_state!r} snapshot_t1={result.snapshot_t1!r}"
    )
    assert "kill_count" in result.divergence, (
        f"Expected ``kill_count`` to diverge under the loot-dropping "
        f"reducer; full divergence list: {result.divergence}"
    )
    assert "returns" in result.divergence, (
        f"Expected ``returns`` to diverge under the loot-dropping "
        f"reducer; full divergence list: {result.divergence}"
    )
