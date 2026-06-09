"""Test-mode endpoints (replay harness only).

The external drive surface for a backend booted as a whole process under
``ENTROPIA_TEST_MODE=1``: an external harness that cannot reach into the
process uses these routes where the in-process suite pokes objects
directly. ``GET /api/testing/drain`` exposes the watcher's drain state;
``POST /api/testing/replay`` runs the loaded scenario through the live
pipeline and returns only once the process is drained and
fingerprint-comparable.

The composition root registers this router ONLY when the test-mode
overlay is active (and never in frozen builds), so the surface does not
exist in production: requests 404 at the routing layer. Every handler
additionally re-checks the gate server-side, the recording router's
defence-in-depth pattern, so even an accidental registration stays
inert (403).
"""

import logging

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from backend.dependencies import Services, get_services
from backend.testing.clock import MockClock
from backend.testing.clock_plan import load_clock_plan
from backend.testing.replay import replay_scenario, wait_for_drain

router = APIRouter(prefix="/testing", tags=["testing"])
log = logging.getLogger(__name__)


def _require_test_mode(svc: Services) -> None:
    if not svc.test_mode.enabled:
        raise HTTPException(status_code=403, detail="Test mode disabled")


class DrainState(BaseModel):
    """The watcher's externally-observable drain state.

    A feeder that appended N lines to the tailed file polls until
    ``lines_seen`` reaches N with no pending tick: the same predicate the
    in-process drain wait uses. Covers watcher-driven events only; events
    published from other threads need the synchronous replay route (or a
    settle condition of the caller's own).
    """

    lines_seen: int
    has_pending_tick: bool


class ReplayResult(BaseModel):
    """Summary returned once a replay has fully settled."""

    session_id: str
    lines_streamed: int
    lines_seen: int
    drained: bool


@router.get("/drain", response_model=DrainState)
def drain_state() -> DrainState:
    """Report how far the watcher has read and whether a tick is pending."""
    svc = get_services()
    _require_test_mode(svc)
    watcher = svc.chatlog_watcher
    return DrainState(
        lines_seen=watcher.lines_seen,
        has_pending_tick=watcher.has_pending_tick,
    )


# Synchronous handler by design: ``wait_for_drain`` blocks, so the route runs
# on the threadpool rather than the event loop, and the response itself is the
# drain barrier (no polling, no sleeps in the caller).
@router.post("/replay", response_model=ReplayResult)
def replay() -> ReplayResult:
    """Run the loaded scenario through the live pipeline; return when drained.

    Server-side transcription of the canonical in-process driver sequence the
    committed goldens were generated from: start a tracking session (the
    direct-session path, matching the golden runs), stream the scenario's
    chat lines into the tailed file one timestamp tick per flush, wait for
    the watcher to drain, advance the frozen clock by the scenario plan's
    step, then stop the session. When this returns, every synchronous bus
    subscriber has settled and the process is fingerprint-comparable.

    The clock-instant precondition doubles as a one-replay-per-boot guard: a
    second replay in the same process finds the clock already advanced past
    the plan start and is refused.
    """
    svc = get_services()
    _require_test_mode(svc)

    scenario = svc.test_mode.scenario_dir
    if scenario is None:
        raise HTTPException(
            status_code=409,
            detail="No scenario loaded: set ENTROPIA_TEST_SCENARIO_DIR",
        )
    source = scenario / "chat_replay.log"
    if not source.is_file():
        raise HTTPException(
            status_code=409,
            detail=f"Scenario has no chat_replay.log: {scenario}",
        )

    watcher = svc.chatlog_watcher
    tailed = watcher.path
    if tailed.resolve() == source.resolve():
        # Streaming a file into itself would grow the committed scenario
        # source unboundedly; the tailed file must be a harness-designated
        # replay sink (the watcher seeks to end-of-file at start, so tailing
        # the source directly could never replay it anyway).
        raise HTTPException(
            status_code=409,
            detail=(
                "The tailed chatlog is the scenario source file; point "
                "ENTROPIA_TEST_CHATLOG at a fresh replay sink instead"
            ),
        )
    if not watcher.is_running:
        raise HTTPException(status_code=409, detail="Chatlog watcher is not running")

    clock = svc.clock
    if not isinstance(clock, MockClock):
        raise HTTPException(
            status_code=409,
            detail=(
                "Deterministic clock required: set ENTROPIA_TEST_CLOCK_START "
                "to the scenario clock plan's start instant"
            ),
        )
    plan = load_clock_plan(scenario)
    if clock.now() != plan.start:
        raise HTTPException(
            status_code=409,
            detail=(
                f"Clock instant {clock.now().isoformat()} does not match the "
                f"scenario clock plan start {plan.start.isoformat()}"
            ),
        )

    tracker = svc.tracker
    if tracker.is_tracking:
        raise HTTPException(
            status_code=409, detail="A tracking session is already active"
        )

    lines_streamed = len(
        source.read_text(encoding="utf-8").splitlines(),
    )

    session = tracker.start_session()
    replay_scenario(scenario, tailed)
    try:
        wait_for_drain(watcher, tailed)
    except TimeoutError as exc:
        # Surface the watcher's diagnostic verbatim; the run is broken and
        # the process is left as-is for post-mortem (the harness owns its
        # lifecycle).
        raise HTTPException(status_code=500, detail=str(exc)) from exc
    clock.advance(plan.step_seconds)
    tracker.stop_session()

    log.info(
        "Replay settled: scenario=%s lines=%d session=%s",
        scenario.name,
        lines_streamed,
        session.id,
    )
    return ReplayResult(
        session_id=session.id,
        lines_streamed=lines_streamed,
        lines_seen=watcher.lines_seen,
        drained=True,
    )
