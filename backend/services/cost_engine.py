"""Cost formula engine — per-use cost and reference damage / heal ranges.

Calculates per-use cost (decay + ammo + markups) and reference damage / heal
ranges from equipment-catalogue payloads, under the assumption that the
player has fully maxed the relevant skill for the weapon or tool. Sub-skill
progression modelling is intentionally out of scope: the app's mental model
is "use weapons you have the skill for", so reference values at max skill
are what the equipment UI, trifecta band check, and heal-reload windowing
need.
"""


def is_limited(entity: dict) -> bool:
    """True if the entity name contains '(L)', indicating a limited item."""
    name = entity.get("name", "")
    return "(L)" in name


def _economy(entity: dict) -> dict:
    """Pull the economy subdict from an equipment entity, defaulting to empty."""
    return entity.get("economy") or {}


_DAMAGE_TYPES = (
    "impact",
    "cut",
    "stab",
    "penetration",
    "shrapnel",
    "burn",
    "cold",
    "acid",
    "electric",
)


def _sum_damage(entity: dict | None) -> float | None:
    """Sum the per-type damage fields published on the entity."""
    if entity is None:
        return None
    damage = entity.get("damage") or {}
    total = sum((damage.get(t) or 0.0) for t in _DAMAGE_TYPES)
    return total or None


def weapon_total_damage(
    weapon: dict,
    amp: dict | None = None,
    damage_enhancers: int = 0,
) -> float | None:
    """Total weapon damage from base + amp + damage enhancers.

    Each damage enhancer adds 10% to the base damage. An amp can add up to
    half the base weapon damage on top (game-imposed cap).
    """
    base_damage = _sum_damage(weapon)
    if base_damage is None:
        return None

    total_damage = base_damage * (1 + damage_enhancers * 0.1)

    amp_damage = _sum_damage(amp)
    if amp_damage is not None:
        total_damage += min(base_damage / 2.0, amp_damage)

    return total_damage


def damage_range_at_max_skill(total_damage: float) -> dict[str, float]:
    """Damage range at maxed skill: ``[0.5 × total, total]``."""
    return {"min": total_damage * 0.5, "max": total_damage}


def get_weapon_damage_profile(
    weapon: dict,
    amp: dict | None = None,
    damage_enhancers: int = 0,
) -> dict[str, float] | None:
    """Return a derived damage profile suitable for tool inference / display."""
    total_damage = weapon_total_damage(
        weapon,
        amp=amp,
        damage_enhancers=damage_enhancers,
    )
    if total_damage is None:
        return None

    interval = damage_range_at_max_skill(total_damage)
    return {
        "totalDamage": total_damage,
        "damageMin": interval["min"],
        "damageMax": interval["max"],
    }


def heal_range_at_max_skill(tool: dict) -> dict[str, float] | None:
    """Heal range at maxed skill: the tool's published ``min_heal`` / ``max_heal``."""
    max_heal = tool.get("max_heal")
    min_heal = tool.get("min_heal")
    if max_heal is None or min_heal is None:
        return None
    return {"min": min_heal, "max": max_heal}


def heal_reload_seconds(tool: dict) -> float:
    """Reload at maxed skill: mindforce cooldown if present, else ``60 / uses_per_minute``."""
    cooldown = (tool.get("mindforce") or {}).get("cooldown")
    if cooldown:
        return float(cooldown)
    uses_per_minute = tool.get("uses_per_minute")
    if not uses_per_minute:
        return 60.0 / 24.0
    return 60.0 / uses_per_minute


def cost_per_shot(
    weapon: dict,
    amp: dict | None = None,
    scope: dict | None = None,
    absorber: dict | None = None,
    damage_enhancers: int = 0,
    weapon_markup: float = 1.0,
    amp_markup: float = 1.0,
    scope_markup: float = 1.0,
    absorber_markup: float = 1.0,
) -> dict:
    """Calculate cost breakdown for a weapon configuration.

    All inputs are raw equipment-catalogue dicts. Markup params are
    multipliers (1.0 = TT, 1.2 = 120% markup). Ammo is always at TT —
    crafted ammo margin is tracked via ledger instead.

    Absorber mechanics: the absorber absorbs a fraction of weapon decay.
    That absorbed portion becomes a separate cost line at the absorber's
    markup; the remaining weapon decay uses the weapon markup.

    AmmoBurn in the data is in ammo units; divide by 100 to get PEC.

    Returns ``{"costBreakdown": [...], "totalCostPerUse": float}`` matching
    the ``CostBreakdownLine[]`` + ``totalCostPerUse`` shape from frontend types.
    """
    eco = _economy(weapon)
    base_decay = eco.get("decay") or 0.0
    base_ammo_pec = (eco.get("ammo_burn") or 0.0) / 100.0

    enhancer_mult = 1 + damage_enhancers * 0.1
    weapon_decay = base_decay * enhancer_mult
    weapon_ammo = base_ammo_pec * enhancer_mult

    absorber_decay = 0.0
    if absorber:
        absorption = _economy(absorber).get("absorption") or 0.0
        absorber_decay = weapon_decay * absorption
        weapon_decay -= absorber_decay

    amp_decay = 0.0
    amp_ammo = 0.0
    if amp is not None:
        amp_eco = _economy(amp)
        amp_decay = amp_eco.get("decay") or 0.0
        amp_ammo = (amp_eco.get("ammo_burn") or 0.0) / 100.0

    breakdown: list[dict] = []
    total = 0.0

    def add_line(component: str, cost_pec: float, markup: float) -> None:
        nonlocal total
        effective = round(cost_pec * markup, 4)
        breakdown.append(
            {
                "component": component,
                "costPec": round(cost_pec, 4),
                "markupMultiplier": round(markup, 4),
                "effectiveCostPec": effective,
            }
        )
        total += effective

    if absorber and absorber_decay > 0:
        add_line("Absorber decay", absorber_decay, absorber_markup)

    add_line("Weapon decay", weapon_decay, weapon_markup)
    if amp is not None:
        add_line("Amp decay", amp_decay, amp_markup)
    if scope:
        scope_decay = _economy(scope).get("decay") or 0.0
        add_line("Scope decay", scope_decay, scope_markup)

    if weapon_ammo > 0:
        label = "Ammo (weapon)" if amp is not None else "Ammo"
        add_line(label, weapon_ammo, 1.0)
    if amp is not None and amp_ammo > 0:
        add_line("Ammo (amp)", amp_ammo, 1.0)

    return {
        "costBreakdown": breakdown,
        "totalCostPerUse": round(total, 4),
    }


def cost_per_shot_from_props(props: dict, damage_enhancers: int | None = None) -> dict:
    """Calculate weapon cost from an ``equipment_library`` ``properties_json`` payload."""
    configured_damage = (
        props.get("damage_enhancers", 0)
        if damage_enhancers is None
        else damage_enhancers
    )
    return cost_per_shot(
        weapon=props["weapon_entity"],
        amp=props.get("amp_entity"),
        scope=props.get("scope_entity"),
        absorber=props.get("absorber_entity"),
        damage_enhancers=max(0, int(configured_damage or 0)),
        weapon_markup=props.get("weapon_markup", 100) / 100.0,
        amp_markup=props.get("amp_markup", 100) / 100.0,
        scope_markup=props.get("scope_markup", 100) / 100.0,
        absorber_markup=props.get("absorber_markup", 100) / 100.0,
    )


def heal_cost_per_use(tool: dict, markup: float = 1.0) -> float:
    """Cost per use for a medical tool: ``(decay + ammo) × markup`` in PEC."""
    eco = _economy(tool)
    decay = eco.get("decay") or 0.0
    ammo_pec = (eco.get("ammo_burn") or 0.0) / 100.0
    return round((decay + ammo_pec) * markup, 4)
