"""Tests for the cost formula engine."""

from backend.services.cost_engine import (
    cost_per_shot,
    damage_range_at_max_skill,
    get_weapon_damage_profile,
    heal_cost_per_use,
    heal_range_at_max_skill,
    heal_reload_seconds,
    is_limited,
    weapon_total_damage,
)


def _weapon(decay: float, ammo_burn: int, name: str = "Test Weapon") -> dict:
    """Build a minimal weapon entity in the catalogue payload shape."""
    return {
        "name": name,
        "economy": {"decay": decay, "ammo_burn": ammo_burn},
    }


def _amp(decay: float, ammo_burn: int, name: str = "Test Amp") -> dict:
    return {
        "name": name,
        "economy": {"decay": decay, "ammo_burn": ammo_burn},
    }


def _scope(decay: float) -> dict:
    return {"name": "Test Scope", "economy": {"decay": decay}}


def _absorber(absorption: float) -> dict:
    """absorption is a fraction (0.12 = 12%)."""
    return {"name": "Test Absorber", "economy": {"absorption": absorption}}


# ── Basic cases ──────────────────────────────────────────────────────────────


def test_weapon_only_bp30():
    """BP-30: Decay 1.116, AmmoBurn 1072 → 1.116 + 10.72 = 11.836 PEC/shot."""
    weapon = _weapon(1.116, 1072, "Scott & Barlow BP-30 (L)")
    result = cost_per_shot(weapon)

    assert result["totalCostPerUse"] == 11.836
    assert len(result["costBreakdown"]) == 2

    decay_line = result["costBreakdown"][0]
    assert decay_line["component"] == "Weapon decay"
    assert decay_line["costPec"] == 1.116
    assert decay_line["markupMultiplier"] == 1.0
    assert decay_line["effectiveCostPec"] == 1.116

    ammo_line = result["costBreakdown"][1]
    assert ammo_line["component"] == "Ammo"
    assert ammo_line["costPec"] == 10.72
    assert ammo_line["effectiveCostPec"] == 10.72


def test_weapon_with_amp():
    """Weapon + amp: sum both decays and ammo burns."""
    weapon = _weapon(2.0, 200)  # 2.0 decay + 2.0 ammo PEC
    amp = _amp(1.0, 100)  # 1.0 decay + 1.0 ammo PEC

    result = cost_per_shot(weapon, amp=amp)

    assert result["totalCostPerUse"] == 6.0  # 2.0 + 2.0 + 1.0 + 1.0
    components = [line["component"] for line in result["costBreakdown"]]
    assert components == ["Weapon decay", "Amp decay", "Ammo (weapon)", "Ammo (amp)"]


def test_damage_enhancers_only_scale_weapon_portion():
    weapon = _weapon(2.0, 200)  # 2.0 decay + 2.0 ammo PEC
    amp = _amp(1.0, 100)  # 1.0 decay + 1.0 ammo PEC

    result = cost_per_shot(weapon, amp=amp, damage_enhancers=2)

    # Weapon portion x1.2, amp untouched
    assert result["totalCostPerUse"] == 6.8
    weapon_decay = next(
        line for line in result["costBreakdown"] if line["component"] == "Weapon decay"
    )
    weapon_ammo = next(
        line for line in result["costBreakdown"] if line["component"] == "Ammo (weapon)"
    )
    amp_decay = next(
        line for line in result["costBreakdown"] if line["component"] == "Amp decay"
    )

    assert weapon_decay["costPec"] == 2.4
    assert weapon_ammo["costPec"] == 2.4
    assert amp_decay["costPec"] == 1.0


def test_weapon_with_absorber():
    """Absorber splits weapon decay: absorbed portion becomes separate cost line.

    - Absorber absorbs 12% of weapon decay → 2.0 * 0.12 = 0.24 PEC absorber decay
    - Remaining weapon decay = 2.0 * 0.88 = 1.76 PEC
    - Total cost = 0.24 + 1.76 = 2.0 PEC (same total, but correctly attributed)
    """
    weapon = _weapon(2.0, 0)  # 2.0 decay, no ammo

    result = cost_per_shot(weapon, absorber=_absorber(0.12))

    components = [line["component"] for line in result["costBreakdown"]]
    assert components == ["Absorber decay", "Weapon decay"]

    absorber_line = result["costBreakdown"][0]
    assert abs(absorber_line["costPec"] - 0.24) < 0.001

    weapon_line = result["costBreakdown"][1]
    assert abs(weapon_line["costPec"] - 1.76) < 0.001

    # Total is same as without absorber at TT markup
    assert abs(result["totalCostPerUse"] - 2.0) < 0.001


def test_weapon_with_absorber_markup():
    """Absorber with markup: absorbed portion costs more than TT."""
    weapon = _weapon(2.0, 0)

    # 12% absorption, absorber at 150% markup
    result = cost_per_shot(weapon, absorber=_absorber(0.12), absorber_markup=1.5)

    absorber_line = result["costBreakdown"][0]
    assert abs(absorber_line["costPec"] - 0.24) < 0.001
    assert absorber_line["markupMultiplier"] == 1.5
    assert abs(absorber_line["effectiveCostPec"] - 0.36) < 0.001

    weapon_line = result["costBreakdown"][1]
    assert abs(weapon_line["costPec"] - 1.76) < 0.001

    # Total = 0.36 (absorber at 150%) + 1.76 (weapon at TT) = 2.12
    assert abs(result["totalCostPerUse"] - 2.12) < 0.001


def test_weapon_with_scope():
    """Scope adds its own decay cost."""
    weapon = _weapon(2.0, 0)
    scope = _scope(0.5)

    result = cost_per_shot(weapon, scope=scope)

    components = [line["component"] for line in result["costBreakdown"]]
    assert "Scope decay" in components
    assert result["totalCostPerUse"] == 2.5


def test_scope_markup():
    """Scope with markup: scope decay costs more than TT."""
    weapon = _weapon(2.0, 0)
    scope = _scope(0.5)

    result = cost_per_shot(weapon, scope=scope, scope_markup=1.3)

    scope_line = next(
        line for line in result["costBreakdown"] if line["component"] == "Scope decay"
    )
    assert scope_line["costPec"] == 0.5
    assert scope_line["markupMultiplier"] == 1.3
    assert scope_line["effectiveCostPec"] == 0.65

    # Total = 2.0 (weapon) + 0.65 (scope at 130%) = 2.65
    assert result["totalCostPerUse"] == 2.65


def test_limited_weapon_markup():
    """Limited weapon with 120% markup: effective cost = decay * 1.2."""
    weapon = _weapon(2.0, 100, "SomeLimitedGun (L)")

    result = cost_per_shot(weapon, weapon_markup=1.2)

    decay_line = result["costBreakdown"][0]
    assert decay_line["markupMultiplier"] == 1.2
    assert decay_line["effectiveCostPec"] == round(2.0 * 1.2, 4)


def test_ammo_always_at_tt():
    """Ammo is always at TT: crafted ammo margin tracked via ledger."""
    weapon = _weapon(1.0, 100)  # 1.0 ammo PEC
    amp = _amp(0.5, 50)  # 0.5 ammo PEC

    result = cost_per_shot(weapon, amp=amp)

    ammo_weapon = next(
        line for line in result["costBreakdown"] if line["component"] == "Ammo (weapon)"
    )
    ammo_amp = next(
        line for line in result["costBreakdown"] if line["component"] == "Ammo (amp)"
    )

    assert ammo_weapon["markupMultiplier"] == 1.0
    assert ammo_amp["markupMultiplier"] == 1.0


def test_full_setup_all_markups():
    """Full weapon setup with all components and markups."""
    weapon = _weapon(2.0, 200)  # 2.0 decay, 2.0 ammo PEC
    amp = _amp(1.0, 100)  # 1.0 decay, 1.0 ammo PEC
    scope = _scope(0.3)  # 0.3 decay
    absorber = _absorber(0.10)  # 10% absorption

    result = cost_per_shot(
        weapon,
        amp=amp,
        scope=scope,
        absorber=absorber,
        weapon_markup=1.2,
        amp_markup=1.1,
        scope_markup=1.3,
        absorber_markup=1.5,
    )

    components = [line["component"] for line in result["costBreakdown"]]
    assert components == [
        "Absorber decay",
        "Weapon decay",
        "Amp decay",
        "Scope decay",
        "Ammo (weapon)",
        "Ammo (amp)",
    ]

    # Absorber: 2.0 * 0.10 = 0.2 PEC at 150% = 0.3
    absorber_line = result["costBreakdown"][0]
    assert abs(absorber_line["costPec"] - 0.2) < 0.001
    assert abs(absorber_line["effectiveCostPec"] - 0.3) < 0.001

    # Weapon: 2.0 * 0.90 = 1.8 PEC at 120% = 2.16
    weapon_line = result["costBreakdown"][1]
    assert abs(weapon_line["costPec"] - 1.8) < 0.001
    assert abs(weapon_line["effectiveCostPec"] - 2.16) < 0.001

    # Ammo always at TT (1.0 multiplier)
    ammo_weapon = next(
        line for line in result["costBreakdown"] if line["component"] == "Ammo (weapon)"
    )
    assert ammo_weapon["markupMultiplier"] == 1.0


# ── Limited detection ────────────────────────────────────────────────────────


def test_is_limited_true():
    assert is_limited({"name": "ArMatrix LR-35 (L)"}) is True


def test_is_limited_false():
    assert is_limited({"name": "Karma Killer Mk. 3a"}) is False


# ── Healing tool ─────────────────────────────────────────────────────────────


def test_heal_cost_per_use():
    """Medical tool: decay + ammo_burn / 100."""
    tool = {"name": "Vivo T20 (L)", "economy": {"decay": 1.5, "ammo_burn": 84}}
    # 1.5 + 0.84 = 2.34
    assert heal_cost_per_use(tool) == 2.34


def test_heal_range_returns_published_min_max():
    """At maxed skill the heal range is just the tool's published values."""
    tool = {
        "name": "Test FAP",
        "min_heal": 21.4,
        "max_heal": 28.6,
    }

    interval = heal_range_at_max_skill(tool)

    assert interval == {"min": 21.4, "max": 28.6}


def test_heal_range_returns_none_when_min_or_max_missing():
    """Tools that don't publish a heal range can't produce an interval."""
    assert heal_range_at_max_skill({"max_heal": 10.0}) is None
    assert heal_range_at_max_skill({"min_heal": 1.0}) is None
    assert heal_range_at_max_skill({}) is None


# ── Damage range (max-skill simplified) ──────────────────────────────────────


def test_damage_range_collapses_to_half_to_full():
    """At maxed skill the damage range is always [0.5 × total, total]."""
    assert damage_range_at_max_skill(40.0) == {"min": 20.0, "max": 40.0}
    assert damage_range_at_max_skill(0.0) == {"min": 0.0, "max": 0.0}


# ── Total damage ─────────────────────────────────────────────────────────────


def test_weapon_total_damage_sums_damage_types():
    weapon = {
        "name": "Mixed Weapon",
        "damage": {"impact": 10.0, "burn": 5.0, "acid": 5.0},
    }
    assert weapon_total_damage(weapon) == 20.0


def test_weapon_total_damage_returns_none_when_no_damage():
    weapon = {"name": "No Damage"}
    assert weapon_total_damage(weapon) is None


def test_weapon_total_damage_applies_enhancers():
    weapon = {"name": "X", "damage": {"impact": 20.0}}
    # 2 enhancers → +20%
    assert weapon_total_damage(weapon, damage_enhancers=2) == 24.0


def test_weapon_total_damage_amp_capped_at_half_base():
    weapon = {"name": "X", "damage": {"impact": 30.0}}
    big_amp = {"name": "Big Amp", "damage": {"impact": 100.0}}
    # base=30, amp capped at 15 → total 45
    assert weapon_total_damage(weapon, amp=big_amp) == 45.0


def test_weapon_total_damage_amp_under_cap_used_as_is():
    weapon = {"name": "X", "damage": {"impact": 30.0}}
    small_amp = {"name": "Small Amp", "damage": {"impact": 5.0}}
    # base=30, amp=5 < 15 cap → total 35
    assert weapon_total_damage(weapon, amp=small_amp) == 35.0


# ── Damage profile end-to-end ────────────────────────────────────────────────


def test_get_weapon_damage_profile_composes_total_and_range():
    weapon = {"name": "X", "damage": {"impact": 30.0}}
    profile = get_weapon_damage_profile(weapon)
    assert profile == {
        "totalDamage": 30.0,
        "damageMin": 15.0,
        "damageMax": 30.0,
    }


def test_get_weapon_damage_profile_returns_none_when_no_damage():
    assert get_weapon_damage_profile({}) is None


# ── Heal reload (max-skill simplified) ───────────────────────────────────────


def test_heal_reload_seconds_from_uses_per_minute():
    tool = {"uses_per_minute": 24}
    assert heal_reload_seconds(tool) == 2.5


def test_heal_reload_seconds_mindforce_cooldown_takes_precedence():
    """Mindforce chips use the explicit cooldown rather than uses_per_minute."""
    tool = {"mindforce": {"cooldown": 1.8}, "uses_per_minute": 60}
    assert heal_reload_seconds(tool) == 1.8


def test_heal_reload_seconds_fallback_when_uses_per_minute_missing():
    """Tools without uses_per_minute fall back to a 24/min default."""
    assert heal_reload_seconds({}) == 60.0 / 24.0
