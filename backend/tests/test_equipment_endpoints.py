"""Endpoint tests for the equipment library router.

Drives the search guards, the library CRUD validation branches, the detail
lookup, and the trifecta-protected delete through the service-locator seam
against a real ``AppDatabase`` in a temp dir, giving the thin HTTP-adapter paths
direct, deterministic cover (they are otherwise only reached incidentally by the
contract suite's randomised requests).
"""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi import HTTPException

from backend.db.app_database import AppDatabase
from backend.routers import equipment


@pytest.fixture
def app_db(tmp_path: Path):
    return AppDatabase(tmp_path / "equipment.db")


def _services(app_db, *, game_data=None, config=None):
    return SimpleNamespace(
        app_db=app_db,
        game_data=game_data or SimpleNamespace(),
        config_service=SimpleNamespace(get=lambda: config),
    )


def _seed_item(app_db, name="FAP-5", item_type="healing", catalog_id="h1"):
    app_db.conn.execute(
        "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) "
        "VALUES (?, ?, ?, ?)",
        (name, item_type, catalog_id, json.dumps({})),
    )
    app_db.conn.commit()
    return app_db.conn.execute("SELECT id FROM equipment_library").fetchone()["id"]


# ── search guards (resolved before any service lookup) ────────────────────────


def test_search_unknown_type_rejected():
    with pytest.raises(HTTPException) as exc:
        equipment.search_items(q="laser", type="bogus")
    assert exc.value.status_code == 400


def test_search_short_query_returns_empty():
    assert equipment.search_items(q="a", type="weapon") == []


# ── library read ──────────────────────────────────────────────────────────────


def test_get_library_empty(app_db, monkeypatch):
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    assert equipment.get_library() == []


# ── add validation branches ───────────────────────────────────────────────────


def test_add_weapon_without_catalog_id_rejected(app_db, monkeypatch):
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    with pytest.raises(HTTPException) as exc:
        equipment.add_to_library(equipment.AddWeaponRequest(type="weapon"))
    assert exc.value.status_code == 400


def test_add_healing_without_catalog_id_rejected(app_db, monkeypatch):
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    with pytest.raises(HTTPException) as exc:
        equipment.add_to_library(equipment.AddWeaponRequest(type="healing"))
    assert exc.value.status_code == 400


def test_add_consumable_without_identity_rejected(app_db, monkeypatch):
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    with pytest.raises(HTTPException) as exc:
        equipment.add_to_library(equipment.AddWeaponRequest(type="consumable"))
    assert exc.value.status_code == 400


def test_add_custom_consumable_stores_freetext_name(app_db, monkeypatch):
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))

    result = equipment.add_to_library(
        equipment.AddWeaponRequest(type="consumable", name="  Home-brew Stim  ")
    )

    assert result["name"] == "Home-brew Stim"
    # Persisted: the library now lists exactly the one item.
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    assert len(equipment.get_library()) == 1


# ── update validation branches ────────────────────────────────────────────────


def test_update_missing_item_returns_404(app_db, monkeypatch):
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    with pytest.raises(HTTPException) as exc:
        equipment.update_library_item(
            999, equipment.AddWeaponRequest(type="weapon", catalog_id="w1")
        )
    assert exc.value.status_code == 404


def test_update_type_change_rejected(app_db, monkeypatch):
    item_id = _seed_item(app_db, item_type="healing")
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    with pytest.raises(HTTPException) as exc:
        equipment.update_library_item(
            item_id, equipment.AddWeaponRequest(type="weapon", catalog_id="w1")
        )
    assert exc.value.status_code == 400


def test_update_healing_without_catalog_id_rejected(app_db, monkeypatch):
    item_id = _seed_item(app_db, item_type="healing")
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    with pytest.raises(HTTPException) as exc:
        equipment.update_library_item(
            item_id, equipment.AddWeaponRequest(type="healing")
        )
    assert exc.value.status_code == 400


def test_update_consumable_renames_freetext(app_db, monkeypatch):
    item_id = _seed_item(
        app_db, name="Old Stim", item_type="consumable", catalog_id=None
    )
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))

    result = equipment.update_library_item(
        item_id, equipment.AddWeaponRequest(type="consumable", name="Renamed Stim")
    )

    assert result["name"] == "Renamed Stim"
    assert result["type"] == "consumable"


# ── detail + delete ───────────────────────────────────────────────────────────


def test_detail_missing_item_returns_404(app_db, monkeypatch):
    monkeypatch.setattr(equipment, "get_services", lambda: _services(app_db))
    with pytest.raises(HTTPException) as exc:
        equipment.get_library_detail(999)
    assert exc.value.status_code == 404


def test_remove_item_succeeds(app_db, monkeypatch):
    item_id = _seed_item(app_db)
    config = SimpleNamespace(trifecta_presets=[])
    monkeypatch.setattr(
        equipment, "get_services", lambda: _services(app_db, config=config)
    )

    assert equipment.remove_from_library(item_id) == {"status": "deleted"}


def test_remove_item_blocked_by_trifecta_preset(app_db, monkeypatch):
    preset = SimpleNamespace(small_weapon_id=1, big_weapon_id=2, heal_id=3)
    config = SimpleNamespace(trifecta_presets=[preset])
    monkeypatch.setattr(
        equipment, "get_services", lambda: _services(app_db, config=config)
    )

    with pytest.raises(HTTPException) as exc:
        equipment.remove_from_library(1)  # 1 is the preset's small_weapon_id
    assert exc.value.status_code == 409


# ── weapon add/update/cost paths (catalogue entity resolution) ────────────────

_CATALOGUE = {
    ("weapons", "w1"): {
        "name": "Test Weapon",
        "economy": {"decay": 2.0, "ammo_burn": 200},
    },
    ("weapon_amplifiers", "a1"): {
        "name": "Test Amp",
        "economy": {"decay": 1.0, "ammo_burn": 100},
    },
    ("weapon_vision_attachments", "s1"): {
        "name": "Test Scope",
        "economy": {"decay": 0.5},
    },
    ("absorbers", "ab1"): {"name": "Test Absorber", "economy": {"absorption": 0.12}},
    ("medical_tools", "t1"): {
        "name": "Test FAP",
        "economy": {"decay": 1.0, "ammo_burn": 0},
    },
}


def _catalogue_game_data():
    return SimpleNamespace(
        find_entity=lambda endpoint, item_id: _CATALOGUE.get((endpoint, item_id))
    )


def test_add_weapon_with_attachments_persists(app_db, monkeypatch):
    monkeypatch.setattr(
        equipment,
        "get_services",
        lambda: _services(app_db, game_data=_catalogue_game_data()),
    )

    result = equipment.add_to_library(
        equipment.AddWeaponRequest(
            type="weapon",
            catalog_id="w1",
            amp_catalog_id="a1",
            scope_catalog_id="s1",
            absorber_catalog_id="ab1",
            damage_enhancers=2,
        )
    )

    assert result["type"] == "weapon"
    assert result["name"] == "Test Weapon"
    assert result["amplifierName"] == "Test Amp"
    assert result["costPerUse"] > 0
    assert result["enrichmentLevel"] == 3  # amp plus scope/absorber


def test_update_weapon_without_catalog_id_rejected(app_db, monkeypatch):
    monkeypatch.setattr(
        equipment,
        "get_services",
        lambda: _services(app_db, game_data=_catalogue_game_data()),
    )
    added = equipment.add_to_library(
        equipment.AddWeaponRequest(type="weapon", catalog_id="w1")
    )

    with pytest.raises(HTTPException) as exc:
        equipment.update_library_item(
            int(added["id"]), equipment.AddWeaponRequest(type="weapon")
        )
    assert exc.value.status_code == 400


def test_update_weapon_replaces_attachments(app_db, monkeypatch):
    monkeypatch.setattr(
        equipment,
        "get_services",
        lambda: _services(app_db, game_data=_catalogue_game_data()),
    )
    added = equipment.add_to_library(
        equipment.AddWeaponRequest(type="weapon", catalog_id="w1")
    )

    result = equipment.update_library_item(
        int(added["id"]),
        equipment.AddWeaponRequest(
            type="weapon", catalog_id="w1", amp_catalog_id="a1", scope_catalog_id="s1"
        ),
    )

    assert result["amplifierName"] == "Test Amp"


def test_calculate_cost_for_weapon_with_scope(monkeypatch):
    monkeypatch.setattr(
        equipment,
        "get_services",
        lambda: _services(None, game_data=_catalogue_game_data()),
    )

    result = equipment.calculate_cost(
        equipment.CalculateCostRequest(
            catalog_id="w1", type="weapon", scope_catalog_id="s1"
        )
    )

    assert result["totalCostPerUse"] > 0
    assert result["costBreakdown"]


def test_calculate_cost_for_healing_tool(monkeypatch):
    monkeypatch.setattr(
        equipment,
        "get_services",
        lambda: _services(None, game_data=_catalogue_game_data()),
    )

    result = equipment.calculate_cost(
        equipment.CalculateCostRequest(catalog_id="t1", type="healing")
    )

    assert "totalCostPerUse" in result
