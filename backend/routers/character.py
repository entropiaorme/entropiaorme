"""Character stats endpoints — computed from calibrated skill levels + the bundled game-data catalogue."""

import time
from collections import defaultdict
from datetime import UTC, datetime
from typing import Any

from fastapi import APIRouter, HTTPException

from backend.data.codex_categories import get_codex_category
from backend.data.tt_value_curve import levels_for_tt_value, tt_value_at
from backend.dependencies import get_services
from backend.routers.response_models import CharacterProspect
from backend.services.character_calc import (
    ATTRIBUTE_SKILLS,
    all_profession_levels,
    codex_next_reward,
    codex_tier_progress,
    effective_points,
    hp_skill_optimizer,
    profession_level,
    profession_path_optimizer,
    profession_skill_optimizer,
    skill_rank,
)
from backend.services.session_summary import load_prospect_sessions

router = APIRouter(prefix="/character", tags=["character"])

# Skills are considered stale after 30 days without recalibration
STALE_DAYS = 30
PROSPECT_SAMPLE_WARN_SESSIONS = 3
PROSPECT_SAMPLE_WARN_HOURS = 2.0
PROSPECT_SAMPLE_WARN_CYCLED_PED = 50.0


# ── Helpers ────────────────────────────────────────────────────────────────────


def _get_skill_calibrations(app_db, source: str | None = None) -> dict[str, float]:
    """Return latest calibrated level per skill name from app_db.

    Default (source=None) returns believed-current — the latest row per skill
    regardless of source, which folds in chatlog and codex gains on top of the
    last scan anchor. Pass source='scan' to read the anchor snapshot only.

    Ordering: `MAX(scanned_at)` with `MAX(id)` as a tiebreaker. The
    tiebreaker only matters when two rows share a timestamp.
    """
    if source is None:
        sql = """
            WITH latest_ts AS (
                SELECT skill_name, MAX(scanned_at) AS ts
                FROM skill_calibrations
                GROUP BY skill_name
            )
            SELECT skill_name, level FROM skill_calibrations
            WHERE id IN (
                SELECT MAX(s2.id) FROM skill_calibrations s2
                JOIN latest_ts m ON s2.skill_name = m.skill_name AND s2.scanned_at = m.ts
                GROUP BY s2.skill_name
            )
        """
        params: tuple = ()
    else:
        sql = """
            WITH latest_ts AS (
                SELECT skill_name, MAX(scanned_at) AS ts
                FROM skill_calibrations
                WHERE source = ?
                GROUP BY skill_name
            )
            SELECT skill_name, level FROM skill_calibrations
            WHERE id IN (
                SELECT MAX(s2.id) FROM skill_calibrations s2
                JOIN latest_ts m ON s2.skill_name = m.skill_name AND s2.scanned_at = m.ts
                WHERE s2.source = ?
                GROUP BY s2.skill_name
            )
        """
        params = (source, source)
    with app_db.lock:
        rows = app_db.conn.execute(sql, params).fetchall()
    return {row["skill_name"]: row["level"] for row in rows}


def _get_anchor_skill_snapshot(app_db) -> dict[str, float]:
    """Return scan-anchor skill levels — the latest source='scan' row per skill.

    This is the single source of truth for "skills as of the last scan", used
    by the visibility surface (anchor column), the scan diff "before" snapshot,
    and any future surfaces that need to compare believed-current to the
    anchor.
    """
    return _get_skill_calibrations(app_db, source="scan")


def _get_last_calibration_ts(app_db) -> float | None:
    """Return epoch timestamp of the most recent calibration, or None."""
    with app_db.lock:
        row = app_db.conn.execute(
            "SELECT MAX(scanned_at) as ts FROM skill_calibrations"
        ).fetchone()
    ts = row["ts"] if row else None
    return float(ts) if ts is not None else None


def _get_ranks(game_data) -> list[dict]:
    """Return sorted list of {name, skill} rank thresholds."""
    entities = game_data.get_entities("skill_ranks")
    if not entities:
        return []
    # skill_ranks endpoint returns a single object with table.rows
    rows = entities[0].get("table", {}).get("rows", [])
    valid_rows = []
    for row in rows:
        threshold = row.get("skill")
        name = row.get("name")
        if threshold is None or name is None:
            continue
        try:
            threshold = float(threshold)
        except (TypeError, ValueError):
            continue
        valid_rows.append({"name": name, "skill": threshold})
    return sorted(valid_rows, key=lambda r: r["skill"])


def _get_profession_entity(game_data, profession_name: str) -> dict | None:
    for prof in game_data.get_entities("professions"):
        if prof["name"] == profession_name:
            return prof
    return None


def _load_prospect_sessions(app_db) -> list[dict]:
    """Return completed session summaries for Prospect analytics.

    Reads the materialised `session_summaries` cache; missing or
    stale-version rows are rebuilt lazily. See
    `backend.services.session_summary` for the cache contract.
    """
    return load_prospect_sessions(app_db.conn)


def _prospect_sample(sessions: list[dict]) -> dict:
    regular_skill_ped: dict[str, float] = defaultdict(float)
    attribute_levels: dict[str, float] = defaultdict(float)

    sample = {
        "sessions": len(sessions),
        "kills": sum(session["kills"] for session in sessions),
        "hours": round(sum(session["durationHours"] for session in sessions), 4),
        "cycledPed": round(sum(session["cycledPed"] for session in sessions), 4),
        "lootTt": round(sum(session["lootTt"] for session in sessions), 4),
        "pes": round(sum(session["regularSkillTt"] for session in sessions), 4),
        "attributeLevels": round(
            sum(session["attributeLevelsTotal"] for session in sessions), 4
        ),
    }

    for session in sessions:
        for name, ped in session["regularSkillPed"].items():
            regular_skill_ped[name] += ped
        for name, amount in session["attributeLevels"].items():
            attribute_levels[name] += amount

    cycled = sample["cycledPed"]
    sample["cycledPerHour"] = (
        round(sample["cycledPed"] / sample["hours"], 4) if sample["hours"] > 0 else 0.0
    )
    sample["lootPerHour"] = (
        round(sample["lootTt"] / sample["hours"], 4) if sample["hours"] > 0 else 0.0
    )
    sample["returnRate"] = round(sample["lootTt"] / cycled, 4) if cycled > 0 else 0.0
    sample["pesPerPed"] = round(sample["pes"] / cycled, 6) if cycled > 0 else 0.0
    sample["lootTtPerPed"] = round(sample["lootTt"] / cycled, 6) if cycled > 0 else 0.0
    sample["skillShares"] = {
        name: ped / sample["pes"]
        for name, ped in regular_skill_ped.items()
        if sample["pes"] > 0 and ped > 0
    }
    sample["attributeRates"] = {
        name: amount / cycled
        for name, amount in attribute_levels.items()
        if cycled > 0 and amount > 0
    }
    return sample


def _prospect_option_list(sessions: list[dict], key: str) -> list[dict]:
    grouped: dict[str, list[dict]] = defaultdict(list)
    for session in sessions:
        value = session.get(key)
        if value:
            grouped[value].append(session)

    options = []
    for value, grouped_sessions in grouped.items():
        sample = _prospect_sample(grouped_sessions)
        options.append(
            {
                "value": value,
                "label": value,
                "sessions": sample["sessions"],
                "kills": sample["kills"],
                "hours": round(sample["hours"], 2),
                "cycledPed": round(sample["cycledPed"], 2),
            }
        )

    options.sort(
        key=lambda option: (-option["sessions"], -option["cycledPed"], option["label"])
    )
    return options


def _match_prospect_sessions(
    sessions: list[dict],
    slice_type: str,
    slice_value: str | None,
) -> list[dict]:
    if slice_type == "global":
        return sessions
    if not slice_value:
        return []

    key_map = {
        "tag": "dominantTag",
        "mob": "dominantMob",
        "weapon": "dominantWeapon",
    }
    key = key_map.get(slice_type)
    if key is None:
        return []
    return [session for session in sessions if session.get(key) == slice_value]


def _build_prospect_warnings(sample: dict, projected_cycled_ped: float) -> list[str]:
    warnings = []
    if sample["sessions"] < PROSPECT_SAMPLE_WARN_SESSIONS:
        warnings.append("Thin sample: fewer than 3 matching sessions.")
    if sample["hours"] < PROSPECT_SAMPLE_WARN_HOURS:
        warnings.append("Thin sample: less than 2 hours of matching play.")
    if sample["cycledPed"] < PROSPECT_SAMPLE_WARN_CYCLED_PED:
        warnings.append("Thin sample: less than 50 PED of matching cycling.")
    if sample["cycledPed"] > 0 and projected_cycled_ped > sample["cycledPed"] * 20:
        warnings.append(
            "Long extrapolation: forecast extends far beyond the observed sample."
        )
    return warnings


def _project_prospect_levels(
    skill_levels: dict[str, float],
    sample: dict,
    total_ped: float,
) -> tuple[dict[str, float], dict[str, float]]:
    projected_levels = {name: float(level) for name, level in skill_levels.items()}
    projected_gains: dict[str, float] = {}

    skill_tt_budget = total_ped * sample["pesPerPed"]
    for skill_name, share in sample["skillShares"].items():
        current_level = projected_levels.get(skill_name, 0.0)
        allocated_tt = skill_tt_budget * share
        gained_levels = levels_for_tt_value(current_level, allocated_tt)
        projected_levels[skill_name] = round(current_level + gained_levels, 4)
        projected_gains[skill_name] = round(gained_levels, 4)

    for skill_name, rate in sample["attributeRates"].items():
        current_level = projected_levels.get(skill_name, 0.0)
        gained_levels = total_ped * rate
        projected_levels[skill_name] = round(current_level + gained_levels, 4)
        projected_gains[skill_name] = round(gained_levels, 4)

    return projected_levels, projected_gains


def _relevant_prospect_progress(sample: dict, profession: dict) -> bool:
    observed_regular = set(sample["skillShares"])
    observed_attrs = set(sample["attributeRates"])
    for skill_entry in profession.get("skills", []):
        skill_obj = skill_entry.get("skill") or {}
        name = skill_obj.get("name", "")
        weight = skill_entry.get("weight") or 0
        if not name or weight <= 0:
            continue
        if name in observed_regular or name in observed_attrs:
            return True
    return False


def _build_prospect_result(
    profession_name: str,
    profession: dict,
    skill_levels: dict[str, float],
    target_level: float,
    sample: dict,
    slice_type: str,
    slice_value: str | None,
    markup_uplift: float,
) -> dict:
    current_level = profession_level(skill_levels, profession)

    if target_level <= current_level:
        projected_levels = {name: float(level) for name, level in skill_levels.items()}
        projected_gains: dict[str, float] = {}
        projected_cycled_ped = 0.0
    else:
        if sample["cycledPed"] <= 0 or sample["hours"] <= 0:
            return {
                "profession": profession_name,
                "sliceType": slice_type,
                "sliceValue": slice_value,
                "markupUplift": markup_uplift,
                "currentLevel": round(current_level, 2),
                "targetLevel": round(target_level, 2),
                "projectedCycledPed": 0.0,
                "projectedHours": 0.0,
                "expectedLootTt": 0.0,
                "expectedNetTtBurn": 0.0,
                "speculativeLootTt": None,
                "speculativeNetTtBurn": None,
                "sample": sample,
                "rows": [],
                "warnings": [],
                "error": "Insufficient matching data for a forecast.",
            }

        if not _relevant_prospect_progress(sample, profession):
            return {
                "profession": profession_name,
                "sliceType": slice_type,
                "sliceValue": slice_value,
                "markupUplift": markup_uplift,
                "currentLevel": round(current_level, 2),
                "targetLevel": round(target_level, 2),
                "projectedCycledPed": 0.0,
                "projectedHours": 0.0,
                "expectedLootTt": 0.0,
                "expectedNetTtBurn": 0.0,
                "speculativeLootTt": None,
                "speculativeNetTtBurn": None,
                "sample": sample,
                "rows": [],
                "warnings": [],
                "error": "The observed sample does not contain gains that move this profession.",
            }

        lower = 0.0
        upper = max(sample["cycledPed"], 1.0)
        projected_levels, projected_gains = _project_prospect_levels(
            skill_levels, sample, upper
        )
        upper_level = profession_level(projected_levels, profession)
        while upper_level < target_level and upper < 1_000_000_000:
            lower = upper
            upper *= 2
            projected_levels, projected_gains = _project_prospect_levels(
                skill_levels, sample, upper
            )
            upper_level = profession_level(projected_levels, profession)

        if upper_level < target_level:
            return {
                "profession": profession_name,
                "sliceType": slice_type,
                "sliceValue": slice_value,
                "markupUplift": markup_uplift,
                "currentLevel": round(current_level, 2),
                "targetLevel": round(target_level, 2),
                "projectedCycledPed": 0.0,
                "projectedHours": 0.0,
                "expectedLootTt": 0.0,
                "expectedNetTtBurn": 0.0,
                "speculativeLootTt": None,
                "speculativeNetTtBurn": None,
                "sample": sample,
                "rows": [],
                "warnings": [],
                "error": "Target is outside the reachable forecast range for this sample.",
            }

        for _ in range(60):
            mid = (lower + upper) / 2
            test_levels, _ = _project_prospect_levels(skill_levels, sample, mid)
            if profession_level(test_levels, profession) >= target_level:
                upper = mid
            else:
                lower = mid

        projected_cycled_ped = round(upper, 2)
        projected_levels, projected_gains = _project_prospect_levels(
            skill_levels, sample, projected_cycled_ped
        )

    expected_loot_tt = round(projected_cycled_ped * sample["lootTtPerPed"], 2)
    expected_net_tt_burn = round(projected_cycled_ped - expected_loot_tt, 2)
    projected_hours = (
        round(
            projected_cycled_ped * (sample["hours"] / sample["cycledPed"]),
            2,
        )
        if sample["cycledPed"] > 0
        else 0.0
    )

    speculative_loot_tt = None
    speculative_net_tt_burn = None
    if markup_uplift > 0:
        speculative_loot_tt = round(expected_loot_tt * (1 + markup_uplift), 2)
        speculative_net_tt_burn = round(projected_cycled_ped - speculative_loot_tt, 2)

    weights = {
        (skill_entry.get("skill") or {}).get("name", ""): float(
            skill_entry.get("weight") or 0
        )
        for skill_entry in profession.get("skills", [])
    }

    row_names = set(sample["skillShares"]) | set(sample["attributeRates"])
    rows = []
    for name in row_names:
        current_skill_level = float(skill_levels.get(name, 0.0))
        projected_gain = float(projected_gains.get(name, 0.0))
        projected_end_level = float(projected_levels.get(name, current_skill_level))
        weight = float(weights.get(name, 0.0))
        is_attribute = name in ATTRIBUTE_SKILLS
        contribution = (
            (effective_points(name, projected_gain) * weight) / 10000
            if weight > 0
            else 0.0
        )
        rows.append(
            {
                "name": name,
                "isAttribute": is_attribute,
                "weight": weight,
                "currentLevel": round(current_skill_level, 2),
                "observedShare": round(sample["skillShares"].get(name, 0.0), 4),
                "observedRate": round(sample["attributeRates"].get(name, 0.0), 6),
                "projectedGain": round(projected_gain, 2),
                "projectedEndLevel": round(projected_end_level, 2),
                "professionContribution": round(contribution, 4),
                "relevant": weight > 0,
            }
        )

    rows.sort(
        key=lambda row: (
            -row["professionContribution"],
            row["isAttribute"],
            row["name"],
        )
    )

    return {
        "profession": profession_name,
        "sliceType": slice_type,
        "sliceValue": slice_value,
        "markupUplift": markup_uplift,
        "currentLevel": round(current_level, 2),
        "targetLevel": round(target_level, 2),
        "projectedCycledPed": projected_cycled_ped,
        "projectedHours": projected_hours,
        "expectedLootTt": expected_loot_tt,
        "expectedNetTtBurn": expected_net_tt_burn,
        "speculativeLootTt": speculative_loot_tt,
        "speculativeNetTtBurn": speculative_net_tt_burn,
        "sample": sample,
        "rows": rows,
        "warnings": _build_prospect_warnings(sample, projected_cycled_ped),
    }


# ── Endpoints ──────────────────────────────────────────────────────────────────


@router.get("/calibration")
def get_calibration():
    """Return skill calibration status."""
    svc = get_services()
    last_ts = _get_last_calibration_ts(svc.app_db)

    if last_ts is None:
        return {"calibrated": False, "lastCalibration": None, "stale": True}

    age_days = (time.time() - last_ts) / 86400
    stale = age_days > STALE_DAYS

    return {
        "calibrated": True,
        "lastCalibration": (
            datetime.fromtimestamp(last_ts, tz=UTC).isoformat()
            if last_ts is not None
            else None
        ),
        "stale": stale,
    }


@router.get("/stats")
def get_character_stats():
    """Return HP and top profession levels.

    HP is read directly from the scanned Health attribute level
    (Health is an attribute whose in-game value equals the player's HP).
    """
    svc = get_services()
    skill_levels = _get_skill_calibrations(svc.app_db)

    hp = int(skill_levels.get("Health", 0))

    professions_data = svc.game_data.get_entities("professions")
    levels_by_name = all_profession_levels(skill_levels, professions_data)
    prof_levels = []
    for prof in professions_data:
        level = levels_by_name.get(prof["name"], 0.0)
        if level > 0:
            category = prof.get("category", "General")
            prof_levels.append(
                {"name": prof["name"], "level": level, "category": category}
            )

    prof_levels.sort(key=lambda p: p["level"], reverse=True)
    top = prof_levels[:5]

    return {"hp": hp, "topProfessions": top}


@router.get("/skills")
def get_skills():
    """Return calibrated skill levels with ranks and TT values.

    Each row carries `level` (believed-current — anchor + chatlog/codex gains
    since), `anchorLevel` (latest source='scan' row, or null if never scanned),
    and `gainSinceAnchor` (level - anchorLevel, or null when no anchor exists).
    """
    svc = get_services()
    skill_levels = _get_skill_calibrations(svc.app_db)

    if not skill_levels:
        return []

    anchor_levels = _get_anchor_skill_snapshot(svc.app_db)
    skills_data = svc.game_data.get_entities("skills")
    ranks = _get_ranks(svc.game_data)

    skill_map = {s["name"]: s for s in skills_data}

    result: list[dict[str, Any]] = []
    for name, level in skill_levels.items():
        entity = skill_map.get(name, {})
        category_obj = entity.get("category") or {}
        category = (
            category_obj.get("name", "General")
            if isinstance(category_obj, dict)
            else "General"
        )
        rank_name = skill_rank(level, ranks)
        tt = tt_value_at(level)
        anchor = anchor_levels.get(name)
        gain = round(level - anchor, 4) if anchor is not None else None
        result.append(
            {
                "name": name,
                "category": category,
                "level": level,
                "anchorLevel": anchor,
                "gainSinceAnchor": gain,
                "rankName": rank_name,
                "ttValue": round(tt, 2),
                "isAttribute": name in ATTRIBUTE_SKILLS,
            }
        )

    result.sort(key=lambda s: s["level"], reverse=True)
    return result


@router.get("/professions")
def get_professions():
    """Return profession levels with anchor and gain-since-anchor breakdown.

    Both `level` (believed-current) and `anchorLevel` (last scan) are computed
    via `profession_level()` against believed-current and anchor skills
    respectively, so the running-sum's contribution is visible per profession.
    The optimizer / prospect / codex paths compute the same way; the formula
    is the canonical source for profession levels.
    """
    svc = get_services()
    professions_data = svc.game_data.get_entities("professions")
    if not professions_data:
        return []

    skill_levels = _get_skill_calibrations(svc.app_db)
    anchor_skills = _get_anchor_skill_snapshot(svc.app_db)
    current_levels = all_profession_levels(skill_levels, professions_data)
    anchor_levels = all_profession_levels(anchor_skills, professions_data)
    has_anchor = bool(anchor_skills)

    result = []
    for prof in professions_data:
        name = prof["name"]
        level = current_levels.get(name, 0.0)
        category = prof.get("category", "General")
        anchor = anchor_levels.get(name, 0.0) if has_anchor else None
        gain = round(level - anchor, 4) if anchor is not None else None
        result.append(
            {
                "name": name,
                "level": level,
                "anchorLevel": anchor,
                "gainSinceAnchor": gain,
                "category": category,
            }
        )

    result.sort(key=lambda p: p["level"], reverse=True)
    return result


@router.get("/prospect-options")
def get_character_prospect_options():
    """Return slice values available for Prospect forecasts."""
    svc = get_services()
    sessions = _load_prospect_sessions(svc.app_db)
    return {
        "tags": _prospect_option_list(sessions, "dominantTag"),
        "mobs": _prospect_option_list(sessions, "dominantMob"),
        "weapons": _prospect_option_list(sessions, "dominantWeapon"),
    }


@router.get(
    "/prospect",
    response_model=CharacterProspect,
    response_model_exclude_unset=True,
)
def get_character_prospect(
    profession: str,
    target_level: float,
    slice_type: str = "global",
    slice_value: str | None = None,
    markup_uplift: float = 0.0,
):
    """Forecast profession progression using observed session skill composition."""
    if target_level <= 0:
        raise HTTPException(status_code=422, detail="target_level must be positive")
    if markup_uplift < 0:
        raise HTTPException(
            status_code=422, detail="markup_uplift must be zero or positive"
        )
    if slice_type not in {"global", "tag", "mob", "weapon"}:
        raise HTTPException(
            status_code=422, detail="slice_type must be global, tag, mob, or weapon"
        )
    if slice_type != "global" and not slice_value:
        raise HTTPException(
            status_code=422, detail="slice_value is required for non-global slices"
        )

    svc = get_services()
    profession_entity = _get_profession_entity(svc.game_data, profession)
    if profession_entity is None:
        return {
            "error": f"Profession '{profession}' not found",
            "rows": [],
            "warnings": [],
        }

    skill_levels = _get_skill_calibrations(svc.app_db)
    sessions = _load_prospect_sessions(svc.app_db)
    matched_sessions = _match_prospect_sessions(sessions, slice_type, slice_value)
    sample = _prospect_sample(matched_sessions)
    return _build_prospect_result(
        profession,
        profession_entity,
        skill_levels,
        target_level,
        sample,
        slice_type,
        slice_value,
        markup_uplift,
    )


@router.get("/profession-optimizer")
def get_profession_optimizer(profession: str):
    """Return skills ranked by PED cost to next profession level.

    Regular skills are ranked by how much PED of skill TT it would take
    for that skill alone to push the profession to its next integer level.
    Attributes are listed separately by raw contribution factor.
    """
    svc = get_services()
    professions_data = svc.game_data.get_entities("professions")

    prof_entity = None
    for p in professions_data:
        if p["name"] == profession:
            prof_entity = p
            break

    if prof_entity is None:
        return {
            "error": f"Profession '{profession}' not found",
            "skills": [],
            "attributes": [],
        }

    skill_levels = _get_skill_calibrations(svc.app_db)
    result = profession_skill_optimizer(skill_levels, prof_entity)
    result["profession"] = profession
    return result


@router.get("/profession-path-optimizer")
def get_profession_path_optimizer(
    profession: str,
    target_level: float | None = None,
    ped_budget: float | None = None,
):
    """Find the cheapest skill allocation to reach a target profession level,
    or the best allocation for a given PED budget."""
    if (target_level is None) == (ped_budget is None):
        raise HTTPException(
            status_code=422,
            detail="Exactly one of target_level or ped_budget must be provided",
        )

    svc = get_services()
    professions_data = svc.game_data.get_entities("professions")

    prof_entity = None
    for p in professions_data:
        if p["name"] == profession:
            prof_entity = p
            break

    if prof_entity is None:
        return {
            "error": f"Profession '{profession}' not found",
            "allocations": [],
            "attributes": [],
        }

    skill_levels = _get_skill_calibrations(svc.app_db)
    result = profession_path_optimizer(
        skill_levels,
        prof_entity,
        target_level=target_level,
        ped_budget=ped_budget,
    )
    result["profession"] = profession
    return result


@router.get("/hp-optimizer")
def get_hp_optimizer():
    """Return skills ranked by PED cost per +1 HP.

    Regular skills are ranked by how much PED of skill TT it costs to gain
    1 HP through that skill alone at the player's current level.
    Attributes are listed separately by HP contribution factor.
    """
    svc = get_services()
    skill_levels = _get_skill_calibrations(svc.app_db)
    skills_data = svc.game_data.get_entities("skills")
    return hp_skill_optimizer(skill_levels, skills_data)


@router.get("/codex")
def get_codex():
    """Return codex progress predictions for calibrated skills in codex categories."""
    svc = get_services()
    skill_levels = _get_skill_calibrations(svc.app_db)

    result: list[dict[str, Any]] = []
    for name, level in skill_levels.items():
        if get_codex_category(name) is None:
            continue
        next_reward = codex_next_reward(name, level)
        progress = codex_tier_progress(name, level)
        if next_reward is not None and progress is not None:
            result.append(
                {
                    "skillName": name,
                    "currentLevel": level,
                    "nextRewardValue": round(next_reward, 2),
                    "progress": progress,
                }
            )

    result.sort(key=lambda c: c["currentLevel"], reverse=True)
    return result
