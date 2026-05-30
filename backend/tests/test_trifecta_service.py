"""Tests for the trifecta-attribution descriptor's guard branches.

``describe_trifecta`` resolves the small/big/heal equipment of a preset and
reports why an attribution cannot proceed. These cover the not-found and
no-usable-damage-range guards against a real ``AppDatabase`` equipment table.
"""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from backend.db.app_database import AppDatabase
from backend.services.trifecta_service import describe_trifecta


@pytest.fixture
def app_db(tmp_path: Path):
    return AppDatabase(tmp_path / "trifecta.db")


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


def test_describe_trifecta_reports_missing_weapon(app_db):
    preset = SimpleNamespace(small_weapon_id=999, big_weapon_id=998, heal_id=997)

    result, error = describe_trifecta(app_db.conn, preset)

    assert result is None
    assert error is not None and "not found" in error


def _weapon_props(impact: float):
    """A minimal weapon entity that exposes a usable damage range and cost."""
    return {
        "weapon_entity": {
            "damage": {"impact": impact},
            "economy": {"decay": 1.0, "ammo_burn": 50},
        },
        "weapon_markup": 100,
        "damage_enhancers": 0,
    }


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


def test_describe_trifecta_resolves_a_full_loadout(app_db):
    """Two non-overlapping weapons plus a heal tool resolve with no error."""
    # total damage 10 -> range [5, 10]; total damage 40 -> range [20, 40]: no overlap.
    small_id = _seed_weapon(app_db, "Small", _weapon_props(10.0))
    big_id = _seed_weapon(app_db, "Big", _weapon_props(40.0))
    heal_id = _seed_heal(
        app_db,
        "Healer",
        {
            "tool_entity": {
                "min_heal": 10,
                "max_heal": 50,
                "uses_per_minute": 30,
                "economy": {"decay": 0.5, "ammo_burn": 20},
            },
            "markup": 110,
        },
    )
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )

    result, error = describe_trifecta(app_db.conn, preset)

    assert error is None
    assert result is not None
    assert set(result) == {"small_weapon", "big_weapon", "heal_tool"}
    assert result["small_weapon"]["damage_max"] == 10.0
    assert result["big_weapon"]["damage_min"] == 20.0
    assert result["heal_tool"]["heal_min"] == 10
    assert result["heal_tool"]["reload_seconds"] == 2.0


def test_describe_trifecta_rejects_overlapping_ranges(app_db):
    """Weapons whose damage ranges overlap cannot attribute by band."""
    small_id = _seed_weapon(app_db, "SmallA", _weapon_props(10.0))
    big_id = _seed_weapon(
        app_db, "BigA", _weapon_props(12.0)
    )  # range [6, 12] overlaps [5, 10]
    heal_id = _seed_heal(
        app_db,
        "HealerA",
        {"tool_entity": {"min_heal": 10, "max_heal": 50}, "markup": 100},
    )
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=heal_id
    )

    result, error = describe_trifecta(app_db.conn, preset)

    assert result is None
    assert error is not None and "non-overlapping" in error


def test_describe_trifecta_reports_missing_heal_tool(app_db):
    """Valid weapons but an absent heal tool is a reported guard failure."""
    small_id = _seed_weapon(app_db, "SmallB", _weapon_props(10.0))
    big_id = _seed_weapon(app_db, "BigB", _weapon_props(40.0))
    preset = SimpleNamespace(
        small_weapon_id=small_id, big_weapon_id=big_id, heal_id=997
    )

    result, error = describe_trifecta(app_db.conn, preset)

    assert result is None
    assert error is not None and "healing tool is not found" in error


def test_describe_trifecta_requires_a_preset():
    """A None preset reports the active-preset requirement."""
    result, error = describe_trifecta(None, None)
    assert result is None
    assert error is not None and "active preset" in error


def test_describe_trifecta_requires_all_three_ids(app_db):
    """A preset missing one of the three ids is rejected."""
    preset = SimpleNamespace(small_weapon_id=1, big_weapon_id=None, heal_id=3)
    result, error = describe_trifecta(app_db.conn, preset)
    assert result is None
    assert error is not None and "configured small weapon" in error


def test_describe_trifecta_reports_no_usable_damage_range(app_db):
    # A weapon entity with no damage fields exposes no usable range.
    props = {
        "weapon_entity": {
            "name": "Decay-only",
            "economy": {"decay": 1.0, "ammo_burn": 0},
        },
        "damage_enhancers": 0,
    }
    weapon_id = _seed_weapon(app_db, "DecayOnly", props)
    preset = SimpleNamespace(
        small_weapon_id=weapon_id, big_weapon_id=weapon_id, heal_id=997
    )

    result, error = describe_trifecta(app_db.conn, preset)

    assert result is None
    assert error is not None and "damage range" in error
