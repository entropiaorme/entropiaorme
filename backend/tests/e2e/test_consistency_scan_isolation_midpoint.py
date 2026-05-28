"""Forward-positioning consistency test for the scan surface.

Skill scans complete via a ``SkillScanManual`` callback rather than a
bus event in the current backend; the snapshot view reads the
``skill_calibrations`` table the callback wrote into. A genuine
event-stream-driven property test for scan therefore waits on the
matching bus contract a future change will introduce.

Until then, this test pins the apparatus's shape for the scan
surface: the ``ConsistencyHarness`` admits it, the ``ScanReducer``
slots into the ``SurfaceAdapter`` plumbing without modification, and
the isolation invariant ("a chat-driven event stream does not move
the scan view's calibration counts") is verified end-to-end. When
the bus contract for scan lands, ``ScanReducer.topics`` and
``on_event`` extend in place and this scenario stops being purely
forward-positioning.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from backend.core.event_bus import EventBus
from backend.db.app_database import AppDatabase
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.consistency import ConsistencyHarness, SurfaceAdapter
from backend.testing.store_reducers import (
    ScanReducer,
    ScanViewContext,
    scan_view_state,
)
from backend.tracking.tracker import HuntTracker


@pytest.fixture
def scan_consistency_pipeline(
    tmp_path: Path,
) -> Iterator[tuple[EventBus, HuntTracker, ChatlogWatcher, Path, AppDatabase]]:
    """Boot a pipeline backed by an ``AppDatabase`` so the scan view's
    ``skill_calibrations`` table exists for the snapshot query."""
    chatlog_path = tmp_path / "chat_testing.log"
    chatlog_path.touch()
    app_db = AppDatabase(tmp_path / "test.db")
    bus = EventBus()
    tracker = HuntTracker(bus, app_db.conn)
    watcher = ChatlogWatcher(bus, chatlog_path)
    watcher.start()
    try:
        yield bus, tracker, watcher, chatlog_path, app_db
    finally:
        watcher.stop()
        app_db.close()


def test_scan_isolation_invariant_holds_across_chat_event_stream(
    scan_consistency_pipeline,
    corpus_root: Path,
    data_regression,
) -> None:
    """Chat events leave the scan view's projection unchanged."""

    bus, tracker, watcher, chatlog, app_db = scan_consistency_pipeline
    scenario_dir = corpus_root / "scripted" / "consistency_scan_isolation_midpoint"

    tracker.start_session()
    try:
        harness = ConsistencyHarness(bus=bus, chatlog_path=chatlog, watcher=watcher)
        adapter = SurfaceAdapter(
            name="scan",
            view_fn=scan_view_state,
            reducer_factory=ScanReducer,
        )
        result = harness.run(
            scenario_dir=scenario_dir,
            adapter=adapter,
            view_context=ScanViewContext(conn=app_db.conn),
        )
    finally:
        if tracker.is_tracking:
            tracker.stop_session()

    assert result.holds, (
        "Scan isolation invariant failed; the chat event stream "
        f"contaminated the scan view's projection: {result.divergence}. "
        f"hydrated_state={result.hydrated_state!r} "
        f"snapshot_t1={result.snapshot_t1!r}"
    )

    # Both snapshots project zero rows since neither segment touches
    # the scan tables; the invariant under test is the equality across
    # T0 and T1, not the absolute value.
    assert result.snapshot_t0 == result.snapshot_t1

    data_regression.check(result.hydrated_state)
