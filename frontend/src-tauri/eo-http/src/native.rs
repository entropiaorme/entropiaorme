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
use crate::extract::{
    decode_path_segment, literal_or_default, parse_int_lax, require_bounded_int, require_str,
    LaxInt, QueryString, Validation,
};
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
async fn body_parts(req: Request) -> (Option<String>, Vec<u8>) {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map(|b| b.to_vec())
        .unwrap_or_default();
    (content_type, bytes)
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
    let mut dump = Map::new();
    let mut overflow = false;
    for field in [
        "name",
        "planet",
        "category",
        "waypoint",
        "reward_description",
        "notes",
        "chain_name",
    ] {
        if object.get(field).is_some() {
            if let Some(value) = opt_str(&mut v, &object, leak_field(field)) {
                dump.insert(field.to_string(), str_value(value));
            }
        }
    }
    for field in [
        "cooldown_hours",
        "reward_ped",
        "expected_reward_markup_percent",
    ] {
        if object.get(field).is_some() {
            if let Some(value) = opt_f64(&mut v, &object, leak_field(field)) {
                dump.insert(field.to_string(), f64_value(value));
            }
        }
    }
    if object.get("reward_is_skill").is_some() {
        if let Some(value) = body::opt_bool(&mut v, &object, "reward_is_skill") {
            dump.insert(
                "reward_is_skill".into(),
                value.map(Value::Bool).unwrap_or(Value::Null),
            );
        }
    }
    for field in ["chain_position", "chain_total"] {
        if object.get(field).is_some() {
            if let Some(value) = opt_int(&mut v, &object, leak_field(field)) {
                match int_value(value) {
                    Ok(rendered) => {
                        dump.insert(field.to_string(), rendered);
                    }
                    Err(_) => overflow = true,
                }
            }
        }
    }
    if object.get("mobs").is_some() {
        if let Some(value) = opt_list_of_str(&mut v, &object, "mobs") {
            dump.insert(
                "mobs".into(),
                value.map(|items| json!(items)).unwrap_or(Value::Null),
            );
        }
    }
    if !v.is_ok() {
        return Err(Box::new(v.into_response()));
    }
    if overflow {
        return Err(Box::new(internal_server_error()));
    }
    Ok(Value::Object(dump))
}

/// The update extractors take `&'static str` names; the fixed field
/// list above is static by construction.
fn leak_field(field: &str) -> &'static str {
    match field {
        "name" => "name",
        "planet" => "planet",
        "category" => "category",
        "waypoint" => "waypoint",
        "reward_description" => "reward_description",
        "notes" => "notes",
        "chain_name" => "chain_name",
        "cooldown_hours" => "cooldown_hours",
        "reward_ped" => "reward_ped",
        "expected_reward_markup_percent" => "expected_reward_markup_percent",
        "chain_position" => "chain_position",
        "chain_total" => "chain_total",
        _ => unreachable!("fixed field list"),
    }
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
        body::body_issue(
            v,
            "list_type",
            &[Loc::Field("items")],
            "Input should be a valid list",
            &value.to_echo_json(),
            None,
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let PyValue::Object(pairs) = item else {
            body::body_issue(
                v,
                "model_attributes_type",
                &[Loc::Field("items"), Loc::Index(index)],
                "Input should be a valid dictionary or object to extract fields from",
                &item.to_echo_json(),
                None,
            );
            ok = false;
            continue;
        };
        let item_echo = item.to_echo_json();
        let quest_id = body::required_int_at(
            v,
            pairs,
            &item_echo,
            "quest_id",
            &[
                Loc::Field("items"),
                Loc::Index(index),
                Loc::Field("quest_id"),
            ],
        );
        let description = pairs
            .iter()
            .find(|(key, _)| key == "description")
            .map(|(_, value)| value.clone())
            .unwrap_or(PyValue::Null);
        let description = match description {
            PyValue::Null => Value::Null,
            PyValue::Str(text) => Value::String(text),
            other => {
                body::body_issue(
                    v,
                    "string_type",
                    &[
                        Loc::Field("items"),
                        Loc::Index(index),
                        Loc::Field("description"),
                    ],
                    "Input should be a valid string",
                    &other.to_echo_json(),
                    None,
                );
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
                body::body_issue(
                    v,
                    "string_type",
                    &[
                        Loc::Field("items"),
                        Loc::Index(index),
                        Loc::Field("group_type"),
                    ],
                    "Input should be a valid string",
                    &other.to_echo_json(),
                    None,
                );
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
            if let Some(value) = opt_str(&mut v, &object, leak_playlist_field(field)) {
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
    if overflow || dump_has_overflow(&dump) {
        return Err(Box::new(internal_server_error()));
    }
    Ok(Value::Object(dump))
}

fn leak_playlist_field(field: &str) -> &'static str {
    match field {
        "name" => "name",
        "planet" => "planet",
        _ => unreachable!("fixed field list"),
    }
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
    let (content_type, bytes) = body_parts(req).await;
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
    let (content_type, bytes) = body_parts(req).await;
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
    let (content_type, bytes) = body_parts(req).await;
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
    let (content_type, bytes) = body_parts(req).await;
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
    let (content_type, bytes) = body_parts(req).await;
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
    let (content_type, bytes) = body_parts(req).await;
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
    hydration.codex_calibrate(&species, rank).await
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
}
