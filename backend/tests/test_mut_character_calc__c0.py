"""Mutation-hardening tests for backend.services.character_calc (cluster c0).

Targets the surviving/no-test mutants in:
  _iter_profession_skills, _iter_hp_skills, profession_level,
  all_profession_levels, skill_rank, profession_skill_optimizer.

Each test drives the public API through the mutated line and asserts the exact
behaviour the mutation breaks.  Scenarios are arithmetic-pinned so precision and
divisor mutants cannot survive on rounding coincidence.
"""

from typing import cast

import pytest

from backend.services.character_calc import (
    all_profession_levels,
    calculate_hp,
    profession_level,
    profession_skill_optimizer,
    skill_rank,
)

# ── _iter_profession_skills (exercised via profession_level / optimizer) ────────


def test_iter_prof_non_dict_entry_is_skipped_not_break():
    """A non-dict entry must be skipped (continue), and a later valid skill
    must still be summed. Kills the `continue -> break` mutant on the
    isinstance guard (mutmut_6)."""
    profession = {
        "skills": [
            "junk-not-a-dict",
            {"skill": {"name": "Rifle"}, "weight": 50},
        ]
    }
    # Only Rifle contributes: 1000 * 50 / 10000 = 5.0
    assert profession_level({"Rifle": 1000.0}, profession) == pytest.approx(5.0)


def test_iter_prof_missing_name_entry_is_skipped_not_break():
    """An entry with no skill name must be skipped (continue), and a later
    valid skill must still be summed. Kills `continue -> break` on the
    name guard (mutmut_19)."""
    profession = {
        "skills": [
            {"skill": {}, "weight": 50},  # no name -> skipped
            {"skill": {"name": "Rifle"}, "weight": 50},
        ]
    }
    assert profession_level({"Rifle": 1000.0}, profession) == pytest.approx(5.0)


def test_iter_prof_missing_name_not_substituted_with_placeholder():
    """A nameless entry must NOT be turned into a synthetic skill. Kills the
    `name or "" -> name or "XXXX"` mutant (mutmut_17): the placeholder would
    surface as an extra optimizer skill row."""
    profession = {
        "skills": [
            {"skill": {}, "weight": 50},  # no name
            {"skill": {"name": "Rifle"}, "weight": 50},
        ]
    }
    result = profession_skill_optimizer({"Rifle": 500.0}, profession)
    names = [s["name"] for s in result["skills"]]
    assert names == ["Rifle"]
    assert "XXXX" not in names


def test_iter_prof_unparseable_weight_defaults_to_zero_not_one():
    """A non-numeric weight must fall back to 0.0 (no contribution), not 1.0.
    Kills the except-branch default mutant (mutmut_28)."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": "abc"}]}
    # weight 0 => no contribution => level 0.0 (mutant 1.0 => 1000/10000 = 0.1)
    assert profession_level({"Rifle": 1000.0}, profession) == pytest.approx(0.0)


# ── _iter_hp_skills (exercised via calculate_hp) ────────────────────────────────


def test_iter_hp_non_dict_entry_is_skipped_not_break():
    """A non-dict skill entry must be skipped, later valid skill still counts.
    Kills `continue -> break` on the isinstance guard (mutmut_3)."""
    skills_data = ["junk", {"name": "Rifle", "hp_increase": 10}]
    # 80 + 100/10 = 90
    assert calculate_hp(
        {"Rifle": 100.0}, cast(list[dict], skills_data)
    ) == pytest.approx(90.0)


def test_iter_hp_unparseable_hp_increase_is_skipped_not_break():
    """A non-numeric hp_increase must be skipped (continue), later valid skill
    still counts. Kills `continue -> break` in the except branch (mutmut_11)."""
    skills_data = [
        {"name": "Bad", "hp_increase": "abc"},
        {"name": "Rifle", "hp_increase": 10},
    ]
    assert calculate_hp(
        {"Rifle": 100.0, "Bad": 100.0}, cast(list[dict], skills_data)
    ) == pytest.approx(90.0)


def test_iter_hp_non_positive_increase_is_skipped_not_break():
    """A skill with hp_increase <= 0 must be skipped (continue), later valid
    skill still counts. Kills `continue -> break` on the <=0 guard (mutmut_14)."""
    skills_data = [
        {"name": "Zero", "hp_increase": 0},
        {"name": "Rifle", "hp_increase": 10},
    ]
    assert calculate_hp({"Rifle": 100.0, "Zero": 100.0}, skills_data) == pytest.approx(
        90.0
    )


def test_iter_hp_missing_name_is_skipped_not_break():
    """A skill with no name must be skipped (continue), later valid skill still
    counts. Kills `continue -> break` on the name guard (mutmut_22)."""
    skills_data = [
        {"name": "", "hp_increase": 10},
        {"name": "Rifle", "hp_increase": 10},
    ]
    assert calculate_hp({"Rifle": 100.0}, skills_data) == pytest.approx(90.0)


# ── profession_level rounding ───────────────────────────────────────────────────


def test_profession_level_rounds_to_two_decimals():
    """profession_level rounds to 2 decimals. Kills the `round(_, 2) -> round(_, 3)`
    mutant (mutmut_11): raw/10000 = 5.1171, which rounds to 5.12 (not 5.117)."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 37}]}
    # 1383 * 37 / 10000 = 5.1171 -> round(,2)=5.12, round(,3)=5.117
    assert profession_level({"Rifle": 1383.0}, profession) == 5.12


# ── all_profession_levels ───────────────────────────────────────────────────────

_ALL_PROFS = [
    {"name": "P1", "skills": [{"skill": {"name": "Rifle"}, "weight": 50}]},
    {"name": "", "skills": [{"skill": {"name": "Aim"}, "weight": 50}]},  # no name
    {"name": "P3", "skills": [{"skill": {"name": "Handgun"}, "weight": 50}]},
]
_ALL_LEVELS = {"Rifle": 1000.0, "Aim": 1000.0, "Handgun": 1000.0}


def test_all_profession_levels_exact_mapping():
    """The named professions map to their computed levels; the empty-name
    profession is dropped. This single assertion pins:
      - result starts as a real dict, not None (mutmut_1)
      - the name is read from the "name" key, not None/wrong key
        (mutmut_2,3,4,5)
      - `if not name` keeps named profs and skips empty (mutmut_6)
      - the empty-name skip is `continue`, not `break`, so P3 survives
        (mutmut_7)
      - the value is profession_level(...), not None (mutmut_8)
      - profession_level is called with (skill_levels, prof) in order, both
        args present (mutmut_9,10,11,12 would raise / mis-call)
    """
    assert all_profession_levels(_ALL_LEVELS, _ALL_PROFS) == {"P1": 5.0, "P3": 5.0}


def test_all_profession_levels_empty_name_skipped_keeps_later():
    """Explicit guard against the `continue -> break` mutant (mutmut_7): the
    empty-name profession sits between P1 and P3, so a break would drop P3."""
    result = all_profession_levels(_ALL_LEVELS, _ALL_PROFS)
    assert "P3" in result
    assert "" not in result
    assert len(result) == 2


# ── skill_rank ──────────────────────────────────────────────────────────────────


def test_skill_rank_drops_rank_with_missing_name():
    """A rank with a present threshold but a None name must be dropped (the
    guard is `threshold is None OR name is None`). Kills the `or -> and`
    mutant (mutmut_14): with `and`, the nameless rank survives and would be
    returned for level 60."""
    ranks = [
        {"name": "Low", "skill": 1},
        {"name": None, "skill": 50},
        {"name": "High", "skill": 100},
    ]
    # Valid ranks are [Low@1, High@100]; level 60 selects Low.
    assert skill_rank(60, cast(list[dict], ranks)) == "Low"


def test_skill_rank_bad_threshold_is_skipped_not_break():
    """A rank with an unparseable threshold must be skipped (continue), so the
    later valid ranks are still considered. Kills `continue -> break` in the
    float() except branch (mutmut_20): a break would empty valid_ranks and
    return 'Unknown'."""
    ranks = [
        {"name": "Bad", "skill": "not-a-number"},
        {"name": "Good", "skill": 1},
        {"name": "Top", "skill": 100},
    ]
    assert skill_rank(150, cast(list[dict], ranks)) == "Top"


# ── profession_skill_optimizer ──────────────────────────────────────────────────
#
# Scenario A: single regular skill Rifle, weight 70, level 500.
#   current_prof = 500*70/10000 = 3.5 ; next = 4 ; gap = 0.5
#   levels_needed = 0.5*10000/70 = 71.42857...  -> round(,1) = 71.4
#   target = 571.4286 ; ped = tt(571.43) - tt(500) = 0.3443 -> round(,2) = 0.34


def _scenario_a():
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 70}]}
    return profession_skill_optimizer({"Rifle": 500.0}, profession)


def test_optimizer_levels_needed_uses_division_and_rounds_to_one_dp():
    """levels_needed = gap * 10000 / weight, rounded to 1 dp = 71.4.

    Kills: 10000/weight -> 10000*weight (mutmut_42, gives 350000);
    levelsNeeded round(,1) -> round(,2) (mutmut_70, 71.43);
    round(_, None) (mutmut_67, 71); round(1) (mutmut_68, 1);
    round(_, ) (mutmut_69, int 71)."""
    s = _scenario_a()["skills"][0]
    assert s["levelsNeeded"] == 71.4


def test_optimizer_ped_is_difference_rounded_to_two_dp():
    """pedToNextLevel = tt(target) - tt(current), rounded to 2 dp = 0.34.

    Kills: subtraction -> addition (mutmut_48, gives 2.72);
    round(ped,2) -> round(2) (mutmut_76, 2); round(_, 3) (mutmut_78, 0.344)."""
    s = _scenario_a()["skills"][0]
    assert s["pedToNextLevel"] == 0.34


def test_optimizer_10000_constant_not_10001():
    """The points-per-level constant is exactly 10000. Kills the
    `10000 -> 10001` mutant (mutmut_44).

    weight 1, Rifle level 5000 => current_prof 0.5, gap 0.5,
    levels_needed = 0.5*10000/1 = 5000.0 exactly (mutant: 0.5*10001 = 5000.5)."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 1}]}
    result = profession_skill_optimizer({"Rifle": 5000.0}, profession)
    assert result["skills"][0]["levelsNeeded"] == 5000.0


def test_optimizer_current_level_from_skill_levels_by_name():
    """currentLevel is skill_levels[name], default 0.0 when absent.

    Scenario A pins the present-skill path (currentLevel 500.0), killing
    get(None, 0.0) (mutmut_21, gives 0.0)."""
    s = _scenario_a()["skills"][0]
    assert s["currentLevel"] == 500.0


def test_optimizer_missing_skill_current_level_defaults_to_zero():
    """A regular skill absent from skill_levels has currentLevel 0.0, not 1.0.
    Kills the default mutant `get(name, 0.0) -> get(name, 1.0)` (mutmut_25)."""
    profession = {"skills": [{"skill": {"name": "Handgun"}, "weight": 50}]}
    result = profession_skill_optimizer({}, profession)
    assert result["skills"][0]["currentLevel"] == 0.0


def test_optimizer_codex_fields_populated_for_known_skill():
    """Rifle is in codex cat1 (divisor 200).

    Kills: codex_cat = None (mutmut_51); get_codex_category(None) (mutmut_52);
    codex_divisor = None (mutmut_53); REWARD_DIVISORS.get(None) (mutmut_54)."""
    s = _scenario_a()["skills"][0]
    assert s["codexCategory"] == "cat1"
    assert s["codexDivisor"] == 200


def test_optimizer_skill_dict_keys_exact():
    """The skill record uses the exact documented keys. Kills the key-rename
    mutants on weight/currentLevel/levelsNeeded/codexCategory/codexDivisor
    (mutmut_58..65, 79..84)."""
    s = _scenario_a()["skills"][0]
    assert set(s.keys()) == {
        "name",
        "weight",
        "currentLevel",
        "levelsNeeded",
        "pedToNextLevel",
        "codexCategory",
        "codexDivisor",
    }
    assert s["weight"] == 70.0
    assert s["currentLevel"] == 500.0
    assert s["levelsNeeded"] == 71.4
    assert s["codexCategory"] == "cat1"
    assert s["codexDivisor"] == 200


def test_optimizer_zero_weight_skill_excluded():
    """A regular skill with weight 0 is excluded (weight <= 0). Kills the
    `weight <= 0 -> weight < 0` mutant (mutmut_17): keeping a weight-0 regular
    skill divides by zero, so the correct code returns no skills here."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 0}]}
    result = profession_skill_optimizer({"Rifle": 500.0}, profession)
    assert result["skills"] == []


def test_optimizer_weight_one_skill_included():
    """A regular skill with weight 1 IS included (boundary > 0). Kills the
    `weight <= 0 -> weight <= 1` mutant (mutmut_18) which would drop it."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 1}]}
    result = profession_skill_optimizer({"Rifle": 500.0}, profession)
    assert [s["name"] for s in result["skills"]] == ["Rifle"]
    assert result["skills"][0]["weight"] == 1.0


def test_optimizer_zero_weight_skip_is_continue_not_break():
    """A weight-0 entry before a valid skill must be skipped (continue), not
    terminate the loop. Kills the `continue -> break` mutant on the weight
    guard (mutmut_19)."""
    profession = {
        "skills": [
            {"skill": {"name": "Handgun"}, "weight": 0},
            {"skill": {"name": "Rifle"}, "weight": 50},
        ]
    }
    result = profession_skill_optimizer({"Handgun": 100.0, "Rifle": 500.0}, profession)
    assert [s["name"] for s in result["skills"]] == ["Rifle"]


def test_optimizer_current_level_precision_two_decimals():
    """currentLevel rounds to 2 dp. Scenario B: Rifle weight 7, level 731.3 ->
    current_prof = 0.51191 -> 0.51.

    Kills: round(current_prof, None) (mutmut_107, -> 1); round(current_prof, )
    (mutmut_109, -> int 1); round(_, 3) (mutmut_110, -> 0.512)."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 7}]}
    result = profession_skill_optimizer({"Rifle": 731.3}, profession)
    assert result["currentLevel"] == 0.51


def test_optimizer_gap_precision_four_decimals():
    """gap rounds to 4 dp. Scenario B: gap = 1 - 0.51191 = 0.48809 -> 0.4881.

    Kills: round(gap, None) (mutmut_117, -> 0); round(gap, ) (mutmut_119,
    -> int 0); round(_, 5) (mutmut_120, -> 0.48809)."""
    profession = {"skills": [{"skill": {"name": "Rifle"}, "weight": 7}]}
    result = profession_skill_optimizer({"Rifle": 731.3}, profession)
    assert result["gap"] == 0.4881


# Scenario D: an attribute (Agility) plus a regular skill, to reach the
# attribute branch of the optimizer.


def _scenario_d():
    profession = {
        "skills": [
            {"skill": {"name": "Agility"}, "weight": 10},
            {"skill": {"name": "Rifle"}, "weight": 50},
        ]
    }
    return profession_skill_optimizer({"Agility": 50.0, "Rifle": 500.0}, profession)


def test_optimizer_attribute_dict_keys_exact():
    """The attribute record uses the exact documented keys. Kills the
    key-rename mutants on weight/currentLevel in the attribute branch
    (mutmut_31..35)."""
    a = _scenario_d()["attributes"][0]
    assert a["name"] == "Agility"
    assert set(a.keys()) == {"name", "weight", "currentLevel", "contributionFactor"}
    assert a["weight"] == 10.0
    assert a["currentLevel"] == 50.0
    assert a["contributionFactor"] == 200.0
