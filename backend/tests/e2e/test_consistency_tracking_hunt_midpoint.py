"""Acceptance test for the tracking-surface consistency property.

Drives the ``consistency_tracking_hunt_midpoint`` scenario through the
real chat-replay pipeline with the ``ConsistencyHarness``. Pre-segment
events resolve one kill; the harness snapshots the tracking surface at
that midpoint (T0), then a fresh ``TrackingReducer`` is installed on
the bus and hydrated with the T0 snapshot. The post-segment events
fold into the reducer (one more kill, a critical, more shots, one more
loot tick); at the end the reducer's state is compared to a freshly
composed T1 snapshot.

The property under test (the one a future event-driven hydration
model will rely on): a hydrating client that fetches a snapshot once
and then follows the bus reproduces the state a fresh re-fetch would
return. ``data_regression``
pins the post-T1 state (the reducer's hydrated-and-folded view, which
equals the fresh snapshot) so a future change that shifts the
projection surfaces as a golden diff for review before ratification.
"""

from __future__ import annotations

from pathlib import Path

from backend.core.event_bus import EventBus
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.consistency import ConsistencyHarness, SurfaceAdapter
from backend.testing.store_reducers import (
    TrackingReducer,
    TrackingViewContext,
    tracking_view_state,
)
from backend.tracking.tracker import HuntTracker


def test_tracking_snapshot_event_stream_consistency(
    e2e_pipeline: tuple[EventBus, HuntTracker, ChatlogWatcher, Path],
    corpus_root: Path,
    data_regression,
) -> None:
    """Hydrate from T0 + apply post-midpoint events == fresh T1 snapshot."""

    bus, tracker, _watcher, chatlog = e2e_pipeline
    scenario_dir = corpus_root / "scripted" / "consistency_tracking_hunt_midpoint"

    tracker.start_session()
    try:
        harness = ConsistencyHarness(bus=bus, chatlog_path=chatlog)
        adapter = SurfaceAdapter(
            name="tracking",
            view_fn=tracking_view_state,
            reducer_factory=TrackingReducer,
        )
        result = harness.run(
            scenario_dir=scenario_dir,
            adapter=adapter,
            view_context=TrackingViewContext(tracker=tracker),
        )
    finally:
        # ``stop_session`` is idempotent on an already-stopped session,
        # so the teardown is safe whether or not the property held.
        if tracker.is_tracking:
            tracker.stop_session()

    assert result.holds, (
        "Tracking-surface consistency property failed; the following "
        f"keys diverged between the hydrated-and-folded reducer state "
        f"and the fresh T1 snapshot: {result.divergence}. "
        f"hydrated_state={result.hydrated_state!r} "
        f"snapshot_t1={result.snapshot_t1!r}"
    )

    # Guard against a vacuous T0 == T1 run trivially satisfying the
    # property assertion above if the scenario is later edited.
    assert result.snapshot_t0["kill_count"] == 1
    assert result.snapshot_t1["kill_count"] == 2
    assert (
        result.snapshot_t1["shots_fired_total"]
        > result.snapshot_t0["shots_fired_total"]
    )

    data_regression.check(_normalise(result.hydrated_state))


def _normalise(state: dict) -> dict:
    """Drop the volatile ``session_id`` so the golden stays stable.

    The session UUID changes per run; every other key is determined by
    the scenario's chat lines plus the reducer's projection, both of
    which are fixed by version control. Dropping the id here keeps the
    golden a property assertion rather than a per-run record.
    """
    sanitised = dict(state)
    sanitised.pop("session_id", None)
    return sanitised
