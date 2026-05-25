"""Dependency injection — global service container initialised at startup."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from backend.db.app_database import AppDatabase
    from backend.services.game_data_store import GameDataStore
    from backend.services.mob_lookup_service import MobLookupService
    from backend.services.config_service import ConfigService
    from backend.core.event_bus import EventBus
    from backend.tracking.tracker import HuntTracker
    from backend.services.chatlog_watcher import ChatlogWatcher
    from backend.services.skill_tracker import SkillTracker
    from backend.services.skill_scan_manual import SkillScanManual
    from backend.services.codex_service import CodexService
    from backend.services.quest_service import QuestService
    from backend.services.hotbar_listener import HotbarListener
    from backend.services.repair_ocr import RepairOcrService
    from backend.services.spacebar_capture_listener import SpacebarCaptureListener


@dataclass
class Services:
    """Container for all backend services. Created once at startup."""

    app_db: AppDatabase
    game_data: GameDataStore
    mob_lookup: MobLookupService
    config_service: ConfigService
    event_bus: EventBus
    tracker: HuntTracker
    chatlog_watcher: ChatlogWatcher
    skill_tracker: SkillTracker
    skill_scan_manual: SkillScanManual
    codex_service: CodexService
    quest_service: QuestService
    hotbar_listener: HotbarListener
    repair_ocr: RepairOcrService
    spacebar_capture_listener: SpacebarCaptureListener


_services: Services | None = None


def set_services(s: Services) -> None:
    global _services
    _services = s


def get_services() -> Services:
    assert _services is not None, "Services not initialised — lifespan not started"
    return _services
