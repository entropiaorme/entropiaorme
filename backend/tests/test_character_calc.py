"""Tests for character calculation functions."""

import pytest

from backend.data.codex_categories import get_codex_category
from backend.data.tt_value_curve import (
    max_tt_curve_level,
    tt_value_at,
    tt_value_of_gain,
)
from backend.services.character_calc import (
    calculate_hp,
    codex_next_reward,
    codex_tier_progress,
    effective_points,
    hp_skill_optimizer,
    profession_level,
    profession_path_optimizer,
    profession_skill_optimizer,
    skill_rank,
)

# ── TT Value Curve ─────────────────────────────────────────────────────────────


def test_tt_value_at_zero():
    assert tt_value_at(0) == 0.0


def test_tt_value_at_anchor_100():
    assert tt_value_at(100) == pytest.approx(0.12, abs=1e-4)


def test_tt_value_at_anchor_5000():
    assert tt_value_at(5000) == pytest.approx(149.86, abs=1e-2)


def test_tt_value_at_interpolation():
    # Midpoint between 0 (0.0) and 100 (0.12) should be ~0.06
    v = tt_value_at(50)
    assert 0.05 < v < 0.07


def test_tt_value_at_max():
    # Beyond the last anchor returns the last value
    assert tt_value_at(20000) == pytest.approx(13381.54, abs=1e-2)


def test_max_tt_curve_level():
    assert max_tt_curve_level() == 20000


def test_tt_value_of_gain():
    gain = tt_value_of_gain(0, 100)
    assert gain == pytest.approx(0.12, abs=1e-4)


def test_tt_value_of_gain_zero():
    assert tt_value_of_gain(500, 500) == pytest.approx(0.0, abs=1e-6)


# ── Effective Points ───────────────────────────────────────────────────────────


def test_effective_points_attribute_skill():
    assert effective_points("Agility", 100) == 2000.0


def test_effective_points_normal_skill():
    assert effective_points("Laser Weaponry Technology", 500) == 500.0


# ── Profession Level ───────────────────────────────────────────────────────────


def test_profession_level_basic():
    # Profession with two skills, each weight 50
    # Catalogue payload nests skill name at Skills[].Skill.Name
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 50},
            {"skill": {"name": "Aim"}, "weight": 50},
        ]
    }
    skill_levels = {"Rifle": 1000.0, "Aim": 1000.0}
    level = profession_level(skill_levels, profession)
    # (1000 * 50 + 1000 * 50) / 10000 = 10.0
    assert level == pytest.approx(10.0, abs=1e-2)


def test_profession_level_attribute_multiplier():
    # Agility (attribute) gets ×20 multiplier
    profession = {"skills": [{"skill": {"name": "Agility"}, "weight": 100}]}
    skill_levels = {"Agility": 100.0}
    level = profession_level(skill_levels, profession)
    # (100 * 20 * 100) / 10000 = 20.0
    assert level == pytest.approx(20.0, abs=1e-2)


def test_profession_level_missing_skills():
    # Skills not in calibration default to 0
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 50}]}
    level = profession_level({}, profession)
    assert level == pytest.approx(0.0, abs=1e-6)


def test_profession_level_null_weight():
    # Null weight treated as 0; no contribution
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": None}]}
    level = profession_level({"Rifle": 1000.0}, profession)
    assert level == pytest.approx(0.0, abs=1e-6)


# ── Profession Skill Optimizer ─────────────────────────────────────────────────


def test_optimizer_skills_sorted_by_ped_cost():
    """Lower-level skills with high weight should cost less PED to next prof level."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 60},
            {"skill": {"name": "Aim"}, "weight": 40},
        ]
    }
    # Rifle at high level (expensive), Aim at low level (cheap)
    skill_levels = {"Rifle": 5000.0, "Aim": 100.0}
    result = profession_skill_optimizer(skill_levels, profession)
    skills = result["skills"]
    assert len(skills) == 2
    # Aim requires more levels (lower weight) but from a much cheaper position
    # Both should have pedToNextLevel > 0
    assert all(s["pedToNextLevel"] > 0 for s in skills)
    # Sorted by PED cost ascending
    assert skills[0]["pedToNextLevel"] <= skills[1]["pedToNextLevel"]


def test_optimizer_attributes_separate():
    """Attributes should be in the attributes list, not skills."""
    profession = {
        "skills": [
            {"skill": {"name": "Agility"}, "weight": 10},
            {"skill": {"name": "Rifle"}, "weight": 60},
        ]
    }
    skill_levels = {"Agility": 50.0, "Rifle": 1000.0}
    result = profession_skill_optimizer(skill_levels, profession)
    assert len(result["skills"]) == 1
    assert result["skills"][0]["name"] == "Rifle"
    assert len(result["attributes"]) == 1
    assert result["attributes"][0]["name"] == "Agility"
    assert result["attributes"][0]["contributionFactor"] == 10 * 20  # weight × 20


def test_optimizer_gap_and_next_level():
    """Should report correct gap to next integer profession level."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 100},
        ]
    }
    # Rifle at level 500 → prof level = 500 * 100 / 10000 = 5.0
    skill_levels = {"Rifle": 500.0}
    result = profession_skill_optimizer(skill_levels, profession)
    assert result["currentLevel"] == 5.0
    assert result["nextLevel"] == 6
    assert result["gap"] == pytest.approx(1.0, abs=1e-4)


# ── Skill Rank ─────────────────────────────────────────────────────────────────


def test_skill_rank_below_first_threshold():
    ranks = [{"name": "Newbie", "skill": 1}, {"name": "Inept", "skill": 10}]
    # Level 0 falls before first rank: should return lowest rank
    assert skill_rank(0, ranks) == "Newbie"


def test_skill_rank_exact_threshold():
    ranks = [
        {"name": "Newbie", "skill": 1},
        {"name": "Inept", "skill": 10},
        {"name": "Competent", "skill": 100},
    ]
    assert skill_rank(10, ranks) == "Inept"


def test_skill_rank_between_thresholds():
    ranks = [
        {"name": "Newbie", "skill": 1},
        {"name": "Inept", "skill": 10},
        {"name": "Competent", "skill": 100},
    ]
    assert skill_rank(50, ranks) == "Inept"


def test_skill_rank_above_all():
    ranks = [
        {"name": "Newbie", "skill": 1},
        {"name": "Entropia Master", "skill": 20000},
    ]
    assert skill_rank(99999, ranks) == "Entropia Master"


def test_skill_rank_empty():
    assert skill_rank(100, []) == "Unknown"


def test_skill_rank_ignores_invalid_threshold_rows():
    ranks: list[dict] = [
        {"name": "Broken", "skill": None},
        {"name": "Newbie", "skill": 1},
        {"name": "Inept", "skill": 10},
    ]
    assert skill_rank(5, ranks) == "Newbie"


# ── Codex ─────────────────────────────────────────────────────────────────────


def test_get_codex_category_known():
    assert get_codex_category("Laser Weaponry Technology") == "cat1"
    assert get_codex_category("Dodge") == "cat3"
    assert get_codex_category("Zoology") == "cat4"


def test_get_codex_category_unknown():
    assert get_codex_category("Mining") is None
    assert get_codex_category("Serendipity") is None


def test_codex_next_reward_cat1():
    # Laser Weaponry Technology (cat1, divisor 200) at level 5000 → 5000/200 = 25.0
    reward = codex_next_reward("Laser Weaponry Technology", 5000)
    assert reward == pytest.approx(25.0, abs=1e-4)


def test_codex_next_reward_cat3():
    # Dodge (cat3, divisor 640) at level 3200 → 3200/640 = 5.0
    reward = codex_next_reward("Dodge", 3200)
    assert reward == pytest.approx(5.0, abs=1e-4)


def test_codex_next_reward_no_category():
    assert codex_next_reward("Mining", 1000) is None


def test_codex_tier_progress():
    # Level 100, divisor 200 → 100/200 = 0.5
    p = codex_tier_progress("Laser Weaponry Technology", 100)
    assert p == pytest.approx(0.5, abs=1e-4)


def test_codex_tier_progress_at_boundary():
    # At exact multiple of divisor → progress = 0.0
    p = codex_tier_progress("Laser Weaponry Technology", 200)
    assert p == pytest.approx(0.0, abs=1e-6)


def test_codex_tier_progress_no_category():
    assert codex_tier_progress("Mining", 1000) is None


# ── HP Calculation ────────────────────────────────────────────────────────────


def _make_skill(name, hp_increase):
    """Helper to build a minimal skill entity in the catalogue payload shape."""
    return {"name": name, "hp_increase": hp_increase}


def test_calculate_hp_base_only():
    """No skills = base HP of 80."""
    assert calculate_hp({}, []) == 80.0


def test_calculate_hp_single_regular_skill():
    """Regular skill: effective_points = level, HP += level / HpIncrease."""
    skills_data = [_make_skill("Rifle", 1600)]
    skill_levels = {"Rifle": 1600.0}
    # HP = 80 + 1600/1600 = 81.0
    assert calculate_hp(skill_levels, skills_data) == pytest.approx(81.0, abs=1e-2)


def test_calculate_hp_attribute_multiplier():
    """Attribute skill gets ×20: effective_points = level × 20."""
    skills_data = [_make_skill("Stamina", 9.25)]
    skill_levels = {"Stamina": 50.0}
    # HP = 80 + (50 × 20) / 9.25 = 80 + 108.108... = 188.108
    assert calculate_hp(skill_levels, skills_data) == pytest.approx(188.108, abs=0.1)


def test_calculate_hp_zero_hp_increase_ignored():
    """Skills with HpIncrease=0 don't contribute."""
    skills_data = [_make_skill("Chemistry", 0)]
    assert calculate_hp({"Chemistry": 5000.0}, skills_data) == 80.0


def test_calculate_hp_multiple_skills():
    """Multiple skills sum their contributions."""
    skills_data = [_make_skill("Rifle", 1600), _make_skill("Commando", 200)]
    skill_levels = {"Rifle": 3200.0, "Commando": 400.0}
    # HP = 80 + 3200/1600 + 400/200 = 80 + 2 + 2 = 84
    assert calculate_hp(skill_levels, skills_data) == pytest.approx(84.0, abs=1e-2)


# ── HP Optimizer ──────────────────────────────────────────────────────────────


def test_hp_optimizer_skills_sorted_by_ped_per_hp():
    """Skills should be sorted by PED/HP ascending (cheapest first)."""
    skills_data = [
        _make_skill("Rifle", 1600),  # 1600 levels per HP
        _make_skill("Commando", 200),  # 200 levels per HP
    ]
    # Commando at low level = cheap PED per HP; Rifle at high level = expensive
    skill_levels = {"Rifle": 5000.0, "Commando": 100.0}
    result = hp_skill_optimizer(skill_levels, skills_data)
    skills = result["skills"]
    assert len(skills) == 2
    assert all(s["pedPerHp"] > 0 for s in skills)
    # Should be sorted ascending by pedPerHp
    assert skills[0]["pedPerHp"] <= skills[1]["pedPerHp"]


def test_hp_optimizer_attributes_separate():
    """Attributes should be in the attributes list, not skills."""
    skills_data = [
        _make_skill("Stamina", 9.25),
        _make_skill("Rifle", 1600),
    ]
    skill_levels = {"Stamina": 50.0, "Rifle": 1000.0}
    result = hp_skill_optimizer(skill_levels, skills_data)
    assert len(result["skills"]) == 1
    assert result["skills"][0]["name"] == "Rifle"
    assert len(result["attributes"]) == 1
    assert result["attributes"][0]["name"] == "Stamina"
    # Stamina: levelsPerHp = 9.25 / 20 = 0.4625
    assert result["attributes"][0]["levelsPerHp"] == pytest.approx(0.46, abs=0.01)


def test_hp_optimizer_levels_per_hp_regular():
    """Regular skill levelsPerHp should equal HpIncrease."""
    skills_data = [_make_skill("Rifle", 1600)]
    skill_levels = {"Rifle": 1000.0}
    result = hp_skill_optimizer(skill_levels, skills_data)
    assert result["skills"][0]["levelsPerHp"] == 1600.0


def test_hp_optimizer_current_hp():
    """Result should include correctly computed current HP."""
    skills_data = [_make_skill("Rifle", 1600)]
    skill_levels = {"Rifle": 1600.0}
    result = hp_skill_optimizer(skill_levels, skills_data)
    assert result["currentHp"] == pytest.approx(81.0, abs=1e-2)


def test_hp_optimizer_no_hp_skills():
    """Skills with HpIncrease=0 should be excluded entirely."""
    skills_data = [_make_skill("Chemistry", 0)]
    result = hp_skill_optimizer({"Chemistry": 5000.0}, skills_data)
    assert len(result["skills"]) == 0
    assert len(result["attributes"]) == 0
    assert result["currentHp"] == 80.0


# ── Profession Path Optimizer ────────────────────────────────────────────────


def test_path_optimizer_target_basic():
    """Two equal-weight skills at 0, target +1 level: both should get allocation."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 50},
            {"skill": {"name": "Aim"}, "weight": 50},
        ]
    }
    skill_levels = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(skill_levels, profession, target_level=1.0)
    assert result["mode"] == "target"
    assert result["endLevel"] >= 1.0
    assert result["totalPed"] > 0
    allocated = [a for a in result["allocations"] if a["levelsToGain"] > 0]
    assert len(allocated) == 2
    # Total profession points gained should cover the gap
    total_points = sum(a["levelsToGain"] * a["weight"] for a in result["allocations"])
    assert total_points >= 10000 - 1  # allow small rounding


def test_path_optimizer_target_prefers_cheaper_skill():
    """Low-level skill should receive more allocation than high-level skill."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 50},
            {"skill": {"name": "Aim"}, "weight": 50},
        ]
    }
    skill_levels = {"Rifle": 0.0, "Aim": 5000.0}
    # Target high enough that both skills must contribute
    result = profession_path_optimizer(skill_levels, profession, target_level=60.0)
    allocs = {a["name"]: a for a in result["allocations"]}
    # Rifle (starting at 0) should get more levels than Aim (starting at 5000)
    assert allocs["Rifle"]["levelsToGain"] > allocs["Aim"]["levelsToGain"]


def test_path_optimizer_target_respects_weight():
    """High-weight skill yields more prof points per level, so it dominates."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 100},
            {"skill": {"name": "Aim"}, "weight": 10},
        ]
    }
    skill_levels = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(skill_levels, profession, target_level=1.0)
    allocs = {a["name"]: a for a in result["allocations"]}
    # Rifle (weight 100) is 10x more efficient per skill level
    assert allocs["Rifle"]["pedCost"] > allocs["Aim"]["pedCost"]
    assert allocs["Rifle"]["levelsToGain"] > allocs["Aim"]["levelsToGain"]


def test_path_optimizer_budget_basic():
    """Budget mode: spend 1 PED across two skills."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 50},
            {"skill": {"name": "Aim"}, "weight": 50},
        ]
    }
    skill_levels = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(skill_levels, profession, ped_budget=1.0)
    assert result["mode"] == "budget"
    assert result["totalPed"] <= 1.01  # within rounding
    assert result["endLevel"] > result["currentLevel"]


def test_path_optimizer_budget_exhausts():
    """Total PED allocated should be close to the budget."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 50},
        ]
    }
    skill_levels = {"Rifle": 0.0}
    result = profession_path_optimizer(skill_levels, profession, ped_budget=5.0)
    assert result["totalPed"] == pytest.approx(5.0, abs=0.05)


def test_path_optimizer_excludes_attributes():
    """Attribute skills should appear in attributes, not allocations."""
    profession = {
        "skills": [
            {"skill": {"name": "Agility"}, "weight": 10},
            {"skill": {"name": "Rifle"}, "weight": 60},
        ]
    }
    skill_levels = {"Rifle": 0.0, "Agility": 0.0}
    result = profession_path_optimizer(skill_levels, profession, target_level=1.0)
    alloc_names = {a["name"] for a in result["allocations"]}
    attr_names = {a["name"] for a in result["attributes"]}
    assert "Agility" not in alloc_names
    assert "Agility" in attr_names
    assert "Rifle" in alloc_names


def test_path_optimizer_excludes_unlocked_skills():
    """Skills not in skill_levels should be excluded, not optimised."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 50},
            {"skill": {"name": "Aim"}, "weight": 50},
        ]
    }
    skill_levels = {"Rifle": 100.0}  # Aim not unlocked
    result = profession_path_optimizer(skill_levels, profession, target_level=2.0)
    alloc_names = {a["name"] for a in result["allocations"]}
    excluded_names = {e["name"] for e in result["excluded"]}
    assert "Aim" not in alloc_names
    assert "Aim" in excluded_names
    assert result["excluded"][0]["reason"] == "not unlocked"
    assert "Rifle" in alloc_names


def test_path_optimizer_target_already_reached():
    """Target at or below current level: zero allocation."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 100},
        ]
    }
    skill_levels = {"Rifle": 500.0}  # prof level = 5.0
    result = profession_path_optimizer(skill_levels, profession, target_level=4.0)
    assert result["totalPed"] == 0
    assert result["professionLevelsGained"] == 0


def test_path_optimizer_can_allocate_past_15000_skill_level():
    """Path optimizer should follow the TT curve ceiling."""
    profession = {
        "skills": [
            {"skill": {"name": "Rifle"}, "weight": 100},
        ]
    }
    skill_levels = {"Rifle": 14990.0}
    result = profession_path_optimizer(skill_levels, profession, target_level=151.5)
    alloc = result["allocations"][0]
    assert result["endLevel"] == pytest.approx(151.5, abs=1e-2)
    assert alloc["levelsToGain"] == pytest.approx(160.0, abs=1e-2)
    assert alloc["newLevel"] == pytest.approx(15150.0, abs=1e-2)


def test_path_optimizer_mutual_exclusion():
    """Both or neither params should raise ValueError."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 50}]}
    with pytest.raises(ValueError):
        profession_path_optimizer(
            {"Rifle": 0.0}, profession, target_level=5.0, ped_budget=10.0
        )
    with pytest.raises(ValueError):
        profession_path_optimizer({"Rifle": 0.0}, profession)
