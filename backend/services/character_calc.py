"""Character calculation service — profession levels, skill ranks, HP, codex prediction.

Pure functions — no I/O. Call these with data already loaded from the cache.

The catalogue payload still arrives in nested-dict form. Two small adapter
helpers (`_iter_profession_skills`, `_iter_hp_skills`) hide that shape from
the math layer so the public functions read as straight formulas.
"""

import bisect
from collections.abc import Iterator

from backend.data.tt_value_curve import (
    levels_for_tt_value,
    max_tt_curve_level,
    tt_value_at,
)
from backend.data.codex_categories import get_codex_category, REWARD_DIVISORS

# Attribute skills receive a ×20 multiplier in profession calculations
ATTRIBUTE_SKILLS = {
    "Agility",
    "Health",
    "Intelligence",
    "Psyche",
    "Stamina",
    "Strength",
}


def effective_points(skill_name: str, level: float) -> float:
    """Apply the ×20 multiplier for attribute skills."""
    return level * 20 if skill_name in ATTRIBUTE_SKILLS else level


def _iter_profession_skills(profession: dict) -> Iterator[tuple[str, float]]:
    """Yield (skill_name, weight) for each skill entry on a profession.

    Entries with no usable skill name are skipped. A missing weight is
    surfaced as 0 so per-entry contributions are still correct; callers
    that care about non-zero-weight skills filter on the yielded value.
    """
    for entry in profession.get("skills") or []:
        if not isinstance(entry, dict):
            continue
        skill_obj = entry.get("skill") or {}
        name = skill_obj.get("name") or ""
        if not name:
            continue
        try:
            weight = float(entry.get("weight") or 0)
        except (TypeError, ValueError):
            weight = 0.0
        yield name, weight


def _iter_hp_skills(skills_data: list[dict]) -> Iterator[tuple[str, float]]:
    """Yield (skill_name, hp_increase) for skills that contribute to HP.

    Skills with a missing or non-positive hp_increase are skipped, so
    callers can iterate without re-checking the contribution flag.
    """
    for skill in skills_data or []:
        if not isinstance(skill, dict):
            continue
        try:
            hp_inc = float(skill.get("hp_increase") or 0)
        except (TypeError, ValueError):
            continue
        if hp_inc <= 0:
            continue
        name = skill.get("name") or ""
        if not name:
            continue
        yield name, hp_inc


def _raw_profession_total(skill_levels: dict[str, float], profession: dict) -> float:
    """Σ(effective_points × weight) — the un-rounded numerator behind profession level."""
    total = 0.0
    for name, weight in _iter_profession_skills(profession):
        level = skill_levels.get(name, 0.0)
        total += effective_points(name, level) * weight
    return total


def profession_level(skill_levels: dict[str, float], profession: dict) -> float:
    """Compute profession level from skill levels and profession entity.

    Formula: profession_level = Σ(effective_points(skill) × weight) / 10000
    Where effective_points = level × 20 for attribute skills, level × 1 otherwise.

    Args:
        skill_levels: {skill_name: level} from calibration data
        profession: profession entity with Skills[].{Skill.Name, Weight}
    """
    return round(_raw_profession_total(skill_levels, profession) / 10000, 2)


def all_profession_levels(
    skill_levels: dict[str, float], professions: list[dict]
) -> dict[str, float]:
    """Apply `profession_level` to every entity in `professions`.

    Used by surfaces that need every profession's level for a given skill
    snapshot (Stats tab, scan-diff "before" view, anchor-vs-current
    visibility). Returns {name: level} including 0.0 — callers filter if
    they only want non-zero levels.
    """
    result: dict[str, float] = {}
    for prof in professions:
        name = prof.get("name")
        if not name:
            continue
        result[name] = profession_level(skill_levels, prof)
    return result


def skill_rank(level: float, ranks: list[dict]) -> str:
    """Return the rank name for a skill level.

    Args:
        ranks: list of {name, skill} sorted ascending by skill threshold
    Returns: rank name (e.g., "Apprentice")
    """
    if not ranks:
        return "Unknown"
    valid_ranks = []
    for rank in ranks:
        threshold = rank.get("skill")
        name = rank.get("name")
        if threshold is None or name is None:
            continue
        try:
            threshold = float(threshold)
        except (TypeError, ValueError):
            continue
        valid_ranks.append({"name": name, "skill": threshold})

    if not valid_ranks:
        return "Unknown"

    thresholds = [rank["skill"] for rank in valid_ranks]
    i = bisect.bisect_right(thresholds, level) - 1
    i = max(0, min(i, len(valid_ranks) - 1))
    return valid_ranks[i]["name"]


def profession_skill_optimizer(
    skill_levels: dict[str, float], profession: dict
) -> dict:
    """Analyse skills for levelling a profession.

    Returns two lists:
    - skills: regular (non-attribute) skills ranked by PED cost to reach the
      next profession level via that skill alone.
    - attributes: attribute skills ranked by raw contribution factor
      (weight × 20). Attributes use a different TT curve and can't be
      targeted for levelling, so only the relative contribution ranking
      is meaningful.

    For each regular skill:
    - pedToNextLevel: PED of skill TT needed for that skill alone to push
      the profession to the next integer level.

    Args:
        skill_levels: {skill_name: level} from calibration data
        profession: profession entity with Skills[].{Skill.Name, Weight}
    """
    # Current profession level and next integer target
    current_prof = _raw_profession_total(skill_levels, profession) / 10000
    next_level = int(current_prof) + 1
    gap = next_level - current_prof  # profession levels to gain

    skills = []
    attributes = []

    for name, weight in _iter_profession_skills(profession):
        if weight <= 0:
            continue

        current_level = skill_levels.get(name, 0.0)
        is_attr = name in ATTRIBUTE_SKILLS

        if is_attr:
            # Attributes: rank by raw contribution factor only
            attributes.append(
                {
                    "name": name,
                    "weight": weight,
                    "currentLevel": current_level,
                    "contributionFactor": weight * 20,
                }
            )
        else:
            # Regular skills: compute PED to next profession level
            # Skill levels needed: gap * 10000 / weight
            levels_needed = gap * 10000 / weight
            # PED cost: TT at (current + needed) - TT at current
            target_level = current_level + levels_needed
            ped_cost = tt_value_at(target_level) - tt_value_at(current_level)

            codex_cat = get_codex_category(name)
            codex_divisor = REWARD_DIVISORS.get(codex_cat) if codex_cat else None

            skills.append(
                {
                    "name": name,
                    "weight": weight,
                    "currentLevel": current_level,
                    "levelsNeeded": round(levels_needed, 1),
                    "pedToNextLevel": round(ped_cost, 2),
                    "codexCategory": codex_cat,
                    "codexDivisor": codex_divisor,
                }
            )

    # Skills: cheapest PED cost first
    skills.sort(key=lambda x: x["pedToNextLevel"])
    # Attributes: highest contribution first
    attributes.sort(key=lambda x: x["contributionFactor"], reverse=True)

    return {
        "skills": skills,
        "attributes": attributes,
        "currentLevel": round(current_prof, 2),
        "nextLevel": next_level,
        "gap": round(gap, 4),
    }


def profession_path_optimizer(
    skill_levels: dict[str, float],
    profession: dict,
    *,
    target_level: float | None = None,
    ped_budget: float | None = None,
) -> dict:
    """Find the cheapest skill allocation to reach a target profession level, or
    the best allocation for a given PED budget.

    Uses greedy marginal-cost allocation: at each step, invest 1 skill level in
    whichever skill yields the most profession points per PED. This is optimal
    because the TT curve is convex (diminishing returns).

    Exactly one of target_level or ped_budget must be provided.
    """
    if (target_level is None) == (ped_budget is None):
        raise ValueError("Exactly one of target_level or ped_budget must be provided")

    # Current profession level
    current_prof = _raw_profession_total(skill_levels, profession) / 10000

    # Build working list of regular (non-attribute) skills
    # Skills not present in skill_levels are excluded (not yet unlocked)
    skills = []
    excluded = []
    attributes = []
    for name, weight in _iter_profession_skills(profession):
        if weight <= 0:
            continue
        if name in ATTRIBUTE_SKILLS:
            current_level = skill_levels.get(name, 0.0)
            attributes.append(
                {
                    "name": name,
                    "weight": weight,
                    "currentLevel": current_level,
                    "contributionFactor": weight * 20,
                }
            )
        elif name not in skill_levels:
            excluded.append({"name": name, "weight": weight, "reason": "not unlocked"})
        else:
            skills.append(
                {
                    "name": name,
                    "weight": weight,
                    "currentLevel": skill_levels[name],
                    "allocated": 0.0,
                    "ped": 0.0,
                }
            )
    attributes.sort(key=lambda x: x["contributionFactor"], reverse=True)
    excluded.sort(key=lambda x: x["name"])

    mode = "target" if target_level is not None else "budget"
    max_skill_level = float(max_tt_curve_level())

    if mode == "target":
        if target_level <= current_prof:
            return _path_result(
                mode,
                target_level,
                ped_budget,
                current_prof,
                current_prof,
                skills,
                attributes,
                excluded,
            )
        points_remaining = (target_level - current_prof) * 10000

        while points_remaining > 0:
            best_idx = -1
            best_ratio = float("inf")
            for i, s in enumerate(skills):
                pos = s["currentLevel"] + s["allocated"]
                if pos >= max_skill_level:
                    continue
                marginal_ped = tt_value_at(pos + 1) - tt_value_at(pos)
                ratio = marginal_ped / s["weight"]
                if ratio < best_ratio:
                    best_ratio = ratio
                    best_idx = i

            if best_idx < 0:
                break  # all skills at ceiling

            s = skills[best_idx]
            if points_remaining < s["weight"]:
                # Fractional final step
                frac_levels = points_remaining / s["weight"]
                pos = s["currentLevel"] + s["allocated"]
                frac_ped = tt_value_at(pos + frac_levels) - tt_value_at(pos)
                s["allocated"] += frac_levels
                s["ped"] += frac_ped
                points_remaining = 0
            else:
                pos = s["currentLevel"] + s["allocated"]
                step_ped = tt_value_at(pos + 1) - tt_value_at(pos)
                s["allocated"] += 1
                s["ped"] += step_ped
                points_remaining -= s["weight"]

    else:  # budget mode
        budget_remaining = ped_budget

        while budget_remaining > 1e-6:
            best_idx = -1
            best_ratio = float("inf")
            best_ped = 0.0
            for i, s in enumerate(skills):
                pos = s["currentLevel"] + s["allocated"]
                if pos >= max_skill_level:
                    continue
                marginal_ped = tt_value_at(pos + 1) - tt_value_at(pos)
                ratio = marginal_ped / s["weight"]
                if ratio < best_ratio:
                    best_ratio = ratio
                    best_idx = i
                    best_ped = marginal_ped

            if best_idx < 0:
                break  # all skills at ceiling

            s = skills[best_idx]
            if best_ped > budget_remaining:
                # Fractional final step: spend remaining budget on this skill
                pos = s["currentLevel"] + s["allocated"]
                frac_levels = levels_for_tt_value(pos, budget_remaining)
                if frac_levels <= 0:
                    break
                s["allocated"] += frac_levels
                s["ped"] += budget_remaining
                budget_remaining = 0
            else:
                s["allocated"] += 1
                s["ped"] += best_ped
                budget_remaining -= best_ped

    # Compute end profession level
    end_prof = 0.0
    for s in skills:
        end_prof += (s["currentLevel"] + s["allocated"]) * s["weight"]
    for a in attributes:
        end_prof += effective_points(a["name"], a["currentLevel"]) * a["weight"]
    end_prof /= 10000

    return _path_result(
        mode,
        target_level,
        ped_budget,
        current_prof,
        end_prof,
        skills,
        attributes,
        excluded,
    )


def _path_result(
    mode,
    target_level,
    ped_budget,
    current_prof,
    end_prof,
    skills,
    attributes,
    excluded=None,
):
    """Build the path optimizer return dict."""
    allocations = []
    for s in skills:
        codex_cat = get_codex_category(s["name"])
        codex_divisor = REWARD_DIVISORS.get(codex_cat) if codex_cat else None
        allocations.append(
            {
                "name": s["name"],
                "weight": s["weight"],
                "currentLevel": s["currentLevel"],
                "levelsToGain": round(s["allocated"], 2),
                "pedCost": round(s["ped"], 2),
                "newLevel": round(s["currentLevel"] + s["allocated"], 2),
                "codexCategory": codex_cat,
                "codexDivisor": codex_divisor,
            }
        )

    # Sort: allocated skills first (by pedCost desc), then unallocated alphabetical
    allocated = [a for a in allocations if a["levelsToGain"] > 0]
    unallocated = [a for a in allocations if a["levelsToGain"] == 0]
    allocated.sort(key=lambda x: x["pedCost"], reverse=True)
    unallocated.sort(key=lambda x: x["name"])

    total_ped = round(sum(s["ped"] for s in skills), 2)

    return {
        "mode": mode,
        "inputTargetLevel": target_level,
        "inputPedBudget": ped_budget,
        "currentLevel": round(current_prof, 2),
        "endLevel": round(end_prof, 2),
        "professionLevelsGained": round(end_prof - current_prof, 2),
        "totalPed": total_ped,
        "allocations": allocated + unallocated,
        "attributes": attributes,
        "excluded": excluded or [],
    }


def calculate_hp(skill_levels: dict[str, float], skills_data: list[dict]) -> float:
    """Compute total HP from skill levels and skill metadata.

    Formula: 80 + Σ(effective_points(skill) / hp_increase)
    Only skills with hp_increase > 0 contribute.
    """
    BASE_HP = 80.0
    hp = BASE_HP
    for name, hp_inc in _iter_hp_skills(skills_data):
        level = skill_levels.get(name, 0.0)
        if level > 0:
            hp += effective_points(name, level) / hp_inc
    return hp


def hp_skill_optimizer(skill_levels: dict[str, float], skills_data: list[dict]) -> dict:
    """Rank skills by cost-efficiency for gaining HP.

    Returns two lists:
    - skills: regular (non-attribute) skills ranked by PED cost per +1 HP.
    - attributes: attribute skills ranked by HP contribution factor.

    For each regular skill with hp_increase > 0:
    - levelsPerHp: skill levels needed to gain 1 HP (= hp_increase)
    - pedPerHp: PED cost to gain those levels at the player's current level
    - hpPerPed: inverse — HP gained per 1 PED of skill TT

    For each attribute with hp_increase > 0:
    - levelsPerHp: raw levels needed (= hp_increase / 20, due to ×20 multiplier)
    - hpContribution: current HP contributed by this attribute
    """
    current_hp = calculate_hp(skill_levels, skills_data)

    skills = []
    attributes = []

    for name, hp_inc in _iter_hp_skills(skills_data):
        current_level = skill_levels.get(name, 0.0)
        is_attr = name in ATTRIBUTE_SKILLS

        if is_attr:
            # Attributes: ×20 multiplier means levelsPerHp = hp_increase / 20
            levels_per_hp = hp_inc / 20
            hp_contributed = (
                (effective_points(name, current_level) / hp_inc)
                if current_level > 0
                else 0.0
            )
            attributes.append(
                {
                    "name": name,
                    "hpIncrease": hp_inc,
                    "currentLevel": current_level,
                    "levelsPerHp": round(levels_per_hp, 2),
                    "hpContribution": round(hp_contributed, 2),
                }
            )
        else:
            # Regular skills: levels for 1 HP = hp_increase (no multiplier)
            levels_per_hp = hp_inc
            target_level = current_level + levels_per_hp
            ped_per_hp = tt_value_at(target_level) - tt_value_at(current_level)
            hp_per_ped = (1.0 / ped_per_hp) if ped_per_hp > 0 else 0.0

            codex_cat = get_codex_category(name)
            codex_divisor = REWARD_DIVISORS.get(codex_cat) if codex_cat else None

            skills.append(
                {
                    "name": name,
                    "hpIncrease": hp_inc,
                    "currentLevel": current_level,
                    "levelsPerHp": round(levels_per_hp, 1),
                    "pedPerHp": round(ped_per_hp, 2),
                    "hpPerPed": round(hp_per_ped, 4),
                    "codexCategory": codex_cat,
                    "codexDivisor": codex_divisor,
                }
            )

    # Sort skills by PED per HP ascending (cheapest first)
    skills.sort(key=lambda x: x["pedPerHp"])
    # Sort attributes by levelsPerHp ascending (most HP per level first)
    attributes.sort(key=lambda x: x["levelsPerHp"])

    return {
        "currentHp": round(current_hp, 2),
        "skills": skills,
        "attributes": attributes,
    }


def codex_next_reward(skill_name: str, current_level: float) -> float | None:
    """Predicted TT value of the next codex reward for a skill.

    Formula: level / divisor (PED).
    Returns None if the skill has no codex category.
    """
    cat = get_codex_category(skill_name)
    if cat is None:
        return None
    divisor = REWARD_DIVISORS[cat]
    return round(current_level / divisor, 4)


def codex_tier_progress(skill_name: str, current_level: float) -> float | None:
    """Estimated progress through the current codex tier (0–1).

    Uses level modulo divisor as a rough proxy for tier progress.
    Returns None if the skill has no codex category.
    """
    cat = get_codex_category(skill_name)
    if cat is None:
        return None
    divisor = REWARD_DIVISORS[cat]
    if divisor == 0:
        return 0.0
    return round((current_level % divisor) / divisor, 4)
