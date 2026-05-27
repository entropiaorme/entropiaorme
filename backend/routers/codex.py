"""Codex endpoints — species listing, rank breakdowns, claims, calibration."""

from typing import Literal

from fastapi import APIRouter, HTTPException, Query
from pydantic import BaseModel

from backend.dependencies import get_services

router = APIRouter(prefix="/codex", tags=["codex"])


# ── Request models ──────────────────────────────────────────────────────────


class ClaimRequest(BaseModel):
    species_name: str
    rank: int
    skill_name: str


class CalibrateRequest(BaseModel):
    species_name: str
    rank: int


class MetaClaimRequest(BaseModel):
    attribute_name: str


# ── Endpoints ───────────────────────────────────────────────────────────────


@router.get("/species")
def list_species():
    """All mob species with codex base cost and player rank."""
    svc = get_services()
    return svc.codex_service.get_all_species()


@router.get("/species/{name}/ranks")
def species_ranks(name: str):
    """25-rank breakdown for a species."""
    svc = get_services()
    result = svc.codex_service.get_species_ranks(name)
    if result is None:
        raise HTTPException(status_code=404, detail=f"Species '{name}' not found")
    return result


@router.post("/claim")
def claim_rank(req: ClaimRequest):
    """Claim a codex rank reward."""
    svc = get_services()
    try:
        result = svc.codex_service.claim_rank(req.species_name, req.rank, req.skill_name)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    # If tracking is active, suppress the upcoming skill gain from deduplication
    if svc.tracker.is_tracking:
        svc.skill_tracker.suppress_next(req.skill_name)

    return result


@router.post("/calibrate")
def calibrate(req: CalibrateRequest):
    """Set codex rank directly (manual calibration, no side effects)."""
    svc = get_services()
    try:
        return svc.codex_service.calibrate(req.species_name, req.rank)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.get("/recommend")
def recommend(
    species_name: str,
    # Codex ranks run 1..25. Out-of-range values index past the reward and cost
    # tables (a negative rank wraps to the wrong row, zero and over-range values
    # overflow), so constrain the input to its valid domain and reject anything
    # else with a 422 rather than letting it reach those lookups.
    rank: int = Query(..., ge=1, le=25),
    profession: str | None = None,
    target: Literal["profession", "hp"] = "profession",
):
    """Skill options for a rank, ranked by profession or HP gain."""
    svc = get_services()
    return svc.codex_service.get_skill_options(species_name, rank, profession, target)


@router.get("/meta/attributes")
def meta_attributes():
    """Return the 6 attributes with current levels."""
    svc = get_services()
    return svc.codex_service.get_meta_attributes()


@router.post("/meta/claim")
def meta_claim(req: MetaClaimRequest):
    """Claim a meta codex reward (1 PED into an attribute)."""
    svc = get_services()
    try:
        result = svc.codex_service.meta_claim(req.attribute_name)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    if svc.tracker.is_tracking:
        svc.skill_tracker.suppress_next(req.attribute_name)

    return result
