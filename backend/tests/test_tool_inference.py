"""Unit and property tests for the damage-based weapon attributor.

``DamageAttributor`` maps an observed damage amount back to one of the
configured trifecta weapons by checking which weapons' damage bands contain the
amount, then picking the narrowest band (ties broken by name). Critical hits
widen each band by the [2.0, 3.0] multiplier, with a special preference for a
known big-weapon regular hit when a small weapon could also have crit.
"""

from __future__ import annotations

from hypothesis import given
from hypothesis import strategies as st

from backend.tracking.tool_inference import (
    CRITICAL_DAMAGE_MAX,
    CRITICAL_DAMAGE_MIN,
    DamageAttribution,
    DamageAttributor,
)


def _attributor(*profiles: dict) -> DamageAttributor:
    attributor = DamageAttributor()
    for profile in profiles:
        attributor.add_weapon_profile(**profile)
    return attributor


# --- Empty / degenerate inputs ------------------------------------------------


def test_no_profiles_returns_none() -> None:
    assert DamageAttributor().match_damage(50.0) is None


def test_non_positive_amount_returns_none() -> None:
    attributor = _attributor({"name": "Rifle", "min_damage": 10.0, "max_damage": 20.0})
    assert attributor.match_damage(0.0) is None
    assert attributor.match_damage(-5.0) is None


def test_amount_outside_every_band_returns_none() -> None:
    attributor = _attributor({"name": "Rifle", "min_damage": 10.0, "max_damage": 20.0})
    assert attributor.match_damage(9.999) is None
    assert attributor.match_damage(20.001) is None


# --- Single-weapon attribution ------------------------------------------------


def test_amount_within_band_attributes_that_weapon() -> None:
    attributor = _attributor(
        {
            "name": "Rifle",
            "min_damage": 10.0,
            "max_damage": 20.0,
            "cost_per_shot": 1.25,
        }
    )
    result = attributor.match_damage(15.0)
    assert result == DamageAttribution(tool_name="Rifle", cost_per_shot=1.25)


def test_band_endpoints_are_inclusive() -> None:
    attributor = _attributor({"name": "Rifle", "min_damage": 10.0, "max_damage": 20.0})
    assert attributor.match_damage(10.0) is not None
    assert attributor.match_damage(20.0) is not None


# --- Narrowest-band selection -------------------------------------------------


def test_narrowest_band_wins() -> None:
    attributor = _attributor(
        {"name": "Wide", "min_damage": 0.0, "max_damage": 100.0, "cost_per_shot": 9.0},
        {"name": "Tight", "min_damage": 40.0, "max_damage": 60.0, "cost_per_shot": 2.0},
    )
    result = attributor.match_damage(50.0)
    assert result is not None
    assert result.tool_name == "Tight"
    assert result.cost_per_shot == 2.0


def test_equal_width_bands_break_ties_by_name() -> None:
    attributor = _attributor(
        {"name": "Bravo", "min_damage": 40.0, "max_damage": 60.0},
        {"name": "Alpha", "min_damage": 40.0, "max_damage": 60.0},
    )
    result = attributor.match_damage(50.0)
    assert result is not None
    # min() over (width, name) picks the lexicographically smaller name.
    assert result.tool_name == "Alpha"


# --- Critical hits ------------------------------------------------------------


def test_critical_widens_the_band_by_the_multipliers() -> None:
    # Regular band [10, 20] does not contain 50; the critical band scales to
    # [10*2.0, 20*3.0] = [20, 60], which does.
    attributor = _attributor(
        {"name": "Rifle", "min_damage": 10.0, "max_damage": 20.0, "cost_per_shot": 1.0}
    )
    assert attributor.match_damage(50.0, critical=False) is None
    crit = attributor.match_damage(50.0, critical=True)
    assert crit is not None
    assert crit.tool_name == "Rifle"


def test_critical_multiplier_constants() -> None:
    assert CRITICAL_DAMAGE_MIN == 2.0
    assert CRITICAL_DAMAGE_MAX == 3.0


def test_critical_prefers_known_big_weapon_regular_over_small_weapon_crit() -> None:
    # The big weapon's regular band contains the amount; the small weapon only
    # reaches it on a crit. When a small weapon *could* crit, a value sitting in
    # the big weapon's ordinary band is attributed to the big weapon.
    attributor = _attributor(
        {
            "name": "BigGun",
            "min_damage": 40.0,
            "max_damage": 60.0,
            "cost_per_shot": 5.0,
            "role": "big_weapon",
        },
        {
            "name": "SmallGun",
            "min_damage": 20.0,
            "max_damage": 30.0,
            "cost_per_shot": 1.0,
            "role": "small_weapon",
        },
    )
    # 50 is in BigGun's regular band [40, 60] and in SmallGun's crit band
    # [40, 90]. The preference rule routes it to the big weapon.
    result = attributor.match_damage(50.0, critical=True)
    assert result is not None
    assert result.tool_name == "BigGun"


def test_critical_without_small_weapon_pattern_uses_narrowest_crit_band() -> None:
    # No small_weapon role present, so the preference rule does not fire and the
    # plain narrowest-crit-band selection applies.
    attributor = _attributor(
        {"name": "Rifle", "min_damage": 10.0, "max_damage": 20.0, "cost_per_shot": 1.0}
    )
    result = attributor.match_damage(45.0, critical=True)
    assert result is not None
    assert result.tool_name == "Rifle"


# --- Profile bookkeeping ------------------------------------------------------


def test_base_damage_defaults_to_max_damage_when_zero() -> None:
    attributor = DamageAttributor()
    attributor.add_weapon_profile(name="Rifle", min_damage=10.0, max_damage=20.0)
    # base_damage is not surfaced through match_damage, so assert via the stored
    # profile: the fallback keeps base_damage from being a misleading 0.0.
    profile = attributor._profiles["Rifle"]
    assert profile.base_damage == 20.0


def test_explicit_base_damage_is_kept() -> None:
    attributor = DamageAttributor()
    attributor.add_weapon_profile(
        name="Rifle", min_damage=10.0, max_damage=20.0, base_damage=12.0
    )
    assert attributor._profiles["Rifle"].base_damage == 12.0


def test_re_adding_a_name_replaces_the_profile() -> None:
    attributor = DamageAttributor()
    attributor.add_weapon_profile(name="Rifle", min_damage=10.0, max_damage=20.0)
    attributor.add_weapon_profile(name="Rifle", min_damage=30.0, max_damage=40.0)
    assert len(attributor._profiles) == 1
    assert attributor.match_damage(15.0) is None
    assert attributor.match_damage(35.0) is not None


def test_clear_drops_all_profiles() -> None:
    attributor = _attributor({"name": "Rifle", "min_damage": 10.0, "max_damage": 20.0})
    assert attributor.match_damage(15.0) is not None
    attributor.clear()
    assert attributor.match_damage(15.0) is None


# --- Properties ---------------------------------------------------------------

_damage = st.floats(min_value=0.1, max_value=1e4, allow_nan=False, allow_infinity=False)


@given(low=_damage, span=st.floats(min_value=0.0, max_value=1e4), amount=_damage)
def test_property_match_implies_amount_in_band(
    low: float, span: float, amount: float
) -> None:
    high = low + span
    attributor = _attributor(
        {"name": "W", "min_damage": low, "max_damage": high, "cost_per_shot": 3.0}
    )
    result = attributor.match_damage(amount)
    if result is not None:
        # A non-None attribution means the amount sat inside the regular band.
        assert low <= amount <= high
        assert result.cost_per_shot == 3.0
    else:
        assert not (low <= amount <= high)


@given(amount=st.floats(min_value=40.0, max_value=60.0))
def test_property_narrowest_is_chosen(amount: float) -> None:
    # Two nested bands both contain anything in [40, 60]; the inner (narrower)
    # band must always win regardless of the amount within the overlap.
    attributor = _attributor(
        {"name": "Wide", "min_damage": 0.0, "max_damage": 100.0, "cost_per_shot": 9.0},
        {"name": "Tight", "min_damage": 40.0, "max_damage": 60.0, "cost_per_shot": 2.0},
    )
    result = attributor.match_damage(amount)
    assert result is not None
    assert result.tool_name == "Tight"
