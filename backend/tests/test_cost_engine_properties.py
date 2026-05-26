"""Property-based tests for the cost / damage / heal formula engine.

Covers ``backend.services.cost_engine``: the per-use cost breakdown, weapon
damage roll-up, damage range, heal reload windowing, and heal cost.
"""

import pytest
from hypothesis import given
from hypothesis import strategies as st

from backend.services.cost_engine import (
    cost_per_shot,
    cost_per_shot_from_props,
    damage_range_at_max_skill,
    heal_cost_per_use,
    heal_reload_seconds,
    weapon_total_damage,
)

_NONNEG = st.floats(
    min_value=0.0, max_value=1000.0, allow_nan=False, allow_infinity=False
)
_MARKUP = st.floats(min_value=1.0, max_value=3.0, allow_nan=False, allow_infinity=False)
_ENHANCERS = st.integers(min_value=0, max_value=10)


def _weapon(decay, ammo_burn):
    return {"name": "W", "economy": {"decay": decay, "ammo_burn": ammo_burn}}


# --- cost_per_shot ---


@given(_NONNEG, _NONNEG, _MARKUP, _ENHANCERS)
def test_lines_are_non_negative_and_total_conserves(decay, ammo, markup, enh):
    result = cost_per_shot(
        _weapon(decay, ammo), weapon_markup=markup, damage_enhancers=enh
    )
    lines = result["costBreakdown"]
    for line in lines:
        assert line["costPec"] >= 0.0
        assert line["effectiveCostPec"] >= 0.0
        # effectiveCostPec is costPec * markup, rounded; tolerate the rounding.
        assert line["effectiveCostPec"] == pytest.approx(
            line["costPec"] * line["markupMultiplier"], rel=1e-3, abs=1e-3
        )
    assert result["totalCostPerUse"] >= 0.0
    assert result["totalCostPerUse"] == pytest.approx(
        sum(line["effectiveCostPec"] for line in lines), abs=1e-4
    )


@given(_NONNEG, _NONNEG, _MARKUP)
def test_weapon_decay_line_always_present_and_ammo_is_at_tt(decay, ammo, markup):
    lines = cost_per_shot(_weapon(decay, ammo), weapon_markup=markup)["costBreakdown"]
    components = [line["component"] for line in lines]
    assert "Weapon decay" in components
    for line in lines:
        if line["component"].startswith("Ammo"):
            assert line["markupMultiplier"] == 1.0


@given(_NONNEG, _NONNEG)
def test_amp_lines_present_iff_amp_supplied(decay, amp_decay):
    weapon = _weapon(decay, 0.0)
    without = cost_per_shot(weapon)["costBreakdown"]
    assert not any(line["component"].startswith("Amp") for line in without)
    amp = {"economy": {"decay": amp_decay, "ammo_burn": 0.0}}
    with_amp = cost_per_shot(weapon, amp=amp)["costBreakdown"]
    assert any(line["component"] == "Amp decay" for line in with_amp)


@given(_NONNEG, _MARKUP, _MARKUP)
def test_total_is_monotonic_in_weapon_markup(decay, m1, m2):
    lo, hi = sorted((m1, m2))
    weapon = _weapon(decay, 0.0)
    t_lo = cost_per_shot(weapon, weapon_markup=lo)["totalCostPerUse"]
    t_hi = cost_per_shot(weapon, weapon_markup=hi)["totalCostPerUse"]
    assert t_hi + 1e-9 >= t_lo


@given(_NONNEG, _NONNEG, _ENHANCERS)
def test_total_is_monotonic_in_enhancers(decay, ammo, enh):
    weapon = _weapon(decay, ammo)
    base = cost_per_shot(weapon, damage_enhancers=enh)["totalCostPerUse"]
    more = cost_per_shot(weapon, damage_enhancers=enh + 1)["totalCostPerUse"]
    assert more + 1e-9 >= base


@given(
    _NONNEG,
    st.floats(min_value=0.0, max_value=1.0, allow_nan=False, allow_infinity=False),
    _ENHANCERS,
)
def test_absorber_redistributes_rather_than_creates_decay(decay, absorption, enh):
    weapon = _weapon(decay, 0.0)
    absorber = {"economy": {"absorption": absorption}}
    lines = cost_per_shot(
        weapon,
        absorber=absorber,
        damage_enhancers=enh,
        weapon_markup=1.0,
        absorber_markup=1.0,
    )["costBreakdown"]
    decay_pec = sum(
        line["costPec"]
        for line in lines
        if line["component"] in ("Absorber decay", "Weapon decay")
    )
    assert decay_pec == pytest.approx(decay * (1 + 0.1 * enh), rel=1e-3, abs=1e-3)


@given(_NONNEG, _NONNEG, st.integers(min_value=-5, max_value=10))
def test_from_props_matches_cost_per_shot_and_clamps_enhancers(decay, ammo, enh):
    weapon = _weapon(decay, ammo)
    props = {"weapon_entity": weapon, "weapon_markup": 100, "damage_enhancers": enh}
    via_props = cost_per_shot_from_props(props)
    direct = cost_per_shot(weapon, damage_enhancers=max(0, enh), weapon_markup=1.0)
    assert via_props == direct


# --- weapon_total_damage / damage range ---


def test_total_damage_is_none_without_damage_fields():
    assert weapon_total_damage({"name": "W"}) is None
    assert weapon_total_damage({"name": "W", "damage": {}}) is None


@given(
    st.floats(min_value=0.01, max_value=1000.0, allow_nan=False, allow_infinity=False),
    _ENHANCERS,
)
def test_total_damage_enhancer_formula_without_amp(base, enh):
    weapon = {"damage": {"impact": base}}
    assert weapon_total_damage(weapon, damage_enhancers=enh) == pytest.approx(
        base * (1 + 0.1 * enh)
    )


@given(
    st.floats(min_value=0.01, max_value=1000.0, allow_nan=False, allow_infinity=False),
    st.floats(min_value=0.0, max_value=1000.0, allow_nan=False, allow_infinity=False),
    _ENHANCERS,
)
def test_amp_damage_is_capped_at_half_base(base, amp_dmg, enh):
    weapon = {"damage": {"impact": base}}
    amp = {"damage": {"impact": amp_dmg}}
    total = weapon_total_damage(weapon, amp=amp, damage_enhancers=enh)
    amp_contribution = min(base / 2.0, amp_dmg) if amp_dmg > 0 else 0.0
    assert total == pytest.approx(base * (1 + 0.1 * enh) + amp_contribution)


@given(
    st.floats(min_value=0.0, max_value=1000.0, allow_nan=False, allow_infinity=False)
)
def test_damage_range_is_half_to_full(total):
    interval = damage_range_at_max_skill(total)
    assert interval["max"] == total
    assert interval["min"] == pytest.approx(0.5 * total)
    assert interval["min"] <= interval["max"] + 1e-12


# --- heal reload / cost ---


def test_heal_reload_defaults_to_2_5_with_neither_field():
    assert heal_reload_seconds({}) == pytest.approx(2.5)


@given(
    st.floats(min_value=0.01, max_value=120.0, allow_nan=False, allow_infinity=False),
    st.floats(min_value=0.01, max_value=600.0, allow_nan=False, allow_infinity=False),
)
def test_heal_reload_cooldown_takes_precedence(cooldown, uses_per_minute):
    tool = {"mindforce": {"cooldown": cooldown}, "uses_per_minute": uses_per_minute}
    assert heal_reload_seconds(tool) == pytest.approx(float(cooldown))


@given(
    st.floats(min_value=0.01, max_value=600.0, allow_nan=False, allow_infinity=False)
)
def test_heal_reload_uses_per_minute_branch(uses_per_minute):
    result = heal_reload_seconds({"uses_per_minute": uses_per_minute})
    assert result > 0.0
    assert result == pytest.approx(60.0 / uses_per_minute)
    assert result * uses_per_minute == pytest.approx(60.0)


@given(_NONNEG, _NONNEG, _MARKUP)
def test_heal_cost_is_non_negative_and_linear_in_markup(decay, ammo, markup):
    tool = {"economy": {"decay": decay, "ammo_burn": ammo}}
    cost = heal_cost_per_use(tool, markup)
    assert cost >= 0.0
    assert cost == pytest.approx((decay + ammo / 100.0) * markup, abs=1e-4)
