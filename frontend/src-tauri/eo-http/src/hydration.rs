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

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Response, StatusCode};
use eo_services::clock::Clock;
use eo_services::codex::{CodexError, CodexService};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::quests::{QuestError, QuestService};
use eo_services::skill_tracker::{SkillTracker, SUPPRESS_TIMEOUT_SECONDS};
use eo_services::tracker::HuntTracker;
use eo_wire::normalizer::to_wire_json;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// The services the hydration handlers read through.
pub struct HydrationState {
    quests: QuestService,
    codex: CodexService,
    pub(crate) db: Db,
    pub(crate) game_data: Arc<GameDataStore>,
    pub(crate) clock: Arc<dyn Clock>,
    /// The data directory the substrate serves: the config read-through
    /// (`settings.json` stays sidecar-written until the producer
    /// cutover) and the `dbPath` settings field both render from it.
    pub(crate) data_dir: PathBuf,
}

impl HydrationState {
    pub fn new(
        db: Db,
        game_data: Arc<GameDataStore>,
        clock: Arc<dyn Clock>,
        data_dir: PathBuf,
    ) -> Self {
        let pool: SqlitePool = db.pool().clone();
        Self {
            quests: QuestService::new(pool.clone(), clock.clone()),
            codex: CodexService::new(pool, game_data.clone(), clock.clone()),
            db,
            game_data,
            clock,
            data_dir,
        }
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        self.db.pool()
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
/// current one. A weak `W/` prefix on a listed tag is removed before
/// the comparison, and whitespace after the prefix is tolerated, both
/// exactly as the backend's parser behaves (it strips around the
/// prefix removal).
fn if_none_match_matches(header_value: Option<&str>, current_etag: &str) -> bool {
    let Some(header_value) = header_value else {
        return false;
    };
    if header_value.trim() == "*" {
        return true;
    }
    header_value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        let candidate = candidate.strip_prefix("W/").unwrap_or(candidate).trim();
        candidate == current_etag
    })
}

/// A 200 response under the conditional-GET contract for an arbitrary body
/// and media type: the strong ETag over the body, `Cache-Control: no-cache`,
/// and `304 Not Modified` (empty body) on a matching `If-None-Match`. The
/// ETag middleware covers every 2xx GET under its prefixes regardless of
/// media type, so the manual-scan capture PNG rides this exactly as the JSON
/// reads do; [`json_response`] is this specialised to JSON.
pub(crate) fn conditional_response(
    body: Vec<u8>,
    media_type: &'static str,
    if_none_match: Option<&str>,
) -> Response<Body> {
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
        response = response.header(header::CONTENT_TYPE, media_type);
    }
    response
        .body(if not_modified {
            Body::empty()
        } else {
            Body::from(body)
        })
        .expect("response assembles")
}

/// A hydration JSON response under the conditional-GET contract: 200
/// with the body (or 304 with none) plus the ETag and Cache-Control
/// headers either way.
pub(crate) fn json_response(payload: &Value, if_none_match: Option<&str>) -> Response<Body> {
    conditional_response(
        to_wire_json(payload).into_bytes(),
        "application/json",
        if_none_match,
    )
}

/// A non-2xx JSON error response (no ETag: the middleware touches
/// only successful responses).
pub(crate) fn error_response(status: StatusCode, payload: &Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(to_wire_json(payload).into_bytes()))
        .expect("response assembles")
}

/// The backend's HTTPException rendering: `{"detail": <message>}`.
pub(crate) fn detail(message: &str) -> Value {
    json!({"detail": message})
}

/// A service failure surfaces as the backend's unhandled-exception
/// envelope (500 with the generic body).
pub(crate) fn internal_error() -> Response<Body> {
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
pub(crate) fn python_str_of(value: &Value) -> String {
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

/// `str(id) if id is not None else None` over a suggestion's nullable
/// id (the quest-link routes stringify the integer ids, leaving null
/// through).
fn str_id_or_null(value: &Value) -> Value {
    if value.is_null() {
        Value::Null
    } else {
        json!(python_str_of(value))
    }
}

/// The quest-link suggestion wire shape, mirroring
/// `get_session_quest_link_suggestion`'s dict construction (all seven
/// fields always present; the link fields null when absent).
fn format_quest_link_suggestion(session_id: &str, suggestion: &Value) -> Value {
    json!({
        "sessionId": session_id,
        "suggestionType": suggestion["suggestion_type"],
        "reason": suggestion["reason"],
        "questId": str_id_or_null(&suggestion["quest_id"]),
        "questName": suggestion["quest_name"],
        "playlistId": str_id_or_null(&suggestion["playlist_id"]),
        "playlistName": suggestion["playlist_name"],
    })
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

/// The write surface: each method mirrors its router handler (the
/// service call, the 404 mapping, the formatter), replying without
/// conditional-GET headers (the backend's middleware covers 2xx GETs
/// only).
impl HydrationState {
    /// GET /api/quests/{quest_id} (a read: the conditional-GET
    /// contract applies).
    pub async fn get_quest_route(
        &self,
        quest_id: i64,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        match self.quests.get_quest(quest_id).await {
            Ok(Some(quest)) => json_response(&format_quest(&quest), if_none_match),
            Ok(None) => quest_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// POST /api/quests
    pub async fn create_quest(&self, data: &Value) -> Response<Body> {
        match self.quests.create_quest(data).await {
            Ok(created) => plain_json_response(&format_quest(&created)),
            Err(error) => quest_error_response(error),
        }
    }

    /// PUT /api/quests/{quest_id}
    pub async fn update_quest(&self, quest_id: i64, data: &Value) -> Response<Body> {
        match self.quests.update_quest(quest_id, data).await {
            Ok(Some(updated)) => plain_json_response(&format_quest(&updated)),
            Ok(None) => quest_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// DELETE /api/quests/{quest_id}
    pub async fn delete_quest(&self, quest_id: i64) -> Response<Body> {
        match self.quests.delete_quest(quest_id).await {
            Ok(true) => plain_json_response(&json!({"ok": true})),
            Ok(false) => quest_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// POST /api/quests/{quest_id}/start
    pub async fn start_quest(&self, quest_id: i64) -> Response<Body> {
        match self.quests.start_quest(quest_id).await {
            Ok(Some(quest)) => plain_json_response(&format_quest(&quest)),
            Ok(None) => quest_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// POST /api/quests/{quest_id}/complete
    pub async fn complete_quest(&self, quest_id: i64) -> Response<Body> {
        match self.quests.complete_quest(quest_id).await {
            Ok(Some(quest)) => plain_json_response(&format_quest(&quest)),
            Ok(None) => quest_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// POST /api/quests/{quest_id}/cancel
    pub async fn cancel_quest(&self, quest_id: i64, undo_reward: bool) -> Response<Body> {
        match self.quests.cancel_quest(quest_id, undo_reward).await {
            Ok(Some(quest)) => plain_json_response(&format_quest(&quest)),
            Ok(None) => quest_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// POST /api/quests/playlists
    pub async fn create_playlist(&self, data: &Value) -> Response<Body> {
        match self.quests.create_playlist(data).await {
            Ok(created) => plain_json_response(&format_playlist(&created)),
            Err(error) => quest_error_response(error),
        }
    }

    /// PUT /api/quests/playlists/{playlist_id}
    pub async fn update_playlist(&self, playlist_id: i64, data: &Value) -> Response<Body> {
        match self.quests.update_playlist(playlist_id, data).await {
            Ok(Some(updated)) => plain_json_response(&format_playlist(&updated)),
            Ok(None) => playlist_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// DELETE /api/quests/playlists/{playlist_id}
    pub async fn delete_playlist(&self, playlist_id: i64) -> Response<Body> {
        match self.quests.delete_playlist(playlist_id).await {
            Ok(true) => plain_json_response(&json!({"ok": true})),
            Ok(false) => playlist_not_found(),
            Err(error) => quest_error_response(error),
        }
    }

    /// GET /api/tracking/session/{session_id}/quest-link-suggestion (a
    /// read: the conditional-GET contract applies). 404 if the session
    /// is absent, checked before the quest service runs.
    pub async fn session_quest_link_suggestion(
        &self,
        session_id: &str,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        match self.session_exists(session_id).await {
            Ok(true) => {}
            Ok(false) => return session_not_found(),
            Err(_) => return internal_error(),
        }
        match self.quests.get_session_link_suggestion(session_id).await {
            Ok(suggestion) => json_response(
                &format_quest_link_suggestion(session_id, &suggestion),
                if_none_match,
            ),
            Err(error) => quest_error_response(error),
        }
    }

    /// POST /api/tracking/session/{session_id}/quest-link (a plain-200
    /// write). 404 if the session is absent; an unknown action is a
    /// 400. Accept persists the curated suggestion and replies with the
    /// full link object; decline records the refusal and replies with
    /// only `sessionId`/`status` (the reference serialises the decision
    /// with `response_model_exclude_unset`, so the two arms emit
    /// different field sets).
    pub async fn decide_session_quest_link(
        &self,
        session_id: &str,
        action: &str,
    ) -> Response<Body> {
        match self.session_exists(session_id).await {
            Ok(true) => {}
            Ok(false) => return session_not_found(),
            Err(_) => return internal_error(),
        }
        let action = action.trim().to_lowercase();
        if action == "accept" {
            return match self.quests.accept_session_link_suggestion(session_id).await {
                Ok(suggestion) => plain_json_response(&json!({
                    "sessionId": session_id,
                    "status": "linked",
                    "linkType": suggestion["suggestion_type"],
                    "questId": str_id_or_null(&suggestion["quest_id"]),
                    "questName": suggestion["quest_name"],
                    "playlistId": str_id_or_null(&suggestion["playlist_id"]),
                    "playlistName": suggestion["playlist_name"],
                })),
                // The route catches the no-linkable-suggestion
                // ValueError and maps it to 409 (unlike the rest of the
                // quest surface, where Invalid stays an unhandled 500);
                // a genuine database failure still surfaces as 500.
                Err(QuestError::Invalid(message)) => {
                    error_response(StatusCode::CONFLICT, &detail(&message))
                }
                Err(QuestError::Db(_)) => internal_error(),
            };
        }
        if action == "decline" {
            return match self.quests.decline_session_link(session_id).await {
                Ok(()) => plain_json_response(&json!({
                    "sessionId": session_id,
                    "status": "declined",
                })),
                Err(error) => quest_error_response(error),
            };
        }
        error_response(
            StatusCode::BAD_REQUEST,
            &detail("Action must be 'accept' or 'decline'"),
        )
    }

    /// The session-existence precondition both quest-link routes apply
    /// before the quest service runs (a bare `SELECT id`, the
    /// reference's own guard).
    async fn session_exists(&self, session_id: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT id FROM tracking_sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.is_some())
    }

    /// The codex routers' ValueError mapping: a 400 with the message
    /// as the detail (adapters reproduce service-adjacent failures the
    /// reference raises as ValueError through this).
    pub fn codex_value_error(&self, message: &str) -> Response<Body> {
        error_response(StatusCode::BAD_REQUEST, &detail(message))
    }

    /// POST /api/codex/calibrate (the codex router maps its service's
    /// invalid-input errors to a 400 with the message as the detail).
    pub async fn codex_calibrate(&self, species_name: &str, rank: i64) -> Response<Body> {
        match self.codex.calibrate(species_name, rank).await {
            Ok(result) => plain_json_response(&result),
            Err(CodexError::Invalid(message)) => {
                error_response(StatusCode::BAD_REQUEST, &detail(&message))
            }
            Err(CodexError::Db(_)) => internal_error(),
        }
    }

    /// POST /api/codex/claim: claim a codex rank reward. Mirrors
    /// `claim_rank`: the service's invalid-input errors map to a 400 with
    /// the message; on success, an active session suppresses the upcoming
    /// skill gain from dedup (`suppress_next`), exactly as the reference
    /// does and only after the claim succeeds.
    pub async fn codex_claim(
        &self,
        tracker: &Arc<HuntTracker>,
        skill_tracker: &Arc<SkillTracker>,
        species_name: &str,
        rank: i64,
        skill_name: &str,
    ) -> Response<Body> {
        match self.codex.claim_rank(species_name, rank, skill_name).await {
            Ok(result) => {
                if tracker.is_tracking() {
                    skill_tracker.suppress_next(skill_name, SUPPRESS_TIMEOUT_SECONDS);
                }
                plain_json_response(&result)
            }
            Err(CodexError::Invalid(message)) => {
                error_response(StatusCode::BAD_REQUEST, &detail(&message))
            }
            Err(CodexError::Db(_)) => internal_error(),
        }
    }

    /// POST /api/codex/meta/claim: claim a meta codex reward (1 PED into an
    /// attribute). Mirrors `meta_claim`: invalid input maps to a 400; on
    /// success, an active session suppresses the upcoming attribute skill
    /// gain (`suppress_next`).
    pub async fn codex_meta_claim(
        &self,
        tracker: &Arc<HuntTracker>,
        skill_tracker: &Arc<SkillTracker>,
        attribute_name: &str,
    ) -> Response<Body> {
        match self.codex.meta_claim(attribute_name).await {
            Ok(result) => {
                if tracker.is_tracking() {
                    skill_tracker.suppress_next(attribute_name, SUPPRESS_TIMEOUT_SECONDS);
                }
                plain_json_response(&result)
            }
            Err(CodexError::Invalid(message)) => {
                error_response(StatusCode::BAD_REQUEST, &detail(&message))
            }
            Err(CodexError::Db(_)) => internal_error(),
        }
    }
}

/// A plain JSON 200: no conditional-GET headers. Write replies use
/// it everywhere; reads outside the ETag middleware's prefixes
/// (settings, character, equipment) use it too.
pub(crate) fn plain_json_response(payload: &Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(to_wire_json(payload)))
        .expect("write response builds")
}

fn quest_not_found() -> Response<Body> {
    error_response(StatusCode::NOT_FOUND, &detail("Quest not found"))
}

fn playlist_not_found() -> Response<Body> {
    error_response(StatusCode::NOT_FOUND, &detail("Playlist not found"))
}

fn session_not_found() -> Response<Body> {
    error_response(StatusCode::NOT_FOUND, &detail("Session not found"))
}

/// The quest router's error mapping: the quests router catches no
/// service error, so every failure surfaces as the backend's
/// unhandled-exception envelope.
pub fn quest_error_response(_error: QuestError) -> Response<Body> {
    internal_error()
}

// Expected values in these tests are the backend's own outputs: the
// ETag form and conditional semantics from its middleware, the
// formatter shapes from its routers, and the error envelopes from its
// HTTP layer (all held byte-for-byte by the A/B fidelity test; these
// hermetic pins keep the same surface guarded without a live backend).
#[cfg(test)]
mod tests {
    use super::*;
    use eo_services::clock::MockClock;
    use http_body_util::BodyExt;

    #[test]
    fn the_etag_is_the_quoted_body_hash() {
        assert_eq!(
            compute_strong_etag(b"hello"),
            "\"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\""
        );
    }

    #[test]
    fn if_none_match_parses_the_backend_way() {
        let current = "\"abc\"";
        assert!(!if_none_match_matches(None, current));
        assert!(if_none_match_matches(Some("*"), current));
        assert!(if_none_match_matches(Some("\"abc\""), current));
        assert!(if_none_match_matches(Some("\"x\", \"abc\""), current));
        assert!(if_none_match_matches(Some("W/\"abc\""), current));
        // Whitespace after the weak prefix is tolerated, as the
        // backend strips around its prefix removal.
        assert!(if_none_match_matches(Some("W/ \"abc\""), current));
        assert!(if_none_match_matches(Some("W/\t\"abc\""), current));
        assert!(if_none_match_matches(Some("\"x\", W/ \"abc\""), current));
        assert!(!if_none_match_matches(Some("\"nope\""), current));
    }

    #[test]
    fn scalar_helpers_match_the_router_layer() {
        assert_eq!(or_empty(&json!(null)), json!(""));
        assert_eq!(or_empty(&json!("")), json!(""));
        assert_eq!(or_empty(&json!("x")), json!("x"));
        assert_eq!(rounded(&json!(5), 2), json!(5));
        assert_eq!(rounded(&json!(1.2345), 2), json!(1.23));
        assert_eq!(rounded(&json!(2.675), 2), json!(2.67));
        assert_eq!(float_field(json!(0)), json!(0.0));
        assert_eq!(float_field(json!(1.5)), json!(1.5));
        assert_eq!(float_field(json!(null)), json!(null));
        assert_eq!(python_str_of(&json!("s")), "s");
        assert_eq!(python_str_of(&json!(42)), "42");
        assert_eq!(detail("gone"), json!({"detail": "gone"}));
    }

    async fn parts(response: Response<Body>) -> (StatusCode, http::HeaderMap, Vec<u8>) {
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, bytes)
    }

    #[tokio::test]
    async fn responses_carry_the_conditional_get_contract() {
        let payload = json!({"a": 1});
        let (status, headers, body) = parts(json_response(&payload, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"{\"a\":1}");
        let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
        assert_eq!(etag, compute_strong_etag(b"{\"a\":1}"));
        assert_eq!(headers.get("cache-control").unwrap(), "no-cache");
        assert_eq!(headers.get("content-type").unwrap(), "application/json");

        let (status, headers, body) = parts(json_response(&payload, Some(&etag))).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert!(body.is_empty());
        assert_eq!(headers.get("etag").unwrap().to_str().unwrap(), etag);
        assert_eq!(headers.get("cache-control").unwrap(), "no-cache");
        assert!(headers.get("content-type").is_none());

        let (status, _, body) = parts(error_response(
            StatusCode::NOT_FOUND,
            &detail("Species 'X' not found"),
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, b"{\"detail\":\"Species 'X' not found\"}");

        let (status, headers, body) = parts(internal_error()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            headers.get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(body, b"Internal Server Error");

        // The quest router catches no service error, so its mapping is
        // the same unhandled-exception envelope.
        let (status, _, body) =
            parts(quest_error_response(QuestError::Invalid("any".to_string()))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, b"Internal Server Error");
    }

    #[test]
    fn the_formatters_shape_the_router_wire() {
        let quest = json!({
            "id": 7, "name": "Iron", "category": null, "mobs": ["Atrox"],
            "planet": "Foma", "waypoint": null, "cooldown_hours": 24.0,
            "cooldown_expires_at": null, "reward_ped": 2.5, "reward_is_skill": 1,
            "expected_reward_markup_percent": null, "reward_description": null,
            "notes": "", "chain_name": null, "chain_position": null,
            "chain_total": null, "playlist_ids": [3], "started_at": null,
        });
        assert_eq!(
            format_quest(&quest),
            json!({
                "id": "7", "name": "Iron", "category": null, "targetMobs": ["Atrox"],
                "planet": "Foma", "waypoint": null, "cooldownDurationHours": 24.0,
                "cooldownExpiresAt": null, "reward": 2.5, "rewardIsSkill": true,
                "expectedRewardMarkupPercent": null, "rewardDescription": "",
                "notes": "", "chainName": null, "chainPosition": null,
                "chainTotal": null, "playlistIds": ["3"], "startedAt": null,
            })
        );

        let playlist = json!({
            "id": 3, "name": "Run", "planet": "Calypso", "estimated_minutes": 30,
            "quest_ids": [7], "immediate_quest_ids": [7], "long_horizon_quest_ids": [],
            "items": [{"quest_id": 7, "description": null, "group_type": "immediate"}],
        });
        assert_eq!(
            format_playlist(&playlist),
            json!({
                "id": "3", "name": "Run", "planet": "Calypso", "estimatedMinutes": 30,
                "questIds": ["7"], "immediateQuestIds": ["7"], "longHorizonQuestIds": [],
                "items": [{"questId": "7", "description": null, "groupType": "immediate"}],
            })
        );

        let row = json!({
            "quest_id": 7, "quest_name": "Iron", "planet": "Foma", "category": null,
            "reward_ped": 2.5, "reward_is_skill": false,
            "expected_reward_markup_percent": 150.0,
            "total_expected_reward_ped": 3.75, "linked_sessions": 1,
            "total_duration": 30.5, "weapon_cost": 0, "heal_cost": 0,
            "enhancer_cost": 0.1, "armour_cost": 0.0, "loot_tt": 0, "skill_tt": 0.2,
        });
        assert_eq!(
            format_quest_analytics(&row),
            json!({
                "questId": "7", "questName": "Iron", "planet": "Foma", "category": null,
                "rewardPed": 2.5, "rewardIsSkill": false,
                "expectedRewardMarkupPercent": 150.0, "totalExpectedRewardPed": 3.75,
                "linkedSessions": 1, "totalDurationSec": 30.5, "totalWeaponCost": 0.0,
                "totalHealCost": 0.0, "totalEnhancerCost": 0.1, "totalArmourCost": 0.0,
                "totalLootTt": 0.0, "totalPes": 0.2,
            })
        );

        let row = json!({
            "playlist_id": 3, "playlist_name": "Run", "quest_count": 1,
            "long_horizon_quest_count": 0, "matched_sessions": 0,
            "total_reward_ped": 0, "total_immediate_reward_ped": 0,
            "total_bonus_reward_ped": 0, "total_skill_reward_ped": 0,
            "total_immediate_skill_reward_ped": 0, "total_bonus_skill_reward_ped": 0,
            "total_expected_reward_ped": 0, "total_expected_immediate_reward_ped": 0,
            "total_expected_bonus_reward_ped": 0, "total_duration": 0,
            "weapon_cost": 0, "heal_cost": 0, "enhancer_cost": 0, "armour_cost": 0,
            "loot_tt": 0, "skill_tt": 0,
        });
        assert_eq!(
            format_playlist_analytics(&row),
            json!({
                "playlistId": "3", "playlistName": "Run", "questCount": 1,
                "longHorizonQuestCount": 0, "matchedSessions": 0,
                "totalRewardPed": 0.0, "totalImmediateRewardPed": 0.0,
                "totalBonusRewardPed": 0.0, "totalPesReward": 0.0,
                "totalImmediatePesReward": 0.0, "totalBonusPesReward": 0.0,
                "totalExpectedRewardPed": 0.0, "totalExpectedImmediateRewardPed": 0.0,
                "totalExpectedBonusRewardPed": 0.0, "totalDurationSec": 0.0,
                "totalWeaponCost": 0.0, "totalHealCost": 0.0, "totalEnhancerCost": 0.0,
                "totalArmourCost": 0.0, "totalLootTt": 0.0, "totalPes": 0.0,
            })
        );
    }

    async fn state(dir: &std::path::Path) -> HydrationState {
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(
            snapshot.join("mobs.json"),
            serde_json::to_string(&json!([
                {"name": "M", "species": {"name": "Boar", "codex_base_cost": 37.5, "codex_type": "Mob"}},
            ]))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(snapshot.join("professions.json"), "[]").unwrap();
        std::fs::write(snapshot.join("skills.json"), "[]").unwrap();
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        HydrationState::new(
            db,
            Arc::new(GameDataStore::new(&snapshot).unwrap()),
            Arc::new(MockClock::new(None, 0.0)),
            dir.to_path_buf(),
        )
    }

    #[tokio::test]
    async fn each_handler_answers_its_route() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(dir.path()).await;
        state
            .quests
            .create_quest(&json!({"name": "Iron", "mobs": ["Atrox"]}))
            .await
            .unwrap();
        state
            .quests
            .create_playlist(&json!({"name": "Run", "quest_ids": [1]}))
            .await
            .unwrap();

        for (label, response, marker) in [
            ("quests", state.list_quests(None).await, "\"Iron\""),
            ("mobs", state.list_mob_names(None).await, "\"Atrox\""),
            ("quest analytics", state.quest_analytics(None).await, "[]"),
            ("playlists", state.list_playlists(None).await, "\"Run\""),
            (
                "playlist analytics",
                state.playlist_analytics(None).await,
                "\"Run\"",
            ),
            ("species", state.codex_species(None).await, "\"Boar\""),
            (
                "ranks",
                state.codex_species_ranks("Boar", None).await,
                "\"speciesName\"",
            ),
            (
                "recommend",
                state
                    .codex_recommend("Boar", 4, None, "profession", None)
                    .await,
                "[",
            ),
            (
                "meta attributes",
                state.codex_meta_attributes(None).await,
                "\"Agility\"",
            ),
        ] {
            let (status, headers, body) = parts(response).await;
            assert_eq!(status, StatusCode::OK, "{label}");
            assert!(headers.contains_key("etag"), "{label}: etag present");
            assert!(
                String::from_utf8_lossy(&body).contains(marker),
                "{label}: body carries {marker}"
            );
        }

        let (status, _, body) = parts(state.codex_species_ranks("Nessie", None).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, b"{\"detail\":\"Species 'Nessie' not found\"}");

        for (rank, fragment) in [(0i64, "greater_than_equal"), (26, "less_than_equal")] {
            let (status, _, body) = parts(
                state
                    .codex_recommend("Boar", rank, None, "profession", None)
                    .await,
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(String::from_utf8_lossy(&body).contains(fragment));
        }
    }
}
