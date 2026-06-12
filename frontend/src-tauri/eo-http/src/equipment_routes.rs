//! Natively-served equipment handlers, byte-faithful to
//! `backend/routers/equipment.py`: catalogue search, the library CRUD
//! (including the trifecta-reference delete guard), the expanded
//! detail, and the standalone cost calculation. Stored
//! `properties_json` bytes match the backend's bare `json.dumps`
//! exactly, so the database state stays comparable across arms.

use axum::body::Body;
use axum::http::{Response, StatusCode};
use eo_services::config_service::load_config_readonly;
use eo_services::cost_engine::{
    cost_per_shot, cost_per_shot_from_props, get_weapon_damage_profile, heal_cost_per_use,
    heal_reload_seconds, is_limited,
};
use eo_wire::normalizer::{round_half_even, to_python_json_dumps};
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::hydration::{
    detail, error_response, internal_error, plain_json_response, HydrationState,
};

/// The `type` query value to catalogue endpoint mapping.
fn type_endpoint(item_type: &str) -> Option<&'static str> {
    match item_type {
        "weapon" => Some("weapons"),
        "amp" => Some("weapon_amplifiers"),
        "healer" => Some("medical_tools"),
        "scope" => Some("weapon_vision_attachments"),
        "absorber" => Some("absorbers"),
        "consumable" => Some("stimulants"),
        _ => None,
    }
}

/// Per-field surrogate-taint flags for an equipment request. The
/// backend only crashes where a tainted value is CONSUMED (a catalogue
/// lookup whose miss echoes the surrogate into an unrenderable 404
/// detail, or a storage binding), so an unused tainted field must keep
/// flowing; the gates fire at exactly those consumption points, in the
/// backend's own evaluation order.
#[derive(Default)]
pub struct EquipmentTaint {
    pub catalog_id: bool,
    pub name: bool,
    pub amp_catalog_id: bool,
    pub scope_catalog_id: bool,
    pub absorber_catalog_id: bool,
}

/// The validated `AddWeaponRequest` / `CalculateCostRequest` payload
/// (the adapter layer reproduces the validation envelopes; handlers
/// receive the typed fields).
pub struct EquipmentRequest {
    pub item_type: String,
    pub catalog_id: Option<String>,
    pub name: Option<String>,
    pub amp_catalog_id: Option<String>,
    pub scope_catalog_id: Option<String>,
    pub absorber_catalog_id: Option<String>,
    pub weapon_markup: i64,
    pub amp_markup: i64,
    pub scope_markup: i64,
    pub absorber_markup: i64,
    pub damage_enhancers: i64,
    pub taint: EquipmentTaint,
}

/// Python `str.strip()`: `char::is_whitespace` plus the four
/// information-separator controls (FS/GS/RS/US) Python's `isspace`
/// includes and Rust's does not.
fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `entity.get(key) or 0.0` over a catalogue economy number.
fn eco_or_zero(entity: &Value, key: &str) -> f64 {
    entity
        .get("economy")
        .and_then(|eco| eco.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// Python truthiness over a JSON value.
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python truthiness over an optional stored entity (`if props.get(..)`).
fn entity_truthy(props: &Value, key: &str) -> bool {
    props.get(key).map(json_truthy).unwrap_or(false)
}

/// `props.get(key) or fallback`: the stored value when truthy, the
/// fallback otherwise.
fn py_or(props: &Value, key: &str, fallback: &Value) -> Value {
    match props.get(key) {
        Some(value) if json_truthy(value) => value.clone(),
        _ => fallback.clone(),
    }
}

/// Enrichment level from the configured components.
fn compute_enrichment(props: &Value) -> i64 {
    if entity_truthy(props, "amp_entity") {
        if entity_truthy(props, "scope_entity") || entity_truthy(props, "absorber_entity") {
            return 3;
        }
        return 2;
    }
    1
}

/// A catalogue search row in the `EquipmentSearchResult` shape.
fn entity_to_search_result(row: &Value) -> Value {
    let entity = &row["data"];
    json!({
        "catalogId": row["item_id"],
        "name": row["item_name"],
        "decay": eco_or_zero(entity, "decay"),
        "ammoBurn": eco_or_zero(entity, "ammo_burn") / 100.0,
        "isLimited": is_limited(entity),
    })
}

/// The weapon/amplifier/scope sub-object of an equipment detail.
fn weapon_search_result_from_entity(
    catalog_id: &Value,
    entity: &Value,
    markup_percent: &Value,
    damage_enhancers: i64,
) -> Value {
    json!({
        "catalogId": catalog_id,
        "name": entity["name"],
        "decay": eco_or_zero(entity, "decay"),
        "ammoBurn": eco_or_zero(entity, "ammo_burn") / 100.0,
        "markupPercent": markup_percent,
        "isLimited": is_limited(entity),
        "damageEnhancers": damage_enhancers,
    })
}

/// `max(0, int(props.get("damage_enhancers", 0) or 0))`.
fn stored_enhancers(props: &Value) -> i64 {
    props
        .get("damage_enhancers")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64
}

/// Convert an equipment_library row to the `Equipment` list shape.
/// None mirrors the backend's unhandled `KeyError` on a weapon row
/// missing its stored entity (the caller answers its 500).
fn library_row_to_equipment(id: i64, name: &str, item_type: &str, props: &Value) -> Option<Value> {
    if item_type == "weapon" {
        let weapon_e = props.get("weapon_entity").filter(|v| !v.is_null())?;
        let amp_e = props.get("amp_entity").filter(|v| !v.is_null());
        let enhancers = stored_enhancers(props).max(0);
        let cost_result = cost_per_shot_from_props(props, None);
        let damage_profile = get_weapon_damage_profile(weapon_e, amp_e, enhancers);
        let rounded = |key: &str| -> Value {
            damage_profile
                .as_ref()
                .and_then(|profile| profile.get(key))
                .and_then(Value::as_f64)
                .map(|v| json!(round_half_even(v, 2)))
                .unwrap_or(Value::Null)
        };
        return Some(json!({
            "id": id.to_string(),
            "name": name,
            "type": "weapon",
            "amplifierName": amp_e.map(|amp| amp["name"].clone()).unwrap_or(Value::Null),
            "costPerUse": cost_result["totalCostPerUse"],
            "damageMin": rounded("damageMin"),
            "damageMax": rounded("damageMax"),
            "reloadSeconds": Value::Null,
            "isLimited": is_limited(weapon_e),
            "enrichmentLevel": compute_enrichment(props),
        }));
    }

    if item_type == "consumable" {
        return Some(json!({
            "id": id.to_string(),
            "name": name,
            "type": "consumable",
            "amplifierName": null,
            "costPerUse": 0.0,
            "damageMin": null,
            "damageMax": null,
            "reloadSeconds": null,
            "isLimited": false,
            "enrichmentLevel": 1,
        }));
    }

    let tool_e = props.get("tool_entity").filter(|v| !v.is_null())?;
    let markup = props.get("markup").and_then(Value::as_f64).unwrap_or(100.0) / 100.0;
    Some(json!({
        "id": id.to_string(),
        "name": name,
        "type": "healing",
        "amplifierName": null,
        "costPerUse": heal_cost_per_use(tool_e, markup),
        "damageMin": null,
        "damageMax": null,
        "reloadSeconds": round_half_even(heal_reload_seconds(tool_e), 2),
        "isLimited": is_limited(tool_e),
        "enrichmentLevel": 1,
    }))
}

/// Convert an equipment_library row to the `EquipmentDetail` shape.
/// `catalog_id` is the row's own column (selected by the detail and
/// update queries). None mirrors the backend's unhandled `KeyError`
/// on a row missing its stored entity (the caller answers its 500).
fn library_row_to_detail(
    id: i64,
    name: &str,
    item_type: &str,
    catalog_id: &Value,
    props: &Value,
) -> Option<Value> {
    let item_id = id.to_string();

    if item_type == "weapon" {
        let weapon_e = props.get("weapon_entity").filter(|v| !v.is_null())?;
        let weapon_markup = props.get("weapon_markup").cloned().unwrap_or(json!(100));
        let amp_markup = props.get("amp_markup").cloned().unwrap_or(json!(100));
        let scope_markup = props.get("scope_markup").cloned().unwrap_or(json!(100));
        let absorber_markup = props.get("absorber_markup").cloned().unwrap_or(json!(100));
        let enhancers = stored_enhancers(props).max(0);
        let cost_result = cost_per_shot_from_props(props, None);

        let component = |entity_key: &str, id_key: &str, markup: &Value| -> Value {
            match props.get(entity_key).filter(|v| !v.is_null()) {
                Some(entity) => weapon_search_result_from_entity(
                    props.get(id_key).unwrap_or(&Value::Null),
                    entity,
                    markup,
                    0,
                ),
                None => Value::Null,
            }
        };

        let absorber_detail = match props.get("absorber_entity").filter(|v| !v.is_null()) {
            Some(absorber) => {
                let absorption_pct = absorber
                    .get("economy")
                    .and_then(|eco| eco.get("absorption"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    * 100.0;
                json!({
                    "catalogId": props.get("absorber_catalog_id").unwrap_or(&Value::Null),
                    "name": absorber["name"],
                    "decay": eco_or_zero(absorber, "decay"),
                    "ammoBurn": eco_or_zero(absorber, "ammo_burn") / 100.0,
                    "absorptionPercent": round_half_even(absorption_pct, 1),
                    "markupPercent": absorber_markup,
                    "isLimited": is_limited(absorber),
                })
            }
            None => Value::Null,
        };

        let weapon_catalog_id = py_or(props, "weapon_catalog_id", catalog_id);
        return Some(json!({
            "id": item_id,
            "weapon": {
                "catalogId": weapon_catalog_id,
                "name": weapon_e["name"],
                "decay": eco_or_zero(weapon_e, "decay"),
                "ammoBurn": eco_or_zero(weapon_e, "ammo_burn") / 100.0,
                "markupPercent": weapon_markup,
                "isLimited": is_limited(weapon_e),
                "damageEnhancers": enhancers,
            },
            "amplifier": component("amp_entity", "amp_catalog_id", &amp_markup),
            "scope": component("scope_entity", "scope_catalog_id", &scope_markup),
            "absorber": absorber_detail,
            "costBreakdown": cost_result["costBreakdown"],
            "totalCostPerUse": cost_result["totalCostPerUse"],
        }));
    }

    if item_type == "consumable" {
        return Some(json!({
            "id": item_id,
            "weapon": {
                "catalogId": catalog_id,
                "name": name,
                "decay": 0.0,
                "ammoBurn": 0.0,
                "markupPercent": 100,
                "isLimited": false,
                "damageEnhancers": 0,
            },
            "amplifier": null,
            "scope": null,
            "absorber": null,
            "costBreakdown": [],
            "totalCostPerUse": 0.0,
        }));
    }

    // Healing tool detail (simplified).
    let tool_e = props.get("tool_entity").filter(|v| !v.is_null())?;
    let markup_raw = props.get("markup").cloned().unwrap_or(json!(100));
    let markup_pct = markup_raw.as_f64().unwrap_or(100.0);
    let cost = heal_cost_per_use(tool_e, markup_pct / 100.0);
    let decay = eco_or_zero(tool_e, "decay");
    let mut breakdown = vec![json!({
        "component": "Decay",
        "costPec": decay,
        "markupMultiplier": markup_pct / 100.0,
        "effectiveCostPec": round_half_even(decay * markup_pct / 100.0, 4),
    })];
    let ammo_pec = eco_or_zero(tool_e, "ammo_burn") / 100.0;
    if ammo_pec > 0.0 {
        breakdown.push(json!({
            "component": "Ammo",
            "costPec": ammo_pec,
            "markupMultiplier": 1.0,
            "effectiveCostPec": ammo_pec,
        }));
    }
    Some(json!({
        "id": item_id,
        "weapon": {
            "catalogId": py_or(props, "tool_catalog_id", catalog_id),
            "name": tool_e["name"],
            "decay": decay,
            "ammoBurn": ammo_pec,
            "markupPercent": markup_raw,
            "isLimited": is_limited(tool_e),
            "damageEnhancers": 0,
        },
        "amplifier": null,
        "scope": null,
        "absorber": null,
        "costBreakdown": breakdown,
        "totalCostPerUse": cost,
    }))
}

/// The outcome of building stored props for an add/update request.
enum BuiltProps {
    Ok {
        name: String,
        stored_catalog_id: Option<String>,
        props: Value,
    },
    Reply(Response<Body>),
}

impl HydrationState {
    /// GET /api/equipment/search.
    pub async fn equipment_search(
        &self,
        q: &str,
        item_type: &str,
        _if_none_match: Option<&str>,
    ) -> Response<Body> {
        let Some(endpoint) = type_endpoint(item_type) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                &detail(&format!("Unknown type '{item_type}'")),
            );
        };
        if q.chars().count() < 2 {
            return plain_json_response(&json!([]));
        }
        let rows = self.game_data.search_entities(q, Some(endpoint), 50);
        let results: Vec<Value> = rows.iter().map(entity_to_search_result).collect();
        plain_json_response(&Value::Array(results))
    }

    /// GET /api/equipment/library.
    pub async fn equipment_library(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let rows = match sqlx::query(
            "SELECT id, name, item_type, properties_json FROM equipment_library ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await
        {
            Ok(rows) => rows,
            Err(_) => return internal_error(),
        };
        let mut results = Vec::new();
        for row in rows {
            let Some(shaped) = shape_library_row(&row) else {
                return internal_error();
            };
            results.push(shaped);
        }
        plain_json_response(&Value::Array(results))
    }

    /// POST /api/equipment/library.
    pub async fn equipment_add(&self, req: &EquipmentRequest) -> Response<Body> {
        let (name, stored_catalog_id, props) = match self.build_props(req) {
            BuiltProps::Ok {
                name,
                stored_catalog_id,
                props,
            } => (name, stored_catalog_id, props),
            BuiltProps::Reply(reply) => return reply,
        };
        let inserted = match sqlx::query(
            "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&name)
        .bind(&req.item_type)
        .bind(&stored_catalog_id)
        .bind(to_python_json_dumps(&props))
        .execute(self.pool())
        .await
        {
            Ok(result) => result.last_insert_rowid(),
            Err(_) => return internal_error(),
        };
        let row = match sqlx::query(
            "SELECT id, name, item_type, properties_json FROM equipment_library WHERE id = ?",
        )
        .bind(inserted)
        .fetch_one(self.pool())
        .await
        {
            Ok(row) => row,
            Err(_) => return internal_error(),
        };
        match shape_library_row(&row) {
            Some(shaped) => plain_ok(&shaped),
            None => internal_error(),
        }
    }

    /// PUT /api/equipment/library/{item_id}.
    pub async fn equipment_update(&self, item_id: i64, req: &EquipmentRequest) -> Response<Body> {
        let existing = match sqlx::query("SELECT id, item_type FROM equipment_library WHERE id = ?")
            .bind(item_id)
            .fetch_optional(self.pool())
            .await
        {
            Ok(row) => row,
            Err(_) => return internal_error(),
        };
        let Some(existing) = existing else {
            return error_response(
                StatusCode::NOT_FOUND,
                &detail(&format!("Equipment item {item_id} not found")),
            );
        };
        let existing_type: String = existing.get("item_type");
        if existing_type != req.item_type {
            return error_response(
                StatusCode::BAD_REQUEST,
                &detail("Cannot change equipment type"),
            );
        }
        let (name, stored_catalog_id, props) = match self.build_props(req) {
            BuiltProps::Ok {
                name,
                stored_catalog_id,
                props,
            } => (name, stored_catalog_id, props),
            BuiltProps::Reply(reply) => return reply,
        };
        if sqlx::query(
            "UPDATE equipment_library SET name = ?, catalog_id = ?, properties_json = ? WHERE id = ?",
        )
        .bind(&name)
        .bind(&stored_catalog_id)
        .bind(to_python_json_dumps(&props))
        .bind(item_id)
        .execute(self.pool())
        .await
        .is_err()
        {
            return internal_error();
        }
        let row = match sqlx::query(
            "SELECT id, name, item_type, catalog_id, properties_json \
             FROM equipment_library WHERE id = ?",
        )
        .bind(item_id)
        .fetch_one(self.pool())
        .await
        {
            Ok(row) => row,
            Err(_) => return internal_error(),
        };
        match shape_library_row(&row) {
            Some(shaped) => plain_ok(&shaped),
            None => internal_error(),
        }
    }

    /// DELETE /api/equipment/library/{item_id}.
    pub async fn equipment_delete(&self, item_id: i64) -> Response<Body> {
        let Ok(config) = load_config_readonly(&self.data_dir) else {
            return internal_error();
        };
        let referenced = config.trifecta_presets.iter().any(|preset| {
            [preset.small_weapon_id, preset.big_weapon_id, preset.heal_id].contains(&Some(item_id))
        });
        if referenced {
            return error_response(
                StatusCode::CONFLICT,
                &detail("Cannot remove equipment selected in a trifecta preset"),
            );
        }
        if sqlx::query("DELETE FROM equipment_library WHERE id = ?")
            .bind(item_id)
            .execute(self.pool())
            .await
            .is_err()
        {
            return internal_error();
        }
        plain_ok(&json!({"status": "deleted"}))
    }

    /// GET /api/equipment/library/{item_id}/detail.
    pub async fn equipment_detail(
        &self,
        item_id: i64,
        _if_none_match: Option<&str>,
    ) -> Response<Body> {
        let row = match sqlx::query(
            "SELECT id, name, item_type, catalog_id, properties_json \
             FROM equipment_library WHERE id = ?",
        )
        .bind(item_id)
        .fetch_optional(self.pool())
        .await
        {
            Ok(row) => row,
            Err(_) => return internal_error(),
        };
        let Some(row) = row else {
            return error_response(
                StatusCode::NOT_FOUND,
                &detail(&format!("Equipment item {item_id} not found")),
            );
        };
        let id: i64 = row.get("id");
        let name: String = row.get("name");
        let item_type: String = row.get("item_type");
        let catalog_id: Option<String> = row.get("catalog_id");
        let raw_props: String = row.get("properties_json");
        let Ok(props) = serde_json::from_str::<Value>(&raw_props) else {
            return internal_error();
        };
        let catalog_value = catalog_id.map(Value::String).unwrap_or(Value::Null);
        match library_row_to_detail(id, &name, &item_type, &catalog_value, &props) {
            Some(shaped) => plain_json_response(&shaped),
            None => internal_error(),
        }
    }

    /// POST /api/equipment/cost/calculate.
    pub async fn equipment_cost(&self, req: &EquipmentRequest) -> Response<Body> {
        if req.item_type == "healing" {
            let tool_e = match self.fetch_entity_gated(
                req.taint.catalog_id,
                "medical_tools",
                req.catalog_id.as_deref(),
            ) {
                Ok(entity) => entity,
                Err(reply) => return *reply,
            };
            let cost = heal_cost_per_use(&tool_e, req.weapon_markup as f64 / 100.0);
            return plain_ok(&json!({"costBreakdown": [], "totalCostPerUse": cost}));
        }
        let weapon_e = match self.fetch_entity_gated(
            req.taint.catalog_id,
            "weapons",
            req.catalog_id.as_deref(),
        ) {
            Ok(entity) => entity,
            Err(reply) => return *reply,
        };
        // The original gates each component on truthiness: an empty
        // string skips it exactly as absence does.
        let truthy = |id: &Option<String>| id.as_deref().is_some_and(|v| !v.is_empty());
        let mut amp_e = None;
        if truthy(&req.amp_catalog_id) {
            match self.fetch_entity_gated(
                req.taint.amp_catalog_id,
                "weapon_amplifiers",
                req.amp_catalog_id.as_deref(),
            ) {
                Ok(entity) => amp_e = Some(entity),
                Err(reply) => return *reply,
            }
        }
        let mut scope_e = None;
        if truthy(&req.scope_catalog_id) {
            match self.fetch_entity_gated(
                req.taint.scope_catalog_id,
                "weapon_vision_attachments",
                req.scope_catalog_id.as_deref(),
            ) {
                Ok(entity) => scope_e = Some(entity),
                Err(reply) => return *reply,
            }
        }
        let mut absorber_e = None;
        if truthy(&req.absorber_catalog_id) {
            match self.fetch_entity_gated(
                req.taint.absorber_catalog_id,
                "absorbers",
                req.absorber_catalog_id.as_deref(),
            ) {
                Ok(entity) => absorber_e = Some(entity),
                Err(reply) => return *reply,
            }
        }
        plain_ok(&cost_per_shot(
            &weapon_e,
            amp_e.as_ref(),
            scope_e.as_ref(),
            absorber_e.as_ref(),
            req.damage_enhancers.max(0),
            req.weapon_markup as f64 / 100.0,
            req.amp_markup as f64 / 100.0,
            req.scope_markup as f64 / 100.0,
            req.absorber_markup as f64 / 100.0,
        ))
    }

    /// `_fetch_entity`: catalogue lookup with the 404 envelope.
    fn fetch_entity(
        &self,
        endpoint: &str,
        item_id: Option<&str>,
    ) -> Result<Value, Box<Response<Body>>> {
        let id_value = Value::String(item_id.unwrap_or_default().to_string());
        match self.game_data.find_entity(endpoint, &id_value) {
            Some(entity) => Ok(entity.clone()),
            None => Err(Box::new(error_response(
                StatusCode::NOT_FOUND,
                &detail(&format!(
                    "Entity '{}' not found in catalogue endpoint '{endpoint}'.",
                    item_id.unwrap_or_default()
                )),
            ))),
        }
    }

    /// `_fetch_entity` at a taint-aware consumption point: a tainted
    /// id always misses the lookup, and the backend's 404 detail would
    /// echo the surrogate it cannot render, so the gate answers its
    /// 500 exactly where the fetch would run.
    fn fetch_entity_gated(
        &self,
        tainted: bool,
        endpoint: &str,
        item_id: Option<&str>,
    ) -> Result<Value, Box<Response<Body>>> {
        if tainted {
            return Err(Box::new(internal_error()));
        }
        self.fetch_entity(endpoint, item_id)
    }

    /// Build the stored props for an add/update request, reproducing
    /// the route-order validation (missing catalogue id 400s, entity
    /// 404s, the consumable identity rule) with the surrogate-taint
    /// gates at each consumption point.
    fn build_props(&self, req: &EquipmentRequest) -> BuiltProps {
        if req.item_type == "weapon" {
            let Some(catalog_id) = req.catalog_id.as_deref().filter(|id| !id.is_empty()) else {
                return BuiltProps::Reply(error_response(
                    StatusCode::BAD_REQUEST,
                    &detail("catalog_id required for weapon"),
                ));
            };
            let weapon_e =
                match self.fetch_entity_gated(req.taint.catalog_id, "weapons", Some(catalog_id)) {
                    Ok(entity) => entity,
                    Err(reply) => return BuiltProps::Reply(*reply),
                };
            let optional = |tainted: bool,
                            endpoint: &str,
                            id: Option<&str>|
             -> Result<Value, Box<Response<Body>>> {
                match id.filter(|v| !v.is_empty()) {
                    Some(id) => self.fetch_entity_gated(tainted, endpoint, Some(id)),
                    None => Ok(Value::Null),
                }
            };
            let amp_e = match optional(
                req.taint.amp_catalog_id,
                "weapon_amplifiers",
                req.amp_catalog_id.as_deref(),
            ) {
                Ok(value) => value,
                Err(reply) => return BuiltProps::Reply(*reply),
            };
            let scope_e = match optional(
                req.taint.scope_catalog_id,
                "weapon_vision_attachments",
                req.scope_catalog_id.as_deref(),
            ) {
                Ok(value) => value,
                Err(reply) => return BuiltProps::Reply(*reply),
            };
            let absorber_e = match optional(
                req.taint.absorber_catalog_id,
                "absorbers",
                req.absorber_catalog_id.as_deref(),
            ) {
                Ok(value) => value,
                Err(reply) => return BuiltProps::Reply(*reply),
            };
            let name = weapon_e["name"].as_str().unwrap_or_default().to_string();
            let mut props = Map::new();
            props.insert("weapon_entity".into(), weapon_e);
            props.insert("weapon_catalog_id".into(), json!(catalog_id));
            props.insert("amp_entity".into(), amp_e);
            props.insert("amp_catalog_id".into(), json!(req.amp_catalog_id));
            props.insert("scope_entity".into(), scope_e);
            props.insert("scope_catalog_id".into(), json!(req.scope_catalog_id));
            props.insert("absorber_entity".into(), absorber_e);
            props.insert("absorber_catalog_id".into(), json!(req.absorber_catalog_id));
            props.insert("weapon_markup".into(), json!(req.weapon_markup));
            props.insert("amp_markup".into(), json!(req.amp_markup));
            props.insert("scope_markup".into(), json!(req.scope_markup));
            props.insert("absorber_markup".into(), json!(req.absorber_markup));
            props.insert(
                "damage_enhancers".into(),
                json!(req.damage_enhancers.max(0)),
            );
            return BuiltProps::Ok {
                name,
                stored_catalog_id: Some(catalog_id.to_string()),
                props: Value::Object(props),
            };
        }

        if req.item_type == "healing" {
            let Some(catalog_id) = req.catalog_id.as_deref().filter(|id| !id.is_empty()) else {
                return BuiltProps::Reply(error_response(
                    StatusCode::BAD_REQUEST,
                    &detail("catalog_id required for healing"),
                ));
            };
            let tool_e = match self.fetch_entity_gated(
                req.taint.catalog_id,
                "medical_tools",
                Some(catalog_id),
            ) {
                Ok(entity) => entity,
                Err(reply) => return BuiltProps::Reply(*reply),
            };
            let name = tool_e["name"].as_str().unwrap_or_default().to_string();
            let mut props = Map::new();
            props.insert("tool_entity".into(), tool_e);
            props.insert("tool_catalog_id".into(), json!(catalog_id));
            props.insert("markup".into(), json!(req.weapon_markup));
            return BuiltProps::Ok {
                name,
                stored_catalog_id: Some(catalog_id.to_string()),
                props: Value::Object(props),
            };
        }

        // Consumable: catalogue pick or free-text name.
        if let Some(catalog_id) = req.catalog_id.as_deref().filter(|id| !id.is_empty()) {
            let entity =
                match self.fetch_entity_gated(req.taint.catalog_id, "stimulants", Some(catalog_id))
                {
                    Ok(entity) => entity,
                    Err(reply) => return BuiltProps::Reply(*reply),
                };
            let name = entity["name"].as_str().unwrap_or_default().to_string();
            let mut props = Map::new();
            props.insert("catalog_id".into(), json!(catalog_id));
            props.insert("entity".into(), entity);
            return BuiltProps::Ok {
                name,
                stored_catalog_id: Some(catalog_id.to_string()),
                props: Value::Object(props),
            };
        }
        if let Some(name) = req
            .name
            .as_deref()
            .map(py_strip)
            .filter(|name| !name.is_empty())
        {
            if req.taint.name {
                // The custom name reaches the storage binding, where
                // the backend crashes on the surrogate.
                return BuiltProps::Reply(internal_error());
            }
            let mut props = Map::new();
            props.insert("catalog_id".into(), Value::Null);
            props.insert("entity".into(), Value::Null);
            return BuiltProps::Ok {
                name: name.to_string(),
                stored_catalog_id: None,
                props: Value::Object(props),
            };
        }
        BuiltProps::Reply(error_response(
            StatusCode::BAD_REQUEST,
            &detail("Consumable requires either catalog_id (catalogue pick) or name (custom)"),
        ))
    }
}

/// Shape one library row (id, name, item_type, properties_json) to the
/// `Equipment` list form; None on unparseable stored JSON.
fn shape_library_row(row: &sqlx::sqlite::SqliteRow) -> Option<Value> {
    let id: i64 = row.get("id");
    let name: String = row.get("name");
    let item_type: String = row.get("item_type");
    let raw_props: String = row.get("properties_json");
    let props = serde_json::from_str::<Value>(&raw_props).ok()?;
    library_row_to_equipment(id, &name, &item_type, &props)
}

/// A plain 200 JSON response.
fn plain_ok(payload: &Value) -> Response<Body> {
    plain_json_response(payload)
}

// The shaping helpers are pure functions over stored rows and the
// catalogue; these pins hold their arithmetic and fallbacks
// hermetically (the cross-language battery holds the same surface
// against the running backend).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eo_services::clock::MockClock;
    use eo_services::db::Db;
    use eo_services::game_data_store::GameDataStore;
    use http_body_util::BodyExt;
    use serde_json::json;

    use super::*;

    #[test]
    fn the_type_map_and_truthiness_helpers_match_the_backend() {
        assert_eq!(type_endpoint("weapon"), Some("weapons"));
        assert_eq!(type_endpoint("amp"), Some("weapon_amplifiers"));
        assert_eq!(type_endpoint("healer"), Some("medical_tools"));
        assert_eq!(type_endpoint("scope"), Some("weapon_vision_attachments"));
        assert_eq!(type_endpoint("absorber"), Some("absorbers"));
        assert_eq!(type_endpoint("consumable"), Some("stimulants"));
        assert_eq!(type_endpoint("banana"), None);

        for (value, expected) in [
            (json!(null), false),
            (json!(false), false),
            (json!(0), false),
            (json!(0.0), false),
            (json!(""), false),
            (json!([]), false),
            (json!({}), false),
            (json!(true), true),
            (json!(1), true),
            (json!("x"), true),
            (json!([0]), true),
            (json!({"k": 0}), true),
        ] {
            assert_eq!(json_truthy(&value), expected, "{value}");
        }

        let props = json!({"amp_entity": {"name": "A"}, "empty": {}, "null": null});
        assert!(entity_truthy(&props, "amp_entity"));
        assert!(!entity_truthy(&props, "empty"));
        assert!(!entity_truthy(&props, "null"));
        assert!(!entity_truthy(&props, "missing"));

        assert_eq!(
            py_or(&props, "amp_entity", &json!("fb")),
            json!({"name": "A"})
        );
        assert_eq!(py_or(&props, "empty", &json!("fb")), json!("fb"));
        assert_eq!(py_or(&props, "missing", &json!("fb")), json!("fb"));

        assert_eq!(py_strip("  x  "), "x");
        assert_eq!(py_strip("\u{1c}\u{1d}x\u{1e}\u{1f}"), "x");
        assert_eq!(py_strip("\u{a0}x\u{85}"), "x");

        let entity = json!({"economy": {"decay": 0.5, "ammo_burn": 200}});
        assert_eq!(eco_or_zero(&entity, "decay"), 0.5);
        assert_eq!(eco_or_zero(&entity, "ammo_burn"), 200.0);
        assert_eq!(eco_or_zero(&entity, "missing"), 0.0);
        assert_eq!(eco_or_zero(&json!({}), "decay"), 0.0);

        assert_eq!(stored_enhancers(&json!({"damage_enhancers": 3})), 3);
        assert_eq!(stored_enhancers(&json!({"damage_enhancers": 2.9})), 2);
        assert_eq!(stored_enhancers(&json!({"damage_enhancers": null})), 0);
        assert_eq!(stored_enhancers(&json!({})), 0);

        assert_eq!(compute_enrichment(&json!({})), 1);
        assert_eq!(compute_enrichment(&json!({"amp_entity": {"a": 1}})), 2);
        assert_eq!(
            compute_enrichment(&json!({"amp_entity": {"a": 1}, "scope_entity": {"s": 1}})),
            3
        );
        assert_eq!(
            compute_enrichment(&json!({"amp_entity": {"a": 1}, "absorber_entity": {"b": 1}})),
            3
        );
        assert_eq!(compute_enrichment(&json!({"scope_entity": {"s": 1}})), 1);
    }

    #[test]
    fn search_rows_and_components_shape_with_pec_conversion() {
        let row = json!({
            "endpoint": "weapons", "item_id": "abc", "item_name": "Opal",
            "data": {"name": "Opal", "economy": {"decay": 0.02, "ammo_burn": 200}},
        });
        assert_eq!(
            entity_to_search_result(&row),
            json!({
                "catalogId": "abc", "name": "Opal", "decay": 0.02,
                "ammoBurn": 2.0, "isLimited": false,
            })
        );
        let limited = json!({
            "endpoint": "weapons", "item_id": "l", "item_name": "L (L)",
            "data": {"name": "L (L)", "economy": {}},
        });
        let shaped = entity_to_search_result(&limited);
        assert_eq!(shaped["decay"], json!(0.0));
        assert_eq!(shaped["ammoBurn"], json!(0.0));

        let entity = json!({"name": "Amp", "economy": {"decay": 1.5, "ammo_burn": 100}});
        assert_eq!(
            weapon_search_result_from_entity(&json!("id1"), &entity, &json!(105), 2),
            json!({
                "catalogId": "id1", "name": "Amp", "decay": 1.5, "ammoBurn": 1.0,
                "markupPercent": 105, "isLimited": false, "damageEnhancers": 2,
            })
        );
    }

    fn weapon_entity() -> Value {
        json!({
            "name": "Opal",
            "economy": {"decay": 0.1, "ammo_burn": 100},
            "damage": {"impact": 10.0},
        })
    }

    fn amp_entity() -> Value {
        json!({
            "name": "Amp",
            "economy": {"decay": 0.5, "ammo_burn": 50},
            "damage": {"impact": 4.0},
        })
    }

    fn tool_entity() -> Value {
        json!({
            "name": "Vivo",
            "economy": {"decay": 0.08, "ammo_burn": 20},
            "min_heal": 10.0, "max_heal": 40.0, "uses_per_minute": 30,
        })
    }

    #[test]
    fn library_rows_shape_each_type_and_refuse_missing_entities() {
        // Weapon with amp and enhancers: the damage profile and the
        // cost engine agree with the service's own figures.
        let props = json!({
            "weapon_entity": weapon_entity(),
            "weapon_catalog_id": "w1",
            "amp_entity": amp_entity(),
            "amp_catalog_id": "a1",
            "scope_entity": null,
            "scope_catalog_id": null,
            "absorber_entity": null,
            "absorber_catalog_id": null,
            "weapon_markup": 120, "amp_markup": 100, "scope_markup": 100,
            "absorber_markup": 100, "damage_enhancers": 2,
        });
        let shaped = library_row_to_equipment(7, "Opal", "weapon", &props).unwrap();
        let cost = cost_per_shot_from_props(&props, None);
        assert_eq!(shaped["id"], "7");
        assert_eq!(shaped["type"], "weapon");
        assert_eq!(shaped["amplifierName"], "Amp");
        assert_eq!(shaped["costPerUse"], cost["totalCostPerUse"]);
        // total damage: 10 * 1.2 (enhancers) + min(10/2, 4) = 16.
        assert_eq!(shaped["damageMin"], json!(8.0));
        assert_eq!(shaped["damageMax"], json!(16.0));
        assert_eq!(shaped["reloadSeconds"], json!(null));
        assert_eq!(shaped["enrichmentLevel"], json!(2));

        // Healing: markup-scaled cost and the reload rounding.
        let props = json!({
            "tool_entity": tool_entity(), "tool_catalog_id": "t1", "markup": 110,
        });
        let shaped = library_row_to_equipment(8, "Vivo", "healing", &props).unwrap();
        assert_eq!(shaped["type"], "healing");
        assert_eq!(
            shaped["costPerUse"],
            json!(heal_cost_per_use(&tool_entity(), 1.1))
        );
        assert_eq!(shaped["reloadSeconds"], json!(2.0));
        assert_eq!(shaped["isLimited"], json!(false));

        // Consumable: the fixed zero-cost shape.
        let shaped = library_row_to_equipment(9, "Bar", "consumable", &json!({})).unwrap();
        assert_eq!(
            shaped,
            json!({
                "id": "9", "name": "Bar", "type": "consumable",
                "amplifierName": null, "costPerUse": 0.0, "damageMin": null,
                "damageMax": null, "reloadSeconds": null, "isLimited": false,
                "enrichmentLevel": 1,
            })
        );

        // A row missing its stored entity refuses to shape (the
        // caller answers the unhandled-error 500).
        assert!(library_row_to_equipment(1, "X", "weapon", &json!({})).is_none());
        assert!(
            library_row_to_equipment(1, "X", "weapon", &json!({"weapon_entity": null})).is_none()
        );
        assert!(library_row_to_equipment(1, "X", "healing", &json!({})).is_none());
    }

    #[test]
    fn details_shape_components_breakdowns_and_fallbacks() {
        let props = json!({
            "weapon_entity": weapon_entity(),
            "weapon_catalog_id": "w1",
            "amp_entity": amp_entity(),
            "amp_catalog_id": "a1",
            "scope_entity": null,
            "scope_catalog_id": null,
            "absorber_entity": {"name": "Ab", "economy": {"decay": 0.2, "ammo_burn": 0, "absorption": 0.255}},
            "absorber_catalog_id": "ab1",
            "weapon_markup": 120, "amp_markup": 105, "scope_markup": 100,
            "absorber_markup": 100, "damage_enhancers": 1,
        });
        let detail = library_row_to_detail(3, "Opal", "weapon", &Value::Null, &props).unwrap();
        assert_eq!(detail["id"], "3");
        assert_eq!(detail["weapon"]["catalogId"], "w1");
        assert_eq!(detail["weapon"]["markupPercent"], json!(120));
        assert_eq!(detail["weapon"]["damageEnhancers"], json!(1));
        assert_eq!(detail["amplifier"]["name"], "Amp");
        assert_eq!(detail["amplifier"]["markupPercent"], json!(105));
        assert_eq!(detail["scope"], json!(null));
        assert_eq!(detail["absorber"]["absorptionPercent"], json!(25.5));
        let cost = cost_per_shot_from_props(&props, None);
        assert_eq!(detail["costBreakdown"], cost["costBreakdown"]);
        assert_eq!(detail["totalCostPerUse"], cost["totalCostPerUse"]);

        // The weapon catalogue id falls to the row column when the
        // stored one is falsy.
        let mut fallback_props = props.clone();
        fallback_props["weapon_catalog_id"] = json!("");
        let detail =
            library_row_to_detail(3, "Opal", "weapon", &json!("row-col"), &fallback_props).unwrap();
        assert_eq!(detail["weapon"]["catalogId"], "row-col");

        // Healing detail: the decay line plus the ammo line only when
        // the tool burns ammo.
        let props = json!({"tool_entity": tool_entity(), "tool_catalog_id": "t1", "markup": 110});
        let detail = library_row_to_detail(4, "Vivo", "healing", &Value::Null, &props).unwrap();
        assert_eq!(detail["weapon"]["catalogId"], "t1");
        assert_eq!(detail["weapon"]["markupPercent"], json!(110));
        assert_eq!(
            detail["costBreakdown"],
            json!([
                {"component": "Decay", "costPec": 0.08, "markupMultiplier": 1.1,
                 "effectiveCostPec": 0.088},
                {"component": "Ammo", "costPec": 0.2, "markupMultiplier": 1.0,
                 "effectiveCostPec": 0.2},
            ])
        );
        let mut no_ammo = tool_entity();
        no_ammo["economy"]["ammo_burn"] = json!(0);
        let props = json!({"tool_entity": no_ammo, "tool_catalog_id": "t1", "markup": 100});
        let detail = library_row_to_detail(4, "Vivo", "healing", &Value::Null, &props).unwrap();
        assert_eq!(detail["costBreakdown"].as_array().unwrap().len(), 1);

        // Consumable detail: the fixed shape over the row column.
        let detail =
            library_row_to_detail(5, "Bar", "consumable", &json!("c1"), &json!({})).unwrap();
        assert_eq!(detail["weapon"]["catalogId"], "c1");
        assert_eq!(detail["costBreakdown"], json!([]));
        assert_eq!(detail["totalCostPerUse"], json!(0.0));

        // Missing stored entities refuse to shape.
        assert!(library_row_to_detail(1, "X", "weapon", &Value::Null, &json!({})).is_none());
        assert!(library_row_to_detail(1, "X", "healing", &Value::Null, &json!({})).is_none());
    }

    fn request(item_type: &str) -> EquipmentRequest {
        EquipmentRequest {
            item_type: item_type.to_string(),
            catalog_id: None,
            name: None,
            amp_catalog_id: None,
            scope_catalog_id: None,
            absorber_catalog_id: None,
            weapon_markup: 100,
            amp_markup: 100,
            scope_markup: 100,
            absorber_markup: 100,
            damage_enhancers: 0,
            taint: EquipmentTaint::default(),
        }
    }

    async fn seeded_state(dir: &std::path::Path) -> HydrationState {
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        let write = |name: &str, value: Value| {
            std::fs::write(snapshot.join(name), serde_json::to_string(&value).unwrap()).unwrap();
        };
        write(
            "weapons.json",
            json!([{"id": "w1", "name": "Opal Mk1", "economy": {"decay": 0.1, "ammo_burn": 100}, "damage": {"impact": 10.0}}]),
        );
        write(
            "weapon_amplifiers.json",
            json!([{"id": "a1", "name": "Amp", "economy": {"decay": 0.5, "ammo_burn": 50}, "damage": {"impact": 4.0}}]),
        );
        write(
            "medical_tools.json",
            json!([{"id": "t1", "name": "Vivo", "economy": {"decay": 0.08, "ammo_burn": 20}, "min_heal": 10.0, "max_heal": 40.0, "uses_per_minute": 30}]),
        );
        write("stimulants.json", json!([{"id": "s1", "name": "Stim"}]));
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        HydrationState::new(
            db,
            Arc::new(GameDataStore::new(&snapshot).unwrap()),
            Arc::new(MockClock::new(None, 0.0)),
            dir.to_path_buf(),
        )
    }

    async fn parts(response: Response<Body>) -> (StatusCode, Vec<u8>) {
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, bytes)
    }

    #[tokio::test]
    async fn the_cost_route_walks_branches_gates_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        let state = seeded_state(dir.path()).await;

        // Weapon only, with markup and enhancers.
        let mut req = request("weapon");
        req.catalog_id = Some("w1".into());
        req.weapon_markup = 120;
        req.damage_enhancers = 2;
        let (status, body) = parts(state.equipment_cost(&req).await).await;
        assert_eq!(status, StatusCode::OK);
        let shaped: Value = serde_json::from_slice(&body).unwrap();
        // decay 0.1 * 1.2 (enhancers) = 0.12 PEC at a 1.2 multiplier.
        assert_eq!(shaped["costBreakdown"][0]["component"], "Weapon decay");
        assert_eq!(shaped["costBreakdown"][0]["markupMultiplier"], json!(1.2));

        // Weapon + amp; empty-string components are skipped as falsy.
        let mut req = request("weapon");
        req.catalog_id = Some("w1".into());
        req.amp_catalog_id = Some("a1".into());
        req.scope_catalog_id = Some(String::new());
        req.absorber_catalog_id = Some(String::new());
        let (status, body) = parts(state.equipment_cost(&req).await).await;
        assert_eq!(status, StatusCode::OK);
        let shaped: Value = serde_json::from_slice(&body).unwrap();
        let components: Vec<&str> = shaped["costBreakdown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["component"].as_str().unwrap())
            .collect();
        assert_eq!(
            components,
            ["Weapon decay", "Amp decay", "Ammo (weapon)", "Ammo (amp)"]
        );

        // Healing: the bare cost result.
        let mut req = request("healing");
        req.catalog_id = Some("t1".into());
        req.weapon_markup = 110;
        let (status, body) = parts(state.equipment_cost(&req).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            format!(
                "{{\"costBreakdown\":[],\"totalCostPerUse\":{}}}",
                heal_cost_per_use(&tool_entity(), 1.1)
            )
            .into_bytes()
        );

        // Unknown ids answer the catalogue 404 in fetch order.
        let mut req = request("weapon");
        req.catalog_id = Some("ghost".into());
        req.amp_catalog_id = Some("also-ghost".into());
        let (status, body) = parts(state.equipment_cost(&req).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            b"{\"detail\":\"Entity 'ghost' not found in catalogue endpoint 'weapons'.\"}"
        );

        // Tainted ids answer the 500 at their consumption point.
        let mut req = request("weapon");
        req.catalog_id = Some("w1".into());
        req.amp_catalog_id = Some("bad".into());
        req.taint.amp_catalog_id = true;
        let (status, _) = parts(state.equipment_cost(&req).await).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        let mut req = request("healing");
        req.catalog_id = Some("bad".into());
        req.taint.catalog_id = true;
        let (status, _) = parts(state.equipment_cost(&req).await).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn search_and_the_write_path_compose_over_the_catalogue() {
        let dir = tempfile::tempdir().unwrap();
        let state = seeded_state(dir.path()).await;

        // Search: substring, case-insensitive, the short-query gate,
        // the unknown-type 400.
        let (status, body) = parts(state.equipment_search("OPAL", "weapon", None).await).await;
        assert_eq!(status, StatusCode::OK);
        let rows: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows[0]["catalogId"], "w1");
        let (status, body) = parts(state.equipment_search("o", "weapon", None).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"[]");
        let (status, body) = parts(state.equipment_search("op", "banana", None).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, b"{\"detail\":\"Unknown type 'banana'\"}");

        // The add path stores the bare-json.dumps props and shapes the
        // response off the re-read row.
        let mut req = request("weapon");
        req.catalog_id = Some("w1".into());
        req.amp_catalog_id = Some("a1".into());
        req.weapon_markup = 120;
        let (status, body) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::OK);
        let shaped: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(shaped["id"], "1");
        assert_eq!(shaped["name"], "Opal Mk1");
        assert_eq!(shaped["amplifierName"], "Amp");
        assert_eq!(shaped["enrichmentLevel"], json!(2));
        let stored: String =
            sqlx::query_scalar("SELECT properties_json FROM equipment_library WHERE id = 1")
                .fetch_one(state.pool())
                .await
                .unwrap();
        assert!(stored.starts_with("{\"weapon_entity\": {\"id\": \"w1\""));
        assert!(stored.contains("\"weapon_markup\": 120"));

        // Update type gate, missing row, and a healing reconfigure.
        let mut req = request("healing");
        req.catalog_id = Some("t1".into());
        let (status, body) = parts(state.equipment_update(1, &req).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, b"{\"detail\":\"Cannot change equipment type\"}");
        let (status, body) = parts(state.equipment_update(9, &req).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, b"{\"detail\":\"Equipment item 9 not found\"}");
        let mut req = request("weapon");
        req.catalog_id = Some("w1".into());
        req.damage_enhancers = -4;
        let (status, body) = parts(state.equipment_update(1, &req).await).await;
        assert_eq!(status, StatusCode::OK);
        let shaped: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(shaped["amplifierName"], json!(null));
        let stored: String =
            sqlx::query_scalar("SELECT properties_json FROM equipment_library WHERE id = 1")
                .fetch_one(state.pool())
                .await
                .unwrap();
        assert!(
            stored.contains("\"damage_enhancers\": 0"),
            "negative enhancers clamp at zero: {stored}"
        );

        // build_props branch gates: the missing-id 400s, the unknown
        // catalogue 404, the consumable identity rule, the name strip
        // and its binding-taint 500.
        let req = request("weapon");
        let (status, body) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, b"{\"detail\":\"catalog_id required for weapon\"}");
        let req = request("healing");
        let (status, body) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, b"{\"detail\":\"catalog_id required for healing\"}");
        let mut req = request("consumable");
        req.catalog_id = Some("ghost".into());
        let (status, _) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let mut req = request("consumable");
        req.catalog_id = Some("s1".into());
        let (status, body) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::OK);
        let shaped: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(shaped["name"], "Stim");
        let mut req = request("consumable");
        req.name = Some("  Bar  ".into());
        let (status, body) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::OK);
        let shaped: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(shaped["name"], "Bar", "the custom name strips");
        let req = request("consumable");
        let (status, body) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            b"{\"detail\":\"Consumable requires either catalog_id (catalogue pick) or name (custom)\"}"
        );
        let mut req = request("consumable");
        req.name = Some("taint".into());
        req.taint.name = true;
        let (status, _) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        // The weapon's own tainted id gates before its fetch.
        let mut req = request("weapon");
        req.catalog_id = Some("bad".into());
        req.taint.catalog_id = true;
        let (status, _) = parts(state.equipment_add(&req).await).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

        // The delete honours the trifecta guard read through the
        // stored settings file.
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::to_string(&json!({
                "trifecta_presets": [
                    {"id": "main", "name": "Main", "small_weapon_id": 1,
                     "big_weapon_id": null, "heal_id": null},
                ],
                "active_trifecta_preset_id": "main",
            }))
            .unwrap(),
        )
        .unwrap();
        let (status, body) = parts(state.equipment_delete(1).await).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body,
            b"{\"detail\":\"Cannot remove equipment selected in a trifecta preset\"}"
        );
        let (status, body) = parts(state.equipment_delete(2).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"{\"status\":\"deleted\"}");
    }
}
