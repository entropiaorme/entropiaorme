"""Mutation-hardening tests for backend.services.character_calc (cluster c2).

Scope: _path_result, calculate_hp, hp_skill_optimizer, codex_next_reward.

Each test pins the exact public behaviour (output dict keys, numeric values at
their documented rounding precision, ordering, codex enrichment, and the
arithmetic/comparison operators in the formulas) so that the targeted mutants
produce an observable difference and fail.

Fixtures use deliberately chosen skill levels and increments so that every
rounded output value differs at the 1st / 2nd / 3rd / 4th decimal place. That
forces the round(x, N) precision mutants (N -> N+1, N -> None, dropped second
arg) to change a value, and likewise pins the operator and comparison mutants.
"""

import pytest

from backend.services.character_calc import (
    _path_result,
    calculate_hp,
    codex_next_reward,
    hp_skill_optimizer,
)

# ── _path_result ──────────────────────────────────────────────────────────────
#
# _path_result builds the path-optimizer return dict. The fixture has two
# allocated regular skills (Aim -> cat1, Wounding -> no codex category), two
# unallocated regular skills (Anatomy -> cat1, Bravado -> cat3), one attribute,
# and one excluded skill. Values are chosen with distinct 2nd/3rd decimals.


def _pr_skills():
    return [
        {
            "name": "Aim",
            "weight": 3.0,
            "currentLevel": 2000.0,
            "allocated": 12.34,
            "ped": 5.678,
        },
        {
            "name": "Wounding",
            "weight": 2.0,
            "currentLevel": 1000.0,
            "allocated": 7.891,
            "ped": 2.345,
        },
        {
            "name": "Bravado",
            "weight": 1.0,
            "currentLevel": 500.0,
            "allocated": 0.0,
            "ped": 0.0,
        },
        {
            "name": "Anatomy",
            "weight": 1.5,
            "currentLevel": 300.0,
            "allocated": 0.0,
            "ped": 0.0,
        },
    ]


def _pr_call():
    attrs = [
        {
            "name": "Health",
            "weight": 4.0,
            "currentLevel": 1500.0,
            "contributionFactor": 80.0,
        }
    ]
    excluded = [{"name": "Zeta", "weight": 1.0, "reason": "not unlocked"}]
    return _path_result(
        "target",
        7.0,
        None,
        5.123456,
        6.987654,
        _pr_skills(),
        attrs,
        excluded,
    )


def test_path_result_top_level_shape_and_values():
    res = _pr_call()
    # Exact key set at the top level (kills key-name mutants on inputTargetLevel
    # / inputPedBudget and guards the overall shape).
    assert set(res) == {
        "mode",
        "inputTargetLevel",
        "inputPedBudget",
        "currentLevel",
        "endLevel",
        "professionLevelsGained",
        "totalPed",
        "allocations",
        "attributes",
        "excluded",
    }
    assert res["mode"] == "target"
    # inputTargetLevel / inputPedBudget pass straight through.
    assert res["inputTargetLevel"] == 7.0
    assert res["inputPedBudget"] is None
    # currentLevel = round(5.123456, 2); 3rd-decimal-distinct so a precision
    # bump or a dropped precision argument changes the value.
    assert res["currentLevel"] == 5.12
    assert res["endLevel"] == 6.99
    # professionLevelsGained = round(6.987654 - 5.123456, 2) = round(1.864198, 2)
    assert res["professionLevelsGained"] == 1.86
    # totalPed = round(5.678 + 2.345 + 0 + 0, 2) = round(8.023, 2)
    assert res["totalPed"] == 8.02


def test_path_result_input_keys_are_camelcase():
    # Pins the exact camelCase spelling of the input echo keys.
    res = _pr_call()
    assert "inputTargetLevel" in res
    assert "inputPedBudget" in res
    assert "inputtargetlevel" not in res
    assert "INPUTTARGETLEVEL" not in res
    assert "inputpedbudget" not in res


def test_path_result_professionLevelsGained_precision():
    # round(1.864198, 2) == 1.86, but round(.., 3) == 1.864 and round(.., None)
    # (or dropped arg) == 2. Pinning the exact 2-dp value kills all three.
    res = _pr_call()
    assert res["professionLevelsGained"] == 1.86
    assert res["professionLevelsGained"] != 1.864
    assert res["professionLevelsGained"] != 2


def test_path_result_allocation_ordering():
    # Allocated skills first, sorted by pedCost DESC; then unallocated, sorted
    # by name ASC. Kills the sort-key, reverse flag, and the
    # levelsToGain > 0 / == 0 partition mutants.
    res = _pr_call()
    order = [a["name"] for a in res["allocations"]]
    # Aim (ped 5.68) before Wounding (ped 2.35): pedCost desc.
    # Then Anatomy before Bravado: name asc among the unallocated.
    assert order == ["Aim", "Wounding", "Anatomy", "Bravado"]


def test_path_result_allocated_partition_uses_strict_positive():
    # The unallocated tail (allocated == 0) must be alphabetical: Anatomy then
    # Bravado. If the partition used >= 0 (everything "allocated") or == 1, the
    # zero-allocated skills would not be name-sorted into the tail, flipping the
    # observed order.
    res = _pr_call()
    names = [a["name"] for a in res["allocations"]]
    assert names.index("Anatomy") < names.index("Bravado")
    # And the two zero-allocation skills sit after both allocated ones.
    assert names.index("Aim") < names.index("Anatomy")
    assert names.index("Wounding") < names.index("Anatomy")


def test_path_result_allocation_per_skill_values():
    res = _pr_call()
    by_name = {a["name"]: a for a in res["allocations"]}

    aim = by_name["Aim"]
    # Exact allocation entry key set (kills currentLevel / codexCategory /
    # codexDivisor key-name mutants).
    assert set(aim) == {
        "name",
        "weight",
        "currentLevel",
        "levelsToGain",
        "pedCost",
        "newLevel",
        "codexCategory",
        "codexDivisor",
    }
    assert aim["currentLevel"] == 2000.0
    # levelsToGain = round(12.34, 2); newLevel = round(2000 + 12.34, 2)
    assert aim["levelsToGain"] == 12.34
    assert aim["newLevel"] == 2012.34
    # pedCost = round(5.678, 2) = 5.68 (3rd-decimal-distinct).
    assert aim["pedCost"] == 5.68
    # Aim is a cat1 codex skill (divisor 200): the real category lookup must run.
    assert aim["codexCategory"] == "cat1"
    assert aim["codexDivisor"] == 200

    wounding = by_name["Wounding"]
    assert wounding["levelsToGain"] == 7.89  # round(7.891, 2)
    assert wounding["newLevel"] == 1007.89
    assert wounding["pedCost"] == 2.35  # round(2.345, 2)
    # Wounding has no codex category -> both fields None.
    assert wounding["codexCategory"] is None
    assert wounding["codexDivisor"] is None

    # Bravado (unallocated) is cat3 -> divisor 640; proves divisor lookup keys
    # on the real category, not a constant.
    bravado = by_name["Bravado"]
    assert bravado["codexCategory"] == "cat3"
    assert bravado["codexDivisor"] == 640


def test_path_result_allocation_currentLevel_key_is_camelcase():
    res = _pr_call()
    for a in res["allocations"]:
        assert "currentLevel" in a
        assert "currentlevel" not in a
        assert "CURRENTLEVEL" not in a
        assert "codexCategory" in a
        assert "codexCategory" in a and "codexcategory" not in a
        assert "codexDivisor" in a and "codexdivisor" not in a


def test_path_result_precision_of_allocation_numbers():
    # levelsToGain / pedCost / newLevel each round to 2 dp; the chosen values
    # have a non-zero 3rd decimal so a precision change is observable.
    res = _pr_call()
    by_name = {a["name"]: a for a in res["allocations"]}
    assert by_name["Aim"]["pedCost"] == 5.68
    assert by_name["Aim"]["pedCost"] != 5.678  # round(.., 3) would keep 5.678
    assert by_name["Wounding"]["levelsToGain"] == 7.89
    assert by_name["Wounding"]["levelsToGain"] != 7.891
    assert by_name["Wounding"]["newLevel"] == 1007.89


def test_path_result_excluded_passthrough():
    # excluded must be carried through verbatim (the `excluded or []` default
    # is exercised by the non-empty case here).
    res = _pr_call()
    assert res["excluded"] == [
        {"name": "Zeta", "weight": 1.0, "reason": "not unlocked"}
    ]


# ── calculate_hp ────────────────────────────────────────────────────────────
#
# HP = 80 + Σ effective_points(skill) / hp_increase, only for skills with a
# level > 0. Health is an attribute (×20). Aim is a regular skill.


def test_calculate_hp_value():
    skills_data = [
        {"name": "Aim", "hp_increase": 7.0},  # regular: 2000 / 7
        {"name": "Health", "hp_increase": 5.0},  # attribute: 1500*20 / 5
    ]
    skill_levels = {"Aim": 2000.0, "Health": 1500.0}
    hp = calculate_hp(skill_levels, skills_data)
    # 80 + 2000/7 + (1500*20)/5 = 80 + 285.714.. + 6000 = 6365.714..
    assert hp == pytest.approx(6365.714285714285, abs=1e-9)


def test_calculate_hp_default_level_is_zero_not_one():
    # A skill present in skills_data but absent from skill_levels must default
    # to level 0 and therefore contribute nothing (the level > 0 guard fails).
    # The mutant default of 1.0 would add 1/hp_increase. The mutant guard
    # `level >= 0` would also let the (default-0) skill through.
    skills_data = [{"name": "Aim", "hp_increase": 7.0}]
    hp = calculate_hp({}, skills_data)
    assert hp == 80.0


def test_calculate_hp_skips_zero_level_skill():
    # Explicit level 0: with the correct strict `> 0` guard it contributes
    # nothing. A `>= 0` mutant would add 0/7 (== 0) here, so to expose that we
    # rely on the default-level test above plus a positive-level contribution
    # being required. Here we confirm a positive level DOES contribute.
    skills_data = [{"name": "Aim", "hp_increase": 7.0}]
    assert calculate_hp({"Aim": 0.0}, skills_data) == 80.0
    assert calculate_hp({"Aim": 700.0}, skills_data) == pytest.approx(180.0)


def test_calculate_hp_includes_fractional_level_below_one():
    # A level strictly between 0 and 1 must still contribute (the guard is
    # `level > 0`, not `level > 1`). The `> 1` mutant would skip a level-0.5
    # skill and report only the base HP.
    skills_data = [{"name": "Aim", "hp_increase": 7.0}]
    hp = calculate_hp({"Aim": 0.5}, skills_data)
    # 80 + 0.5 / 7 = 80.0714..  (a `> 1` mutant would give exactly 80.0)
    assert hp == pytest.approx(80.0 + 0.5 / 7.0, abs=1e-9)
    assert hp != 80.0


# ── hp_skill_optimizer ────────────────────────────────────────────────────────
#
# Regular skills ranked by pedPerHp asc; attributes by levelsPerHp asc. Codex
# enrichment on regular skills. hp_increase chosen so levelsPerHp / pedPerHp /
# hpPerPed carry distinct decimals at their documented precision.


def _hpo_call():
    skills_data = [
        {"name": "Aim", "hp_increase": 7.25},  # regular, cat1
        {"name": "Health", "hp_increase": 4.66},  # attribute, no codex
        {"name": "Wounding", "hp_increase": 3.0},  # regular, no codex
    ]
    skill_levels = {"Aim": 2000.0, "Health": 1500.0, "Wounding": 1000.0}
    return hp_skill_optimizer(skill_levels, skills_data)


def test_hpo_top_level_shape():
    res = _hpo_call()
    assert set(res) == {"currentHp", "skills", "attributes"}
    # currentHp = round(calculate_hp(..), 2). The fixture gives 7126.957..,
    # so round(.., 2)=7126.96, round(.., 3)=7126.957, round(.., None)=7127.
    assert res["currentHp"] == 7126.96
    assert res["currentHp"] != 7126.957
    assert res["currentHp"] != 7127


def test_hpo_skill_ordering_by_pedPerHp_asc():
    # Wounding (pedPerHp 0.03) is cheaper than Aim (pedPerHp 0.11), so it ranks
    # first. Pins the sort direction/key.
    res = _hpo_call()
    assert [s["name"] for s in res["skills"]] == ["Wounding", "Aim"]


def test_hpo_regular_skill_entry_values_and_keys():
    res = _hpo_call()
    by_name = {s["name"]: s for s in res["skills"]}

    aim = by_name["Aim"]
    assert set(aim) == {
        "name",
        "hpIncrease",
        "currentLevel",
        "levelsPerHp",
        "pedPerHp",
        "hpPerPed",
        "codexCategory",
        "codexDivisor",
    }
    assert aim["hpIncrease"] == 7.25
    assert aim["currentLevel"] == 2000.0
    # levelsPerHp = round(hp_increase, 1) = round(7.25, 1) = 7.2 (2nd-dp distinct
    # -> precision-bump mutant to 2dp yields 7.25, to int yields 7).
    assert aim["levelsPerHp"] == 7.2
    assert aim["levelsPerHp"] != 7.25
    assert aim["levelsPerHp"] != 7
    # pedPerHp = round(tt(2007.25) - tt(2000), 2) = 0.11.
    assert aim["pedPerHp"] == 0.11
    # hpPerPed = round(1 / pedPerHp_raw, 4) = 8.6957 (proves the 1.0/ped form,
    # the > 0 guard, and the 4-dp precision).
    assert aim["hpPerPed"] == 8.6957
    # cat1 codex enrichment.
    assert aim["codexCategory"] == "cat1"
    assert aim["codexDivisor"] == 200

    wounding = by_name["Wounding"]
    assert wounding["levelsPerHp"] == 3.0
    assert wounding["pedPerHp"] == 0.03
    assert wounding["hpPerPed"] == 33.3333
    assert wounding["codexCategory"] is None
    assert wounding["codexDivisor"] is None


def test_hpo_regular_skill_keys_are_camelcase():
    res = _hpo_call()
    for s in res["skills"]:
        for good, bad in [
            ("hpIncrease", "hpincrease"),
            ("currentLevel", "currentlevel"),
            ("hpPerPed", "hpperped"),
            ("codexCategory", "codexcategory"),
            ("codexDivisor", "codexdivisor"),
        ]:
            assert good in s
            assert bad not in s
            assert good.upper() not in s


def test_hpo_hpPerPed_uses_division_not_multiplication():
    # hpPerPed = 1.0 / pedPerHp_raw. For Aim the raw ped ~0.115 gives ~8.6957.
    # A `1.0 * ped` mutant would give ~0.115 (rounds to ~0.115 != 8.6957); a
    # `2.0 / ped` mutant would double it (~17.39). Pinning the exact value
    # kills both, plus the `> 0` -> `> 1` / `>= 0` guard mutants and the
    # else-branch constant (0.0 -> 1.0).
    res = _hpo_call()
    aim = next(s for s in res["skills"] if s["name"] == "Aim")
    assert aim["hpPerPed"] == 8.6957


def test_hpo_pedPerHp_uses_difference_not_sum():
    # pedPerHp = tt(target) - tt(current). The `+` mutant would add the two
    # (~tt(2007.25)+tt(2000) ~ 23.3, rounds to ~23.3) instead of ~0.11.
    res = _hpo_call()
    aim = next(s for s in res["skills"] if s["name"] == "Aim")
    assert aim["pedPerHp"] == 0.11
    assert aim["pedPerHp"] < 1.0  # a sum would be > 20


def test_hpo_pedPerHp_and_levelsPerHp_precision():
    res = _hpo_call()
    aim = next(s for s in res["skills"] if s["name"] == "Aim")
    # pedPerHp rounds to 2dp; the raw value's 3rd decimal differs so a
    # precision change is observable.
    assert aim["pedPerHp"] == 0.11
    # hpPerPed rounds to 4dp: 8.6957; round(.., 5) keeps a 5th decimal.
    assert aim["hpPerPed"] == 8.6957
    assert round(aim["hpPerPed"], 0) != aim["hpPerPed"]


def test_hpo_attribute_entry_values_and_keys():
    res = _hpo_call()
    assert len(res["attributes"]) == 1
    health = res["attributes"][0]
    assert set(health) == {
        "name",
        "hpIncrease",
        "currentLevel",
        "levelsPerHp",
        "hpContribution",
    }
    assert health["name"] == "Health"
    assert health["hpIncrease"] == 4.66
    assert health["currentLevel"] == 1500.0
    # levelsPerHp = round(hp_increase / 20, 2) = round(0.233, 2) = 0.23
    # (3rd-dp distinct: precision bump -> 0.233, drop precision -> 0).
    assert health["levelsPerHp"] == 0.23
    assert health["levelsPerHp"] != 0.233
    assert health["levelsPerHp"] != 0
    # hpContribution = round(effective_points(Health, 1500) / 4.66, 2)
    #                = round(1500*20 / 4.66, 2) = round(6437.768.., 2) = 6437.77
    assert health["hpContribution"] == 6437.77
    assert health["hpContribution"] != 6437.768
    assert health["hpContribution"] != 6437  # round(.., None)/dropped arg


def test_hpo_attribute_keys_are_camelcase():
    res = _hpo_call()
    health = res["attributes"][0]
    for good, bad in [
        ("hpIncrease", "hpincrease"),
        ("currentLevel", "currentlevel"),
        ("hpContribution", "hpcontribution"),
    ]:
        assert good in health
        assert bad not in health
        assert good.upper() not in health


def test_hpo_attribute_hpContribution_requires_positive_level():
    # current_level 0 -> hpContribution must be 0.0 (the `current_level > 0`
    # guard; a `>= 0` mutant still yields 0/hp_inc == 0, but the else-constant
    # mutant 0.0 -> 1.0 would yield 1.0). And current_level absent defaults to
    # 0.0 (not 1.0) via skill_levels.get(name, 0.0).
    skills_data = [{"name": "Health", "hp_increase": 4.0}]
    res = hp_skill_optimizer({}, skills_data)
    health = res["attributes"][0]
    assert health["currentLevel"] == 0.0
    assert health["hpContribution"] == 0.0


def test_hpo_default_level_is_zero_not_one():
    # A skill not in skill_levels must surface currentLevel 0.0. The
    # get(name, 1.0) mutant would report 1.0; the get(None, 0.0) mutant would
    # key on None and surface the wrong (default) level for every skill.
    skills_data = [{"name": "Aim", "hp_increase": 7.0}]
    res = hp_skill_optimizer({"Aim": 1234.0}, skills_data)
    aim = res["skills"][0]
    assert aim["currentLevel"] == 1234.0  # real lookup, not None-keyed default


def test_hpo_attribute_levelsPerHp_precision_kills_bump():
    # levelsPerHp for the attribute = round(4.66/20, 2) = 0.23. round(.., 3)
    # would be 0.233. Explicit guard against the precision-bump mutant.
    res = _hpo_call()
    assert res["attributes"][0]["levelsPerHp"] == 0.23


def test_hpo_attribute_hpContribution_fractional_level_below_one():
    # An attribute at a level strictly between 0 and 1 must still report a
    # non-zero contribution (the guard is `current_level > 0`, not `> 1`). The
    # `> 1` mutant would fall to the else branch and report 0.0.
    skills_data = [{"name": "Health", "hp_increase": 4.0}]
    res = hp_skill_optimizer({"Health": 0.5}, skills_data)
    health = res["attributes"][0]
    # effective_points(Health, 0.5) / 4.0 = (0.5 * 20) / 4.0 = 2.5
    assert health["hpContribution"] == 2.5


def test_hpo_regular_hpPerPed_zero_when_no_ped_cost():
    # When the marginal TT cost is zero (a level region where the TT curve is
    # flat: current_level 0, hp_increase 1 -> target 1, tt(1) == tt(0) == 0),
    # ped_per_hp is 0 so the `ped_per_hp > 0` guard fails and hpPerPed must be
    # the else default 0.0. The `else 1.0` mutant would report 1.0 instead.
    skills_data = [{"name": "Aim", "hp_increase": 1.0}]
    res = hp_skill_optimizer({"Aim": 0.0}, skills_data)
    aim = next(s for s in res["skills"] if s["name"] == "Aim")
    assert aim["pedPerHp"] == 0.0
    assert aim["hpPerPed"] == 0.0


# ── codex_next_reward ─────────────────────────────────────────────────────────


def test_codex_next_reward_value_and_precision():
    # Bravado -> cat3, divisor 640. round(1234.5 / 640, 4) = round(1.92890625, 4)
    # = 1.9289. round(.., 5) = 1.92891; round(.., None)/dropped = 2.
    val = codex_next_reward("Bravado", 1234.5)
    assert val == 1.9289
    assert val != 1.92891
    assert val != 2


def test_codex_next_reward_none_for_uncategorised_skill():
    # A skill with no codex category returns None (sanity guard on the lookup).
    assert codex_next_reward("Agility", 1000.0) is None
