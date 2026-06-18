"""FastAPI backend for EntropiaOrme — serves REST API to Svelte frontend."""

import asyncio
import contextlib
import os
import sys
from collections.abc import Callable
from datetime import datetime
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
    # Equivalent: nice the process down so it doesn't compete with the game.
    # os.nice may fail without the right permissions; that is non-critical.
    with contextlib.suppress(OSError):
        os.nice(5)

import logging

log = logging.getLogger(__name__)
from contextlib import asynccontextmanager

import uvicorn
from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

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
    # Canonical dev hostname when the optional Caddy reverse-proxy
    # substrate fronts the frontend over HTTPS. Always allowed because the
    # entry is harmless when Caddy isn't in play (no request would carry
    # this Origin) and avoids a second env-var check just to gate it.
    "https://entropiaorme.localhost",
}

# Per-checkout HTTPS dev hostname when the optional Caddy substrate is in
# play and the checkout has been configured for it (an additional
# checkout exports ENTROPIAORME_HOSTNAME via its `.env.local`, which the
# load_dotenv call at module top has already sourced into the process
# environment). Absent in fresh clones without the substrate, in which
# case the static origins above cover the canonical case.
#
# Defence-in-depth: only accept .localhost values so a mis-set or
# malicious env var can't silently widen the CORS allowlist to a non-local
# origin. Matches _read_port's fail-fast posture for invalid env config.
_per_checkout_hostname = os.environ.get("ENTROPIAORME_HOSTNAME", "").strip()
if _per_checkout_hostname:
    if not _per_checkout_hostname.endswith(".localhost"):
        raise RuntimeError(
            f"ENTROPIAORME_HOSTNAME must end with '.localhost' "
            f"(got: {_per_checkout_hostname!r})"
        )
    ALLOWED_API_ORIGINS.add(f"https://{_per_checkout_hostname}")

ALLOWED_API_HOSTS = {f"127.0.0.1:{BACKEND_PORT}", f"localhost:{BACKEND_PORT}"}

from backend.core.event_bus import EventBus
from backend.db.app_database import AppDatabase
from backend.dependencies import Services, set_services
from backend.middleware.etag import install_etag_middleware
from backend.routers import (
    analytics,
    character,
    codex,
    demo,
    equipment,
    events,
    health,
    quests,
    scan_manual,
    settings,
    testing,
    tracking,
)
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.services.codex_service import CodexService
from backend.services.config_service import ConfigService, active_trifecta_preset
from backend.services.cost_engine import (
    cost_per_shot_from_props,
    heal_cost_per_use,
    heal_reload_seconds,
)
from backend.services.event_stream import EventStreamHub
from backend.services.game_data_store import GameDataStore
from backend.services.hotbar_listener import HOTBAR_SLOT_KEYS, HotbarListener
from backend.services.mob_lookup_service import MobLookupService
from backend.services.quest_service import QuestService
from backend.services.repair_ocr import RepairOcrService
from backend.services.scan_completion import (
    hydrate_skill_scan_state,
    make_skill_scan_completion,
)
from backend.services.skill_scan_manual import SkillScanManual
from backend.services.skill_tracker import SkillTracker
from backend.services.spacebar_capture_listener import SpacebarCaptureListener
from backend.services.trifecta_service import describe_trifecta
from backend.testing.capturer import SequencedFixtureCapturer
from backend.testing.clock import Clock, MockClock, RealClock
from backend.testing.config import TestModeConfig
from backend.testing.events_sink import EventsJsonlSink
from backend.testing.keystroke_source import (
    KeystrokeSource,
    MockKeystrokeSource,
    PynputKeystrokeSource,
)
from backend.tracking.tracker import HuntTracker


def _build_test_mode() -> TestModeConfig:
    """Resolve the test-mode overlay for this process.

    Frozen builds refuse test mode outright, whatever their environment
    says: the replay-harness surface (redirected chatlog, mock input
    sources, fixture capturers, the test-only API routes) must never be
    reachable in a packaged install. Same defence-in-depth posture as the
    data-dir override below.
    """
    if getattr(sys, "frozen", False):
        return TestModeConfig()
    return TestModeConfig.from_env()


def _producers_idle() -> bool:
    """Whether this process should stand its event producers down.

    When the native substrate owns production, the sidecar must not also
    run its producers: two chat-log tailers writing the same database and
    two OS keyboard hooks would double-count and conflict. Setting
    ``ENTROPIAORME_PRODUCERS_IDLE`` to a truthy value keeps every proxied
    HTTP route serving while constructing no live producer machinery (no
    chat-log tail thread, no OS key hooks, no background scan or OCR). It
    touches only producer startup, never any HTTP or database-state
    response shape, so it is golden-neutral by construction. Same
    defence-in-depth posture as the test-mode overlay: a frozen build
    still honours it, because the substrate that sets it is the frozen
    app's own shell.
    """
    raw = os.environ.get("ENTROPIAORME_PRODUCERS_IDLE", "").strip().lower()
    return raw not in ("", "0", "false", "no", "off")


def _constant_factory(instance: object) -> Callable[[], object]:
    """A capturer factory returning one shared pre-built instance.

    Consumers that re-resolve their factory per scan still walk a single
    recorded fixture sequence.
    """

    def factory() -> object:
        return instance

    return factory


def _build_clock() -> Clock:
    """Construct the process-wide time source.

    Production runs on the real clock. A replay harness driving this backend
    as a whole process freezes it instead by setting
    ``ENTROPIA_TEST_CLOCK_START`` to the scenario clock plan's naive
    ISO-8601 start instant (see ``backend/testing/clock_plan.py``); every
    injected read then returns that frozen instant, keeping each stamped
    timestamp deterministic and independent of how many times the
    implementation reads time.
    """
    raw = os.environ.get("ENTROPIA_TEST_CLOCK_START", "").strip()
    if not raw:
        return RealClock()
    try:
        start = datetime.fromisoformat(raw)
    except ValueError as exc:
        raise RuntimeError(
            "ENTROPIA_TEST_CLOCK_START must be a naive ISO-8601 instant"
        ) from exc
    if start.tzinfo is not None:
        # An aware instant would be reinterpreted by MockClock's later
        # ``.timestamp()`` conversion (UTC vs host-local), silently shifting
        # replay semantics away from the naive plan. Reject it, matching the
        # same guard in backend/testing/clock_plan.py.
        raise RuntimeError("ENTROPIA_TEST_CLOCK_START must be a naive ISO-8601 instant")
    log.info("Deterministic clock active: frozen at %s", start.isoformat())
    return MockClock(start=start)


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

    # Test-mode overlay (inert unless ENTROPIA_TEST_MODE=1; never active in
    # frozen builds). Resolved once here; every seam selection below is a
    # one-shot wiring decision, never a hot-path branch.
    test_mode = _build_test_mode()

    # Producer-idle overlay: when the native substrate owns production, the
    # sidecar constructs its services (so every proxied route still serves)
    # but starts no live producer machinery. Resolved once here; each
    # producer-start site below is a one-shot wiring decision. Test mode and
    # idle mode are independent: test mode redirects the seams, idle mode
    # declines to start them.
    producers_idle = _producers_idle()
    if producers_idle:
        log.info("Producers idle: the sidecar serves routes but starts no producers")

    events_sink: EventsJsonlSink | None = None
    if test_mode.enabled:
        log.info(
            "Test mode active: scenario=%s chatlog=%s fixtures=%s",
            test_mode.scenario_dir,
            test_mode.chatlog_path,
            test_mode.fixture_dir,
        )
        # Full publish-order event sink, installed before any producer starts
        # so it provably observes every publish of the process's lifetime.
        events_sink = EventsJsonlSink(data_dir / "events.jsonl")
        events_sink.install(event_bus)

    # SSE fan-out hub: subscribes to the coarse domain topics on the bus and
    # forwards their frames to GET /api/events streams. Bind it to the running
    # uvicorn loop now (we are inside it) so the bus callback, which fires on the
    # producer thread, can hop frames across the thread boundary.
    event_stream_hub = EventStreamHub(event_bus)
    event_stream_hub.bind_loop(asyncio.get_running_loop())

    def _equipment_profile_lookup(tool_name: str) -> dict | None:
        import json

        # Escape LIKE wildcards in the user-supplied fragment so embedded
        # `%` / `_` / `\\` cannot widen the match. Catalogue names today
        # contain none of these, but the escape closes the smell.
        safe = (
            tool_name.strip()
            .replace("\\", "\\\\")
            .replace("%", "\\%")
            .replace("_", "\\_")
        )
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
        trifecta, _error = describe_trifecta(
            app_db.conn, active_trifecta_preset(config_service.get())
        )
        return trifecta

    # The process-wide time source. One instance is constructed here at the
    # composition root and injected into every service that reads the clock,
    # so a deterministic clock can be swapped in at exactly one place.
    # RealClock preserves the stdlib semantics each call site had before
    # injection. The watcher takes this clock too, but guards its own drain
    # timeout against a frozen clock internally (see ChatlogWatcher), so the
    # composition root injects it uniformly with every other service.
    clock = _build_clock()

    tracker = HuntTracker(
        event_bus,
        app_db.conn,
        equipment_cost_lookup=_equipment_cost_lookup,
        equipment_profile_lookup=_equipment_profile_lookup,
        player_name=config.player_name,
        enhancer_tt_lookup=_enhancer_tt_lookup,
        loot_filter_blacklist=config.loot_filter_blacklist,
        loot_filter_blacklist_provider=lambda: (
            config_service.get().loot_filter_blacklist
        ),
        weapon_attribution_trifecta_provider=_is_weapon_attribution_trifecta,
        mob_tracking_mode_provider=_get_mob_tracking_mode,
        mob_tracking_tag_provider=_get_mob_tracking_tag,
        manual_mob_entry_enabled_provider=_is_manual_mob_entry_enabled,
        manual_mob_provider=_get_manual_mob,
        trifecta_resolver=_resolve_trifecta,
        clock=clock,
    )

    quest_service = QuestService(app_db, event_bus, clock=clock)

    # Chatlog seam: production tails the user-configured chat.log; test mode
    # redirects to the harness-designated file (never the user's real log)
    # and guarantees it exists, because the watcher's tail loop silently
    # declines to start on a missing file and a never-draining replay is far
    # harder to diagnose than a loud startup failure here.
    if test_mode.enabled:
        watched_chatlog = test_mode.chatlog_path or (data_dir / "chat_replay.log")
        if not watched_chatlog.exists():
            # Create-only-if-missing: an existing file (e.g. a committed
            # scenario source) must not even have its mtime disturbed.
            watched_chatlog.parent.mkdir(parents=True, exist_ok=True)
            watched_chatlog.touch()
    else:
        watched_chatlog = Path(config.chatlog_path)
    chatlog_watcher = ChatlogWatcher(
        event_bus,
        watched_chatlog,
        quest_reward_filter=quest_service.quest_reward_filter,
        clock=clock,
    )

    # Skill tracking: records chat.log skill gains during sessions
    skill_tracker = SkillTracker(event_bus, app_db, clock=clock)

    # Hydrate last scan stats from DB so status survives backend restart
    _skill_scan_time, _skill_scan_count = hydrate_skill_scan_state(app_db)

    # Skill-scan completion callback — persists scanned levels and emits drift logs
    on_skill_scan_complete = make_skill_scan_completion(app_db, clock=clock)

    # Capture seam: test mode serves the scenario's recorded panel series
    # instead of grabbing the screen (one shared sequence per panel type, so
    # per-scan factory resolution still walks the recorded order); production
    # keeps the lazy screen capturer the consumers default to.
    skill_capturer_factory: Callable[[], object] | None = None
    repair_capturer_factory: Callable[[], object] | None = None
    if test_mode.enabled:
        skill_capturer_factory = _constant_factory(
            SequencedFixtureCapturer(test_mode.fixture_dir, "skill")
        )
        repair_capturer_factory = _constant_factory(
            SequencedFixtureCapturer(test_mode.fixture_dir, "repair")
        )

    # Manual skill scan service
    skill_scan_manual = SkillScanManual(
        config_service,
        data_dir,
        event_bus=event_bus,
        initial_scan_time=_skill_scan_time,
        initial_skills_count=_skill_scan_count,
        clock=clock,
        capturer_factory=skill_capturer_factory,
    )
    skill_scan_manual.set_completion_callback(on_skill_scan_complete)

    codex_service = CodexService(app_db, game_data, clock=clock)

    # Hotbar key listener. Consumes a PynputKeystrokeSource filtered to the
    # number-row hotbar keys at the OS-hook boundary (input minimisation made
    # structural); subscribes to session lifecycle events on the bus; only
    # runs while a tracking session is active AND the user-facing toggle is on.
    # Test mode swaps in an injectable mock source (one per listener,
    # preserving the production shape) so a replay process never installs a
    # real OS keyboard hook.
    # Idle mode swaps in the mock source too, so no real OS keyboard hook is
    # installed when the substrate owns input; the listener is constructed
    # either way so the routes that read its state still serve.
    hotbar_keystroke_source: KeystrokeSource
    spacebar_keystroke_source: KeystrokeSource
    if test_mode.enabled or producers_idle:
        hotbar_keystroke_source = MockKeystrokeSource()
    else:
        hotbar_keystroke_source = PynputKeystrokeSource(
            key_allowlist=HOTBAR_SLOT_KEYS, thread_name="hotbar-key-listener"
        )
    hotbar_listener = HotbarListener(
        event_bus,
        keystroke_source=hotbar_keystroke_source,
        hotbar_resolver=_hotbar_resolver,
    )
    # Idle mode keeps the hook latent regardless of the stored toggle; the
    # substrate's listener owns the hotbar instead.
    hotbar_listener.apply_config(
        hotbar_hooks_enabled=config.hotbar_hooks_enabled and not producers_idle
    )

    repair_ocr = RepairOcrService(
        config_service, capturer_factory=repair_capturer_factory
    )

    # Spacebar capture listener. Consumes a PynputKeystrokeSource filtered to
    # the space key only; fires capture on the active skill scan when the user
    # opts in via the scan-overlay toggle. Off until the frontend explicitly
    # enables it.
    if test_mode.enabled or producers_idle:
        spacebar_keystroke_source = MockKeystrokeSource()
    else:
        spacebar_keystroke_source = PynputKeystrokeSource(
            key_allowlist={"space"}, thread_name="spacebar-key-listener"
        )
    spacebar_capture_listener = SpacebarCaptureListener(
        skill_scan_manual=skill_scan_manual,
        keystroke_source=spacebar_keystroke_source,
    )

    services = Services(
        app_db=app_db,
        game_data=game_data,
        mob_lookup=mob_lookup_service,
        config_service=config_service,
        event_bus=event_bus,
        event_stream_hub=event_stream_hub,
        tracker=tracker,
        chatlog_watcher=chatlog_watcher,
        skill_tracker=skill_tracker,
        skill_scan_manual=skill_scan_manual,
        codex_service=codex_service,
        quest_service=quest_service,
        hotbar_listener=hotbar_listener,
        repair_ocr=repair_ocr,
        spacebar_capture_listener=spacebar_capture_listener,
        clock=clock,
        test_mode=test_mode,
        hotbar_keystroke_source=hotbar_keystroke_source,
        spacebar_keystroke_source=spacebar_keystroke_source,
    )
    set_services(services)

    # Start event producers. Idle mode declines: the substrate's own
    # chat-log tailer owns production, so the sidecar must not tail the same
    # log into the same database.
    if not producers_idle:
        chatlog_watcher.start()

    yield

    # Cleanup. Detach the SSE hub from the bus first so a producer's final
    # shutdown event cannot hop a frame onto the closing loop.
    event_stream_hub.close()
    spacebar_capture_listener.stop()
    hotbar_listener.stop()
    if tracker.is_tracking:
        tracker.stop_session()
    skill_scan_manual.shutdown()
    chatlog_watcher.stop()
    # Close the sink after every producer above has stopped, so the file
    # carries the complete publish stream including shutdown-path events.
    if events_sink is not None:
        events_sink.close()
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
                return JSONResponse(
                    {"detail": "Origin header required"}, status_code=403
                )
        elif origin and origin not in ALLOWED_API_ORIGINS:
            return JSONResponse({"detail": "Invalid Origin header"}, status_code=403)

        return await call_next(request)

    install_etag_middleware(app)

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
    app.include_router(events.router, prefix="/api")

    # Test-only surface, registered only when the test-mode overlay is active
    # (never in frozen builds): absent registration means a hard 404 in
    # production, and the schema surface (OpenAPI snapshot, contract suites,
    # the generated frontend client) is untouched because those derive from
    # an app built without test mode. Each handler re-checks the gate too.
    if _build_test_mode().enabled:
        app.include_router(testing.router, prefix="/api")

    return app


app = create_app()

if __name__ == "__main__":
    # Pass the app object directly: an import string would re-import
    # `backend.main` as a separate module from `__main__` in a frozen
    # bundle, doubling the lifespan-init work.
    uvicorn.run(app, host="127.0.0.1", port=BACKEND_PORT)
