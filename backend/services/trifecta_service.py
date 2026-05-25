"""Trifecta resolution and validation helpers (used for damage-band attribution)."""

from __future__ import annotations

import json

from backend.services.cost_engine import (
    cost_per_shot_from_props,
    get_weapon_damage_profile,
    heal_cost_per_use,
    heal_range_at_max_skill,
    heal_reload_seconds,
)


def _ranges_overlap(
    first_min: float, first_max: float, second_min: float, second_max: float
) -> bool:
    return max(first_min, second_min) <= min(first_max, second_max)


def _format_range(minimum: float, maximum: float) -> str:
    return f"{minimum:.1f}-{maximum:.1f}"


def describe_trifecta(conn, preset) -> tuple[dict | None, str | None]:
    """Resolve a trifecta preset into tracking-ready data plus validation."""
    if preset is None:
        return None, "Trifecta attribution requires an active preset"
    ids = {
        "small_weapon": preset.small_weapon_id,
        "big_weapon": preset.big_weapon_id,
        "heal_tool": preset.heal_id,
    }
    if any(value is None for value in ids.values()):
        return (
            None,
            "Trifecta attribution requires a configured small weapon, big weapon, and healing tool",
        )

    result: dict[str, dict] = {}

    for key, label in (("small_weapon", "small weapon"), ("big_weapon", "big weapon")):
        row = conn.execute(
            "SELECT id, name, properties_json FROM equipment_library WHERE id = ? AND item_type = 'weapon'",
            (ids[key],),
        ).fetchone()
        if row is None:
            return (
                None,
                f"Trifecta attribution {label} is not found in the equipment library",
            )

        props = json.loads(row["properties_json"])
        damage_enhancers = max(0, int(props.get("damage_enhancers", 0) or 0))
        damage_profile = get_weapon_damage_profile(
            props["weapon_entity"],
            amp=props.get("amp_entity"),
            damage_enhancers=damage_enhancers,
        )
        if damage_profile is None:
            return (
                None,
                f"Trifecta attribution {label} does not expose a usable damage range",
            )

        cost_result = cost_per_shot_from_props(props)
        result[key] = {
            "id": row["id"],
            "name": row["name"],
            "role": key,
            "cost_per_shot_ped": cost_result["totalCostPerUse"] / 100.0,
            "damage_min": damage_profile["damageMin"],
            "damage_max": damage_profile["damageMax"],
            "total_damage": damage_profile["totalDamage"],
            "weapon_props": props,
        }

    small = result["small_weapon"]
    big = result["big_weapon"]
    if _ranges_overlap(
        small["damage_min"],
        small["damage_max"],
        big["damage_min"],
        big["damage_max"],
    ):
        return None, (
            "Trifecta attribution requires non-overlapping small/big weapon ranges "
            f"({small['name']}: {_format_range(small['damage_min'], small['damage_max'])}, "
            f"{big['name']}: {_format_range(big['damage_min'], big['damage_max'])})"
        )

    heal_row = conn.execute(
        "SELECT id, name, properties_json FROM equipment_library WHERE id = ? AND item_type = 'healing'",
        (ids["heal_tool"],),
    ).fetchone()
    if heal_row is None:
        return (
            None,
            "Trifecta attribution healing tool is not found in the equipment library",
        )

    heal_props = json.loads(heal_row["properties_json"])
    markup = heal_props.get("markup", 100) / 100.0
    heal_interval = heal_range_at_max_skill(heal_props["tool_entity"])
    result["heal_tool"] = {
        "id": heal_row["id"],
        "name": heal_row["name"],
        "cost_per_use_ped": heal_cost_per_use(heal_props["tool_entity"], markup)
        / 100.0,
        "reload_seconds": heal_reload_seconds(heal_props["tool_entity"]),
        "heal_min": heal_interval["min"] if heal_interval else None,
        "heal_max": heal_interval["max"] if heal_interval else None,
    }

    return result, None


def validate_trifecta(conn, preset) -> tuple[bool, str | None]:
    trifecta, error = describe_trifecta(conn, preset)
    return trifecta is not None, error
