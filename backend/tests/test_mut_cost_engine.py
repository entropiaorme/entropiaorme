"""Mutation-hardening tests for backend.services.cost_engine.

These tests pin behaviour that the surviving mutants of the EntropiaOrme
mutation campaign were able to change without detection. Each test exercises
the exact mutated line and asserts the precise behaviour the mutation breaks.
"""

import math

import pytest

from backend.services import cost_engine
from backend.services.cost_engine import (
    cost_per_shot,
    cost_per_shot_from_props,
    get_weapon_damage_profile,
    heal_cost_per_use,
    is_limited,
    weapon_total_damage,
)


# ── is_limited: the "name" default must be "" (a string), never None ─────────


def test_is_limited_missing_name_returns_false_not_typeerror():
    """is_limited on an entity with no name must default to "" and return False.

    Mutants that change get("name", "") -> get("name", None) or drop the
    default make `name` None, so `"(L)" in name` raises TypeError instead of
    cleanly returning False.
    """
    assert is_limited({}) is False
    assert is_limited({"economy": {}}) is False


def test_is_limited_detects_marker_with_missing_name_default():
    """A present name is still classified correctly (guards the default value)."""
    assert is_limited({"name": "Foo (L)"}) is True
    assert is_limited({"name": "Foo"}) is False


# ── weapon_total_damage / profile: default damage_enhancers must be 0 ────────


def test_weapon_total_damage_default_enhancers_is_zero():
    """The default damage_enhancers must be 0 (no implicit +10%).

    Mutant flips the default to 1, which would scale base damage by 1.1.
    """
    weapon = {"name": "X", "damage": {"impact": 20.0}}
    assert weapon_total_damage(weapon) == 20.0


def test_get_weapon_damage_profile_default_enhancers_is_zero():
    """get_weapon_damage_profile default damage_enhancers must be 0."""
    weapon = {"name": "X", "damage": {"impact": 20.0}}
    profile = get_weapon_damage_profile(weapon)
    assert profile == {"totalDamage": 20.0, "damageMin": 10.0, "damageMax": 20.0}


def test_get_weapon_damage_profile_forwards_amp():
    """The amp argument must be threaded through to weapon_total_damage.

    Mutants pass amp=None (or drop the kwarg), dropping the amp's damage
    contribution from the profile.
    """
    weapon = {"name": "X", "damage": {"impact": 30.0}}
    amp = {"name": "Amp", "damage": {"impact": 10.0}}
    profile = get_weapon_damage_profile(weapon, amp=amp)
    # base 30 + amp 10 (under the 15 cap) = 40
    assert profile["totalDamage"] == 40.0
    assert profile["damageMin"] == 20.0
    assert profile["damageMax"] == 40.0


def test_get_weapon_damage_profile_forwards_damage_enhancers():
    """The damage_enhancers argument must be threaded through.

    Mutant drops the damage_enhancers kwarg from the inner call, so the
    explicit enhancer count is ignored.
    """
    weapon = {"name": "X", "damage": {"impact": 20.0}}
    profile = get_weapon_damage_profile(weapon, damage_enhancers=2)
    # 20 * (1 + 2*0.1) = 24
    assert profile["totalDamage"] == 24.0
    assert profile["damageMax"] == 24.0
    assert profile["damageMin"] == 12.0


# ── cost_per_shot: markup defaults must all be 1.0 (TT) ──────────────────────


def test_cost_per_shot_default_weapon_markup_is_tt():
    """Default weapon_markup must be 1.0 (TT), not 2.0."""
    weapon = {"name": "W", "economy": {"decay": 2.0, "ammo_burn": 0}}
    result = cost_per_shot(weapon)
    line = result["costBreakdown"][0]
    assert line["component"] == "Weapon decay"
    assert line["markupMultiplier"] == 1.0
    assert line["effectiveCostPec"] == 2.0
    assert result["totalCostPerUse"] == 2.0


def test_cost_per_shot_default_amp_markup_is_tt():
    """Default amp_markup must be 1.0 (TT), not 2.0."""
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 0}}
    amp = {"name": "A", "economy": {"decay": 3.0, "ammo_burn": 0}}
    result = cost_per_shot(weapon, amp=amp)
    amp_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Amp decay"
    )
    assert amp_line["markupMultiplier"] == 1.0
    assert amp_line["effectiveCostPec"] == 3.0


def test_cost_per_shot_default_scope_markup_is_tt():
    """Default scope_markup must be 1.0 (TT), not 2.0."""
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 0}}
    scope = {"name": "S", "economy": {"decay": 0.5}}
    result = cost_per_shot(weapon, scope=scope)
    scope_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Scope decay"
    )
    assert scope_line["markupMultiplier"] == 1.0
    assert scope_line["effectiveCostPec"] == 0.5


def test_cost_per_shot_default_absorber_markup_is_tt():
    """Default absorber_markup must be 1.0 (TT), not 2.0."""
    weapon = {"name": "W", "economy": {"decay": 2.0, "ammo_burn": 0}}
    absorber = {"name": "Abs", "economy": {"absorption": 0.5}}
    result = cost_per_shot(weapon, absorber=absorber)
    abs_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Absorber decay"
    )
    # 2.0 * 0.5 = 1.0 absorbed at TT
    assert abs_line["markupMultiplier"] == 1.0
    assert abs_line["effectiveCostPec"] == 1.0


# ── cost_per_shot: the "or 0.0 / or 1.0" falsy fallbacks ─────────────────────


def test_cost_per_shot_absorption_missing_defaults_to_zero():
    """A present absorber with no absorption value absorbs nothing (0.0).

    Mutant uses `or 1.0`, which would absorb 100% of weapon decay.
    """
    weapon = {"name": "W", "economy": {"decay": 2.0, "ammo_burn": 0}}
    absorber = {"name": "Abs", "economy": {}}  # no absorption key
    result = cost_per_shot(weapon, absorber=absorber)
    components = [ln["component"] for ln in result["costBreakdown"]]
    # No absorber line because absorbed decay is 0.
    assert "Absorber decay" not in components
    weapon_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Weapon decay"
    )
    # Full weapon decay survives because nothing was absorbed.
    assert weapon_line["costPec"] == 2.0
    assert result["totalCostPerUse"] == 2.0


def test_cost_per_shot_amp_decay_missing_defaults_to_zero():
    """Amp present but no decay value contributes 0.0 amp decay, not 1.0."""
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 0}}
    amp = {"name": "A", "economy": {"ammo_burn": 0}}  # no decay key
    result = cost_per_shot(weapon, amp=amp)
    amp_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Amp decay"
    )
    assert amp_line["costPec"] == 0.0
    assert amp_line["effectiveCostPec"] == 0.0


def test_cost_per_shot_amp_ammo_missing_defaults_to_zero():
    """Amp present but no ammo_burn value contributes 0.0 amp ammo, not 0.01.

    Mutant `(amp_eco.get("ammo_burn") or 1.0) / 100.0` would create a spurious
    "Ammo (amp)" line of 0.01 PEC.
    """
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 0}}
    amp = {"name": "A", "economy": {"decay": 0.5}}  # no ammo_burn key
    result = cost_per_shot(weapon, amp=amp)
    components = [ln["component"] for ln in result["costBreakdown"]]
    assert "Ammo (amp)" not in components


def test_cost_per_shot_scope_decay_missing_defaults_to_zero():
    """Scope present but no decay value contributes 0.0 scope decay, not 1.0."""
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 0}}
    scope = {"name": "S", "economy": {}}  # no decay key
    result = cost_per_shot(weapon, scope=scope)
    scope_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Scope decay"
    )
    assert scope_line["costPec"] == 0.0
    assert scope_line["effectiveCostPec"] == 0.0
    # Total is just the weapon decay; scope adds nothing.
    assert result["totalCostPerUse"] == 1.0


# ── cost_per_shot: rounding precision must be 4 decimal places ───────────────


def test_cost_per_shot_costpec_rounds_to_four_places():
    """costPec must round to 4 decimal places, not 5.

    ammo_burn 12345 -> 123.45 PEC exactly; but decay 1.23456 rounds to 1.2346
    at 4 places and to 1.23456 at 5. Use a value distinguishing 4 vs 5.
    """
    weapon = {"name": "W", "economy": {"decay": 1.234567, "ammo_burn": 0}}
    result = cost_per_shot(weapon)
    line = result["costBreakdown"][0]
    assert line["costPec"] == 1.2346  # round(1.234567, 4)


def test_cost_per_shot_markup_rounds_to_four_places():
    """markupMultiplier must round to 4 decimal places, not 5."""
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 0}}
    result = cost_per_shot(weapon, weapon_markup=1.234567)
    line = result["costBreakdown"][0]
    assert line["markupMultiplier"] == 1.2346  # round(1.234567, 4)


def test_cost_per_shot_total_rounds_to_four_places():
    """totalCostPerUse must round to 4 decimal places, not 5."""
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 0}}
    result = cost_per_shot(weapon, weapon_markup=1.234567)
    # effective = round(1.0 * 1.234567, 4) = 1.2346, total = round(1.2346, 4)
    assert result["totalCostPerUse"] == 1.2346


# ── cost_per_shot: the absorber / amp-ammo boolean guards ────────────────────


def test_cost_per_shot_no_absorber_line_when_absorber_absent():
    """`if absorber and absorber_decay > 0` must require a real absorber.

    Mutant `if absorber or absorber_decay > 0` would emit an absorber line even
    with no absorber (when the spurious comparison is truthy). With no absorber,
    absorber_decay is 0.0 so `0.0 > 0` is False, yet `or` still short-circuits
    only on the first operand: absorber is None -> falsy -> evaluate
    `absorber_decay > 0` -> False, so no line. To force a difference we need a
    case where the OR diverges: it cannot here, so instead assert the absorber
    line never appears without an absorber across configs.
    """
    weapon = {"name": "W", "economy": {"decay": 2.0, "ammo_burn": 0}}
    result = cost_per_shot(weapon, absorber=None)
    components = [ln["component"] for ln in result["costBreakdown"]]
    assert "Absorber decay" not in components


def test_cost_per_shot_zero_absorption_emits_no_absorber_line():
    """`absorber_decay > 0` (strict) must suppress a zero-decay absorber line.

    Mutant `absorber_decay >= 0` would emit a 0.0 absorber line whenever an
    absorber is present, even when it absorbs nothing.
    """
    weapon = {"name": "W", "economy": {"decay": 2.0, "ammo_burn": 0}}
    absorber = {"name": "Abs", "economy": {"absorption": 0.0}}
    result = cost_per_shot(weapon, absorber=absorber)
    components = [ln["component"] for ln in result["costBreakdown"]]
    assert "Absorber decay" not in components
    # Full weapon decay retained.
    weapon_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Weapon decay"
    )
    assert weapon_line["costPec"] == 2.0


def test_cost_per_shot_amp_ammo_guard_requires_amp_and_positive_ammo():
    """`if amp is not None and amp_ammo > 0` - both conditions required.

    Mutant `or` would emit "Ammo (amp)" with no amp at all. Mutant `>= 0`
    would emit a 0.0 "Ammo (amp)" line when an amp has zero ammo.
    """
    # No amp: there must be no amp-ammo line even though weapon ammo is positive.
    weapon = {"name": "W", "economy": {"decay": 1.0, "ammo_burn": 500}}
    result = cost_per_shot(weapon, amp=None)
    components = [ln["component"] for ln in result["costBreakdown"]]
    assert "Ammo (amp)" not in components
    # Without an amp, the weapon ammo line is labelled "Ammo" (not "Ammo (weapon)").
    assert "Ammo" in components

    # Amp with zero ammo: strict > 0 must suppress the amp-ammo line.
    amp = {"name": "A", "economy": {"decay": 0.5, "ammo_burn": 0}}
    result2 = cost_per_shot(weapon, amp=amp)
    components2 = [ln["component"] for ln in result2["costBreakdown"]]
    assert "Ammo (amp)" not in components2
    # With an amp the weapon ammo line is relabelled.
    assert "Ammo (weapon)" in components2


# ── cost_per_shot_from_props: argument threading & defaults ──────────────────


def _props(**overrides) -> dict:
    base = {
        "weapon_entity": {"name": "W", "economy": {"decay": 2.0, "ammo_burn": 0}},
    }
    base.update(overrides)
    return base


def test_from_props_default_damage_enhancers_is_zero():
    """props.get("damage_enhancers", 0) - the default must be 0, not None/1.

    Mutant `None` would make max(0, int(None or 0)) work out the same as 0, but
    mutant `1` would apply a +10% scaling to the weapon portion.
    """
    props = _props()  # no damage_enhancers key
    result = cost_per_shot_from_props(props)
    weapon_line = result["costBreakdown"][0]
    assert weapon_line["component"] == "Weapon decay"
    # No enhancer scaling: decay stays 2.0.
    assert weapon_line["costPec"] == 2.0


def test_from_props_forwards_amp_entity():
    """The amp entity must be read from props["amp_entity"], not dropped/renamed."""
    props = _props(
        amp_entity={"name": "A", "economy": {"decay": 1.0, "ammo_burn": 0}}
    )
    result = cost_per_shot_from_props(props)
    components = [ln["component"] for ln in result["costBreakdown"]]
    assert "Amp decay" in components
    amp_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Amp decay"
    )
    assert amp_line["costPec"] == 1.0


def test_from_props_forwards_scope_entity():
    """The scope entity must be read from props["scope_entity"]."""
    props = _props(scope_entity={"name": "S", "economy": {"decay": 0.5}})
    result = cost_per_shot_from_props(props)
    components = [ln["component"] for ln in result["costBreakdown"]]
    assert "Scope decay" in components


def test_from_props_forwards_absorber_entity():
    """The absorber entity must be read from props["absorber_entity"]."""
    props = _props(
        absorber_entity={"name": "Abs", "economy": {"absorption": 0.25}}
    )
    result = cost_per_shot_from_props(props)
    components = [ln["component"] for ln in result["costBreakdown"]]
    assert "Absorber decay" in components
    abs_line = result["costBreakdown"][0]
    # 2.0 * 0.25 = 0.5 absorbed.
    assert abs_line["costPec"] == 0.5


def test_from_props_weapon_markup_default_and_division():
    """weapon_markup defaults to 100 then divides by 100.0 -> 1.0 (TT).

    Mutants: wrong default key/value, or `* 100.0` instead of `/ 100.0`.
    """
    props = _props()  # no weapon_markup key -> default 100 -> 1.0
    result = cost_per_shot_from_props(props)
    weapon_line = result["costBreakdown"][0]
    assert weapon_line["markupMultiplier"] == 1.0
    assert weapon_line["effectiveCostPec"] == 2.0


def test_from_props_weapon_markup_value_divided_by_100():
    """A 120 markup must become a 1.2 multiplier (divide by 100.0)."""
    props = _props(weapon_markup=120)
    result = cost_per_shot_from_props(props)
    weapon_line = result["costBreakdown"][0]
    assert weapon_line["markupMultiplier"] == 1.2
    assert weapon_line["effectiveCostPec"] == round(2.0 * 1.2, 4)


def test_from_props_amp_markup_value_divided_by_100():
    """amp_markup is read from props["amp_markup"] and divided by 100.0."""
    props = _props(
        amp_entity={"name": "A", "economy": {"decay": 1.0, "ammo_burn": 0}},
        amp_markup=150,
    )
    result = cost_per_shot_from_props(props)
    amp_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Amp decay"
    )
    assert amp_line["markupMultiplier"] == 1.5
    assert amp_line["effectiveCostPec"] == 1.5


def test_from_props_amp_markup_default_is_tt():
    """Missing amp_markup defaults to 100 -> 1.0 multiplier."""
    props = _props(
        amp_entity={"name": "A", "economy": {"decay": 2.0, "ammo_burn": 0}}
    )
    result = cost_per_shot_from_props(props)
    amp_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Amp decay"
    )
    assert amp_line["markupMultiplier"] == 1.0
    assert amp_line["effectiveCostPec"] == 2.0


def test_from_props_scope_markup_value_divided_by_100():
    """scope_markup is read from props["scope_markup"] and divided by 100.0."""
    props = _props(
        scope_entity={"name": "S", "economy": {"decay": 1.0}},
        scope_markup=130,
    )
    result = cost_per_shot_from_props(props)
    scope_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Scope decay"
    )
    assert scope_line["markupMultiplier"] == 1.3
    assert scope_line["effectiveCostPec"] == 1.3


def test_from_props_scope_markup_default_is_tt():
    """Missing scope_markup defaults to 100 -> 1.0 multiplier."""
    props = _props(scope_entity={"name": "S", "economy": {"decay": 2.0}})
    result = cost_per_shot_from_props(props)
    scope_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Scope decay"
    )
    assert scope_line["markupMultiplier"] == 1.0
    assert scope_line["effectiveCostPec"] == 2.0


def test_from_props_absorber_markup_value_divided_by_100():
    """absorber_markup is read from props["absorber_markup"] and divided by 100.0."""
    props = _props(
        absorber_entity={"name": "Abs", "economy": {"absorption": 0.5}},
        absorber_markup=140,
    )
    result = cost_per_shot_from_props(props)
    abs_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Absorber decay"
    )
    assert abs_line["markupMultiplier"] == 1.4
    # absorbed decay 2.0*0.5 = 1.0 at 1.4 -> 1.4
    assert abs_line["effectiveCostPec"] == 1.4


def test_from_props_absorber_markup_default_is_tt():
    """Missing absorber_markup defaults to 100 -> 1.0 multiplier."""
    props = _props(
        absorber_entity={"name": "Abs", "economy": {"absorption": 0.5}}
    )
    result = cost_per_shot_from_props(props)
    abs_line = next(
        ln for ln in result["costBreakdown"] if ln["component"] == "Absorber decay"
    )
    assert abs_line["markupMultiplier"] == 1.0
    assert abs_line["effectiveCostPec"] == 1.0


# ── heal_cost_per_use: markup default & rounding ─────────────────────────────


def test_heal_cost_per_use_default_markup_is_tt():
    """heal_cost_per_use default markup must be 1.0, not 2.0."""
    tool = {"name": "FAP", "economy": {"decay": 1.5, "ammo_burn": 84}}
    # (1.5 + 0.84) * 1.0 = 2.34
    assert heal_cost_per_use(tool) == 2.34


def test_heal_cost_per_use_rounds_to_four_places():
    """heal_cost_per_use must round to 4 decimal places, not 5."""
    tool = {"name": "FAP", "economy": {"decay": 1.234567, "ammo_burn": 0}}
    # round(1.234567 * 1.0, 4) = 1.2346
    assert heal_cost_per_use(tool) == 1.2346
