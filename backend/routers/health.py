"""Health check endpoint."""

from fastapi import APIRouter

from backend.routers.response_models import HealthStatus

router = APIRouter(tags=["health"])


@router.get("/health", response_model=HealthStatus)
def health_check():
    return {"status": "ok"}
