"""Unit coverage for SkillScanManual's typed status outbox (the coalescer).

The scan service is an actor with a typed outbox: it publishes a
``scan.status.changed`` envelope on the in-process bus whenever its status
settles to a new value, and AT MOST once per discrete change. Phase transitions
and each per-page OCR step are announced; a no-op publish on an unchanged status
is suppressed. That settled-boundary coalescing is the property that keeps the
event-driven scan surface from relocating the retired 500ms poll onto the SSE
stream as a per-tick flood: one frame per real change, never one per call.

The real verbs (``start`` / ``capture_current_page`` / ``process``) gate on the
OCR engine and the game window, both absent in a headless test, so these drive
the producer through its owned state and outbox directly. The verb business
logic is covered by the existing scan suite; what is pinned here is the
emit/coalesce contract the wire depends on.
"""

from __future__ import annotations

from pathlib import Path

from backend.core.domain_events import TOPIC_SCAN_STATUS_CHANGED, ScanStatusChanged
from backend.core.event_bus import EventBus
from backend.services.skill_scan_manual import SkillScanManual


def _recording_service() -> tuple[SkillScanManual, list[ScanStatusChanged]]:
    """A scan service wired to a bus that records every published envelope."""
    frames: list[ScanStatusChanged] = []
    bus = EventBus()
    bus.subscribe(TOPIC_SCAN_STATUS_CHANGED, frames.append)
    return SkillScanManual(None, Path("."), event_bus=bus), frames


def test_no_event_bus_means_a_silent_outbox() -> None:
    """A producer constructed without a bus (pure-OCR unit tests) emits nothing
    and never raises when a status change would otherwise publish."""
    svc = SkillScanManual(None, Path("."))
    with svc._lock:
        svc._active = True
        svc._captures = [b"page"]
    svc._publish_status()  # None outbox: no-op, no raise


def test_resting_status_publishes_nothing() -> None:
    """Publishing on the unchanged idle baseline emits no frame: a listener has
    already hydrated the idle status via the GET on mount, so an initial idle
    frame would be redundant."""
    svc, frames = _recording_service()
    svc._publish_status()
    svc._publish_status()
    assert frames == []


def test_one_typed_frame_per_discrete_change() -> None:
    """Each settled status change is exactly one typed frame; a repeat publish on
    an unchanged status is coalesced away."""
    svc, frames = _recording_service()

    with svc._lock:
        svc._active = True
        svc._captures = [b"page"]
    svc._publish_status()
    svc._publish_status()  # unchanged: coalesced

    assert len(frames) == 1
    assert isinstance(frames[0], ScanStatusChanged)
    assert frames[0].type == "scan.status.changed"
    assert frames[0].event_version == 1
    assert frames[0].payload.phase == "capturing"


def test_each_per_page_step_is_one_frame() -> None:
    """Per-page OCR progress emits one frame per advanced ``done/total`` step, so
    the overlay's page-by-page liveness survives the poll's removal without a
    timer (the granularity decision). A repeated count is coalesced."""
    svc, frames = _recording_service()

    with svc._lock:
        svc._active = False
        svc._processing = True
        svc._processing_progress = (0, 3)
    svc._publish_status()  # idle -> processing
    for done in (1, 2, 3):
        with svc._lock:
            svc._processing_progress = (done, 3)
        svc._publish_status()  # one frame per page
    with svc._lock:
        svc._processing_progress = (3, 3)
    svc._publish_status()  # unchanged: coalesced

    phases = [f.payload.phase for f in frames]
    assert phases == ["processing", "processing", "processing", "processing"]
    assert len(frames) == 4  # 1 transition + 3 page steps, the repeat coalesced


def test_completion_transitions_to_awaiting_review() -> None:
    """When the worker resolves, the producer announces awaiting_review."""
    svc, frames = _recording_service()

    with svc._lock:
        svc._processing = True
        svc._processing_progress = (3, 3)
    svc._publish_status()  # -> processing
    frames.clear()

    with svc._lock:
        svc._processing = False
        svc._pending_result = {"Aim": 10.0}
    svc._publish_status()  # -> awaiting_review

    assert len(frames) == 1
    assert frames[0].payload.phase == "awaiting_review"


# ── Terminal transitions (the headline behaviour: an observer clears on idle) ──
#
# Each reaches its precondition phase via a *published* change first, so the
# coalescer's last-emitted baseline is that phase; the terminal verb then runs
# for real (none of these gate on the OCR engine or the game window) and must
# announce its settled phase exactly once.


def test_cancel_publishes_the_return_to_idle() -> None:
    """Cancelling a capture session announces the return to idle."""
    svc, frames = _recording_service()
    with svc._lock:
        svc._active = True
        svc._captures = [b"page"]
    svc._publish_status()  # -> capturing
    frames.clear()

    svc.cancel()

    assert len(frames) == 1
    assert frames[0].payload.phase == "idle"


def test_reject_publishes_the_return_to_idle() -> None:
    """Rejecting a pending result announces the return to idle."""
    svc, frames = _recording_service()
    with svc._lock:
        svc._pending_result = {"Aim": 10.0}
    svc._publish_status()  # -> awaiting_review
    frames.clear()

    svc.reject()

    assert len(frames) == 1
    assert frames[0].payload.phase == "idle"


def test_accept_publishes_the_return_to_idle() -> None:
    """Accepting a pending result persists it and announces the return to idle.

    The accept path is the only one that both advances the last-scan bookkeeping
    and resets the phase, so its single idle frame is what an observer (the
    character view) relies on to clear its in-flight state."""
    svc, frames = _recording_service()
    svc.set_completion_callback(lambda skills: None)
    with svc._lock:
        svc._pending_result = {"Aim": 10.0}
    svc._publish_status()  # -> awaiting_review
    frames.clear()

    svc.accept()

    assert len(frames) == 1
    assert frames[0].payload.phase == "idle"


def test_undo_publishes_the_capture_decrement() -> None:
    """Undoing a capture announces the decremented page count (still capturing)."""
    svc, frames = _recording_service()
    with svc._lock:
        svc._active = True
        svc._captures = [b"a", b"b"]
    svc._publish_status()  # -> capturing, 2 captured
    frames.clear()

    svc.undo_last_capture()

    assert len(frames) == 1
    assert frames[0].payload.phase == "capturing"


def test_capturer_factory_threads_through_to_the_core() -> None:
    """The manual scan hands its injected capturer factory to the core.

    The composition root selects the fixture capturer on ``SkillScanManual``;
    the capture call the core makes must come off that factory.
    """
    served: list[tuple[int, int, int, int]] = []

    class _Fixture:
        def capture_region_png(self, x, y, w, h) -> bytes:
            served.append((x, y, w, h))
            return b"FIXTURE"

    svc = SkillScanManual(None, Path("."), capturer_factory=_Fixture)

    assert svc._core.capture_region([0, 0], [10, 10]) == b"FIXTURE"
    assert served == [(0, 0, 10, 10)]
