"""Read-only `/demo/*` API namespace for guide-mode playback.

Serves the bundled curated demo DB (`data/demo/entropia_orme.db`, produced by
the demo seeder under `backend/scripts/demo_seed/`) through a parallel router
that re-uses the existing analytics + tracking read handlers via their
conn-parametric and services-parametric impls.

Architecture:
- The bundled demo DB on disk is opened read-only and backed up into a
  process-private in-memory SQLite connection. The parallel HuntTracker can
  prime + write into the in-memory copy (mid-hunt session row, kill rows,
  notable events, skill gains) without ever mutating the bundled file.
- Analytics-style endpoints (overview/activity/ledger/etc.) just need the
  conn; the tracker isn't constructed until a tracker-state endpoint
  (status/live/recent-events) is actually hit. Surfaces that only consume
  analytics data therefore never trigger priming, keeping their behaviour
  consistent across walks regardless of which surface the user opens first.
- Routes here are GET-only; mutating verbs simply do not exist on this prefix
  and FastAPI returns 405.
- If the demo DB is not present on disk (e.g. dev environment without a seed,
  or a frozen bundle before build-time bundling is wired in), endpoints return
  503 with a hint pointing at the seeder command.

Build-time bundling for frozen mode is handled by `backend/build_sidecar.spec`
shipping `data/demo/entropia_orme.db` under the bundle root; the path resolver
below picks up `sys._MEIPASS` automatically when frozen.
"""

from __future__ import annotations

import logging
import sqlite3
import sys
import threading
from pathlib import Path
from types import SimpleNamespace

from fastapi import APIRouter, HTTPException

from backend.core.event_bus import EventBus
from backend.routers import analytics as analytics_router
from backend.routers import tracking as tracking_router
from backend.routers.response_models import (
    AnalyticsOverview,
    NotableEvent,
    TrackingLive,
    TrackingStatus,
)

log = logging.getLogger(__name__)

router = APIRouter(prefix="/demo", tags=["demo"])

_DEMO_DB_RELPATH = Path("data") / "demo" / "entropia_orme.db"
_state_lock = threading.Lock()
_state: dict = {"conn": None, "svc": None}


def _project_root() -> Path:
    """Resolve PROJECT_ROOT consistently with backend/main.py."""
    if getattr(sys, "frozen", False):
        return Path(sys._MEIPASS)  # type: ignore[attr-defined]
    return Path(__file__).resolve().parents[2]


def _resolve_demo_db_path() -> Path:
    return _project_root() / _DEMO_DB_RELPATH


def _ensure_conn() -> sqlite3.Connection:
    """Lazy-build the in-memory demo connection (backup-clone of the bundled DB)."""
    with _state_lock:
        if _state["conn"] is not None:
            return _state["conn"]
        db_path = _resolve_demo_db_path()
        if not db_path.exists():
            raise HTTPException(
                status_code=503,
                detail=(
                    "Demo DB not bundled. Run "
                    "`python -m backend.scripts.demo_seed --reseed --out data/demo` "
                    "from the public-tier root, or rebuild the app."
                ),
            )
        src = sqlite3.connect(f"file:{db_path.as_posix()}?mode=ro", uri=True)
        dst = sqlite3.connect(":memory:", check_same_thread=False)
        src.backup(dst)
        src.close()
        dst.row_factory = sqlite3.Row
        _state["conn"] = dst
        log.info("Cloned demo DB into in-memory connection from %s.", db_path)
        return dst


def _ensure_svc():
    """Lazy-build the parallel HuntTracker + stub services object.

    Triggers `_ensure_conn` then constructs a HuntTracker pointed at the
    in-memory DB and primes it with the `mid_hunt` scenario. Stub services
    exposes the attribute surface the impl helpers consume.
    """
    with _state_lock:
        if _state["svc"] is not None:
            return _state["svc"]
    conn = _ensure_conn()
    with _state_lock:
        if _state["svc"] is not None:
            return _state["svc"]

        try:
            from backend.scripts.demo_seed.live_injection import prime_tracker
            from backend.tracking.tracker import HuntTracker
        except ImportError as exc:
            raise HTTPException(
                status_code=503,
                detail=(
                    "Demo tracker priming not available in this build. "
                    f"Underlying error: {exc}"
                ),
            ) from exc

        tracker = HuntTracker(event_bus=EventBus(), db_conn=conn)
        primed = prime_tracker(tracker, "mid_hunt")
        if not primed:
            log.warning(
                "Demo mid_hunt priming returned False; demo tracker state may be incomplete."
            )

        # hotbar_hooks_enabled=False routes weapon_attribution through trifecta
        # (per tracking._weapon_attribution); the guide's overlay-spawn step
        # then renders the trifecta dropdown affordance rather than the static
        # hotbar weapon text. Preset equipment is resolved by name from the
        # seeded library so the references survive any seeder rerun.
        def _lookup_id(name: str) -> int:
            row = conn.execute(
                "SELECT id FROM equipment_library WHERE name = ?", (name,)
            ).fetchone()
            if row is None:
                raise HTTPException(
                    status_code=503,
                    detail=(
                        f"Demo equipment '{name}' missing from seeded library; "
                        "rebuild the demo DB."
                    ),
                )
            return int(row[0])

        trifecta_preset = SimpleNamespace(
            id="demo_default",
            name="Calypso",
            small_weapon_id=_lookup_id("Jester D-1"),
            big_weapon_id=_lookup_id("Korss H400"),
            heal_id=_lookup_id("Vivo T1"),
        )
        config = SimpleNamespace(
            hotbar_hooks_enabled=False,
            mob_tracking_mode="mob",
            mob_tracking_tag="",
            repair_ocr_enabled=False,
            end_of_session_armour_reminder_enabled=False,
            manual_mob_species="",
            manual_mob_maturity="",
            trifecta_presets=[trifecta_preset],
            active_trifecta_preset_id=trifecta_preset.id,
        )
        svc = SimpleNamespace(
            tracker=tracker,
            app_db=SimpleNamespace(conn=conn),
            config_service=SimpleNamespace(get=lambda: config),
            hotbar_listener=SimpleNamespace(is_running=True),
        )
        _state["svc"] = svc
        log.info("Demo namespace primed: parallel HuntTracker active on in-memory DB.")
        return svc


# ── Analytics ──────────────────────────────────────────────────────────────


@router.get("/analytics/overview", response_model=AnalyticsOverview)
def demo_analytics_overview(period: str = "all"):
    return analytics_router.overview_impl(_ensure_conn(), period)


@router.get("/analytics/activity")
def demo_analytics_activity():
    return analytics_router.activity_impl(_ensure_conn())


@router.get("/analytics/ledger")
def demo_list_ledger():
    return analytics_router.list_ledger_impl(_ensure_conn())


@router.get("/analytics/ledger/presets")
def demo_list_ledger_presets():
    return analytics_router.list_ledger_presets_impl(_ensure_conn())


@router.get("/analytics/inventory")
def demo_list_inventory_items():
    return analytics_router.list_inventory_items_impl(_ensure_conn())


# ── Tracking ───────────────────────────────────────────────────────────────


@router.get("/tracking/sessions")
def demo_list_sessions():
    return tracking_router.list_sessions_impl(_ensure_conn())


@router.get("/tracking/session/{session_id}")
def demo_get_session(session_id: str):
    return tracking_router.get_session_impl(_ensure_conn(), session_id)


@router.get(
    "/tracking/status",
    response_model=TrackingStatus,
    response_model_exclude_unset=True,
)
def demo_tracking_status():
    return tracking_router.tracking_status_impl(_ensure_svc())


@router.get(
    "/tracking/live",
    response_model=TrackingLive,
    response_model_exclude_unset=True,
)
def demo_tracking_live():
    return tracking_router.tracking_live_impl(_ensure_svc())


@router.get(
    "/tracking/recent-events",
    response_model=list[NotableEvent],
    response_model_exclude_unset=True,
)
def demo_recent_events():
    return tracking_router.recent_events_impl(_ensure_svc())
