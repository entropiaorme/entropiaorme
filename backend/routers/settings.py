"""Settings endpoints — config persistence + cache status assembly."""

import logging
from pathlib import Path

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from backend.dependencies import get_services
from backend.routers.response_models import AppSettings, OkResponse, OverlayPosition
from backend.services.trifecta_service import validate_trifecta

router = APIRouter(prefix="/settings", tags=["settings"])
log = logging.getLogger(__name__)

APP_VERSION = "0.1.0"


def _build_trifecta_response(config, conn) -> dict:
    presets = []
    active_ready = False
    active_message = None
    active_name = None

    for preset in config.trifecta_presets:
        ready, message = validate_trifecta(conn, preset)
        payload = {
            "id": preset.id,
            "name": preset.name,
            "smallWeaponId": preset.small_weapon_id,
            "bigWeaponId": preset.big_weapon_id,
            "healId": preset.heal_id,
            "ready": ready,
            "message": message,
        }
        presets.append(payload)
        if preset.id == config.active_trifecta_preset_id:
            active_ready = ready
            active_message = message
            active_name = preset.name

    return {
        "activePresetId": config.active_trifecta_preset_id,
        "activePresetName": active_name,
        "presets": presets,
        "ready": active_ready,
        "message": active_message,
    }


def _build_response(svc) -> dict:
    """Assemble AppSettings shape for the frontend."""
    config = svc.config_service.get()
    chatlog_valid = svc.config_service.validate_chatlog()
    trifecta = _build_trifecta_response(config, svc.app_db.conn)

    return {
        "gameConnection": {
            "chatLogPath": config.chatlog_path,
            "chatLogValid": chatlog_valid,
            "playerName": config.player_name,
        },
        "hotbarHooksEnabled": config.hotbar_hooks_enabled,
        "repairOcrEnabled": config.repair_ocr_enabled,
        "endOfSessionArmourReminderEnabled": config.end_of_session_armour_reminder_enabled,
        "developerModeEnabled": config.developer_mode_enabled,
        "mobTrackingMode": config.mob_tracking_mode,
        "mobTrackingTag": config.mob_tracking_tag,
        "hotbar": config.hotbar,
        "trifecta": trifecta,
        "lootFilterBlacklist": config.loot_filter_blacklist,
        "dbPath": str(svc.app_db.db_path),
        "appVersion": APP_VERSION,
    }


def _validate_chatlog_path(value: str) -> str:
    if not value:
        raise HTTPException(status_code=400, detail="chat.log path is required")
    path = Path(value).expanduser()
    if path.name.lower() != "chat.log":
        raise HTTPException(
            status_code=400, detail="chat.log path must point to a chat.log file"
        )
    if not path.is_file():
        raise HTTPException(status_code=400, detail="chat.log path does not exist")
    return str(path)


@router.get("", response_model=AppSettings)
def get_settings():
    """Return full settings including live cache stats."""
    svc = get_services()
    return _build_response(svc)


class SettingsPatch(BaseModel):
    chatlog_path: str | None = None
    player_name: str | None = None
    hotbar_hooks_enabled: bool | None = None
    repair_ocr_enabled: bool | None = None
    end_of_session_armour_reminder_enabled: bool | None = None
    developer_mode_enabled: bool | None = None
    mob_tracking_mode: str | None = None
    mob_tracking_tag: str | None = None
    hotbar: dict[str, int | None] | None = None
    active_trifecta_preset_id: str | None = None
    trifecta_presets: list[dict] | None = None
    loot_filter_blacklist: list[str] | None = None


@router.patch("", response_model=AppSettings)
def update_settings(patch: SettingsPatch):
    """Partial update — only provided fields are written."""
    svc = get_services()
    updates = patch.model_dump(exclude_unset=True)
    if not updates:
        raise HTTPException(status_code=400, detail="No fields to update")

    candidate = svc.config_service.clone_with_updates(updates)

    if "chatlog_path" in updates:
        updates["chatlog_path"] = _validate_chatlog_path(updates["chatlog_path"])
        candidate.chatlog_path = updates["chatlog_path"]

    if candidate.mob_tracking_mode not in ("mob", "tag"):
        raise HTTPException(status_code=400, detail="Unknown mob tracking mode")

    # Attribution-mechanism readiness is enforced at start-tracking time, not at
    # toggle time: the user can flip mechanisms freely; the start-session gate
    # surfaces a yellow warning if the active mechanism isn't usable.
    svc.config_service.update(updates)

    # Restart chatlog watcher if the path changed
    if "chatlog_path" in updates:
        svc.chatlog_watcher.restart(updates["chatlog_path"])

    if "hotbar_hooks_enabled" in updates:
        svc.hotbar_listener.apply_config(
            hotbar_hooks_enabled=svc.config_service.get().hotbar_hooks_enabled,
        )
    svc.tracker.reload_config()

    return _build_response(svc)


@router.post("/reset", response_model=AppSettings)
def reset_settings():
    """Reset all settings to defaults."""
    svc = get_services()
    svc.config_service.reset()
    cfg = svc.config_service.get()
    svc.chatlog_watcher.restart(cfg.chatlog_path)
    svc.hotbar_listener.apply_config(hotbar_hooks_enabled=cfg.hotbar_hooks_enabled)
    svc.tracker.reload_config()
    return _build_response(svc)


class OverlayPositionPatch(BaseModel):
    x: int
    y: int


@router.get("/overlay-position", response_model=OverlayPosition)
def get_overlay_position():
    config = get_services().config_service.get()
    return {"x": config.overlay_x, "y": config.overlay_y}


@router.put("/overlay-position", response_model=OkResponse)
def set_overlay_position(body: OverlayPositionPatch):
    svc = get_services()
    svc.config_service.update({"overlay_x": body.x, "overlay_y": body.y})
    return {"ok": True}
