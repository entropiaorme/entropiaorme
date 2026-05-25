"""Manual scan router — user-driven page-by-page capture flow.

Endpoints drive ``SkillScanManual``. The user clicks "capture" in the
dedicated scan overlay; the backend snaps the preset region and stores
the PNG. ``process`` extracts via the local OCR engine on a background
thread; ``pending`` exposes the held result for diff-review; ``accept``
persists and ``reject`` discards.
"""

from fastapi import APIRouter, Depends, HTTPException, Response

from backend.dependencies import Services, get_services

router = APIRouter(prefix="/scan", tags=["scan-manual"])


# ── Skill scan ──


@router.get("/skills/status")
def skill_status(services: Services = Depends(get_services)):
    return services.skill_scan_manual.get_status()


@router.post("/skills/start")
def skill_start(
    page_count: int | None = None, services: Services = Depends(get_services)
):
    return services.skill_scan_manual.start(page_count=page_count)


@router.post("/skills/capture")
def skill_capture(services: Services = Depends(get_services)):
    return services.skill_scan_manual.capture_current_page()


@router.post("/skills/cancel")
def skill_cancel(services: Services = Depends(get_services)):
    return services.skill_scan_manual.cancel()


@router.post("/skills/undo")
def skill_undo(services: Services = Depends(get_services)):
    return services.skill_scan_manual.undo_last_capture()


@router.post("/skills/process")
def skill_process(services: Services = Depends(get_services)):
    return services.skill_scan_manual.process()


@router.post("/skills/accept")
def skill_accept(services: Services = Depends(get_services)):
    return services.skill_scan_manual.accept()


@router.post("/skills/reject")
def skill_reject(services: Services = Depends(get_services)):
    return services.skill_scan_manual.reject()


@router.get("/skills/pending")
def skill_pending(services: Services = Depends(get_services)):
    pending = services.skill_scan_manual.get_pending_result()
    if pending is None:
        raise HTTPException(status_code=404, detail="No pending skill scan result")
    return {"skills": pending}


@router.get("/skills/capture/{page}")
def skill_capture_png(page: int, services: Services = Depends(get_services)):
    png = services.skill_scan_manual.get_capture_png(page)
    if png is None:
        raise HTTPException(status_code=404, detail="Capture not available")
    return Response(content=png, media_type="image/png")


# ── Spacebar capture (overlay-wide toggle) ──


@router.post("/spacebar-capture")
def set_spacebar_capture(enabled: bool, services: Services = Depends(get_services)):
    services.spacebar_capture_listener.set_enabled(enabled)
    return {"ok": True, "enabled": services.spacebar_capture_listener.is_enabled}
