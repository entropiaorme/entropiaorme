"""Recording-mode endpoints (developer-only).

Drives the session recorder lifecycle. Every handler is gated server-side on
the ``developer_mode_enabled`` config flag, independent of the frontend, so
the surface is inert (403) unless the developer has explicitly opted in.
"""

import logging

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from backend.dependencies import get_services
from backend.testing.recording_controller import (
    RecordingStateError,
    RecordingValidationError,
)

router = APIRouter(prefix="/recording", tags=["recording"])
log = logging.getLogger(__name__)


def _require_dev(svc) -> None:
    if not svc.config_service.get().developer_mode_enabled:
        raise HTTPException(status_code=403, detail="Developer mode disabled")


class StopRecordingBody(BaseModel):
    scenario_name: str
    description: str = ""
    surfaces: list[str] = Field(default_factory=list)
    character_context: dict = Field(default_factory=dict)
    rare_event_flags: list[str] = Field(default_factory=list)
    notes: str = ""


@router.post("/start")
def start_recording():
    """Begin a recording session."""
    svc = get_services()
    _require_dev(svc)
    try:
        return svc.recording_controller.start()
    except RecordingStateError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc


@router.get("/status")
def recording_status():
    """Current recording state plus live capture counters."""
    svc = get_services()
    _require_dev(svc)
    return svc.recording_controller.status()


@router.post("/stop")
def stop_recording(body: StopRecordingBody):
    """Finalise the recording into the recorded-scenario corpus."""
    svc = get_services()
    _require_dev(svc)
    try:
        return svc.recording_controller.stop(body.model_dump())
    except RecordingValidationError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    except RecordingStateError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc


@router.post("/abort")
def abort_recording():
    """Discard the in-flight recording without finalising."""
    svc = get_services()
    _require_dev(svc)
    return svc.recording_controller.abort()
