//! Natively-served hydration handlers for the quests and codex read
//! surface, byte-faithful to the backend's responses: the router-layer
//! camelCase formatting (ids as strings, rounded analytics columns,
//! or-empty text fields), the body serialisation form the backend's
//! HTTP layer emits, and the strong-ETag conditional-GET semantics of
//! its middleware (`backend/middleware/etag.py`): a SHA-256 ETag over
//! the body, `Cache-Control: no-cache`, and `304 Not Modified` with an
//! empty body when `If-None-Match` already names the representation.
//!
//! The handlers live here proof-first: each is exercised against the
//! running backend over a shared database before any route registers
//! natively (registration in `native_routes` is the cutover, one line
//! per route, with the runtime arm override as the rollback).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Response, StatusCode};
use eo_services::clock::Clock;
use eo_services::codex::{CodexError, CodexService};
use eo_services::game_data_store::GameDataStore;
use eo_services::quests::{QuestError, QuestService};
use eo_wire::normalizer::to_wire_json;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// The services the hydration handlers read through.
pub struct HydrationState {
    quests: QuestService,
    codex: CodexService,
}

impl HydrationState {
    pub fn new(pool: SqlitePool, game_data: Arc<GameDataStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            quests: QuestService::new(pool.clone(), clock.clone()),
            codex: CodexService::new(pool, game_data, clock),
        }
    }
}

/// The strong ETag value (quoted SHA-256 hex) for a body, exactly as
/// the backend's middleware computes it.
pub fn compute_strong_etag(body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body);
    format!("\"{:x}\"", hasher.finalize())
}

/// Whether `If-None-Match` indicates the client already holds the
/// representation: the wildcard, or any listed tag equal to the
/// current one (a weak `W/` prefix on a listed tag is ignored for the
/// comparison, mirroring the backend's parser).
fn if_none_match_matches(header_value: Option<&str>, current_etag: &str) -> bool {
    let Some(header_value) = header_value else {
        return false;
    };
    if header_value.trim() == "*" {
        return true;
    }
    header_value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        let candidate = candidate.strip_prefix("W/").unwrap_or(candidate);
        candidate == current_etag
    })
}

/// A hydration JSON response under the conditional-GET contract: 200
/// with the body (or 304 with none) plus the ETag and Cache-Control
/// headers either way.
fn json_response(payload: &Value, if_none_match: Option<&str>) -> Response<Body> {
    let body = to_wire_json(payload).into_bytes();
    let etag = compute_strong_etag(&body);
    let not_modified = if_none_match_matches(if_none_match, &etag);
    let mut response = Response::builder()
        .status(if not_modified {
            StatusCode::NOT_MODIFIED
        } else {
            StatusCode::OK
        })
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, "no-cache");
    if !not_modified {
        response = response.header(header::CONTENT_TYPE, "application/json");
    }
    response
        .body(if not_modified {
            Body::empty()
        } else {
            Body::from(body)
        })
        .expect("response assembles")
}

/// A non-2xx JSON error response (no ETag: the middleware touches
/// only successful responses).
fn error_response(status: StatusCode, payload: &Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(to_wire_json(payload).into_bytes()))
        .expect("response assembles")
}

/// The backend's HTTPException rendering: `{"detail": <message>}`.
fn detail(message: &str) -> Value {
    json!({"detail": message})
}

/// A service failure surfaces as the backend's unhandled-exception
/// envelope (500 with the generic body).
fn internal_error() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("Internal Server Error"))
        .expect("response assembles")
}

// ── Router-layer formatters (backend/routers/quests.py) ────────────

/// The quest wire shape, mirroring `_format_quest` key for key.
fn format_quest(quest: &Value) -> Value {
    let string_id = |value: &Value| json!(python_str_of(value));
    let mut out = Map::new();
    out.insert("id".into(), string_id(&quest["id"]));
    out.insert("name".into(), quest["name"].clone());
    out.insert("category".into(), quest["category"].clone());
    out.insert("targetMobs".into(), quest["mobs"].clone());
    out.insert("planet".into(), quest["planet"].clone());
    out.insert("waypoint".into(), quest["waypoint"].clone());
    out.insert(
        "cooldownDurationHours".into(),
        quest["cooldown_hours"].clone(),
    );
    out.insert(
        "cooldownExpiresAt".into(),
        quest["cooldown_expires_at"].clone(),
    );
    out.insert("reward".into(), quest["reward_ped"].clone());
    out.insert(
        "rewardIsSkill".into(),
        json!(quest["reward_is_skill"].as_i64().unwrap_or(0) != 0),
    );
    out.insert(
        "expectedRewardMarkupPercent".into(),
        quest["expected_reward_markup_percent"].clone(),
    );
    out.insert(
        "rewardDescription".into(),
        or_empty(&quest["reward_description"]),
    );
    out.insert("notes".into(), or_empty(&quest["notes"]));
    out.insert("chainName".into(), quest["chain_name"].clone());
    out.insert("chainPosition".into(), quest["chain_position"].clone());
    out.insert("chainTotal".into(), quest["chain_total"].clone());
    out.insert(
        "playlistIds".into(),
        json!(quest["playlist_ids"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|id| json!(python_str_of(id)))
            .collect::<Vec<_>>()),
    );
    out.insert("startedAt".into(), quest["started_at"].clone());
    Value::Object(out)
}

/// The playlist wire shape, mirroring `_format_playlist`.
fn format_playlist(playlist: &Value) -> Value {
    let string_ids = |value: &Value| {
        json!(value
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|id| json!(python_str_of(id)))
            .collect::<Vec<_>>())
    };
    let items = playlist["items"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|item| {
            json!({
                "questId": python_str_of(&item["quest_id"]),
                "description": item["description"],
                "groupType": item.get("group_type").cloned().unwrap_or_else(|| json!("immediate")),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": python_str_of(&playlist["id"]),
        "name": playlist["name"],
        "planet": playlist["planet"],
        "estimatedMinutes": playlist["estimated_minutes"],
        "questIds": string_ids(&playlist["quest_ids"]),
        "immediateQuestIds": string_ids(&playlist["immediate_quest_ids"]),
        "longHorizonQuestIds": string_ids(&playlist["long_horizon_quest_ids"]),
        "items": items,
    })
}

/// The per-quest analytics wire shape, mirroring
/// `_format_quest_analytics` (rounded columns included).
fn format_quest_analytics(row: &Value) -> Value {
    json!({
        "questId": python_str_of(&row["quest_id"]),
        "questName": row["quest_name"],
        "planet": row["planet"],
        "category": row["category"],
        "rewardPed": float_field(rounded(&row["reward_ped"], 2)),
        "rewardIsSkill": row["reward_is_skill"],
        "expectedRewardMarkupPercent": row["expected_reward_markup_percent"],
        "totalExpectedRewardPed": float_field(rounded(&row["total_expected_reward_ped"], 2)),
        "linkedSessions": row["linked_sessions"],
        "totalDurationSec": float_field(rounded(&row["total_duration"], 1)),
        "totalWeaponCost": float_field(rounded(&row["weapon_cost"], 4)),
        "totalHealCost": float_field(rounded(&row["heal_cost"], 4)),
        "totalEnhancerCost": float_field(rounded(&row["enhancer_cost"], 4)),
        "totalArmourCost": float_field(rounded(&row["armour_cost"], 4)),
        "totalLootTt": float_field(rounded(&row["loot_tt"], 4)),
        "totalPes": float_field(rounded(&row["skill_tt"], 4)),
    })
}

/// The per-playlist analytics wire shape, mirroring
/// `_format_playlist_analytics`.
fn format_playlist_analytics(row: &Value) -> Value {
    json!({
        "playlistId": python_str_of(&row["playlist_id"]),
        "playlistName": row["playlist_name"],
        "questCount": row["quest_count"],
        "longHorizonQuestCount": row["long_horizon_quest_count"],
        "matchedSessions": row["matched_sessions"],
        "totalRewardPed": float_field(rounded(&row["total_reward_ped"], 2)),
        "totalImmediateRewardPed": float_field(rounded(&row["total_immediate_reward_ped"], 2)),
        "totalBonusRewardPed": float_field(rounded(&row["total_bonus_reward_ped"], 2)),
        "totalPesReward": float_field(rounded(&row["total_skill_reward_ped"], 2)),
        "totalImmediatePesReward": float_field(rounded(&row["total_immediate_skill_reward_ped"], 2)),
        "totalBonusPesReward": float_field(rounded(&row["total_bonus_skill_reward_ped"], 2)),
        "totalExpectedRewardPed": float_field(rounded(&row["total_expected_reward_ped"], 2)),
        "totalExpectedImmediateRewardPed": float_field(rounded(&row["total_expected_immediate_reward_ped"], 2)),
        "totalExpectedBonusRewardPed": float_field(rounded(&row["total_expected_bonus_reward_ped"], 2)),
        "totalDurationSec": float_field(rounded(&row["total_duration"], 1)),
        "totalWeaponCost": float_field(rounded(&row["weapon_cost"], 4)),
        "totalHealCost": float_field(rounded(&row["heal_cost"], 4)),
        "totalEnhancerCost": float_field(rounded(&row["enhancer_cost"], 4)),
        "totalArmourCost": float_field(rounded(&row["armour_cost"], 4)),
        "totalLootTt": float_field(rounded(&row["loot_tt"], 4)),
        "totalPes": float_field(rounded(&row["skill_tt"], 4)),
    })
}

/// `str(value)` as the formatters apply it to ids (integers render
/// identically in both languages; strings pass through).
fn python_str_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// `value or ""` over nullable text columns.
fn or_empty(value: &Value) -> Value {
    match value.as_str() {
        Some(text) if !text.is_empty() => value.clone(),
        _ => json!(""),
    }
}

/// A model-declared float field: the response models coerce an
/// integer value to its float form at serialisation, so an
/// engine-typed integer zero leaves the wire as `0.0`.
fn float_field(value: Value) -> Value {
    match value.as_i64() {
        Some(integer) => json!(integer as f64),
        None => value,
    }
}

/// `round(value, places)` over the engine-typed analytics numbers:
/// Python's round keeps an int an int and applies banker's rounding
/// to floats.
fn rounded(value: &Value, places: usize) -> Value {
    match value.as_f64() {
        Some(number) if value.is_f64() => {
            json!(eo_wire::normalizer::round_half_even(number, places))
        }
        _ => value.clone(),
    }
}

// ── The nine hydration handlers ─────────────────────────────────────

impl HydrationState {
    /// GET /api/quests
    pub async fn list_quests(&self, if_none_match: Option<&str>) -> Response<Body> {
        match self.quests.get_quests(true).await {
            Ok(quests) => json_response(
                &json!(quests.iter().map(format_quest).collect::<Vec<_>>()),
                if_none_match,
            ),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/quests/mobs
    pub async fn list_mob_names(&self, if_none_match: Option<&str>) -> Response<Body> {
        match self.quests.get_all_mob_names().await {
            Ok(names) => json_response(&json!(names), if_none_match),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/quests/analytics
    pub async fn quest_analytics(&self, if_none_match: Option<&str>) -> Response<Body> {
        match self.quests.get_quest_analytics().await {
            Ok(rows) => json_response(
                &json!(rows.iter().map(format_quest_analytics).collect::<Vec<_>>()),
                if_none_match,
            ),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/quests/playlists
    pub async fn list_playlists(&self, if_none_match: Option<&str>) -> Response<Body> {
        match self.quests.get_playlists(true).await {
            Ok(playlists) => json_response(
                &json!(playlists.iter().map(format_playlist).collect::<Vec<_>>()),
                if_none_match,
            ),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/quests/playlists/analytics
    pub async fn playlist_analytics(&self, if_none_match: Option<&str>) -> Response<Body> {
        match self.quests.get_all_playlist_analytics().await {
            Ok(rows) => json_response(
                &json!(rows
                    .iter()
                    .map(format_playlist_analytics)
                    .collect::<Vec<_>>()),
                if_none_match,
            ),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/codex/species
    pub async fn codex_species(&self, if_none_match: Option<&str>) -> Response<Body> {
        match self.codex.get_all_species().await {
            Ok(species) => json_response(&json!(species), if_none_match),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/codex/species/{name}/ranks
    pub async fn codex_species_ranks(
        &self,
        name: &str,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        match self.codex.get_species_ranks(name).await {
            Ok(Some(ranks)) => json_response(&ranks, if_none_match),
            Ok(None) => error_response(
                StatusCode::NOT_FOUND,
                &detail(&format!("Species '{name}' not found")),
            ),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/codex/recommend
    pub async fn codex_recommend(
        &self,
        species_name: &str,
        rank: i64,
        profession: Option<&str>,
        target: &str,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        // The route model constrains the rank to the codex table's
        // domain and rejects everything else before the service runs.
        if !(1..=25).contains(&rank) {
            let bound = if rank < 1 {
                json!({
                    "type": "greater_than_equal",
                    "loc": ["query", "rank"],
                    "msg": "Input should be greater than or equal to 1",
                    "input": rank.to_string(),
                    "ctx": {"ge": 1},
                })
            } else {
                json!({
                    "type": "less_than_equal",
                    "loc": ["query", "rank"],
                    "msg": "Input should be less than or equal to 25",
                    "input": rank.to_string(),
                    "ctx": {"le": 25},
                })
            };
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &json!({"detail": [bound]}),
            );
        }
        match self
            .codex
            .get_skill_options(species_name, rank, profession, target)
            .await
        {
            Ok(options) => json_response(&json!(options), if_none_match),
            Err(CodexError::Invalid(_)) | Err(CodexError::Db(_)) => internal_error(),
        }
    }

    /// GET /api/codex/meta/attributes
    pub async fn codex_meta_attributes(&self, if_none_match: Option<&str>) -> Response<Body> {
        match self.codex.get_meta_attributes().await {
            Ok(attributes) => json_response(&json!(attributes), if_none_match),
            Err(_) => internal_error(),
        }
    }
}

/// The quest router's error mapping: the quests router catches no
/// service error, so every failure surfaces as the backend's
/// unhandled-exception envelope.
pub fn quest_error_response(_error: QuestError) -> Response<Body> {
    internal_error()
}
