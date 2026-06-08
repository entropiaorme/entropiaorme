"""Forward-positioning consistency test for the codex surface.

Codex claims and rank progression are HTTP-mutated via ``/codex/claim``
and ``/codex/calibrate``; no bus event flows in the current backend.
A genuine event-stream-driven property test for codex therefore waits
on the bus contract a future change will introduce.

Until then, this test pins the apparatus's shape for the codex
surface: the ``ConsistencyHarness`` admits it, the ``CodexReducer``
slots into the ``SurfaceAdapter`` plumbing without modification, and
the isolation invariant ("a chat-driven event stream does not move
the codex view's progress / claim counts") is verified end-to-end.
When the bus contract for codex lands, ``CodexReducer.topics`` and
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
from backend.testing.clock_plan import load_clock_plan
from backend.testing.consistency import ConsistencyHarness, SurfaceAdapter
from backend.testing.store_reducers import (
    CodexReducer,
    CodexViewContext,
    codex_view_state,
)
from backend.tracking.tracker import HuntTracker


@pytest.fixture
def codex_consistency_pipeline(
    tmp_path: Path,
) -> Iterator[tuple[EventBus, HuntTracker, ChatlogWatcher, Path, AppDatabase]]:
    """Boot a pipeline backed by an ``AppDatabase`` so the codex view's
    ``codex_progress`` / ``codex_claims`` tables exist for the
    snapshot query."""
    chatlog_path = tmp_path / "chat_testing.log"
    chatlog_path.touch()
    # The scenario's committed clock plan drives every timestamp this
    # pipeline stamps; the watcher stays on its real default clock per
    # the drain-timeout caveat. The harness's mid-flow snapshots need no
    # advancement: a frozen instant is deterministic, and no golden
    # observes the session boundaries here.
    clock = load_clock_plan(
        Path(__file__).parent.joinpath(
            "corpus", "scripted", "consistency_codex_isolation_midpoint"
        )
    ).build_clock()
    app_db = AppDatabase(tmp_path / "test.db")
    bus = EventBus()
    tracker = HuntTracker(bus, app_db.conn, clock=clock)
    watcher = ChatlogWatcher(bus, chatlog_path)
    watcher.start()
    try:
        yield bus, tracker, watcher, chatlog_path, app_db
    finally:
        watcher.stop()
        app_db.close()


def test_codex_isolation_invariant_holds_across_chat_event_stream(
    codex_consistency_pipeline,
    corpus_root: Path,
    data_regression,
) -> None:
    """Chat events leave the codex view's projection unchanged."""

    bus, tracker, watcher, chatlog, app_db = codex_consistency_pipeline
    scenario_dir = corpus_root / "scripted" / "consistency_codex_isolation_midpoint"

    tracker.start_session()
    try:
        harness = ConsistencyHarness(bus=bus, chatlog_path=chatlog, watcher=watcher)
        adapter = SurfaceAdapter(
            name="codex",
            view_fn=codex_view_state,
            reducer_factory=CodexReducer,
        )
        result = harness.run(
            scenario_dir=scenario_dir,
            adapter=adapter,
            view_context=CodexViewContext(conn=app_db.conn),
        )
    finally:
        if tracker.is_tracking:
            tracker.stop_session()

    assert result.holds, (
        "Codex isolation invariant failed; the chat event stream "
        f"contaminated the codex view's projection: {result.divergence}. "
        f"hydrated_state={result.hydrated_state!r} "
        f"snapshot_t1={result.snapshot_t1!r}"
    )

    # Both snapshots project zero rows since neither segment touches
    # the codex tables; the invariant under test is the equality
    # across T0 and T1.
    assert result.snapshot_t0 == result.snapshot_t1

    # Pin the projected keys and values explicitly, not just their
    # cross-segment stability: a view that swapped the two COUNT(*)
    # queries, counted the wrong table, or collapsed to a constant
    # would still report equal snapshots. The fresh codex tables hold
    # zero rows because the harness drives no /codex/claim or
    # /codex/calibrate HTTP mutation.
    expected_projection = {
        "codex_progress_row_count": 0,
        "codex_claim_row_count": 0,
    }
    assert result.snapshot_t0 == expected_projection
    assert result.snapshot_t1 == expected_projection
    # The hydrated state is rebuilt by the reducer from snapshot_t0 and
    # must match the live snapshot_t1 field-for-field for the invariant
    # to hold; pin it against the same concrete projection.
    assert result.hydrated_state == expected_projection

    data_regression.check(result.hydrated_state)
