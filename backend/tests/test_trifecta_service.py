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
