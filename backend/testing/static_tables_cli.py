"""Line-server oracle exposing the static-table functions.

The native port's differential tests drive this one JSON request per
line and compare the JSON reply byte-for-byte (both sides serialise
with sorted keys through their Python-faithful encoders). Part of the
equivalence oracle surface; never imported by production code.
"""

from __future__ import annotations

import json
import sys
from functools import lru_cache
from pathlib import Path

from backend.data import codex_categories, tt_value_curve
from backend.services import character_calc
from backend.testing.stdio import pin_utf8_line_protocol


@lru_cache(maxsize=1)
def _game_data():
    from backend.services.game_data_store import GameDataStore

    snapshot_dir = Path(__file__).resolve().parents[1] / "data" / "snapshot"
    return GameDataStore(snapshot_dir)


@lru_cache(maxsize=1)
def _mob_lookup():
    from backend.services.mob_lookup_service import MobLookupService

    return MobLookupService(_game_data())


def _dispatch(request: dict) -> object:
    op = request["op"]
    if op == "tt_value_at":
        return tt_value_curve.tt_value_at(request["level"])
    if op == "tt_value_of_gain":
        return tt_value_curve.tt_value_of_gain(
            request["from_level"], request["to_level"]
        )
    if op == "levels_for_tt_value":
        return tt_value_curve.levels_for_tt_value(
            request["from_level"], request["ped_value"]
        )
    if op == "max_tt_curve_level":
        return tt_value_curve.max_tt_curve_level()
    if op == "get_codex_category":
        return codex_categories.get_codex_category(request["skill_name"])
    if op == "game_search":
        return _game_data().search_entities(
            request["query"],
            endpoint=request.get("endpoint"),
            limit=request.get("limit", 50),
        )
    if op == "game_find":
        return _game_data().find_entity(request["endpoint"], request["item_id"])
    if op == "game_counts":
        return _game_data().endpoint_counts()
    if op == "mob_suggest":
        return _mob_lookup().search_mob_names(
            request["query"], limit=request.get("limit", 10)
        )
    if op == "mob_has":
        return _mob_lookup().has_mob_name(request["species"], request["maturity"])
    if op == "profession_level":
        return character_calc.profession_level(
            request["skill_levels"], request["profession"]
        )
    if op == "all_profession_levels":
        return character_calc.all_profession_levels(
            request["skill_levels"], request["professions"]
        )
    if op == "skill_rank":
        return character_calc.skill_rank(request["level"], request["ranks"])
    if op == "profession_skill_optimizer":
        return character_calc.profession_skill_optimizer(
            request["skill_levels"], request["profession"]
        )
    if op == "profession_path_optimizer":
        return character_calc.profession_path_optimizer(
            request["skill_levels"],
            request["profession"],
            target_level=request.get("target_level"),
            ped_budget=request.get("ped_budget"),
        )
    if op == "calculate_hp":
        return character_calc.calculate_hp(
            request["skill_levels"], request["skills_data"]
        )
    if op == "hp_skill_optimizer":
        return character_calc.hp_skill_optimizer(
            request["skill_levels"], request["skills_data"]
        )
    if op == "codex_next_reward":
        return character_calc.codex_next_reward(
            request["skill_name"], request["current_level"]
        )
    if op == "codex_tier_progress":
        return character_calc.codex_tier_progress(
            request["skill_name"], request["current_level"]
        )
    if op == "summarize_level_drift":
        from backend.services.scan_drift import summarize_level_drift

        return summarize_level_drift(
            request["tracked_levels"], request["scanned_levels"]
        )
    if op == "panel_anchors":
        from dataclasses import asdict

        from backend.services import scan_presets

        return {
            "skill": asdict(scan_presets.SKILL_ANCHOR),
            "profession": asdict(scan_presets.PROFESSION_ANCHOR),
            "repair": asdict(scan_presets.REPAIR_ANCHOR),
        }
    if op == "match_damage":
        from backend.tracking.tool_inference import DamageAttributor

        attributor = DamageAttributor()
        for profile in request["profiles"]:
            attributor.add_weapon_profile(
                name=profile["name"],
                min_damage=profile["min_damage"],
                max_damage=profile["max_damage"],
                base_damage=profile.get("base_damage", 0.0),
                cost_per_shot=profile.get("cost_per_shot", 0.0),
                role=profile.get("role"),
            )
        attribution = attributor.match_damage(
            request["amount"], critical=request.get("critical", False)
        )
        if attribution is None:
            return None
        return {
            "tool_name": attribution.tool_name,
            "cost_per_shot": attribution.cost_per_shot,
        }
    if op == "is_tracked_loot":
        from backend.tracking.loot_filter import is_tracked_loot, normalize_blacklist

        blacklist = normalize_blacklist(request.get("blacklist"))
        return is_tracked_loot(request["item_name"], blacklist)
    if op == "normalize_blacklist":
        from backend.tracking.loot_filter import normalize_blacklist

        return sorted(normalize_blacklist(request.get("names")))
    if op == "build_rank_breakdown":
        return codex_categories.build_rank_breakdown(
            request["base_cost"], request.get("codex_type")
        )
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    pin_utf8_line_protocol()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        result = _dispatch(json.loads(line))
        sys.stdout.write(json.dumps(result, sort_keys=True, ensure_ascii=False) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
