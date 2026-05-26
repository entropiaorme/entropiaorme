"""Property-based tests for codex category data and rank derivations.

Covers ``backend.data.codex_categories``: skill-to-category mapping, rank
cycling, rank cost / reward formulas, and the 25-row breakdown builder.
"""

import pytest
from hypothesis import assume, given
from hypothesis import strategies as st

from backend.data.codex_categories import (
    CODEX_MULTIPLIERS,
    CODEX_SKILL_CATEGORIES,
    REWARD_DIVISORS,
    build_rank_breakdown,
    get_category_for_rank,
    get_codex_category,
    get_rank_cost,
    get_reward_ped,
    is_cat4_rank,
)

_ALL_SKILLS = sorted({s for skills in CODEX_SKILL_CATEGORIES.values() for s in skills})
_BASE_COST = st.floats(
    min_value=0.0, max_value=1e6, allow_nan=False, allow_infinity=False
)


# --- category membership ---


def test_categories_are_disjoint():
    seen: set[str] = set()
    for skills in CODEX_SKILL_CATEGORIES.values():
        for skill in skills:
            assert skill not in seen
            seen.add(skill)


@given(st.sampled_from(_ALL_SKILLS))
def test_catalogued_skill_maps_back_to_its_category(skill):
    cat = get_codex_category(skill)
    assert cat is not None
    assert skill in CODEX_SKILL_CATEGORIES[cat]


@given(st.text())
def test_unknown_skill_has_no_category(name):
    assume(name not in _ALL_SKILLS)
    assert get_codex_category(name) is None


# --- rank cycling ---


@given(st.integers(min_value=1, max_value=200))
def test_category_for_rank_is_mod5_periodic(rank):
    mod = rank % 5
    expected = "cat1" if mod in (1, 2) else "cat2" if mod in (3, 4) else "cat3"
    assert get_category_for_rank(rank) == expected
    assert get_category_for_rank(rank) == get_category_for_rank(rank + 5)


@given(
    st.integers(min_value=1, max_value=25),
    st.sampled_from([None, "MobLooter", "Crafter"]),
)
def test_is_cat4_rank_only_on_mob_looter_5_15_25(rank, codex_type):
    result = is_cat4_rank(rank, codex_type)
    assert result == (codex_type == "MobLooter" and rank % 10 == 5)
    if result:
        assert rank in (5, 15, 25)


# --- rank cost / reward ---


@given(st.integers(min_value=1, max_value=25), _BASE_COST)
def test_rank_cost_is_linear_and_rank1_equals_base(rank, base):
    assert get_rank_cost(1, base) == pytest.approx(base)
    assert get_rank_cost(rank, base) == pytest.approx(
        CODEX_MULTIPLIERS[rank - 1] * base
    )


@given(st.integers(min_value=1, max_value=24), _BASE_COST)
def test_rank_cost_is_non_decreasing_in_rank(rank, base):
    assert get_rank_cost(rank + 1, base) + 1e-9 >= get_rank_cost(rank, base)


@given(st.integers(min_value=1, max_value=25), _BASE_COST)
def test_reward_is_non_increasing_across_categories(rank, base):
    r1 = get_reward_ped(rank, base, "cat1")
    r2 = get_reward_ped(rank, base, "cat2")
    r3 = get_reward_ped(rank, base, "cat3")
    r4 = get_reward_ped(rank, base, "cat4")
    # Larger divisor (cat1 < cat2 < cat3 < cat4) yields a smaller reward.
    assert r1 + 1e-9 >= r2 >= r3 - 1e-9
    assert r3 + 1e-9 >= r4


@given(
    st.integers(min_value=1, max_value=25),
    st.sampled_from(sorted(REWARD_DIVISORS)),
    _BASE_COST,
)
def test_reward_matches_cost_over_divisor(rank, category, base):
    expected = get_rank_cost(rank, base) / REWARD_DIVISORS[category]
    assert get_reward_ped(rank, base, category) == pytest.approx(expected, abs=1e-4)


# --- breakdown builder ---


@given(
    st.floats(min_value=0.01, max_value=1000.0, allow_nan=False, allow_infinity=False),
    st.sampled_from([None, "MobLooter"]),
)
def test_build_rank_breakdown_is_25_consistent_rows(base, codex_type):
    rows = build_rank_breakdown(base, codex_type)
    assert len(rows) == 25
    for expected_rank, row in enumerate(rows, start=1):
        assert row["rank"] == expected_rank
        assert row["category"] == get_category_for_rank(expected_rank)
        assert row["cost"] == pytest.approx(
            round(get_rank_cost(expected_rank, base), 2)
        )
        assert row["cat4Bonus"] == is_cat4_rank(expected_rank, codex_type)
        # The skills list is a fresh copy, never an alias of the module data.
        assert row["skills"] == CODEX_SKILL_CATEGORIES[row["category"]]
        assert row["skills"] is not CODEX_SKILL_CATEGORIES[row["category"]]
        if row["cat4Bonus"]:
            assert row["cat4RewardPed"] is not None
            assert row["cat4Skills"] == list(CODEX_SKILL_CATEGORIES["cat4"])
        else:
            assert row["cat4RewardPed"] is None
            assert row["cat4Skills"] == []
