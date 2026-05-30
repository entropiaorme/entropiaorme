"""Acceptance test for the quests-surface consistency property.

Drives the ``consistency_quests_mission_lifecycle_midpoint`` scenario
through a pipeline with ``QuestService`` wired into the bus. The pre-
segment fires one ``mission_received`` event, auto-starting the
matching pre-populated quest; the harness snapshots the quests view
at that midpoint (T0), installs a ``QuestsReducer`` hydrated with the
T0 snapshot, then the post-segment fires a second ``mission_received``
event. The reducer's hydrated-and-folded state must equal a freshly
composed T1 snapshot.

The property under test (the one a future event-driven hydration
model will rely on for quests): a hydrating client that fetches the
quests view once and then follows ``mission_received`` events on the
bus reproduces the auto-start log a fresh re-fetch would return.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from backend.core.event_bus import EventBus
from backend.db.app_database import AppDatabase
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.services.quest_service import QuestService
from backend.testing.consistency import ConsistencyHarness, SurfaceAdapter
from backend.testing.store_reducers import (
    QuestsReducer,
    QuestsViewContext,
    quests_view_state,
)
from backend.tracking.tracker import HuntTracker


@pytest.fixture
def quests_consistency_pipeline(
    tmp_path: Path,
) -> Iterator[
    tuple[EventBus, HuntTracker, QuestService, ChatlogWatcher, Path, AppDatabase]
]:
    """Boot the quests-consistency pipeline with QuestService bus-wired."""
    chatlog_path = tmp_path / "chat_testing.log"
    chatlog_path.touch()
    app_db = AppDatabase(tmp_path / "test.db")
    bus = EventBus()
    quest_service = QuestService(app_db, event_bus=bus)
    tracker = HuntTracker(bus, app_db.conn)
    watcher = ChatlogWatcher(bus, chatlog_path)
    watcher.start()
    try:
        yield bus, tracker, quest_service, watcher, chatlog_path, app_db
    finally:
        watcher.stop()
        app_db.close()


def test_quests_snapshot_event_stream_consistency(
    quests_consistency_pipeline,
    corpus_root: Path,
    data_regression,
) -> None:
    """Hydrate from T0 + apply post-midpoint mission_received == fresh T1 snapshot."""

    bus, tracker, quest_service, watcher, chatlog, _app_db = quests_consistency_pipeline
    scenario_dir = (
        corpus_root / "scripted" / "consistency_quests_mission_lifecycle_midpoint"
    )

    quest_service.create_quest({"name": "Alpha Hunt"})
    quest_service.create_quest({"name": "Beta Hunt"})

    session = tracker.start_session()
    session_id = session.id
    try:
        harness = ConsistencyHarness(bus=bus, chatlog_path=chatlog, watcher=watcher)
        adapter = SurfaceAdapter(
            name="quests",
            view_fn=quests_view_state,
            reducer_factory=QuestsReducer,
        )
        result = harness.run(
            scenario_dir=scenario_dir,
            adapter=adapter,
            view_context=QuestsViewContext(
                quest_service=quest_service,
                conn=quest_service._conn,
                session_id=session_id,
            ),
        )
    finally:
        if tracker.is_tracking:
            tracker.stop_session()

    assert result.holds, (
        "Quests-surface consistency property failed; the following "
        f"keys diverged: {result.divergence}. "
        f"hydrated_state={result.hydrated_state!r} "
        f"snapshot_t1={result.snapshot_t1!r}"
    )

    # Guard against a vacuous T0 == T1 run trivially satisfying the
    # property assertion above if the scenario is later edited.
    assert result.snapshot_t0["mission_names_received"] == ["Alpha Hunt"]
    assert result.snapshot_t1["mission_names_received"] == ["Alpha Hunt", "Beta Hunt"]

    data_regression.check(_normalise(result.hydrated_state))


def _normalise(state: dict) -> dict:
    """Drop the volatile ``session_id`` so the golden stays stable."""
    sanitised = dict(state)
    sanitised.pop("session_id", None)
    return sanitised
