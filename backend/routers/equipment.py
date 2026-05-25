"""Equipment library endpoints — search, CRUD, cost calculation."""

import json
from typing import Literal

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from backend.dependencies import get_services
from backend.services.cost_engine import (
    cost_per_shot,
    cost_per_shot_from_props,
    get_weapon_damage_profile,
    heal_cost_per_use,
    heal_reload_seconds,
    is_limited,
)

router = APIRouter(prefix="/equipment", tags=["equipment"])

# ── Type → catalogue endpoint mapping ────────────────────────────────────────

_TYPE_ENDPOINT = {
    "weapon": "weapons",
    "amp": "weapon_amplifiers",
    "healer": "medical_tools",
    "scope": "weapon_vision_attachments",
    "absorber": "absorbers",
    "consumable": "stimulants",
}


def _resolve_consumable_identity(req, game_data) -> tuple[str, str | None, dict | None]:
    """Return (name, stored_id, entity) for a consumable add/update request.

    Catalogue pick: req.catalog_id resolves in the stimulants catalogue; the
    full entity is returned so the library can cache it alongside the id.
    Free-text: req.name provided → stored with catalog_id=None and no entity.
    """
    if req.catalog_id:
        entity = _fetch_entity(game_data, "stimulants", req.catalog_id)
        return entity["name"], req.catalog_id, entity
    if req.name and req.name.strip():
        return req.name.strip(), None, None
    raise HTTPException(
        status_code=400,
        detail="Consumable requires either catalog_id (catalogue pick) or name (custom)",
    )


def _fetch_entity(game_data, endpoint: str, item_id: str) -> dict:
    """Look up a single entity from the catalogue by its Id."""
    entity = game_data.find_entity(endpoint, item_id)
    if entity is None:
        raise HTTPException(
            status_code=404,
            detail=f"Entity '{item_id}' not found in catalogue endpoint '{endpoint}'.",
        )
    return entity


def _entity_to_search_result(row: dict) -> dict:
    """Map a catalogue search row to the EquipmentSearchResult shape."""
    entity = row["data"]
    eco = entity.get("economy") or {}
    return {
        "catalogId": row["item_id"],
        "name": row["item_name"],
        "decay": eco.get("decay") or 0.0,
        "ammoBurn": (eco.get("ammo_burn") or 0.0) / 100.0,  # ammo units → PEC
        "isLimited": is_limited(entity),
    }


def _compute_enrichment(props: dict) -> int:
    """Enrichment level based on which components are configured."""
    if props.get("amp_entity"):
        if props.get("scope_entity") or props.get("absorber_entity"):
            return 3
        return 2
    return 1


def _weapon_search_result_from_entity(
    catalog_id: str | None,
    entity: dict,
    markup_percent: int,
    *,
    damage_enhancers: int = 0,
) -> dict:
    eco = entity.get("economy") or {}
    return {
        "catalogId": catalog_id,
        "name": entity["name"],
        "decay": eco.get("decay") or 0.0,
        "ammoBurn": (eco.get("ammo_burn") or 0.0) / 100.0,
        "markupPercent": markup_percent,
        "isLimited": is_limited(entity),
        "damageEnhancers": damage_enhancers,
    }


def _row_optional_value(row, key: str):
    """Read an optional sqlite row column without requiring every query to select it."""
    keys = row.keys() if hasattr(row, "keys") else ()
    if key not in keys:
        return None
    return row[key]


def _library_row_to_equipment(row) -> dict:
    """Convert an equipment_library DB row to the Equipment frontend shape."""
    props = json.loads(row["properties_json"])
    item_type = row["item_type"]

    if item_type == "weapon":
        weapon_e = props["weapon_entity"]
        amp_e = props.get("amp_entity")
        scope_e = props.get("scope_entity")
        absorber_e = props.get("absorber_entity")
        damage_enhancers = max(0, int(props.get("damage_enhancers", 0) or 0))
        cost_result = cost_per_shot_from_props(props)
        damage_profile = get_weapon_damage_profile(
            weapon_e,
            amp=amp_e,
            damage_enhancers=damage_enhancers,
        )
        return {
            "id": str(row["id"]),
            "name": row["name"],
            "type": "weapon",
            "amplifierName": amp_e["name"] if amp_e else None,
            "costPerUse": cost_result["totalCostPerUse"],
            "damageMin": round(damage_profile["damageMin"], 2)
            if damage_profile
            else None,
            "damageMax": round(damage_profile["damageMax"], 2)
            if damage_profile
            else None,
            "reloadSeconds": None,
            "isLimited": is_limited(weapon_e),
            "enrichmentLevel": _compute_enrichment(props),
        }

    if item_type == "consumable":
        return {
            "id": str(row["id"]),
            "name": row["name"],
            "type": "consumable",
            "amplifierName": None,
            "costPerUse": 0.0,
            "damageMin": None,
            "damageMax": None,
            "reloadSeconds": None,
            "isLimited": False,
            "enrichmentLevel": 1,
        }

    # healing
    tool_e = props["tool_entity"]
    markup = props.get("markup", 100) / 100.0
    return {
        "id": str(row["id"]),
        "name": row["name"],
        "type": "healing",
        "amplifierName": None,
        "costPerUse": heal_cost_per_use(tool_e, markup),
        "damageMin": None,
        "damageMax": None,
        "reloadSeconds": round(heal_reload_seconds(tool_e), 2),
        "isLimited": is_limited(tool_e),
        "enrichmentLevel": 1,
    }


def _library_row_to_detail(row) -> dict:
    """Convert an equipment_library DB row to the EquipmentDetail frontend shape."""
    props = json.loads(row["properties_json"])
    item_type = row["item_type"]
    item_id = str(row["id"])

    if item_type == "weapon":
        weapon_e = props["weapon_entity"]
        amp_e = props.get("amp_entity")
        scope_e = props.get("scope_entity")
        absorber_e = props.get("absorber_entity")
        weapon_markup_pct = props.get("weapon_markup", 100)
        amp_markup_pct = props.get("amp_markup", 100)
        scope_markup_pct = props.get("scope_markup", 100)
        absorber_markup_pct = props.get("absorber_markup", 100)
        damage_enhancers = max(0, int(props.get("damage_enhancers", 0) or 0))

        weapon_eco = weapon_e.get("economy") or {}
        cost_result = cost_per_shot_from_props(props)

        amp_detail = None
        if amp_e:
            amp_detail = _weapon_search_result_from_entity(
                props.get("amp_catalog_id"),
                amp_e,
                amp_markup_pct,
            )

        scope_detail = None
        if scope_e:
            scope_detail = _weapon_search_result_from_entity(
                props.get("scope_catalog_id"),
                scope_e,
                scope_markup_pct,
            )

        absorber_detail = None
        if absorber_e:
            absorber_eco = absorber_e.get("economy") or {}
            absorption_pct = (absorber_eco.get("absorption") or 0.0) * 100.0
            absorber_detail = {
                "catalogId": props.get("absorber_catalog_id"),
                "name": absorber_e["name"],
                "decay": absorber_eco.get("decay") or 0.0,
                "ammoBurn": (absorber_eco.get("ammo_burn") or 0.0) / 100.0,
                "absorptionPercent": round(absorption_pct, 1),
                "markupPercent": absorber_markup_pct,
                "isLimited": is_limited(absorber_e),
            }

        return {
            "id": item_id,
            "weapon": {
                "catalogId": props.get("weapon_catalog_id")
                or _row_optional_value(row, "catalog_id"),
                "name": weapon_e["name"],
                "decay": weapon_eco.get("decay") or 0.0,
                "ammoBurn": (weapon_eco.get("ammo_burn") or 0.0) / 100.0,
                "markupPercent": weapon_markup_pct,
                "isLimited": is_limited(weapon_e),
                "damageEnhancers": damage_enhancers,
            },
            "amplifier": amp_detail,
            "scope": scope_detail,
            "absorber": absorber_detail,
            "costBreakdown": cost_result["costBreakdown"],
            "totalCostPerUse": cost_result["totalCostPerUse"],
        }

    if item_type == "consumable":
        return {
            "id": item_id,
            "weapon": {
                "catalogId": _row_optional_value(row, "catalog_id"),
                "name": row["name"],
                "decay": 0.0,
                "ammoBurn": 0.0,
                "markupPercent": 100,
                "isLimited": False,
                "damageEnhancers": 0,
            },
            "amplifier": None,
            "scope": None,
            "absorber": None,
            "costBreakdown": [],
            "totalCostPerUse": 0.0,
        }

    # Healing tool detail (simplified)
    tool_e = props["tool_entity"]
    tool_eco = tool_e.get("economy") or {}
    markup_pct = props.get("markup", 100)
    cost = heal_cost_per_use(tool_e, markup_pct / 100.0)
    breakdown = [
        {
            "component": "Decay",
            "costPec": tool_eco.get("decay") or 0.0,
            "markupMultiplier": markup_pct / 100.0,
            "effectiveCostPec": round(
                (tool_eco.get("decay") or 0.0) * markup_pct / 100.0, 4
            ),
        }
    ]
    ammo_pec = (tool_eco.get("ammo_burn") or 0.0) / 100.0
    if ammo_pec > 0:
        breakdown.append(
            {
                "component": "Ammo",
                "costPec": ammo_pec,
                "markupMultiplier": 1.0,
                "effectiveCostPec": ammo_pec,
            }
        )

    return {
        "id": item_id,
        "weapon": {
            "catalogId": props.get("tool_catalog_id")
            or _row_optional_value(row, "catalog_id"),
            "name": tool_e["name"],
            "decay": tool_eco.get("decay") or 0.0,
            "ammoBurn": ammo_pec,
            "markupPercent": markup_pct,
            "isLimited": is_limited(tool_e),
            "damageEnhancers": 0,
        },
        "amplifier": None,
        "scope": None,
        "absorber": None,
        "costBreakdown": breakdown,
        "totalCostPerUse": cost,
    }


# ── Request models ────────────────────────────────────────────────────────────


class AddWeaponRequest(BaseModel):
    type: Literal["weapon", "healing", "consumable"]
    catalog_id: str | None = None
    name: str | None = None
    amp_catalog_id: str | None = None
    scope_catalog_id: str | None = None
    absorber_catalog_id: str | None = None
    weapon_markup: int = 100
    amp_markup: int = 100
    scope_markup: int = 100
    absorber_markup: int = 100
    damage_enhancers: int = 0


class CalculateCostRequest(BaseModel):
    catalog_id: str
    type: Literal["weapon", "healing"] = "weapon"
    amp_catalog_id: str | None = None
    scope_catalog_id: str | None = None
    absorber_catalog_id: str | None = None
    weapon_markup: int = 100
    amp_markup: int = 100
    scope_markup: int = 100
    absorber_markup: int = 100
    damage_enhancers: int = 0


# ── Endpoints ─────────────────────────────────────────────────────────────────


@router.get("/search")
def search_items(q: str = "", type: str = "weapon"):
    """Search the bundled equipment catalogue by name and type.

    type: weapon | amp | healer | scope | absorber | consumable
    Returns items with ammoBurn already converted to PEC.
    """
    endpoint = _TYPE_ENDPOINT.get(type)
    if endpoint is None:
        raise HTTPException(status_code=400, detail=f"Unknown type '{type}'")
    if len(q) < 2:
        return []

    svc = get_services()
    rows = svc.game_data.search_entities(q, endpoint=endpoint)
    return [_entity_to_search_result(r) for r in rows]


@router.get("/library")
def get_library():
    """Return all items in the user's equipment library with computed costs."""
    svc = get_services()
    rows = svc.app_db.conn.execute(
        "SELECT id, name, item_type, properties_json FROM equipment_library ORDER BY created_at"
    ).fetchall()
    return [_library_row_to_equipment(r) for r in rows]


@router.post("/library")
def add_to_library(req: AddWeaponRequest):
    """Add an item to the equipment library."""
    svc = get_services()

    if req.type == "weapon":
        if not req.catalog_id:
            raise HTTPException(
                status_code=400, detail="catalog_id required for weapon"
            )
        weapon_e = _fetch_entity(svc.game_data, "weapons", req.catalog_id)
        amp_e = None
        if req.amp_catalog_id:
            amp_e = _fetch_entity(
                svc.game_data, "weapon_amplifiers", req.amp_catalog_id
            )
        scope_e = None
        if req.scope_catalog_id:
            scope_e = _fetch_entity(
                svc.game_data, "weapon_vision_attachments", req.scope_catalog_id
            )
        absorber_e = None
        if req.absorber_catalog_id:
            absorber_e = _fetch_entity(
                svc.game_data, "absorbers", req.absorber_catalog_id
            )

        props = {
            "weapon_entity": weapon_e,
            "weapon_catalog_id": req.catalog_id,
            "amp_entity": amp_e,
            "amp_catalog_id": req.amp_catalog_id,
            "scope_entity": scope_e,
            "scope_catalog_id": req.scope_catalog_id,
            "absorber_entity": absorber_e,
            "absorber_catalog_id": req.absorber_catalog_id,
            "weapon_markup": req.weapon_markup,
            "amp_markup": req.amp_markup,
            "scope_markup": req.scope_markup,
            "absorber_markup": req.absorber_markup,
            "damage_enhancers": max(0, req.damage_enhancers),
        }
        name = weapon_e["name"]
        stored_catalog_id: str | None = req.catalog_id

    elif req.type == "healing":
        if not req.catalog_id:
            raise HTTPException(
                status_code=400, detail="catalog_id required for healing"
            )
        tool_e = _fetch_entity(svc.game_data, "medical_tools", req.catalog_id)
        props = {
            "tool_entity": tool_e,
            "tool_catalog_id": req.catalog_id,
            "markup": req.weapon_markup,
        }
        name = tool_e["name"]
        stored_catalog_id = req.catalog_id

    else:  # consumable
        name, stored_catalog_id, entity = _resolve_consumable_identity(
            req, svc.game_data
        )
        props = {"catalog_id": stored_catalog_id, "entity": entity}

    cursor = svc.app_db.conn.execute(
        "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) "
        "VALUES (?, ?, ?, ?)",
        (name, req.type, stored_catalog_id, json.dumps(props)),
    )
    svc.app_db.conn.commit()

    row = svc.app_db.conn.execute(
        "SELECT id, name, item_type, properties_json FROM equipment_library WHERE id = ?",
        (cursor.lastrowid,),
    ).fetchone()
    return _library_row_to_equipment(row)


@router.put("/library/{item_id}")
def update_library_item(item_id: int, req: AddWeaponRequest):
    """Update an existing equipment library entry."""
    svc = get_services()
    existing = svc.app_db.conn.execute(
        "SELECT id, item_type FROM equipment_library WHERE id = ?",
        (item_id,),
    ).fetchone()
    if existing is None:
        raise HTTPException(
            status_code=404, detail=f"Equipment item {item_id} not found"
        )
    if existing["item_type"] != req.type:
        raise HTTPException(status_code=400, detail="Cannot change equipment type")

    if req.type == "weapon":
        if not req.catalog_id:
            raise HTTPException(
                status_code=400, detail="catalog_id required for weapon"
            )
        weapon_e = _fetch_entity(svc.game_data, "weapons", req.catalog_id)
        amp_e = (
            _fetch_entity(svc.game_data, "weapon_amplifiers", req.amp_catalog_id)
            if req.amp_catalog_id
            else None
        )
        scope_e = (
            _fetch_entity(
                svc.game_data, "weapon_vision_attachments", req.scope_catalog_id
            )
            if req.scope_catalog_id
            else None
        )
        absorber_e = (
            _fetch_entity(svc.game_data, "absorbers", req.absorber_catalog_id)
            if req.absorber_catalog_id
            else None
        )

        props = {
            "weapon_entity": weapon_e,
            "weapon_catalog_id": req.catalog_id,
            "amp_entity": amp_e,
            "amp_catalog_id": req.amp_catalog_id,
            "scope_entity": scope_e,
            "scope_catalog_id": req.scope_catalog_id,
            "absorber_entity": absorber_e,
            "absorber_catalog_id": req.absorber_catalog_id,
            "weapon_markup": req.weapon_markup,
            "amp_markup": req.amp_markup,
            "scope_markup": req.scope_markup,
            "absorber_markup": req.absorber_markup,
            "damage_enhancers": max(0, req.damage_enhancers),
        }
        name = weapon_e["name"]
        stored_catalog_id: str | None = req.catalog_id
    elif req.type == "healing":
        if not req.catalog_id:
            raise HTTPException(
                status_code=400, detail="catalog_id required for healing"
            )
        tool_e = _fetch_entity(svc.game_data, "medical_tools", req.catalog_id)
        props = {
            "tool_entity": tool_e,
            "tool_catalog_id": req.catalog_id,
            "markup": req.weapon_markup,
        }
        name = tool_e["name"]
        stored_catalog_id = req.catalog_id
    else:  # consumable
        name, stored_catalog_id, entity = _resolve_consumable_identity(
            req, svc.game_data
        )
        props = {"catalog_id": stored_catalog_id, "entity": entity}

    svc.app_db.conn.execute(
        "UPDATE equipment_library SET name = ?, catalog_id = ?, properties_json = ? WHERE id = ?",
        (name, stored_catalog_id, json.dumps(props), item_id),
    )
    svc.app_db.conn.commit()

    row = svc.app_db.conn.execute(
        "SELECT id, name, item_type, catalog_id, properties_json FROM equipment_library WHERE id = ?",
        (item_id,),
    ).fetchone()
    return _library_row_to_equipment(row)


@router.delete("/library/{item_id}")
def remove_from_library(item_id: int):
    """Remove an item from the equipment library."""
    svc = get_services()
    config = svc.config_service.get()
    trifecta_ids: set[int | None] = set()
    for preset in config.trifecta_presets:
        trifecta_ids.update(
            {preset.small_weapon_id, preset.big_weapon_id, preset.heal_id}
        )

    if item_id in trifecta_ids:
        raise HTTPException(
            status_code=409,
            detail="Cannot remove equipment selected in a trifecta preset",
        )
    svc.app_db.conn.execute("DELETE FROM equipment_library WHERE id = ?", (item_id,))
    svc.app_db.conn.commit()
    return {"status": "deleted"}


@router.get("/library/{item_id}/detail")
def get_library_detail(item_id: int):
    """Return full detail with cost breakdown for a library item."""
    svc = get_services()
    row = svc.app_db.conn.execute(
        "SELECT id, name, item_type, catalog_id, properties_json FROM equipment_library WHERE id = ?",
        (item_id,),
    ).fetchone()
    if row is None:
        raise HTTPException(
            status_code=404, detail=f"Equipment item {item_id} not found"
        )
    return _library_row_to_detail(row)


@router.post("/cost/calculate")
def calculate_cost(req: CalculateCostRequest):
    """Calculate cost breakdown for a weapon configuration without saving."""
    svc = get_services()

    if req.type == "healing":
        tool_e = _fetch_entity(svc.game_data, "medical_tools", req.catalog_id)
        cost = heal_cost_per_use(tool_e, req.weapon_markup / 100.0)
        return {"costBreakdown": [], "totalCostPerUse": cost}

    weapon_e = _fetch_entity(svc.game_data, "weapons", req.catalog_id)
    amp_e = None
    if req.amp_catalog_id:
        amp_e = _fetch_entity(svc.game_data, "weapon_amplifiers", req.amp_catalog_id)
    scope_e = None
    if req.scope_catalog_id:
        scope_e = _fetch_entity(
            svc.game_data, "weapon_vision_attachments", req.scope_catalog_id
        )
    absorber_e = None
    if req.absorber_catalog_id:
        absorber_e = _fetch_entity(svc.game_data, "absorbers", req.absorber_catalog_id)

    return cost_per_shot(
        weapon_e,
        amp=amp_e,
        scope=scope_e,
        absorber=absorber_e,
        damage_enhancers=max(0, req.damage_enhancers),
        weapon_markup=req.weapon_markup / 100.0,
        amp_markup=req.amp_markup / 100.0,
        scope_markup=req.scope_markup / 100.0,
        absorber_markup=req.absorber_markup / 100.0,
    )
