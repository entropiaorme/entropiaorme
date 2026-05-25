"""Configuration service — typed settings with atomic persistence.

Settings stored as JSON in data/settings.json.
Atomic save: write to .tmp → os.replace() → keep .bak
"""

import contextlib
import json
import os
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

HOTBAR_SLOTS = [str(i) for i in range(1, 10)] + ["0"]
DEFAULT_TRIFECTA_PRESET_ID = "default"
DEFAULT_TRIFECTA_PRESET_NAME = "Default"


@dataclass
class TrifectaPresetConfig:
    id: str
    name: str
    small_weapon_id: int | None = None
    big_weapon_id: int | None = None
    heal_id: int | None = None


def active_trifecta_preset(config: "AppConfig") -> TrifectaPresetConfig | None:
    """Return the currently active trifecta preset, or None if not resolvable."""
    if not config.active_trifecta_preset_id:
        return None
    for preset in config.trifecta_presets:
        if preset.id == config.active_trifecta_preset_id:
            return preset
    return None


@dataclass
class AppConfig:
    """All user-configurable settings. Serialisation driven by this dataclass."""

    # Game connection
    chatlog_path: str = ""
    player_name: str = ""

    # Capability toggles — each independently controls one subsystem.
    # All default off: the user opts in to each automation as they configure it.
    hotbar_hooks_enabled: bool = False  # Hotbar slot-key listener
    repair_ocr_enabled: bool = False  # Post-session repair cost OCR.
    end_of_session_armour_reminder_enabled: bool = (
        False  # Yellow "Track armour?" prompt on session stop.
    )

    mob_tracking_mode: str = "mob"  # mob | tag
    mob_tracking_tag: str = ""
    manual_mob_species: str = ""
    manual_mob_maturity: str = ""

    # Hotbar — maps slot keys "1"-"9","0" to equipment_library IDs
    # None means the slot is empty
    hotbar: dict[str, int | None] = field(
        default_factory=lambda: dict.fromkeys(HOTBAR_SLOTS)
    )

    # Trifecta-attribution tool selection presets (small weapon, big weapon, heal tool)
    trifecta_presets: list[TrifectaPresetConfig] = field(
        default_factory=lambda: [
            TrifectaPresetConfig(
                id=DEFAULT_TRIFECTA_PRESET_ID, name=DEFAULT_TRIFECTA_PRESET_NAME
            )
        ]
    )
    active_trifecta_preset_id: str | None = DEFAULT_TRIFECTA_PRESET_ID

    # Loot filter — item names to exclude from tracking returns
    loot_filter_blacklist: list[str] = field(default_factory=lambda: ["Universal Ammo"])

    # Overlay window position (persisted across restarts)
    overlay_x: int | None = None
    overlay_y: int | None = None

    @staticmethod
    def default_chatlog_path() -> str:
        return str(Path.home() / "Documents" / "Entropia Universe" / "chat.log")


class ConfigService:
    def __init__(self, data_dir: Path):
        self.config_path = data_dir / "settings.json"
        self.config = self._load()

    def _load(self) -> AppConfig:
        if self.config_path.exists():
            try:
                data = json.loads(self.config_path.read_text(encoding="utf-8"))
                return self._from_dict(data)
            except (json.JSONDecodeError, KeyError):
                pass
        config = AppConfig(chatlog_path=AppConfig.default_chatlog_path())
        self._save(config)
        return config

    def _from_dict(self, data: dict) -> AppConfig:
        """Reconstruct AppConfig from stored JSON, handling missing/extra fields."""
        hotbar = self._normalize_hotbar(data.get("hotbar", {}))
        trifecta_presets, active_trifecta_preset_id = self._normalize_trifecta_presets(
            data.get("trifecta_presets"),
            active_id=data.get("active_trifecta_preset_id"),
        )
        config = AppConfig(
            chatlog_path=data.get("chatlog_path", AppConfig.default_chatlog_path()),
            player_name=data.get("player_name", ""),
            **self._migrate_capability_toggles(data),
            mob_tracking_mode=data.get("mob_tracking_mode", "mob"),
            mob_tracking_tag=data.get("mob_tracking_tag", ""),
            manual_mob_species=data.get("manual_mob_species", ""),
            manual_mob_maturity=data.get("manual_mob_maturity", ""),
            hotbar=hotbar,
            trifecta_presets=trifecta_presets,
            active_trifecta_preset_id=active_trifecta_preset_id,
            overlay_x=data.get("overlay_x"),
            overlay_y=data.get("overlay_y"),
            loot_filter_blacklist=data.get(
                "loot_filter_blacklist", AppConfig().loot_filter_blacklist
            ),
        )
        return config

    def _migrate_capability_toggles(self, data: dict) -> dict[str, bool]:
        return {
            "hotbar_hooks_enabled": bool(data.get("hotbar_hooks_enabled", False)),
            "repair_ocr_enabled": bool(data.get("repair_ocr_enabled", False)),
            "end_of_session_armour_reminder_enabled": bool(
                data.get("end_of_session_armour_reminder_enabled", False)
            ),
        }

    def _normalize_trifecta_presets(
        self,
        presets_raw: Any,
        *,
        active_id: str | None = None,
    ) -> tuple[list[TrifectaPresetConfig], str]:
        presets: list[TrifectaPresetConfig] = []
        seen_ids: set[str] = set()

        if isinstance(presets_raw, list):
            for index, raw in enumerate(presets_raw):
                if isinstance(raw, TrifectaPresetConfig):
                    preset = raw
                elif isinstance(raw, dict):
                    preset_id = str(raw.get("id") or "").strip()
                    if not preset_id:
                        continue
                    name = str(raw.get("name") or "").strip() or f"Preset {index + 1}"
                    preset = TrifectaPresetConfig(
                        id=preset_id,
                        name=name,
                        small_weapon_id=raw.get("small_weapon_id"),
                        big_weapon_id=raw.get("big_weapon_id"),
                        heal_id=raw.get("heal_id"),
                    )
                else:
                    continue

                if preset.id in seen_ids:
                    continue
                seen_ids.add(preset.id)
                presets.append(preset)

        if not presets:
            presets = [
                TrifectaPresetConfig(
                    id=DEFAULT_TRIFECTA_PRESET_ID, name=DEFAULT_TRIFECTA_PRESET_NAME
                )
            ]

        normalized_active_id = (
            active_id
            if active_id in {preset.id for preset in presets}
            else presets[0].id
        )
        return presets, normalized_active_id

    def _ensure_active_trifecta_preset(self, config: AppConfig) -> TrifectaPresetConfig:
        preset = active_trifecta_preset(config)
        if preset is not None:
            return preset
        fallback = TrifectaPresetConfig(
            id=DEFAULT_TRIFECTA_PRESET_ID, name=DEFAULT_TRIFECTA_PRESET_NAME
        )
        config.trifecta_presets = [fallback]
        config.active_trifecta_preset_id = fallback.id
        return fallback

    def _apply_updates(self, config: AppConfig, updates: dict[str, Any]) -> AppConfig:
        for key, value in updates.items():
            if not hasattr(config, key):
                continue
            if key == "hotbar":
                config.hotbar = self._normalize_hotbar(
                    {str(k): v for k, v in value.items()}
                )
            elif key == "trifecta_presets":
                config.trifecta_presets, config.active_trifecta_preset_id = (
                    self._normalize_trifecta_presets(
                        value,
                        active_id=config.active_trifecta_preset_id,
                    )
                )
            else:
                setattr(config, key, value)

        if "trifecta_presets" in updates or "active_trifecta_preset_id" in updates:
            self._ensure_active_trifecta_preset(config)

        return config

    def _normalize_hotbar(
        self, hotbar_raw: dict[str, int | None]
    ) -> dict[str, int | None]:
        """Fill any missing hotbar slots so config always has the full 1-9,0 shape."""
        return {slot: hotbar_raw.get(slot) for slot in HOTBAR_SLOTS}

    def _save(self, config: AppConfig) -> None:
        """Atomic save: write .tmp → os.replace → keep .bak.

        Merges with any unknown keys already on disk so values written by
        an extension (or third-party tooling) survive a save by a process
        that doesn't know about them.
        """
        self.config_path.parent.mkdir(parents=True, exist_ok=True)
        tmp_path = self.config_path.with_suffix(".tmp")
        bak_path = self.config_path.with_suffix(".bak")

        existing: dict[str, Any] = {}
        if self.config_path.exists():
            try:
                loaded = json.loads(self.config_path.read_text(encoding="utf-8"))
                if isinstance(loaded, dict):
                    existing = loaded
            except (json.JSONDecodeError, OSError):
                existing = {}

        merged: dict[str, Any] = {**existing, **asdict(config)}

        tmp_path.write_text(json.dumps(merged, indent=2), encoding="utf-8")

        if self.config_path.exists():
            with contextlib.suppress(OSError):
                os.replace(str(self.config_path), str(bak_path))
        os.replace(str(tmp_path), str(self.config_path))

    def get(self) -> AppConfig:
        return self.config

    def clone_with_updates(self, updates: dict[str, Any]) -> AppConfig:
        candidate = self._from_dict(asdict(self.config))
        return self._apply_updates(candidate, updates)

    def update(self, updates: dict[str, Any]) -> AppConfig:
        """Apply partial updates to config. Only known fields are accepted."""
        self._apply_updates(self.config, updates)
        self._save(self.config)
        return self.config

    def reset(self) -> AppConfig:
        self.config = AppConfig(chatlog_path=AppConfig.default_chatlog_path())
        self._save(self.config)
        return self.config

    def validate_chatlog(self) -> bool:
        """Check if the configured chat.log path exists and is readable."""
        return Path(self.config.chatlog_path).is_file()
