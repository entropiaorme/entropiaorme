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
use crate::equipment_routes::{EquipmentRequest, EquipmentTaint};
use crate::extract::{
    decode_path_segment, float_or_default, literal_or_default, opt_float, opt_query_int,
    parse_int_lax, query_int_or_default, require_bounded_int, require_float, require_query_bool,
    require_str, LaxInt, QueryString, Validation,
};
use crate::hydration::{detail, error_response, internal_error};
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

/// A path parameter on a BODY-CARRYING route. The backend validates
/// path and body parameters together: an unparsable id records its
/// issue and the body still validates, one aggregated envelope with
/// the path issue first. A decoded slash stays the route-level 404
/// (matching precedes validation), and a beyond-i64 id VALIDATES
/// (Python's int is unbounded) and crashes at the handler's first
/// parameter binding, the deliberate 500, after the envelope resolves.
enum PathParam {
    NotFound,
    Invalid,
    Value(i64),
    Overflow,
}

fn path_param(v: &mut Validation, raw_segment: &str, name: &'static str) -> PathParam {
    let decoded = decode_path_segment(raw_segment);
    if decoded.contains('/') {
        return PathParam::NotFound;
    }
    match parse_int_lax(&decoded) {
        Some(LaxInt::Value(value)) => PathParam::Value(value),
        Some(_) => PathParam::Overflow,
        None => {
            v.int_parsing("path", name, &decoded);
            PathParam::Invalid
        }
    }
}

/// A dump builder's outcome once extraction has run. Any issues live
/// in the SHARED validation (path issues included), which the caller
/// renders first; the deferred 500s (surrogate taint at a binding, a
/// beyond-i64 integer) apply only to an otherwise-clean request.
enum Built<T> {
    Invalid,
    Deferred500,
    Value(T),
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
fn quest_create_dump(v: &mut Validation, object: &BodyObject) -> Built<Value> {
    let name = body::required_str(v, object, "name");
    let planet = str_or_default(v, object, "planet", "Calypso");
    let category = opt_str(v, object, "category");
    let waypoint = opt_str(v, object, "waypoint");
    let cooldown_hours = opt_f64(v, object, "cooldown_hours");
    let reward_ped = opt_f64(v, object, "reward_ped");
    let reward_is_skill = bool_or_default(v, object, "reward_is_skill", false);
    let markup = opt_f64(v, object, "expected_reward_markup_percent");
    let reward_description = opt_str(v, object, "reward_description");
    let notes = opt_str(v, object, "notes");
    let chain_name = opt_str(v, object, "chain_name");
    let chain_position = opt_int(v, object, "chain_position");
    let chain_total = opt_int(v, object, "chain_total");
    let mobs = list_of_str_or_default(v, object, "mobs");
    if !v.is_ok() {
        return Built::Invalid;
    }
    if v.binding_taint() {
        // A surrogate-tainted string survived validation; the backend
        // crashes at storage binding before any commit on these
        // single-statement writes (multi-statement partial commits are
        // the register's recorded residual).
        return Built::Deferred500;
    }
    let Some(chain_position) = int_value(chain_position.expect("validated")) else {
        return Built::Deferred500;
    };
    let Some(chain_total) = int_value(chain_total.expect("validated")) else {
        return Built::Deferred500;
    };
    Built::Value(json!({
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

/// An optional int; None marks the deliberate beyond-i64 500.
fn int_value(value: Option<BodyInt>) -> Option<Value> {
    match value {
        None => Some(Value::Null),
        Some(BodyInt::Value(v)) => Some(json!(v)),
        Some(BodyInt::Overflow) => None,
    }
}

/// Extract a QuestUpdate set-fields dump: only fields present in the
/// body land in the dict (present-null included), mirroring the
/// backend's exclude-unset model dump.
fn quest_update_dump(v: &mut Validation, object: &BodyObject) -> Built<Value> {
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
                if let Some(value) = opt_str(v, object, name) {
                    dump.insert(field.to_string(), str_value(value));
                }
            }
            FieldKind::Float => {
                if let Some(value) = opt_f64(v, object, name) {
                    dump.insert(field.to_string(), f64_value(value));
                }
            }
            FieldKind::Bool => {
                if let Some(value) = body::opt_bool(v, object, name) {
                    dump.insert(
                        field.to_string(),
                        value.map(Value::Bool).unwrap_or(Value::Null),
                    );
                }
            }
            FieldKind::Int => {
                if let Some(value) = opt_int(v, object, name) {
                    match int_value(value) {
                        Some(rendered) => {
                            dump.insert(field.to_string(), rendered);
                        }
                        None => overflow = true,
                    }
                }
            }
            FieldKind::StrList => {
                if let Some(value) = opt_list_of_str(v, object, name) {
                    dump.insert(
                        field.to_string(),
                        value.map(|items| json!(items)).unwrap_or(Value::Null),
                    );
                }
            }
        }
    }
    if !v.is_ok() {
        return Built::Invalid;
    }
    if v.binding_taint() || overflow {
        // The taint comment on the create dump applies here verbatim.
        return Built::Deferred500;
    }
    Built::Value(Value::Object(dump))
}

/// PlaylistCreate: name, planet and estimated-minutes defaults,
/// quest_ids, and the optional nested items.
fn playlist_create_dump(v: &mut Validation, object: &BodyObject) -> Built<Value> {
    let name = body::required_str(v, object, "name");
    let planet = str_or_default(v, object, "planet", "Calypso");
    let estimated = int_or_default(v, object, "estimated_minutes", 30);
    let quest_ids = list_of_int_or_default(v, object, "quest_ids");
    let items = playlist_items(v, object);
    if !v.is_ok() {
        return Built::Invalid;
    }
    if v.binding_taint() {
        // The taint comment on the create dump applies here verbatim.
        return Built::Deferred500;
    }
    let Some(estimated) = int_value(estimated.map(Some).expect("validated")) else {
        return Built::Deferred500;
    };
    let Some(quest_ids) = int_list_value(quest_ids.expect("validated")) else {
        return Built::Deferred500;
    };
    let items = items.expect("validated");
    if items
        .as_array()
        .is_some_and(|entries| entries.iter().any(|item| item.get("__overflow").is_some()))
    {
        return Built::Deferred500;
    }
    Built::Value(json!({
        "name": name.expect("validated"),
        "planet": planet.expect("validated"),
        "estimated_minutes": estimated,
        "quest_ids": quest_ids,
        "items": items,
    }))
}

/// None marks a beyond-i64 member (the deliberate 500).
fn int_list_value(values: Vec<BodyInt>) -> Option<Value> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        match value {
            BodyInt::Value(v) => out.push(json!(v)),
            BodyInt::Overflow => return None,
        }
    }
    Some(Value::Array(out))
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
fn playlist_update_dump(v: &mut Validation, object: &BodyObject) -> Built<Value> {
    let mut dump = Map::new();
    let mut overflow = false;
    for field in ["name", "planet"] {
        if object.get(field).is_some() {
            if let Some(value) = opt_str(v, object, field) {
                dump.insert(field.to_string(), str_value(value));
            }
        }
    }
    if object.get("estimated_minutes").is_some() {
        if let Some(value) = opt_int(v, object, "estimated_minutes") {
            match int_value(value) {
                Some(rendered) => {
                    dump.insert("estimated_minutes".into(), rendered);
                }
                None => overflow = true,
            }
        }
    }
    if object.get("quest_ids").is_some() {
        match object.get("quest_ids") {
            Some(PyValue::Null) => {
                dump.insert("quest_ids".into(), Value::Null);
            }
            _ => {
                if let Some(values) = list_of_int_or_default(v, object, "quest_ids") {
                    match int_list_value(values) {
                        Some(rendered) => {
                            dump.insert("quest_ids".into(), rendered);
                        }
                        None => overflow = true,
                    }
                }
            }
        }
    }
    if object.get("items").is_some() {
        if let Some(items) = playlist_items(v, object) {
            dump.insert("items".into(), items);
        }
    }
    if !v.is_ok() {
        return Built::Invalid;
    }
    if v.binding_taint() || overflow || dump_has_overflow(&dump) {
        // The taint comment on the create dump applies here verbatim.
        return Built::Deferred500;
    }
    Built::Value(Value::Object(dump))
}

fn dump_has_overflow(dump: &Map<String, Value>) -> bool {
    dump.get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.get("__overflow").is_some()))
}

/// Read the request body to its VALUE level, answering the standalone
/// reply forms (the scanner's envelope, the encoding/depth 400s, the
/// render 500) that the backend never aggregates with path issues.
async fn standalone_body_value(req: Request) -> Result<body::BodyValue, Box<Response<Body>>> {
    let (content_type, bytes) = body_parts(req).await?;
    let mut value_validation = Validation::new();
    match body::read_body_value(content_type.as_deref(), &bytes, &mut value_validation) {
        Some(value) => Ok(value),
        None => Err(Box::new(value_validation.into_response())),
    }
}

/// POST /api/quests
async fn quests_create(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    match quest_create_dump(&mut v, &object) {
        Built::Invalid => v.into_response(),
        Built::Deferred500 => internal_server_error(),
        Built::Value(dump) => hydration.create_quest(&dump).await,
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
    let mut v = Validation::new();
    let path = match path_param(&mut v, quest_id_segment(req.uri().path(), ""), "quest_id") {
        PathParam::NotFound => return router_not_found(),
        outcome => outcome,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    match quest_update_dump(&mut v, &object) {
        Built::Invalid => v.into_response(),
        Built::Deferred500 => internal_server_error(),
        Built::Value(dump) => match path {
            PathParam::Value(id) => hydration.update_quest(id, &dump).await,
            // The unbounded path int validated; it crashes at the
            // handler's first parameter binding.
            _ => internal_server_error(),
        },
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
    let mut v = Validation::new();
    let path = match path_param(
        &mut v,
        quest_id_segment(req.uri().path(), "/cancel"),
        "quest_id",
    ) {
        PathParam::NotFound => return router_not_found(),
        outcome => outcome,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let undo_reward = match body::optional_object_from_body(body_value, &mut v) {
        Some(None) => Some(false),
        Some(Some(object)) => bool_or_default(&mut v, &object, "undo_reward", false),
        None => None,
    };
    if !v.is_ok() {
        return v.into_response();
    }
    let undo_reward = undo_reward.unwrap_or(false);
    match path {
        PathParam::Value(id) => hydration.cancel_quest(id, undo_reward).await,
        _ => internal_server_error(),
    }
}

/// POST /api/quests/playlists
async fn playlists_create(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    match playlist_create_dump(&mut v, &object) {
        Built::Invalid => v.into_response(),
        Built::Deferred500 => internal_server_error(),
        Built::Value(dump) => hydration.create_playlist(&dump).await,
    }
}

/// PUT /api/quests/playlists/{playlist_id}
async fn playlist_update(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let mut v = Validation::new();
    let path = match path_param(&mut v, playlist_id_segment(req.uri().path()), "playlist_id") {
        PathParam::NotFound => return router_not_found(),
        outcome => outcome,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    match playlist_update_dump(&mut v, &object) {
        Built::Invalid => v.into_response(),
        Built::Deferred500 => internal_server_error(),
        Built::Value(dump) => match path {
            PathParam::Value(id) => hydration.update_playlist(id, &dump).await,
            _ => internal_server_error(),
        },
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

/// POST /api/codex/claim: `{species_name, rank, skill_name}`. The service's
/// invalid-input errors map to a 400; on success a live session suppresses
/// the claimed skill's next gain. Mirrors `claim_rank`.
async fn codex_claim(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(tracker), Some(skill_tracker)) =
        (state.hydration(), state.tracker(), state.skill_tracker())
    else {
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
    let skill = body::required_str(&mut v, &object, "skill_name");
    if !v.is_ok() {
        return v.into_response();
    }
    // A surrogate-tainted string reaches the codex service before any gate
    // and crashes its lookup unhandled (the reference's 500), unlike the
    // calibrate path whose encode raises a ValueError -> 400. Mirror the 500.
    if v.binding_taint() {
        return internal_error();
    }
    let rank = body_int_or_max(rank.expect("validated"));
    hydration
        .codex_claim(
            &tracker,
            &skill_tracker,
            &species.expect("validated"),
            rank,
            &skill.expect("validated"),
        )
        .await
}

/// POST /api/codex/meta/claim: `{attribute_name}`. Mirrors `meta_claim`.
async fn codex_meta_claim(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(tracker), Some(skill_tracker)) =
        (state.hydration(), state.tracker(), state.skill_tracker())
    else {
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
    let attribute = body::required_str(&mut v, &object, "attribute_name");
    if !v.is_ok() {
        return v.into_response();
    }
    // The surrogate reaches the meta-claim lookup unhandled (the reference's
    // 500), mirrored here.
    if v.binding_taint() {
        return internal_error();
    }
    hydration
        .codex_meta_claim(&tracker, &skill_tracker, &attribute.expect("validated"))
        .await
}

// ── Manual scan adapters (scan_manual.py) ──────────────────────────────
//
// The skill-scan state machine and the one-shot repair-cost read serve over
// the composed `SkillScanManual` / `RepairOcrService` (always constructed;
// off Windows the OCR runtime is absent, so they report "engine unavailable"
// but the state machine still serves). They proxy only when composition was
// declined. The verbs' projection-and-serialise logic lives in `scan_routes`;
// these adapters extract each route's parameters the backend's way.

/// GET /api/scan/skills/status
async fn scan_skills_status(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    let inm = if_none_match(&req);
    crate::scan_routes::status(&scan, inm.as_deref())
}

/// POST /api/scan/skills/start?page_count=: `page_count` is `int | None`; an
/// unparseable value is the backend's 422 int_parsing, the range check (the
/// service's 1..=30) rides the plain-200 body.
async fn scan_skills_start(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let mut validation = Validation::new();
    let page_count = opt_query_int(&mut validation, &query, "page_count");
    if !validation.is_ok() {
        return validation.into_response();
    }
    crate::scan_routes::start(&scan, page_count.flatten())
}

/// POST /api/scan/skills/capture
async fn scan_skills_capture(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    crate::scan_routes::capture(&scan)
}

/// POST /api/scan/skills/cancel
async fn scan_skills_cancel(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    crate::scan_routes::cancel(&scan)
}

/// POST /api/scan/skills/undo
async fn scan_skills_undo(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    crate::scan_routes::undo(&scan)
}

/// POST /api/scan/skills/process
async fn scan_skills_process(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    crate::scan_routes::process(&scan)
}

/// POST /api/scan/skills/accept
async fn scan_skills_accept(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    crate::scan_routes::accept(&scan)
}

/// POST /api/scan/skills/reject
async fn scan_skills_reject(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    crate::scan_routes::reject(&scan)
}

/// GET /api/scan/skills/pending
async fn scan_skills_pending(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    let inm = if_none_match(&req);
    crate::scan_routes::pending(&scan, inm.as_deref())
}

/// GET /api/scan/skills/capture/{page}: the `{page}` is an `int` path param.
/// A percent-encoded slash de-matches (the framework 404), unparseable text
/// is the 422 int_parsing, and a magnitude beyond `i64` saturates to the
/// bound (the service then finds no such page and serves its own 404, exactly
/// as the reference's unbounded `int` indexes out of range to None).
async fn scan_skills_capture_png(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(scan) = state.skill_scan() else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/scan/skills/capture/")
        .unwrap_or_default();
    let decoded = decode_path_segment(raw);
    if decoded.contains('/') {
        return router_not_found();
    }
    let page = match parse_int_lax(&decoded) {
        Some(LaxInt::Value(value)) => value,
        Some(LaxInt::OverflowPositive) => i64::MAX,
        Some(LaxInt::OverflowNegative) => i64::MIN,
        None => {
            let mut v = Validation::new();
            v.int_parsing("path", "page", &decoded);
            return v.into_response();
        }
    };
    let inm = if_none_match(&req);
    crate::scan_routes::capture_png(&scan, page, inm.as_deref())
}

/// POST /api/tracking/session/{session_id}/repair-scan: the `session_id` is
/// routing-only (the reference ignores it), so a decoded slash still
/// de-matches (the framework 404) but the value is unused. Gated on the live
/// `repair_ocr_enabled` config flag (400 when off).
async fn tracking_repair_scan(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(repair), Some(config)) = (state.repair_ocr(), state.config_service()) else {
        return state.proxy(req).await;
    };
    if let Err(reply) = string_path_id(session_id_segment(req.uri().path(), "/repair-scan")) {
        return *reply;
    }
    let enabled = {
        let Ok(guard) = config.lock() else {
            return internal_error();
        };
        guard.get().repair_ocr_enabled
    };
    crate::scan_routes::repair_scan(&repair, enabled)
}

/// POST /api/scan/spacebar-capture?enabled=: toggle the hands-free capture
/// listener. `enabled` is a required bool; an uninterpretable value is the
/// backend's 422 bool_parsing, absent is its 422 missing.
async fn scan_spacebar_capture(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(listener) = state.spacebar_listener() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let mut validation = Validation::new();
    let enabled = require_query_bool(&mut validation, &query, "enabled");
    if !validation.is_ok() {
        return validation.into_response();
    }
    crate::scan_routes::spacebar_capture(&listener, enabled.expect("validated"))
}

// ── Settings adapters ───────────────────────────────────────────────
//
// The reads serve natively; the overlay-position write serves natively
// too (it has no producer side effects). PATCH /settings + POST
// /settings/reset stay proxied until the input-listener composition
// (they signal the hotbar listener), falling to each path's proxy arm.

simple_get!(settings_get, settings);
simple_get!(overlay_position_get, overlay_position);

/// PUT /api/settings/overlay-position: `{x, y}` ints. An unparseable
/// coordinate is the backend's 422 int_parsing.
async fn overlay_position_set(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(config)) = (state.hydration(), state.config_service()) else {
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
    let x = body::required_int_at(
        &mut v,
        object.pairs(),
        object.echo(),
        "x",
        &[Loc::Field("x")],
    );
    let y = body::required_int_at(
        &mut v,
        object.pairs(),
        object.echo(),
        "y",
        &[Loc::Field("y")],
    );
    if !v.is_ok() {
        return v.into_response();
    }
    let x = body_int_or_max(x.expect("validated"));
    let y = body_int_or_max(y.expect("validated"));
    hydration.overlay_position_set(&config, x, y).await
}

/// A parsed body int, with a beyond-`i64` value clamped to the max (an
/// absurd coordinate the reference would store as an unbounded Python int;
/// realistic values fit `i64`, so the clamp is never reached in practice).
fn body_int_or_max(value: BodyInt) -> i64 {
    match value {
        BodyInt::Value(parsed) => parsed,
        BodyInt::Overflow => i64::MAX,
    }
}

// ── Character adapters ──────────────────────────────────────────────

simple_get!(character_calibration, character_calibration);
simple_get!(character_stats, character_stats);
simple_get!(character_skills, character_skills);
simple_get!(character_professions, character_professions);
simple_get!(character_prospect_options, character_prospect_options);
simple_get!(character_hp_optimizer, character_hp_optimizer);
simple_get!(character_codex, character_codex);

// ── Analytics adapters ──────────────────────────────────────────────

simple_get!(analytics_activity, analytics_activity);

/// GET /api/analytics/overview: the `period` query selects the window.
/// Any unrecognised value falls through to all-time (the reference's
/// `dict.get` miss), so no validation envelope applies.
async fn analytics_overview(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let period = query.last("period").unwrap_or("all").to_string();
    hydration.analytics_overview(&period).await
}

// ── Tracking session-read adapters ──────────────────────────────────

simple_get!(tracking_sessions, tracking_sessions);

/// GET /api/tracking/session/{session_id}: the one path-parameter route of
/// this surface. The raw segment percent-decodes before the lookup; a
/// decoded slash reproduces the backend's route-level 404 (matching precedes
/// the handler, exactly as the codex-ranks adapter handles it).
async fn tracking_session(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/tracking/session/")
        .unwrap_or_default();
    let session_id = match string_path_id(raw) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let inm = if_none_match(&req);
    hydration
        .tracking_session(&session_id, inm.as_deref())
        .await
}

// ── Guide-mode demo adapters (`/api/demo/*`) ────────────────────────
//
// The demo serves a curated read-only dataset over a parallel hydration +
// tracker built on a writable clone of the bundled demo DB (see [`crate::demo`]).
// Each adapter resolves the lazily-built demo state, falling back to the proxy
// arm when it is unavailable (no native composition, no bundled demo DB, or a
// build failure). The demo prefix is outside the ETag middleware, so every
// reply is a plain JSON 200 (the demo state's methods enforce that).

async fn demo_analytics_overview(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let period = query.last("period").unwrap_or("all").to_string();
    demo.analytics_overview(&period).await
}

async fn demo_analytics_activity(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    demo.analytics_activity().await
}

async fn demo_analytics_ledger(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    demo.list_ledger().await
}

async fn demo_analytics_ledger_presets(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    demo.list_ledger_presets().await
}

async fn demo_analytics_inventory(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    demo.list_inventory().await
}

async fn demo_tracking_sessions(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    demo.list_sessions().await
}

/// GET /api/demo/tracking/session/{session_id}: the demo's one path-parameter
/// route, decoded exactly as the live [`tracking_session`] adapter.
async fn demo_tracking_session(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/demo/tracking/session/")
        .unwrap_or_default();
    let session_id = match string_path_id(raw) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    demo.get_session(&session_id).await
}

async fn demo_tracking_snapshot(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(demo) = crate::demo::ensure_demo(&state).await else {
        return state.proxy(req).await;
    };
    demo.tracking_snapshot().await
}

/// GET /api/tracking/tag-suggestions?q=&limit=: `q` defaults to the empty
/// string (short-circuiting to `[]`), `limit` to 10 (clamped to 1..=20 in
/// the handler). An unparseable `limit` is the backend's 422 int_parsing.
async fn tracking_tag_suggestions(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let q = query.last("q").unwrap_or("").to_string();
    let mut validation = Validation::new();
    let limit = query_int_or_default(&mut validation, &query, "limit", 10);
    if !validation.is_ok() {
        return validation.into_response();
    }
    let inm = if_none_match(&req);
    hydration
        .tracking_tag_suggestions(&q, limit.expect("validated"), inm.as_deref())
        .await
}

// ── Tracking producer adapters (live-tracker spine) ─────────────────
//
// These three reach a DIFFERENT dependency than the session-read
// adapters above: the live `Arc<HuntTracker>` (start/stop, and the
// manual-mob tag-mode gate) plus the bundled mobs catalogue (the
// suggestions). They proxy unless BOTH the tracker and the read surface
// are composed (the suggestions handler reads the catalogue through the
// hydration state), exactly as the read surface proxies without
// `with_hydration`.

/// POST /api/tracking/start
async fn tracking_start(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(tracker)) = (state.hydration(), state.tracker()) else {
        return state.proxy(req).await;
    };
    hydration.tracking_start(&tracker).await
}

/// POST /api/tracking/stop
async fn tracking_stop(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(tracker)) = (state.hydration(), state.tracker()) else {
        return state.proxy(req).await;
    };
    hydration.tracking_stop(&tracker).await
}

/// GET /api/tracking/manual-mob-suggestions?q=&limit=: `q` defaults to the
/// empty string (the tag-mode 409 still precedes the `[]` short-circuit),
/// `limit` to 10 (clamped to 1..=20 in the handler). An unparseable `limit`
/// is the backend's 422 int_parsing.
async fn tracking_manual_mob_suggestions(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(tracker)) = (state.hydration(), state.tracker()) else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let q = query.last("q").unwrap_or("").to_string();
    let mut validation = Validation::new();
    let limit = query_int_or_default(&mut validation, &query, "limit", 10);
    if !validation.is_ok() {
        return validation.into_response();
    }
    let inm = if_none_match(&req);
    hydration
        .tracking_manual_mob_suggestions(&tracker, &q, limit.expect("validated"), inm.as_deref())
        .await
}

/// POST /api/tracking/release-mob: clear the locked mob or tag (empty body).
async fn tracking_release_mob(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(config), Some(tracker)) =
        (state.hydration(), state.config_service(), state.tracker())
    else {
        return state.proxy(req).await;
    };
    hydration.release_mob(&config, &tracker).await
}

/// POST /api/tracking/manual-mob-lock: `{species, maturity?}`.
async fn tracking_manual_mob_lock(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(config), Some(tracker)) =
        (state.hydration(), state.config_service(), state.tracker())
    else {
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
    let species = body::required_str(&mut v, &object, "species");
    let maturity = body::str_or_default(&mut v, &object, "maturity", "");
    if !v.is_ok() {
        return v.into_response();
    }
    hydration
        .manual_mob_lock(
            &config,
            &tracker,
            &species.expect("validated"),
            &maturity.expect("validated"),
        )
        .await
}

/// POST /api/tracking/tag-lock: `{tag}`.
async fn tracking_tag_lock(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(config), Some(tracker)) =
        (state.hydration(), state.config_service(), state.tracker())
    else {
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
    let tag = body::required_str(&mut v, &object, "tag");
    if !v.is_ok() {
        return v.into_response();
    }
    hydration
        .tag_lock(
            &config,
            &tracker,
            &tag.expect("validated"),
            v.binding_taint(),
        )
        .await
}

/// GET /api/tracking/snapshot: the consolidated dashboard hydration, over the
/// live tracker, the config, and the hotbar listener's running state. Proxies
/// unless all three are composed (the snapshot reads each).
async fn tracking_snapshot(state: Arc<AppState>, req: Request) -> Response<Body> {
    let (Some(hydration), Some(tracker), Some(hotbar)) =
        (state.hydration(), state.tracker(), state.hotbar_listener())
    else {
        return state.proxy(req).await;
    };
    let inm = if_none_match(&req);
    hydration
        .tracking_snapshot(&tracker, &hotbar, inm.as_deref())
        .await
}

// ── SSE event stream (producer-bus fan-out) ────────────────────────────
//
// `GET /api/events` is a long-lived `text/event-stream`. The native arm
// serves it from the composed SSE hub (the producer-bus bridge forwards the
// frontend-facing domain topics onto it); without a composed hub it proxies
// to the sidecar, which streams the same contract. The route sits outside
// the ETag hydration prefixes (an unbounded stream cannot be body-hashed)
// and is OpenAPI-excluded, exactly as the sidecar's is.

/// GET /api/events
async fn events_stream(state: Arc<AppState>, req: Request) -> Response<Body> {
    match state.sse_hub() {
        Some(hub) => crate::sse::event_stream_response(hub),
        None => state.proxy(req).await,
    }
}

// ── Tracking session-edit write adapters ──────────────────────────────

/// The `{session_id}` of a `/api/tracking/session/{session_id}/<suffix>`
/// edit route. A percent-encoded slash de-matches (the framework 404),
/// exactly as the single-segment string path-id rule elsewhere.
fn session_id_segment<'p>(path: &'p str, suffix: &str) -> &'p str {
    path.strip_prefix("/api/tracking/session/")
        .and_then(|rest| rest.strip_suffix(suffix))
        .unwrap_or_default()
}

/// POST /api/tracking/session/{session_id}/rename-mob
async fn tracking_rename_mob(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let session_id = match string_path_id(session_id_segment(req.uri().path(), "/rename-mob")) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    // RenameMobRequest declaration order: fromMobName, then toMobName.
    let from_mob = body::required_str(&mut v, &object, "fromMobName");
    let to_mob = body::required_str(&mut v, &object, "toMobName");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    hydration
        .tracking_rename_mob(
            &session_id,
            &from_mob.expect("validated"),
            &to_mob.expect("validated"),
        )
        .await
}

/// POST /api/tracking/session/{session_id}/restore-mob
async fn tracking_restore_mob(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let session_id = match string_path_id(session_id_segment(req.uri().path(), "/restore-mob")) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let current_mob = body::required_str(&mut v, &object, "currentMobName");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    hydration
        .tracking_restore_mob(&session_id, &current_mob.expect("validated"))
        .await
}

/// The `{session_id}` and `{item_name:path}` of a loot-flip route. The
/// item segment is a FastAPI `:path` converter: it CONTAINS slashes
/// (raw or percent-encoded), so a decoded slash is KEPT rather than
/// turned into a 404 (the single-segment rule). The session id stays a
/// single segment, so its own decoded slash still de-matches.
fn loot_flip_segments(path: &str, suffix: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("/api/tracking/session/")?;
    let rest = rest.strip_suffix(suffix)?;
    // rest is `{session_id}/loot-item/{item_name:path}`; the session id
    // is the first segment, the item name is everything after the
    // `/loot-item/` marker (slashes included).
    let (session_raw, after) = rest.split_once('/')?;
    let item_raw = after.strip_prefix("loot-item/")?;
    let session_id = decode_path_segment(session_raw);
    if session_id.contains('/') {
        return None;
    }
    Some((session_id, decode_path_segment(item_raw)))
}

/// POST /api/tracking/session/{session_id}/loot-item/{item_name:path}/{deactivate|activate}
///
/// One adapter for both flip directions: axum's catch-all (`{*rest}`)
/// must be terminal, so the two suffix-distinguished FastAPI routes land
/// on a single wildcard registration here, dispatched on the trailing
/// `/deactivate` or `/activate` segment. A tail matching neither suffix
/// is the framework 404 (no FastAPI route would have matched it either).
async fn tracking_loot_item_flip(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let path = req.uri().path();
    if let Some((session_id, item_name)) = loot_flip_segments(path, "/deactivate") {
        hydration
            .tracking_deactivate_loot_item(&session_id, &item_name)
            .await
    } else if let Some((session_id, item_name)) = loot_flip_segments(path, "/activate") {
        hydration
            .tracking_activate_loot_item(&session_id, &item_name)
            .await
    } else {
        router_not_found()
    }
}

/// POST /api/tracking/session/{session_id}/armour-cost
async fn tracking_armour_cost(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let session_id = match string_path_id(session_id_segment(req.uri().path(), "/armour-cost")) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let cost = body::required_f64(&mut v, &object, "cost");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    hydration
        .tracking_set_armour_cost(&session_id, cost.expect("validated"))
        .await
}

/// GET /api/tracking/session/{session_id}/quest-link-suggestion: the
/// curated post-session linkage suggestion, under the conditional-GET
/// contract. A decoded slash in the session id de-matches (framework
/// 404), exactly as the other single-segment session routes.
async fn tracking_quest_link_suggestion(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let session_id = match string_path_id(session_id_segment(
        req.uri().path(),
        "/quest-link-suggestion",
    )) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let inm = if_none_match(&req);
    hydration
        .session_quest_link_suggestion(&session_id, inm.as_deref())
        .await
}

/// POST /api/tracking/session/{session_id}/quest-link: persist the
/// accept/decline decision. The body model (`SessionQuestLinkDecisionBody`)
/// requires a string `action`; validation precedes the route logic, so a
/// missing action is the 422 the framework raises before the handler's 404.
async fn tracking_quest_link_decide(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let session_id = match string_path_id(session_id_segment(req.uri().path(), "/quest-link")) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let action = body::required_str(&mut v, &object, "action");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    hydration
        .decide_session_quest_link(&session_id, &action.expect("validated"))
        .await
}

// ── Analytics ledger / preset / inventory write adapters ──

/// Decode a string path-id; a percent-encoded slash de-matches the route
/// (the backend decodes before matching), reproducing its framework 404.
fn string_path_id(raw_segment: &str) -> Result<String, Box<Response<Body>>> {
    let decoded = decode_path_segment(raw_segment);
    if decoded.contains('/') {
        return Err(Box::new(router_not_found()));
    }
    Ok(decoded)
}

async fn ledger_list(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    hydration.list_ledger().await
}

async fn ledger_create(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let date = body::required_str(&mut v, &object, "date");
    let kind = body::required_str(&mut v, &object, "type");
    let description = body::required_str(&mut v, &object, "description");
    let amount = body::required_f64(&mut v, &object, "amount");
    let tag = body::required_str(&mut v, &object, "tag");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    hydration
        .create_ledger_entry(
            &date.expect("validated"),
            &kind.expect("validated"),
            &description.expect("validated"),
            amount.expect("validated"),
            &tag.expect("validated"),
        )
        .await
}

async fn ledger_delete(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/analytics/ledger/")
        .unwrap_or_default();
    let id = match string_path_id(raw) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    hydration.delete_ledger_entry(&id).await
}

async fn presets_list(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    hydration.list_ledger_presets().await
}

async fn presets_create(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let name = body::required_str(&mut v, &object, "name");
    let kind = body::required_str(&mut v, &object, "type");
    let description = body::required_str(&mut v, &object, "description");
    let amount = body::required_f64(&mut v, &object, "amount");
    let tag = body::required_str(&mut v, &object, "tag");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    hydration
        .create_ledger_preset(
            &name.expect("validated"),
            &kind.expect("validated"),
            &description.expect("validated"),
            amount.expect("validated"),
            &tag.expect("validated"),
        )
        .await
}

async fn preset_delete(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/analytics/ledger/presets/")
        .unwrap_or_default();
    let id = match string_path_id(raw) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    hydration.delete_ledger_preset(&id).await
}

async fn inventory_list(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    hydration.list_inventory().await
}

async fn inventory_create(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let name = body::required_str(&mut v, &object, "name");
    let tt_value = body::required_f64(&mut v, &object, "tt_value");
    let markup_paid = body::required_f64(&mut v, &object, "markup_paid");
    let notes = opt_str(&mut v, &object, "notes");
    let acquired_at = opt_str(&mut v, &object, "acquired_at");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    let notes = notes.expect("validated");
    let acquired_at = acquired_at.expect("validated");
    hydration
        .create_inventory_item(
            &name.expect("validated"),
            tt_value.expect("validated"),
            markup_paid.expect("validated"),
            notes.as_deref(),
            acquired_at.as_deref(),
        )
        .await
}

async fn inventory_patch(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/analytics/inventory/")
        .unwrap_or_default();
    let id = match string_path_id(raw) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let name = opt_str(&mut v, &object, "name");
    let tt_value = opt_f64(&mut v, &object, "tt_value");
    let markup_paid = opt_f64(&mut v, &object, "markup_paid");
    let notes = opt_str(&mut v, &object, "notes");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    // Only a provided (non-null) field updates: the reference's
    // `if patch.x is not None` over each Optional field.
    let name = name.expect("validated");
    let tt_value = tt_value.expect("validated");
    let markup_paid = markup_paid.expect("validated");
    let notes = notes.expect("validated");
    hydration
        .update_inventory_item(
            &id,
            name.as_deref(),
            tt_value,
            markup_paid,
            notes.as_deref(),
        )
        .await
}

async fn inventory_delete(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/analytics/inventory/")
        .unwrap_or_default();
    let id = match string_path_id(raw) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    hydration.delete_inventory_item(&id).await
}

async fn inventory_sell(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let raw = req
        .uri()
        .path()
        .strip_prefix("/api/analytics/inventory/")
        .and_then(|rest| rest.strip_suffix("/sell"))
        .unwrap_or_default();
    let id = match string_path_id(raw) {
        Ok(id) => id,
        Err(reply) => return *reply,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    let sale_price = body::required_f64(&mut v, &object, "sale_price");
    let description = opt_str(&mut v, &object, "description");
    let sold_at = opt_str(&mut v, &object, "sold_at");
    if !v.is_ok() {
        return v.into_response();
    }
    if v.binding_taint() {
        return internal_server_error();
    }
    let description = description.expect("validated");
    let sold_at = sold_at.expect("validated");
    hydration
        .sell_inventory_item(
            &id,
            sale_price.expect("validated"),
            description.as_deref(),
            sold_at.as_deref(),
        )
        .await
}

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

/// Whether a string field arrived surrogate-tainted: read from the
/// value itself, so EVERY tainted field flags independently (the
/// validation's own marker is a single sticky bool).
fn field_tainted(object: &BodyObject, name: &str) -> bool {
    matches!(object.get(name), Some(PyValue::TaintedStr { .. }))
}

/// Extract an AddWeaponRequest in model declaration order, capturing
/// per-field surrogate taints (the handler resolves each at its
/// consumption point; an unused tainted field keeps flowing, exactly
/// as the backend lets it).
fn add_weapon_request(v: &mut Validation, object: &BodyObject) -> Built<EquipmentRequest> {
    let taint = EquipmentTaint {
        catalog_id: field_tainted(object, "catalog_id"),
        name: field_tainted(object, "name"),
        amp_catalog_id: field_tainted(object, "amp_catalog_id"),
        scope_catalog_id: field_tainted(object, "scope_catalog_id"),
        absorber_catalog_id: field_tainted(object, "absorber_catalog_id"),
    };
    let item_type = body::literal_required(v, object, "type", &["weapon", "healing", "consumable"]);
    let catalog_id = opt_str(v, object, "catalog_id");
    let name = opt_str(v, object, "name");
    let amp_catalog_id = opt_str(v, object, "amp_catalog_id");
    let scope_catalog_id = opt_str(v, object, "scope_catalog_id");
    let absorber_catalog_id = opt_str(v, object, "absorber_catalog_id");
    let weapon_markup = int_or_default(v, object, "weapon_markup", 100);
    let amp_markup = int_or_default(v, object, "amp_markup", 100);
    let scope_markup = int_or_default(v, object, "scope_markup", 100);
    let absorber_markup = int_or_default(v, object, "absorber_markup", 100);
    let damage_enhancers = int_or_default(v, object, "damage_enhancers", 0);
    if !v.is_ok() {
        return Built::Invalid;
    }
    let ints = [
        weapon_markup.expect("validated"),
        amp_markup.expect("validated"),
        scope_markup.expect("validated"),
        absorber_markup.expect("validated"),
        damage_enhancers.expect("validated"),
    ]
    .map(equipment_int);
    let [Some(weapon_markup), Some(amp_markup), Some(scope_markup), Some(absorber_markup), Some(damage_enhancers)] =
        ints
    else {
        return Built::Deferred500;
    };
    Built::Value(EquipmentRequest {
        item_type: item_type.expect("validated"),
        catalog_id: catalog_id.expect("validated"),
        name: name.expect("validated"),
        amp_catalog_id: amp_catalog_id.expect("validated"),
        scope_catalog_id: scope_catalog_id.expect("validated"),
        absorber_catalog_id: absorber_catalog_id.expect("validated"),
        weapon_markup,
        amp_markup,
        scope_markup,
        absorber_markup,
        damage_enhancers,
        taint,
    })
}

/// Extract a CalculateCostRequest in model declaration order
/// (catalog_id first, then the two-value type literal).
fn calculate_cost_request(v: &mut Validation, object: &BodyObject) -> Built<EquipmentRequest> {
    let taint = EquipmentTaint {
        catalog_id: field_tainted(object, "catalog_id"),
        name: false,
        amp_catalog_id: field_tainted(object, "amp_catalog_id"),
        scope_catalog_id: field_tainted(object, "scope_catalog_id"),
        absorber_catalog_id: field_tainted(object, "absorber_catalog_id"),
    };
    let catalog_id = body::required_str(v, object, "catalog_id");
    let item_type = body::literal_with_default(v, object, "type", &["weapon", "healing"], "weapon");
    let amp_catalog_id = opt_str(v, object, "amp_catalog_id");
    let scope_catalog_id = opt_str(v, object, "scope_catalog_id");
    let absorber_catalog_id = opt_str(v, object, "absorber_catalog_id");
    let weapon_markup = int_or_default(v, object, "weapon_markup", 100);
    let amp_markup = int_or_default(v, object, "amp_markup", 100);
    let scope_markup = int_or_default(v, object, "scope_markup", 100);
    let absorber_markup = int_or_default(v, object, "absorber_markup", 100);
    let damage_enhancers = int_or_default(v, object, "damage_enhancers", 0);
    if !v.is_ok() {
        return Built::Invalid;
    }
    let ints = [
        weapon_markup.expect("validated"),
        amp_markup.expect("validated"),
        scope_markup.expect("validated"),
        absorber_markup.expect("validated"),
        damage_enhancers.expect("validated"),
    ]
    .map(equipment_int);
    let [Some(weapon_markup), Some(amp_markup), Some(scope_markup), Some(absorber_markup), Some(damage_enhancers)] =
        ints
    else {
        return Built::Deferred500;
    };
    Built::Value(EquipmentRequest {
        item_type: item_type.expect("validated"),
        catalog_id: Some(catalog_id.expect("validated")),
        name: None,
        amp_catalog_id: amp_catalog_id.expect("validated"),
        scope_catalog_id: scope_catalog_id.expect("validated"),
        absorber_catalog_id: absorber_catalog_id.expect("validated"),
        weapon_markup,
        amp_markup,
        scope_markup,
        absorber_markup,
        damage_enhancers,
        taint,
    })
}

/// An equipment int field. The backend carries arbitrary-precision
/// integers through these (they flow into a JSON text column, never a
/// direct parameter binding); the native side answers the deliberate
/// 500 beyond i64 instead. See the divergence register (D-14).
fn equipment_int(value: BodyInt) -> Option<i64> {
    match value {
        BodyInt::Value(v) => Some(v),
        BodyInt::Overflow => None,
    }
}

/// POST /api/equipment/library.
async fn equipment_add(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    match add_weapon_request(&mut v, &object) {
        Built::Invalid => v.into_response(),
        Built::Deferred500 => internal_server_error(),
        Built::Value(request) => hydration.equipment_add(&request).await,
    }
}

/// PUT /api/equipment/library/{item_id}.
async fn equipment_update(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let mut v = Validation::new();
    let path = match path_param(
        &mut v,
        equipment_id_segment(req.uri().path(), ""),
        "item_id",
    ) {
        PathParam::NotFound => return router_not_found(),
        outcome => outcome,
    };
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    match add_weapon_request(&mut v, &object) {
        Built::Invalid => v.into_response(),
        Built::Deferred500 => internal_server_error(),
        Built::Value(request) => match path {
            PathParam::Value(id) => hydration.equipment_update(id, &request).await,
            _ => internal_server_error(),
        },
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
    let body_value = match standalone_body_value(req).await {
        Ok(value) => value,
        Err(reply) => return *reply,
    };
    let mut v = Validation::new();
    let Some(object) = body::object_from_body(body_value, &mut v) else {
        return v.into_response();
    };
    match calculate_cost_request(&mut v, &object) {
        Built::Invalid => v.into_response(),
        Built::Deferred500 => internal_server_error(),
        Built::Value(request) => hydration.equipment_cost(&request).await,
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
            "/api/codex/claim",
            arm_routed(MethodFilter::POST, "/api/codex/claim", codex_claim),
        )
        .route(
            "/api/codex/meta/claim",
            arm_routed(
                MethodFilter::POST,
                "/api/codex/meta/claim",
                codex_meta_claim,
            ),
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
            "/api/analytics/overview",
            arm_routed(
                MethodFilter::GET,
                "/api/analytics/overview",
                analytics_overview,
            ),
        )
        .route(
            "/api/analytics/activity",
            arm_routed(
                MethodFilter::GET,
                "/api/analytics/activity",
                analytics_activity,
            ),
        )
        .route(
            "/api/tracking/sessions",
            arm_routed(
                MethodFilter::GET,
                "/api/tracking/sessions",
                tracking_sessions,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}",
            arm_routed(
                MethodFilter::GET,
                "/api/tracking/session/{session_id}",
                tracking_session,
            ),
        )
        .route(
            "/api/tracking/tag-suggestions",
            arm_routed(
                MethodFilter::GET,
                "/api/tracking/tag-suggestions",
                tracking_tag_suggestions,
            ),
        )
        .route(
            "/api/tracking/start",
            arm_routed(MethodFilter::POST, "/api/tracking/start", tracking_start),
        )
        .route(
            "/api/tracking/stop",
            arm_routed(MethodFilter::POST, "/api/tracking/stop", tracking_stop),
        )
        .route(
            "/api/tracking/manual-mob-suggestions",
            arm_routed(
                MethodFilter::GET,
                "/api/tracking/manual-mob-suggestions",
                tracking_manual_mob_suggestions,
            ),
        )
        .route(
            "/api/tracking/snapshot",
            arm_routed(
                MethodFilter::GET,
                "/api/tracking/snapshot",
                tracking_snapshot,
            ),
        )
        // Guide-mode demo read namespace (`backend/routers/demo.py`): the eight
        // GETs the guide retargets analytics/tracking reads onto.
        .route(
            "/api/demo/analytics/overview",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/analytics/overview",
                demo_analytics_overview,
            ),
        )
        .route(
            "/api/demo/analytics/activity",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/analytics/activity",
                demo_analytics_activity,
            ),
        )
        .route(
            "/api/demo/analytics/ledger",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/analytics/ledger",
                demo_analytics_ledger,
            ),
        )
        .route(
            "/api/demo/analytics/ledger/presets",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/analytics/ledger/presets",
                demo_analytics_ledger_presets,
            ),
        )
        .route(
            "/api/demo/analytics/inventory",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/analytics/inventory",
                demo_analytics_inventory,
            ),
        )
        .route(
            "/api/demo/tracking/sessions",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/tracking/sessions",
                demo_tracking_sessions,
            ),
        )
        .route(
            "/api/demo/tracking/session/{session_id}",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/tracking/session/{session_id}",
                demo_tracking_session,
            ),
        )
        .route(
            "/api/demo/tracking/snapshot",
            arm_routed(
                MethodFilter::GET,
                "/api/demo/tracking/snapshot",
                demo_tracking_snapshot,
            ),
        )
        .route(
            "/api/tracking/release-mob",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/release-mob",
                tracking_release_mob,
            ),
        )
        .route(
            "/api/tracking/manual-mob-lock",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/manual-mob-lock",
                tracking_manual_mob_lock,
            ),
        )
        .route(
            "/api/tracking/tag-lock",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/tag-lock",
                tracking_tag_lock,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}/rename-mob",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/session/{session_id}/rename-mob",
                tracking_rename_mob,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}/restore-mob",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/session/{session_id}/restore-mob",
                tracking_restore_mob,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}/loot-item/{*item_action}",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/session/{session_id}/loot-item/{*item_action}",
                tracking_loot_item_flip,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}/armour-cost",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/session/{session_id}/armour-cost",
                tracking_armour_cost,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}/quest-link-suggestion",
            arm_routed(
                MethodFilter::GET,
                "/api/tracking/session/{session_id}/quest-link-suggestion",
                tracking_quest_link_suggestion,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}/quest-link",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/session/{session_id}/quest-link",
                tracking_quest_link_decide,
            ),
        )
        .route(
            "/api/tracking/session/{session_id}/repair-scan",
            arm_routed(
                MethodFilter::POST,
                "/api/tracking/session/{session_id}/repair-scan",
                tracking_repair_scan,
            ),
        )
        .route(
            "/api/scan/skills/status",
            arm_routed(
                MethodFilter::GET,
                "/api/scan/skills/status",
                scan_skills_status,
            ),
        )
        .route(
            "/api/scan/skills/start",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/skills/start",
                scan_skills_start,
            ),
        )
        .route(
            "/api/scan/skills/capture",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/skills/capture",
                scan_skills_capture,
            ),
        )
        .route(
            "/api/scan/skills/capture/{page}",
            arm_routed(
                MethodFilter::GET,
                "/api/scan/skills/capture/{page}",
                scan_skills_capture_png,
            ),
        )
        .route(
            "/api/scan/skills/cancel",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/skills/cancel",
                scan_skills_cancel,
            ),
        )
        .route(
            "/api/scan/skills/undo",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/skills/undo",
                scan_skills_undo,
            ),
        )
        .route(
            "/api/scan/skills/process",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/skills/process",
                scan_skills_process,
            ),
        )
        .route(
            "/api/scan/skills/accept",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/skills/accept",
                scan_skills_accept,
            ),
        )
        .route(
            "/api/scan/skills/reject",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/skills/reject",
                scan_skills_reject,
            ),
        )
        .route(
            "/api/scan/skills/pending",
            arm_routed(
                MethodFilter::GET,
                "/api/scan/skills/pending",
                scan_skills_pending,
            ),
        )
        .route(
            "/api/scan/spacebar-capture",
            arm_routed(
                MethodFilter::POST,
                "/api/scan/spacebar-capture",
                scan_spacebar_capture,
            ),
        )
        .route(
            "/api/analytics/ledger",
            ArmRoutes::at("/api/analytics/ledger")
                .on(MethodFilter::GET, ledger_list)
                .on(MethodFilter::POST, ledger_create)
                .into_method_router(),
        )
        .route(
            "/api/analytics/ledger/presets",
            ArmRoutes::at("/api/analytics/ledger/presets")
                .on(MethodFilter::GET, presets_list)
                .on(MethodFilter::POST, presets_create)
                .into_method_router(),
        )
        .route(
            "/api/analytics/ledger/presets/{preset_id}",
            arm_routed(
                MethodFilter::DELETE,
                "/api/analytics/ledger/presets/{preset_id}",
                preset_delete,
            ),
        )
        .route(
            "/api/analytics/ledger/{entry_id}",
            arm_routed(
                MethodFilter::DELETE,
                "/api/analytics/ledger/{entry_id}",
                ledger_delete,
            ),
        )
        .route(
            "/api/analytics/inventory",
            ArmRoutes::at("/api/analytics/inventory")
                .on(MethodFilter::GET, inventory_list)
                .on(MethodFilter::POST, inventory_create)
                .into_method_router(),
        )
        .route(
            "/api/analytics/inventory/{item_id}/sell",
            arm_routed(
                MethodFilter::POST,
                "/api/analytics/inventory/{item_id}/sell",
                inventory_sell,
            ),
        )
        .route(
            "/api/analytics/inventory/{item_id}",
            ArmRoutes::at("/api/analytics/inventory/{item_id}")
                .on(MethodFilter::PATCH, inventory_patch)
                .on(MethodFilter::DELETE, inventory_delete)
                .into_method_router(),
        )
        .route(
            "/api/settings",
            arm_routed(MethodFilter::GET, "/api/settings", settings_get),
        )
        .route(
            "/api/settings/overlay-position",
            ArmRoutes::at("/api/settings/overlay-position")
                .on(MethodFilter::GET, overlay_position_get)
                .on(MethodFilter::PUT, overlay_position_set)
                .into_method_router(),
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
        .route(
            "/api/events",
            arm_routed(MethodFilter::GET, "/api/events", events_stream),
        )
}
