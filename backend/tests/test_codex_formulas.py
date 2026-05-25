"""Tests for codex formula calculations and inverse TT lookup."""

import pytest

from backend.data.codex_categories import (
    CODEX_MULTIPLIERS,
    CODEX_SKILL_CATEGORIES,
    build_rank_breakdown,
    get_category_for_rank,
    get_rank_cost,
    get_reward_ped,
    is_cat4_rank,
)
from backend.data.tt_value_curve import (
    levels_for_tt_value,
    max_tt_curve_level,
    tt_value_of_gain,
)

# ── CODEX_MULTIPLIERS ──────────────────────────────────────────────────────────


def test_multipliers_length():
    assert len(CODEX_MULTIPLIERS) == 25


def test_multipliers_endpoints():
    assert CODEX_MULTIPLIERS[0] == 1
    assert CODEX_MULTIPLIERS[24] == 100


# ── Category cycling ────────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    "rank,expected",
    [
        (1, "cat1"),
        (2, "cat1"),
        (3, "cat2"),
        (4, "cat2"),
        (5, "cat3"),
        (6, "cat1"),
        (7, "cat1"),
        (8, "cat2"),
        (9, "cat2"),
        (10, "cat3"),
        (11, "cat1"),
        (16, "cat1"),
        (13, "cat2"),
        (14, "cat2"),
        (15, "cat3"),
        (20, "cat3"),
        (25, "cat3"),
    ],
)
def test_category_cycling(rank, expected):
    assert get_category_for_rank(rank) == expected


# ── Cat4 bonus ──────────────────────────────────────────────────────────────────


def test_cat4_mob_looter_ranks():
    """Cat4 bonus on ranks 5, 15, 25 for MobLooter."""
    for rank in range(1, 26):
        expected = rank in (5, 15, 25)
        assert is_cat4_rank(rank, "MobLooter") == expected, f"rank {rank}"


def test_cat4_not_mob_type():
    """Regular Mob codex type never gets cat4."""
    for rank in range(1, 26):
        assert is_cat4_rank(rank, "Mob") is False


def test_cat4_none_type():
    assert is_cat4_rank(5, None) is False


# ── Rank cost and reward ────────────────────────────────────────────────────────


def test_rank_cost():
    # Rank 1, base_cost 100 → 1 × 100 = 100
    assert get_rank_cost(1, 100) == 100
    # Rank 5, base_cost 50 → 6 × 50 = 300
    assert get_rank_cost(5, 50) == 300
    # Rank 25, base_cost 10 → 100 × 10 = 1000
    assert get_rank_cost(25, 10) == 1000


def test_reward_ped():
    # Rank 1, base 200, cat1 (divisor 200) → (1×200)/200 = 1.0
    assert get_reward_ped(1, 200, "cat1") == 1.0
    # Rank 5, base 100, cat3 (divisor 640) → (6×100)/640 = 0.9375
    assert get_reward_ped(5, 100, "cat3") == 0.9375
    # Rank 25, base 100, cat4 (divisor 1000) → (100×100)/1000 = 10.0
    assert get_reward_ped(25, 100, "cat4") == 10.0


# ── build_rank_breakdown ────────────────────────────────────────────────────────


def test_build_rank_breakdown_length():
    breakdown = build_rank_breakdown(100, "MobLooter")
    assert len(breakdown) == 25


def test_build_rank_breakdown_cat4_on_5():
    breakdown = build_rank_breakdown(100, "MobLooter")
    rank5 = breakdown[4]  # index 4 = rank 5
    assert rank5["cat4Bonus"] is True
    assert rank5["cat4RewardPed"] is not None
    assert len(rank5["cat4Skills"]) == len(CODEX_SKILL_CATEGORIES["cat4"])


def test_build_rank_breakdown_no_cat4_for_mob():
    breakdown = build_rank_breakdown(100, "Mob")
    for item in breakdown:
        assert item["cat4Bonus"] is False
        assert item["cat4RewardPed"] is None


def test_build_rank_breakdown_skills_match_category():
    breakdown = build_rank_breakdown(50, "Mob")
    for item in breakdown:
        expected_skills = CODEX_SKILL_CATEGORIES[item["category"]]
        assert item["skills"] == expected_skills


# ── Inverse TT lookup ──────────────────────────────────────────────────────────


def test_levels_for_tt_zero_ped():
    assert levels_for_tt_value(100, 0) == 0.0


def test_levels_for_tt_round_trip():
    """Forward then inverse should round-trip."""
    from_level = 500.0
    to_level = 1000.0
    ped = tt_value_of_gain(from_level, to_level)
    levels_gained = levels_for_tt_value(from_level, ped)
    assert abs(levels_gained - 500.0) < 2.0  # within 2 levels (binary search precision)


def test_levels_for_tt_small_gain():
    """Small PED value should give a small level gain."""
    from_level = 200.0
    ped = tt_value_of_gain(200.0, 300.0)
    levels = levels_for_tt_value(from_level, ped)
    assert abs(levels - 100.0) < 5.0


def test_levels_for_tt_beyond_curve():
    """PED exceeding the curve should return levels up to the max."""
    levels = levels_for_tt_value(0, 999999)
    assert levels == pytest.approx(float(max_tt_curve_level()), abs=1e-4)
