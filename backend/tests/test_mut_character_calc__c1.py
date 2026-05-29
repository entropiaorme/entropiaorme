"""Mutation-hardening tests for ``profession_path_optimizer`` (cluster character_calc__c1).

Each test exercises a specific line/branch of
``backend.services.character_calc.profession_path_optimizer`` and asserts the
exact behaviour a surviving mutant breaks. Tests are grouped by the region of
the function they pin down. The anchor values were captured from the real
(unmutated) implementation against the bundled TT-value curve.
"""

import pytest

from backend.data.tt_value_curve import tt_value_at
from backend.services.character_calc import profession_path_optimizer


def _prof(*skills):
    """Build a profession payload from (name, weight) pairs."""
    return {"skills": [{"skill": {"name": n}, "weight": w} for n, w in skills]}


# ── Argument validation (line 252) ──────────────────────────────────────────
# mutmut 4/5/6/7 mutate the ValueError message text (None / wrapped / lower /
# upper). Asserting both the type AND the exact message kills all four.


def test_xor_validation_message_exact():
    prof = _prof(("Rifle", 50))
    expected = "Exactly one of target_level or ped_budget must be provided"
    # both provided
    with pytest.raises(
        ValueError,
        match=r"^Exactly one of target_level or ped_budget must be provided$",
    ) as e1:
        profession_path_optimizer(
            {"Rifle": 0.0}, prof, target_level=5.0, ped_budget=10.0
        )
    assert str(e1.value) == expected
    # neither provided
    with pytest.raises(
        ValueError,
        match=r"^Exactly one of target_level or ped_budget must be provided$",
    ) as e2:
        profession_path_optimizer({"Rifle": 0.0}, prof)
    assert str(e2.value) == expected


# ── Skill weight filter (lines 263, 21) ──────────────────────────────────────
# mutmut 19 (weight < 0): a weight==0 skill is no longer skipped -> divide by
#   zero in the ratio. mutmut 20 (weight <= 1): a weight==1 skill is wrongly
#   skipped. mutmut 21 (continue -> break): a zero-weight entry would abort the
#   whole loop, dropping every later skill.


def test_zero_and_unit_weight_handling():
    prof = _prof(("WZero", 0), ("WOne", 1), ("Rifle", 50))
    sl = {"WZero": 0.0, "WOne": 0.0, "Rifle": 0.0}
    # Must not raise (weight 0 skill excluded, no ZeroDivisionError) -> kills 19
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    names = sorted(a["name"] for a in result["allocations"])
    # WOne (weight 1) is a valid skill and must be present -> kills 20.
    # Rifle must be present even though a zero-weight skill preceded it -> kills 21.
    assert names == ["Rifle", "WOne"]


# ── Attribute current level default (lines 266, 24/25/27/28) ─────────────────


def test_attribute_currentlevel_present():
    # Attribute present in skill_levels at a non-zero level.
    prof = _prof(("Agility", 10), ("Rifle", 60))
    sl = {"Rifle": 0.0, "Agility": 100.0}
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    agi = next(a for a in result["attributes"] if a["name"] == "Agility")
    # get(name, 0.0): mutmut 24 looks up None -> currentLevel 0.0 (wrong).
    # mutmut 28 uses default 1.0 (irrelevant here but the value must be 100.0).
    assert agi["currentLevel"] == 100.0


def test_attribute_currentlevel_absent_default():
    # Attribute present in the profession but NOT in skill_levels -> default 0.0.
    prof = _prof(("Agility", 10), ("Rifle", 60))
    sl = {"Rifle": 500.0}  # Agility absent
    result = profession_path_optimizer(sl, prof, target_level=4.0)
    agi = next(a for a in result["attributes"] if a["name"] == "Agility")
    # mutmut 25/27 default to None -> TypeError downstream; mutmut 28 -> 1.0.
    assert agi["currentLevel"] == 0.0
    # endLevel must reflect a 0.0 attribute (not 1.0): kills 28.
    assert result["endLevel"] == pytest.approx(4.0, abs=1e-6)


# ── Attribute contribution factor (line 272, 40/41) ──────────────────────────


def test_attribute_contribution_factor_is_weight_times_20():
    prof = _prof(("Agility", 10), ("Rifle", 60))
    sl = {"Rifle": 0.0, "Agility": 50.0}
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    agi = next(a for a in result["attributes"] if a["name"] == "Agility")
    # weight*20 = 200.0 ; mutmut 40 -> 0.5 (weight/20), mutmut 41 -> 210 (weight*21)
    assert agi["contributionFactor"] == 200.0


# ── Excluded entry shape (line 276, 46/47) ───────────────────────────────────


def test_excluded_entry_has_weight_key():
    prof = _prof(("Locked", 50), ("Rifle", 50))
    sl = {"Rifle": 0.0}  # Locked not unlocked
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    assert len(result["excluded"]) == 1
    entry = result["excluded"][0]
    # mutmut 46 -> "XXweightXX", mutmut 47 -> "WEIGHT"
    assert entry["name"] == "Locked"
    assert entry["reason"] == "not unlocked"
    assert entry["weight"] == 50.0
    assert set(entry.keys()) == {"name", "weight", "reason"}


# ── Attribute sort (line 287, 66/67/68/69/70/74) ─────────────────────────────


def test_attributes_sorted_by_contribution_descending():
    prof = _prof(("Agility", 5), ("Health", 20), ("Rifle", 50))
    sl = {"Rifle": 0.0, "Agility": 10.0, "Health": 10.0}
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    attrs = result["attributes"]
    # Health (cf 400) must come before Agility (cf 100); a missing key or a
    # bad/absent reverse flag either raises or yields ascending order.
    assert [a["name"] for a in attrs] == ["Health", "Agility"]
    assert [a["contributionFactor"] for a in attrs] == [400.0, 100.0]


# ── Excluded sort (line 288, 75/76) ──────────────────────────────────────────


def test_excluded_sorted_by_name_ascending():
    prof = _prof(("Zebra", 50), ("Alpha", 50), ("Mango", 50), ("Rifle", 50))
    sl = {"Rifle": 0.0}
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    # mutmut 75/76 give key=None / lambda->None -> TypeError sorting dicts.
    assert [e["name"] for e in result["excluded"]] == ["Alpha", "Mango", "Zebra"]


# ── Early-return path when target already reached (lines 293-304) ────────────


def test_target_already_reached_returns_full_dict():
    prof = _prof(("Agility", 10), ("Locked", 50), ("Rifle", 100))
    sl = {"Rifle": 500.0, "Agility": 100.0}  # current prof = 7.0; Locked excluded
    result = profession_path_optimizer(sl, prof, target_level=3.0)
    # mode/target/budget args must be carried through (mutmut 89/90).
    assert result["mode"] == "target"
    assert result["inputTargetLevel"] == 3.0
    assert result["inputPedBudget"] is None
    # attributes must be the real list, not None (mutmut 95) and not shifted
    # (mutmut 103). excluded must be carried, not dropped (mutmut 96/104).
    assert [a["name"] for a in result["attributes"]] == ["Agility"]
    assert [e["name"] for e in result["excluded"]] == ["Locked"]
    assert [a["name"] for a in result["allocations"]] == ["Rifle"]
    assert result["endLevel"] == pytest.approx(7.0, abs=1e-6)
    assert result["totalPed"] == 0.0
    assert result["professionLevelsGained"] == 0.0


# ── Greedy target loop: marginal cost & step accounting ──────────────────────
# Two equal-weight skills at level 0 with target +1 level: the optimiser fills
# the cheaper marginal steps first. Index 0 (Rifle) wins ties (strict <).


def test_target_two_equal_skills_split_and_cost():
    prof = _prof(("Rifle", 50), ("Aim", 50))
    sl = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    allocs = {a["name"]: a for a in result["allocations"]}
    assert result["endLevel"] == pytest.approx(1.0, abs=1e-6)
    assert result["totalPed"] == pytest.approx(0.25, abs=1e-6)
    # Exact greedy split (kills marginal-cost mutants 129/132 and tie-break 138).
    assert allocs["Rifle"]["levelsToGain"] == 148.0
    assert allocs["Aim"]["levelsToGain"] == 52.0
    # pedCost from the difference of curve anchors, not a sum (mutmut 182/185).
    assert allocs["Rifle"]["pedCost"] == pytest.approx(0.19, abs=1e-6)
    assert allocs["Aim"]["pedCost"] == pytest.approx(0.06, abs=1e-6)


def test_target_tiebreak_prefers_first_index():
    # On an exact ratio tie the FIRST skill (index 0) must receive the larger
    # allocation. mutmut 138 (ratio <= best_ratio) flips this to the last.
    prof = _prof(("Rifle", 50), ("Aim", 50))
    sl = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    allocs = {a["name"]: a["levelsToGain"] for a in result["allocations"]}
    assert allocs["Rifle"] > allocs["Aim"]


def test_target_fractional_tail_is_added():
    # Final partial step lands in (0, 1] profession point. mutmut 110
    # (while > 1) stops one fractional step short.
    prof = _prof(("Rifle", 50))
    sl = {"Rifle": 10000.0}  # current prof 50.0
    result = profession_path_optimizer(sl, prof, target_level=50.01005)
    alloc = result["allocations"][0]
    # 2 full steps + a 0.01-level fractional tail -> 2.01 levels.
    assert alloc["levelsToGain"] == pytest.approx(2.01, abs=1e-6)


def test_target_fractional_step_pos_and_ped():
    # Large allocation before a fractional final step: the fractional pos must
    # be currentLevel + allocated (mutmut 153 uses '-' -> wrong frac_ped).
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 8000.0}
    result = profession_path_optimizer(sl, prof, target_level=95.005)
    alloc = result["allocations"][0]
    assert alloc["levelsToGain"] == pytest.approx(1500.5, abs=1e-6)
    assert alloc["pedCost"] == pytest.approx(1072.15, abs=1e-2)


def test_target_high_level_fractional_pedcost():
    # frac_ped = tt(pos+frac) - tt(pos); mutmut 160 (+), 162 (pos-frac),
    # 169 (ped -= frac) all change the fractional pedCost.
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 15000.0}
    result = profession_path_optimizer(sl, prof, target_level=150.025)
    alloc = result["allocations"][0]
    assert alloc["levelsToGain"] == pytest.approx(2.5, abs=1e-6)
    assert alloc["pedCost"] == pytest.approx(3.86, abs=1e-2)


def test_target_step_pos_offset():
    # mutmut 132/185 use pos+2 instead of pos+1 for the per-step marginal.
    prof = _prof(("Rifle", 50), ("Aim", 50))
    sl = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(sl, prof, target_level=1.0)
    allocs = {a["name"]: a["levelsToGain"] for a in result["allocations"]}
    assert allocs["Rifle"] == 148.0 and allocs["Aim"] == 52.0


# ── Greedy target loop: ceiling handling ─────────────────────────────────────


def test_target_skill_exactly_at_ceiling_is_skipped():
    # Skill exactly at the curve ceiling: pos >= max -> skip. mutmut 126
    # (pos > max) tries to allocate past the ceiling.
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 20000.0}  # at ceiling, prof 200.0
    result = profession_path_optimizer(sl, prof, target_level=201.0)
    assert result["endLevel"] == pytest.approx(200.0, abs=1e-6)
    assert result["allocations"][0]["levelsToGain"] == 0.0


def test_target_all_skills_at_ceiling_returns_dict():
    # All skills at ceiling, target unreachable -> break and return the dict.
    # mutmut 143 (break -> return) returns None; mutmut 111 (best_idx None)
    # raises; mutmut 112 (best_idx +1) allocates a non-existent slot.
    prof = _prof(("Rifle", 100), ("Aim", 50))
    sl = {"Rifle": 20000.0, "Aim": 20000.0}
    result = profession_path_optimizer(sl, prof, target_level=1000.0)
    assert isinstance(result, dict)
    assert result["endLevel"] == pytest.approx(300.0, abs=1e-6)


def test_target_continue_skips_only_maxed_skill():
    # A maxed skill listed first must be SKIPPED (continue) so a later skill is
    # still allocated. mutmut 127 (continue -> break) aborts the inner loop.
    prof = _prof(("Maxed", 50), ("Rifle", 50))
    sl = {"Maxed": 20000.0, "Rifle": 0.0}  # current prof 100.0
    result = profession_path_optimizer(sl, prof, target_level=101.0)
    allocs = {a["name"]: a["levelsToGain"] for a in result["allocations"]}
    assert allocs["Rifle"] == 200.0
    assert result["endLevel"] == pytest.approx(101.0, abs=1e-6)


# ── Budget loop: basics, selection, accounting ───────────────────────────────


def test_budget_two_equal_skills_split_and_cost():
    prof = _prof(("Rifle", 50), ("Aim", 50))
    sl = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(sl, prof, ped_budget=1.0)
    allocs = {a["name"]: a for a in result["allocations"]}
    assert result["mode"] == "budget"
    assert result["totalPed"] == pytest.approx(1.0, abs=1e-6)
    assert result["endLevel"] == pytest.approx(2.48, abs=1e-6)
    # marginal cost is a difference (mutmut 224 uses +, 227 uses pos+2).
    assert allocs["Rifle"]["levelsToGain"] == 445.0
    assert allocs["Aim"]["levelsToGain"] == 52.0


def test_budget_tiebreak_prefers_first_index():
    # mutmut 233 (ratio <= best_ratio) flips the budget-loop tie-break.
    prof = _prof(("Rifle", 50), ("Aim", 50))
    sl = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(sl, prof, ped_budget=1.0)
    allocs = {a["name"]: a["levelsToGain"] for a in result["allocations"]}
    assert allocs["Rifle"] > allocs["Aim"]


def test_budget_ratio_uses_division_by_weight():
    # Higher weight = more profession points per PED, so the high-weight skill
    # dominates. mutmut 230 (ratio *= weight) inverts this preference.
    prof = _prof(("Rifle", 100), ("Aim", 10))
    sl = {"Rifle": 0.0, "Aim": 0.0}
    result = profession_path_optimizer(sl, prof, ped_budget=1.0)
    allocs = {a["name"]: a["levelsToGain"] for a in result["allocations"]}
    assert allocs["Rifle"] > allocs["Aim"]
    assert result["endLevel"] == pytest.approx(4.59, abs=1e-6)


def test_budget_zero_budget_does_not_allocate():
    # while budget_remaining > 1e-6: a budget of exactly 1e-6 must NOT trigger a
    # step. mutmut 202 (>= 1e-6) runs the loop once and allocates a free level.
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 0.0}
    result = profession_path_optimizer(sl, prof, ped_budget=1e-6)
    assert result["endLevel"] == pytest.approx(0.0, abs=1e-9)
    assert result["allocations"][0]["levelsToGain"] == 0.0


def test_budget_all_skills_at_ceiling_returns_dict():
    # mutmut 239 (break -> return) -> None; mutmut 204 (best_idx None) raises.
    prof = _prof(("Rifle", 100), ("Aim", 50))
    sl = {"Rifle": 20000.0, "Aim": 20000.0}
    result = profession_path_optimizer(sl, prof, ped_budget=100.0)
    assert isinstance(result, dict)
    assert result["endLevel"] == pytest.approx(300.0, abs=1e-6)


def test_budget_continue_skips_only_maxed_skill():
    # Budget mode: a maxed skill listed first must be SKIPPED (continue) so a
    # later skill is still allocated. mutmut 222 (continue -> break) aborts the
    # inner loop, leaving nothing allocated.
    prof = _prof(("Maxed", 50), ("Rifle", 50))
    sl = {"Maxed": 20000.0, "Rifle": 0.0}  # current prof 100.0
    result = profession_path_optimizer(sl, prof, ped_budget=1.0)
    allocs = {a["name"]: a["levelsToGain"] for a in result["allocations"]}
    assert allocs["Rifle"] == 459.0
    assert result["endLevel"] == pytest.approx(102.29, abs=1e-2)


def test_budget_input_echoed():
    # mutmut 310 nulls inputPedBudget in the final return; mutmut 309 nulls
    # inputTargetLevel in the target-mode final return.
    prof = _prof(("Rifle", 50))
    rb = profession_path_optimizer({"Rifle": 0.0}, prof, ped_budget=2.0)
    assert rb["inputPedBudget"] == 2.0
    assert rb["inputTargetLevel"] is None
    rt = profession_path_optimizer({"Rifle": 0.0}, prof, target_level=1.0)
    assert rt["inputTargetLevel"] == 1.0
    assert rt["inputPedBudget"] is None


# ── Budget loop: fractional final step accounting (high level) ───────────────


def test_budget_fractional_final_step_accounting():
    # Budget funds 2 full steps + a 0.5-level fractional tail at level 15000.
    # Pins mutmut 257 (allocated = frac), 258 (allocated -= frac),
    # 261 (ped = budget), 262 (ped -= budget), 271 (allocated += 2),
    # 267 (allocated = 1), 268 (allocated -= 1).
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 15000.0}
    step = tt_value_at(15001) - tt_value_at(15000)
    result = profession_path_optimizer(sl, prof, ped_budget=step * 2.5)
    alloc = result["allocations"][0]
    assert alloc["levelsToGain"] == pytest.approx(2.51, abs=1e-2)
    assert alloc["pedCost"] == pytest.approx(3.87, abs=1e-2)


def test_budget_full_step_pos_in_budget_mode():
    # mutmut 215 uses pos = currentLevel - allocated inside the budget ratio
    # loop. With many full steps the running position is wrong and the final
    # allocation drifts. Anchored against the real implementation.
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 10000.0}
    result = profession_path_optimizer(sl, prof, ped_budget=50.0)
    alloc = result["allocations"][0]
    assert alloc["levelsToGain"] == pytest.approx(41.33, abs=1e-2)
    assert alloc["newLevel"] == pytest.approx(10041.33, abs=1e-2)


def test_budget_allocated_decrement_drops_allocation():
    # mutmut 268 (allocated -= 1) drives the level negative; at a high start it
    # terminates with an allocation that is no longer positive.
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 15000.0}
    step = tt_value_at(15001) - tt_value_at(15000)
    result = profession_path_optimizer(sl, prof, ped_budget=step * 2.5)
    # The single skill must gain a positive number of levels.
    pos_alloc = [a for a in result["allocations"] if a["levelsToGain"] > 0]
    assert len(pos_alloc) == 1
    assert result["endLevel"] > result["currentLevel"]


def test_budget_zero_fraction_at_ceiling_returns_dict():
    # Tiny budget at a very high level buys < 0.00005 levels (frac rounds to 0):
    # the fractional branch must break and the function must still return a dict
    # (mutmut 256 turns the break into a bare return -> None).
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 19000.0}  # prof 190.0
    result = profession_path_optimizer(sl, prof, ped_budget=1e-4)
    assert isinstance(result, dict)
    assert result["endLevel"] == pytest.approx(190.0, abs=1e-6)


def test_budget_fractional_partial_level_allocated():
    # A fractional final step with 0 < frac_levels < 1 must add the partial
    # level (mutmut 255 breaks before allocating it).
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 15000.0}
    step = tt_value_at(15001) - tt_value_at(15000)
    result = profession_path_optimizer(sl, prof, ped_budget=step * 1.5)
    alloc = result["allocations"][0]
    assert alloc["levelsToGain"] == pytest.approx(1.5, abs=1e-2)


def test_budget_full_step_boundary_takes_full_step():
    # Budget exactly equal to one full step cost: orig takes the full step
    # (1.0 level). mutmut 241 (best_ped >= budget) drops into the fractional
    # branch and lands at ~0.99 levels.
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 100.0}
    step = tt_value_at(101) - tt_value_at(100)
    result = profession_path_optimizer(sl, prof, ped_budget=step)
    alloc = result["allocations"][0]
    assert alloc["levelsToGain"] == pytest.approx(1.0, abs=1e-9)


# ── End-of-profession recomputation (lines 380-385, 291/292/293/294) ─────────


def test_endlevel_accumulates_attribute_contributions():
    prof = _prof(("Agility", 10), ("Rifle", 100))
    sl = {"Rifle": 500.0, "Agility": 50.0}  # current prof 6.0
    result = profession_path_optimizer(sl, prof, ped_budget=1.0)
    # endLevel must include both the allocated skill gain AND the (unchanged)
    # attribute contribution. mutmut 291 (=) overwrites, 292 (-=) subtracts,
    # 293 (/) divides, 294 (effective_points(None,...)) drops the x20 factor.
    assert result["currentLevel"] == pytest.approx(6.0, abs=1e-6)
    assert result["endLevel"] == pytest.approx(8.5, abs=1e-6)
    assert result["professionLevelsGained"] == pytest.approx(2.5, abs=1e-6)


# ── Genuine infinite-loop mutants (watchdog-caught) ──────────────────────────
# mutmut 109 (target loop `while points_remaining >= 0`) and mutmut 205
# (budget `best_idx = +1`) turn a terminating loop into an infinite one once
# the goal is reached / the ceiling is hit. These tests drive those exact paths
# so the engine's wall-clock watchdog fires deterministically.


def test_target_loop_reaches_goal_and_terminates():
    # Drives points_remaining to exactly 0 via a fractional final step: under
    # mutmut 109 (`>= 0`) this never exits.
    prof = _prof(("Rifle", 100))
    sl = {"Rifle": 0.0}
    result = profession_path_optimizer(sl, prof, target_level=0.01)
    assert result["endLevel"] == pytest.approx(0.01, abs=1e-6)


def test_budget_loop_ceiling_terminates():
    # All skills at ceiling with budget remaining: under mutmut 205
    # (`best_idx = +1`) the loop never breaks.
    prof = _prof(("Rifle", 100), ("Aim", 50))
    sl = {"Rifle": 20000.0, "Aim": 20000.0}
    result = profession_path_optimizer(sl, prof, ped_budget=50.0)
    assert isinstance(result, dict)
    assert result["endLevel"] == pytest.approx(300.0, abs=1e-6)
