"""Mutation-hardening tests for backend.data.codex_categories.

Targets the surviving mutants in the codex_categories cluster:

* x_get_reward_ped__mutmut_12  -- round(..., 4) -> round(..., 5)
* x_build_rank_breakdown__mutmut_15 -- reward = get_reward_ped(...) -> reward = None
* x_build_rank_breakdown__mutmut_48 -- key "rewardPed" -> "XXrewardPedXX"
* x_build_rank_breakdown__mutmut_49 -- key "rewardPed" -> "rewardped"
* x_build_rank_breakdown__mutmut_50 -- key "rewardPed" -> "REWARDPED"
"""

from __future__ import annotations

from backend.data.codex_categories import (
    build_rank_breakdown,
    get_reward_ped,
)


def test_get_reward_ped_rounds_to_exactly_four_decimals() -> None:
    """Kill mutmut_12: round(cost/divisor, 4) -> round(..., 5).

    rank 1 -> CODEX_MULTIPLIERS[0] == 1, cat1 -> divisor 200.
    cost = 1 * 0.123 = 0.123; 0.123 / 200 = 0.000615.
    round(0.000615, 4) == 0.0006   (correct, 4 places)
    round(0.000615, 5) == 0.00061  (the mutant, 5 places)
    The raw value MUST be quantised to 4 decimal places.
    """
    value = get_reward_ped(1, 0.123, "cat1")
    assert value == 0.0006

    # Be explicit that it is NOT the 5-decimal value the mutant produces.
    assert value != round(0.123 / 200, 5)
    assert value != 0.00061


def test_get_reward_ped_four_decimals_second_witness() -> None:
    """A second independent witness for the 4-vs-5 decimal boundary.

    rank 24 -> CODEX_MULTIPLIERS[23] == 90, cat2 -> divisor 320.
    cost = 90 * 3.33 = 299.7; 299.7 / 320 = 0.93656250
    round(..., 4) == 0.9366 vs round(..., 5) == 0.93656.
    """
    value = get_reward_ped(24, 3.33, "cat2")
    assert value == 0.9366
    assert value != round(90 * 3.33 / 320, 5)


def test_build_rank_breakdown_reward_ped_is_the_computed_value() -> None:
    """Kill mutmut_15: reward = get_reward_ped(...) -> reward = None.

    Every row's rewardPed must equal the formula output and never be None.
    """
    base_cost = 0.123
    breakdown = build_rank_breakdown(base_cost, "MobLooter")
    assert len(breakdown) == 25

    for row in breakdown:
        expected = get_reward_ped(row["rank"], base_cost, row["category"])
        assert row["rewardPed"] is not None
        assert row["rewardPed"] == expected

    # Concrete witness: rank 1 reward is a known non-None, non-zero value.
    assert breakdown[0]["rank"] == 1
    assert breakdown[0]["rewardPed"] == 0.0006


def test_build_rank_breakdown_uses_exact_rewardPed_key() -> None:
    """Kill mutmut_48/49/50: key "rewardPed" renamed to a variant.

    The output dict must expose the reward under the exact camelCase key
    "rewardPed" (the API/frontend contract), and must NOT expose any of the
    mutant key spellings.
    """
    breakdown = build_rank_breakdown(0.123, "MobLooter")

    for row in breakdown:
        # Exact key present (kills the all-three renames at once).
        assert "rewardPed" in row
        # None of the mutant spellings leak into the contract.
        assert "XXrewardPedXX" not in row  # mutmut_48
        assert "rewardped" not in row  # mutmut_49 (lowercased)
        assert "REWARDPED" not in row  # mutmut_50 (uppercased)
