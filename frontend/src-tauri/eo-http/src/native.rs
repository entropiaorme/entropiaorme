//! Native route adapters: the HTTP-request face of the proven
//! [`hydration`](crate::hydration) handlers.
//!
//! Each adapter extracts its route's parameters through
//! [`extract`](crate::extract) (reproducing the backend's validation
//! envelopes), calls the corresponding handler, and returns its
//! response. Adapters proxy to the sidecar when the native services are
//! not composed (a substrate built without a database keeps serving
//! every route through the proxy arm, exactly as before composition).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, Response, StatusCode};
use axum::routing::MethodFilter;
use axum::Router;
use serde_json::{json, Map, Value};

use crate::body::{
    self, bool_or_default, int_or_default, internal_server_error, list_of_int_or_default,
    list_of_str_or_default, opt_f64, opt_int, opt_list_of_str, opt_str, str_or_default, BodyInt,
    BodyObject, Loc,
};
use crate::character_routes::ProspectQuery;
use crate::equipment_routes::EquipmentRequest;
use crate::extract::{
    decode_path_segment, float_or_default, literal_or_default, opt_float, parse_int_lax,
    require_bounded_int, require_float, require_str, LaxInt, QueryString, Validation,
};
use crate::hydration::{detail, error_response};
use crate::pyjson::PyValue;
use crate::{arm_routed, AppState, ArmRoutes};

/// `If-None-Match`, as the backend's middleware reads it (a non-UTF-8
/// value reads as absent).
fn if_none_match(req: &Request) -> Option<String> {
    req.headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// The framework-level 404 the backend serves when no route matches
/// (a percent-encoded slash inside a path parameter de-matches the
/// route there, because the backend decodes the path before matching).
fn router_not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{\"detail\":\"Not Found\"}"))
        .expect("static 404 builds")
}

macro_rules! simple_get {
    ($fn_name:ident, $handler:ident) => {
        async fn $fn_name(state: Arc<AppState>, req: Request) -> Response<Body> {
            let Some(hydration) = state.hydration() else {
                return state.proxy(req).await;
            };
            let inm = if_none_match(&req);
            hydration.$handler(inm.as_deref()).await
        }
    };
}

simple_get!(quests_list, list_quests);
simple_get!(quests_mobs, list_mob_names);
simple_get!(quests_analytics, quest_analytics);
simple_get!(playlists_list, list_playlists);
simple_get!(playlists_analytics, playlist_analytics);
simple_get!(codex_species, codex_species);
simple_get!(codex_meta_attributes, codex_meta_attributes);

/// GET /api/codex/species/{name}/ranks: the one path-parameter route of
/// this surface. The raw segment percent-decodes before the lookup; a
/// decoded slash reproduces the backend's route-level 404.
async fn codex_species_ranks(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let path = req.uri().path();
    let raw_name = path
        .strip_prefix("/api/codex/species/")
        .and_then(|rest| rest.strip_suffix("/ranks"))
        .unwrap_or_default();
    let name = decode_path_segment(raw_name);
    if name.contains('/') {
        return router_not_found();
    }
    let inm = if_none_match(&req);
    hydration.codex_species_ranks(&name, inm.as_deref()).await
}

/// GET /api/codex/recommend: the constrained-parameter route. The
/// extraction layer validates in route-signature order (species_name,
/// rank, profession, target) and answers violations with the backend's
/// 422 envelope.
async fn codex_recommend(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let mut validation = Validation::new();
    let species = require_str(&mut validation, &query, "species_name");
    let rank = require_bounded_int(&mut validation, &query, "rank", 1, 25);
    let profession = query.last("profession");
    let target = literal_or_default(
        &mut validation,
        &query,
        "target",
        &["profession", "hp"],
        "profession",
    );
    if !validation.is_ok() {
        return validation.into_response();
    }
    let (species, rank, target) = (
        species.expect("validated"),
        rank.expect("validated"),
        target.expect("validated"),
    );
    let inm = if_none_match(&req);
    hydration
        .codex_recommend(species, rank, profession, target, inm.as_deref())
        .await
}

/// Split a request into its content type and collected body bytes.
///
/// A transport-level read failure answers the backend's unhandled-error
/// 500: the reference never reaches the handler when the body cannot be
/// read, so nothing may be written from a partial payload (an empty
/// fallback would make optional-body routes proceed with defaults).
async fn body_parts(req: Request) -> Result<(Option<String>, Vec<u8>), Box<Response<Body>>> {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => Ok((content_type, bytes.to_vec())),
        Err(_) => Err(Box::new(internal_server_error())),
    }
}

/// The outcome of reading an integer path parameter the backend's way:
/// percent-decode, route-level 404 on a decoded slash, the validation
/// envelope on unparseable text, and the backend's unhandled-overflow
/// 500 beyond its storage range.
enum PathId {
    Value(i64),
    Reply(Response<Body>),
}

fn path_id(raw_segment: &str, name: &'static str) -> PathId {
    let decoded = decode_path_segment(raw_segment);
    if decoded.contains('/') {
        return PathId::Reply(router_not_found());
    }
    match parse_int_lax(&decoded) {
        Some(LaxInt::Value(value)) => PathId::Value(value),
        Some(_) => PathId::Reply(internal_server_error()),
        None => {
            let mut validation = Validation::new();
            validation.int_parsing("path", name, &decoded);
            PathId::Reply(validation.into_response())
        }
    }
}

/// The `{quest_id}` segment of `/api/quests/{quest_id}[/suffix]`.
fn quest_id_segment<'p>(path: &'p str, suffix: &str) -> &'p str {
    path.strip_prefix("/api/quests/")
        .and_then(|rest| rest.strip_suffix(suffix))
        .unwrap_or_default()
}

/// The `{playlist_id}` segment of `/api/quests/playlists/{playlist_id}`.
fn playlist_id_segment(path: &str) -> &str {
    path.strip_prefix("/api/quests/playlists/")
        .unwrap_or_default()
}

/// A finite float becomes a JSON number; the backend's non-finite
/// floats land as null end to end (probed: an `inf` reward reads back
/// null), and the conversion mirrors that.
fn f64_value(value: Option<f64>) -> Value {
    match value {
        Some(v) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        None => Value::Null,
    }
}

fn str_value(value: Option<String>) -> Value {
    value.map(Value::String).unwrap_or(Value::Null)
}

/// Extract a full QuestCreate model dump (every field present,
/// defaults applied) in declaration order. `Err` carries the reply
/// (422 or the deliberate overflow 500).
fn quest_create_dump(
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Value, Box<Response<Body>>> {
    let mut v = Validation::new();
    let Some(object) = body::read_object(content_type, bytes, &mut v) else {
        return Err(Box::new(v.into_response()));
    };
    let name = body::required_str(&mut v, &object, "name");
    let planet = str_or_default(&mut v, &object, "planet", "Calypso");
    let category = opt_str(&mut v, &object, "category");
    let waypoint = opt_str(&mut v, &object, "waypoint");
    let cooldown_hours = opt_f64(&mut v, &object, "cooldown_hours");
    let reward_ped = opt_f64(&mut v, &object, "reward_ped");
    let reward_is_skill = bool_or_default(&mut v, &object, "reward_is_skill", false);
    let markup = opt_f64(&mut v, &object, "expected_reward_markup_percent");
    let reward_description = opt_str(&mut v, &object, "reward_description");
    let notes = opt_str(&mut v, &object, "notes");
    let chain_name = opt_str(&mut v, &object, "chain_name");
    let chain_position = opt_int(&mut v, &object, "chain_position");
    let chain_total = opt_int(&mut v, &object, "chain_total");
    let mobs = list_of_str_or_default(&mut v, &object, "mobs");
    if !v.is_ok() {
        return Err(Box::new(v.into_response()));
    }
    if v.binding_taint() {
        // A surrogate-tainted string survived validation; the backend
        // crashes at storage binding before any commit on these
        // single-statement writes (multi-statement partial commits are
        // the register's recorded residual).
        return Err(Box::new(internal_server_error()));
    }
    let chain_position = int_value(chain_position.expect("validated"))?;
    let chain_total = int_value(chain_total.expect("validated"))?;
    Ok(json!({
        "name": name.expect("validated"),
        "planet": planet.expect("validated"),
        "category": str_value(category.expect("validated")),
        "waypoint": str_value(waypoint.expect("validated")),
        "cooldown_hours": f64_value(cooldown_hours.expect("validated")),
        "reward_ped": f64_value(reward_ped.expect("validated")),
        "reward_is_skill": reward_is_skill.expect("validated"),
        "expected_reward_markup_percent": f64_value(markup.expect("validated")),
        "reward_description": str_value(reward_description.expect("validated")),
        "notes": str_value(notes.expect("validated")),
        "chain_name": str_value(chain_name.expect("validated")),
        "chain_position": chain_position,
        "chain_total": chain_total,
        "mobs": mobs.expect("validated"),
    }))
}

/// An optional int that may carry the deliberate overflow reply.
fn int_value(value: Option<BodyInt>) -> Result<Value, Box<Response<Body>>> {
    match value {
        None => Ok(Value::Null),
        Some(BodyInt::Value(v)) => Ok(json!(v)),
        Some(BodyInt::Overflow) => Err(Box::new(internal_server_error())),
    }
}

/// Extract a QuestUpdate set-fields dump: only fields present in the
/// body land in the dict (present-null included), mirroring the
/// backend's exclude-unset model dump.
fn quest_update_dump(
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Value, Box<Response<Body>>> {
    let mut v = Validation::new();
    let Some(object) = body::read_object(content_type, bytes, &mut v) else {
        return Err(Box::new(v.into_response()));
    };
    // Fields validate in MODEL DECLARATION ORDER (multi-error
    // envelopes list issues in that order), present fields only.
    enum FieldKind {
        Str,
        Float,
        Bool,
        Int,
        StrList,
    }
    const FIELDS: [(&str, FieldKind); 14] = [
        ("name", FieldKind::Str),
        ("planet", FieldKind::Str),
        ("category", FieldKind::Str),
        ("waypoint", FieldKind::Str),
        ("cooldown_hours", FieldKind::Float),
        ("reward_ped", FieldKind::Float),
        ("reward_is_skill", FieldKind::Bool),
        ("expected_reward_markup_percent", FieldKind::Float),
        ("reward_description", FieldKind::Str),
        ("notes", FieldKind::Str),
        ("chain_name", FieldKind::Str),
        ("chain_position", FieldKind::Int),
        ("chain_total", FieldKind::Int),
        ("mobs", FieldKind::StrList),
    ];
    let mut dump = Map::new();
    let mut overflow = false;
    for (field, kind) in FIELDS {
        if object.get(field).is_none() {
            continue;
        }
        let name = field;
        match kind {
            FieldKind::Str => {
                if let Some(value) = opt_str(&mut v, &object, name) {
                    dump.insert(field.to_string(), str_value(value));
                }
            }
            FieldKind::Float => {
                if let Some(value) = opt_f64(&mut v, &object, name) {
                    dump.insert(field.to_string(), f64_value(value));
                }
            }
            FieldKind::Bool => {
                if let Some(value) = body::opt_bool(&mut v, &object, name) {
                    dump.insert(
                        field.to_string(),
                        value.map(Value::Bool).unwrap_or(Value::Null),
                    );
                }
            }
            FieldKind::Int => {
                if let Some(value) = opt_int(&mut v, &object, name) {
                    match int_value(value) {
                        Ok(rendered) => {
                            dump.insert(field.to_string(), rendered);
                        }
                        Err(_) => overflow = true,
                    }
                }
            }
            FieldKind::StrList => {
                if let Some(value) = opt_list_of_str(&mut v, &object, name) {
                    dump.insert(
                        field.to_string(),
                        value.map(|items| json!(items)).unwrap_or(Value::Null),
                    );
                }
            }
        }
    }
    if !v.is_ok() {
        return Err(Box::new(v.into_response()));
    }
    if v.binding_taint() {
        // A surrogate-tainted string survived validation; the backend
        // crashes at storage binding before any commit on these
        // single-statement writes (multi-statement partial commits are
        // the register's recorded residual).
        return Err(Box::new(internal_server_error()));
    }
    if overflow {
        return Err(Box::new(internal_server_error()));
    }
    Ok(Value::Object(dump))
}

/// PlaylistCreate: name, planet and estimated-minutes defaults,
/// quest_ids, and the optional nested items.
fn playlist_create_dump(
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Value, Box<Response<Body>>> {
    let mut v = Validation::new();
    let Some(object) = body::read_object(content_type, bytes, &mut v) else {
        return Err(Box::new(v.into_response()));
    };
    let name = body::required_str(&mut v, &object, "name");
    let planet = str_or_default(&mut v, &object, "planet", "Calypso");
    let estimated = int_or_default(&mut v, &object, "estimated_minutes", 30);
    let quest_ids = list_of_int_or_default(&mut v, &object, "quest_ids");
    let items = playlist_items(&mut v, &object);
    if !v.is_ok() {
        return Err(Box::new(v.into_response()));
    }
    if v.binding_taint() {
        // A surrogate-tainted string survived validation; the backend
        // crashes at storage binding before any commit on these
        // single-statement writes (multi-statement partial commits are
        // the register's recorded residual).
        return Err(Box::new(internal_server_error()));
    }
    let estimated = int_value(estimated.map(Some).expect("validated"))?;
    let quest_ids = int_list_value(quest_ids.expect("validated"))?;
    Ok(json!({
        "name": name.expect("validated"),
        "planet": planet.expect("validated"),
        "estimated_minutes": estimated,
        "quest_ids": quest_ids,
        "items": items.expect("validated"),
    }))
}

fn int_list_value(values: Vec<BodyInt>) -> Result<Value, Box<Response<Body>>> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        match value {
            BodyInt::Value(v) => out.push(json!(v)),
            BodyInt::Overflow => return Err(Box::new(internal_server_error())),
        }
    }
    Ok(Value::Array(out))
}

/// `items: list[PlaylistItemInput] | None`, validated item by item in
/// model order (quest_id, description, group_type).
fn playlist_items(v: &mut Validation, object: &BodyObject) -> Option<Value> {
    let value = match object.get("items") {
        None | Some(PyValue::Null) => return Some(Value::Null),
        Some(value) => value,
    };
    let PyValue::List(items) = value else {
        if let Some(echo) = body::echo_or_unrenderable(v, value) {
            body::body_issue(
                v,
                "list_type",
                &[Loc::Field("items")],
                "Input should be a valid list",
                &echo,
                None,
            );
        }
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let PyValue::Object(pairs) = item else {
            if let Some(echo) = body::echo_or_unrenderable(v, item) {
                body::body_issue(
                    v,
                    "model_attributes_type",
                    &[Loc::Field("items"), Loc::Index(index)],
                    "Input should be a valid dictionary or object to extract fields from",
                    &echo,
                    None,
                );
            }
            ok = false;
            continue;
        };
        // The item echo renders only when needed (a missing quest_id),
        // so a hazardous value in an otherwise-valid item never trips
        // a render check the reference would not perform.
        let quest_id = if pairs.iter().any(|(key, _)| key == "quest_id") {
            body::required_int_at(
                v,
                pairs,
                None,
                "quest_id",
                &[
                    Loc::Field("items"),
                    Loc::Index(index),
                    Loc::Field("quest_id"),
                ],
            )
        } else {
            if let Some(echo) = body::echo_or_unrenderable(v, item) {
                body::body_issue(
                    v,
                    "missing",
                    &[
                        Loc::Field("items"),
                        Loc::Index(index),
                        Loc::Field("quest_id"),
                    ],
                    "Field required",
                    &echo,
                    None,
                );
            }
            None
        };
        let description = pairs
            .iter()
            .find(|(key, _)| key == "description")
            .map(|(_, value)| value.clone())
            .unwrap_or(PyValue::Null);
        let description = match description {
            PyValue::Null => Value::Null,
            PyValue::Str(text) => Value::String(text),
            other => {
                if let Some(echo) = body::echo_or_unrenderable(v, &other) {
                    body::body_issue(
                        v,
                        "string_type",
                        &[
                            Loc::Field("items"),
                            Loc::Index(index),
                            Loc::Field("description"),
                        ],
                        "Input should be a valid string",
                        &echo,
                        None,
                    );
                }
                ok = false;
                Value::Null
            }
        };
        let group_type = pairs
            .iter()
            .find(|(key, _)| key == "group_type")
            .map(|(_, value)| value.clone())
            .unwrap_or(PyValue::Str("immediate".into()));
        let group_type = match group_type {
            PyValue::Str(text) => Value::String(text),
            other => {
                if let Some(echo) = body::echo_or_unrenderable(v, &other) {
                    body::body_issue(
                        v,
                        "string_type",
                        &[
                            Loc::Field("items"),
                            Loc::Index(index),
                            Loc::Field("group_type"),
                        ],
                        "Input should be a valid string",
                        &echo,
                        None,
                    );
                }
                ok = false;
                Value::Null
            }
        };
        match quest_id {
            Some(BodyInt::Value(id)) => out.push(json!({
                "quest_id": id,
                "description": description,
                "group_type": group_type,
            })),
            Some(BodyInt::Overflow) => {
                // The overflow reply is composed by the caller; mark
                // the item as the sentinel the storage layer rejects.
                out.push(json!({
                    "quest_id": Value::Null,
                    "__overflow": true,
                }));
            }
            None => ok = false,
        }
    }
    if !ok {
        return None;
    }
    Some(Value::Array(out))
}

/// PlaylistUpdate: only-present fields, exclude-unset shaped.
fn playlist_update_dump(
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Value, Box<Response<Body>>> {
    let mut v = Validation::new();
    let Some(object) = body::read_object(content_type, bytes, &mut v) else {
        return Err(Box::new(v.into_response()));
    };
    let mut dump = Map::new();
    let mut overflow = false;
    for field in ["name", "planet"] {
        if object.get(field).is_some() {
            if let Some(value) = opt_str(&mut v, &object, field) {
                dump.insert(field.to_string(), str_value(value));
            }
        }
    }
    if object.get("estimated_minutes").is_some() {
        if let Some(value) = opt_int(&mut v, &object, "estimated_minutes") {
            match int_value(value) {
                Ok(rendered) => {
                    dump.insert("estimated_minutes".into(), rendered);
                }
                Err(_) => overflow = true,
            }
        }
    }
    if object.get("quest_ids").is_some() {
        match object.get("quest_ids") {
            Some(PyValue::Null) => {
                dump.insert("quest_ids".into(), Value::Null);
            }
            _ => {
                if let Some(values) = list_of_int_or_default(&mut v, &object, "quest_ids") {
                    match int_list_value(values) {
                        Ok(rendered) => {
                            dump.insert("quest_ids".into(), rendered);
                        }
                        Err(_) => overflow = true,
                    }
                }
            }
        }
    }
    if object.get("items").is_some() {
        if let Some(items) = playlist_items(&mut v, &object) {
            dump.insert("items".into(), items);
        }
    }
    if !v.is_ok() {
        return Err(Box::new(v.into_response()));
    }
    if v.binding_taint() {
        // A surrogate-tainted string survived validation; the backend
        // crashes at storage binding before any commit on these
        // single-statement writes (multi-statement partial commits are
        // the register's recorded residual).
        return Err(Box::new(internal_server_error()));
    }
    if overflow || dump_has_overflow(&dump) {
        return Err(Box::new(internal_server_error()));
    }
    Ok(Value::Object(dump))
}

fn dump_has_overflow(dump: &Map<String, Value>) -> bool {
    dump.get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.get("__overflow").is_some()))
}

/// POST /api/quests
async fn quests_create(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    match quest_create_dump(content_type.as_deref(), &bytes) {
        Ok(dump) => hydration.create_quest(&dump).await,
        Err(reply) => *reply,
    }
}

/// GET /api/quests/{quest_id}
async fn quest_get(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let id = match path_id(quest_id_segment(req.uri().path(), ""), "quest_id") {
        PathId::Value(id) => id,
        PathId::Reply(reply) => return reply,
    };
    let inm = if_none_match(&req);
    hydration.get_quest_route(id, inm.as_deref()).await
}

/// PUT /api/quests/{quest_id}
async fn quest_update(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let id = match path_id(quest_id_segment(req.uri().path(), ""), "quest_id") {
        PathId::Value(id) => id,
        PathId::Reply(reply) => return reply,
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    match quest_update_dump(content_type.as_deref(), &bytes) {
        Ok(dump) => hydration.update_quest(id, &dump).await,
        Err(reply) => *reply,
    }
}

/// DELETE /api/quests/{quest_id}
async fn quest_delete(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    match path_id(quest_id_segment(req.uri().path(), ""), "quest_id") {
        PathId::Value(id) => hydration.delete_quest(id).await,
        PathId::Reply(reply) => reply,
    }
}

/// POST /api/quests/{quest_id}/start | complete | cancel
async fn quest_start(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    match path_id(quest_id_segment(req.uri().path(), "/start"), "quest_id") {
        PathId::Value(id) => hydration.start_quest(id).await,
        PathId::Reply(reply) => reply,
    }
}

async fn quest_complete(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    match path_id(quest_id_segment(req.uri().path(), "/complete"), "quest_id") {
        PathId::Value(id) => hydration.complete_quest(id).await,
        PathId::Reply(reply) => reply,
    }
}

async fn quest_cancel(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let id = match path_id(quest_id_segment(req.uri().path(), "/cancel"), "quest_id") {
        PathId::Value(id) => id,
        PathId::Reply(reply) => return reply,
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let undo_reward = match body::read_optional_object(content_type.as_deref(), &bytes, &mut v) {
        Some(None) => false,
        Some(Some(object)) => match bool_or_default(&mut v, &object, "undo_reward", false) {
            Some(value) => value,
            None => return v.into_response(),
        },
        None => return v.into_response(),
    };
    hydration.cancel_quest(id, undo_reward).await
}

/// POST /api/quests/playlists
async fn playlists_create(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    match playlist_create_dump(content_type.as_deref(), &bytes) {
        Ok(dump) => {
            if dump
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.get("__overflow").is_some()))
            {
                return internal_server_error();
            }
            hydration.create_playlist(&dump).await
        }
        Err(reply) => *reply,
    }
}

/// PUT /api/quests/playlists/{playlist_id}
async fn playlist_update(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let id = match path_id(playlist_id_segment(req.uri().path()), "playlist_id") {
        PathId::Value(id) => id,
        PathId::Reply(reply) => return reply,
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    match playlist_update_dump(content_type.as_deref(), &bytes) {
        Ok(dump) => hydration.update_playlist(id, &dump).await,
        Err(reply) => *reply,
    }
}

/// DELETE /api/quests/playlists/{playlist_id}
async fn playlist_delete(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    match path_id(playlist_id_segment(req.uri().path()), "playlist_id") {
        PathId::Value(id) => hydration.delete_playlist(id).await,
        PathId::Reply(reply) => reply,
    }
}

/// POST /api/codex/calibrate
async fn codex_calibrate(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::read_object(content_type.as_deref(), &bytes, &mut v) else {
        return v.into_response();
    };
    let species = body::required_str(&mut v, &object, "species_name");
    let rank = body::required_int_at(
        &mut v,
        object.pairs(),
        object.echo(),
        "rank",
        &[Loc::Field("rank")],
    );
    // Validation issues answer first (their 422s, or the render 500
    // when the envelope cannot serialise), as the backend orders it.
    if !v.is_ok() {
        return v.into_response();
    }
    let species = species.expect("validated");
    let rank = match rank.expect("validated") {
        BodyInt::Value(value) => value,
        // A beyond-i64 rank violates the service's 0-25 domain in the
        // backend before any storage is reached; any out-of-domain
        // value yields the same bound reply.
        BodyInt::Overflow => 26,
    };
    // The backend's service checks the rank domain BEFORE the species
    // string reaches an encoder, so the bound message wins over the
    // surrogate failure.
    if !(0..=25).contains(&rank) {
        return hydration.codex_value_error("Rank must be 0-25");
    }
    // A surrogate-tainted species then reaches the encode step, whose
    // failure is a ValueError the backend's router maps to a 400 with
    // the codec message (singular for one surrogate, a position range
    // for a consecutive run).
    if let Some(PyValue::TaintedStr {
        code,
        position,
        run,
        ..
    }) = object.get("species_name")
    {
        let detail = if *run > 1 {
            format!(
                "'utf-8' codec can't encode characters in position {}-{}: surrogates not allowed",
                position,
                position + run - 1
            )
        } else {
            format!(
                "'utf-8' codec can't encode character '\\u{code:04x}' in position {position}: \
                 surrogates not allowed"
            )
        };
        return hydration.codex_value_error(&detail);
    }
    hydration.codex_calibrate(&species, rank).await
}

// ── Settings adapters (reads only; the writes stay proxied until the
//    producer cutover, falling to each path's proxy fallback) ────────

simple_get!(settings_get, settings);
simple_get!(overlay_position_get, overlay_position);

// ── Character adapters ──────────────────────────────────────────────

simple_get!(character_calibration, character_calibration);
simple_get!(character_stats, character_stats);
simple_get!(character_skills, character_skills);
simple_get!(character_professions, character_professions);
simple_get!(character_prospect_options, character_prospect_options);
simple_get!(character_hp_optimizer, character_hp_optimizer);
simple_get!(character_codex, character_codex);

/// GET /api/character/prospect: the query family validates in
/// signature order (the envelope), then the handler's own 422 details
/// in code order.
async fn character_prospect(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let mut validation = Validation::new();
    let profession = require_str(&mut validation, &query, "profession");
    let target_level = require_float(&mut validation, &query, "target_level");
    let slice_type = query.last("slice_type").unwrap_or("global");
    let slice_value = query.last("slice_value");
    let markup_uplift = float_or_default(&mut validation, &query, "markup_uplift", 0.0);
    if !validation.is_ok() {
        return validation.into_response();
    }
    let (profession, target_level, markup_uplift) = (
        profession.expect("validated"),
        target_level.expect("validated"),
        markup_uplift.expect("validated"),
    );
    if target_level <= 0.0 {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &detail("target_level must be positive"),
        );
    }
    if markup_uplift < 0.0 {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &detail("markup_uplift must be zero or positive"),
        );
    }
    if !["global", "tag", "mob", "weapon"].contains(&slice_type) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &detail("slice_type must be global, tag, mob, or weapon"),
        );
    }
    if slice_type != "global" && slice_value.is_none_or(|v| v.is_empty()) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &detail("slice_value is required for non-global slices"),
        );
    }
    let prospect_query = ProspectQuery {
        profession: profession.to_string(),
        target_level,
        slice_type: slice_type.to_string(),
        slice_value: slice_value.map(str::to_string),
        markup_uplift,
    };
    let inm = if_none_match(&req);
    hydration
        .character_prospect(&prospect_query, inm.as_deref())
        .await
}

/// GET /api/character/profession-optimizer.
async fn character_profession_optimizer(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let mut validation = Validation::new();
    let profession = require_str(&mut validation, &query, "profession");
    if !validation.is_ok() {
        return validation.into_response();
    }
    let inm = if_none_match(&req);
    hydration
        .character_profession_optimizer(profession.expect("validated"), inm.as_deref())
        .await
}

/// GET /api/character/profession-path-optimizer: exactly one of
/// target_level / ped_budget after the envelope validation.
async fn character_path_optimizer(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let mut validation = Validation::new();
    let profession = require_str(&mut validation, &query, "profession");
    let target_level = opt_float(&mut validation, &query, "target_level");
    let ped_budget = opt_float(&mut validation, &query, "ped_budget");
    if !validation.is_ok() {
        return validation.into_response();
    }
    let (profession, target_level, ped_budget) = (
        profession.expect("validated"),
        target_level.expect("validated"),
        ped_budget.expect("validated"),
    );
    if target_level.is_none() == ped_budget.is_none() {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &detail("Exactly one of target_level or ped_budget must be provided"),
        );
    }
    let inm = if_none_match(&req);
    hydration
        .character_path_optimizer(profession, target_level, ped_budget, inm.as_deref())
        .await
}

// ── Equipment adapters ──────────────────────────────────────────────

simple_get!(equipment_library, equipment_library);

/// GET /api/equipment/search.
async fn equipment_search(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let q = query.last("q").unwrap_or("").to_string();
    let item_type = query.last("type").unwrap_or("weapon").to_string();
    let inm = if_none_match(&req);
    hydration
        .equipment_search(&q, &item_type, inm.as_deref())
        .await
}

/// The `{item_id}` segment of `/api/equipment/library/{item_id}[/suffix]`.
fn equipment_id_segment<'p>(path: &'p str, suffix: &str) -> &'p str {
    path.strip_prefix("/api/equipment/library/")
        .and_then(|rest| rest.strip_suffix(suffix))
        .unwrap_or_default()
}

/// Per-field surrogate-taint flags for an equipment request: the
/// backend only crashes when a tainted value actually reaches a
/// catalogue lookup's 404 detail or a storage binding, so an unused
/// tainted field must keep flowing (quests bind every field;
/// equipment does not).
#[derive(Default)]
struct EquipmentTaint {
    catalog_id: bool,
    name: bool,
    amp_catalog_id: bool,
    scope_catalog_id: bool,
    absorber_catalog_id: bool,
}

/// Extract an AddWeaponRequest in model declaration order.
fn add_weapon_request(
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<(EquipmentRequest, EquipmentTaint), Box<Response<Body>>> {
    let mut v = Validation::new();
    let Some(object) = body::read_object(content_type, bytes, &mut v) else {
        return Err(Box::new(v.into_response()));
    };
    let mut taint = EquipmentTaint::default();
    let tainted = |v: &Validation, before: bool| v.binding_taint() && !before;

    let item_type = body::literal_required(
        &mut v,
        &object,
        "type",
        &["weapon", "healing", "consumable"],
    );
    let before = v.binding_taint();
    let catalog_id = opt_str(&mut v, &object, "catalog_id");
    taint.catalog_id = tainted(&v, before);
    let before = v.binding_taint();
    let name = opt_str(&mut v, &object, "name");
    taint.name = tainted(&v, before);
    let before = v.binding_taint();
    let amp_catalog_id = opt_str(&mut v, &object, "amp_catalog_id");
    taint.amp_catalog_id = tainted(&v, before);
    let before = v.binding_taint();
    let scope_catalog_id = opt_str(&mut v, &object, "scope_catalog_id");
    taint.scope_catalog_id = tainted(&v, before);
    let before = v.binding_taint();
    let absorber_catalog_id = opt_str(&mut v, &object, "absorber_catalog_id");
    taint.absorber_catalog_id = tainted(&v, before);
    let weapon_markup = int_or_default(&mut v, &object, "weapon_markup", 100);
    let amp_markup = int_or_default(&mut v, &object, "amp_markup", 100);
    let scope_markup = int_or_default(&mut v, &object, "scope_markup", 100);
    let absorber_markup = int_or_default(&mut v, &object, "absorber_markup", 100);
    let damage_enhancers = int_or_default(&mut v, &object, "damage_enhancers", 0);
    if !v.is_ok() {
        return Err(Box::new(v.into_response()));
    }
    Ok((
        EquipmentRequest {
            item_type: item_type.expect("validated"),
            catalog_id: catalog_id.expect("validated"),
            name: name.expect("validated"),
            amp_catalog_id: amp_catalog_id.expect("validated"),
            scope_catalog_id: scope_catalog_id.expect("validated"),
            absorber_catalog_id: absorber_catalog_id.expect("validated"),
            weapon_markup: equipment_int(weapon_markup.expect("validated"))?,
            amp_markup: equipment_int(amp_markup.expect("validated"))?,
            scope_markup: equipment_int(scope_markup.expect("validated"))?,
            absorber_markup: equipment_int(absorber_markup.expect("validated"))?,
            damage_enhancers: equipment_int(damage_enhancers.expect("validated"))?,
        },
        taint,
    ))
}

/// Extract a CalculateCostRequest in model declaration order
/// (catalog_id first, then the two-value type literal).
fn calculate_cost_request(
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<(EquipmentRequest, EquipmentTaint), Box<Response<Body>>> {
    let mut v = Validation::new();
    let Some(object) = body::read_object(content_type, bytes, &mut v) else {
        return Err(Box::new(v.into_response()));
    };
    let mut taint = EquipmentTaint::default();
    let tainted = |v: &Validation, before: bool| v.binding_taint() && !before;

    let before = v.binding_taint();
    let catalog_id = body::required_str(&mut v, &object, "catalog_id");
    taint.catalog_id = tainted(&v, before);
    let item_type =
        body::literal_with_default(&mut v, &object, "type", &["weapon", "healing"], "weapon");
    let before = v.binding_taint();
    let amp_catalog_id = opt_str(&mut v, &object, "amp_catalog_id");
    taint.amp_catalog_id = tainted(&v, before);
    let before = v.binding_taint();
    let scope_catalog_id = opt_str(&mut v, &object, "scope_catalog_id");
    taint.scope_catalog_id = tainted(&v, before);
    let before = v.binding_taint();
    let absorber_catalog_id = opt_str(&mut v, &object, "absorber_catalog_id");
    taint.absorber_catalog_id = tainted(&v, before);
    let weapon_markup = int_or_default(&mut v, &object, "weapon_markup", 100);
    let amp_markup = int_or_default(&mut v, &object, "amp_markup", 100);
    let scope_markup = int_or_default(&mut v, &object, "scope_markup", 100);
    let absorber_markup = int_or_default(&mut v, &object, "absorber_markup", 100);
    let damage_enhancers = int_or_default(&mut v, &object, "damage_enhancers", 0);
    if !v.is_ok() {
        return Err(Box::new(v.into_response()));
    }
    Ok((
        EquipmentRequest {
            item_type: item_type.expect("validated"),
            catalog_id: Some(catalog_id.expect("validated")),
            name: None,
            amp_catalog_id: amp_catalog_id.expect("validated"),
            scope_catalog_id: scope_catalog_id.expect("validated"),
            absorber_catalog_id: absorber_catalog_id.expect("validated"),
            weapon_markup: equipment_int(weapon_markup.expect("validated"))?,
            amp_markup: equipment_int(amp_markup.expect("validated"))?,
            scope_markup: equipment_int(scope_markup.expect("validated"))?,
            absorber_markup: equipment_int(absorber_markup.expect("validated"))?,
            damage_enhancers: equipment_int(damage_enhancers.expect("validated"))?,
        },
        taint,
    ))
}

/// An equipment int field. The backend carries arbitrary-precision
/// integers through these (they flow into a JSON text column, never a
/// direct parameter binding); the native side answers the deliberate
/// 500 beyond i64 instead. See the divergence register.
fn equipment_int(value: BodyInt) -> Result<i64, Box<Response<Body>>> {
    match value {
        BodyInt::Value(v) => Ok(v),
        BodyInt::Overflow => Err(Box::new(internal_server_error())),
    }
}

/// POST /api/equipment/library.
async fn equipment_add(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    match add_weapon_request(content_type.as_deref(), &bytes) {
        Ok((request, taint)) => match equipment_taint_reply(&request, &taint) {
            Some(reply) => reply,
            None => hydration.equipment_add(&request).await,
        },
        Err(reply) => *reply,
    }
}

/// PUT /api/equipment/library/{item_id}.
async fn equipment_update(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let id = match path_id(equipment_id_segment(req.uri().path(), ""), "item_id") {
        PathId::Value(id) => id,
        PathId::Reply(reply) => return reply,
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    match add_weapon_request(content_type.as_deref(), &bytes) {
        Ok((request, taint)) => match equipment_taint_reply(&request, &taint) {
            Some(reply) => reply,
            None => hydration.equipment_update(id, &request).await,
        },
        Err(reply) => *reply,
    }
}

/// DELETE /api/equipment/library/{item_id}.
async fn equipment_delete(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let id = match path_id(equipment_id_segment(req.uri().path(), ""), "item_id") {
        PathId::Value(id) => id,
        PathId::Reply(reply) => return reply,
    };
    hydration.equipment_delete(id).await
}

/// GET /api/equipment/library/{item_id}/detail.
async fn equipment_detail(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let id = match path_id(equipment_id_segment(req.uri().path(), "/detail"), "item_id") {
        PathId::Value(id) => id,
        PathId::Reply(reply) => return reply,
    };
    let inm = if_none_match(&req);
    hydration.equipment_detail(id, inm.as_deref()).await
}

/// POST /api/equipment/cost/calculate.
async fn equipment_cost(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let (content_type, bytes) = match body_parts(req).await {
        Ok(parts) => parts,
        Err(reply) => return *reply,
    };
    match calculate_cost_request(content_type.as_deref(), &bytes) {
        Ok((request, taint)) => match equipment_taint_reply(&request, &taint) {
            Some(reply) => reply,
            None => hydration.equipment_cost(&request).await,
        },
        Err(reply) => *reply,
    }
}

/// Where a tainted field reaches the backend's failure surface for the
/// REQUESTED branch, answer its 500; unused tainted fields keep
/// flowing exactly as the backend lets them. A tainted catalogue id
/// always misses the lookup and the resulting 404 detail echoes the
/// surrogate, which the backend cannot render; a tainted custom name
/// reaches the storage binding.
fn equipment_taint_reply(
    request: &EquipmentRequest,
    taint: &EquipmentTaint,
) -> Option<Response<Body>> {
    let used_catalog = request
        .catalog_id
        .as_deref()
        .is_some_and(|id| !id.is_empty());
    match request.item_type.as_str() {
        "weapon" => {
            if used_catalog && taint.catalog_id {
                return Some(internal_server_error());
            }
            for (id, flag) in [
                (&request.amp_catalog_id, taint.amp_catalog_id),
                (&request.scope_catalog_id, taint.scope_catalog_id),
                (&request.absorber_catalog_id, taint.absorber_catalog_id),
            ] {
                if id.as_deref().is_some_and(|v| !v.is_empty()) && flag {
                    return Some(internal_server_error());
                }
            }
            None
        }
        "healing" => (used_catalog && taint.catalog_id).then(internal_server_error),
        _ => {
            if used_catalog {
                return (taint.catalog_id).then(internal_server_error);
            }
            let used_name = request
                .name
                .as_deref()
                .map(str::trim)
                .is_some_and(|name| !name.is_empty());
            (used_name && taint.name).then(internal_server_error)
        }
    }
}

/// Register the natively-served quests/codex hydration GETs; one
/// `arm_routed` line per route, the registration order mirroring the
/// takeover record. Each line is individually revertable, and the arm
/// override covers every one at runtime.
pub(crate) fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/api/quests",
            ArmRoutes::at("/api/quests")
                .on(MethodFilter::GET, quests_list)
                .on(MethodFilter::POST, quests_create)
                .into_method_router(),
        )
        .route(
            "/api/quests/mobs",
            arm_routed(MethodFilter::GET, "/api/quests/mobs", quests_mobs),
        )
        .route(
            "/api/quests/analytics",
            arm_routed(MethodFilter::GET, "/api/quests/analytics", quests_analytics),
        )
        .route(
            "/api/quests/playlists",
            ArmRoutes::at("/api/quests/playlists")
                .on(MethodFilter::GET, playlists_list)
                .on(MethodFilter::POST, playlists_create)
                .into_method_router(),
        )
        .route(
            "/api/quests/playlists/analytics",
            arm_routed(
                MethodFilter::GET,
                "/api/quests/playlists/analytics",
                playlists_analytics,
            ),
        )
        .route(
            "/api/quests/playlists/{playlist_id}",
            ArmRoutes::at("/api/quests/playlists/{playlist_id}")
                .on(MethodFilter::PUT, playlist_update)
                .on(MethodFilter::DELETE, playlist_delete)
                .into_method_router(),
        )
        .route(
            "/api/quests/{quest_id}",
            ArmRoutes::at("/api/quests/{quest_id}")
                .on(MethodFilter::GET, quest_get)
                .on(MethodFilter::PUT, quest_update)
                .on(MethodFilter::DELETE, quest_delete)
                .into_method_router(),
        )
        .route(
            "/api/quests/{quest_id}/start",
            arm_routed(
                MethodFilter::POST,
                "/api/quests/{quest_id}/start",
                quest_start,
            ),
        )
        .route(
            "/api/quests/{quest_id}/complete",
            arm_routed(
                MethodFilter::POST,
                "/api/quests/{quest_id}/complete",
                quest_complete,
            ),
        )
        .route(
            "/api/quests/{quest_id}/cancel",
            arm_routed(
                MethodFilter::POST,
                "/api/quests/{quest_id}/cancel",
                quest_cancel,
            ),
        )
        .route(
            "/api/codex/species",
            arm_routed(MethodFilter::GET, "/api/codex/species", codex_species),
        )
        .route(
            "/api/codex/species/{name}/ranks",
            arm_routed(
                MethodFilter::GET,
                "/api/codex/species/{name}/ranks",
                codex_species_ranks,
            ),
        )
        .route(
            "/api/codex/recommend",
            arm_routed(MethodFilter::GET, "/api/codex/recommend", codex_recommend),
        )
        .route(
            "/api/codex/calibrate",
            arm_routed(MethodFilter::POST, "/api/codex/calibrate", codex_calibrate),
        )
        .route(
            "/api/codex/meta/attributes",
            arm_routed(
                MethodFilter::GET,
                "/api/codex/meta/attributes",
                codex_meta_attributes,
            ),
        )
        .route(
            "/api/settings",
            arm_routed(MethodFilter::GET, "/api/settings", settings_get),
        )
        .route(
            "/api/settings/overlay-position",
            arm_routed(
                MethodFilter::GET,
                "/api/settings/overlay-position",
                overlay_position_get,
            ),
        )
        .route(
            "/api/character/calibration",
            arm_routed(
                MethodFilter::GET,
                "/api/character/calibration",
                character_calibration,
            ),
        )
        .route(
            "/api/character/stats",
            arm_routed(MethodFilter::GET, "/api/character/stats", character_stats),
        )
        .route(
            "/api/character/skills",
            arm_routed(MethodFilter::GET, "/api/character/skills", character_skills),
        )
        .route(
            "/api/character/professions",
            arm_routed(
                MethodFilter::GET,
                "/api/character/professions",
                character_professions,
            ),
        )
        .route(
            "/api/character/prospect-options",
            arm_routed(
                MethodFilter::GET,
                "/api/character/prospect-options",
                character_prospect_options,
            ),
        )
        .route(
            "/api/character/prospect",
            arm_routed(
                MethodFilter::GET,
                "/api/character/prospect",
                character_prospect,
            ),
        )
        .route(
            "/api/character/profession-optimizer",
            arm_routed(
                MethodFilter::GET,
                "/api/character/profession-optimizer",
                character_profession_optimizer,
            ),
        )
        .route(
            "/api/character/profession-path-optimizer",
            arm_routed(
                MethodFilter::GET,
                "/api/character/profession-path-optimizer",
                character_path_optimizer,
            ),
        )
        .route(
            "/api/character/hp-optimizer",
            arm_routed(
                MethodFilter::GET,
                "/api/character/hp-optimizer",
                character_hp_optimizer,
            ),
        )
        .route(
            "/api/character/codex",
            arm_routed(MethodFilter::GET, "/api/character/codex", character_codex),
        )
        .route(
            "/api/equipment/search",
            arm_routed(MethodFilter::GET, "/api/equipment/search", equipment_search),
        )
        .route(
            "/api/equipment/library",
            ArmRoutes::at("/api/equipment/library")
                .on(MethodFilter::GET, equipment_library)
                .on(MethodFilter::POST, equipment_add)
                .into_method_router(),
        )
        .route(
            "/api/equipment/library/{item_id}",
            ArmRoutes::at("/api/equipment/library/{item_id}")
                .on(MethodFilter::PUT, equipment_update)
                .on(MethodFilter::DELETE, equipment_delete)
                .into_method_router(),
        )
        .route(
            "/api/equipment/library/{item_id}/detail",
            arm_routed(
                MethodFilter::GET,
                "/api/equipment/library/{item_id}/detail",
                equipment_detail,
            ),
        )
        .route(
            "/api/equipment/cost/calculate",
            arm_routed(
                MethodFilter::POST,
                "/api/equipment/cost/calculate",
                equipment_cost,
            ),
        )
}
