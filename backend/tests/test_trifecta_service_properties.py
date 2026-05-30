"""Property-based tests for the trifecta-attribution descriptor.

Covers ``backend.services.trifecta_service.describe_trifecta``: a pure resolver
keyed only on a preset and a DB connection that reads static ``equipment_library``
rows. It never consults session, loot, or event-bus state, so the generated
inputs here are equipment configurations rather than event sequences.

The success-path properties supply two weapons whose damage bands are
constructed to be non-overlapping (small-weapon max strictly below big-weapon
min) plus a heal tool, so the resolver reaches its ``(result, None)`` return.
"""

import json
from types import SimpleNamespace

from hypothesis import given
from hypothesis import strategies as st

from backend.db.app_database import AppDatabase
from backend.services.cost_engine import (
    cost_per_shot_from_props,
    heal_cost_per_use,
)
from backend.services.trifecta_service import describe_trifecta

# Damage on a single per-type channel keeps the base sum equal to the value we
# emit, so a generated total is exactly predictable from the inputs.
_DAMAGE = st.floats(
    min_value=1.0, max_value=200.0, allow_nan=False, allow_infinity=False
)
_ENHANCERS = st.integers(min_value=0, max_value=10)
_MARKUP = st.integers(min_value=100, max_value=300)
_DECAY = st.floats(min_value=0.0, max_value=20.0, allow_nan=False, allow_infinity=False)
_AMMO = st.integers(min_value=0, max_value=500)


def _weapon_props(impact, enhancers, markup, decay, ammo):
    return {
        "weapon_entity": {
            "damage": {"impact": impact},
            "economy": {"decay": decay, "ammo_burn": ammo},
        },
        "weapon_markup": markup,
        "damage_enhancers": enhancers,
    }


def _heal_props(markup, decay, ammo, min_heal, max_heal, uses_per_minute):
    return {
        "tool_entity": {
            "min_heal": min_heal,
            "max_heal": max_heal,
            "uses_per_minute": uses_per_minute,
            "economy": {"decay": decay, "ammo_burn": ammo},
        },
        "markup": markup,
    }


def _seed(conn, item_type, name, props):
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) "
        "VALUES (?, ?, ?, ?)",
        (name, item_type, name.lower(), json.dumps(props)),
    )
    conn.commit()
    return conn.execute(
        "SELECT id FROM equipment_library ORDER BY id DESC LIMIT 1"
    ).fetchone()["id"]


# A loadout whose two weapon bands cannot overlap: the big-weapon base is the
# small-weapon total scaled past the point where its half-damage floor clears
# the small weapon's full-damage ceiling, regardless of the big weapon's own
# enhancers. With no amp, total = base * (1 + 0.1 * enhancers).
_LOADOUT = st.fixed_dictionaries(
    {
        "small_impact": _DAMAGE,
        "small_enhancers": _ENHANCERS,
        "small_markup": _MARKUP,
        "small_decay": _DECAY,
        "small_ammo": _AMMO,
        "separation": st.floats(min_value=3.0, max_value=50.0, allow_nan=False),
        "big_enhancers": _ENHANCERS,
        "big_markup": _MARKUP,
        "big_decay": _DECAY,
        "big_ammo": _AMMO,
        "heal_markup": _MARKUP,
        "heal_decay": _DECAY,
        "heal_ammo": _AMMO,
        "heal_min": st.integers(min_value=1, max_value=200),
        "heal_span": st.integers(min_value=0, max_value=200),
        "uses_per_minute": st.integers(min_value=1, max_value=120),
    }
)


def _resolve_loadout(spec):
    """Seed a non-overlapping small/big/heal loadout and resolve it.

    An in-memory database is built per call so the resolver runs against a real
    ``equipment_library`` schema without leaning on a function-scoped fixture
    (which ``@given`` would not reset between generated inputs).
    """
    db = AppDatabase(":memory:")
    conn = db.conn

    small_total = spec["small_impact"] * (1 + 0.1 * spec["small_enhancers"])
    # Big base chosen so 0.5 * big_total > small_total even after the big
    # weapon's own enhancer multiplier is applied, keeping the bands disjoint.
    big_base = small_total * spec["separation"]

    small_props = _weapon_props(
        spec["small_impact"],
        spec["small_enhancers"],
        spec["small_markup"],
        spec["small_decay"],
        spec["small_ammo"],
    )
    big_props = _weapon_props(
        big_base,
        spec["big_enhancers"],
        spec["big_markup"],
        spec["big_decay"],
        spec["big_ammo"],
    )
    heal_props = _heal_props(
        spec["heal_markup"],
        spec["heal_decay"],
        spec["heal_ammo"],
        spec["heal_min"],
        spec["heal_min"] + spec["heal_span"],
        spec["uses_per_minute"],
    )

    small_id = _seed(conn, "weapon", "SmallW", small_props)
    big_id = _seed(conn, "weapon", "BigW", big_props)
    heal_id = _seed(conn, "healing", "Healer", heal_props)
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )
    result, error = describe_trifecta(conn, preset)
    return result, error, small_props, big_props, heal_props


# --- result_keys_complete_on_success ---


@given(_LOADOUT)
def test_success_result_has_exactly_the_three_roles(spec):
    result, error, *_ = _resolve_loadout(spec)
    # The constructed loadout always resolves; a None result here would mean
    # the disjoint-band construction failed, which is itself a defect signal.
    assert error is None
    assert result is not None
    assert set(result) == {"small_weapon", "big_weapon", "heal_tool"}


# --- damage_band_ordering ---


@given(_LOADOUT)
def test_weapon_bands_are_ordered_low_to_high(spec):
    result, error, *_ = _resolve_loadout(spec)
    assert error is None and result is not None
    for role in ("small_weapon", "big_weapon"):
        band = result[role]
        # Strict ordering holds because an emitted band always has positive
        # base damage (a zero sum suppresses the band entirely).
        assert band["damage_min"] < band["damage_max"]
        assert band["damage_min"] == band["total_damage"] * 0.5
        assert band["damage_max"] == band["total_damage"]


# --- cost_unit_conversion ---


@given(_LOADOUT)
def test_costs_are_pec_totals_divided_by_one_hundred(spec):
    result, error, small_props, big_props, heal_props = _resolve_loadout(spec)
    assert error is None and result is not None

    for role, props in (
        ("small_weapon", small_props),
        ("big_weapon", big_props),
    ):
        expected = cost_per_shot_from_props(props)["totalCostPerUse"] / 100.0
        assert result[role]["cost_per_shot_ped"] == expected

    markup = heal_props.get("markup", 100) / 100.0
    expected_heal = heal_cost_per_use(heal_props["tool_entity"], markup) / 100.0
    assert result["heal_tool"]["cost_per_use_ped"] == expected_heal


# --- damage_enhancers_non_negative ---


@given(_LOADOUT)
def test_enhancers_never_push_a_band_below_its_zero_enhancer_baseline(spec):
    # The configured enhancer count is clamped with ``max(0, ...)``, so the
    # resolved band sits at or above the band the same weapon would expose with
    # zero enhancers (the multiplier 1 + 0.1 * n is monotone non-decreasing).
    result, error, *_ = _resolve_loadout(spec)
    assert error is None and result is not None

    small = result["small_weapon"]
    base_total = spec["small_impact"]  # the zero-enhancer total for one channel
    assert small["damage_max"] >= base_total
    assert small["damage_min"] >= base_total * 0.5

    big = result["big_weapon"]
    big_base_total = (
        spec["small_impact"] * (1 + 0.1 * spec["small_enhancers"]) * spec["separation"]
    )
    assert big["damage_max"] >= big_base_total
    assert big["damage_min"] >= big_base_total * 0.5
