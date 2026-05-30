"""Property-based tests for character calculations.

Covers ``backend.services.character_calc``: effective points, profession level,
skill rank lookup, HP, the codex predictions, and the path optimiser.
"""

import pytest
from hypothesis import assume, given, settings
from hypothesis import strategies as st

from backend.data.codex_categories import get_codex_category
from backend.services.character_calc import (
    ATTRIBUTE_SKILLS,
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

_SKILL_NAMES = st.sampled_from(["Handgun", "Rifle", "Aim", "Health", "Strength"])
_WEIGHT = st.floats(
    min_value=0.0, max_value=10.0, allow_nan=False, allow_infinity=False
)
_LEVEL = st.floats(
    min_value=0.0, max_value=1000.0, allow_nan=False, allow_infinity=False
)


def _profession(weights):
    return {
        "name": "P",
        "skills": [{"skill": {"name": n}, "weight": w} for n, w in weights.items()],
    }


# --- effective_points ---


@given(st.sampled_from(sorted(ATTRIBUTE_SKILLS)), _LEVEL)
def test_attribute_points_are_times_twenty(skill, level):
    assert effective_points(skill, level) == pytest.approx(level * 20)


@given(st.text().filter(lambda s: s not in ATTRIBUTE_SKILLS), _LEVEL)
def test_regular_points_are_the_level(skill, level):
    assert effective_points(skill, level) == pytest.approx(level)


def test_zero_level_gives_zero_points():
    assert effective_points("Handgun", 0.0) == 0.0
    assert effective_points("Health", 0.0) == 0.0


# --- profession_level ---


@given(
    st.dictionaries(_SKILL_NAMES, _WEIGHT, max_size=5),
    st.dictionaries(_SKILL_NAMES, _LEVEL, max_size=5),
)
def test_profession_level_is_non_negative_and_matches_the_formula(weights, levels):
    profession = _profession(weights)
    result = profession_level(levels, profession)
    assert result >= 0.0
    raw = (
        sum(effective_points(n, levels.get(n, 0.0)) * w for n, w in weights.items())
        / 10000
    )
    assert result == pytest.approx(raw, abs=0.005)  # rounded to two decimals


@given(
    _WEIGHT,
    _LEVEL,
    st.floats(min_value=0.1, max_value=100.0, allow_nan=False, allow_infinity=False),
)
def test_profession_level_is_monotone_in_skill_level(weight, base_level, delta):
    profession = _profession({"Handgun": weight})
    lo = profession_level({"Handgun": base_level}, profession)
    hi = profession_level({"Handgun": base_level + delta}, profession)
    assert hi + 1e-9 >= lo


@given(
    st.dictionaries(_SKILL_NAMES, _WEIGHT, min_size=1, max_size=5),
    st.dictionaries(_SKILL_NAMES, _LEVEL, max_size=5),
    _SKILL_NAMES,
    st.floats(min_value=0.1, max_value=100.0, allow_nan=False, allow_infinity=False),
)
def test_profession_level_is_monotone_raising_any_one_skill(
    weights, levels, raised, delta
):
    # Raising a single skill (regular or attribute) inside a multi-skill
    # profession never lowers the rounded profession level, given the
    # weight >= 0 precondition the _WEIGHT strategy enforces.
    profession = _profession(weights)
    lo = profession_level(levels, profession)
    bumped = dict(levels)
    bumped[raised] = bumped.get(raised, 0.0) + delta
    hi = profession_level(bumped, profession)
    assert hi + 1e-9 >= lo


def test_missing_skill_contributes_zero():
    assert profession_level({}, _profession({"Handgun": 5.0})) == 0.0


# --- skill_rank ---


def test_skill_rank_is_unknown_without_valid_ranks():
    assert skill_rank(50.0, []) == "Unknown"
    assert skill_rank(50.0, [{"skill": None, "name": None}]) == "Unknown"


@given(
    st.floats(min_value=-100.0, max_value=200.0, allow_nan=False, allow_infinity=False)
)
def test_skill_rank_returns_a_string_and_clamps(level):
    ranks = [
        {"skill": 0, "name": "A"},
        {"skill": 10, "name": "B"},
        {"skill": 20, "name": "C"},
    ]
    result = skill_rank(level, ranks)
    assert isinstance(result, str)
    if level < 0:
        assert result == "A"
    if level >= 20:
        assert result == "C"


@given(
    st.floats(min_value=0.0, max_value=100.0, allow_nan=False, allow_infinity=False),
    st.floats(min_value=0.1, max_value=50.0, allow_nan=False, allow_infinity=False),
)
def test_skill_rank_is_non_decreasing_in_level(level, delta):
    ranks = [
        {"skill": 0, "name": "A"},
        {"skill": 10, "name": "B"},
        {"skill": 20, "name": "C"},
    ]
    order = {"A": 0, "B": 1, "C": 2}
    assert order[skill_rank(level + delta, ranks)] >= order[skill_rank(level, ranks)]


# --- profession_path_optimizer ---


def test_path_optimizer_requires_exactly_one_objective():
    profession = _profession({"Handgun": 5.0})
    with pytest.raises(ValueError):
        profession_path_optimizer({"Handgun": 10.0}, profession)
    with pytest.raises(ValueError):
        profession_path_optimizer(
            {"Handgun": 10.0}, profession, target_level=5.0, ped_budget=10.0
        )


@given(
    st.floats(min_value=0.0, max_value=10.0, allow_nan=False, allow_infinity=False),
    st.floats(min_value=0.1, max_value=20.0, allow_nan=False, allow_infinity=False),
)
def test_path_optimizer_raises_unless_exactly_one_objective_given(target, budget):
    # XOR guard: the call must succeed iff exactly one of target_level /
    # ped_budget is supplied, and raise ValueError when neither or both are.
    profession = _profession({"Handgun": 6.0})
    levels = {"Handgun": 50.0}

    with pytest.raises(ValueError):
        profession_path_optimizer(levels, profession)
    with pytest.raises(ValueError):
        profession_path_optimizer(
            levels, profession, target_level=target, ped_budget=budget
        )
    # Exactly one provided: both succeed without raising.
    profession_path_optimizer(levels, profession, target_level=target)
    profession_path_optimizer(levels, profession, ped_budget=budget)


@settings(max_examples=20)
@given(st.floats(min_value=0.5, max_value=15.0, allow_nan=False, allow_infinity=False))
def test_budget_mode_never_exceeds_the_budget(budget):
    profession = _profession({"Handgun": 6.0, "Rifle": 4.0})
    result = profession_path_optimizer(
        {"Handgun": 50.0, "Rifle": 50.0}, profession, ped_budget=budget
    )
    assert result["totalPed"] <= budget + 0.05
    assert result["endLevel"] + 1e-9 >= result["currentLevel"]


@settings(max_examples=20)
@given(st.floats(min_value=0.01, max_value=0.3, allow_nan=False, allow_infinity=False))
def test_target_mode_reaches_the_target(delta):
    profession = _profession({"Handgun": 6.0, "Rifle": 4.0})
    levels = {"Handgun": 50.0, "Rifle": 50.0}
    current = profession_level(levels, profession)
    result = profession_path_optimizer(levels, profession, target_level=current + delta)
    assert result["endLevel"] + 0.01 >= current + delta
    assert result["endLevel"] + 1e-9 >= result["currentLevel"]


def test_target_below_current_is_a_no_op():
    profession = _profession({"Handgun": 6.0})
    levels = {"Handgun": 80.0}
    current = profession_level(levels, profession)
    result = profession_path_optimizer(levels, profession, target_level=current - 1.0)
    assert result["endLevel"] == pytest.approx(result["currentLevel"])
    assert result["totalPed"] == 0.0


# --- HP ---


@given(st.dictionaries(_SKILL_NAMES, _LEVEL, max_size=5))
def test_hp_is_at_least_base(levels):
    skills_data = [
        {"name": "Anatomy", "hp_increase": 140.0},
        {"name": "Health", "hp_increase": 35.0},
    ]
    assert calculate_hp(levels, skills_data) >= 80.0


@given(
    _LEVEL,
    st.floats(min_value=0.1, max_value=100.0, allow_nan=False, allow_infinity=False),
)
def test_hp_is_monotone_in_level(base_level, delta):
    skills_data = [{"name": "Anatomy", "hp_increase": 140.0}]
    lo = calculate_hp({"Anatomy": base_level}, skills_data)
    hi = calculate_hp({"Anatomy": base_level + delta}, skills_data)
    assert hi + 1e-9 >= lo


def test_hp_skips_non_positive_increase():
    skills_data = [{"name": "X", "hp_increase": 0}, {"name": "Y", "hp_increase": -5}]
    assert calculate_hp({"X": 100.0, "Y": 100.0}, skills_data) == 80.0


# --- codex predictions ---


@given(st.text(), _LEVEL)
def test_codex_predictions_are_none_for_non_codex_skills(name, level):
    assume(get_codex_category(name) is None)
    assert codex_next_reward(name, level) is None
    assert codex_tier_progress(name, level) is None


@given(
    st.sampled_from(["Handgun", "Rifle", "Aim"]),
    st.floats(min_value=0.0, max_value=1e5, allow_nan=False, allow_infinity=False),
)
def test_codex_tier_progress_stays_in_the_unit_interval(skill, level):
    progress = codex_tier_progress(skill, level)
    assert progress is not None
    # Rounding to four decimals can touch 1.0 at the very top of a tier.
    assert 0.0 <= progress <= 1.0


# --- optimiser sort order ---

# Mix of regular and attribute skills so both result lists are populated.
_OPTIMISER_SKILLS = ["Handgun", "Rifle", "Aim", "Health", "Strength", "Agility"]


def _is_non_decreasing(values):
    return all(a <= b for a, b in zip(values, values[1:], strict=False))


def _is_non_increasing(values):
    return all(a >= b for a, b in zip(values, values[1:], strict=False))


@given(
    st.dictionaries(
        st.sampled_from(_OPTIMISER_SKILLS),
        st.floats(min_value=0.1, max_value=10.0, allow_nan=False, allow_infinity=False),
        min_size=1,
        max_size=6,
    ),
    st.dictionaries(
        st.sampled_from(_OPTIMISER_SKILLS),
        st.floats(
            min_value=0.0, max_value=500.0, allow_nan=False, allow_infinity=False
        ),
        max_size=6,
    ),
)
def test_profession_optimiser_returns_sorted_lists(weights, levels):
    profession = _profession(weights)
    result = profession_skill_optimizer(levels, profession)
    # Regular skills ascending by PED to the next profession level.
    assert _is_non_decreasing([s["pedToNextLevel"] for s in result["skills"]])
    # Attributes descending by contribution factor.
    assert _is_non_increasing([a["contributionFactor"] for a in result["attributes"]])


@given(
    st.lists(
        st.fixed_dictionaries(
            {
                "name": st.sampled_from(_OPTIMISER_SKILLS),
                "hp_increase": st.floats(
                    min_value=1.0,
                    max_value=300.0,
                    allow_nan=False,
                    allow_infinity=False,
                ),
            }
        ),
        min_size=1,
        max_size=6,
    ),
    st.dictionaries(
        st.sampled_from(_OPTIMISER_SKILLS),
        st.floats(
            min_value=0.0, max_value=500.0, allow_nan=False, allow_infinity=False
        ),
        max_size=6,
    ),
)
def test_hp_optimiser_returns_sorted_lists(skills_data, levels):
    result = hp_skill_optimizer(levels, skills_data)
    # Regular skills ascending by PED per HP.
    assert _is_non_decreasing([s["pedPerHp"] for s in result["skills"]])
    # Attributes ascending by levels per HP.
    assert _is_non_decreasing([a["levelsPerHp"] for a in result["attributes"]])
