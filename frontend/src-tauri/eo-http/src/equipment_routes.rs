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
        let mut amp_e = None;
        if req.amp_catalog_id.is_some() {
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
        if req.scope_catalog_id.is_some() {
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
        if req.absorber_catalog_id.is_some() {
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
