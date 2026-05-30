"""Property-based tests for the equipment-library router.

Covers ``backend.routers.equipment``: the thin HTTP-adapter CRUD surface over
``equipment_library`` plus the catalogue-resolution helpers. These handlers are
keyed only on the request body, a static bundled catalogue, and the persisted
rows; they never consult session, loot, or event-bus state, so the generated
inputs here are equipment configurations rather than event sequences.

Each ``@given`` builds its own in-memory ``AppDatabase`` and a stubbed service
locator so the router runs against a real schema without leaning on a
function-scoped fixture (which ``@given`` would not reset between examples).

Two router-surface invariants are encoded:

* update never mutates an item's stored type: a type-mismatched update is
  rejected with 400, and a matching update rewrites only name/catalog_id/
  properties (the SET clause never touches item_type);
* catalogue resolution is total: a provided component id either resolves to a
  catalogue entity or the handler raises 404, never silently storing or pricing
  an unresolved id.
"""

import contextlib
import json
from types import SimpleNamespace

import pytest
from fastapi import HTTPException
from hypothesis import given
from hypothesis import strategies as st

from backend.db.app_database import AppDatabase
from backend.routers import equipment

# A small in-memory catalogue mirroring the shape the bundled snapshot exposes:
# every entity carries a non-empty name and a unique, non-empty id, which is the
# data precondition the catalogue-resolution invariant relies on.
_CATALOGUE = {
    ("weapons", "w1"): {
        "id": "w1",
        "name": "Test Weapon",
        "economy": {"decay": 2.0, "ammo_burn": 200},
    },
    ("weapon_amplifiers", "a1"): {
        "id": "a1",
        "name": "Test Amp",
        "economy": {"decay": 1.0, "ammo_burn": 100},
    },
    ("weapon_vision_attachments", "s1"): {
        "id": "s1",
        "name": "Test Scope",
        "economy": {"decay": 0.5},
    },
    ("absorbers", "ab1"): {
        "id": "ab1",
        "name": "Test Absorber",
        "economy": {"absorption": 0.12},
    },
    ("medical_tools", "t1"): {
        "id": "t1",
        "name": "Test FAP",
        "economy": {"decay": 1.0, "ammo_burn": 0},
    },
    ("stimulants", "c1"): {"id": "c1", "name": "Test Stim", "economy": {}},
}

# Ids the catalogue deliberately does not know about, so a resolution attempt
# must take the 404 path rather than returning a row.
_UNKNOWN_IDS = ["", "  ", "missing", "w1 ", "W1", "00", "zzz"]


def _catalogue_game_data():
    return SimpleNamespace(
        find_entity=lambda endpoint, item_id: _CATALOGUE.get((endpoint, item_id))
    )


def _services(app_db, *, config=None):
    return SimpleNamespace(
        app_db=app_db,
        game_data=_catalogue_game_data(),
        config_service=SimpleNamespace(get=lambda: config),
    )


@contextlib.contextmanager
def _router_env(config=None):
    """Yield a fresh in-memory-backed router environment.

    A new ``AppDatabase`` and service stub are built per call and ``get_services``
    is swapped for the duration, so each generated example runs in isolation.
    """
    db = AppDatabase(":memory:")
    original = equipment.get_services
    equipment.get_services = lambda: _services(db, config=config)
    try:
        yield db
    finally:
        equipment.get_services = original


def _seed_row(db, *, name, item_type, catalog_id, props):
    db.conn.execute(
        "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) "
        "VALUES (?, ?, ?, ?)",
        (name, item_type, catalog_id, json.dumps(props)),
    )
    db.conn.commit()
    return db.conn.execute(
        "SELECT id FROM equipment_library ORDER BY id DESC LIMIT 1"
    ).fetchone()["id"]


def _stored_type(db, item_id: int) -> str:
    return db.conn.execute(
        "SELECT item_type FROM equipment_library WHERE id = ?", (item_id,)
    ).fetchone()["item_type"]


_ALL_TYPES = ("weapon", "healing", "consumable")
# Identity catalog_ids per type so a matching-type update is always a valid,
# resolvable request (the type-preservation invariant is about the gate, not
# the resolution path).
_CATALOG_FOR_TYPE = {"weapon": "w1", "healing": "t1", "consumable": "c1"}
_TYPE_PAIRS = [(s, t) for s in _ALL_TYPES for t in _ALL_TYPES]


# --- update_preserves_item_type -----------------------------------------------


@given(
    pair=st.sampled_from(_TYPE_PAIRS), markup=st.integers(min_value=1, max_value=300)
)
def test_update_never_changes_the_stored_item_type(pair, markup):
    """A request whose type differs from the stored row is rejected with 400;
    a same-type update leaves item_type untouched. Either way the persisted
    item_type equals the type the row was created with."""
    existing_type, req_type = pair
    with _router_env() as db:
        item_id = int(
            equipment.add_to_library(_seed_request(existing_type, markup=markup))["id"]
            if existing_type != "consumable"
            else _seed_row(
                db,
                name="Seed Stim",
                item_type="consumable",
                catalog_id="c1",
                props={"catalog_id": "c1", "entity": _CATALOGUE[("stimulants", "c1")]},
            )
        )

        req = _seed_request(req_type, markup=markup)
        if req_type != existing_type:
            with pytest.raises(HTTPException) as exc:
                equipment.update_library_item(item_id, req)
            assert exc.value.status_code == 400
        else:
            result = equipment.update_library_item(item_id, req)
            assert result["type"] == existing_type

        # The persisted type is structurally immutable through this endpoint:
        # the SET clause never writes item_type, and the gate rejects any
        # mismatched request before it could.
        assert _stored_type(db, item_id) == existing_type


def _seed_request(item_type, *, markup=100):
    if item_type == "weapon":
        return equipment.AddWeaponRequest(
            type="weapon", catalog_id="w1", weapon_markup=markup
        )
    if item_type == "healing":
        return equipment.AddWeaponRequest(
            type="healing", catalog_id="t1", weapon_markup=markup
        )
    return equipment.AddWeaponRequest(type="consumable", catalog_id="c1")


# --- catalogue_resolution_or_404 ----------------------------------------------


@given(item_id=st.sampled_from(_UNKNOWN_IDS))
def test_add_weapon_with_unresolved_catalog_id_raises_404(item_id):
    """A provided weapon catalog_id that the catalogue does not know about takes
    the 404 path rather than silently storing/pricing a None entity."""
    with _router_env():
        req = equipment.AddWeaponRequest(type="weapon", catalog_id=item_id)
        if not item_id:
            # An empty/whitespace primary id is caught by the explicit 400 guard
            # before resolution; that is still "not silently stored".
            with pytest.raises(HTTPException) as exc:
                equipment.add_to_library(req)
            assert exc.value.status_code == 400
        else:
            with pytest.raises(HTTPException) as exc:
                equipment.add_to_library(req)
            assert exc.value.status_code == 404


@given(
    amp_id=st.sampled_from([i for i in _UNKNOWN_IDS if i.strip()]),
)
def test_add_weapon_with_unresolved_component_id_raises_404(amp_id):
    """A truthy but unresolved optional component id (amp here) is fetched and
    must 404; it is never stored as a None entity beside a real catalog_id."""
    with _router_env():
        req = equipment.AddWeaponRequest(
            type="weapon", catalog_id="w1", amp_catalog_id=amp_id
        )
        with pytest.raises(HTTPException) as exc:
            equipment.add_to_library(req)
        assert exc.value.status_code == 404


@given(item_id=st.sampled_from(_UNKNOWN_IDS))
def test_calculate_cost_with_unresolved_catalog_id_raises_404(item_id):
    """calculate_cost has no explicit empty-id 400 guard, so an empty/whitespace
    or unknown weapon id reaches resolution; with no catalogue entity carrying a
    matching (or empty) id it always 404s rather than 500ing or pricing None."""
    with _router_env():
        req = equipment.CalculateCostRequest(catalog_id=item_id, type="weapon")
        with pytest.raises(HTTPException) as exc:
            equipment.calculate_cost(req)
        assert exc.value.status_code == 404


@given(item_id=st.sampled_from(_UNKNOWN_IDS))
def test_calculate_cost_healing_with_unresolved_catalog_id_raises_404(item_id):
    """The healing branch of calculate_cost resolves the tool id the same way;
    an unknown id must 404 rather than pricing a None tool."""
    with _router_env():
        req = equipment.CalculateCostRequest(catalog_id=item_id, type="healing")
        with pytest.raises(HTTPException) as exc:
            equipment.calculate_cost(req)
        assert exc.value.status_code == 404


@given(
    weapon=st.sampled_from(["w1"]),
    amp=st.sampled_from([None, "a1"]),
    scope=st.sampled_from([None, "s1"]),
    absorber=st.sampled_from([None, "ab1"]),
)
def test_resolved_weapon_add_stores_only_known_entities(weapon, amp, scope, absorber):
    """When every provided id resolves, the persisted props hold a real entity
    (a dict with the catalogue name) for each provided id, and None exactly
    where no id was supplied; a provided id never yields a stored None."""
    with _router_env() as db:
        req = equipment.AddWeaponRequest(
            type="weapon",
            catalog_id=weapon,
            amp_catalog_id=amp,
            scope_catalog_id=scope,
            absorber_catalog_id=absorber,
        )
        result = equipment.add_to_library(req)
        props = json.loads(
            db.conn.execute(
                "SELECT properties_json FROM equipment_library WHERE id = ?",
                (int(result["id"]),),
            ).fetchone()["properties_json"]
        )

        assert isinstance(props["weapon_entity"], dict)
        assert props["weapon_entity"]["name"] == _CATALOGUE[("weapons", weapon)]["name"]
        for provided, key, endpoint in (
            (amp, "amp_entity", "weapon_amplifiers"),
            (scope, "scope_entity", "weapon_vision_attachments"),
            (absorber, "absorber_entity", "absorbers"),
        ):
            if provided is None:
                assert props[key] is None
            else:
                assert isinstance(props[key], dict)
                assert props[key]["name"] == _CATALOGUE[(endpoint, provided)]["name"]
