"""FastAPI backend for EntropiaOrme — serves REST API to Svelte frontend."""

import sys
import os
from pathlib import Path

# Load .env.local at startup so direct invocations (python -m backend.main,
# pytest backend/) honour per-checkout env overrides. Skipped in frozen
# builds: packaged installs read env from the user's shell, not from a
# file beside the executable. load_dotenv defaults to override=False, so
# values already in the environment (e.g. set by a parent shell) take
# precedence over the file.
if not getattr(sys, "frozen", False):
    from dotenv import load_dotenv
    load_dotenv(Path(__file__).parent.parent / ".env.local")

if sys.platform == "win32":
    os.environ.setdefault("PYTHONUTF8", "1")
    # Set entire process to below-normal priority so we don't compete with game
    import ctypes
    BELOW_NORMAL_PRIORITY_CLASS = 0x00004000
    ctypes.windll.kernel32.SetPriorityClass(
        ctypes.windll.kernel32.GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS
    )
elif sys.platform == "linux":
    # Equivalent: nice the process down so it doesn't compete with the game
    try:
        os.nice(5)
    except OSError:
        pass  # May fail without permissions — non-critical

import logging

log = logging.getLogger(__name__)
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
import uvicorn

logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s: %(message)s")

# In a PyInstaller --onefile bundle, sys._MEIPASS is the temp extraction root
# where bundled package data (backend/data/snapshot, panel_geometry.json) lives.
# In dev, the equivalent root is the parent of the backend/ dir.
if getattr(sys, "frozen", False):
    PROJECT_ROOT = Path(sys._MEIPASS)  # type: ignore[attr-defined]
else:
    PROJECT_ROOT = Path(__file__).parent.parent
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

# Port configuration: the backend listens on ENTROPIAORME_BACKEND_PORT
# (default 8421) and the matching CORS origins / Host-header entries derive
# from ENTROPIAORME_FRONTEND_PORT (default 5173). Defaults preserve the
# historical behaviour when the env vars are unset; setting them at process
# start lets multiple instances of the app run concurrently on the same
# machine without port collisions. Invalid values fail fast at module import
# with a descriptive error rather than producing a bare ValueError during
# int() conversion.
def _read_port(name: str, default: int) -> int:
    raw = os.environ.get(name, str(default)).strip()
    try:
        port = int(raw)
    except ValueError as exc:
        raise RuntimeError(f"{name} must be an integer between 1 and 65535") from exc
    if not 1 <= port <= 65535:
        raise RuntimeError(f"{name} must be between 1 and 65535")
    return port


BACKEND_PORT = _read_port("ENTROPIAORME_BACKEND_PORT", 8421)
FRONTEND_PORT = _read_port("ENTROPIAORME_FRONTEND_PORT", 5173)

ALLOWED_API_ORIGINS = {
    "tauri://localhost",
    "http://tauri.localhost",
    f"http://localhost:{FRONTEND_PORT}",
    f"http://127.0.0.1:{FRONTEND_PORT}",
}
ALLOWED_API_HOSTS = {f"127.0.0.1:{BACKEND_PORT}", f"localhost:{BACKEND_PORT}"}

from backend.dependencies import Services, set_services
from backend.db.app_database import AppDatabase
from backend.services.game_data_store import GameDataStore
from backend.services.mob_lookup_service import MobLookupService
from backend.services.config_service import ConfigService
from backend.core.event_bus import EventBus
from backend.tracking.tracker import HuntTracker
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.services.cost_engine import (
    cost_per_shot_from_props,
    heal_cost_per_use,
    heal_reload_seconds,
)
from backend.services.config_service import active_trifecta_preset
from backend.services.trifecta_service import describe_trifecta
from backend.services.skill_tracker import SkillTracker
from backend.services.skill_scan_manual import SkillScanManual
from backend.services.scan_completion import (
    hydrate_skill_scan_state,
    make_skill_scan_completion,
)
from backend.services.codex_service import CodexService
from backend.services.quest_service import QuestService
from backend.services.hotbar_listener import HotbarListener
from backend.services.repair_ocr import RepairOcrService
from backend.services.spacebar_capture_listener import SpacebarCaptureListener
from backend.routers import health, character, equipment, settings, tracking, analytics, codex, quests
from backend.routers import scan_manual
from backend.routers import demo

@asynccontextmanager
async def lifespan(app: FastAPI):
    # Data dir resolves either to PROJECT_ROOT/data (default), to the
    # ENTROPIAORME_DATA_DIR env var (dev only), or in frozen builds to
    # %APPDATA%\EntropiaOrme\backend. The env-var override is honoured only
    # in dev so a frozen install cannot have its data dir redirected by a
    # local env var (a hardening defence-in-depth posture; the override is a
    # dev affordance, not a user feature). Relative override paths resolve
    # against PROJECT_ROOT.
    is_frozen = getattr(sys, "frozen", False)
    override = os.environ.get("ENTROPIAORME_DATA_DIR", "").strip()
    if override and not is_frozen:
        candidate = Path(override)
        data_dir = candidate if candidate.is_absolute() else (PROJECT_ROOT / candidate)
        log.info("Data dir override active: %s", data_dir)
    elif is_frozen:
        # PROJECT_ROOT in frozen mode is the read-only _MEIPASS extraction dir;
        # the user's writable data lives in %APPDATA%\EntropiaOrme\backend.
        appdata = os.environ.get("APPDATA") or str(Path.home())
        data_dir = Path(appdata) / "EntropiaOrme" / "backend"
        log.info("Frozen-mode data dir: %s", data_dir)
    else:
        data_dir = PROJECT_ROOT / "data"
    data_dir.mkdir(parents=True, exist_ok=True)

    app_db = AppDatabase(data_dir / "entropia_orme.db")
    game_data = GameDataStore(PROJECT_ROOT / "backend" / "data" / "snapshot")
    mob_lookup_service = MobLookupService(game_data)
    config_service = ConfigService(data_dir)
    event_bus = EventBus()

    def _equipment_profile_lookup(tool_name: str) -> dict | None:
        import json
        # Escape LIKE wildcards in the user-supplied fragment so embedded
        # `%` / `_` / `\\` cannot widen the match. Catalogue names today
        # contain none of these, but the escape closes the smell.
        safe = tool_name.strip().replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
        row = app_db.conn.execute(
            "SELECT properties_json, item_type FROM equipment_library "
            "WHERE item_type = 'weapon' AND name LIKE ? ESCAPE '\\'",
            (f"%{safe}%",),
        ).fetchone()
        if not row:
            return None
        return json.loads(row[0])

    def _equipment_cost_lookup(tool_name: str) -> float:
        props = _equipment_profile_lookup(tool_name)
        if not props:
            return 0.0
        return cost_per_shot_from_props(props)["totalCostPerUse"] / 100.0

    # Heal cost lookup: resolves equipment library ID → (cost_per_use_ped, reload_seconds)
    _heal_cost_cache: dict[int, tuple[float, float]] = {}

    def _heal_tool_cost_lookup(equip_id: int) -> tuple[float, float]:
        """Returns (cost_per_use_ped, reload_seconds) for a healing tool by library ID."""
        import json
        if equip_id in _heal_cost_cache:
            return _heal_cost_cache[equip_id]
        row = app_db.conn.execute(
            "SELECT properties_json FROM equipment_library WHERE id = ? AND item_type = 'healing'",
            (equip_id,),
        ).fetchone()
        if not row:
            _heal_cost_cache[equip_id] = (0.0, 2.5)
            return 0.0, 2.5
        props = json.loads(row[0])
        tool_entity = props.get("tool_entity")
        markup = props.get("markup", 100) / 100.0
        if not tool_entity:
            _heal_cost_cache[equip_id] = (0.0, 2.5)
            return 0.0, 2.5
        cost_ped = heal_cost_per_use(tool_entity, markup) / 100.0  # PEC → PED
        reload_s = heal_reload_seconds(tool_entity)
        _heal_cost_cache[equip_id] = (cost_ped, reload_s)
        return cost_ped, reload_s

    # Enhancer TT lookup: resolves enhancer name → TT value in PED from the
    # bundled game-data snapshot.
    _enhancer_cache: dict[str, float] = {}

    def _enhancer_tt_lookup(name: str) -> float:
        if name in _enhancer_cache:
            return _enhancer_cache[name]
        results = game_data.search_entities(name, endpoint="enhancers", limit=1)
        if not results:
            _enhancer_cache[name] = 0.0
            return 0.0
        entity = results[0].get("data", {})
        eco = entity.get("economy") or {}
        tt = eco.get("value") or eco.get("max_tt") or 0.0
        _enhancer_cache[name] = float(tt)
        return float(tt)

    config = config_service.get()
    # Hotbar resolver: slot key → (name, cost, item_type) or None
    # Reads the hotbar config and looks up equipment from the library
    def _hotbar_resolver(slot_key: str):
        hotbar = config_service.get().hotbar
        equip_id = hotbar.get(slot_key)
        if equip_id is None:
            return None
        row = app_db.conn.execute(
            "SELECT id, name, item_type FROM equipment_library WHERE id = ?",
            (equip_id,),
        ).fetchone()
        if not row:
            return None
        db_id, name, item_type = row
        if item_type == "healing":
            cost_ped, reload_s = _heal_tool_cost_lookup(db_id)
            return (name, cost_ped, "healing", reload_s)
        if item_type == "consumable":
            return (name, 0.0, "consumable", 0.0)
        cost = _equipment_cost_lookup(name)
        return (name, cost, "weapon", 0.0)

    def _is_weapon_attribution_trifecta() -> bool:
        return not config_service.get().hotbar_hooks_enabled

    def _get_mob_tracking_mode() -> str:
        return config_service.get().mob_tracking_mode

    def _get_mob_tracking_tag() -> str:
        return config_service.get().mob_tracking_tag

    def _is_manual_mob_entry_enabled() -> bool:
        return config_service.get().mob_tracking_mode != "tag"

    def _get_manual_mob() -> tuple[str, str] | None:
        config = config_service.get()
        species = config.manual_mob_species.strip()
        maturity = config.manual_mob_maturity.strip()
        if not species:
            return None
        return (species, maturity)

    def _resolve_trifecta():
        trifecta, _error = describe_trifecta(app_db.conn, active_trifecta_preset(config_service.get()))
        return trifecta

    tracker = HuntTracker(
        event_bus, app_db.conn,
        equipment_cost_lookup=_equipment_cost_lookup,
        equipment_profile_lookup=_equipment_profile_lookup,
        player_name=config.player_name,
        enhancer_tt_lookup=_enhancer_tt_lookup,
        loot_filter_blacklist=config.loot_filter_blacklist,
        loot_filter_blacklist_provider=lambda: config_service.get().loot_filter_blacklist,
        weapon_attribution_trifecta_provider=_is_weapon_attribution_trifecta,
        mob_tracking_mode_provider=_get_mob_tracking_mode,
        mob_tracking_tag_provider=_get_mob_tracking_tag,
        manual_mob_entry_enabled_provider=_is_manual_mob_entry_enabled,
        manual_mob_provider=_get_manual_mob,
        trifecta_resolver=_resolve_trifecta,
    )

    quest_service = QuestService(app_db, event_bus)
    chatlog_watcher = ChatlogWatcher(
        event_bus, config.chatlog_path,
        quest_reward_filter=quest_service.quest_reward_filter,
    )

    # Skill tracking: records chat.log skill gains during sessions
    skill_tracker = SkillTracker(event_bus, app_db)

    # Hydrate last scan stats from DB so status survives backend restart
    _skill_scan_time, _skill_scan_count = hydrate_skill_scan_state(app_db)

    # Skill-scan completion callback — persists scanned levels and emits drift logs
    on_skill_scan_complete = make_skill_scan_completion(app_db)

    # Manual skill scan service
    skill_scan_manual = SkillScanManual(
        config_service, data_dir,
        initial_scan_time=_skill_scan_time,
        initial_skills_count=_skill_scan_count,
    )
    skill_scan_manual.set_completion_callback(on_skill_scan_complete)

    codex_service = CodexService(app_db, game_data)

    # Hotbar key listener — pynput hotbar-slot hook. Subscribes to session
    # lifecycle events on the bus; only runs while a tracking session is
    # active AND the user-facing toggle is on.
    hotbar_listener = HotbarListener(event_bus, hotbar_resolver=_hotbar_resolver)
    hotbar_listener.apply_config(hotbar_hooks_enabled=config.hotbar_hooks_enabled)

    repair_ocr = RepairOcrService(config_service)

    # Spacebar capture listener — pynput Space hook that fires capture on the
    # active skill scan when the user opts in via the scan-overlay toggle.
    # Off until the frontend explicitly enables it.
    spacebar_capture_listener = SpacebarCaptureListener(
        skill_scan_manual=skill_scan_manual,
    )

    services = Services(
        app_db=app_db,
        game_data=game_data,
        mob_lookup=mob_lookup_service,
        config_service=config_service,
        event_bus=event_bus,
        tracker=tracker,
        chatlog_watcher=chatlog_watcher,
        skill_tracker=skill_tracker,
        skill_scan_manual=skill_scan_manual,
        codex_service=codex_service,
        quest_service=quest_service,
        hotbar_listener=hotbar_listener,
        repair_ocr=repair_ocr,
        spacebar_capture_listener=spacebar_capture_listener,
    )
    set_services(services)

    # Start event producers
    chatlog_watcher.start()

    yield

    # Cleanup
    spacebar_capture_listener.stop()
    hotbar_listener.stop()
    if tracker.is_tracking:
        tracker.stop_session()
    skill_scan_manual.shutdown()
    chatlog_watcher.stop()
    app_db.close()

def create_app() -> FastAPI:
    app = FastAPI(title="EntropiaOrme API", version="0.1.0", lifespan=lifespan)

    app.add_middleware(
        CORSMiddleware,
        allow_origins=sorted(ALLOWED_API_ORIGINS),
        allow_methods=["GET", "POST", "PATCH", "PUT", "DELETE", "OPTIONS"],
        allow_headers=["content-type"],
    )

    @app.middleware("http")
    async def enforce_api_origin(request: Request, call_next):
        if request.method == "OPTIONS":
            return await call_next(request)

        path = request.url.path
        if not path.startswith("/api/"):
            return await call_next(request)

        host = request.headers.get("host", "").lower()
        if host and host not in ALLOWED_API_HOSTS:
            return JSONResponse({"detail": "Invalid Host header"}, status_code=403)

        origin = request.headers.get("origin")
        # Tighten unsafe methods: require Origin to be present AND allowed.
        # Browsers / Tauri webview always send Origin on fetch; a same-machine
        # non-browser process omitting Origin should not be able to mutate state.
        if request.method not in ("GET", "HEAD", "OPTIONS"):
            if not origin or origin not in ALLOWED_API_ORIGINS:
                return JSONResponse({"detail": "Origin header required"}, status_code=403)
        elif origin and origin not in ALLOWED_API_ORIGINS:
            return JSONResponse({"detail": "Invalid Origin header"}, status_code=403)

        return await call_next(request)

    app.include_router(health.router, prefix="/api")
    app.include_router(character.router, prefix="/api")
    app.include_router(equipment.router, prefix="/api")
    app.include_router(settings.router, prefix="/api")
    app.include_router(tracking.router, prefix="/api")
    app.include_router(analytics.router, prefix="/api")
    app.include_router(scan_manual.router, prefix="/api")
    app.include_router(codex.router, prefix="/api")
    app.include_router(quests.router, prefix="/api")
    app.include_router(demo.router, prefix="/api")

    return app

app = create_app()

if __name__ == "__main__":
    # Pass the app object directly: an import string would re-import
    # `backend.main` as a separate module from `__main__` in a frozen
    # bundle, doubling the lifespan-init work.
    uvicorn.run(app, host="127.0.0.1", port=BACKEND_PORT)
