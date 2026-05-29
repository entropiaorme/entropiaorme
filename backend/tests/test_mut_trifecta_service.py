"""Mutation-hardening tests for backend.services.trifecta_service.

These tests target surviving/no-test mutants in the trifecta-attribution
descriptor and validator. Each test exercises the exact mutated line and
asserts the precise behaviour a mutation would break.
"""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from backend.db.app_database import AppDatabase
from backend.services.trifecta_service import (
    _ranges_overlap,
    describe_trifecta,
    validate_trifecta,
)


@pytest.fixture
def app_db(tmp_path: Path):
    return AppDatabase(tmp_path / "trifecta_mut.db")


def _seed_weapon(app_db, name, props):
    app_db.conn.execute(
        "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) "
        "VALUES (?, 'weapon', ?, ?)",
        (name, name.lower(), json.dumps(props)),
    )
    app_db.conn.commit()
    return app_db.conn.execute(
        "SELECT id FROM equipment_library ORDER BY id DESC LIMIT 1"
    ).fetchone()["id"]


def _seed_heal(app_db, name, props):
    app_db.conn.execute(
        "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) "
        "VALUES (?, 'healing', ?, ?)",
        (name, name.lower(), json.dumps(props)),
    )
    app_db.conn.commit()
    return app_db.conn.execute(
        "SELECT id FROM equipment_library ORDER BY id DESC LIMIT 1"
    ).fetchone()["id"]


def _weapon_props(impact: float, **extra):
    """A minimal weapon entity exposing a usable damage range and cost."""
    props = {
        "weapon_entity": {
            "damage": {"impact": impact},
            "economy": {"decay": 1.0, "ammo_burn": 50},
        },
        "weapon_markup": 100,
        "damage_enhancers": 0,
    }
    props.update(extra)
    return props


def _heal_props(**extra):
    props = {
        "tool_entity": {
            "min_heal": 10,
            "max_heal": 50,
            "uses_per_minute": 30,
            "economy": {"decay": 0.5, "ammo_burn": 20},
        },
        "markup": 100,
    }
    props.update(extra)
    return props


def _full_loadout(app_db, small_impact=10.0, big_impact=40.0, small_extra=None,
                  heal_extra=None):
    """Seed a resolvable trifecta preset; returns (preset, ids)."""
    small_props = _weapon_props(small_impact, **(small_extra or {}))
    small_id = _seed_weapon(app_db, "SmallW", small_props)
    big_id = _seed_weapon(app_db, "BigW", _weapon_props(big_impact))
    heal_id = _seed_heal(app_db, "HealW", _heal_props(**(heal_extra or {})))
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )
    return preset


# ---------------------------------------------------------------------------
# _ranges_overlap boundary  (mutmut_5: <= -> <)
# ---------------------------------------------------------------------------
def test_ranges_overlap_inclusive_boundary():
    """Touching-at-a-point ranges count as overlapping (<=, not <)."""
    # small [5, 10], big [10, 20] meet exactly at 10.
    assert _ranges_overlap(5.0, 10.0, 10.0, 20.0) is True
    # strictly separated stay non-overlapping
    assert _ranges_overlap(5.0, 10.0, 10.01, 20.0) is False


def test_describe_trifecta_touching_ranges_are_overlap(app_db):
    """End-to-end: small_max == big_min must be rejected as overlapping."""
    # small impact 20 -> range [10, 20]; big impact 40 -> range [20, 40].
    # They touch exactly at 20: original treats this as overlap (<=).
    preset = _full_loadout(app_db, small_impact=20.0, big_impact=40.0)
    result, error = describe_trifecta(app_db.conn, preset)
    assert result is None
    assert error is not None and "non-overlapping" in error


# ---------------------------------------------------------------------------
# Exact error-message text (mutmut 2,3,14,15,127,128,161,162)
# ---------------------------------------------------------------------------
def test_no_preset_exact_message():
    result, error = describe_trifecta(None, None)
    assert result is None
    assert error == "Trifecta attribution requires an active preset"


def test_missing_ids_exact_message(app_db):
    preset = SimpleNamespace(small_weapon_id=1, big_weapon_id=None, heal_id=3)
    result, error = describe_trifecta(app_db.conn, preset)
    assert result is None
    assert error == (
        "Trifecta attribution requires a configured small weapon, "
        "big weapon, and healing tool"
    )


def test_overlap_message_exact_prefix(app_db):
    """The overlap error keeps its exact (cased, spaced) prefix."""
    # small impact 10 -> [5, 10]; big impact 12 -> [6, 12]: overlap.
    small_id = _seed_weapon(app_db, "SmO", _weapon_props(10.0))
    big_id = _seed_weapon(app_db, "BiO", _weapon_props(12.0))
    heal_id = _seed_heal(app_db, "HeO", _heal_props())
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert result is None
    assert error is not None
    assert error.startswith(
        "Trifecta attribution requires non-overlapping small/big weapon ranges "
    )


def test_missing_heal_exact_message(app_db):
    small_id = _seed_weapon(app_db, "SmH", _weapon_props(10.0))
    big_id = _seed_weapon(app_db, "BiH", _weapon_props(40.0))
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=999999
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert result is None
    assert error == (
        "Trifecta attribution healing tool is not found in the equipment library"
    )


# ---------------------------------------------------------------------------
# Per-weapon "not found" label text (mutmut 20,21,24,25)
# ---------------------------------------------------------------------------
def test_small_weapon_not_found_label(app_db):
    """Missing small weapon names the role 'small weapon' (lowercase)."""
    preset = SimpleNamespace(
        small_weapon_id=900001, big_weapon_id=900002, heal_id=900003
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert result is None
    assert error == (
        "Trifecta attribution small weapon is not found in the equipment library"
    )


def test_big_weapon_not_found_label(app_db):
    """Present small weapon but missing big weapon names 'big weapon'."""
    small_id = _seed_weapon(app_db, "SmL", _weapon_props(10.0))
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=900002, heal_id=900003
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert result is None
    assert error == (
        "Trifecta attribution big weapon is not found in the equipment library"
    )


# ---------------------------------------------------------------------------
# damage_enhancers resolution (mutmut 46,47,49,51,52,53,61)
# ---------------------------------------------------------------------------
def test_damage_enhancers_applied_to_profile(app_db):
    """5 damage enhancers raise the small weapon's damage by 50%.

    base impact 10 -> total 10*(1+5*0.1)=15 -> damage_max 15.
    Mutants that drop the enhancer count (or, get-key, default) yield 10.
    """
    preset = _full_loadout(
        app_db, small_impact=10.0, big_impact=80.0,
        small_extra={"damage_enhancers": 5},
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    assert result is not None
    assert result["small_weapon"]["damage_max"] == pytest.approx(15.0)
    assert result["small_weapon"]["total_damage"] == pytest.approx(15.0)


def test_missing_damage_enhancers_defaults_to_zero(app_db):
    """No damage_enhancers key -> enhancer count 0 (default 0, not 1).

    base impact 10 with enhancers=0 -> damage_max 10.0 exactly. The
    default-of-1 mutant would yield 11.0.
    """
    small_props = {
        "weapon_entity": {
            "damage": {"impact": 10.0},
            "economy": {"decay": 1.0, "ammo_burn": 50},
        },
        "weapon_markup": 100,
        # no "damage_enhancers" key at all
    }
    small_id = _seed_weapon(app_db, "SmNoEnh", small_props)
    big_id = _seed_weapon(app_db, "BiNoEnh", _weapon_props(40.0))
    heal_id = _seed_heal(app_db, "HeNoEnh", _heal_props())
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    assert result is not None
    assert result["small_weapon"]["damage_max"] == pytest.approx(10.0)


# ---------------------------------------------------------------------------
# amp_entity passed into the damage profile (mutmut 57,60,64,65,66)
# ---------------------------------------------------------------------------
def test_amp_entity_contributes_to_damage(app_db):
    """An amp adds min(base/2, amp_damage) to the small weapon damage.

    base impact 10 (-> base/2 = 5), amp impact 5 -> +5 -> total 15.
    Mutants that drop the amp yield total 10.
    """
    small_props = {
        "weapon_entity": {
            "damage": {"impact": 10.0},
            "economy": {"decay": 1.0, "ammo_burn": 50},
        },
        "amp_entity": {
            "damage": {"impact": 5.0},
            "economy": {"decay": 0.2, "ammo_burn": 10},
        },
        "weapon_markup": 100,
        "amp_markup": 100,
        "damage_enhancers": 0,
    }
    small_id = _seed_weapon(app_db, "SmAmp", small_props)
    big_id = _seed_weapon(app_db, "BiAmp", _weapon_props(80.0))
    heal_id = _seed_heal(app_db, "HeAmp", _heal_props())
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    assert result is not None
    assert result["small_weapon"]["total_damage"] == pytest.approx(15.0)
    assert result["small_weapon"]["damage_max"] == pytest.approx(15.0)


# ---------------------------------------------------------------------------
# Output dict keys for weapon entries (mutmut 71,72,79,80,103,104)
# ---------------------------------------------------------------------------
def test_weapon_entry_dict_keys(app_db):
    """Each weapon entry exposes id/role/weapon_props under exact keys."""
    preset = _full_loadout(app_db, small_impact=10.0, big_impact=40.0)
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    small = result["small_weapon"]
    assert set(small) == {
        "id",
        "name",
        "role",
        "cost_per_shot_ped",
        "damage_min",
        "damage_max",
        "total_damage",
        "weapon_props",
    }
    assert small["role"] == "small_weapon"
    assert result["big_weapon"]["role"] == "big_weapon"
    assert isinstance(small["weapon_props"], dict)
    assert small["weapon_props"]["weapon_markup"] == 100
    assert isinstance(small["id"], int)
    assert small["name"] == "SmallW"


# ---------------------------------------------------------------------------
# Output dict keys for heal entry (mutmut 185,186,189,190,212,213)
# ---------------------------------------------------------------------------
def test_heal_entry_dict_keys(app_db):
    """The heal entry exposes id/name/heal_max under exact keys and values."""
    preset = _full_loadout(app_db, small_impact=10.0, big_impact=40.0)
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    heal = result["heal_tool"]
    assert set(heal) == {
        "id",
        "name",
        "cost_per_use_ped",
        "reload_seconds",
        "heal_min",
        "heal_max",
    }
    assert heal["name"] == "HealW"
    assert isinstance(heal["id"], int)
    assert heal["heal_min"] == 10
    assert heal["heal_max"] == 50


# ---------------------------------------------------------------------------
# heal markup default (mutmut 171,173,176)
# ---------------------------------------------------------------------------
def test_heal_markup_defaults_to_100_percent(app_db):
    """A heal tool with no markup uses 100% (1.0), not None or 101.

    cost_per_use = (decay + ammo/100) * markup, rounded to 4dp, then /100.
    decay 0.5, ammo_burn 20 -> ammo_pec 0.2 -> base 0.7 PEC.
    markup 1.0 -> 0.7 PEC -> 0.007 PED.  markup 1.01 -> 0.00707 PED.
    A None markup raises (None/100.0) inside describe_trifecta.
    """
    small_id = _seed_weapon(app_db, "SmMk", _weapon_props(10.0))
    big_id = _seed_weapon(app_db, "BiMk", _weapon_props(40.0))
    heal_props = {
        "tool_entity": {
            "min_heal": 10,
            "max_heal": 50,
            "uses_per_minute": 30,
            "economy": {"decay": 0.5, "ammo_burn": 20},
        },
        # no "markup" key
    }
    heal_id = _seed_heal(app_db, "HeMk", heal_props)
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    assert result is not None
    assert result["heal_tool"]["cost_per_use_ped"] == pytest.approx(0.007)


def test_heal_markup_used_when_present(app_db):
    """A 110% markup scales the heal cost (0.7 PEC * 1.1 = 0.77 PEC)."""
    small_id = _seed_weapon(app_db, "SmMk2", _weapon_props(10.0))
    big_id = _seed_weapon(app_db, "BiMk2", _weapon_props(40.0))
    heal_id = _seed_heal(app_db, "HeMk2", _heal_props(markup=110))
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    assert result["heal_tool"]["cost_per_use_ped"] == pytest.approx(0.0077)


# ---------------------------------------------------------------------------
# SQL column / row-key paths (mutmut 32,38,156,167 sanity end-to-end)
# These are exercised by the resolve test above, but assert the resolved
# values to anchor the queries against silent breakage.
# ---------------------------------------------------------------------------
def test_full_loadout_resolves_with_expected_values(app_db):
    preset = _full_loadout(app_db, small_impact=10.0, big_impact=40.0)
    result, error = describe_trifecta(app_db.conn, preset)
    assert error is None
    assert set(result) == {"small_weapon", "big_weapon", "heal_tool"}
    assert result["small_weapon"]["damage_max"] == pytest.approx(10.0)
    assert result["big_weapon"]["damage_min"] == pytest.approx(20.0)
    assert result["heal_tool"]["reload_seconds"] == pytest.approx(2.0)


# ---------------------------------------------------------------------------
# validate_trifecta (mutmut 1..6 -- "no tests")
# ---------------------------------------------------------------------------
def test_validate_trifecta_true_on_resolvable(app_db):
    """A resolvable preset validates True with no error.

    Kills: tuple-from-None (mutmut_1), wrong-arity calls (mutmut_4,5),
    swapped conn/preset args, and the is-None inversion (mutmut_6).
    """
    preset = _full_loadout(app_db, small_impact=10.0, big_impact=40.0)
    ok, error = validate_trifecta(app_db.conn, preset)
    assert ok is True
    assert error is None


def test_validate_trifecta_false_with_preset_none(app_db):
    """Passing preset=None must not be substituted away (mutmut_3).

    With a real preset the descriptor resolves; the mutant that forces
    preset=None reports the active-preset error and validates False.
    """
    preset = _full_loadout(app_db, small_impact=10.0, big_impact=40.0)
    ok, error = validate_trifecta(app_db.conn, preset)
    assert ok is True
    assert error is None


def test_validate_trifecta_false_on_unresolvable(app_db):
    """An unresolvable preset validates False with the descriptor's error."""
    preset = SimpleNamespace(
        small_weapon_id=900100, big_weapon_id=900101, heal_id=900102
    )
    ok, error = validate_trifecta(app_db.conn, preset)
    assert ok is False
    assert error is not None and "not found" in error


def test_validate_trifecta_uses_real_conn(app_db):
    """The conn arg must reach describe_trifecta (mutmut_2 forces conn=None).

    With conn=None and a real preset, describe_trifecta raises on
    conn.execute; the original resolves cleanly and validates True.
    """
    preset = _full_loadout(app_db, small_impact=10.0, big_impact=40.0)
    ok, error = validate_trifecta(app_db.conn, preset)
    assert ok is True
    assert error is None
